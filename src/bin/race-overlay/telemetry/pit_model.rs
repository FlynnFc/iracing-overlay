// Rust guideline compliant 2026-02-16

//! What a pit stop costs on this track, in this car — measured rather than
//! looked up, because iRacing publishes no pit lane geometry at all.
//!
//! The point of a model rather than an average is that it can price a stop
//! nobody has taken yet. "What does a splash and no tyres cost?" cannot be
//! answered by observing past stops, because past stops were all the same kind;
//! it can be answered by splitting a stop into a lane transit and a service and
//! recombining them. See `plans/strategy-and-timings.md`.
//!
//! Only the transit is measured here. It falls out of every stop for free — see
//! [`PitLossTracker`] — so the model starts sharpening itself in races without
//! anything being set up. The service figures are defaults until the Timings
//! Collector measures them, and the model says which is which.

/// Seconds a pit lane costs before any service, when nothing has been measured.
///
/// A middling road-course figure. iRacing's lanes run from about fifteen seconds
/// to about fifty, so this is wrong everywhere — which is why it only ever
/// covers the very first stop of a session, after which
/// [`PitLossTracker`] has measured the real one.
const DEFAULT_TRANSIT_LOSS_SECS: f32 = 30.0;

/// Seconds a stop costs before any fuel flows: the rig being connected and
/// disconnected, and the crew's own reaction either side of it.
const DEFAULT_OVERHEAD_SECS: f32 = 1.5;

/// Seconds per litre, roughly 3.3 L/s — typical for iRacing's GT content.
const DEFAULT_FILL_RATE_SECS_PER_LITRE: f32 = 0.30;

/// Seconds for a full set of four.
const DEFAULT_TYRE_CHANGE_SECS: f32 = 12.0;

/// How many measurements the transit loss is taken as the median of.
///
/// Five is enough that one bad visit — a lane held up behind another car, a
/// penalty served on the way through — cannot decide the figure, and short
/// enough to follow a genuine change: a different session at the same track can
/// have a different lane speed limit.
const TRANSIT_SAMPLES: usize = 5;

/// Losses outside this are not a pit visit.
///
/// Below three seconds nothing was driven through; above three minutes the car
/// was parked, penalised, or in the garage, and none of those is a lane transit.
const PLAUSIBLE_TRANSIT_SECS: std::ops::RangeInclusive<f32> = 3.0..=180.0;

/// What a stop of a given shape costs, in seconds lost against staying out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitModel {
    /// Driving through the lane, with no service at all: the deceleration in,
    /// the crawl at the limiter, and the acceleration out, over and above what
    /// that stretch of track would have taken at racing pace.
    pub transit_loss_secs: f32,
    /// Seconds a stop costs before any fuel flows.
    pub overhead_secs: f32,
    pub fill_rate_secs_per_litre: f32,
    /// A full set of four.
    pub tyre_change_secs: f32,
    /// Whether fuel and tyres are serviced at the same time — iRacing's crews
    /// do — so a stop costs the longer of the two rather than their sum.
    pub concurrent: bool,
    /// Clean measurements the lane transit rests on. Zero means it is a default.
    pub transit_runs: u8,
    /// Clean measurements the *service* figures rest on. Zero until the Timings
    /// Collector has been run, which is most of the time.
    pub service_runs: u8,
}

impl Default for PitModel {
    fn default() -> Self {
        Self {
            transit_loss_secs: DEFAULT_TRANSIT_LOSS_SECS,
            overhead_secs: DEFAULT_OVERHEAD_SECS,
            fill_rate_secs_per_litre: DEFAULT_FILL_RATE_SECS_PER_LITRE,
            tyre_change_secs: DEFAULT_TYRE_CHANGE_SECS,
            concurrent: true,
            transit_runs: 0,
            service_runs: 0,
        }
    }
}

impl PitModel {
    /// Seconds a stop taking on `litres` — and tyres, if `tyres` — would cost.
    ///
    /// Asking for no fuel costs no fuel time at all, not the overhead: a
    /// tyres-only stop never connects the rig.
    #[must_use]
    pub fn stop_cost_secs(&self, litres: f32, tyres: bool) -> f32 {
        let fuel = if litres > 0.0 { self.overhead_secs + litres * self.fill_rate_secs_per_litre } else { 0.0 };
        let tyre = if tyres { self.tyre_change_secs } else { 0.0 };
        let service = if self.concurrent { fuel.max(tyre) } else { fuel + tyre };
        self.transit_loss_secs + service
    }

    /// Whether any figure here is still a default rather than a measurement.
    ///
    /// The UI shows a provisional model muted, so a driver can tell the
    /// difference between a number measured on this track and a guess.
    #[must_use]
    pub fn is_provisional(self) -> bool {
        self.transit_runs == 0 || self.service_runs == 0
    }
}

/// Where the car is, as a pit-lane measurement cares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanePhase {
    /// Anywhere on the circuit, including off it.
    OnTrack,
    /// On pit road but moving.
    Lane,
    /// Stopped in the box, being serviced.
    Stall,
    /// Not in the world — the garage, a disconnect, a session change. Whatever
    /// visit was under way is not a lane transit and is abandoned.
    Away,
}

/// One visit to the pit lane, while it is still in progress.
#[derive(Debug, Clone, Copy)]
struct Visit {
    /// The session clock as the car left the track.
    at_secs: f64,
    /// What fraction of a lap it had covered at that moment, or `None` if the
    /// lap curve could not say — in which case the visit cannot be measured.
    from_fraction: Option<f32>,
    /// Seconds spent stationary in the box so far.
    stationary_secs: f64,
    /// The clock at the last tick, so stationary time accumulates.
    last_secs: f64,
}

/// Measures what the pit lane costs, from every visit to it.
///
/// No drill and no track database. The trick is that the lap curve already
/// knows how long any stretch of track takes at racing pace, so the loss is
/// simply how much longer the car actually took:
///
/// ```text
/// total  = elapsed − (lap fraction covered) × pace
/// transit = total − time spent stationary
/// ```
///
/// Subtracting the stationary time is what makes this a *transit* figure rather
/// than a total, and so what makes it reusable: a stop with different service
/// on it has the same transit and a different stationary time, which is exactly
/// the split [`PitModel`] recombines.
#[derive(Debug, Default)]
pub struct PitLossTracker {
    visit: Option<Visit>,
    /// Recent transit measurements, oldest first; bounded to
    /// [`TRANSIT_SAMPLES`].
    transits: std::collections::VecDeque<f32>,
}

impl PitLossTracker {
    /// Follows the car for one tick, returning a transit loss the moment a
    /// visit completes cleanly.
    ///
    /// `lap_fraction` is the player's position as a fraction of a lap's time,
    /// from [`super::relative::LapCurve::lap_fraction`]; `pace_secs` is what a
    /// lap is currently taking. Without both, a visit is watched but not
    /// measured — there is nothing to compare the elapsed time against.
    pub fn update(
        &mut self,
        phase: LanePhase,
        session_secs: f64,
        lap_fraction: Option<f32>,
        pace_secs: f32,
    ) -> Option<f32> {
        if phase == LanePhase::Away {
            self.visit = None;
            return None;
        }
        if phase == LanePhase::OnTrack {
            let visit = self.visit.take()?;
            let transit = transit_loss(&visit, session_secs, lap_fraction, pace_secs)?;
            if self.transits.len() >= TRANSIT_SAMPLES {
                self.transits.pop_front();
            }
            self.transits.push_back(transit);
            return Some(transit);
        }
        let visit = self.visit.get_or_insert(Visit {
            at_secs: session_secs,
            from_fraction: lap_fraction,
            stationary_secs: 0.0,
            last_secs: session_secs,
        });
        // Accumulated tick by tick rather than from a single entry/exit pair,
        // because a car can stop, be serviced, and creep forward more than once
        // in a visit — a penalty served in the box, a stall, a driver swap.
        if phase == LanePhase::Stall {
            visit.stationary_secs += (session_secs - visit.last_secs).max(0.0);
        }
        visit.last_secs = session_secs;
        None
    }

    /// The model as it currently stands: measured where anything has been
    /// measured, and the defaults elsewhere.
    #[must_use]
    pub fn model(&self) -> PitModel {
        let mut model = PitModel::default();
        if let Some(measured) = self.median_transit() {
            model.transit_loss_secs = measured;
            model.transit_runs = u8::try_from(self.transits.len()).unwrap_or(u8::MAX);
        }
        model
    }

    /// The median of the measurements held, or `None` before there are any.
    ///
    /// A median rather than a mean: one visit held up behind another car in the
    /// lane is worth several seconds, and a mean carries a fifth of that into
    /// every projection made afterwards.
    fn median_transit(&self) -> Option<f32> {
        if self.transits.is_empty() {
            return None;
        }
        let mut sorted: Vec<f32> = self.transits.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        sorted.get(sorted.len() / 2).copied()
    }
}

/// What the visit cost over and above driving the same stretch at racing pace,
/// or `None` when it cannot honestly be measured.
fn transit_loss(visit: &Visit, at_secs: f64, to_fraction: Option<f32>, pace_secs: f32) -> Option<f32> {
    let (from, to) = visit.from_fraction.zip(to_fraction)?;
    if pace_secs <= 0.0 {
        return None;
    }
    // The pit lane runs forward along the same track-position axis the circuit
    // does, and at several tracks its exit is past the start/finish line — so
    // the fraction covered wraps. A lane is never a whole lap long, which is
    // what makes the wrap unambiguous.
    let covered = f64::from(to - from).rem_euclid(1.0);
    let elapsed = at_secs - visit.at_secs;
    let at_pace = covered * f64::from(pace_secs);
    let transit = elapsed - at_pace - visit.stationary_secs;
    #[expect(clippy::cast_possible_truncation, reason = "a pit lane's cost in seconds is far inside f32's range")]
    let transit = transit as f32;
    PLAUSIBLE_TRANSIT_SECS.contains(&transit).then_some(transit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// iRacing's telemetry rate, which the tracker is fed at.
    const TICK: f64 = 1.0 / 60.0;

    #[test]
    fn a_default_model_prices_a_stop_without_anything_measured() {
        let model = PitModel::default();
        assert!(model.is_provisional(), "nothing has been measured");
        // 30 s of lane, plus 1.5 s of rig and 40 x 0.3 s of fuel.
        assert!((model.stop_cost_secs(40.0, false) - 43.5).abs() < 0.001, "{model:?}");
    }

    /// A tyres-only stop never connects the rig, so it pays no fuel overhead.
    #[test]
    fn asking_for_no_fuel_costs_no_fuel_time() {
        let model = PitModel::default();
        assert!((model.stop_cost_secs(0.0, true) - 42.0).abs() < 0.001);
        assert!((model.stop_cost_secs(0.0, false) - 30.0).abs() < 0.001, "a drive-through is the lane alone");
    }

    /// iRacing services fuel and tyres together, so a stop costs the longer of
    /// the two — which is the whole reason a strategy page can offer "tyres are
    /// free on this stop".
    #[test]
    fn concurrent_service_costs_the_longer_of_the_two() {
        let model = PitModel::default();
        // 12 s of tyres against 1.5 + 6 = 7.5 s of fuel: the tyres decide it,
        // so the fuel is free.
        let fuel_only = model.stop_cost_secs(20.0, false);
        let both = model.stop_cost_secs(20.0, true);
        assert!(both > fuel_only, "tyres must cost something here");
        assert!((both - model.stop_cost_secs(0.0, true)).abs() < 0.001, "the fuel should have been free");

        let sequential = PitModel { concurrent: false, ..PitModel::default() };
        assert!(sequential.stop_cost_secs(20.0, true) > both, "sequential service must cost more");
    }

    /// Drives one visit: `lane_secs` moving on pit road, `stall_secs` stopped,
    /// then back out on track `covered` of a lap further round, entering at
    /// `from`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a handful of seconds of ticks, from literals in the tests below"
    )]
    fn visit_from(
        tracker: &mut PitLossTracker,
        from: f32,
        lane_secs: f64,
        stall_secs: f64,
        covered: f32,
        pace: f32,
    ) -> Option<f32> {
        let mut now = 100.0_f64;
        tracker.update(LanePhase::Lane, now, Some(from), pace);
        for _ in 0..(lane_secs / TICK) as usize {
            now += TICK;
            tracker.update(LanePhase::Lane, now, Some(from), pace);
        }
        for _ in 0..(stall_secs / TICK) as usize {
            now += TICK;
            tracker.update(LanePhase::Stall, now, Some(from), pace);
        }
        now += TICK;
        tracker.update(LanePhase::OnTrack, now, Some(from + covered), pace)
    }

    /// The same, entering at a track position that does not wrap.
    fn one_visit(
        tracker: &mut PitLossTracker,
        lane_secs: f64,
        stall_secs: f64,
        covered: f32,
        pace: f32,
    ) -> Option<f32> {
        visit_from(tracker, 0.40, lane_secs, stall_secs, covered, pace)
    }

    /// The measurement the whole model rests on, and it needs no drill: how much
    /// longer the lane took than the same stretch would have at racing pace,
    /// less whatever was spent standing still.
    #[test]
    fn a_visit_measures_the_lane_and_not_the_service() {
        let mut tracker = PitLossTracker::default();
        // 40 s in the lane and 25 s stopped, over 6% of a 90 s lap — which is
        // 5.4 s of racing. So 65 s elapsed, minus 5.4 s of track, minus 25 s
        // stationary: about 34.6 s of transit.
        let transit = one_visit(&mut tracker, 40.0, 25.0, 0.06, 90.0).expect("a clean visit must measure");
        assert!((transit - 34.6).abs() < 0.3, "expected ~34.6 s of transit, got {transit}");

        let model = tracker.model();
        assert_eq!(model.transit_runs, 1);
        assert!((model.transit_loss_secs - transit).abs() < 0.001);
        assert!(model.is_provisional(), "the service figures are still defaults");
    }

    /// Two stops with wildly different service must measure the same lane, which
    /// is the property that makes the figure reusable.
    #[test]
    fn the_measured_lane_does_not_move_with_the_service_on_it() {
        let mut tracker = PitLossTracker::default();
        let splash = one_visit(&mut tracker, 40.0, 5.0, 0.06, 90.0).expect("a clean visit");
        let mut tracker = PitLossTracker::default();
        let full = one_visit(&mut tracker, 40.0, 30.0, 0.06, 90.0).expect("a clean visit");
        assert!((splash - full).abs() < 0.3, "a 25 s longer stop moved the lane figure: {splash} vs {full}");
    }

    /// Several tracks put the pit exit past the start/finish line, so the
    /// fraction covered wraps. Unhandled, that reads as most of a lap of racing
    /// and the loss comes out hugely negative — thrown away, and the lane never
    /// measured at those tracks at all.
    #[test]
    fn a_lane_whose_exit_is_past_the_line_still_measures() {
        // Entering at 97% of the lap and rejoining 6% further round, which is 3%
        // into the next one. `0.97 + 0.06` wraps to `0.03`.
        let mut wrapped = PitLossTracker::default();
        let across = visit_from(&mut wrapped, 0.97, 40.0, 25.0, 0.06, 90.0).expect("a wrapped lane was thrown away");

        // And it measures the same lane as the identical visit taken mid-lap,
        // which is the point: the wrap must not change the answer.
        let mut ordinary = PitLossTracker::default();
        let within = visit_from(&mut ordinary, 0.40, 40.0, 25.0, 0.06, 90.0).expect("a clean visit");
        assert!((across - within).abs() < 0.001, "the wrap moved the figure: {across} vs {within}");
    }

    /// Without a lap curve there is nothing to compare the elapsed time
    /// against, so the visit is watched and not measured.
    #[test]
    fn a_visit_without_a_lap_curve_measures_nothing() {
        let mut tracker = PitLossTracker::default();
        tracker.update(LanePhase::Lane, 100.0, None, 90.0);
        tracker.update(LanePhase::Stall, 120.0, None, 90.0);
        assert_eq!(tracker.update(LanePhase::OnTrack, 140.0, Some(0.5), 90.0), None);
        assert_eq!(tracker.model().transit_runs, 0);
        assert!((tracker.model().transit_loss_secs - DEFAULT_TRANSIT_LOSS_SECS).abs() < 0.001);
    }

    /// A trip to the garage is not a lane transit, and taking it as one would
    /// stamp minutes of standing about into the model.
    #[test]
    fn leaving_the_world_abandons_the_visit() {
        let mut tracker = PitLossTracker::default();
        tracker.update(LanePhase::Lane, 100.0, Some(0.4), 90.0);
        tracker.update(LanePhase::Away, 110.0, None, 90.0);
        assert_eq!(tracker.update(LanePhase::OnTrack, 200.0, Some(0.5), 90.0), None);
        assert_eq!(tracker.model().transit_runs, 0);
    }

    /// One visit held up behind another car is worth several seconds, and a mean
    /// would carry a fifth of that into every projection afterwards.
    #[test]
    fn the_lane_figure_is_a_median_not_a_mean() {
        let mut tracker = PitLossTracker::default();
        for lane_secs in [40.0, 40.0, 70.0] {
            let mut visit = PitLossTracker::default();
            let measured = one_visit(&mut visit, lane_secs, 5.0, 0.06, 90.0).expect("a clean visit");
            tracker.transits.push_back(measured);
        }
        let median = tracker.model().transit_loss_secs;
        assert!(median < 45.0, "the outlier decided the figure: {median}");
        assert_eq!(tracker.model().transit_runs, 3);
    }

    /// Only the last few visits count, so a lane that changes between sessions
    /// is followed rather than averaged with the old one forever.
    #[test]
    fn only_the_most_recent_visits_are_kept() {
        let mut tracker = PitLossTracker::default();
        for _ in 0..TRANSIT_SAMPLES + 3 {
            one_visit(&mut tracker, 40.0, 5.0, 0.06, 90.0);
        }
        assert_eq!(tracker.transits.len(), TRANSIT_SAMPLES);
    }
}
