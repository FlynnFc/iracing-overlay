// Rust guideline compliant 2026-02-16

//! How far the car is from the perfect stopping point in its own pit box.
//!
//! iRacing publishes the player's stall as `DriverPitTrkPct`, a fraction of a
//! lap in the same coordinate `CarIdxLapDistPct` reports positions in. The
//! signed difference between the two is the whole measurement; everything else
//! in this module exists to turn that fraction into honest metres and to decide
//! when it is worth showing.
//!
//! Three things are less obvious than they look, and each has a type here:
//!
//! - **A percentage is not a distance.** The pit lane and the racing line are
//!   different lengths, so `error_pct * track_length` is wrong by however much
//!   the lane cuts the corner. [`MetresPerPct`] measures the real scale from
//!   speed and position while the car is still driving down the lane.
//! - **The lap wraps.** A pit stall can sit either side of the start/finish
//!   line, and an unwrapped subtraction reads -0.98 where it means +0.02.
//! - **"Within range" is true twice a visit** — once coming in and once
//!   leaving. [`Visit`] is what ends the bar the moment the car is parked and
//!   stops it reappearing behind a car that has already been served.
//!
//! No egui and no I/O: everything here is a pure function or a small tracker
//! driven by one telemetry tick, which is what makes it testable.

use crate::telemetry::relative::wrap_shortest;
use crate::telemetry::snapshot::PitStallSnapshot;

/// How close the car must be to its box before the bar appears, in metres.
///
/// Roughly seven stalls' worth of lane, which is four or five seconds at the
/// pit lane speed limit. Far enough that the bar is up and being read well
/// before the braking point, near enough that it is not up for the whole lane —
/// finding a box you cannot see is a different problem and deliberately not
/// this widget's.
///
/// Larger than the bar's own full scale (`PitStallConfig::range_m`), so the
/// widget is already on screen and settled before its level starts to move.
pub const APPEAR_RANGE_M: f32 = 100.0;

/// Half-width of the green zone before the box has been measured, in metres.
///
/// Deliberately tighter than a real iRacing stall box: until the box's true
/// extent is known (a later phase), erring small means the uncalibrated bar
/// says "get it closer" rather than "that will do" and is never wrong in the
/// direction that costs a stop.
pub const DEFAULT_GREEN_HALF_WIDTH_M: f32 = 1.0;

/// Lap length assumed when the sim publishes none, in metres.
///
/// Only ever used to place the marker, never to print a number — see
/// [`PitStallSnapshot::readout_m`]. A bar with no figure beside it is honest
/// about what it knows; a figure derived from a guessed lap length is not.
const NOMINAL_LAP_M: f32 = 4000.0;

/// At or below this speed the car counts as stopped, in m/s.
///
/// Under 2 km/h. Loose enough to catch a car settling onto its marks against
/// the last of its own rolling, tight enough that a crawl up the box still
/// counts as moving and keeps the bar.
const STOPPED_SPEED_MPS: f32 = 0.5;

/// Slowest the car may be moving for a scale sample to count, in m/s.
///
/// Around 18 km/h. Below this the position change per tick is small enough
/// that its own quantization dominates the ratio being measured.
const MIN_SAMPLE_SPEED_MPS: f32 = 5.0;

/// Longest gap between ticks a scale sample may span, in seconds.
///
/// A longer gap means the app was stalled — a frame hitch, a session pause —
/// and the distance covered in between is not `speed * dt`.
const MAX_SAMPLE_DT_SECS: f64 = 0.5;

/// Bounds on a believable lap length, in metres, used to reject bad samples.
///
/// Every iRacing circuit falls inside this by a wide margin; anything outside
/// it came from a wrapped or dropped tick, not from the car.
const MIN_PLAUSIBLE_LAP_M: f32 = 500.0;
/// See [`MIN_PLAUSIBLE_LAP_M`].
const MAX_PLAUSIBLE_LAP_M: f32 = 30_000.0;

/// How many recent samples the measured scale is the median of.
///
/// About a second at iRacing's 60 Hz. Long enough to shrug off the occasional
/// bad tick, short enough to be full well before the car reaches its box even
/// if the lane was joined late.
const SCALE_WINDOW: usize = 64;

/// The measured metres-per-percent-of-lap, from speed against track position.
///
/// The ratio is sampled every tick the car is moving down the pit lane and
/// reported as the median of a short window. A median rather than a mean
/// because one wrapped tick produces an enormous outlier and a mean never
/// recovers from it inside a single pit entry — which is the only window this
/// has to be right in.
#[derive(Debug, Default)]
pub struct MetresPerPct {
    /// Most recent samples, oldest first, capped at [`SCALE_WINDOW`].
    samples: Vec<f32>,
    /// Last tick's position and clock, to difference against.
    last: Option<(f32, f64)>,
}

impl MetresPerPct {
    /// Feeds one tick, returning nothing; read the result with [`Self::get`].
    ///
    /// `sampling` is whether this tick is one worth learning from at all — in
    /// practice, whether the car is on pit road. Position and time are still
    /// recorded when it is false, so the first sample after it turns true
    /// differences against an adjacent tick rather than against wherever the
    /// car was the last time it was in the lane.
    pub fn update(&mut self, lap_dist_pct: f32, speed_mps: Option<f32>, session_time_secs: f64, sampling: bool) {
        let previous = self.last.replace((lap_dist_pct, session_time_secs));
        let (Some((last_pct, last_time)), Some(speed), true) = (previous, speed_mps, sampling) else {
            return;
        };
        let dt = session_time_secs - last_time;
        if dt <= 0.0 || dt > MAX_SAMPLE_DT_SECS || speed < MIN_SAMPLE_SPEED_MPS {
            return;
        }
        // Wrapped, because a car can cross the start/finish line inside the
        // pit lane; forward only, because a reversing car measures nothing
        // useful and a stationary one divides by zero.
        let delta_pct = wrap_shortest(lap_dist_pct - last_pct, 1.0);
        if delta_pct <= 0.0 {
            return;
        }
        #[expect(clippy::cast_possible_truncation, reason = "dt is bounded by MAX_SAMPLE_DT_SECS, well inside f32")]
        let metres = speed * dt as f32;
        let scale = metres / delta_pct;
        if !(MIN_PLAUSIBLE_LAP_M..=MAX_PLAUSIBLE_LAP_M).contains(&scale) {
            return;
        }
        if self.samples.len() == SCALE_WINDOW {
            self.samples.remove(0);
        }
        self.samples.push(scale);
    }

    /// The median of the current window, or `None` if nothing has been sampled.
    #[must_use]
    pub fn get(&self) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable_by(f32::total_cmp);
        Some(sorted[sorted.len() / 2])
    }
}

/// Where this visit to the pit lane has got to.
///
/// "Within range of the box" is true twice per visit — once on the way in and
/// once on the way out — so range alone cannot drive visibility. This is what
/// distinguishes the two, and what ends the bar once the car is parked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Visit {
    /// Not on pit road.
    #[default]
    Away,
    /// On pit road, still being placed.
    Approaching,
    /// Parked in the box, or past it and finished with; nothing more to show
    /// until the car leaves pit road.
    Done,
}

/// Tracks one player's approach to their own pit box, tick by tick.
///
/// Owns the scale estimator and the visit state machine, and turns each tick
/// into the [`PitStallSnapshot`] the widget draws.
///
/// One per session, and discarded wholesale when iRacing moves on to the next
/// one — see `SessionTrackers::sync_to_session`. Nothing here is worth
/// carrying across: the measured scale refills within a second of driving the
/// lane, long before the car reaches its box.
#[derive(Debug, Default)]
pub struct PitStallTracker {
    scale: MetresPerPct,
    visit: Visit,
    /// Whether the "no stall published" note has been said for this session,
    /// so it is said once rather than sixty times a second.
    missing_target_reported: bool,
}

/// One tick's worth of the inputs this tracker reads.
///
/// A struct rather than seven positional arguments, because five of them are
/// `f32` or `Option<f32>` and swapping two at a call site would compile.
#[derive(Debug, Clone, Copy)]
pub struct PitStallTick {
    /// The player's own position around the lap, `CarIdxLapDistPct`.
    pub lap_dist_pct: Option<f32>,
    /// The player's own stall, `DriverInfo.DriverPitTrkPct`. Zero where the
    /// session publishes none.
    pub target_pct: f32,
    /// `Speed` in m/s, absent on a car that doesn't publish it.
    pub speed_mps: Option<f32>,
    /// The lap length in metres, from `WeekendInfo.TrackLength`.
    pub track_length_m: Option<f32>,
    /// `SessionTime`, which the scale samples are differenced against.
    pub session_time_secs: f64,
    /// Whether the car is anywhere on pit road, box included.
    pub on_pit_road: bool,
    /// `PlayerCarInPitStall` — the sim's own word for "properly in the box".
    pub in_box: bool,
    /// Whether the player is driving rather than watching. A spectator's
    /// camera car has a stall, but not one this reading describes.
    pub is_driving: bool,
}

impl PitStallTracker {
    /// Folds one telemetry tick in and returns what the widget should draw.
    pub fn update(&mut self, tick: PitStallTick) -> PitStallSnapshot {
        let Some(lap_dist_pct) = tick.lap_dist_pct else {
            return PitStallSnapshot::default();
        };
        self.scale.update(lap_dist_pct, tick.speed_mps, tick.session_time_secs, tick.on_pit_road);

        if !tick.on_pit_road {
            // Leaving the lane ends the visit, so a second stop on the same
            // lap — a penalty served straight after a stop — gets a fresh bar.
            self.visit = Visit::Away;
            return PitStallSnapshot::default();
        }
        if !tick.is_driving {
            return PitStallSnapshot::default();
        }
        if !target_is_published(tick.target_pct) {
            if !self.missing_target_reported {
                self.missing_target_reported = true;
                println!("note: this session publishes no pit stall position; the pit stall bar stays hidden");
            }
            return PitStallSnapshot::default();
        }

        let error_pct = wrap_shortest(lap_dist_pct - tick.target_pct, 1.0);
        let (scale_m_per_pct, measured) = match self.scale.get().or(tick.track_length_m) {
            Some(scale) => (scale, true),
            None => (NOMINAL_LAP_M, false),
        };
        let error_m = error_pct * scale_m_per_pct;

        if self.visit == Visit::Away {
            self.visit = Visit::Approaching;
        }
        // Parked in the box, and nothing else. The stop is under way, and a bar
        // telling you where to stop is clutter through the fuelling and the
        // drive out.
        //
        // Stopped rather than merely in the box, because `PlayerCarInPitStall`
        // goes true while the car is still rolling in — hiding on the flag
        // alone would take the bar away during the very seconds it is being
        // used to place the car.
        //
        // And deliberately not "has driven past the box", which an earlier
        // draft also ended on: overshooting your marks and reversing back is
        // exactly the moment you need this widget most, and that rule put it
        // away just as the driver started to need it. A car that never stops —
        // a drive-through, or one whose sim publishes no speed — simply runs
        // out of `APPEAR_RANGE_M` and the bar goes on its own.
        if tick.in_box && tick.speed_mps.is_some_and(|speed| speed.abs() < STOPPED_SPEED_MPS) {
            self.visit = Visit::Done;
        }

        PitStallSnapshot {
            // The sim's own word outranks the arithmetic: a car still rolling
            // into its box must never be looking at a bar that says otherwise.
            visible: self.visit != Visit::Done && (tick.in_box || error_m.abs() <= APPEAR_RANGE_M),
            error_m,
            readout_m: measured.then_some(error_m),
            in_box: tick.in_box,
            green_half_width_m: DEFAULT_GREEN_HALF_WIDTH_M,
        }
    }
}

/// Whether the session published a usable stall position at all.
///
/// Exactly zero is iRacing's "there is no pit lane here" — a real stall sits
/// somewhere along the lap and reads as a small but non-zero fraction. A
/// negative or non-finite value can only be a parse artefact.
fn target_is_published(target_pct: f32) -> bool {
    target_pct.is_finite() && target_pct > 0.0 && target_pct < 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tick with the car driving down its own pit lane, 10 m short of the
    /// box on a 4000 m lap, which every test below varies from.
    fn tick() -> PitStallTick {
        PitStallTick {
            lap_dist_pct: Some(0.1 - 10.0 / 4000.0),
            target_pct: 0.1,
            speed_mps: Some(16.0),
            track_length_m: Some(4000.0),
            session_time_secs: 100.0,
            on_pit_road: true,
            in_box: false,
            is_driving: true,
        }
    }

    /// A distance comfortably outside [`APPEAR_RANGE_M`], expressed against it
    /// rather than written out: these tests are about the rule, not the number,
    /// and a literal here goes stale the moment the range is retuned.
    fn out_of_range_m() -> f32 {
        APPEAR_RANGE_M * 1.5
    }

    /// A stall can sit either side of the start/finish line. Unwrapped, a car
    /// four metres before the line reads as most of a lap past a stall four
    /// metres after it, and the bar points the wrong way — or, being out of
    /// range, never appears at all.
    #[test]
    fn a_stall_across_the_start_finish_line_reads_as_a_short_gap() {
        let mut tracker = PitStallTracker::default();
        let snapshot =
            tracker.update(PitStallTick { lap_dist_pct: Some(1.0 - 4.0 / 4000.0), target_pct: 4.0 / 4000.0, ..tick() });
        assert!((snapshot.error_m + 8.0).abs() < 0.5, "expected 8 m short, got {}", snapshot.error_m);
        assert!(snapshot.visible, "8 m short is well inside the appear range");
    }

    #[test]
    fn short_of_the_box_is_negative_and_past_it_is_positive() {
        let mut tracker = PitStallTracker::default();
        let short = tracker.update(tick());
        assert!(short.error_m < 0.0);

        let mut tracker = PitStallTracker::default();
        let past = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 2.0 / 4000.0), ..tick() });
        assert!(past.error_m > 0.0);
    }

    #[test]
    fn nothing_is_drawn_off_pit_road() {
        let mut tracker = PitStallTracker::default();
        assert!(!tracker.update(PitStallTick { on_pit_road: false, ..tick() }).visible);
    }

    #[test]
    fn nothing_is_drawn_while_spectating() {
        let mut tracker = PitStallTracker::default();
        assert!(!tracker.update(PitStallTick { is_driving: false, ..tick() }).visible);
    }

    /// A session with no pit lane publishes zero, and a bar drawn against a
    /// zero target would point at the start/finish line.
    #[test]
    fn nothing_is_drawn_without_a_published_stall() {
        let mut tracker = PitStallTracker::default();
        assert!(!tracker.update(PitStallTick { target_pct: 0.0, ..tick() }).visible);
    }

    #[test]
    fn the_bar_stays_hidden_until_the_box_is_within_range() {
        let mut tracker = PitStallTracker::default();
        let far = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 - out_of_range_m() / 4000.0), ..tick() });
        assert!(!far.visible);
        assert!(tracker.update(tick()).visible, "10 m out is well inside the appear range");
    }

    /// The normal end of a visit: the car is parked and being worked on, and a
    /// bar saying where to stop is clutter for the rest of the stop.
    #[test]
    fn stopping_in_the_box_ends_the_visit() {
        let mut tracker = PitStallTracker::default();
        assert!(tracker.update(tick()).visible);
        let parked =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(0.0), ..tick() });
        assert!(!parked.visible, "a car stopped in its box needs no bar");

        // And it stays gone through the fuelling and the drive out, including
        // the stretch where the box is still well inside the appear range.
        let fuelling =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(0.0), ..tick() });
        assert!(!fuelling.visible);
        let pulling_away =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 3.0 / 4000.0), speed_mps: Some(6.0), ..tick() });
        assert!(!pulling_away.visible);
        let leaving = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 20.0 / 4000.0), ..tick() });
        assert!(!leaving.visible);
    }

    /// Overshooting the box and reversing back into it is the moment this
    /// widget is worth the most, so nothing about driving past the marks may
    /// end the visit — only actually stopping in them.
    #[test]
    fn overshooting_keeps_the_bar_all_the_way_back_into_the_box() {
        let mut tracker = PitStallTracker::default();
        assert!(tracker.update(tick()).visible);
        // Rolled straight through the box without stopping.
        assert!(tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, ..tick() }).visible);
        let overshot = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 9.0 / 4000.0), ..tick() });
        assert!(overshot.visible, "nine metres past the marks still needs the bar");
        assert!(overshot.error_m > 0.0, "and must read as long, not short");

        // Reversing back toward the marks.
        let backing_up = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 4.0 / 4000.0), ..tick() });
        assert!(backing_up.visible);
        let nearly = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 1.5 / 4000.0), ..tick() });
        assert!(nearly.visible);

        // Home, and stopped: now it goes.
        let parked =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(0.0), ..tick() });
        assert!(!parked.visible);
    }

    /// `PlayerCarInPitStall` goes true while the car is still rolling in, so
    /// hiding on the flag alone would take the bar away during the very
    /// seconds it is being used to place the car.
    #[test]
    fn rolling_into_the_box_keeps_the_bar_until_the_car_actually_stops() {
        let mut tracker = PitStallTracker::default();
        let rolling =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(3.0), ..tick() });
        assert!(rolling.visible, "still moving in the box: the bar is still doing its job");
        assert!(rolling.in_box);

        let crawling =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(0.8), ..tick() });
        assert!(crawling.visible, "a crawl up the box still counts as moving");

        let stopped =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(0.05), ..tick() });
        assert!(!stopped.visible);
    }

    /// Without a speed to read, "stopped" can never be detected, so the visit
    /// has to end the only other way it can: by the box falling out of range
    /// behind the car.
    #[test]
    fn a_car_publishing_no_speed_still_loses_the_bar_on_the_way_out() {
        let mut tracker = PitStallTracker::default();
        let no_speed = PitStallTick { speed_mps: None, ..tick() };
        assert!(tracker.update(no_speed).visible);
        assert!(tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, ..no_speed }).visible);
        let pulling_away = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 8.0 / 4000.0), ..no_speed });
        assert!(pulling_away.visible, "still in range, so still shown — it cannot know the stop happened");
        let gone = tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + out_of_range_m() / 4000.0), ..no_speed });
        assert!(!gone.visible, "past the appear range, the bar goes on its own");
    }

    #[test]
    fn leaving_pit_road_arms_the_bar_for_a_second_stop_the_same_lap() {
        let mut tracker = PitStallTracker::default();
        tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, speed_mps: Some(0.0), ..tick() });
        assert!(!tracker.update(tick()).visible, "still the same visit");

        tracker.update(PitStallTick { on_pit_road: false, ..tick() });
        assert!(tracker.update(tick()).visible, "a new visit gets a new bar");
    }

    /// A drive-through never stops, so nothing latches; it simply sweeps past
    /// and runs out of range on its own.
    #[test]
    fn a_drive_through_shows_then_clears() {
        let mut tracker = PitStallTracker::default();
        assert!(tracker.update(tick()).visible);
        assert!(tracker.update(PitStallTick { lap_dist_pct: Some(0.1), in_box: true, ..tick() }).visible);
        assert!(tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + 10.0 / 4000.0), ..tick() }).visible);
        assert!(
            !tracker.update(PitStallTick { lap_dist_pct: Some(0.1 + out_of_range_m() / 4000.0), ..tick() }).visible
        );
    }

    /// The sim's own flag outranks the arithmetic, so a car the crew is
    /// working on keeps its bar however far off the computed error says it is.
    #[test]
    fn the_sims_own_in_box_flag_keeps_the_bar_up() {
        let mut tracker = PitStallTracker::default();
        let snapshot =
            tracker.update(PitStallTick { lap_dist_pct: Some(0.1 - 200.0 / 4000.0), in_box: true, ..tick() });
        assert!(snapshot.visible);
        assert!(snapshot.in_box);
    }

    #[test]
    fn the_readout_is_suppressed_when_the_scale_was_guessed() {
        let mut tracker = PitStallTracker::default();
        let guessed = tracker.update(PitStallTick { track_length_m: None, speed_mps: None, ..tick() });
        assert!(guessed.visible, "the bar still places its marker");
        assert!(guessed.readout_m.is_none(), "but must not print a distance");

        let mut tracker = PitStallTracker::default();
        assert!(tracker.update(tick()).readout_m.is_some());
    }

    #[test]
    fn the_measured_scale_converges_on_the_real_lap_length() {
        // A 5000 m lap driven at 20 m/s: 0.004 of a lap every tenth of a
        // second. The track-length fallback would say 4000 m.
        let mut scale = MetresPerPct::default();
        let mut pct = 0.0_f32;
        let mut time = 0.0_f64;
        for _ in 0..20 {
            scale.update(pct, Some(20.0), time, true);
            pct += 20.0 * 0.1 / 5000.0;
            time += 0.1;
        }
        let measured = scale.get().expect("twenty clean samples must produce a median");
        assert!((measured - 5000.0).abs() < 50.0, "expected about 5000 m, got {measured}");
    }

    #[test]
    fn one_wrapped_tick_does_not_move_the_measured_scale() {
        let mut scale = MetresPerPct::default();
        let mut pct = 0.0_f32;
        let mut time = 0.0_f64;
        for i in 0..21 {
            scale.update(pct, Some(20.0), time, true);
            // One tick reports a position from a lap away.
            pct += if i == 10 { 0.5 } else { 20.0 * 0.1 / 5000.0 };
            time += 0.1;
        }
        let measured = scale.get().expect("must have samples");
        assert!((measured - 5000.0).abs() < 50.0, "the median must ignore the outlier, got {measured}");
    }

    #[test]
    fn a_stationary_or_crawling_car_contributes_no_scale_samples() {
        let mut scale = MetresPerPct::default();
        scale.update(0.10, Some(0.0), 0.0, true);
        scale.update(0.10, Some(0.0), 0.1, true);
        assert!(scale.get().is_none());

        // Below the minimum sampling speed, position changes too little a
        // tick for the ratio to mean anything.
        let mut scale = MetresPerPct::default();
        scale.update(0.10, Some(2.0), 0.0, true);
        scale.update(0.100_04, Some(2.0), 0.1, true);
        assert!(scale.get().is_none());
    }

    #[test]
    fn a_stalled_frame_contributes_no_scale_sample() {
        let mut scale = MetresPerPct::default();
        scale.update(0.10, Some(20.0), 0.0, true);
        // Two seconds later: the car did not travel `speed * dt`.
        scale.update(0.12, Some(20.0), 2.0, true);
        assert!(scale.get().is_none());
    }

    #[test]
    fn samples_are_only_taken_on_pit_road() {
        let mut scale = MetresPerPct::default();
        scale.update(0.10, Some(20.0), 0.0, false);
        scale.update(0.104, Some(20.0), 0.1, false);
        assert!(scale.get().is_none());
    }

    #[test]
    fn a_published_stall_is_a_fraction_inside_the_lap() {
        assert!(target_is_published(0.035));
        assert!(!target_is_published(0.0));
        assert!(!target_is_published(-0.1));
        assert!(!target_is_published(1.0));
        assert!(!target_is_published(f32::NAN));
    }
}
