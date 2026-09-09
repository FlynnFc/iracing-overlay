// Rust guideline compliant 2026-02-16

//! Team sync: one race's measurements shared across a team's overlays.
//!
//! See `plans/team-sync.md` for the full design. In short: members of the
//! same iRacing subsession exchange small, `SessionTime`-stamped events
//! through a relay embedded in the hosting overlay, every party keeps the
//! same append-only [`ledger::Ledger`], and a member who joins late or drops
//! is caught up from it — so a rebuilt overlay is bit-identical to one that
//! never disconnected.
//!
//! This module is the transport and the log. What the events *mean* — which
//! telemetry produces them and which widgets consume them — is phase 2 and
//! lives with the telemetry, not here.

pub mod client;
pub mod feed;
pub mod ledger;
pub mod protocol;
pub mod relay;
pub mod runtime;
pub mod store;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::client::{Outgoing, SyncClient};
    use super::protocol::{Event, FromRelay, Member};
    use super::relay::{InviteCode, Relay};

    /// Generous next to the pump intervals; a healthy loopback round trip is
    /// milliseconds, so hitting this means something actually broke.
    const WAIT: Duration = Duration::from_secs(10);

    fn member(cust_id: u32, name: &str) -> Member {
        Member { cust_id, name: name.to_owned() }
    }

    fn lap(n: u16) -> Event {
        Event::LapClosed { lap: n, fuel_litres: 60.0 - f32::from(n), used_litres: 1.0 }
    }

    /// Pulls frames until `accept` says done, or the wait expires.
    fn wait_for<T>(client: &SyncClient, mut accept: impl FnMut(&FromRelay) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        while let Ok(frame) = client.incoming.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if let Some(found) = accept(&frame) {
                return found;
            }
        }
        panic!("the expected frame never arrived");
    }

    /// The whole phase-1 story over loopback: join, publish, relay, a late
    /// joiner catching up from the ledger, and a producer resuming its own
    /// numbering — one test because it is one lifecycle, and the relay port
    /// is the only piece of shared state worth setting up.
    #[test]
    fn events_relay_live_and_replay_to_late_joiners() {
        let invite = InviteCode::generate().expect("the OS has entropy");
        let relay = Relay::spawn(0, invite.clone()).expect("an ephemeral port must bind");
        let url = format!("ws://{}", relay.local_addr());
        let subsession = 987_654;

        let driver = SyncClient::start(url.clone(), subsession, invite.to_string(), member(11, "Driver"));
        wait_for(&driver, |frame| matches!(frame, FromRelay::Welcome { .. }).then_some(()));
        for n in 1..=3 {
            driver.publish.send(Outgoing { session_time: f64::from(n) * 90.0, event: lap(n) }).expect("thread alive");
        }

        // A spectator connected the whole time sees the laps live.
        let spec = SyncClient::start(url.clone(), subsession, invite.to_string(), member(22, "Spec"));
        let mut live = Vec::new();
        wait_for(&spec, |frame| {
            match frame {
                FromRelay::Backlog(envelopes) => live.extend(envelopes.iter().map(|held| held.seq)),
                FromRelay::Relayed(envelope) => live.push(envelope.seq),
                _ => {}
            }
            (live.len() >= 3).then_some(())
        });
        assert_eq!(live, vec![1, 2, 3], "backlog and live delivery together carry every lap in order");

        // A crew chief joining only now gets the same three laps from the
        // ledger — the DC-recovery path is the same code as the late join.
        let late = SyncClient::start(url.clone(), subsession, invite.to_string(), member(33, "Late"));
        let backlog = wait_for(&late, |frame| match frame {
            FromRelay::Backlog(envelopes) if !envelopes.is_empty() => {
                Some(envelopes.iter().map(|held| (held.producer, held.seq)).collect::<Vec<_>>())
            }
            _ => None,
        });
        assert_eq!(backlog, vec![(11, 1), (11, 2), (11, 3)]);

        // The driver's overlay restarts (fresh replica, empty `have`) and
        // publishes again: the relay's tips must push its numbering past the
        // laps the first run published, not fork it back to seq 1.
        drop(driver);
        let reborn = SyncClient::start(url, subsession, invite.to_string(), member(11, "Driver"));
        wait_for(&reborn, |frame| matches!(frame, FromRelay::Welcome { .. }).then_some(()));
        reborn.publish.send(Outgoing { session_time: 400.0, event: lap(4) }).expect("thread alive");
        let seq = wait_for(&late, |frame| match frame {
            FromRelay::Relayed(envelope) if envelope.producer == 11 => Some(envelope.seq),
            _ => None,
        });
        assert_eq!(seq, 4, "a restarted producer continues its sequence from the relay's tip");
    }

    /// A wrong code gets a `Refused` and no data; the client stops retrying.
    #[test]
    fn a_wrong_invite_is_refused() {
        let invite = InviteCode::generate().expect("the OS has entropy");
        let relay = Relay::spawn(0, invite).expect("an ephemeral port must bind");
        let url = format!("ws://{}", relay.local_addr());

        let intruder = SyncClient::start(url, 1, "WRONG-CODE".to_owned(), member(99, "Intruder"));
        wait_for(&intruder, |frame| matches!(frame, FromRelay::Refused { .. }).then_some(()));
        assert!(
            intruder.incoming.recv_timeout(Duration::from_millis(300)).is_err(),
            "nothing follows a refusal; the thread has stopped"
        );
    }

    /// The producer and the consumer meet: what [`feed::EventSource`] emits
    /// from a driver's frames, folded straight into a [`store::TeamState`],
    /// must reconstruct the same fuel and tyre picture — with no network in
    /// between, so the data model is tested apart from the transport.
    #[test]
    fn a_drivers_frames_reconstruct_the_same_car_in_a_spectators_store() {
        use super::feed::{DriverObservation, EventSource};
        use super::protocol::Envelope;
        use super::store::TeamState;
        use crate::telemetry::snapshot::{TyreInfo, TyreState};

        let mut source = EventSource::default();
        let mut store = TeamState::default();
        let mut seq = 0_u32;
        // Fold whatever the source emits for one frame into the store.
        let mut carry = |source: &mut EventSource, store: &mut TeamState, obs: DriverObservation, at: Instant| {
            for event in source.observe(obs, at, true) {
                seq += 1;
                store.apply(&Envelope { producer: 11, seq, session_time: obs.session_time, event });
            }
        };

        let worn = TyreInfo { corners: [TyreState { temps_c: [85.0; 3], wear: [0.82; 3], pressure_kpa: 172.0 }; 4] };
        let base = DriverObservation {
            session_time: 0.0,
            lap: 10,
            fuel_litres: 55.0,
            fuel_per_lap_litres: Some(2.4),
            service_fuel_litres: Some(30),
            tyres_armed: [true, true, false, false],
            tyre_pressures_kpa: [165.0, 166.0, 167.0, 168.0],
            tyres: worn,
            on_pit_road: false,
        };

        let start = Instant::now();
        // The opening frame, then a lap closes with fresh fuel a heartbeat on.
        carry(&mut source, &mut store, base, start);
        let mut next_lap = base;
        next_lap.session_time = 90.0;
        next_lap.lap = 11;
        next_lap.fuel_litres = 52.6;
        carry(&mut source, &mut store, next_lap, start + Duration::from_secs(2));

        let car = store.synced_car().expect("the store rebuilt the car");
        assert!((car.fuel_litres - 52.6).abs() < 0.01, "the latest tank");
        assert!((car.burn_per_lap.expect("a lap closed") - 2.4).abs() < 0.01, "the measured burn");
        assert_eq!(car.service_fuel_litres, Some(30));
        assert_eq!(car.tyres_armed, [true, true, false, false]);
        let tyres = car.tyres.expect("tyre life crossed over");
        assert!((tyres.corners[0].wear[0] - 0.82).abs() < 0.001);
    }

    /// A snapshot with just the identity and seat team sync reads.
    fn snapshot_for(
        subsession: u64,
        cust_id: u32,
        seat: crate::telemetry::snapshot::Seat,
    ) -> crate::telemetry::snapshot::TelemetrySnapshot {
        let mut snapshot = crate::demo::snapshot();
        snapshot.identity = crate::telemetry::snapshot::SessionIdentity {
            subsession: Some(subsession),
            player_cust_id: Some(cust_id),
            player_name: Some(std::sync::Arc::from("Member")),
        };
        snapshot.seat = seat;
        snapshot
    }

    /// The standing tyre directive end to end: a spec sets it once, and the
    /// driver's overlay turns it into an arm/disarm write at each pit entry —
    /// at the *edge*, once per entry, never while merely sitting on pit road.
    /// Unlike a pit write this is session state, so it reaches the driver
    /// whether it arrived live or from a backlog.
    #[test]
    fn a_standing_tyre_policy_writes_the_tyre_call_at_each_pit_entry() {
        use super::protocol::TyrePolicy;
        use super::runtime::TeamSync;
        use crate::config::SyncConfig;
        use crate::telemetry::pit::PitRequest;
        use crate::telemetry::snapshot::Seat;

        let invite = InviteCode::generate().expect("entropy");
        let relay = Relay::spawn(0, invite.clone()).expect("bind");
        let url = format!("ws://{}", relay.local_addr());
        let subsession = 555_777;
        let config = SyncConfig {
            enabled: true,
            relay_url: url.clone(),
            invite: invite.to_string(),
            allow_team_pit_control: true,
            ..SyncConfig::default()
        };

        // The spec's directive goes up first; the driver joins after, so the
        // policy reaches them from the *backlog* — the driver-swap case.
        let spec = SyncClient::start(url, subsession, invite.to_string(), member(22, "Spec"));
        wait_for(&spec, |frame| matches!(frame, FromRelay::Welcome { .. }).then_some(()));
        spec.publish
            .send(Outgoing {
                session_time: 5.0,
                event: Event::TyrePolicySet { requester: "Spec".to_owned(), policy: Some(TyrePolicy::Never) },
            })
            .expect("the spec thread is alive");

        let mut driver = TeamSync::default();
        let mut snap = snapshot_for(subsession, 11, Seat::Driving);
        snap.pit_service.on_pit_road = false;
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline && driver.tyre_policy().is_none() {
            driver.update(&config, Some(&snap), Instant::now());
            assert!(driver.take_pit_writes().is_empty(), "no write before any pit entry");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(driver.tyre_policy().map(|(policy, _)| policy), Some(TyrePolicy::Never), "the backlog carried it");

        // Entering pit road fires the call once, attributed to its setter.
        snap.pit_service.on_pit_road = true;
        driver.update(&config, Some(&snap), Instant::now());
        assert_eq!(
            driver.take_pit_writes(),
            vec![("Spec".to_owned(), PitRequest::SetAllTyres(false))],
            "a Never policy disarms the tyres at the entry edge"
        );

        // Still on pit road: an edge, not a level — nothing fires again.
        driver.update(&config, Some(&snap), Instant::now());
        assert!(driver.take_pit_writes().is_empty(), "sitting on pit road must not repeat the write");

        // Out and back in: the next stop gets its own call.
        snap.pit_service.on_pit_road = false;
        driver.update(&config, Some(&snap), Instant::now());
        snap.pit_service.on_pit_road = true;
        driver.update(&config, Some(&snap), Instant::now());
        assert_eq!(driver.take_pit_writes().len(), 1, "each entry decides afresh");
    }

    /// The safety property the whole crew-chief write path turns on: a pit
    /// write applies **once, live**, and a reconnecting or late-joining driver
    /// never has it sprung on them from a backlog. Getting this wrong arms the
    /// pit box on a stale command mid-race, so it is tested end to end.
    #[test]
    fn a_live_pit_write_reaches_the_driver_but_a_replayed_one_never_re_arms() {
        use super::runtime::TeamSync;
        use crate::config::SyncConfig;
        use crate::telemetry::pit::PitRequest;
        use crate::telemetry::snapshot::Seat;

        let invite = InviteCode::generate().expect("entropy");
        let relay = Relay::spawn(0, invite.clone()).expect("bind");
        let url = format!("ws://{}", relay.local_addr());
        let subsession = 424_242;
        let config = SyncConfig {
            enabled: true,
            relay_url: url.clone(),
            invite: invite.to_string(),
            allow_team_pit_control: true,
            ..SyncConfig::default()
        };

        // The spectator connects first and watches the roster, so we can tell
        // exactly when the driver is on — the write must be sent *after* that,
        // or it would arrive as backlog and the "live" half proves nothing.
        let spec = SyncClient::start(url.clone(), subsession, invite.to_string(), member(22, "Spec"));
        wait_for(&spec, |frame| matches!(frame, FromRelay::Welcome { .. }).then_some(()));

        // Drive the driver's TeamSync until the spectator sees it join.
        let mut driver = TeamSync::default();
        let driver_snap = snapshot_for(subsession, 11, Seat::Driving);
        let deadline = Instant::now() + WAIT;
        let mut driver_seen = false;
        while Instant::now() < deadline && !driver_seen {
            driver.update(&config, Some(&driver_snap), Instant::now());
            while let Ok(frame) = spec.incoming.try_recv() {
                if let FromRelay::Roster(members) | FromRelay::Welcome { members, .. } = frame
                    && members.iter().any(|m| m.cust_id == 11)
                {
                    driver_seen = true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(driver_seen, "the driver never connected");

        // Now the spectator sends a pit write. The driver, already connected,
        // must receive it live.
        spec.publish
            .send(Outgoing {
                session_time: 5.0,
                event: Event::PitWrite { requester: "Spec".to_owned(), request: PitRequest::SetFuel(48) },
            })
            .expect("the spec thread is alive");

        let mut live = Vec::new();
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline && live.is_empty() {
            driver.update(&config, Some(&driver_snap), Instant::now());
            live = driver.take_pit_writes();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(live.len(), 1, "the live write reaches the seated driver");
        assert_eq!(live[0], ("Spec".to_owned(), PitRequest::SetFuel(48)));

        // A driver joining *now* gets the write in its backlog — but it must
        // never be queued for application. Loop until the backlog has plainly
        // arrived (the driver's synced fuel is known), asserting throughout
        // that no write is ever queued: this is the anti-re-arm guard.
        let mut late = TeamSync::default();
        let late_snap = snapshot_for(subsession, 33, Seat::Driving);
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline && late.synced_car().is_none() {
            late.update(&config, Some(&late_snap), Instant::now());
            assert!(late.take_pit_writes().is_empty(), "a backlogged pit write must never re-arm");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(late.synced_car().is_some(), "the late joiner did receive the ledger backlog");
        assert!(late.take_pit_writes().is_empty(), "and still nothing to apply from the replay");
    }
}
