// Rust guideline compliant 2026-02-16

//! Pure gap/lap-wrap math and nearby-car selection for the relative widget.
//! Kept free of telemetry and UI types so it is cheap to unit test.

use iracing_telem::flags::Flags;

use super::snapshot::{CarSnapshot, Penalty};

/// How finely the lap curve is sampled, in track-position bins.
///
/// Every gap on the panel is two readings off this curve, so its resolution
/// sets theirs: 512 bins is a fifth of a percent of a lap, and the straight
/// line drawn between neighbouring bins takes what is left of that error far
/// below the hundredth of a second the widget prints. Raising it costs four
/// bytes a bin and makes a lap slower to qualify, since every bin has to have
/// been driven through before the lap counts.
const CURVE_BINS: usize = 512;

/// The player has crossed the start/finish line when track position falls
/// from beyond this to below [`LINE_JUST_AFTER`].
///
/// A pair of readings either side of the line rather than a bare "position
/// went down", because a tow to the pits, a reset and the drop into a garage
/// stall all move track position backwards from the middle of a lap.
const LINE_JUST_BEFORE: f32 = 0.9;

/// See [`LINE_JUST_BEFORE`].
const LINE_JUST_AFTER: f32 = 0.1;

/// How far outside `0.0..=1.0` a track-position reading may stray and still be
/// pulled back in rather than thrown away.
///
/// `CarIdxLapDistPct` reads slightly negative in the last moments before the
/// start/finish line and slightly over 1.0 just after it. Discarding those
/// readings discards the tick either side of the crossing — which is the exact
/// pair the lap timer is started and stopped by, so the curve never completes
/// a lap, never becomes usable, and every gap silently falls back to the
/// coarsest measure there is. Clamping keeps the crossing; dropping loses it.
const PCT_SLOP: f32 = 0.05;

/// The most track position a single reading may advance and still be treated
/// as the car having driven the stretch in between.
///
/// A twentieth of a lap: two hundred times an honest tick's worth at 60 Hz, so
/// nothing a car does on track comes near it, and small enough that a tow or a
/// rejoin never reaches back far enough to overwrite real readings.
const CURVE_MAX_STEP: f32 = 0.05;

/// Times outside this are not laps — a lap timed across a session restart, a
/// rejoin or a clock reset reads as a fraction of a second or as hours.
const PLAUSIBLE_LAP_SECS: std::ops::RangeInclusive<f64> = 10.0..=1200.0;

/// What fraction of a lap's *time* it takes to reach each point on the lap,
/// measured from the player's own driving.
///
/// This is the yardstick every relative gap is measured against, and it exists
/// because the two obvious ways of measuring one are each wrong in a way a
/// driver can see.
///
/// Scaling the track-position difference (`CarIdxLapDistPct`) by a lap time
/// divides distance by an *average* speed, so it reads the gap high wherever
/// the cars are going faster than that average and low wherever they are going
/// slower. Two cars a steady half-second apart show around a second down a
/// straight and a quarter of that at the apex of a slow corner: the gap
/// appears to collapse under braking and spring back on exit, every corner,
/// though neither car has gained anything.
///
/// Subtracting one car's `CarIdxEstTime` from another's carries the track's
/// speed profile and so does not do that — but each car's est-time is
/// projected against its own reference lap, not the player's. Two reference
/// laps that differ put the whole of that difference into the gap, scaled by
/// how far round the lap the cars are: nothing at the start/finish line,
/// growing steadily all the way round, and snapping back to nothing at the
/// line again. Across classes, where the references differ by seconds a lap,
/// that alone is worth several seconds on a row by the end of one.
///
/// So the curve is timed here instead, off the session clock and the player's
/// own track position, and stored as a *fraction* of the lap rather than as
/// seconds. Three things follow, and all three are the point:
///
/// - It depends on no telemetry whose meaning has to be assumed. A stopwatch
///   and a position are all it reads.
/// - Its period is exactly 1.0, so a pair of cars either side of the
///   start/finish line wrap with no error at all. Measured in seconds the
///   period is a number that has to be estimated, and the estimate lands
///   precisely on the rows a driver is watching most closely.
/// - Holding the shape and the scale apart lets the scale be chosen freely, and
///   getting it right is what makes the gaps the same size iRacing's own panel
///   shows — see [`ReferenceLap`]. A curve held in seconds is stuck with the
///   pace of whichever lap built it instead.
///
/// The fastest clean lap seen so far wins, on the grounds that it is the one
/// least distorted by traffic, lifts and mistakes; a slower lap is measured
/// and thrown away. Only laps driven wholly on track count, so an out-lap, an
/// in-lap and anything with a tow or a reset in it are discarded rather than
/// averaged in.
///
/// The curve describes the track and the car, not a session, so it is worth
/// keeping across a session change.
pub struct LapCurve {
    /// Fraction of the lap's total time taken to reach each bin's centre, from
    /// the best lap measured so far. All `NaN` until one has qualified.
    fraction: Box<[f32]>,
    /// How long that lap took, so a faster one can replace it, and so
    /// `fraction` can be told apart from "nothing measured yet".
    measured_lap_secs: f64,
    /// Seconds from the line to each bin's centre on the lap being driven now.
    /// Normalised into `fraction` if and when the lap completes cleanly.
    pending: Box<[f32]>,
    /// The session clock when the lap being timed began, or `None` when no lap
    /// is being timed — before the first crossing, and after one was spoiled.
    pending_started_secs: Option<f64>,
    /// The previous tick's `(track position, session clock)`.
    last: Option<(f32, f64)>,
}

impl Default for LapCurve {
    fn default() -> Self {
        Self {
            fraction: vec![f32::NAN; CURVE_BINS].into_boxed_slice(),
            measured_lap_secs: 0.0,
            pending: vec![f32::NAN; CURVE_BINS].into_boxed_slice(),
            pending_started_secs: None,
            last: None,
        }
    }
}

/// Summarised rather than derived: the samples are a thousand floats between
/// them, and how long the measured lap was and how much of it is known are the
/// two things worth seeing in a dump.
#[expect(
    clippy::missing_fields_in_debug,
    reason = "the sample arrays and the previous tick's reading are bookkeeping, not state worth dumping"
)]
impl std::fmt::Debug for LapCurve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let known = self.fraction.iter().filter(|s| s.is_finite()).count();
        f.debug_struct("LapCurve")
            .field("measured_lap_secs", &self.measured_lap_secs)
            .field("bins_known", &known)
            .field("timing_a_lap", &self.pending_started_secs.is_some())
            .finish()
    }
}

impl LapCurve {
    /// The lap the curve was measured from, or `None` while it has none and
    /// every gap is coming from the fallback.
    ///
    /// Exposed so the caller can say so once, out loud: a curve that never
    /// qualifies a lap is the difference between gaps good to a hundredth and
    /// gaps that breathe through every corner, and it is otherwise invisible.
    #[must_use]
    pub fn measured_lap_secs(&self) -> Option<f64> {
        (self.measured_lap_secs > 0.0).then_some(self.measured_lap_secs)
    }

    /// Records where the player is now: `pct` of the way round the lap at
    /// `session_secs` on the session clock, with `on_track` false anywhere the
    /// lap being timed should be abandoned.
    ///
    /// `on_track` covers the pit lane, the garage and being out of the world.
    /// A lap that includes any of them is not a lap of the track, and timing
    /// one would write the pit lane's own pace into the bins the main straight
    /// shares with it.
    pub fn observe(&mut self, pct: Option<f32>, session_secs: f64, on_track: bool) {
        let Some(pct) = pct.and_then(usable_pct).filter(|_| session_secs.is_finite() && on_track) else {
            self.spoil();
            return;
        };
        let previous = self.last.replace((pct, session_secs));
        let Some((last_pct, last_secs)) = previous else { return };
        if pct < LINE_JUST_AFTER && last_pct > LINE_JUST_BEFORE {
            self.finish_lap(session_secs);
            return;
        }
        if let Some(started) = self.pending_started_secs {
            self.fill_between((last_pct, last_secs - started), (pct, session_secs - started));
        }
    }

    /// Abandons the lap being timed, keeping the curve already measured.
    ///
    /// For the one thing [`Self::observe`] cannot see for itself: the samples
    /// having started arriving from a different car. A camera switch mid-lap
    /// splices two cars' way around the track into one lap, and only the
    /// splices large enough to trip [`CURVE_MAX_STEP`] are caught on their own.
    pub fn spoil_lap(&mut self) {
        self.spoil();
    }

    /// Abandons the lap being timed. The curve already measured is kept — it is
    /// still the best lap seen, and a stop in the pits does not make it wrong.
    fn spoil(&mut self) {
        self.pending_started_secs = None;
        self.last = None;
    }

    /// Takes the lap just completed, keeps it if it is clean and quicker than
    /// the one held, and starts timing the next either way.
    ///
    /// "Clean" is: a plausible length, and every bin driven through. The second
    /// test is what rejects a lap the car spent partly in the pits or partly
    /// out of the world — [`Self::spoil`] clears the timer for those, so the
    /// bins after it are never written and the lap cannot qualify.
    #[expect(clippy::cast_possible_truncation, reason = "a fraction of a lap is far inside f32's range")]
    fn finish_lap(&mut self, at_secs: f64) {
        if let Some(started) = self.pending_started_secs.take() {
            let lap_secs = at_secs - started;
            let quicker = self.measured_lap_secs <= 0.0 || lap_secs < self.measured_lap_secs;
            if quicker && PLAUSIBLE_LAP_SECS.contains(&lap_secs) && self.pending.iter().all(|s| s.is_finite()) {
                for (fraction, elapsed) in self.fraction.iter_mut().zip(self.pending.iter()) {
                    *fraction = (f64::from(*elapsed) / lap_secs).clamp(0.0, 1.0) as f32;
                }
                self.measured_lap_secs = lap_secs;
            }
        }
        self.pending.fill(f32::NAN);
        self.pending_started_secs = Some(at_secs);
    }

    /// Writes the elapsed time at every bin centre the car has just driven
    /// over.
    ///
    /// Straight to the centres rather than dropping each reading into whichever
    /// bin it lands in, because a reading lands wherever the tick happened to
    /// fall and would then be read back as if it were the centre's. Half a bin
    /// of track position is nothing down a straight and a couple of tenths of a
    /// second through a hairpin, where the car covers very little of the lap
    /// per second — and a couple of tenths is the whole quantity this widget
    /// prints.
    ///
    /// A step of more than [`CURVE_MAX_STEP`], or one that does not advance in
    /// both position and time, fills nothing.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bin indices are far below f64's exact-integer range, the loop bounds keep the index inside the slice, and an elapsed lap time is far inside f32's range"
    )]
    fn fill_between(&mut self, (from_pct, from_secs): (f32, f64), (to_pct, to_secs): (f32, f64)) {
        let span = to_pct - from_pct;
        if span <= 0.0 || span > CURVE_MAX_STEP || to_secs <= from_secs || from_secs < 0.0 {
            return;
        }
        // Bin `i`'s centre sits at `(i + 0.5) / CURVE_BINS`, so in the
        // half-bin-shifted space both ends convert to below, the centres are
        // the whole numbers.
        let from_x = f64::from(from_pct) * CURVE_BINS as f64 - 0.5;
        let to_x = f64::from(to_pct) * CURVE_BINS as f64 - 0.5;
        let mut centre = from_x.floor() + 1.0;
        while centre <= to_x {
            if centre >= 0.0 {
                let index = (centre as usize).min(CURVE_BINS - 1);
                let along = (centre - from_x) / (to_x - from_x);
                self.pending[index] = (from_secs + (to_secs - from_secs) * along) as f32;
            }
            centre += 1.0;
        }
    }

    /// What fraction of a lap it takes to reach `pct`.
    ///
    /// `None` until a lap has been measured. Anchored at both ends — zero at
    /// the start/finish line and one a lap later — which is what makes the wrap
    /// in [`Self::gap_seconds`] exact rather than estimated.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bin indices are far below f32's exact-integer range, and `x` is bounded to 0..CURVE_BINS-1 here"
    )]
    fn fraction_at(&self, pct: f32) -> Option<f32> {
        if self.measured_lap_secs <= 0.0 || !pct.is_finite() {
            return None;
        }
        let x = pct.clamp(0.0, 1.0) * CURVE_BINS as f32 - 0.5;
        let last_bin = CURVE_BINS - 1;
        if x <= 0.0 {
            // Short of the first centre, against the line itself, where no time
            // has passed at all.
            return Some(self.bin(0)? * (x + 0.5) / 0.5);
        }
        if x >= last_bin as f32 {
            // And past the last centre, to a whole lap at the line.
            let last = self.bin(last_bin)?;
            return Some(last + (1.0 - last) * (x - last_bin as f32) / 0.5);
        }
        let lower = x.floor();
        let index = lower as usize;
        let before = self.bin(index)?;
        let after = self.bin(index + 1)?;
        Some(before + (after - before) * (x - lower))
    }

    /// What fraction of a lap's time has passed at `pct`, or `None` until a lap
    /// has been measured.
    ///
    /// Public because fuel burns with time under power rather than with
    /// distance, so the Strategy page's live fuel-save row wants this and not a
    /// track-position fraction — see `telemetry::strategy::lap_progress_litres`.
    #[must_use]
    pub fn lap_fraction(&self, pct: f32) -> Option<f32> {
        self.fraction_at(pct)
    }

    /// One bin's reading, or `None` where nothing has been recorded in it.
    fn bin(&self, index: usize) -> Option<f32> {
        self.fraction.get(index).copied().filter(|fraction| fraction.is_finite())
    }

    /// Seconds from the player at `me_pct` to a car at `other_pct`, positive
    /// when that car is ahead, for a lap currently taking `lap_secs`.
    ///
    /// Which of the two is ahead is settled by track position, whose period is
    /// exactly 1.0; the curve is asked only what fraction of a lap the stretch
    /// between them is worth. That split is what lets a pair straddling the
    /// start/finish line read as the tenth of a second apart they are rather
    /// than the lap their raw curve readings differ by.
    #[must_use]
    pub fn gap_seconds(&self, me_pct: f32, other_pct: f32, lap_secs: f32) -> Option<f32> {
        if lap_secs <= 0.0 {
            return None;
        }
        let me = self.fraction_at(me_pct)?;
        let other = self.fraction_at(other_pct)?;
        let ahead = wrap_shortest(other_pct - me_pct, 1.0);
        let fraction = other - me;
        let fraction = if ahead >= 0.0 && fraction < 0.0 {
            fraction + 1.0
        } else if ahead < 0.0 && fraction > 0.0 {
            fraction - 1.0
        } else {
            fraction
        };
        Some(fraction * lap_secs)
    }
}

/// How far into the lap the player must be before `CarIdxEstTime` can be
/// divided by their place on the curve to recover a reference lap.
///
/// The recovery is that division, and at the start/finish line both sides of it
/// are nearly zero: the answer swings by tens of seconds from tick to tick. A
/// tenth of a lap in, the divisor has settled. Before it, the estimate is
/// simply held — which is what keeps the whole column steady across the line,
/// the one place a driver is watching it most closely.
const REFERENCE_MIN_FRACTION: f32 = 0.1;

/// How far from the player's own pace a recovered reference lap may sit, as a
/// multiple of it, and still be believed.
///
/// This rejects `CarIdxEstTime` being unpublished, zeroed or stale, not a
/// genuinely quick reference lap. iRacing's sits a few percent inside race
/// pace and further inside a slower driver's, so the band has to be wide;
/// anything it lets through is at worst the fallback this replaces.
const REFERENCE_PACE_BAND: std::ops::RangeInclusive<f32> = 0.4..=1.6;

/// How much of each tick's reading is taken into the held estimate.
///
/// Every gap on the panel is scaled by this one number, so taking a reading
/// whole would let one twitchy est-time move the entire column at once. A
/// hundredth settles a fresh estimate inside a couple of seconds of driving,
/// averages away tick noise, and still follows a reference lap that genuinely
/// moves — which it does whenever the session's fastest lap improves.
const REFERENCE_SMOOTHING: f32 = 0.01;

/// The reference lap `CarIdxEstTime` is projected against, measured from the
/// player's own reading rather than assumed.
///
/// [`LapCurve`] carries the *shape* of a lap; something still has to say how
/// many seconds a lap of it is worth, and that scale is what every gap on the
/// panel is multiplied by. The player's own pace is the obvious candidate and
/// is the wrong one: iRacing's own Relative shows est-time differences, and
/// `CarIdxEstTime` is projected against a reference lap of the car's class —
/// quicker than race pace, and quicker again than a slower driver's. Scaling
/// by the player's pace instead reads every gap high by exactly that
/// difference, which for a driver a few seconds off the class pace was ten to
/// sixteen percent. Far enough out that a car half a lap back crossed the
/// wrap-around threshold early and jumped from the bottom of the panel to the
/// top.
///
/// So the reference is measured. `CarIdxEstTime` is seconds from the
/// start/finish line at that reference pace, and the curve says what fraction
/// of a lap's *time* the player is into theirs; one divided by the other is the
/// reference lap itself. Nothing about it has to be assumed, and it tracks
/// whatever iRacing is doing — including a class reference lap that improves
/// mid-session.
///
/// One reference for the whole field, rather than each car's own, is what keeps
/// this clear of the multi-class error in raw est-time differences: two cars in
/// the same place read level whatever they are driving. See [`LapCurve`].
#[derive(Debug, Default)]
pub struct ReferenceLap {
    /// The held estimate in seconds, or `None` until a usable reading has
    /// arrived — before the curve has measured a lap, or before the player has
    /// been [`REFERENCE_MIN_FRACTION`] into one.
    secs: Option<f32>,
}

impl ReferenceLap {
    /// Folds this tick's reading in and returns the lap length to scale gaps by.
    ///
    /// `me_est_time` is the player's `CarIdxEstTime` and `me_lap_fraction`
    /// their place on [`LapCurve`], which is `None` until it has measured a
    /// lap. `pace_secs` bounds what is believed — see [`REFERENCE_PACE_BAND`].
    ///
    /// `None` until a reading has been recovered, leaving the caller to fall
    /// back to the player's pace.
    pub fn observe(&mut self, me_est_time: f32, me_lap_fraction: Option<f32>, pace_secs: f32) -> Option<f32> {
        if let Some(reading) = recovered_reference(me_est_time, me_lap_fraction, pace_secs) {
            self.secs = Some(match self.secs {
                Some(held) => held + (reading - held) * REFERENCE_SMOOTHING,
                None => reading,
            });
        }
        self.secs
    }
}

/// The reference lap one tick's readings imply, or `None` where they cannot
/// support one.
fn recovered_reference(me_est_time: f32, me_lap_fraction: Option<f32>, pace_secs: f32) -> Option<f32> {
    let fraction = me_lap_fraction.filter(|f| *f >= REFERENCE_MIN_FRACTION)?;
    if !me_est_time.is_finite() || me_est_time <= 0.0 || pace_secs <= 0.0 {
        return None;
    }
    let reference = me_est_time / fraction;
    REFERENCE_PACE_BAND.contains(&(reference / pace_secs)).then_some(reference)
}

/// A track position pulled back into `0.0..=1.0`, or `None` if it is too far
/// outside to be a reading at all. See [`PCT_SLOP`].
fn usable_pct(pct: f32) -> Option<f32> {
    if !pct.is_finite() || !(-PCT_SLOP..=1.0 + PCT_SLOP).contains(&pct) {
        return None;
    }
    Some(pct.clamp(0.0, 1.0))
}

/// Gap in seconds from the player to another car, measured around the track.
///
/// Positive means the other car is ahead of the player.
///
/// Read off `curve` wherever it can answer — see [`LapCurve`] for why that is
/// the only one of the three measures here that is right in a multi-class
/// field. Until it can, before the player has completed one clean lap, the
/// track-position difference scaled by `pace_secs` stands in: approximate
/// through the corners, but never a lap wrong.
///
/// The raw est-time difference is the last resort, for a car whose
/// `CarIdxLapDistPct` isn't published at all. It is wrapped by `pace_secs`
/// because it is measured from the start/finish line and so runs most of a lap
/// out for a pair either side of it.
///
/// Lap counts deliberately play no part. A car a lap down sitting alongside
/// the player is alongside them, and that is the only thing this widget is
/// asked; the lap difference is carried separately, for the row's coloring.
///
/// `pace_secs` is how many seconds a lap is worth: [`ReferenceLap`] where it
/// has one, since that is the scale iRacing's own panel prints, and the
/// player's pace before then. A non-positive value means the lap's length isn't
/// known at all yet, leaving only the raw, unwrapped est-time difference.
#[must_use]
pub fn gap_seconds(
    curve: &LapCurve,
    me_lap_dist_pct: Option<f32>,
    other_lap_dist_pct: Option<f32>,
    me_est_time: f32,
    other_est_time: f32,
    pace_secs: f32,
) -> f32 {
    if let (Some(me_pct), Some(other_pct)) = (me_lap_dist_pct, other_lap_dist_pct) {
        if let Some(gap) = curve.gap_seconds(me_pct, other_pct, pace_secs) {
            return gap;
        }
        if pace_secs > 0.0 {
            // One whole lap is 1.0 of track position; converting to seconds
            // afterwards keeps the wrap exact.
            return wrap_shortest(other_pct - me_pct, 1.0) * pace_secs;
        }
    }
    wrap_shortest(other_est_time - me_est_time, pace_secs)
}

/// How many whole laps up (positive) or down (negative) another car is.
///
/// Not simply the `CarIdxLap` difference. That counter ticks at the
/// start/finish line, so any car on the far side of the line from the player
/// reads a lap out while the two are plainly racing the same lap — which put
/// the "different lap" warning behind almost every row of the widget, where
/// it means nothing. Adding each car's fraction of the lap covered gives a
/// continuous distance run instead, and rounding that difference to the
/// nearest whole lap discounts anything under half a lap of track position.
///
/// Falls back to the raw counter difference when track positions aren't
/// published, which is no worse than what it replaces.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "lap counts are far below f32's exact-integer range, and their difference is nowhere near i32's"
)]
pub fn lap_difference(me_lap: i32, me_pct: Option<f32>, other_lap: i32, other_pct: Option<f32>) -> i32 {
    let counter_diff = other_lap - me_lap;
    let Some((me_pct, other_pct)) = me_pct.zip(other_pct) else {
        return counter_diff;
    };
    let laps_apart = counter_diff as f32 + (other_pct - me_pct);
    if laps_apart.is_finite() { laps_apart.round() as i32 } else { counter_diff }
}

/// Folds `delta` into +/- half of `period` — the shortest way round a lap.
///
/// `rem_euclid` rather than a single add or subtract, so a stale or garbage
/// reading several laps out still lands inside the window instead of merely
/// one lap closer to it. A non-positive `period`, or a `delta` that isn't a
/// number, passes straight through: there is nothing to wrap by.
pub fn wrap_shortest(delta: f32, period: f32) -> f32 {
    if period <= 0.0 || !delta.is_finite() {
        return delta;
    }
    let wrapped = delta.rem_euclid(period);
    if wrapped > period / 2.0 { wrapped - period } else { wrapped }
}

/// How much a car must gain on the one ahead of it, in seconds of gap, before
/// the two rows change places.
///
/// Cars stopped alongside each other — a starting grid, a queue in the pit
/// lane, a pack sitting in their boxes — are separated by less track position
/// than `CarIdxLapDistPct` resolves cleanly, so the sign of the difference
/// between them flips from tick to tick and their rows trade places at the
/// frame rate. This is the band inside which that difference is treated as
/// no difference at all.
///
/// A twentieth of a second is a car length or two at racing speed: far below
/// anything a driver would notice being ordered wrongly, and far above the
/// tick-to-tick wobble between two cars that are not moving. Raising it makes
/// the panel calmer at the cost of holding a real change-over longer; the
/// cost is paid per place moved, since passing two rows needs twice this.
const ORDER_HYSTERESIS_SECS: f32 = 0.05;

/// The previous tick's row order, so the next one can be held steady against
/// it. See [`ORDER_HYSTERESIS_SECS`] for what it is steadying.
#[derive(Debug, Default)]
pub struct RowOrder {
    /// Each car's place in the last order produced, by `CarIdx`.
    ranks: std::collections::HashMap<i32, usize>,
}

/// Orders the whole field in display order: farthest ahead first, down
/// through the player, to farthest behind.
///
/// `cars` must include the player's own entry. The whole field is returned
/// rather than the handful of rows that fit on screen, because the widget is
/// scrollable — the rows outside the current window are exactly what
/// scrolling reveals, and slicing here would leave nothing to scroll into.
///
/// `previous` carries the last tick's order and is updated to this one. Rows
/// are held in the order they were in unless a car has gained more than
/// [`ORDER_HYSTERESIS_SECS`] per place on the cars it would pass, which stops
/// stationary cars trading rows every frame. A car that has genuinely gone by
/// is worth far more than that and moves immediately.
///
/// Cars whose gap isn't a finite number are dropped. A NaN compares false
/// against everything, so leaving one in would put a row at an arbitrary
/// point in the order with no indication of why.
#[must_use]
pub fn order_by_gap(cars: &[CarSnapshot], previous: &mut RowOrder) -> Vec<CarSnapshot> {
    // Ordered by reference until the last step: a `CarSnapshot` carries four
    // strings and this runs on every telemetry tick.
    let mut ordered: Vec<&CarSnapshot> = cars.iter().filter(|c| c.gap_to_player_secs.is_finite()).collect();

    // Where each car would sit on gap alone. That is the answer for a car the
    // last tick knew nothing about — one that has just joined, or come back
    // into the world — which has no place of its own to be held in.
    ordered.sort_by(|a, b| b.gap_to_player_secs.total_cmp(&a.gap_to_player_secs).then(a.car_idx.cmp(&b.car_idx)));

    // Each car is handicapped by the place it already held, so passing the car
    // ahead costs one hysteresis band and the one beyond it costs two. Because
    // the handicap rises strictly with that place, this stays a total order:
    // the comparison is over one number per car, not a "close enough to count
    // as equal" test, which is not transitive and could reorder the field from
    // tick to tick on its own — the very thing being fixed.
    #[expect(clippy::cast_precision_loss, reason = "a field is tens of cars, exact in f32")]
    let keys: Vec<f32> = ordered
        .iter()
        .enumerate()
        .map(|(rank, car)| {
            let held = previous.ranks.get(&car.car_idx).copied().unwrap_or(rank);
            car.gap_to_player_secs - held as f32 * ORDER_HYSTERESIS_SECS
        })
        .collect();
    let mut places: Vec<usize> = (0..ordered.len()).collect();
    // `car_idx` breaks what's left, so two cars that really are level settle on
    // one order rather than following whichever way the telemetry listed them.
    places.sort_by(|&a, &b| keys[b].total_cmp(&keys[a]).then(ordered[a].car_idx.cmp(&ordered[b].car_idx)));

    previous.ranks = places.iter().enumerate().map(|(rank, &i)| (ordered[i].car_idx, rank)).collect();
    places.into_iter().map(|i| ordered[i].clone()).collect()
}

/// How far the window can actually move from the player, in rows.
///
/// The stored scroll is clamped to this, not just the window it produces.
/// Clamping only the window lets the count keep climbing against a view that
/// is already pinned to the end of the field — so the header claims `+9`
/// while nothing moves, and the next nine presses back the other way appear
/// to do nothing at all. Returns `(0, 0)` when the whole field already fits.
#[must_use]
pub fn scroll_bounds(len: usize, focus_index: usize, ahead: usize, behind: usize) -> (i32, i32) {
    let span = ahead.saturating_add(behind).saturating_add(1);
    if len <= span {
        return (0, 0);
    }
    let unscrolled_start = i64::try_from(focus_index).unwrap_or(0) - i64::try_from(ahead).unwrap_or(0);
    let last_start = i64::try_from(len - span).unwrap_or(0);
    let min = i32::try_from(-unscrolled_start).unwrap_or(i32::MIN);
    let max = i32::try_from(last_start - unscrolled_start).unwrap_or(i32::MAX);
    (min.min(0), max.max(0))
}

/// Which slice of the ordered field to draw.
///
/// Centred `scroll` rows away from the player: negative looks further ahead
/// (toward the front of the list), positive further behind. Also clamped to
/// the field, so a stale scroll can never produce blank rows, and a field
/// shorter than the window always returns all of it.
#[must_use]
pub fn window(len: usize, focus_index: usize, ahead: usize, behind: usize, scroll: i32) -> std::ops::Range<usize> {
    if len == 0 {
        return 0..0;
    }
    let span = ahead.saturating_add(behind).saturating_add(1).min(len);
    let centre = i64::try_from(focus_index).unwrap_or(0) + i64::from(scroll);
    let first = centre - i64::try_from(ahead).unwrap_or(0);
    let last_possible = i64::try_from(len - span).unwrap_or(0);
    let start = usize::try_from(first.clamp(0, last_possible)).unwrap_or(0);
    start..start + span
}

/// The black flag a car is carrying, from its `CarIdxSessionFlags` bits.
///
/// Reduced to the single most severe flag: iRacing raises a black flag with
/// its pit-serviceable bit alongside, and a car can be under a slowdown while
/// a meatball is out, but a gutter has one slot per row and the driver wants
/// the one that matters. The course-wide bits — yellow, caution, blue — are
/// not penalties and are ignored here.
#[must_use]
pub fn penalty_from_flags(bits: i32) -> Option<Penalty> {
    // The SDK publishes the bitfield as a signed int; the bits are what they
    // are either way.
    #[expect(clippy::cast_sign_loss, reason = "a bitfield reinterpreted, not a quantity converted")]
    let flags = Flags::from_bits_truncate(bits as u32);
    if flags.contains(Flags::DISQUALIFY) {
        Some(Penalty::Disqualified)
    } else if flags.contains(Flags::BLACK) {
        Some(Penalty::BlackFlag)
    } else if flags.contains(Flags::REPAIR) {
        Some(Penalty::Repair)
    } else if flags.contains(Flags::FURLED) {
        Some(Penalty::Slowdown)
    } else {
        None
    }
}

/// The course flag the black box's status border shows, from the session-wide
/// `SessionFlags` bits.
///
/// Reduced to the four the border has a colour for, most-final first: the
/// chequered ends everything, the white flag is the last lap, and a caution or
/// yellow (waving or held) is the caution state. Everything else — start
/// lights, ten-to-go, blue and red — is not a border state and reads as green.
#[must_use]
pub fn course_flag_from_bits(bits: i32) -> crate::telemetry::snapshot::CourseFlag {
    use crate::telemetry::snapshot::CourseFlag;
    // The SDK publishes the bitfield as a signed int; the bits are what they
    // are either way.
    #[expect(clippy::cast_sign_loss, reason = "a bitfield reinterpreted, not a quantity converted")]
    let flags = Flags::from_bits_truncate(bits as u32);
    if flags.contains(Flags::CHECKERED) {
        CourseFlag::Checkered
    } else if flags.contains(Flags::WHITE) {
        CourseFlag::White
    } else if flags.intersects(Flags::YELLOW | Flags::YELLOW_WAVING | Flags::CAUTION | Flags::CAUTION_WAVING) {
        CourseFlag::Yellow
    } else {
        CourseFlag::Green
    }
}

#[cfg(test)]
mod tests {
    use iracing_telem::flags::TrackLocation;

    use super::*;
    use std::sync::Arc;

    /// Track positions alone against a curve that has measured nothing — the
    /// fallback path.
    fn gap(me_pct: f32, other_pct: f32, lap_secs: f32) -> f32 {
        gap_seconds(&LapCurve::default(), Some(me_pct), Some(other_pct), 0.0, 0.0, lap_secs)
    }

    /// iRacing's telemetry rate, which is what the curve is sampled at.
    const TICK_HZ: f64 = 60.0;

    /// A curve measured from a lap driven at an evenly rising speed: the car is
    /// twice as quick over the line as it was at the end of the last straight,
    /// so the same second of gap spans very different fractions of the lap at
    /// different points on it.
    ///
    /// Driven tick by tick through [`LapCurve::observe`] exactly as a real lap
    /// is, including the reading that reads slightly *negative* just before the
    /// line — which is what iRacing actually publishes there, and what the
    /// crossing has to survive for the curve to ever measure a lap at all.
    fn measured_curve(lap_secs: f64) -> LapCurve {
        let mut curve = LapCurve::default();
        // Two laps: the first starts the timer at its end, the second is the
        // one measured.
        drive_a_lap(&mut curve, lap_secs, 0.0);
        drive_a_lap(&mut curve, lap_secs, lap_secs);
        curve
    }

    /// Feeds one lap of readings, ending with the crossing onto the next.
    fn drive_a_lap(curve: &mut LapCurve, lap_secs: f64, started_at: f64) {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a lap is a few thousand ticks")]
        let ticks = (lap_secs * TICK_HZ) as usize;
        for tick in 0..ticks {
            #[expect(clippy::cast_precision_loss, reason = "a few thousand ticks are exact in f64")]
            let elapsed = tick as f64 / TICK_HZ;
            curve.observe(Some(pct_after(elapsed, lap_secs)), started_at + elapsed, true);
        }
        // The last reading of the lap, a whisker short of the line, where
        // `CarIdxLapDistPct` reads a shade below zero rather than a shade below
        // one — then the first of the next lap, which is the crossing.
        curve.observe(Some(-0.000_02), started_at + lap_secs - 0.5 / TICK_HZ, true);
        curve.observe(Some(0.000_01), started_at + lap_secs, true);
    }

    /// Where the car is after `elapsed` seconds of a `lap_secs` lap, for the
    /// evenly-rising-speed profile: position goes as the square root of time.
    #[expect(clippy::cast_possible_truncation, reason = "a track position is far inside f32's range")]
    fn pct_after(elapsed: f64, lap_secs: f64) -> f32 {
        (elapsed / lap_secs).clamp(0.0, 1.0).sqrt() as f32
    }

    /// The inverse: how long into the lap the car at `pct` is.
    fn seconds_to(pct: f32, lap_secs: f32) -> f32 {
        lap_secs * pct * pct
    }

    /// And where on the lap the car `seconds` into it sits.
    fn pct_at(seconds: f32, lap_secs: f32) -> f32 {
        (seconds / lap_secs).sqrt()
    }

    /// A lap has to be measurable at all before any of the rest matters. The
    /// crossing is detected across a reading that reads slightly negative, as
    /// iRacing's own does — dropping those readings instead of pulling them
    /// back into range is what left the curve permanently unmeasured, and every
    /// gap on the panel coming from the coarsest fallback there is.
    #[test]
    fn a_clean_lap_is_measured_across_a_negative_reading_at_the_line() {
        let curve = measured_curve(90.0);
        let measured = curve.measured_lap_secs().expect("a clean lap must be measured");
        assert!((measured - 90.0).abs() < 0.05, "expected a ~90s lap, got {measured}");
    }

    /// The gap this widget exists to show: a steady half-second, held through a
    /// braking zone.
    ///
    /// Track position alone cannot do this. The follower is 35 m back at 250
    /// km/h and 14 m back at 100 km/h for the same half-second, so scaling
    /// distance by an average lap speed makes the gap appear to collapse into
    /// every corner and spring back out of it.
    #[test]
    fn a_steady_gap_holds_through_a_braking_zone() {
        let curve = measured_curve(90.0);
        // The same half-second, once where the pair are going slowly and once
        // where they are going twice as fast: 1.1% of the lap apart in the
        // first place and 0.3% in the second.
        for me_secs in [5.625_f32, 72.9] {
            let me = pct_at(me_secs, 90.0);
            let other = pct_at(me_secs + 0.5, 90.0);
            let gap = gap_seconds(&curve, Some(me), Some(other), 0.0, 0.0, 90.0);
            assert!((gap - 0.5).abs() < 0.02, "expected 0.5 at {me_secs}s round the lap, got {gap}");
        }
    }

    /// A pair either side of the start/finish line read almost a whole lap
    /// apart on the curve. Track position settles which way round they are and
    /// the curve supplies only the distance — and because the curve is held as
    /// a fraction of a lap, the wrap is exactly 1.0 rather than a number that
    /// has to be estimated.
    #[test]
    fn a_pair_across_the_line_reads_as_the_seconds_it_is() {
        let curve = measured_curve(90.0);
        let me = 0.999_f32;
        let other = pct_at(seconds_to(me, 90.0) + 0.4 - 90.0, 90.0);
        let gap = gap_seconds(&curve, Some(me), Some(other), 0.0, 0.0, 90.0);
        assert!((gap - 0.4).abs() < 0.02, "expected ~0.4 across the line, got {gap}");
        let behind = gap_seconds(&curve, Some(other), Some(me), 0.0, 0.0, 90.0);
        assert!((behind + 0.4).abs() < 0.02, "expected ~-0.4 the other way, got {behind}");
    }

    /// Two cars in the same place are in the same place, whatever they are
    /// driving. This is the multi-class error that reading both off one curve
    /// exists to remove: their own `CarIdxEstTime` figures are projected
    /// against different reference laps and disagree by seconds here.
    #[test]
    fn cars_at_the_same_track_position_are_level() {
        let curve = measured_curve(90.0);
        for pct in [0.0_f32, 0.05, 0.5, 0.87, 0.999] {
            let gap = gap_seconds(&curve, Some(pct), Some(pct), 88.0, 103.0, 90.0);
            assert!(gap.abs() < 0.01, "expected level at {pct}, got {gap}");
        }
    }

    /// The curve holds a lap's *shape*; the pace passed in is what turns a
    /// fraction of it into seconds. A driver lapping ten percent off the lap
    /// the curve was measured from has gaps ten percent larger, which is what
    /// their stopwatch would say too.
    #[test]
    fn gaps_scale_with_the_pace_they_are_asked_for() {
        let curve = measured_curve(90.0);
        let me = pct_at(45.0, 90.0);
        let other = pct_at(46.0, 90.0);
        let at_reference = gap_seconds(&curve, Some(me), Some(other), 0.0, 0.0, 90.0);
        let ten_percent_slower = gap_seconds(&curve, Some(me), Some(other), 0.0, 0.0, 99.0);
        assert!((at_reference - 1.0).abs() < 0.02, "expected ~1.0 at reference pace, got {at_reference}");
        assert!((ten_percent_slower - 1.1).abs() < 0.03, "expected ~1.1 ten percent slower, got {ten_percent_slower}");
    }

    /// A quicker lap is a cleaner lap — less traffic, fewer lifts — so it
    /// replaces the one held. A slower one is measured and thrown away.
    #[test]
    fn only_a_quicker_lap_replaces_the_curve() {
        let mut curve = LapCurve::default();
        drive_a_lap(&mut curve, 95.0, 0.0);
        drive_a_lap(&mut curve, 95.0, 95.0);
        let first = curve.measured_lap_secs().expect("a lap must be measured");

        drive_a_lap(&mut curve, 99.0, 190.0);
        let after_slower = curve.measured_lap_secs().expect("the measured lap must survive a slower one");
        assert!((after_slower - first).abs() < f64::EPSILON, "a slower lap replaced the curve");

        drive_a_lap(&mut curve, 92.0, 289.0);
        let after_quicker = curve.measured_lap_secs().expect("a lap must be measured");
        assert!(after_quicker < first, "a quicker lap did not replace the curve");
    }

    /// Nothing measured yet — the opening lap — so the scaled track-position
    /// gap stands in rather than the widget going blank.
    #[test]
    fn an_unmeasured_curve_falls_back_to_track_position() {
        let result = gap_seconds(&LapCurve::default(), Some(0.20), Some(0.30), 18.0, 0.0, 90.0);
        assert!((result - 9.0).abs() < 0.001, "expected the track-position answer, got {result}");
    }

    /// A lap the car spent any part of off the track is not a lap of the
    /// track. Timing one would write the pit lane's own pace into the bins the
    /// main straight shares with it, and every gap read across that stretch
    /// would be wrong for the rest of the session.
    #[test]
    fn a_lap_through_the_pit_lane_is_not_measured() {
        let mut curve = LapCurve::default();
        // A full lap to start the timer, then one with a stretch spent off the
        // track in the middle of it.
        drive_a_lap(&mut curve, 90.0, 0.0);
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a lap is a few thousand ticks")]
        let ticks = (90.0 * TICK_HZ) as usize;
        for tick in 0..ticks {
            #[expect(clippy::cast_precision_loss, reason = "a few thousand ticks are exact in f64")]
            let elapsed = tick as f64 / TICK_HZ;
            let on_track = !(20.0..30.0).contains(&elapsed);
            curve.observe(Some(pct_after(elapsed, 90.0)), 90.0 + elapsed, on_track);
        }
        curve.observe(Some(0.000_01), 180.0, true);
        assert_eq!(curve.measured_lap_secs(), None, "a lap through the pits was measured");
    }

    /// Track position runs backwards for a tow, a reset and a drop into the
    /// garage as well as for a lap, and only one of those is a lap. Taking the
    /// others would stamp a part-lap in as the whole thing and shrink every gap
    /// on the panel.
    #[test]
    fn being_towed_in_from_mid_lap_is_not_a_lap() {
        let mut curve = LapCurve::default();
        curve.observe(Some(0.60), 32.4, true);
        curve.observe(Some(0.0), 32.5, true);
        assert_eq!(curve.measured_lap_secs(), None, "a tow from the middle of the lap measured a lap");
    }

    /// Settles [`ReferenceLap`] by feeding one reading until the smoothing has
    /// converged, as a lap of driving does.
    fn settled_reference(me_est_time: f32, me_lap_fraction: Option<f32>, pace_secs: f32) -> Option<f32> {
        let mut reference = ReferenceLap::default();
        let mut settled = None;
        // Comfortably more than the smoothing's time constant, so the answer is
        // the reading rather than a point on the way to it.
        for _ in 0..2_000 {
            settled = reference.observe(me_est_time, me_lap_fraction, pace_secs);
        }
        settled
    }

    /// The whole point of [`ReferenceLap`]: iRacing's est-times are projected
    /// against a reference lap quicker than a slower driver's pace, so scaling
    /// the curve by that pace reads every gap on the panel long. Dividing the
    /// player's own est-time by their place on the curve recovers the reference
    /// lap itself.
    #[test]
    fn the_reference_lap_is_recovered_from_the_players_own_est_time() {
        // Half a lap's *time* in, with iRacing reporting 50.5s elapsed: a
        // 101s reference lap, against a driver whose own pace is 113s.
        let reference = settled_reference(50.5, Some(0.5), 113.0).expect("a reference lap must be recovered");
        assert!((reference - 101.0).abs() < 0.1, "expected a ~101s reference lap, got {reference}");
    }

    /// And the gaps that reference lap produces are the ones the in-sim panel
    /// shows, not the ten-percent-long ones the player's own pace gives.
    #[test]
    fn gaps_scaled_by_the_reference_lap_match_the_sim_rather_than_the_players_pace() {
        let curve = measured_curve(101.0);
        let me = pct_at(50.5, 101.0);
        // A third of a lap's time up the road, which at the reference pace is
        // a shade under 34 seconds.
        let other = pct_at(50.5 + 101.0 / 3.0, 101.0);
        let reference = settled_reference(50.5, Some(0.5), 113.0).expect("a reference lap must be recovered");

        let matched = gap_seconds(&curve, Some(me), Some(other), 0.0, 0.0, reference);
        let at_player_pace = gap_seconds(&curve, Some(me), Some(other), 0.0, 0.0, 113.0);
        assert!((matched - 101.0 / 3.0).abs() < 0.3, "expected ~33.7s at the reference lap, got {matched}");
        assert!(at_player_pace > matched * 1.1, "the player's pace must be the long reading this replaces");
    }

    /// At the start/finish line the recovery divides a number near zero by a
    /// number near zero. Held instead, so the column does not lurch across the
    /// line — which is where a driver is reading it hardest.
    #[test]
    fn the_estimate_is_held_across_the_start_finish_line() {
        let mut reference = ReferenceLap::default();
        for _ in 0..2_000 {
            reference.observe(50.5, Some(0.5), 113.0);
        }
        let settled = reference.observe(50.5, Some(0.5), 113.0).expect("a reference lap must be recovered");
        // A hair past the line: est-time and lap fraction both nearly nothing,
        // and their ratio meaningless.
        for fraction in [0.0_f32, 0.001, 0.02, 0.09] {
            let held = reference.observe(0.02, Some(fraction), 113.0).expect("the estimate must be held");
            assert!((held - settled).abs() < f32::EPSILON, "the estimate moved at fraction {fraction}");
        }
    }

    /// Nothing to divide by until the curve has measured a lap, and nothing to
    /// divide when iRacing isn't publishing an est-time. Either way the caller
    /// falls back to the player's pace rather than getting a wrong scale.
    #[test]
    fn an_unusable_reading_recovers_nothing() {
        assert_eq!(settled_reference(50.5, None, 113.0), None, "no curve, no reference");
        assert_eq!(settled_reference(0.0, Some(0.5), 113.0), None, "an unpublished est-time recovered a reference");
        assert_eq!(settled_reference(f32::NAN, Some(0.5), 113.0), None, "a garbage est-time recovered a reference");
        assert_eq!(settled_reference(50.5, Some(0.5), 0.0), None, "no pace, nothing to sanity-check against");
    }

    /// A reading that implies a lap nothing like the player's own is a stale or
    /// zeroed est-time, not a quick reference lap, and taking it would rescale
    /// the whole panel at once.
    #[test]
    fn a_reading_far_from_the_players_pace_is_not_believed() {
        // A tenth of the player's pace, and three times it.
        assert_eq!(settled_reference(5.0, Some(0.5), 113.0), None, "an implausibly quick reference was believed");
        assert_eq!(settled_reference(170.0, Some(0.5), 113.0), None, "an implausibly slow reference was believed");
    }

    #[test]
    fn a_car_further_round_the_lap_is_ahead_by_that_fraction_of_it() {
        assert!((gap(0.20, 0.30, 90.0) - 9.0).abs() < 0.001);
    }

    #[test]
    fn a_car_just_across_the_line_reads_as_barely_ahead_not_a_lap_behind() {
        // The player is a fiftieth of a lap from the line; the other car
        // crossed it a hundredth ago. It is 2.7s ahead, not 87s behind.
        let result = gap(0.98, 0.01, 90.0);
        assert!((result - 2.7).abs() < 0.001, "expected ~2.7, got {result}");
    }

    #[test]
    fn a_car_not_yet_across_the_line_reads_as_barely_behind() {
        let result = gap(0.01, 0.98, 90.0);
        assert!((result - (-2.7)).abs() < 0.001, "expected ~-2.7, got {result}");
    }

    #[test]
    fn a_gap_never_exceeds_half_a_lap() {
        // Whatever the inputs say — including a stale reading several laps
        // out — the shortest way round is at most half a lap.
        for other in [0.0, 0.2, 0.49, 0.51, 0.99, 3.4, -2.0] {
            let result = gap(0.0, other, 90.0);
            assert!(result.abs() <= 45.0 + 0.001, "expected |gap| <= 45.0 at pct {other}, got {result}");
        }
    }

    #[test]
    fn lap_counts_do_not_enter_into_it() {
        // A car forty laps up but sitting nine seconds up the road is nine
        // seconds up the road; the widget is asked where cars are, not what
        // the scoreboard says. Nothing in the signature can express a lap
        // count, which is the point.
        assert!((gap(0.50, 0.60, 90.0) - 9.0).abs() < 0.001);
    }

    #[test]
    fn est_times_stand_in_when_track_positions_are_missing() {
        // Same two cars, no `CarIdxLapDistPct`: the est-time difference is
        // wrapped the same way.
        let curve = measured_curve(90.0);
        assert!((gap_seconds(&curve, None, None, 88.0, 1.0, 90.0) - 3.0).abs() < 0.001);
        assert!((gap_seconds(&curve, Some(0.5), None, 88.0, 1.0, 90.0) - 3.0).abs() < 0.001);
    }

    #[test]
    fn a_car_across_the_start_finish_line_is_still_on_the_players_lap() {
        // The player is a hundredth of a lap short of the line on lap 10; the
        // car just ahead has crossed onto lap 11. Same racing lap.
        assert_eq!(lap_difference(10, Some(0.99), 11, Some(0.01)), 0);
        assert_eq!(lap_difference(11, Some(0.01), 10, Some(0.99)), 0);
    }

    #[test]
    fn a_car_genuinely_a_lap_down_still_reads_a_lap_down() {
        // Alongside the player on track, but a whole lap behind on count.
        assert_eq!(lap_difference(10, Some(0.30), 9, Some(0.30)), -1);
        assert_eq!(lap_difference(10, Some(0.30), 12, Some(0.32)), 2);
    }

    #[test]
    fn without_track_positions_the_raw_counters_are_all_there_is() {
        assert_eq!(lap_difference(10, None, 11, None), 1);
        assert_eq!(lap_difference(10, Some(0.99), 11, None), 1);
    }

    #[test]
    fn an_unknown_lap_length_disables_wrapping() {
        let gap = gap_seconds(&LapCurve::default(), Some(0.01), Some(0.98), 100.0, 10.0, 0.0);
        assert!((gap - (-90.0)).abs() < f32::EPSILON);
    }

    fn car(gap: f32) -> CarSnapshot {
        car_at(0, gap)
    }

    fn car_at(car_idx: i32, gap: f32) -> CarSnapshot {
        CarSnapshot {
            car_idx,
            cust_id: None,
            position: 1,
            track_location: TrackLocation::OnTrack,
            gap_to_player_secs: gap,
            driver_name: Arc::from(""),
            car_number: Arc::from(""),
            car_screen_name: Arc::from(""),
            irating: 0,
            flair_id: 0,
            license_color: Arc::from(""),
            car_class_color: Arc::from(""),
            is_fastest_overall: false,
            irating_change_estimate: None,
            is_focus: false,
            off_tracks: 0,
            lap_diff: 0,
            best_recent_lap_secs: None,
            recent_laps: [None; 3],
            penalty: None,
        }
    }

    #[test]
    fn the_course_flag_reduces_to_its_border_state_most_final_first() {
        use crate::telemetry::snapshot::CourseFlag;
        assert_eq!(course_flag_from_bits(Flags::GREEN.bits().cast_signed()), CourseFlag::Green);
        assert_eq!(course_flag_from_bits(Flags::YELLOW.bits().cast_signed()), CourseFlag::Yellow);
        assert_eq!(course_flag_from_bits(Flags::CAUTION_WAVING.bits().cast_signed()), CourseFlag::Yellow);
        assert_eq!(course_flag_from_bits(Flags::WHITE.bits().cast_signed()), CourseFlag::White);
        // The chequered outranks a white still set beside it on the last lap.
        let last_lap_then_finish = (Flags::WHITE | Flags::CHECKERED).bits();
        assert_eq!(course_flag_from_bits(last_lap_then_finish.cast_signed()), CourseFlag::Checkered);
        // A blue or start flag is not a border state.
        assert_eq!(course_flag_from_bits(Flags::BLUE.bits().cast_signed()), CourseFlag::Green);
    }

    #[test]
    fn a_black_flag_outranks_the_bits_raised_beside_it() {
        let black_and_serviceable = (Flags::BLACK | Flags::SERVICABLE).bits();
        assert_eq!(penalty_from_flags(black_and_serviceable.cast_signed()), Some(Penalty::BlackFlag));
        let furled_under_a_meatball = (Flags::FURLED | Flags::REPAIR).bits();
        assert_eq!(penalty_from_flags(furled_under_a_meatball.cast_signed()), Some(Penalty::Repair));
        assert_eq!(penalty_from_flags(Flags::DISQUALIFY.bits().cast_signed()), Some(Penalty::Disqualified));
        assert_eq!(penalty_from_flags(Flags::FURLED.bits().cast_signed()), Some(Penalty::Slowdown));
    }

    /// The course-wide flags are not this car's problem.
    #[test]
    fn course_flags_are_not_penalties() {
        let green_and_blue = (Flags::GREEN | Flags::BLUE | Flags::CAUTION).bits();
        assert_eq!(penalty_from_flags(green_and_blue.cast_signed()), None);
        assert_eq!(penalty_from_flags(0), None);
    }

    #[test]
    fn a_car_with_no_usable_gap_is_dropped_rather_than_silently_lost() {
        let ordered = order_by_gap(&[car(2.0), car(f32::NAN), car(-1.0)], &mut RowOrder::default());
        assert_eq!(ordered.len(), 2);
    }

    #[test]
    fn the_field_reads_from_farthest_ahead_down_to_farthest_behind() {
        let ordered = order_by_gap(&[car(5.0), car(1.0), car(-2.0), car(-8.0), car(3.0)], &mut RowOrder::default());
        let gaps: Vec<f32> = ordered.iter().map(|c| c.gap_to_player_secs).collect();
        assert_eq!(gaps, vec![5.0, 3.0, 1.0, -2.0, -8.0]);
    }

    /// The order this returns, by `CarIdx`.
    fn order_of(cars: &[CarSnapshot], previous: &mut RowOrder) -> Vec<i32> {
        order_by_gap(cars, previous).iter().map(|c| c.car_idx).collect()
    }

    /// Two cars stopped alongside each other sit within the noise of
    /// `CarIdxLapDistPct`, so the sign of the gap between them flips from tick
    /// to tick. Left alone, their rows trade places at the frame rate — the
    /// thing this hysteresis exists to stop.
    #[test]
    fn stationary_cars_within_the_noise_do_not_trade_rows() {
        let mut previous = RowOrder::default();
        let first = order_of(&[car_at(7, 0.004), car_at(3, -0.004)], &mut previous);
        assert_eq!(first, vec![7, 3]);

        // The same two cars, the difference between them having flipped sign,
        // as it does every few ticks while neither is moving.
        for wobble in [-0.004_f32, 0.006, -0.002, 0.005] {
            let now = order_of(&[car_at(7, wobble), car_at(3, -wobble)], &mut previous);
            assert_eq!(now, vec![7, 3], "rows moved on a {wobble}s wobble");
        }
    }

    /// Holding rows still must not hold a real overtake still. Anything a
    /// driver would call a change of position clears the band several times
    /// over.
    #[test]
    fn a_real_change_of_position_still_moves_the_row() {
        let mut previous = RowOrder::default();
        assert_eq!(order_of(&[car_at(7, 0.2), car_at(3, -0.2)], &mut previous), vec![7, 3]);
        assert_eq!(order_of(&[car_at(7, -0.4), car_at(3, 0.4)], &mut previous), vec![3, 7]);
    }

    /// Passing two rows at once has to beat two bands, not one, so a car
    /// cannot creep up the order on noise alone.
    #[test]
    fn the_band_is_paid_for_every_place_moved() {
        let mut previous = RowOrder::default();
        let field = |a: f32, b: f32, c: f32| [car_at(1, a), car_at(2, b), car_at(3, c)];
        assert_eq!(order_of(&field(1.0, 0.0, -1.0), &mut previous), vec![1, 2, 3]);

        // Car 3 comes up level with car 1: two places, so it needs to be more
        // than 2 x 0.05 clear of it, and it is not.
        assert_eq!(order_of(&field(1.0, 0.0, 1.04), &mut previous), vec![1, 3, 2], "one place, not two");
        // Now it is.
        assert_eq!(order_of(&field(1.0, 0.0, 1.2), &mut previous), vec![3, 1, 2]);
    }

    /// A car the last tick never saw has no place to be held in, so it lands
    /// where its gap puts it rather than at one end of the field.
    #[test]
    fn a_car_joining_the_field_lands_on_its_gap() {
        let mut previous = RowOrder::default();
        assert_eq!(order_of(&[car_at(1, 5.0), car_at(2, -5.0)], &mut previous), vec![1, 2]);
        assert_eq!(order_of(&[car_at(1, 5.0), car_at(9, 0.0), car_at(2, -5.0)], &mut previous), vec![1, 9, 2]);
    }

    /// Two cars genuinely level settle on a fixed order rather than depending
    /// on which order the telemetry arrays happened to hand them over in.
    #[test]
    fn cars_exactly_level_settle_rather_than_swapping() {
        let mut previous = RowOrder::default();
        assert_eq!(order_of(&[car_at(4, 0.0), car_at(2, 0.0)], &mut previous), vec![2, 4]);
        assert_eq!(order_of(&[car_at(2, 0.0), car_at(4, 0.0)], &mut previous), vec![2, 4]);
    }

    #[test]
    fn an_unscrolled_window_is_centred_on_the_player() {
        // Twenty cars, player tenth, three either side.
        assert_eq!(window(20, 10, 3, 3, 0), 7..14);
    }

    #[test]
    fn scrolling_moves_the_window_and_stops_at_both_ends() {
        assert_eq!(window(20, 10, 3, 3, -2), 5..12, "negative scroll looks further ahead");
        assert_eq!(window(20, 10, 3, 3, 2), 9..16);
        // Past the front of the field, and past the back.
        assert_eq!(window(20, 10, 3, 3, -100), 0..7);
        assert_eq!(window(20, 10, 3, 3, 100), 13..20);
    }

    #[test]
    fn a_field_shorter_than_the_window_is_shown_whole_and_never_scrolls() {
        for scroll in [-5, 0, 5] {
            assert_eq!(window(4, 1, 3, 3, scroll), 0..4);
        }
    }

    #[test]
    fn the_scroll_bounds_are_what_the_window_can_actually_reach() {
        // Twenty cars, player tenth, seven rows visible: three rows of travel
        // toward the front, and the rest toward the back.
        assert_eq!(scroll_bounds(20, 10, 3, 3), (-7, 6));
        // Nothing to scroll when the whole field already fits.
        assert_eq!(scroll_bounds(4, 1, 3, 3), (0, 0));
        // A player at the very back can only look forward.
        assert_eq!(scroll_bounds(20, 19, 3, 3), (-16, 0));
    }

    #[test]
    fn scrolling_to_the_bound_reaches_the_end_of_the_field() {
        let (min, max) = scroll_bounds(20, 10, 3, 3);
        assert_eq!(window(20, 10, 3, 3, min), 0..7);
        assert_eq!(window(20, 10, 3, 3, max), 13..20);
    }

    #[test]
    fn an_empty_field_yields_an_empty_window() {
        assert_eq!(window(0, 0, 3, 3, 0), 0..0);
    }
}
