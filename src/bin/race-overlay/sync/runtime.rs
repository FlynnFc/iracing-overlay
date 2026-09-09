// Rust guideline compliant 2026-02-16

//! The app's team-sync runtime: connect, produce, consume, once per frame.
//!
//! Bundles the three pieces the overlay owns — the [`SyncClient`] connection,
//! the [`EventSource`] that turns the driver's telemetry into events, and the
//! [`TeamState`] a spectator's pages read — behind one [`TeamSync::update`]
//! call the frame loop makes with the latest snapshot. Everything network
//! lives on the client's own thread; this only moves already-decoded frames
//! between channels and the store, so it adds nothing to the frame's cost
//! beyond draining a queue.
//!
//! Inert unless `sync.enabled` and the session has published its identity:
//! an overlay with no team, or one still loading in, never opens a socket.

use std::time::Instant;

use super::client::{Outgoing, SyncClient};
use super::feed::{DriverObservation, EventSource};
use super::protocol::{Envelope, Event, FromRelay, Member, TyrePolicy};
use super::store::TeamState;
use crate::config::SyncConfig;
use crate::telemetry::pit::{Corner, PitRequest};
use crate::telemetry::snapshot::{Seat, TelemetrySnapshot};

/// Owns the connection, the producer and the consumer store.
#[derive(Debug, Default)]
pub struct TeamSync {
    client: Option<SyncClient>,
    source: EventSource,
    state: TeamState,
    /// Whether anyone else is in the room, from the last roster — the gate on
    /// the scalars tick (nothing to animate for an empty room).
    listeners: bool,
    /// The subsession the current client is joined to, so a new session
    /// (`SubSessionID` changes) tears the old connection down and starts fresh.
    joined_subsession: Option<u64>,
    /// This member's display name, for stamping the writes it sends.
    member_name: Option<String>,
    /// The latest `SessionTime` seen, stamped onto events this member sends
    /// outside the per-frame produce path (a spec's writes).
    last_session_time: f64,
    /// Crew-chief pit writes received live, waiting for the driver's overlay
    /// to apply them — see [`TeamSync::take_pit_writes`]. Only live frames add
    /// here; a backlog never re-arms a stale write.
    pending_writes: Vec<(String, PitRequest)>,
    /// Events this member published itself, waiting to be folded into its own
    /// store on the next frame — the relay never echoes a frame back to its
    /// sender, so without this a spec would set a fuel target or tyre policy
    /// and never see it on their own screen. Interior-mutable because
    /// publishing happens under the frame's shared borrows.
    self_published: std::sync::Mutex<Vec<Event>>,
    /// Whether the driver was on pit road last frame — the entry edge the
    /// standing tyre policy fires its arm/disarm write on.
    was_on_pit_road: bool,
}

impl TeamSync {
    /// Runs one frame of sync: (re)connect if needed, drain incoming into the
    /// store, and publish the driver's events.
    ///
    /// Safe to call every frame with no snapshot and sync disabled — it does
    /// nothing until there is a session to join and a config that says to.
    pub fn update(&mut self, config: &SyncConfig, snapshot: Option<&TelemetrySnapshot>, now: Instant) {
        if !config.enabled {
            self.disconnect();
            return;
        }
        self.ensure_connected(config, snapshot);
        self.drain_incoming();
        self.fold_own_writes();
        self.apply_tyre_policy(snapshot);
        self.publish_if_driving(snapshot, now);
    }

    /// The team car's fuel picture for a spectator's Fuel page, or `None`
    /// before the ledger has carried a tank reading. See
    /// [`super::store::TeamState::synced_car`].
    #[must_use]
    pub fn synced_car(&self) -> Option<super::store::SyncedCar> {
        self.state.synced_car()
    }

    /// The shared fuel-per-lap target the driver's header chases, if set.
    #[must_use]
    pub fn fuel_target(&self) -> Option<f32> {
        self.state.fuel_target()
    }

    /// Sets or clears the shared fuel-per-lap target (`None` clears).
    pub fn set_fuel_target(&self, litres_per_lap: Option<f32>) {
        self.publish(Event::FuelTarget { requester: self.requester(), litres_per_lap });
    }

    /// Folds a canned set of events straight into the store, for `--demo`.
    ///
    /// Demo mode has no relay and no team, so every sync-fed surface — the
    /// spectator's Fuel and Tyres pages, the shared fuel target, the standing
    /// tyre directive — would otherwise be invisible to a screenshot. This is
    /// the one way in that skips the network; the events themselves go through
    /// the same [`TeamState::apply`] fold as a real backlog, so what is drawn
    /// is what a real ledger would have produced. See `demo::sync_events`.
    pub fn demo_seed(&mut self, events: &[(f64, Event)]) {
        for (seq, (session_time, event)) in events.iter().enumerate() {
            let seq = u32::try_from(seq).unwrap_or(u32::MAX).saturating_add(1);
            self.state.apply(&Envelope { producer: 1, seq, session_time: *session_time, event: event.clone() });
        }
    }

    /// The standing tyre directive and who set it, if one stands.
    #[must_use]
    pub fn tyre_policy(&self) -> Option<(TyrePolicy, &str)> {
        self.state.tyre_policy()
    }

    /// Sets or clears the standing tyre directive (`None` clears).
    pub fn set_tyre_policy(&self, policy: Option<TyrePolicy>) {
        self.publish(Event::TyrePolicySet { requester: self.requester(), policy });
    }

    /// Takes the crew-chief pit writes received since the last call, for the
    /// driver's overlay to apply. Draining, so each write is applied once.
    #[must_use]
    pub fn take_pit_writes(&mut self) -> Vec<(String, PitRequest)> {
        std::mem::take(&mut self.pending_writes)
    }

    /// Sends a crew-chief pit adjustment to the seated driver's overlay.
    ///
    /// Publishes a [`Event::PitWrite`]; the driver's side applies it behind
    /// the consent gate. No-op if not connected.
    pub fn send_pit_write(&self, request: PitRequest) {
        self.publish(Event::PitWrite { requester: self.requester(), request });
    }

    /// This member's name for a "set by" note, or a plain default.
    fn requester(&self) -> String {
        self.member_name.clone().unwrap_or_else(|| "crew".to_owned())
    }

    /// Publishes one event on the current connection, if any.
    ///
    /// Also queues it for this member's own store: the relay fans a frame out
    /// to everyone *except* its sender, so session state this member sets
    /// (fuel target, tyre policy) has to be folded in locally or it would be
    /// on every screen but the one that set it.
    fn publish(&self, event: Event) {
        if let Some(client) = &self.client {
            let _ = client.publish.send(Outgoing { session_time: self.last_session_time, event: event.clone() });
            if let Ok(mut queue) = self.self_published.lock() {
                queue.push(event);
            }
        }
    }

    /// Folds this member's own recent publishes into its own store.
    fn fold_own_writes(&mut self) {
        let events = match self.self_published.get_mut() {
            Ok(queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        };
        for event in events {
            // Producer and seq are wire concerns the store never reads; the
            // clock is this member's latest, which is when the write happened.
            self.state.apply(&Envelope { producer: 0, seq: 0, session_time: self.last_session_time, event });
        }
    }

    /// Applies the standing tyre policy as the driver enters pit road.
    ///
    /// Fires once per entry, queuing an arm/disarm-all-tyres write attributed
    /// to whoever set the policy — it goes through [`TeamSync::take_pit_writes`]
    /// like any crew write, so the driver's consent gate and "set by" note
    /// apply unchanged. Decided at the entry edge, against the latest latched
    /// wear, so a policy set mid-stint still lands on the very next stop.
    fn apply_tyre_policy(&mut self, snapshot: Option<&TelemetrySnapshot>) {
        let Some(snap) = snapshot else { return };
        let on_pit_road = snap.pit_service.on_pit_road;
        let entering = on_pit_road && !self.was_on_pit_road;
        self.was_on_pit_road = on_pit_road;
        if !entering || snap.seat != Seat::Driving {
            return;
        }
        let Some((policy, requester)) = self.state.tyre_policy() else { return };
        let take = policy.tyres_this_stop(snap.tyres.lowest_remaining_pct());
        let requester = requester.to_owned();
        self.pending_writes.push((requester, PitRequest::SetAllTyres(take)));
    }

    /// Drops any connection — sync turned off, or the app shutting a session.
    fn disconnect(&mut self) {
        if self.client.take().is_some() {
            // The store is kept: turning sync off and on within a session
            // should not forget the ledger already gathered.
            self.listeners = false;
            self.joined_subsession = None;
            // Un-applied writes are dropped rather than carried across a
            // reconnect — a stale pit adjustment is not one to spring later.
            self.pending_writes.clear();
            if let Ok(mut queue) = self.self_published.lock() {
                queue.clear();
            }
        }
    }

    /// Opens a connection once the identity is known, or replaces one whose
    /// session has changed underneath it.
    fn ensure_connected(&mut self, config: &SyncConfig, snapshot: Option<&TelemetrySnapshot>) {
        let Some(identity) = snapshot.map(|snap| &snap.identity) else { return };
        let (Some(subsession), Some(cust_id)) = (identity.subsession, identity.player_cust_id) else {
            return;
        };
        if self.joined_subsession == Some(subsession) && self.client.is_some() {
            return;
        }
        if config.relay_url.trim().is_empty() {
            return;
        }
        let name = identity.player_name.as_deref().unwrap_or("driver").to_owned();
        let member = Member { cust_id, name: name.clone() };
        // A new subsession is a new race: the store, the producer's lap and
        // scalars memory, and any un-applied writes all belong to the old one
        // and start fresh — a stale `last_lap` from the previous session must
        // not swallow the new one's first crossing, and last session's scalars
        // must not suppress this session's opening tick.
        self.state = TeamState::default();
        self.source = EventSource::default();
        self.pending_writes.clear();
        if let Ok(mut queue) = self.self_published.lock() {
            queue.clear();
        }
        self.was_on_pit_road = false;
        self.listeners = false;
        self.member_name = Some(name);
        self.client = Some(SyncClient::start(config.relay_url.clone(), subsession, config.invite.clone(), member));
        self.joined_subsession = Some(subsession);
    }

    /// Folds every waiting relay frame into the store and roster.
    fn drain_incoming(&mut self) {
        let Some(client) = &self.client else { return };
        // `try_iter` takes only what is queued now; the connection thread
        // keeps filling it, and the next frame takes the rest.
        let frames: Vec<FromRelay> = client.incoming.try_iter().collect();
        for frame in frames {
            match frame {
                FromRelay::Backlog(envelopes) => {
                    for envelope in &envelopes {
                        self.state.apply(envelope);
                    }
                }
                FromRelay::Relayed(envelope) => {
                    // A live pit write is queued for the driver's overlay to
                    // apply — never from a backlog, so a stale write can't
                    // re-arm the box on reconnect.
                    if let Event::PitWrite { requester, request } = &envelope.event {
                        self.pending_writes.push((requester.clone(), *request));
                    }
                    self.state.apply(&envelope);
                }
                FromRelay::Welcome { members, .. } | FromRelay::Roster(members) => {
                    self.listeners = members.len() > 1;
                }
                FromRelay::Refused { .. } => self.disconnect(),
            }
        }
    }

    /// Produces and publishes the driver's events, if the player is driving.
    fn publish_if_driving(&mut self, snapshot: Option<&TelemetrySnapshot>, now: Instant) {
        if let Some(snap) = snapshot {
            // Tracked whatever the seat, so a spectator's writes are stamped
            // with a live session clock too.
            self.last_session_time = snap.session_time_secs;
        }
        let Some(client) = &self.client else { return };
        let Some(snapshot) = snapshot.filter(|snap| snap.seat == Seat::Driving) else { return };

        let service = &snapshot.pit_service;
        let observation = DriverObservation {
            session_time: snapshot.session_time_secs,
            lap: u16::try_from(snapshot.relative_meta.current_lap.max(0)).unwrap_or(u16::MAX),
            fuel_litres: service.fuel_level_litres,
            fuel_per_lap_litres: service.fuel_per_lap_litres,
            service_fuel_litres: service.fuel_armed.then(|| clamp_litres(service.fuel_amount_litres)),
            tyres_armed: [
                service.tyre_armed(Corner::LeftFront),
                service.tyre_armed(Corner::RightFront),
                service.tyre_armed(Corner::LeftRear),
                service.tyre_armed(Corner::RightRear),
            ],
            tyre_pressures_kpa: service.tyre_pressures_kpa,
            tyres: snapshot.tyres,
            on_pit_road: service.on_pit_road,
        };
        for event in self.source.observe(observation, now, self.listeners) {
            // A full outbox means the connection thread has died; the next
            // `ensure_connected` after the client drops will rebuild it.
            let _ = client.publish.send(Outgoing { session_time: observation.session_time, event });
        }
    }
}

/// Rounds an armed fuel load to whole litres for the wire, clamped into
/// `i16`. A pit fuel load is at most a few hundred litres, so the clamp only
/// ever guards against a garbage reading, not a real value.
fn clamp_litres(litres: f32) -> i16 {
    let rounded = litres.round();
    if rounded >= f32::from(i16::MAX) {
        i16::MAX
    } else if rounded <= 0.0 {
        0
    } else {
        // In range and positive: the cast is exact for a value this small.
        #[expect(clippy::cast_possible_truncation, reason = "guarded above into i16's positive range")]
        let litres = rounded as i16;
        litres
    }
}
