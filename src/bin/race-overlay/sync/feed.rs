// Rust guideline compliant 2026-02-16

//! Turning the driver's telemetry into sync events — the producer side.
//!
//! The seated driver's overlay is the only member that produces car data
//! (everyone else's sim publishes an empty tank for that car), so this runs
//! only while [`Seat::Driving`]. It watches successive snapshots and emits
//! two things: a [`Event::LapClosed`] each time the lap counter ticks over,
//! carrying the authoritative fuel record, and a throttled
//! [`Event::DriverScalars`] that animates the number in between.
//!
//! Pure and stateful, no network: `observe` takes what a frame measured and
//! returns the events to publish. That keeps the lap-edge and throttle logic
//! unit-testable without a sim or a socket — the tests below are the whole
//! specification of when an event fires.

use std::time::{Duration, Instant};

use super::protocol::Event;
use crate::telemetry::snapshot::TyreInfo;

/// The least time between driver-scalars ticks — the "~1 Hz" of the plan.
///
/// Changing values are capped to this cadence. Unchanged values use the
/// separate, slower liveness heartbeat below.
const SCALARS_INTERVAL: Duration = Duration::from_secs(1);

/// A seated car can sit stationary with an unchanged tank for minutes. The
/// heartbeat proves the driver overlay is still alive to spectators without
/// turning every frame into a network sample.
const SCALARS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// Litres a fuel figure is rounded to before it is compared and sent.
///
/// Fine enough that a spectator's readout matches the driver's to the
/// hundredth, coarse enough that sensor jitter in the low bits doesn't
/// manufacture a tick a frame. Quantizing before the unchanged-check is what
/// keeps a settled tank quiet between liveness heartbeats.
const FUEL_QUANTUM_LITRES: f32 = 0.01;

/// What one frame measured about the driver's own car.
///
/// Assembled by the app from a [`TelemetrySnapshot`] before calling
/// [`EventSource::observe`]; kept as a plain struct so the producer has no
/// dependency on the snapshot's shape and its tests need no snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriverObservation {
    /// iRacing `SessionTime`, stamped onto every event produced.
    pub session_time: f64,
    /// The player's current lap number.
    pub lap: u16,
    /// iRacing car index of the team car. `None` while the focus row has not
    /// arrived yet; the measurement can still be retained but must not be
    /// shown against an arbitrary spectator camera.
    pub car_idx: Option<i32>,
    /// Litres in the tank now.
    pub fuel_litres: f32,
    /// The rolling per-lap burn, used as the lap's consumption when the tank
    /// delta can't be trusted (a stop added fuel this lap). `None` before a
    /// lap has been measured.
    pub fuel_per_lap_litres: Option<f32>,
    /// Litres armed to add at the next stop, `None` when fuelling is unticked.
    pub service_fuel_litres: Option<i16>,
    /// Which corners are armed, in `pit::corner_index` order.
    pub tyres_armed: [bool; 4],
    /// The armed cold pressures per corner, kPa.
    pub tyre_pressures_kpa: [f32; 4],
    /// The latched last-stop tyre life, sent on change.
    pub tyres: TyreInfo,
    /// The car is on pit road, so this lap's fuel delta includes a fill and
    /// is not a racing burn.
    pub on_pit_road: bool,
}

/// The quantized scalars last sent, for the unchanged-check.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SentScalars {
    fuel_centilitres: i32,
    service_fuel_litres: Option<i16>,
    tyres_armed: [bool; 4],
    /// Armed pressures, rounded to whole kPa for the unchanged-check.
    pressures_kpa: [i32; 4],
}

/// Watches the driver's frames and yields the events to publish.
#[derive(Debug, Default)]
pub struct EventSource {
    /// The lap number last seen, to detect the edge. `None` until the first
    /// frame, so a fresh source does not mistake the opening lap for a close.
    last_lap: Option<u16>,
    /// The tank at the start of the current lap, to measure what it burned.
    lap_start_fuel: Option<f32>,
    /// Whether `lap_start_fuel` was sampled at a timing-line crossing. A
    /// source can attach halfway around a lap after a driver swap or telemetry
    /// resume; that partial segment must never become a full-lap fuel record.
    lap_start_is_crossing: bool,
    /// A stop anywhere on this lap invalidates its tank delta, even if the
    /// car has left pit road before crossing the timing line.
    lap_had_stop: bool,
    /// The last scalars actually sent, and when — the throttle's memory.
    last_scalars: Option<SentScalars>,
    last_scalars_at: Option<Instant>,
    /// The tyre readings last sent, so a stop's fresh values are sent once and
    /// an unchanged latch stays silent.
    last_tyres: Option<TyreInfo>,
}

impl EventSource {
    /// Advances by one frame, returning the events to publish this tick.
    ///
    /// `listeners` gates the scalars tick: with nobody watching there is
    /// nothing to animate, so the ~1 Hz stream falls silent entirely — the
    /// lap-closed record still fires, because the ledger wants it whether or
    /// not anyone is connected right now.
    ///
    /// `now` is the wall clock for the throttle only; every event is stamped
    /// with the observation's own `session_time`, which is what consumers
    /// order by.
    pub fn observe(&mut self, obs: DriverObservation, now: Instant, listeners: bool) -> Vec<Event> {
        let mut events = Vec::new();

        let crossed_line = self.last_lap.is_some_and(|previous| obs.lap > previous);
        if let Some(previous) = self.last_lap
            && obs.lap > previous
            && self.lap_start_is_crossing
            && let Some(started_with) = self.lap_start_fuel
        {
            // The tank's fall since the last counted crossing — unless a stop
            // added fuel mid-lap, where the delta is meaningless and the
            // rolling average is the honest figure. A lag spike can jump the
            // counter by more than one, in which case the delta spans every
            // lap of the gap: divided down, so a spike can't report three
            // laps' fuel as one lap's burn and poison the shared average.
            let laps_elapsed = f32::from(obs.lap - previous);
            let burned = started_with - obs.fuel_litres;
            let used = if self.lap_had_stop || obs.on_pit_road || burned < 0.0 {
                obs.fuel_per_lap_litres.unwrap_or(burned.max(0.0))
            } else {
                burned / laps_elapsed
            };
            events.push(Event::LapClosed {
                lap: obs.lap.saturating_sub(1),
                fuel_litres: obs.fuel_litres,
                used_litres: used,
            });
        }
        if self.last_lap == Some(obs.lap) {
            self.lap_had_stop |= obs.on_pit_road;
        } else {
            self.lap_start_fuel = Some(obs.fuel_litres);
            self.lap_had_stop = obs.on_pit_road;
            // The first forward lap change after attachment is the timing
            // line that seeds the following lap. A backwards reset is not.
            self.lap_start_is_crossing = crossed_line;
        }
        self.last_lap = Some(obs.lap);

        if listeners && let Some(scalars) = self.scalars_if_due(obs, now) {
            events.push(scalars);
        }

        // Tyre readings change only at a stop, so they are sent on change and
        // not on a clock — and regardless of listeners, so a stop that happens
        // while nobody is watching is still in the ledger for whoever joins
        // next. An all-zero latch (no stop yet) is not worth sending.
        if obs.tyres.any_collected() && self.last_tyres != Some(obs.tyres) {
            self.last_tyres = Some(obs.tyres);
            events.push(Event::TyreReadings(obs.tyres));
        }
        events
    }

    /// The scalars event to send this frame, if the throttle and the
    /// unchanged-check both allow it.
    fn scalars_if_due(&mut self, obs: DriverObservation, now: Instant) -> Option<Event> {
        let quantized = SentScalars {
            fuel_centilitres: quantize(obs.fuel_litres),
            service_fuel_litres: obs.service_fuel_litres,
            tyres_armed: obs.tyres_armed,
            pressures_kpa: obs.tyre_pressures_kpa.map(round_kpa),
        };
        // An unchanged tank is normally silent, but still needs a small
        // heartbeat: otherwise a stationary, engine-off driver would look
        // indistinguishable from a disappeared driver after the freshness
        // window expires.
        if self.last_scalars == Some(quantized) {
            let due = self.last_scalars_at.is_none_or(|at| now.duration_since(at) >= SCALARS_HEARTBEAT_INTERVAL);
            if !due {
                return None;
            }
        } else if let Some(at) = self.last_scalars_at
            && now.duration_since(at) < SCALARS_INTERVAL
        {
            // Changed, but not yet a heartbeat apart: a rapidly-draining
            // value is still capped to the interval so it can't flood the
            // wire.
            return None;
        }
        self.last_scalars = Some(quantized);
        self.last_scalars_at = Some(now);
        Some(Event::DriverScalars {
            car_idx: obs.car_idx,
            fuel_litres: obs.fuel_litres,
            service_fuel_litres: obs.service_fuel_litres,
            tyres_armed: obs.tyres_armed,
            tyre_pressures_kpa: obs.tyre_pressures_kpa,
        })
    }
}

/// Rounds a pressure to whole kPa, as an integer so the unchanged-check is
/// exact. A cold pressure is a few hundred kPa at most — far inside `i32`.
#[expect(clippy::cast_possible_truncation, reason = "a tyre pressure in kPa is far inside i32")]
fn round_kpa(kpa: f32) -> i32 {
    kpa.round() as i32
}

/// Rounds litres to whole [`FUEL_QUANTUM_LITRES`] steps, as an integer so the
/// unchanged-check is exact rather than a float comparison.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fuel load is at most a few hundred litres; in centilitres it is far inside i32"
)]
fn quantize(litres: f32) -> i32 {
    (litres / FUEL_QUANTUM_LITRES).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(session_time: f64, lap: u16, fuel: f32) -> DriverObservation {
        DriverObservation {
            session_time,
            lap,
            car_idx: Some(0),
            fuel_litres: fuel,
            fuel_per_lap_litres: Some(2.5),
            service_fuel_litres: None,
            tyres_armed: [false; 4],
            tyre_pressures_kpa: [165.0; 4],
            tyres: TyreInfo::default(),
            on_pit_road: false,
        }
    }

    /// A tyre reading with a distinguishing wear value in the first slot, so a
    /// change is visible.
    fn tyres_with(wear: f32) -> TyreInfo {
        use crate::telemetry::snapshot::TyreState;
        let corner = TyreState { temps_c: [80.0; 3], wear: [wear; 3], pressure_kpa: 170.0 };
        TyreInfo { corners: [corner; 4] }
    }

    #[test]
    fn a_lap_closes_with_the_fuel_it_burned() {
        let mut source = EventSource::default();
        let start = Instant::now();
        // First frame establishes the baseline; nothing closes yet.
        assert!(source.observe(obs(10.0, 5, 60.0), start, false).is_empty());
        // First observed crossing only seeds a complete following lap.
        assert!(source.observe(obs(100.0, 6, 57.4), start, false).is_empty());
        // The next crossing closes lap 6, burning 57.4 -> 54.8.
        let events = source.observe(obs(190.0, 7, 54.8), start, false);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::LapClosed { lap, fuel_litres, used_litres } => {
                assert_eq!(*lap, 6);
                assert!((fuel_litres - 54.8).abs() < 0.01);
                assert!((used_litres - 2.6).abs() < 0.01, "used is the measured tank delta");
            }
            other => panic!("expected a lap close, got {other:?}"),
        }
    }

    /// A lag spike jumping the counter several laps must not report the whole
    /// gap's fuel as one lap's burn — that lap would poison every average and
    /// projection built on the shared ledger.
    #[test]
    fn a_multi_lap_jump_reports_the_per_lap_burn_not_the_whole_gap() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(10.0, 5, 60.0), start, false);
        source.observe(obs(100.0, 6, 57.5), start, false);
        // Laps 6, 7 and 8 pass in one observation (7.5 L over three laps).
        let events = source.observe(obs(370.0, 9, 50.0), start, false);
        match events.as_slice() {
            [Event::LapClosed { lap, used_litres, .. }] => {
                assert_eq!(*lap, 8, "the most recently completed lap is the one named");
                assert!((used_litres - 2.5).abs() < 0.01, "the burn is per lap, not the whole gap");
            }
            other => panic!("expected one lap close, got {other:?}"),
        }
    }

    /// A session restart walks the lap counter backwards; nothing closes, and
    /// the fuel baseline resets so the next real crossing measures honestly.
    #[test]
    fn a_backwards_lap_resets_the_baseline_without_an_event() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(500.0, 30, 40.0), start, false);
        // The session restarts: lap 0, full tank.
        let events = source.observe(obs(1.0, 0, 60.0), start, false);
        assert!(!events.iter().any(|e| matches!(e, Event::LapClosed { .. })), "a rewind closes nothing");
        // The first real lap after measures from the new baseline, not the
        // old session's tank.
        assert!(source.observe(obs(95.0, 1, 57.4), start, false).is_empty());
        let events = source.observe(obs(190.0, 2, 54.8), start, false);
        match events.as_slice() {
            [Event::LapClosed { used_litres, .. }] => {
                assert!((used_litres - 2.6).abs() < 0.01, "measured from the restart's 60 L, not the old 40 L");
            }
            other => panic!("expected one lap close, got {other:?}"),
        }
    }

    #[test]
    fn a_lap_with_a_stop_uses_the_rolling_average_not_the_delta() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(10.0, 5, 20.0), start, false);
        source.observe(obs(20.0, 6, 20.0), start, false);
        // Fuel went up over the lap (a stop): the delta is negative and
        // meaningless, so the rolling average stands in.
        let mut pitted = obs(100.0, 7, 60.0);
        pitted.on_pit_road = true;
        let events = source.observe(pitted, start, false);
        match events.as_slice() {
            [Event::LapClosed { used_litres, .. }] => {
                assert!((used_litres - 2.5).abs() < 0.01, "the rolling average, not the fill");
            }
            other => panic!("expected one lap close, got {other:?}"),
        }
    }

    #[test]
    fn a_small_top_up_before_pit_exit_does_not_understate_lap_burn() {
        let mut source = EventSource::default();
        let now = Instant::now();
        source.observe(obs(10.0, 5, 20.0), now, false);
        source.observe(obs(20.0, 6, 20.0), now, false);
        let mut in_pits = obs(40.0, 6, 18.0);
        in_pits.on_pit_road = true;
        source.observe(in_pits, now, false);
        // A one-litre top-up is less than this lap's consumption. At the
        // crossing the tank is lower than at lap start and pit road is false.
        in_pits.fuel_litres = 19.0;
        source.observe(in_pits, now, false);
        source.observe(obs(70.0, 6, 18.8), now, false);
        let events = source.observe(obs(100.0, 7, 18.5), now, false);
        let [Event::LapClosed { used_litres, .. }] = events.as_slice() else {
            panic!("expected a lap close, got {events:?}");
        };
        // Use the rolling average, not the 1.5 L tank delta.
        assert!((used_litres - 2.5).abs() < 0.01);

        // The stop latch clears at the crossing: a normal following lap
        // must use its actual delta, even if the rolling average differs.
        let events = source.observe(obs(190.0, 8, 15.5), now, false);
        let [Event::LapClosed { used_litres, .. }] = events.as_slice() else {
            panic!("expected a lap close, got {events:?}");
        };
        assert!((used_litres - 3.0).abs() < 0.01);
    }

    #[test]
    fn scalars_are_silent_with_no_listeners() {
        let mut source = EventSource::default();
        let start = Instant::now();
        let events = source.observe(obs(10.0, 5, 60.0), start, false);
        assert!(!events.iter().any(|event| matches!(event, Event::DriverScalars { .. })));
    }

    #[test]
    fn scalars_send_on_change_then_stay_silent_until_they_change_again() {
        let mut source = EventSource::default();
        let start = Instant::now();
        // First observation with a listener sends the opening value.
        let first = source.observe(obs(10.0, 5, 60.0), start, true);
        assert!(first.iter().any(|event| matches!(event, Event::DriverScalars { .. })), "the first value is news");

        // A heartbeat later but unchanged (quantized): nothing.
        let later = start + SCALARS_INTERVAL + Duration::from_millis(1);
        let quiet = source.observe(obs(11.0, 5, 60.004), later, true);
        assert!(!quiet.iter().any(|event| matches!(event, Event::DriverScalars { .. })), "a settled tank is silent");

        // A real change, another heartbeat on: a tick.
        let later2 = later + SCALARS_INTERVAL;
        let moved = source.observe(obs(12.0, 5, 59.5), later2, true);
        assert!(moved.iter().any(|event| matches!(event, Event::DriverScalars { .. })), "a changed tank ticks");
    }

    #[test]
    fn an_unchanged_stationary_car_heartbeats_for_liveness() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(10.0, 5, 60.0), start, true);
        let quiet = source.observe(obs(11.0, 5, 60.0), start + Duration::from_secs(1), true);
        assert!(!quiet.iter().any(|event| matches!(event, Event::DriverScalars { .. })));
        let heartbeat = source.observe(obs(12.0, 5, 60.0), start + SCALARS_HEARTBEAT_INTERVAL, true);
        assert!(heartbeat.iter().any(|event| matches!(event, Event::DriverScalars { .. })));
    }

    #[test]
    fn a_changing_tank_is_capped_to_the_interval() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(10.0, 5, 60.0), start, true);
        // Well within the interval, and changed — still throttled.
        let soon = start + Duration::from_millis(200);
        let events = source.observe(obs(10.2, 5, 59.9), soon, true);
        assert!(
            !events.iter().any(|event| matches!(event, Event::DriverScalars { .. })),
            "under the interval, no tick"
        );
    }

    #[test]
    fn tyre_readings_send_on_change_regardless_of_listeners() {
        let mut source = EventSource::default();
        let start = Instant::now();

        // A fresh stop's readings send even with nobody watching — the ledger
        // wants them for whoever joins next.
        let mut first = obs(10.0, 5, 60.0);
        first.tyres = tyres_with(0.9);
        let events = source.observe(first, start, false);
        assert!(
            events.iter().any(|e| matches!(e, Event::TyreReadings(_))),
            "a first stop's readings are sent without a listener"
        );

        // Unchanged latch next tick: silent.
        let mut same = obs(11.0, 5, 59.0);
        same.tyres = tyres_with(0.9);
        let events = source.observe(same, start, false);
        assert!(!events.iter().any(|e| matches!(e, Event::TyreReadings(_))), "an unchanged latch stays silent");

        // A new stop (different wear): sent again.
        let mut next_stop = obs(200.0, 6, 58.0);
        next_stop.tyres = tyres_with(0.7);
        let events = source.observe(next_stop, start, false);
        match events.iter().find(|e| matches!(e, Event::TyreReadings(_))) {
            Some(Event::TyreReadings(info)) => assert!((info.corners[0].wear[0] - 0.7).abs() < 0.001),
            _ => panic!("a new stop's readings should send"),
        }
    }

    #[test]
    fn an_all_zero_tyre_latch_is_never_sent() {
        let mut source = EventSource::default();
        // Default TyreInfo is all zeros — no stop yet, nothing to say.
        let events = source.observe(obs(10.0, 5, 60.0), Instant::now(), true);
        assert!(!events.iter().any(|e| matches!(e, Event::TyreReadings(_))), "no stop, no readings");
    }

    #[test]
    fn an_armed_pressure_change_alone_ticks_scalars() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(10.0, 5, 60.0), start, true);
        let mut repressed = obs(10.5, 5, 60.0);
        repressed.tyre_pressures_kpa = [172.0, 165.0, 165.0, 165.0];
        let events = source.observe(repressed, start + SCALARS_INTERVAL, true);
        match events.iter().find(|e| matches!(e, Event::DriverScalars { .. })) {
            Some(Event::DriverScalars { tyre_pressures_kpa, .. }) => {
                assert!((tyre_pressures_kpa[0] - 172.0).abs() < 0.01);
            }
            _ => panic!("a pressure change should tick scalars"),
        }
    }

    #[test]
    fn arming_tyres_is_a_change_worth_sending() {
        let mut source = EventSource::default();
        let start = Instant::now();
        source.observe(obs(10.0, 5, 60.0), start, true);
        let mut armed = obs(10.5, 5, 60.0);
        armed.tyres_armed = [true, true, true, true];
        // Same fuel, but the tyres changed — and a heartbeat has passed.
        let events = source.observe(armed, start + SCALARS_INTERVAL, true);
        match events.iter().find(|event| matches!(event, Event::DriverScalars { .. })) {
            Some(Event::DriverScalars { tyres_armed, .. }) => assert_eq!(*tyres_armed, [true; 4]),
            _ => panic!("arming tyres should send scalars"),
        }
    }
}
