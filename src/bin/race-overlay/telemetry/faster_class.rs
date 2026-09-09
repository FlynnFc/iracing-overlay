// Rust guideline compliant 2026-02-16

//! Pure logic for the Faster Class widget: which class is quicker, how fast a
//! car from it is closing, and when that is worth a warning.
//!
//! Kept free of telemetry and UI types so it is cheap to unit test — see
//! `plans/faster-class.md` for the decisions. The telemetry thread uses the
//! first half (class speed, sightings, closing rates) to build the list of
//! approaching cars; the UI thread uses the second half ([`Thresholds`],
//! [`Alarm`]) to turn that list into a level, so that the two thresholds a
//! driver sets take effect as the slider moves rather than at the next start.
//!
//! # Units
//!
//! Every gap here is **seconds of relative time behind the focus car**,
//! positive behind and negative once the car is ahead — the same figure the
//! Relative prints beside each row, with the sign flipped so that the number a
//! driver is warned about is a positive one. Seconds rather than metres because
//! that is how a driver thinks about approaching traffic, and because it needs
//! no track length to be honest.

use std::collections::{HashMap, VecDeque};

/// What is known about how quick one class is.
///
/// Three sources of decreasing authority: iRacing's own ranking, iRacing's
/// estimated lap, and the quickest lap actually seen. See
/// [`is_faster_class`] for how they are weighed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassPace {
    /// `CarClassID`. Two cars in the same class are never faster traffic to
    /// each other, whatever their laps.
    pub class_id: i32,
    /// `CarClassRelSpeed`: iRacing's integer ranking, higher being faster.
    /// `None` where the YAML omits it or writes zero.
    pub rel_speed: Option<i32>,
    /// `CarClassEstLapTime` in seconds, `None` where absent or non-positive.
    pub est_lap_secs: Option<f32>,
    /// The quickest `CarIdxBestLapTime` seen in the class this session.
    pub best_lap_secs: Option<f32>,
}

/// How much quicker a measured best lap has to be before it says anything
/// about class speed, as a fraction.
///
/// Two percent is about a second and a half on a typical lap: more than the
/// spread between a quick and a slow driver in one class would ever be read
/// as, less than the gap between any two classes that share a grid.
const MEASURED_MARGIN: f32 = 0.02;

/// Two estimated class laps closer than this are the same class speed.
///
/// iRacing's estimates are round figures, and half a second between two
/// classes is not a difference a driver would feel as traffic.
const EST_LAP_TIE_SECS: f32 = 0.5;

/// Whether `other`'s class is quicker than `mine`.
///
/// Same class is never faster. Otherwise the first source both classes have
/// an answer for decides: the ranking, then the estimated lap, then the
/// measured lap with [`MEASURED_MARGIN`] in hand. With nothing to go on the
/// answer is no — a warning built on a guess is worse than none.
#[must_use]
pub fn is_faster_class(other: ClassPace, mine: ClassPace) -> bool {
    if other.class_id == mine.class_id {
        return false;
    }
    if let (Some(theirs), Some(ours)) = (other.rel_speed, mine.rel_speed)
        && theirs != ours
    {
        return theirs > ours;
    }
    if let (Some(theirs), Some(ours)) = (positive(other.est_lap_secs), positive(mine.est_lap_secs))
        && (theirs - ours).abs() > EST_LAP_TIE_SECS
    {
        return theirs < ours;
    }
    if let (Some(theirs), Some(ours)) = (positive(other.best_lap_secs), positive(mine.best_lap_secs)) {
        return theirs < ours * (1.0 - MEASURED_MARGIN);
    }
    false
}

/// A lap time that is actually a lap time: finite and above zero.
fn positive(secs: Option<f32>) -> Option<f32> {
    secs.filter(|secs| secs.is_finite() && *secs > 0.0)
}

// ---- Sightings -----------------------------------------------------------

/// How far behind the focus car the scan reaches, in seconds.
///
/// The widest warning the settings page offers, so everything the sliders can
/// ask for is already in the list the telemetry thread sends.
pub const SCAN_BEHIND_SECS: f32 = 15.0;

/// How far *ahead* a car that has just gone past is still reported.
///
/// Wider than [`PASSED_SECS`], where the UI stops showing it, so the UI can
/// apply its own hysteresis at that line rather than having the car vanish
/// from the list underneath it.
pub const SCAN_AHEAD_SECS: f32 = 1.0;

/// Whether a gap is one the telemetry thread should report at all.
#[must_use]
pub fn in_scan(behind_secs: f32) -> bool {
    behind_secs.is_finite() && (-SCAN_AHEAD_SECS..=SCAN_BEHIND_SECS).contains(&behind_secs)
}

/// How far back the closing-rate window reaches, in seconds.
///
/// Longer than the radar's, deliberately: a gap read off the lap curve at
/// three seconds' separation breathes through corners in a way a gap at half
/// a car length does not, and a rate measured over a fraction of a second of
/// that would be jitter with a sign.
pub const CLOSING_WINDOW_SECS: f64 = 4.0;

/// One car's recent gaps, keyed by `CarIdx`, for measuring how fast it closes.
///
/// Keyed per car rather than per slot so that a different car arriving nearer
/// can never be measured as one car moving — the radar guards the same case
/// with a rate limit, which this does not need.
#[derive(Debug, Default)]
pub struct ClosingRates {
    /// `(session time, seconds behind)`, oldest first, per car.
    histories: HashMap<i32, VecDeque<(f64, f32)>>,
}

impl ClosingRates {
    /// Records this tick's sightings and forgets every car not among them.
    ///
    /// `seen` is `(car index, seconds behind)`. A tick whose clock has not
    /// moved on discards the car's window rather than dividing by it.
    pub fn update(&mut self, seen: &[(i32, f32)], time_secs: f64) {
        self.histories.retain(|car_idx, _| seen.iter().any(|(seen_idx, _)| seen_idx == car_idx));
        for &(car_idx, behind_secs) in seen {
            let history = self.histories.entry(car_idx).or_default();
            if history.back().is_some_and(|&(last, _)| time_secs <= last) {
                history.clear();
            }
            history.push_back((time_secs, behind_secs));
            // Keep exactly one sample at or behind the window's trailing
            // edge, so the measured interval spans the whole window rather
            // than shrinking to whatever arrived inside it.
            while history.len() > 2 && history.get(1).is_some_and(|&(t, _)| time_secs - t >= CLOSING_WINDOW_SECS) {
                history.pop_front();
            }
        }
    }

    /// Seconds of gap this car loses per second; negative while it drops back.
    ///
    /// `None` until two samples separated in time exist, which the widget
    /// reads as "no arrival to project" — the safe default.
    #[must_use]
    #[expect(clippy::cast_possible_truncation, reason = "the window is CLOSING_WINDOW_SECS wide, well inside f32")]
    pub fn rate(&self, car_idx: i32) -> Option<f32> {
        let history = self.histories.get(&car_idx)?;
        let (&(old_time, old_gap), &(new_time, new_gap)) = history.front().zip(history.back())?;
        let dt = new_time - old_time;
        if dt <= 0.0 {
            return None;
        }
        Some((old_gap - new_gap) / dt as f32)
    }
}

/// The slowest closing rate worth projecting an arrival from, in s/s.
///
/// Below this the two cars are holding station, and the figure would be
/// minutes with a tilde in front of them.
const MIN_CLOSING_RATE: f32 = 0.02;

/// The longest arrival worth printing, in seconds.
///
/// A minute is the far end of what a driver plans a lap around; past it the
/// figure says only "not yet", which the amber already says.
const MAX_ARRIVAL_SECS: f32 = 60.0;

/// Seconds until the car is on the focus car's bumper, where that is honest.
///
/// `None` for a car not closing, closing too slowly to say, or more than a
/// minute out — and for a car already alongside, where there is nothing left
/// to project.
#[must_use]
pub fn arrival_secs(behind_secs: f32, closing_rate: Option<f32>) -> Option<f32> {
    let rate = closing_rate.filter(|rate| rate.is_finite() && *rate >= MIN_CLOSING_RATE)?;
    if !behind_secs.is_finite() || behind_secs <= PASSING_SECS {
        return None;
    }
    let arrival = behind_secs / rate;
    (arrival <= MAX_ARRIVAL_SECS).then_some(arrival)
}

// ---- Levels --------------------------------------------------------------

/// The gap at which the card appears, unless the settings say otherwise.
///
/// Between a prototype's twenty seconds of closing on a GT3 and a GTE's
/// minute: long enough to plan where to let the car by, short enough that the
/// card is not up for most of a stint in a busy field.
pub const DEFAULT_WARN_SECS: f32 = 4.0;

/// The gap at which the card turns red, unless the settings say otherwise.
///
/// Roughly the point at which the faster car is committing to a move rather
/// than merely catching up.
pub const DEFAULT_ALERT_SECS: f32 = 1.5;

/// The widest warning the settings page allows, in seconds.
pub const MAX_WARN_SECS: f32 = SCAN_BEHIND_SECS;

/// Within this many seconds behind, the car is alongside rather than coming.
pub const PASSING_SECS: f32 = 0.4;

/// This far ahead, the car has gone by and the widget's job is done.
pub const PASSED_SECS: f32 = 0.5;

/// How far past a threshold the gap has to grow before a level steps back
/// down, in seconds.
///
/// A gap read off the lap curve breathes by a tenth or two through a corner;
/// three tenths is comfortably past that and still well inside what a driver
/// would notice as lag.
const HYSTERESIS_SECS: f32 = 0.3;

/// The two gaps a driver sets: where the warning starts and where it turns
/// into an alert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub warn_secs: f32,
    pub alert_secs: f32,
}

impl Thresholds {
    /// Sanitises a pair of configured gaps.
    ///
    /// A warning that is not a positive number takes the default, and an
    /// alert set further out than the warning is clamped to it — the alert
    /// escalates the warning, so it cannot come first.
    #[must_use]
    pub fn new(warn_secs: f32, alert_secs: f32) -> Self {
        let warn_secs = if warn_secs.is_finite() && warn_secs > 0.0 { warn_secs } else { DEFAULT_WARN_SECS };
        let alert_secs = if alert_secs.is_finite() && alert_secs > 0.0 { alert_secs.min(warn_secs) } else { 0.0 };
        Self { warn_secs, alert_secs }
    }
}

/// How urgent the nearest faster car is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Inside the warning threshold: a factor, not yet on you.
    Warn,
    /// Inside the alert threshold: on you.
    Alert,
    /// Alongside, or just gone by.
    Passing,
}

/// The car the widget is about this frame, and how urgent it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject {
    /// Its index in the list handed to [`Alarm::update`].
    pub index: usize,
    pub level: Level,
}

/// Turns the nearest-first list of approaching cars into a level, holding
/// each boundary through [`HYSTERESIS_SECS`] on the way out.
///
/// Lives on the UI thread, one per widget, so the thresholds it is given can
/// change between frames.
#[derive(Debug, Default)]
pub struct Alarm {
    /// The car the widget was about last frame.
    car_idx: Option<i32>,
    level: Option<Level>,
    /// A car that has gone by. It cannot become the subject again until it is
    /// genuinely behind — more than [`PASSING_SECS`] back — so a passer that
    /// is blocked alongside for a moment does not bring the card back as it
    /// wanders across the [`PASSED_SECS`] line.
    dismissed: Option<i32>,
}

impl Alarm {
    /// Advances by one frame.
    ///
    /// `cars` is `(car index, seconds behind)`, nearest first — the order the
    /// telemetry thread sends. Returns the subject and its level, or `None`
    /// when nothing is worth drawing.
    pub fn update(&mut self, cars: &[(i32, f32)], thresholds: Thresholds) -> Option<Subject> {
        let mut candidate = None;
        for (index, &(car_idx, behind_secs)) in cars.iter().enumerate() {
            if behind_secs < -PASSED_SECS {
                if self.car_idx == Some(car_idx) {
                    self.dismissed = Some(car_idx);
                }
                continue;
            }
            if self.dismissed == Some(car_idx) {
                if behind_secs > PASSING_SECS {
                    self.dismissed = None;
                } else {
                    continue;
                }
            }
            candidate = Some((index, car_idx, behind_secs));
            break;
        }
        let Some((index, car_idx, behind_secs)) = candidate else {
            self.clear();
            return None;
        };

        let same = self.car_idx == Some(car_idx);
        let holding = |level: Level| same && self.level == Some(level);
        let level = if behind_secs <= PASSING_SECS {
            Level::Passing
        } else if behind_secs <= thresholds.alert_secs
            || (holding(Level::Alert) && behind_secs <= thresholds.alert_secs + HYSTERESIS_SECS)
        {
            Level::Alert
        } else if behind_secs <= thresholds.warn_secs
            || (same && self.level.is_some() && behind_secs <= thresholds.warn_secs + HYSTERESIS_SECS)
        {
            Level::Warn
        } else {
            self.clear();
            return None;
        };
        self.car_idx = Some(car_idx);
        self.level = Some(level);
        Some(Subject { index, level })
    }

    fn clear(&mut self) {
        self.car_idx = None;
        self.level = None;
    }
}

/// How many faster cars besides the subject are inside the warning window
/// and still behind.
#[must_use]
pub fn others_within(cars: &[(i32, f32)], subject_index: usize, warn_secs: f32) -> usize {
    cars.iter()
        .enumerate()
        .filter(|&(index, &(_, behind_secs))| {
            index != subject_index && behind_secs > PASSING_SECS && behind_secs <= warn_secs
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pace(class_id: i32, rel_speed: Option<i32>, est_lap_secs: Option<f32>, best_lap_secs: Option<f32>) -> ClassPace {
        ClassPace { class_id, rel_speed, est_lap_secs, best_lap_secs }
    }

    const DEFAULTS: Thresholds = Thresholds { warn_secs: DEFAULT_WARN_SECS, alert_secs: DEFAULT_ALERT_SECS };

    #[test]
    fn the_same_class_is_never_faster_traffic() {
        let quick = pace(7, Some(90), Some(90.0), Some(89.0));
        let slow = pace(7, Some(50), Some(100.0), Some(101.0));
        assert!(!is_faster_class(quick, slow));
    }

    #[test]
    fn the_sims_own_ranking_decides_first() {
        let lmp2 = pace(1, Some(65), Some(95.0), None);
        let gt3 = pace(2, Some(55), Some(104.0), None);
        assert!(is_faster_class(lmp2, gt3));
        assert!(!is_faster_class(gt3, lmp2));
        // The ranking wins even where a measured lap says otherwise.
        let slow_today = pace(1, Some(65), None, Some(120.0));
        let quick_today = pace(2, Some(55), None, Some(100.0));
        assert!(is_faster_class(slow_today, quick_today));
    }

    #[test]
    fn the_estimated_lap_decides_when_the_ranking_cannot() {
        let gte = pace(1, None, Some(99.0), None);
        let gt3 = pace(2, None, Some(104.0), None);
        assert!(is_faster_class(gte, gt3));
        assert!(!is_faster_class(gt3, gte));
        // A tied ranking falls through to the estimate too.
        assert!(is_faster_class(pace(1, Some(50), Some(99.0), None), pace(2, Some(50), Some(104.0), None)));
        // Half a second is not a class difference.
        assert!(!is_faster_class(pace(1, None, Some(103.6), None), pace(2, None, Some(104.0), None)));
    }

    #[test]
    fn measured_laps_need_the_margin() {
        let a = pace(1, None, None, Some(100.0));
        let b = pace(2, None, None, Some(103.0));
        assert!(is_faster_class(a, b), "three seconds on a hundred is a class apart");
        assert!(!is_faster_class(b, a));
        let close = pace(1, None, None, Some(102.0));
        assert!(!is_faster_class(close, b), "one second on a hundred is a quick driver, not a quick class");
    }

    #[test]
    fn nothing_known_is_not_faster() {
        assert!(!is_faster_class(pace(1, None, None, None), pace(2, None, None, None)));
        assert!(!is_faster_class(pace(1, Some(60), None, None), pace(2, None, Some(100.0), None)));
        assert!(!is_faster_class(pace(1, None, Some(0.0), Some(0.0)), pace(2, None, Some(100.0), Some(100.0))));
    }

    #[test]
    fn the_scan_covers_behind_and_a_little_ahead() {
        assert!(in_scan(0.0));
        assert!(in_scan(SCAN_BEHIND_SECS));
        assert!(in_scan(-SCAN_AHEAD_SECS));
        assert!(!in_scan(SCAN_BEHIND_SECS + 0.1));
        assert!(!in_scan(-SCAN_AHEAD_SECS - 0.1));
        assert!(!in_scan(f32::NAN));
    }

    #[test]
    fn closing_rate_is_positive_while_the_gap_shrinks() {
        let mut rates = ClosingRates::default();
        rates.update(&[(4, 3.0)], 0.0);
        rates.update(&[(4, 2.8)], 2.0);
        let rate = rates.rate(4).expect("two samples must yield a rate");
        assert!((rate - 0.1).abs() < 1e-5, "0.2 s in 2 s is 0.1 s/s, got {rate}");
        rates.update(&[(4, 3.2)], 4.0);
        assert!(rates.rate(4).is_some_and(|rate| rate < 0.0), "opening reads negative");
    }

    #[test]
    fn closing_rates_are_per_car_and_forget_cars_that_leave() {
        let mut rates = ClosingRates::default();
        rates.update(&[(4, 3.0), (9, 8.0)], 0.0);
        rates.update(&[(4, 2.5), (9, 8.0)], 1.0);
        assert!(rates.rate(4).is_some_and(|rate| rate > 0.0));
        assert!(rates.rate(9).is_some_and(|rate| rate.abs() < 1e-6));
        rates.update(&[(4, 2.0)], 2.0);
        assert_eq!(rates.rate(9), None, "a car no longer sighted is forgotten");
        assert_eq!(rates.rate(1), None, "a car never sighted has no rate");
    }

    #[test]
    fn one_sample_is_not_a_rate() {
        let mut rates = ClosingRates::default();
        rates.update(&[(4, 3.0)], 0.0);
        assert_eq!(rates.rate(4), None);
        // The same instant again is not a second sample either.
        rates.update(&[(4, 2.0)], 0.0);
        assert_eq!(rates.rate(4), None);
    }

    /// The measured interval must not shrink as ticks arrive at 60 Hz.
    #[test]
    #[expect(clippy::cast_possible_truncation, reason = "a few seconds of simulated ticks, well inside f32")]
    fn the_window_keeps_a_sample_behind_its_trailing_edge() {
        let mut rates = ClosingRates::default();
        for tick in 0..600 {
            let t = f64::from(tick) / 60.0;
            rates.update(&[(4, 10.0 - 0.1 * t as f32)], t);
        }
        let rate = rates.rate(4).expect("a full window must yield a rate");
        assert!((rate - 0.1).abs() < 0.01, "a steady 0.1 s/s must read as such, got {rate}");
    }

    #[test]
    fn arrival_is_the_gap_over_the_rate_where_that_is_honest() {
        let eta = arrival_secs(3.0, Some(0.1)).expect("closing at a real rate projects");
        assert!((eta - 30.0).abs() < 1e-4);
        assert_eq!(arrival_secs(3.0, Some(0.01)), None, "holding station projects nothing");
        assert_eq!(arrival_secs(3.0, Some(-0.1)), None, "dropping back projects nothing");
        assert_eq!(arrival_secs(3.0, None), None);
        assert_eq!(arrival_secs(14.0, Some(0.05)), None, "more than a minute out is not a figure");
        assert_eq!(arrival_secs(0.2, Some(0.5)), None, "alongside has nothing left to project");
    }

    #[test]
    fn thresholds_sanitise_nonsense_and_order_themselves() {
        let t = Thresholds::new(f32::NAN, 1.0);
        assert!((t.warn_secs - DEFAULT_WARN_SECS).abs() < f32::EPSILON);
        let t = Thresholds::new(0.0, 1.0);
        assert!((t.warn_secs - DEFAULT_WARN_SECS).abs() < f32::EPSILON);
        let t = Thresholds::new(3.0, 5.0);
        assert!((t.alert_secs - 3.0).abs() < f32::EPSILON, "the alert cannot come before the warning");
        let t = Thresholds::new(3.0, -1.0);
        assert!(t.alert_secs.abs() < f32::EPSILON);
    }

    #[test]
    fn the_alarm_appears_at_the_warning_and_escalates_at_the_alert() {
        let mut alarm = Alarm::default();
        assert_eq!(alarm.update(&[(4, 6.0)], DEFAULTS), None, "beyond the warning is nothing");
        assert_eq!(alarm.update(&[(4, 4.0)], DEFAULTS), Some(Subject { index: 0, level: Level::Warn }));
        assert_eq!(alarm.update(&[(4, 1.5)], DEFAULTS), Some(Subject { index: 0, level: Level::Alert }));
        assert_eq!(alarm.update(&[(4, 0.3)], DEFAULTS), Some(Subject { index: 0, level: Level::Passing }));
        assert_eq!(alarm.update(&[(4, -0.4)], DEFAULTS), Some(Subject { index: 0, level: Level::Passing }));
        assert_eq!(alarm.update(&[(4, -0.6)], DEFAULTS), None, "gone by");
    }

    #[test]
    fn the_alarm_holds_a_level_through_the_hysteresis() {
        let mut alarm = Alarm::default();
        alarm.update(&[(4, 1.4)], DEFAULTS);
        assert_eq!(alarm.update(&[(4, 1.7)], DEFAULTS).map(|s| s.level), Some(Level::Alert), "inside the hold");
        assert_eq!(alarm.update(&[(4, 1.9)], DEFAULTS).map(|s| s.level), Some(Level::Warn), "past it");
        assert_eq!(alarm.update(&[(4, 4.2)], DEFAULTS).map(|s| s.level), Some(Level::Warn), "inside the hold");
        assert_eq!(alarm.update(&[(4, 4.4)], DEFAULTS), None, "past it");
        // No hold on the way in: a fresh car at 4.2 is not shown.
        assert_eq!(alarm.update(&[(4, 4.2)], DEFAULTS), None);
    }

    /// A passer blocked alongside wanders across the "gone by" line; the card
    /// must not come back each time it does.
    #[test]
    fn a_dismissed_car_does_not_return_until_it_is_genuinely_behind() {
        let mut alarm = Alarm::default();
        alarm.update(&[(4, 1.0)], DEFAULTS);
        alarm.update(&[(4, -0.3)], DEFAULTS);
        assert_eq!(alarm.update(&[(4, -0.6)], DEFAULTS), None);
        assert_eq!(alarm.update(&[(4, -0.4)], DEFAULTS), None, "back across the line, still dismissed");
        assert_eq!(alarm.update(&[(4, 0.2)], DEFAULTS), None, "alongside again, still dismissed");
        assert_eq!(alarm.update(&[(4, 0.8)], DEFAULTS).map(|s| s.level), Some(Level::Alert), "genuinely behind");
    }

    #[test]
    fn a_dismissed_car_does_not_hide_the_one_behind_it() {
        let mut alarm = Alarm::default();
        alarm.update(&[(4, 0.2), (9, 3.0)], DEFAULTS);
        alarm.update(&[(4, -0.7), (9, 2.8)], DEFAULTS);
        assert_eq!(alarm.update(&[(4, -0.4), (9, 2.6)], DEFAULTS), Some(Subject { index: 1, level: Level::Warn }));
    }

    #[test]
    fn a_nearer_car_takes_over_as_the_subject() {
        let mut alarm = Alarm::default();
        assert_eq!(alarm.update(&[(9, 3.0)], DEFAULTS), Some(Subject { index: 0, level: Level::Warn }));
        // The list is nearest first, so the new car is at the front.
        assert_eq!(alarm.update(&[(4, 1.0), (9, 3.0)], DEFAULTS), Some(Subject { index: 0, level: Level::Alert }));
        // And the hold belongs to the car, not the slot: the new subject at
        // 1.7 is not inside an alert it never had.
        let mut fresh = Alarm::default();
        fresh.update(&[(9, 1.0)], DEFAULTS);
        assert_eq!(fresh.update(&[(4, 1.7), (9, 3.0)], DEFAULTS).map(|s| s.level), Some(Level::Warn));
    }

    #[test]
    fn an_empty_list_clears_the_alarm() {
        let mut alarm = Alarm::default();
        alarm.update(&[(4, 1.0)], DEFAULTS);
        assert_eq!(alarm.update(&[], DEFAULTS), None);
        assert_eq!(alarm.update(&[(4, 4.2)], DEFAULTS), None, "no hold survives an empty frame");
    }

    #[test]
    fn others_counts_the_rest_of_the_window() {
        let cars = [(4, 1.0), (9, 3.0), (2, 3.9), (7, 8.0), (5, 0.2)];
        assert_eq!(others_within(&cars, 0, DEFAULT_WARN_SECS), 2);
        assert_eq!(others_within(&cars, 1, DEFAULT_WARN_SECS), 2, "the subject itself is not an other");
        assert_eq!(others_within(&[(4, 1.0)], 0, DEFAULT_WARN_SECS), 0);
    }
}
