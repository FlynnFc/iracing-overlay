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
use super::protocol::{Envelope, Event, FromRelay, HandoverKey, Member, TyrePolicy};
use super::store::TeamState;
use crate::config::SyncConfig;
use crate::telemetry::pit::{Corner, PitRequest};
use crate::telemetry::snapshot::{Seat, TelemetrySnapshot};

/// A driver scalar normally arrives at most a second apart while any crew
/// member is connected. Keep a short outage grace period, then withhold the
/// live-only car gauges rather than presenting an old tank as current.
const SYNCED_CAR_STALE_AFTER_SECS: f64 = 5.0;

/// A one-off pit action is useful only while it is genuinely live. The app
/// gives an accepted write a further short local queue lifetime; reject a
/// frame already older than this on the shared session clock before it enters
/// that queue at all.
const LIVE_PIT_WRITE_MAX_AGE_SECS: f64 = 2.0;

/// Inputs of a connection attempt, retained after refusal to avoid retrying it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionIdentity {
    relay_url: String,
    invite: String,
    subsession: u64,
    session_num: i32,
    cust_id: u32,
    name: String,
}

/// Owns the connection, the producer and the consumer store.
#[derive(Debug, Default)]
pub struct TeamSync {
    client: Option<SyncClient>,
    source: EventSource,
    state: TeamState,
    /// Whether anyone else is in the room, from the last roster — the gate on
    /// the scalars tick (nothing to animate for an empty room).
    listeners: bool,
    /// The exact iRacing session phase the current client is joined to. A
    /// new `SessionNum` in the same subsession is still a new room because
    /// the phase clock restarts at zero.
    joined_session: Option<(u64, i32)>,
    connection_identity: Option<ConnectionIdentity>,
    refused_identity: Option<ConnectionIdentity>,
    /// This member's display name, for stamping the writes it sends.
    member_name: Option<String>,
    /// The latest `SessionTime` seen, stamped onto events this member sends
    /// outside the per-frame produce path (a spec's writes).
    last_session_time: f64,
    /// Session clock of the newest live driver-scalar measurement. The store
    /// retains older facts for history and restart recovery; this separate
    /// marker controls whether its fuel/armed-service readout is safe to show
    /// as *current*.
    last_driver_scalar_session_time: Option<f64>,
    /// Wall-clock arrival time of the newest scalar. A remote session clock
    /// alone cannot prove liveness after a stalled socket, so both clocks
    /// must remain fresh before the UI calls the values current.
    last_driver_scalar_received_at: Option<Instant>,
    /// Crew-chief pit writes received live, waiting for the driver's overlay
    /// to apply them — see [`TeamSync::take_pit_writes`]. Only live frames add
    /// here; a backlog never re-arms a stale write.
    pending_writes: Vec<(String, PitRequest)>,
    /// Whether this frame has a current driving snapshot. Replicated state is
    /// useful to every seat, while one-off pit commands are only safe here.
    accept_pit_writes: bool,
    /// Events this member published itself, waiting to be folded into its own
    /// store on the next frame — the relay never echoes a frame back to its
    /// sender, so without this a spec would set a fuel target or tyre policy
    /// and never see it on their own screen. Interior-mutable because
    /// publishing happens under the frame's shared borrows.
    self_published: std::sync::Mutex<Vec<Outgoing>>,
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
            self.refused_identity = None;
            return;
        }
        // A missing or non-driving snapshot is not merely an empty producer
        // tick. It means we no longer have authority to act on the car. Do
        // keep draining the replicated ledger below, but discard the local
        // edge detectors and transient commands so an old `Driving` frame
        // cannot publish a made-up lap or apply a pit change after telemetry
        // has gone stale (or after a driver swap).
        self.accept_pit_writes = snapshot.is_some_and(|snap| snap.seat == Seat::Driving);
        if let Some(snap) = snapshot {
            self.last_session_time = snap.session_time_secs;
        }
        if !self.accept_pit_writes {
            self.source = EventSource::default();
            self.was_on_pit_road = false;
            self.pending_writes.clear();
        }
        self.ensure_connected(config, snapshot);
        self.drain_incoming(now);
        self.fold_own_writes();
        self.apply_tyre_policy(snapshot);
        self.publish_if_driving(snapshot, now);
    }

    /// The team car's fuel picture for a spectator's Fuel page, or `None`
    /// before the ledger has carried a tank reading. See
    /// [`super::store::TeamState::synced_car`].
    #[must_use]
    pub fn synced_car(&self) -> Option<super::store::SyncedCar> {
        let measured_at = self.last_driver_scalar_session_time?;
        let received_at = self.last_driver_scalar_received_at?;
        if self.last_session_time >= measured_at && self.last_session_time - measured_at > SYNCED_CAR_STALE_AFTER_SECS {
            return None;
        }
        if Instant::now().saturating_duration_since(received_at).as_secs_f64() > SYNCED_CAR_STALE_AFTER_SECS {
            return None;
        }
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

    pub fn handover_ready(&self, key: HandoverKey) -> bool {
        self.state.handover_ready(key)
    }

    /// Readiness is a deliberate acknowledgement by the named driver, never by a spec on their behalf.
    pub fn can_mark_ready(&self, key: HandoverKey) -> bool {
        self.client.is_some() && self.connection_identity.as_ref().is_some_and(|id| id.cust_id == key.driver_id)
    }

    pub fn set_handover_ready(&self, key: HandoverKey, ready: bool) {
        if self.can_mark_ready(key) {
            self.publish(Event::HandoverReady { key, ready });
        }
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
            let producer = if let Event::HandoverReady { key, .. } = event { key.driver_id } else { 1 };
            self.state.apply(&Envelope { producer, seq, session_time: *session_time, event: event.clone() });
            if matches!(event, Event::DriverScalars { .. }) {
                self.last_driver_scalar_session_time = Some(*session_time);
                self.last_driver_scalar_received_at = Some(Instant::now());
            }
            self.last_session_time = self.last_session_time.max(*session_time);
        }
    }

    /// Keep the fixed demo measurement current while rendering, even on a slow first frame.
    /// Only demo mode calls this; live data keeps its five-second expiry.
    pub fn refresh_demo(&mut self, now: Instant) {
        if self.last_driver_scalar_session_time.is_some() {
            self.last_driver_scalar_received_at = Some(now);
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
                queue.push(Outgoing { session_time: self.last_session_time, event });
            }
        }
    }

    /// Folds this member's own recent publishes into its own store.
    fn fold_own_writes(&mut self) {
        let events = match self.self_published.get_mut() {
            Ok(queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        };
        for outgoing in events {
            // Synthetic identity for a local write; preserve the same clock
            // sent to the relay, even if the frame clock has since advanced.
            self.state.apply(&Envelope {
                producer: self.connection_identity.as_ref().map_or(0, |identity| identity.cust_id),
                seq: 0,
                session_time: outgoing.session_time,
                event: outgoing.event,
            });
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
            self.joined_session = None;
            self.connection_identity = None;
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
        self.ensure_connected_with(config, snapshot, SyncClient::start);
    }

    fn ensure_connected_with(
        &mut self,
        config: &SyncConfig,
        snapshot: Option<&TelemetrySnapshot>,
        start: impl FnOnce(String, u64, i32, String, Member) -> SyncClient,
    ) {
        let Some(identity) = snapshot.map(|snap| &snap.identity) else { return };
        let (Some(subsession), Some(session_num), Some(cust_id)) =
            (identity.subsession, identity.session_num, identity.player_cust_id)
        else {
            return;
        };
        if config.relay_url.trim().is_empty() {
            return;
        }
        let name = identity.player_name.as_deref().unwrap_or("driver").to_owned();
        let attempt = ConnectionIdentity {
            relay_url: config.relay_url.clone(),
            invite: config.invite.clone(),
            subsession,
            session_num,
            cust_id,
            name: name.clone(),
        };
        if self.refused_identity.as_ref() == Some(&attempt) {
            return;
        }
        // A settings edit during the same race must replace the live socket:
        // otherwise pasting a corrected relay URL or invite appears to work
        // in the UI while the client stays connected to the old room.
        if self.connection_identity.as_ref() == Some(&attempt) && self.client.is_some() {
            return;
        }
        if self.client.is_some() {
            self.disconnect();
        }
        self.refused_identity = None;
        let member = Member { cust_id, name: name.clone() };
        // A new subsession is a new race: the store, the producer's lap and
        // scalars memory, and any un-applied writes all belong to the old one
        // and start fresh — a stale `last_lap` from the previous session must
        // not swallow the new one's first crossing, and last session's scalars
        // must not suppress this session's opening tick.
        self.state = TeamState::default();
        self.source = EventSource::default();
        self.last_driver_scalar_session_time = None;
        self.last_driver_scalar_received_at = None;
        self.pending_writes.clear();
        if let Ok(mut queue) = self.self_published.lock() {
            queue.clear();
        }
        self.was_on_pit_road = false;
        self.listeners = false;
        self.member_name = Some(name);
        self.client = Some(start(config.relay_url.clone(), subsession, session_num, config.invite.clone(), member));
        self.connection_identity = Some(attempt);
        self.joined_session = Some((subsession, session_num));
    }

    /// Folds every waiting relay frame into the store and roster.
    fn drain_incoming(&mut self, now: Instant) {
        let Some(client) = &self.client else { return };
        // `try_iter` takes only what is queued now; the connection thread
        // keeps filling it, and the next frame takes the rest.
        let frames: Vec<FromRelay> = client.incoming.try_iter().collect();
        for frame in frames {
            match frame {
                FromRelay::Backlog(envelopes) => {
                    for envelope in &envelopes {
                        self.observe_driver_freshness(envelope, now);
                        self.state.apply(envelope);
                    }
                }
                FromRelay::Recovered(envelope) => {
                    // Recovered data repairs state after a relay restart. It
                    // is deliberately not a live relay frame: in particular
                    // a historical PitWrite cannot reach pending_writes.
                    self.observe_driver_freshness(&envelope, now);
                    self.state.apply(&envelope);
                }
                FromRelay::Relayed(envelope) => {
                    // A live pit write is queued for the driver's overlay to
                    // apply — never from a backlog, so a stale write can't
                    // re-arm the box on reconnect.
                    if self.is_actively_driving()
                        && self.live_pit_write_is_fresh(&envelope)
                        && let Event::PitWrite { requester, request } = &envelope.event
                    {
                        self.pending_writes.push((requester.clone(), *request));
                    }
                    self.observe_driver_freshness(&envelope, now);
                    self.state.apply(&envelope);
                }
                FromRelay::Welcome { members, .. } | FromRelay::Roster(members) => {
                    self.listeners = members.len() > 1;
                }
                FromRelay::CaughtUp => {}
                FromRelay::Refused { .. } => {
                    self.refused_identity = self.connection_identity.clone();
                    self.disconnect();
                    break;
                }
            }
        }
    }

    /// A pit write can only be acted on during a frame backed by a current
    /// driving snapshot. The app deliberately calls `update(None)` when its
    /// telemetry has aged out; that still consumes replicated state but makes
    /// one-off controls expire rather than wait for a later reconnect.
    fn is_actively_driving(&self) -> bool {
        self.accept_pit_writes
    }

    /// Uses the session clock rather than receipt time, so a websocket frame
    /// delayed in transit cannot become fresh merely by arriving now. A value
    /// slightly ahead of this snapshot is allowed: teammates' snapshots do
    /// not tick in lockstep, and phase isolation prevents an old phase from
    /// appearing as a future event.
    fn live_pit_write_is_fresh(&self, envelope: &Envelope) -> bool {
        envelope.session_time >= self.last_session_time
            || self.last_session_time - envelope.session_time <= LIVE_PIT_WRITE_MAX_AGE_SECS
    }

    /// Records only the driver's once-per-second heartbeat. A lap or tyre
    /// reading can be old for a whole stint, so neither is evidence that the
    /// tank is still live. `max` keeps a late/out-of-order backlog frame from
    /// making a newer reading look stale.
    fn observe_driver_freshness(&mut self, envelope: &Envelope, now: Instant) {
        if matches!(envelope.event, Event::DriverScalars { .. }) {
            let previous = self.last_driver_scalar_session_time.unwrap_or(f64::NEG_INFINITY);
            if envelope.session_time >= previous {
                self.last_driver_scalar_session_time = Some(envelope.session_time);
                self.last_driver_scalar_received_at = Some(now);
            }
        }
    }

    /// Produces and publishes the driver's events, if the player is driving.
    fn publish_if_driving(&mut self, snapshot: Option<&TelemetrySnapshot>, now: Instant) {
        let Some(snapshot) = snapshot.filter(|snap| snap.seat == Seat::Driving) else { return };

        // iRacing can expose the seat before its private fuel scalar. Never
        // turn that temporary absence into a believable zero-litre team
        // update; reset the edge source so the first valid sample becomes a
        // fresh baseline instead.
        if !snapshot.pit_service.fuel_reading_valid {
            self.source = EventSource::default();
            return;
        }
        let Some(client) = &self.client else { return };

        let service = &snapshot.pit_service;
        let observation = DriverObservation {
            session_time: snapshot.session_time_secs,
            lap: u16::try_from(snapshot.relative_meta.current_lap.max(0)).unwrap_or(u16::MAX),
            car_idx: snapshot.relative.get(snapshot.focus_index).map(|car| car.car_idx),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn fake_client() -> SyncClient {
        let (_, incoming) = mpsc::channel();
        let (publish, _) = mpsc::channel();
        SyncClient::from_test_channels(incoming, publish)
    }

    fn refused_runtime() -> (TeamSync, SyncConfig, TelemetrySnapshot) {
        let config = SyncConfig {
            enabled: true,
            relay_url: "ws://relay".to_owned(),
            invite: "wrong".to_owned(),
            ..SyncConfig::default()
        };
        let mut snapshot = crate::demo::snapshot();
        snapshot.identity.subsession = Some(123);
        snapshot.identity.session_num = Some(0);
        snapshot.identity.player_cust_id = Some(456);
        let mut sync = TeamSync::default();
        sync.ensure_connected_with(&config, Some(&snapshot), |_, _, _, _, _| fake_client());
        let (tx, incoming) = mpsc::channel();
        sync.client.as_mut().expect("connected").incoming = incoming;
        tx.send(FromRelay::Refused { reason: "bad invite".to_owned() }).expect("queue refusal");
        sync.drain_incoming(Instant::now());
        assert!(sync.client.is_none());
        (sync, config, snapshot)
    }

    #[test]
    fn refused_credentials_do_not_restart_each_frame() {
        let (mut sync, config, snapshot) = refused_runtime();
        for _ in 0..10 {
            sync.ensure_connected_with(&config, Some(&snapshot), |_, _, _, _, _| {
                panic!("refused credentials must not restart")
            });
        }
    }

    #[test]
    fn changed_connection_inputs_allow_retry_after_refusal() {
        for change in 0..4 {
            let (mut sync, mut config, mut snapshot) = refused_runtime();
            match change {
                0 => config.invite = "corrected".to_owned(),
                1 => config.relay_url = "ws://other-relay".to_owned(),
                2 => snapshot.identity.subsession = Some(124),
                _ => snapshot.identity.player_cust_id = Some(457),
            }
            sync.ensure_connected_with(&config, Some(&snapshot), |_, _, _, _, _| fake_client());
            assert!(sync.client.is_some(), "changed input {change} must allow retry");
        }
    }

    #[test]
    fn explicit_toggle_allows_retry_after_refusal() {
        let (mut sync, config, snapshot) = refused_runtime();
        let disabled = SyncConfig { enabled: false, ..config.clone() };
        sync.update(&disabled, Some(&snapshot), Instant::now());
        sync.ensure_connected_with(&config, Some(&snapshot), |_, _, _, _, _| fake_client());
        assert!(sync.client.is_some());
    }

    #[test]
    fn own_writes_keep_their_publish_clock() {
        let (publish, outgoing) = mpsc::channel();
        let (_, incoming) = mpsc::channel();
        let mut sync = TeamSync {
            client: Some(SyncClient::from_test_channels(incoming, publish)),
            last_session_time: 10.0,
            ..TeamSync::default()
        };
        sync.set_fuel_target(Some(2.0));
        assert!((outgoing.recv().expect("published").session_time - 10.0).abs() < f64::EPSILON);
        sync.last_session_time = 30.0;
        sync.fold_own_writes();
        sync.state.apply(&Envelope {
            producer: 1,
            seq: 1,
            session_time: 20.0,
            event: Event::FuelTarget { requester: "other".to_owned(), litres_per_lap: Some(3.0) },
        });
        assert!((sync.fuel_target().expect("other target") - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_missing_snapshot_keeps_replication_but_expires_driver_authority() {
        let (publish, published) = mpsc::channel();
        let (incoming_tx, incoming) = mpsc::channel();
        let mut sync = TeamSync {
            client: Some(SyncClient::from_test_channels(incoming, publish)),
            listeners: true,
            ..TeamSync::default()
        };
        let config = SyncConfig { enabled: true, relay_url: "ws://relay".to_owned(), ..SyncConfig::default() };
        let mut snap = crate::demo::snapshot();
        snap.seat = Seat::Driving;
        snap.relative_meta.current_lap = 5;
        snap.identity.subsession = Some(1);
        snap.identity.session_num = Some(0);
        snap.identity.player_cust_id = Some(7);
        sync.joined_session = Some((1, 0));
        sync.connection_identity = Some(ConnectionIdentity {
            relay_url: config.relay_url.clone(),
            invite: config.invite.clone(),
            subsession: 1,
            session_num: 0,
            cust_id: 7,
            name: "driver".to_owned(),
        });

        // Establish the source's lap baseline, then let telemetry age out.
        sync.update(&config, Some(&snap), Instant::now());
        while published.try_recv().is_ok() {}
        sync.update(&config, None, Instant::now());

        // A live control arriving while telemetry is stale is consumed from
        // the ledger but must not wait around to alter a later driver's box.
        incoming_tx
            .send(FromRelay::Relayed(Envelope {
                producer: 22,
                seq: 1,
                session_time: 20.0,
                event: Event::PitWrite { requester: "Spec".to_owned(), request: PitRequest::SetFuel(40) },
            }))
            .expect("queue live write");
        sync.update(&config, None, Instant::now());
        assert!(sync.take_pit_writes().is_empty(), "stale telemetry cannot defer a pit write");

        // Returning with a later lap establishes a new baseline instead of
        // treating all disconnected time as one made-up completed lap.
        snap.relative_meta.current_lap = 7;
        sync.update(&config, Some(&snap), Instant::now());
        assert!(
            published.try_iter().all(|outgoing| !matches!(outgoing.event, Event::LapClosed { .. })),
            "no lap spans stale telemetry"
        );
    }

    #[test]
    fn synced_car_is_hidden_when_either_liveness_clock_is_old() {
        let scalar = Event::DriverScalars {
            car_idx: Some(7),
            fuel_litres: 42.0,
            service_fuel_litres: Some(20),
            tyres_armed: [false; 4],
            tyre_pressures_kpa: [165.0; 4],
        };
        let mut sync = TeamSync::default();
        sync.state.apply(&Envelope { producer: 11, seq: 1, session_time: 100.0, event: scalar });
        sync.last_driver_scalar_session_time = Some(100.0);
        sync.last_driver_scalar_received_at = Some(Instant::now());
        sync.last_session_time = 100.0;
        assert!(sync.synced_car().is_some(), "a just-arrived scalar is live");

        sync.last_session_time = 106.0;
        assert!(sync.synced_car().is_none(), "a six-second session gap is stale");

        sync.last_session_time = 100.0;
        sync.last_driver_scalar_received_at = Instant::now().checked_sub(std::time::Duration::from_secs(6));
        assert!(sync.synced_car().is_none(), "a six-second receive gap is stale");
    }

    #[test]
    fn an_old_backlog_scalar_cannot_become_live_just_by_arriving_now() {
        let (incoming_tx, incoming) = mpsc::channel();
        let (publish, _published) = mpsc::channel();
        let mut sync = TeamSync {
            client: Some(SyncClient::from_test_channels(incoming, publish)),
            last_session_time: 100.0,
            ..TeamSync::default()
        };
        incoming_tx
            .send(FromRelay::Backlog(vec![Envelope {
                producer: 11,
                seq: 1,
                session_time: 90.0,
                event: Event::DriverScalars {
                    car_idx: Some(7),
                    fuel_litres: 42.0,
                    service_fuel_litres: None,
                    tyres_armed: [false; 4],
                    tyre_pressures_kpa: [165.0; 4],
                },
            }]))
            .expect("queue old history");
        sync.drain_incoming(Instant::now());
        assert!(sync.synced_car().is_none(), "receipt time must not revive a ten-second-old scalar");
    }

    #[test]
    fn recovered_pit_writes_rebuild_history_without_rearming_the_driver() {
        let (incoming_tx, incoming) = mpsc::channel();
        let (publish, _published) = mpsc::channel();
        let mut sync = TeamSync {
            client: Some(SyncClient::from_test_channels(incoming, publish)),
            accept_pit_writes: true,
            ..TeamSync::default()
        };
        incoming_tx
            .send(FromRelay::Recovered(Envelope {
                producer: 22,
                seq: 1,
                session_time: 10.0,
                event: Event::PitWrite { requester: "Spec".to_owned(), request: PitRequest::SetFuel(40) },
            }))
            .expect("queue recovery");
        sync.drain_incoming(Instant::now());
        assert!(sync.take_pit_writes().is_empty(), "recovery is history, never a live pit action");
    }

    #[test]
    fn a_network_delayed_live_pit_write_expires_on_the_session_clock() {
        let (incoming_tx, incoming) = mpsc::channel();
        let (publish, _published) = mpsc::channel();
        let mut sync = TeamSync {
            client: Some(SyncClient::from_test_channels(incoming, publish)),
            accept_pit_writes: true,
            last_session_time: 15.0,
            ..TeamSync::default()
        };
        incoming_tx
            .send(FromRelay::Relayed(Envelope {
                producer: 22,
                seq: 1,
                session_time: 10.0,
                event: Event::PitWrite { requester: "Spec".to_owned(), request: PitRequest::SetFuel(40) },
            }))
            .expect("queue delayed command");
        sync.drain_incoming(Instant::now());
        assert!(sync.take_pit_writes().is_empty(), "a five-second-old network frame must not arm the box");

        incoming_tx
            .send(FromRelay::Relayed(Envelope {
                producer: 22,
                seq: 2,
                session_time: 14.0,
                event: Event::PitWrite { requester: "Spec".to_owned(), request: PitRequest::SetFuel(40) },
            }))
            .expect("queue fresh command");
        sync.drain_incoming(Instant::now());
        assert_eq!(sync.take_pit_writes(), vec![("Spec".to_owned(), PitRequest::SetFuel(40))]);
    }

    #[test]
    fn an_unavailable_private_fuel_reading_never_publishes_a_fake_zero() {
        let (publish, published) = mpsc::channel();
        let (_, incoming) = mpsc::channel();
        let mut sync =
            TeamSync { client: Some(SyncClient::from_test_channels(incoming, publish)), ..TeamSync::default() };
        let mut snap = crate::demo::snapshot();
        snap.seat = Seat::Driving;
        snap.pit_service.fuel_level_litres = 0.0;
        snap.pit_service.fuel_reading_valid = false;
        sync.publish_if_driving(Some(&snap), Instant::now());
        assert!(published.try_recv().is_err(), "missing fuel must be silent, not a zero-litre scalar");

        snap.pit_service.fuel_reading_valid = true;
        snap.pit_service.fuel_level_litres = 42.0;
        sync.publish_if_driving(Some(&snap), Instant::now());
        let emitted = published.recv().expect("the first valid reading publishes");
        assert!(!matches!(emitted.event, Event::DriverScalars { fuel_litres, .. } if fuel_litres == 0.0));
    }
}
