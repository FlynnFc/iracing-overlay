// Rust guideline compliant 2026-02-16

//! What a spectator's overlay knows about the team's car — the consumer side.
//!
//! Every member folds the ledger's events into this, so a watching seat can
//! render the Fuel and Tyres pages from the driver's real numbers instead of
//! the empty tank its own sim publishes for a car it isn't driving. Fed from
//! both the backlog (on join or reconnect) and live frames, through the one
//! [`TeamState::apply`] path — so a rebuilt state is identical to one that
//! never dropped.
//!
//! Deliberately a plain fold with no time logic: events carry their own
//! `session_time`, and the ledger has already ordered and de-duplicated them
//! by the time they arrive here. This just remembers the latest of each.

use super::protocol::{Envelope, Event, TyrePolicy};
use crate::telemetry::snapshot::TyreInfo;

/// How many recent laps of fuel history to keep, for the trend readouts the
/// spec-mode Strategy page draws. Enough for a ~10-lap burn sparkline with
/// room to spare; a long stint past this drops its oldest laps.
const LAP_HISTORY: usize = 32;

/// Laps averaged for the burn a spectator's Fuel page shows — the same short
/// window Auto Fuel uses, so the crew's figure matches the driver's.
const BURN_WINDOW_LAPS: usize = 5;

/// One completed lap's fuel record, oldest-first in [`TeamState::laps`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LapFuel {
    pub lap: u16,
    pub fuel_after_litres: f32,
    pub used_litres: f32,
}

/// The team car's fuel picture, ready for a spectator's Fuel page.
///
/// Extracted from [`TeamState`] once per frame while watching, so the page
/// reads a plain struct rather than the ledger — see
/// [`TeamState::synced_car`]. `None`-ing the whole thing (no tank reading yet)
/// is what keeps the page from unlocking on an empty session.
#[derive(Debug, Clone)]
pub struct SyncedCar {
    /// Who is in the car, for the page's "via <driver>" tag.
    pub driver: Option<String>,
    /// The latest tank, in litres.
    pub fuel_litres: f32,
    /// The shared-ledger rolling burn, or `None` before a lap has closed.
    pub burn_per_lap: Option<f32>,
    /// Litres armed to add at the next stop, `None` when fuelling is off.
    pub service_fuel_litres: Option<i16>,
    /// Which corners are armed, in `pit::corner_index` order.
    pub tyres_armed: [bool; 4],
    /// The armed cold pressures per corner, kPa — the base a spectator's
    /// pressure control steps from.
    pub tyre_pressures_kpa: [f32; 4],
    /// The latched last-stop tyre life, `None` before any stop has been seen.
    pub tyres: Option<TyreInfo>,
}

/// The team car's state as reconstructed from the ledger.
///
/// Every field is `Option`/empty until an event has set it: a spectator who
/// just joined an in-progress race knows only what the backlog carried, which
/// is exactly right — a made-up zero would render as a real reading.
#[derive(Debug, Default, Clone)]
pub struct TeamState {
    /// The latest tank reading, from the most recent scalars or lap-close.
    fuel_litres: Option<f32>,
    /// The latest armed pit service: litres to add (`None` = fuelling off)
    /// and which corners are ticked. This is the echo every screen shows.
    service_fuel_litres: Option<i16>,
    /// The armed-tyre echo the ledger carries: which corners are ticked, and
    /// their armed cold pressures.
    tyres_armed: [bool; 4],
    tyre_pressures_kpa: [f32; 4],
    /// The latched last-stop tyre life, `None` before a stop has been seen.
    tyre_readings: Option<TyreInfo>,
    /// The shared fuel-per-lap target a spec set, `None` if cleared or never
    /// set — the number the driver's header chases.
    fuel_target: Option<f32>,
    /// The standing tyre directive and who set it, `None` if cleared or never
    /// set — the driver's overlay decides each stop against this.
    tyre_policy: Option<(TyrePolicy, String)>,
    /// Recent completed laps, oldest first, capped at [`LAP_HISTORY`].
    laps: Vec<LapFuel>,
    /// Who is currently in the car, from the last stint boundary.
    driver: Option<String>,
    /// The highest `session_time` any applied event carried, so a stale
    /// scalars frame arriving after a newer one can't walk the tank
    /// backwards. Lap history is keyed by lap number and immune to this.
    fuel_as_of: f64,
}

impl TeamState {
    /// Folds one envelope in. Idempotent for lap history (keyed by lap) and
    /// monotonic for the live tank (a later `session_time` wins).
    pub fn apply(&mut self, envelope: &Envelope) {
        match &envelope.event {
            Event::LapClosed { lap, fuel_litres, used_litres } => {
                self.record_lap(LapFuel { lap: *lap, fuel_after_litres: *fuel_litres, used_litres: *used_litres });
                self.set_fuel(*fuel_litres, envelope.session_time);
            }
            Event::DriverScalars { fuel_litres, service_fuel_litres, tyres_armed, tyre_pressures_kpa } => {
                self.set_fuel(*fuel_litres, envelope.session_time);
                // Armed service is not timestamped against `fuel_as_of`: it
                // is small, changes rarely, and the last one seen is the one
                // the sim last echoed, whatever the ordering of two nearby
                // scalars frames.
                self.service_fuel_litres = *service_fuel_litres;
                self.tyres_armed = *tyres_armed;
                self.tyre_pressures_kpa = *tyre_pressures_kpa;
            }
            Event::TyreReadings(info) => self.tyre_readings = Some(*info),
            Event::FuelTarget { litres_per_lap, .. } => self.fuel_target = *litres_per_lap,
            Event::TyrePolicySet { requester, policy } => {
                self.tyre_policy = policy.map(|policy| (policy, requester.clone()));
            }
            Event::StintBoundary { driver } => self.driver = Some(driver.clone()),
            // Field-wide events (pit stops, off-tracks) belong to the trackers'
            // replay path, not the car's fuel/tyre picture; a pit write is
            // transient and handled live by the runtime, never from the store.
            Event::PitStopObserved { .. } | Event::OffTrack { .. } | Event::PitWrite { .. } => {}
        }
    }

    /// The shared fuel-per-lap target, or `None` if none is set.
    #[must_use]
    pub fn fuel_target(&self) -> Option<f32> {
        self.fuel_target
    }

    /// The standing tyre directive and who set it, or `None` if none stands.
    ///
    /// Session state like the fuel target: it replays from the backlog, so a
    /// driver who joins (or rejoins) mid-race inherits the standing call
    /// without a spec re-sending it.
    #[must_use]
    pub fn tyre_policy(&self) -> Option<(TyrePolicy, &str)> {
        self.tyre_policy.as_ref().map(|(policy, requester)| (*policy, requester.as_str()))
    }

    /// Advances the live tank only for a not-older reading.
    fn set_fuel(&mut self, litres: f32, session_time: f64) {
        if session_time >= self.fuel_as_of {
            self.fuel_litres = Some(litres);
            self.fuel_as_of = session_time;
        }
    }

    /// Inserts or replaces a lap in the history, keeping it sorted and capped.
    fn record_lap(&mut self, lap: LapFuel) {
        match self.laps.binary_search_by_key(&lap.lap, |held| held.lap) {
            Ok(at) => self.laps[at] = lap,
            Err(at) => self.laps.insert(at, lap),
        }
        // Cap from the front: the oldest laps are the ones a trend has least
        // use for.
        if self.laps.len() > LAP_HISTORY {
            let overflow = self.laps.len() - LAP_HISTORY;
            self.laps.drain(0..overflow);
        }
    }

    /// The team car's fuel picture for a spectator's Fuel page, or `None`
    /// before any tank reading has arrived — which is what keeps the page from
    /// unlocking on a session the ledger knows nothing about yet.
    #[must_use]
    pub fn synced_car(&self) -> Option<SyncedCar> {
        let fuel_litres = self.fuel_litres?;
        Some(SyncedCar {
            driver: self.driver.clone(),
            fuel_litres,
            burn_per_lap: self.recent_burn_litres(BURN_WINDOW_LAPS),
            service_fuel_litres: self.service_fuel_litres,
            tyres_armed: self.tyres_armed,
            tyre_pressures_kpa: self.tyre_pressures_kpa,
            tyres: self.tyre_readings,
        })
    }

    /// Recent completed laps, oldest first.
    ///
    /// The tests exercise it; production reads it once the spec-mode Strategy
    /// fuel-trend band lands (`plans/strategy-spec-mode.md`), so the
    /// dead-code allowance is scoped to non-test builds.
    #[must_use]
    #[cfg_attr(not(test), expect(dead_code, reason = "consumed by the spec-mode Strategy fuel-trend band"))]
    pub fn laps(&self) -> &[LapFuel] {
        &self.laps
    }

    /// The mean burn over the last `window` laps, or `None` with none held.
    ///
    /// The same figure Auto Fuel and the spec-mode fuel band read, computed
    /// from the shared ledger so every member's baseline is identical.
    #[must_use]
    pub fn recent_burn_litres(&self, window: usize) -> Option<f32> {
        let recent: &[LapFuel] = self.laps.get(self.laps.len().saturating_sub(window)..)?;
        if recent.is_empty() {
            return None;
        }
        #[expect(clippy::cast_precision_loss, reason = "a lap window is a handful, exact in f32")]
        let count = recent.len() as f32;
        Some(recent.iter().map(|lap| lap.used_litres).sum::<f32>() / count)
    }

    /// Who is in the car, if a stint boundary has said.
    #[must_use]
    #[expect(dead_code, reason = "read via synced_car's clone; kept as a borrowed accessor for the spec-mode header")]
    pub fn driver(&self) -> Option<&str> {
        self.driver.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u32, session_time: f64, event: Event) -> Envelope {
        Envelope { producer: 1, seq, session_time, event }
    }

    #[test]
    fn scalars_set_the_live_tank_and_armed_service() {
        let mut state = TeamState::default();
        state.apply(&envelope(
            1,
            100.0,
            Event::DriverScalars {
                fuel_litres: 55.5,
                service_fuel_litres: Some(30),
                tyres_armed: [true, true, false, false],
                tyre_pressures_kpa: [165.0; 4],
            },
        ));
        let car = state.synced_car().expect("a reading");
        assert!((car.fuel_litres - 55.5).abs() < 0.01);
        assert_eq!(car.service_fuel_litres, Some(30));
    }

    #[test]
    fn a_stale_scalars_frame_does_not_rewind_the_tank() {
        let mut state = TeamState::default();
        state.apply(&envelope(
            2,
            200.0,
            Event::DriverScalars {
                fuel_litres: 50.0,
                service_fuel_litres: None,
                tyres_armed: [false; 4],
                tyre_pressures_kpa: [165.0; 4],
            },
        ));
        // An older frame arrives late — its tank must not overwrite the newer.
        state.apply(&envelope(
            1,
            100.0,
            Event::DriverScalars {
                fuel_litres: 58.0,
                service_fuel_litres: None,
                tyres_armed: [false; 4],
                tyre_pressures_kpa: [165.0; 4],
            },
        ));
        assert!((state.synced_car().expect("a reading").fuel_litres - 50.0).abs() < 0.01, "the newer reading holds");
    }

    #[test]
    fn laps_accumulate_in_order_and_feed_the_burn_average() {
        let mut state = TeamState::default();
        for lap in 1..=4_u16 {
            state.apply(&envelope(
                u32::from(lap),
                f64::from(lap) * 90.0,
                Event::LapClosed { lap, fuel_litres: 60.0 - f32::from(lap) * 2.0, used_litres: 2.0 },
            ));
        }
        assert_eq!(state.laps().len(), 4);
        assert_eq!(state.laps()[0].lap, 1);
        assert!((state.recent_burn_litres(3).expect("laps held") - 2.0).abs() < 0.01);
    }

    #[test]
    fn synced_car_is_none_until_a_tank_reading_then_carries_the_fuel_picture() {
        let mut state = TeamState::default();
        assert!(state.synced_car().is_none(), "no page unlocks on a session the ledger knows nothing of");

        state.apply(&envelope(1, 10.0, Event::StintBoundary { driver: "Flynn".to_owned() }));
        state.apply(&envelope(2, 20.0, Event::LapClosed { lap: 5, fuel_litres: 48.0, used_litres: 2.4 }));
        state.apply(&envelope(
            3,
            21.0,
            Event::DriverScalars {
                fuel_litres: 47.5,
                service_fuel_litres: Some(35),
                tyres_armed: [true; 4],
                tyre_pressures_kpa: [165.0; 4],
            },
        ));

        let car = state.synced_car().expect("a tank reading has arrived");
        assert!((car.fuel_litres - 47.5).abs() < 0.01, "the latest tank");
        assert_eq!(car.driver.as_deref(), Some("Flynn"));
        assert_eq!(car.service_fuel_litres, Some(35));
        assert!((car.burn_per_lap.expect("a lap closed") - 2.4).abs() < 0.01);
    }

    #[test]
    fn tyre_readings_and_armed_state_reach_the_synced_car() {
        use crate::telemetry::snapshot::{TyreInfo, TyreState};
        let mut state = TeamState::default();

        // Scalars carry the armed ticks and pressures.
        state.apply(&envelope(
            1,
            10.0,
            Event::DriverScalars {
                fuel_litres: 40.0,
                service_fuel_litres: Some(25),
                tyres_armed: [true, true, false, false],
                tyre_pressures_kpa: [165.0, 166.0, 167.0, 168.0],
            },
        ));
        // A stop's readings carry the tyre life.
        let corner = TyreState { temps_c: [80.0; 3], wear: [0.88; 3], pressure_kpa: 171.0 };
        state.apply(&envelope(2, 20.0, Event::TyreReadings(TyreInfo { corners: [corner; 4] })));

        let car = state.synced_car().expect("a reading");
        assert_eq!(car.tyres_armed, [true, true, false, false]);
        assert!((car.tyre_pressures_kpa[3] - 168.0).abs() < 0.01);
        let tyres = car.tyres.expect("a stop's readings arrived");
        assert!((tyres.corners[0].wear[0] - 0.88).abs() < 0.001);
    }

    #[test]
    fn synced_car_tyres_stay_none_until_a_stop() {
        let mut state = TeamState::default();
        state.apply(&envelope(
            1,
            10.0,
            Event::DriverScalars {
                fuel_litres: 40.0,
                service_fuel_litres: None,
                tyres_armed: [false; 4],
                tyre_pressures_kpa: [165.0; 4],
            },
        ));
        // Fuel is known, but no stop yet — the Tyres page must not unlock.
        assert!(state.synced_car().expect("a reading").tyres.is_none());
    }

    #[test]
    fn the_fuel_target_is_session_state_that_sets_revises_and_clears() {
        let mut state = TeamState::default();
        assert_eq!(state.fuel_target(), None, "no target until a spec sets one");

        state.apply(&envelope(1, 10.0, Event::FuelTarget { requester: "Ben".to_owned(), litres_per_lap: Some(2.55) }));
        assert!((state.fuel_target().expect("set") - 2.55).abs() < 0.001);

        // A revision replaces it — last write wins, like everything in sync.
        state.apply(&envelope(
            2,
            20.0,
            Event::FuelTarget { requester: "Flynn".to_owned(), litres_per_lap: Some(2.40) },
        ));
        assert!((state.fuel_target().expect("revised") - 2.40).abs() < 0.001);

        // Clearing drops the readout entirely.
        state.apply(&envelope(3, 30.0, Event::FuelTarget { requester: "Ben".to_owned(), litres_per_lap: None }));
        assert_eq!(state.fuel_target(), None);
    }

    /// A driver swap or reconnect replays the ledger; the standing target must
    /// come back with it, without anyone re-sending.
    #[test]
    fn a_replayed_ledger_restores_the_standing_fuel_target() {
        let mut fresh = TeamState::default();
        for envelope in [
            envelope(1, 10.0, Event::FuelTarget { requester: "Ben".to_owned(), litres_per_lap: Some(2.6) }),
            envelope(2, 20.0, Event::LapClosed { lap: 4, fuel_litres: 50.0, used_litres: 2.7 }),
        ] {
            fresh.apply(&envelope);
        }
        assert!((fresh.fuel_target().expect("from the backlog") - 2.6).abs() < 0.001);
    }

    #[test]
    fn the_tyre_policy_is_session_state_that_sets_revises_and_clears() {
        let mut state = TeamState::default();
        assert_eq!(state.tyre_policy(), None, "no directive until a spec sets one");

        state.apply(&envelope(
            1,
            10.0,
            Event::TyrePolicySet { requester: "Ben".to_owned(), policy: Some(TyrePolicy::Never) },
        ));
        assert_eq!(state.tyre_policy(), Some((TyrePolicy::Never, "Ben")));

        state.apply(&envelope(
            2,
            20.0,
            Event::TyrePolicySet {
                requester: "Flynn".to_owned(),
                policy: Some(TyrePolicy::BelowWear { threshold_pct: 75 }),
            },
        ));
        assert_eq!(state.tyre_policy(), Some((TyrePolicy::BelowWear { threshold_pct: 75 }, "Flynn")));

        state.apply(&envelope(3, 30.0, Event::TyrePolicySet { requester: "Ben".to_owned(), policy: None }));
        assert_eq!(state.tyre_policy(), None, "cleared back to the driver's own call");
    }

    /// The point of policy-not-action: a fresh driver's rebuilt store must
    /// carry the standing call so their overlay applies it at the next stop.
    #[test]
    fn a_replayed_ledger_restores_the_standing_tyre_policy() {
        let mut fresh = TeamState::default();
        for envelope in [
            envelope(1, 10.0, Event::TyrePolicySet { requester: "Ben".to_owned(), policy: Some(TyrePolicy::Never) }),
            envelope(2, 20.0, Event::LapClosed { lap: 4, fuel_litres: 50.0, used_litres: 2.7 }),
        ] {
            fresh.apply(&envelope);
        }
        assert_eq!(fresh.tyre_policy(), Some((TyrePolicy::Never, "Ben")));
    }

    #[test]
    fn a_pit_write_is_transient_and_never_becomes_car_state() {
        use crate::telemetry::pit::PitRequest;
        let mut state = TeamState::default();
        state.apply(&envelope(
            1,
            10.0,
            Event::DriverScalars {
                fuel_litres: 40.0,
                service_fuel_litres: Some(20),
                tyres_armed: [false; 4],
                tyre_pressures_kpa: [165.0; 4],
            },
        ));
        // A pit write folded into the store must change nothing about the
        // car's picture — it is applied live by the runtime, not stored.
        state.apply(&envelope(
            2,
            11.0,
            Event::PitWrite { requester: "Flynn".to_owned(), request: PitRequest::SetFuel(60) },
        ));
        let car = state.synced_car().expect("a reading");
        assert_eq!(car.service_fuel_litres, Some(20), "the write did not overwrite the echoed armed load");
        assert!((car.fuel_litres - 40.0).abs() < 0.01);
    }

    #[test]
    fn a_replayed_lap_is_not_duplicated() {
        let mut state = TeamState::default();
        let lap = Event::LapClosed { lap: 7, fuel_litres: 40.0, used_litres: 2.3 };
        state.apply(&envelope(1, 630.0, lap.clone()));
        state.apply(&envelope(1, 630.0, lap));
        assert_eq!(state.laps().len(), 1, "redelivery on reconnect must not double a lap");
    }

    #[test]
    fn history_is_capped_to_the_recent_window() {
        let mut state = TeamState::default();
        let over = u16::try_from(LAP_HISTORY).unwrap_or(u16::MAX) + 10;
        for lap in 1..=over {
            state.apply(&envelope(
                u32::from(lap),
                f64::from(lap),
                Event::LapClosed { lap, fuel_litres: 1.0, used_litres: 1.0 },
            ));
        }
        assert_eq!(state.laps().len(), LAP_HISTORY);
        assert_eq!(state.laps().first().expect("held").lap, 11, "the oldest laps drop off the front");
    }
}
