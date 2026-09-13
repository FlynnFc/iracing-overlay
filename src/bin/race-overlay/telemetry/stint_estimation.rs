//! Estimates stint age from incomplete public observations; measured stops stay separate.

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateBasis {
    LapTiming,
    DriverChange,
    StrategyPrior,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StintAge {
    Observed(i32),
    Estimated {
        min: i32,
        max: i32,
        basis: EstimateBasis,
    },
    #[default]
    Unknown,
}

impl StintAge {
    #[must_use]
    pub fn bounds(self) -> Option<(i32, i32)> {
        match self {
            Self::Observed(age) => Some((age, age)),
            Self::Estimated { min, max, .. } => Some((min, max)),
            Self::Unknown => None,
        }
    }

    #[must_use]
    pub fn is_estimated(self) -> bool {
        matches!(self, Self::Estimated { .. })
    }
}

/// A normalized row from the session layer. The scored-lap field is the current valid
/// YAML completed-lap/time pair. Equal times are legal; only its lap advances
/// the lap ledger. The range hint must be learned from measured completed stints.
#[derive(Debug, Clone, Copy)]
pub struct Observation {
    pub car_idx: i32,
    pub class_id: i32,
    pub lap: i32,
    pub scored_lap: Option<(i32, f32)>,
    pub in_world: bool,
    pub in_pit_lane: bool,
    pub driver_id: Option<i32>,
    pub observed_boundary_lap: Option<i32>,
    pub completed_stops: i32,
    pub range_hint: Option<(i32, i32)>,
    pub pit_loss_secs: f32,
}

const CLEAN_SAMPLES: usize = 12;
const MIN_PACE_SAMPLES: usize = 3;
const GAP_SECS: f64 = 2.0;
const MIN_STALE_SECS: f64 = 300.0;
const MIN_UNSEEN_SECS: f64 = 2.0;
const FIELD_WINDOW_SECS: f64 = 30.0;
const NORMAL_RATIO: f32 = 0.08;
const FIELD_SLOW_RATIO: f32 = 0.08;
const MIN_LOSS: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
enum Boundary {
    Observed(i32),
    Estimated { first: i32, last: i32, basis: EstimateBasis },
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    first: i32,
    last: i32,
    loss: f32,
    pit_loss: f32,
}

#[derive(Debug, Clone, Copy)]
struct BlindWindow {
    first_lap: i32,
    last_lap: i32,
}

#[derive(Debug, Default)]
struct Car {
    class_id: Option<i32>,
    last_scored_lap: Option<i32>,
    last_driver: Option<i32>,
    last_driver_lap: Option<i32>,
    last_stop_count: Option<i32>,
    last_observed_boundary: Option<i32>,
    boundary: Option<Boundary>,
    candidate: Option<Candidate>,
    clean_pace: VecDeque<f32>,
    last_score_progress: Option<f64>,
    blind_since: Option<f64>,
    blind_first_lap: Option<i32>,
    blind_last_lap: Option<i32>,
    recent_blind: Option<BlindWindow>,
}

impl Car {
    fn pace(&self) -> Option<f32> {
        (self.clean_pace.len() >= MIN_PACE_SAMPLES).then(|| median(self.clean_pace.iter().copied())).flatten()
    }

    fn add_clean(&mut self, time: f32) {
        if !valid_time(time) {
            return;
        }
        if self.clean_pace.len() == CLEAN_SAMPLES {
            self.clean_pace.pop_front();
        }
        self.clean_pace.push_back(time);
    }

    #[expect(clippy::collapsible_if, reason = "the nested expiry guard keeps the conditional-prior proof legible")]
    fn strategy_prior(&mut self, lap: i32, hint: Option<(i32, i32)>) {
        // A strategy prior is conditional on normal cycles. Once its newest
        // possible exit is more than a normal stint behind us, retaining its
        // old anchor would display a precise-looking impossible age. Start a
        // fresh conditional enumeration at the current late-race distance.
        if let (Some(Boundary::Estimated { last, basis: EstimateBasis::StrategyPrior, .. }), Some((_, maximum))) =
            (self.boundary, valid_range(hint))
        {
            if lap - last > maximum {
                self.boundary = None;
            }
        }
        if self.boundary.is_some() {
            return;
        }
        let Some((minimum, maximum)) = valid_range(hint) else {
            return;
        };
        // Do not invent a history for a first partial stint.
        if lap < minimum.saturating_mul(2) {
            return;
        }
        let mut youngest = i32::MAX;
        let mut oldest = i32::MIN;
        for count in 1..=lap / minimum {
            let earliest_stop = count.saturating_mul(minimum);
            let latest_stop = count.saturating_mul(maximum).min(lap);
            if earliest_stop <= latest_stop {
                let min_age = lap - latest_stop;
                let max_age = lap - earliest_stop;
                // Only normal current stints remain in this prior.
                if min_age <= maximum {
                    youngest = youngest.min(min_age);
                    oldest = oldest.max(max_age);
                }
            }
        }
        if youngest <= oldest {
            self.boundary = Some(Boundary::Estimated {
                first: lap - oldest,
                last: lap - youngest,
                basis: EstimateBasis::StrategyPrior,
            });
        }
    }

    fn age(&self, lap: i32, stale: bool) -> StintAge {
        if stale {
            return StintAge::Unknown;
        }
        match self.boundary {
            Some(Boundary::Observed(start)) if lap >= start => StintAge::Observed(lap - start),
            Some(Boundary::Estimated { first, last, basis }) if lap >= last => {
                StintAge::Estimated { min: lap - last, max: lap - first, basis }
            }
            _ => StintAge::Unknown,
        }
    }
}

/// Bounded per-car unseen-stop estimator.
#[derive(Debug, Default)]
pub struct Estimator {
    cars: HashMap<i32, Car>,
    last_revision: Option<i32>,
    last_now: Option<f64>,
    // A receiver gap invalidates the model but not the fact that a caller may
    // keep publishing an old measured start. Ignore that one repeated value.
    boundaries_before_gap: HashMap<i32, Option<i32>>,
    recent_residuals: HashMap<i32, VecDeque<(f64, i32, f32)>>,
}

impl Estimator {
    pub fn clear(&mut self) {
        self.cars.clear();
        self.last_revision = None;
        self.last_now = None;
        self.boundaries_before_gap.clear();
        self.recent_residuals.clear();
    }

    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one ordered tick carries coupled freshness, absence, scorer, and boundary transitions"
    )]
    #[expect(clippy::collapsible_if, reason = "separate absence and scoring guards preserve transition order")]
    pub fn update(
        &mut self,
        now_secs: f64,
        scoring_revision: Option<i32>,
        observations: &[Observation],
        race_running: bool,
        caution: bool,
    ) -> HashMap<i32, StintAge> {
        if !race_running || !now_secs.is_finite() {
            self.clear();
            return HashMap::new();
        }
        let local_gap = self.last_now.is_some_and(|last| now_secs < last || now_secs - last > GAP_SECS);
        if local_gap {
            self.boundaries_before_gap =
                self.cars.iter().map(|(idx, car)| (*idx, car.last_observed_boundary)).collect();
            self.cars.clear();
            self.last_revision = None;
        }
        self.last_now = Some(now_secs);
        let advanced = match (self.last_revision, scoring_revision) {
            (Some(old), Some(new)) if new < old => {
                self.clear();
                self.last_now = Some(now_secs);
                self.last_revision = Some(new);
                true
            }
            (Some(old), Some(new)) => new > old,
            (None, Some(_)) => true,
            _ => false,
        };
        if let Some(revision) = scoring_revision {
            if self.last_revision.is_none_or(|old| revision > old) {
                self.last_revision = Some(revision);
            }
        }
        let field_slow =
            if advanced && !caution { self.record_field_slowdowns(now_secs, observations) } else { HashSet::new() };
        for class_id in &field_slow {
            self.clear_timing_for_class(*class_id);
        }

        for observation in observations {
            let car = self.cars.entry(observation.car_idx).or_default();
            car.class_id = Some(observation.class_id);
            // A lane boolean is never a synthetic boundary; it is merely
            // retained in the contract alongside absence/scoring freshness.
            let _ = observation.in_pit_lane;
            if observation.in_world {
                let blind = (car.blind_since.take(), car.blind_first_lap.take(), car.blind_last_lap.take());
                if let (Some(start), Some(first_lap), Some(last_lap)) = blind {
                    if now_secs - start >= MIN_UNSEEN_SECS {
                        // The line can be crossed while entering or leaving
                        // the blind interval. Keep only its affected
                        // completed laps, including one adjacent exit lap.
                        car.recent_blind =
                            Some(BlindWindow { first_lap, last_lap: last_lap.max(observation.lap).saturating_add(1) });
                    }
                }
            } else if car.blind_since.is_none() {
                car.blind_since = Some(now_secs);
                car.blind_first_lap = Some(observation.lap.max(0));
                car.blind_last_lap = Some(observation.lap.max(0));
            } else {
                car.blind_last_lap = Some(car.blind_last_lap.unwrap_or_default().max(observation.lap.max(0)));
            }
            let driver_changed = matches!(
                (car.last_driver, observation.driver_id),
                (Some(old), Some(new)) if old != new
            );
            let prior_driver_lap = car.last_driver_lap;
            if observation.driver_id.is_some() {
                car.last_driver = observation.driver_id;
                car.last_driver_lap = Some(observation.lap.max(0));
            }
            let stops_increased = car.last_stop_count.is_some_and(|old| observation.completed_stops > old);
            car.last_stop_count = Some(observation.completed_stops);
            if stops_increased {
                car.boundary = None;
                car.candidate = None;
            }
            if driver_changed {
                let lap = observation.lap.max(0);
                car.boundary = Some(Boundary::Estimated {
                    first: prior_driver_lap.unwrap_or_else(|| lap.saturating_sub(1)).min(lap),
                    last: lap,
                    basis: EstimateBasis::DriverChange,
                });
                car.candidate = None;
            }
            // A long unavailable interval means the former witnessed start
            // can no longer be stated exactly. This creates no stop event.
            if !observation.in_world
                && observation.pit_loss_secs.is_finite()
                && observation.pit_loss_secs > 0.0
                && car
                    .blind_since
                    .is_some_and(|start| now_secs - start >= f64::from(observation.pit_loss_secs.max(0.0)))
                && matches!(car.boundary, Some(Boundary::Observed(_)))
            {
                car.boundary = None;
            }
            if advanced {
                let scored_lap = observation.scored_lap.map_or(observation.lap, |(lap, _)| lap);
                let unseen_window = (!observation.in_world
                    && car.blind_since.is_some_and(|start| now_secs - start >= MIN_UNSEEN_SECS))
                    || car
                        .recent_blind
                        .is_some_and(|window| scored_lap >= window.first_lap && scored_lap <= window.last_lap);
                Self::scored_lap(
                    car,
                    *observation,
                    now_secs,
                    unseen_window,
                    field_slow.contains(&observation.class_id),
                    caution,
                );
                if car.recent_blind.is_some_and(|window| scored_lap > window.last_lap) {
                    car.recent_blind = None;
                }
            }
            let stale_after_gap = self
                .boundaries_before_gap
                .get(&observation.car_idx)
                .is_some_and(|old| *old == observation.observed_boundary_lap);
            if observation.observed_boundary_lap != car.last_observed_boundary && !stale_after_gap {
                car.last_observed_boundary = observation.observed_boundary_lap;
                if let Some(lap) = observation.observed_boundary_lap {
                    car.boundary = Some(Boundary::Observed(lap.max(0)));
                    car.candidate = None;
                }
            } else if stale_after_gap {
                // Prevent the same stale StintState start from reviving exact
                // age on the next telemetry tick.
                car.last_observed_boundary = observation.observed_boundary_lap;
            }
            car.strategy_prior(observation.lap.max(0), observation.range_hint);
        }

        observations
            .iter()
            .map(|observation| {
                let age = self.cars.get(&observation.car_idx).map_or(StintAge::Unknown, |car| {
                    let stale_limit =
                        car.pace().map_or(MIN_STALE_SECS, |pace| MIN_STALE_SECS.max(f64::from(pace) * 2.0 + 30.0));
                    let stale = !observation.in_world
                        && car.blind_since.is_some_and(|start| now_secs - start > stale_limit)
                        && car.last_score_progress.is_none_or(|then| now_secs - then > stale_limit);
                    car.age(observation.lap.max(0), stale)
                });
                (observation.car_idx, age)
            })
            .collect()
    }

    fn record_field_slowdowns(&mut self, now_secs: f64, rows: &[Observation]) -> HashSet<i32> {
        for row in rows {
            let Some((lap, time)) = row.scored_lap else {
                continue;
            };
            let Some(car) = self.cars.get(&row.car_idx) else {
                continue;
            };
            let (Some(previous), Some(pace)) = (car.last_scored_lap, car.pace()) else {
                continue;
            };
            if lap == previous + 1 && valid_time(time) {
                let residuals = self.recent_residuals.entry(row.class_id).or_default();
                residuals.retain(|(then, car_idx, _)| now_secs - then <= FIELD_WINDOW_SECS && *car_idx != row.car_idx);
                if residuals.len() == CLEAN_SAMPLES {
                    residuals.pop_front();
                }
                residuals.push_back((now_secs, row.car_idx, (time - pace) / pace));
            }
        }
        self.recent_residuals
            .iter_mut()
            .filter_map(|(class, values)| {
                values.retain(|(then, _, _)| now_secs - then <= FIELD_WINDOW_SECS);
                let slow = values.iter().filter(|(_, _, value)| *value >= FIELD_SLOW_RATIO).count();
                (values.len() >= 3 && slow * 3 >= values.len() * 2).then_some(*class)
            })
            .collect()
    }

    fn clear_timing_for_class(&mut self, class_id: i32) {
        for car in self.cars.values_mut().filter(|car| car.class_id == Some(class_id)) {
            car.candidate = None;
            if matches!(car.boundary, Some(Boundary::Estimated { basis: EstimateBasis::LapTiming, .. })) {
                car.boundary = None;
            }
        }
    }

    #[expect(
        clippy::collapsible_if,
        reason = "candidate confirmation stays explicitly separate from a normal-lap test"
    )]
    fn scored_lap(
        car: &mut Car,
        row: Observation,
        now_secs: f64,
        unseen_window: bool,
        field_slow: bool,
        caution: bool,
    ) {
        let Some((lap, time)) = row.scored_lap else {
            return;
        };
        if !valid_time(time) {
            return;
        }
        match car.last_scored_lap {
            Some(old) if lap < old => {
                *car = Car::default();
                car.last_scored_lap = Some(lap);
                car.last_score_progress = Some(now_secs);
                car.add_clean(time);
                return;
            }
            Some(old) if lap == old => return,
            Some(old) if lap > old + 1 => {
                // LastTime describes only this newest lap. No candidate can
                // be located inside an unknown multi-lap gap.
                car.last_scored_lap = Some(lap);
                car.last_score_progress = Some(now_secs);
                car.candidate = None;
                car.add_clean(time);
                return;
            }
            None => {
                car.last_scored_lap = Some(lap);
                car.last_score_progress = Some(now_secs);
                car.add_clean(time);
                return;
            }
            _ => {}
        }
        car.last_scored_lap = Some(lap);
        car.last_score_progress = Some(now_secs);
        let Some(pace) = car.pace() else {
            car.add_clean(time);
            return;
        };
        let loss = time - pace;
        let normal = loss.abs() <= pace * NORMAL_RATIO;
        if caution || field_slow {
            car.candidate = None;
            return;
        }
        if normal {
            if let Some(candidate) = car.candidate.take() {
                if pit_like(candidate.loss, candidate.pit_loss) {
                    car.boundary = Some(Boundary::Estimated {
                        first: candidate.first,
                        last: candidate.last,
                        basis: EstimateBasis::LapTiming,
                    });
                }
            }
            car.add_clean(time);
            return;
        }
        if loss <= 0.0 {
            car.candidate = None;
            car.add_clean(time);
            return;
        }
        match car.candidate.as_mut() {
            Some(candidate) if unseen_window && lap == candidate.last + 1 => {
                // With two split laps their own span is the boundary window;
                // the extra neighbouring-lap allowance is only for a lone
                // slow lap.
                candidate.first = candidate.last;
                candidate.last = lap;
                candidate.loss += loss;
                if candidate.last - candidate.first >= 2 || candidate.loss > candidate.pit_loss * 1.9 + 10.0 {
                    car.candidate = None;
                }
            }
            _ if unseen_window && possible_pit_loss(loss, row.pit_loss_secs) => {
                car.candidate =
                    Some(Candidate { first: lap.saturating_sub(1), last: lap, loss, pit_loss: row.pit_loss_secs });
            }
            _ => car.candidate = None,
        }
    }
}

fn valid_range(range: Option<(i32, i32)>) -> Option<(i32, i32)> {
    let (min, max) = range?;
    (min > 0 && min <= max).then_some((min, max))
}

fn valid_time(time: f32) -> bool {
    time.is_finite() && time > 1.0
}

fn pit_like(loss: f32, pit_loss: f32) -> bool {
    pit_loss.is_finite() && pit_loss > 0.0 && loss >= MIN_LOSS.max(pit_loss * 0.45) && loss <= pit_loss * 1.9 + 10.0
}

fn possible_pit_loss(loss: f32, pit_loss: f32) -> bool {
    pit_loss.is_finite() && pit_loss > 0.0 && loss >= 5.0_f32.max(pit_loss * 0.15) && loss <= pit_loss * 1.9 + 10.0
}

fn median(values: impl Iterator<Item = f32>) -> Option<f32> {
    let mut values: Vec<_> = values.filter(|value| value.is_finite()).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(lap: i32, time: f32) -> Observation {
        Observation {
            car_idx: 7,
            class_id: 1,
            lap,
            scored_lap: Some((lap, time)),
            in_world: true,
            in_pit_lane: false,
            driver_id: None,
            observed_boundary_lap: None,
            completed_stops: 0,
            range_hint: None,
            pit_loss_secs: 60.0,
        }
    }

    fn one(estimator: &mut Estimator, now: f64, revision: i32, row: Observation) -> StintAge {
        estimator.update(now, Some(revision), &[row], true, false)[&7]
    }

    fn pace(estimator: &mut Estimator) {
        for lap in 1..=4 {
            one(estimator, f64::from(lap), lap, row(lap, 120.0));
        }
    }

    fn make_unseen(estimator: &mut Estimator, start: f64) {
        let mut absent = row(4, 120.0);
        absent.in_world = false;
        one(estimator, start, 4, absent);
    }

    fn keep_blind_stream_alive(estimator: &mut Estimator, now: f64) {
        let mut absent = row(4, 120.0);
        absent.in_world = false;
        let _ = estimator.update(now, Some(4), &[absent], true, false);
    }

    #[test]
    fn unseen_single_lap_stop_needs_return_to_normal() {
        let mut estimator = Estimator::default();
        pace(&mut estimator);
        make_unseen(&mut estimator, 4.1);
        keep_blind_stream_alive(&mut estimator, 5.9);
        let mut slow = row(5, 180.0);
        slow.in_world = false;
        assert_eq!(one(&mut estimator, 6.2, 5, slow), StintAge::Unknown);
        assert_eq!(
            one(&mut estimator, 7.0, 6, row(6, 120.0)),
            StintAge::Estimated { min: 1, max: 2, basis: EstimateBasis::LapTiming }
        );
    }

    #[test]
    fn scoring_can_publish_hidden_lap_after_visual_return() {
        let mut estimator = Estimator::default();
        pace(&mut estimator);
        make_unseen(&mut estimator, 4.1);
        keep_blind_stream_alive(&mut estimator, 5.9);
        // The car returned before YAML published its slow completed lap.
        assert_eq!(one(&mut estimator, 6.2, 5, row(5, 180.0)), StintAge::Unknown);
        assert_eq!(
            one(&mut estimator, 7.0, 6, row(6, 120.0)),
            StintAge::Estimated { min: 1, max: 2, basis: EstimateBasis::LapTiming }
        );
        // A later fully observed spin is outside the retained blind lap range.
        one(&mut estimator, 8.0, 7, row(7, 180.0));
        assert_eq!(
            one(&mut estimator, 9.0, 8, row(8, 120.0)),
            StintAge::Estimated { min: 3, max: 4, basis: EstimateBasis::LapTiming }
        );
    }

    #[test]
    fn two_partial_unseen_laps_make_a_bounded_window() {
        let mut estimator = Estimator::default();
        pace(&mut estimator);
        make_unseen(&mut estimator, 4.1);
        keep_blind_stream_alive(&mut estimator, 5.9);
        let mut first = row(5, 145.0);
        first.in_world = false;
        one(&mut estimator, 6.2, 5, first);
        let mut second = row(6, 153.0);
        second.in_world = false;
        one(&mut estimator, 7.2, 6, second);
        assert_eq!(
            one(&mut estimator, 8.0, 7, row(7, 120.0)),
            StintAge::Estimated { min: 1, max: 2, basis: EstimateBasis::LapTiming }
        );
    }

    #[test]
    fn fully_observed_slow_lap_and_continuing_damage_do_not_infer_stop() {
        let mut estimator = Estimator::default();
        pace(&mut estimator);
        one(&mut estimator, 5.0, 5, row(5, 180.0));
        assert_eq!(one(&mut estimator, 6.0, 6, row(6, 120.0)), StintAge::Unknown);

        make_unseen(&mut estimator, 6.1);
        keep_blind_stream_alive(&mut estimator, 7.9);
        let mut damaged = row(7, 180.0);
        damaged.in_world = false;
        one(&mut estimator, 8.2, 7, damaged);
        let mut still_damaged = row(8, 180.0);
        still_damaged.in_world = false;
        one(&mut estimator, 9.2, 8, still_damaged);
        let mut third = row(9, 180.0);
        third.in_world = false;
        assert_eq!(one(&mut estimator, 10.2, 9, third), StintAge::Unknown);
    }

    #[test]
    fn absence_or_lane_boolean_alone_never_creates_a_boundary() {
        let mut estimator = Estimator::default();
        let mut unseen = row(12, 120.0);
        unseen.in_world = false;
        unseen.in_pit_lane = true;
        assert_eq!(one(&mut estimator, 1.0, 1, unseen), StintAge::Unknown);
        unseen.in_pit_lane = false;
        unseen.scored_lap = None;
        assert_eq!(one(&mut estimator, 1.5, 1, unseen), StintAge::Unknown);
    }

    #[test]
    fn caution_and_duplicate_or_jumped_rows_cannot_be_stop_evidence() {
        let mut estimator = Estimator::default();
        pace(&mut estimator);
        make_unseen(&mut estimator, 4.1);
        keep_blind_stream_alive(&mut estimator, 5.9);
        let mut slow = row(5, 180.0);
        slow.in_world = false;
        let _ = estimator.update(6.2, Some(5), &[slow], true, true);
        assert_eq!(one(&mut estimator, 7.0, 6, row(6, 120.0)), StintAge::Unknown);
        let mut duplicate = row(6, 180.0);
        duplicate.in_world = false;
        one(&mut estimator, 8.0, 7, duplicate);
        let mut jumped = row(10, 180.0);
        jumped.in_world = false;
        assert_eq!(one(&mut estimator, 9.0, 8, jumped), StintAge::Unknown);
    }

    #[test]
    fn field_wide_slowdown_is_not_three_independent_stops() {
        let mut estimator = Estimator::default();
        for lap in 1..=4 {
            let rows: Vec<_> = (0..3).map(|car_idx| Observation { car_idx, ..row(lap, 120.0) }).collect();
            let _ = estimator.update(f64::from(lap), Some(lap), &rows, true, false);
        }
        let absent: Vec<_> = (0..3).map(|car_idx| Observation { car_idx, in_world: false, ..row(4, 120.0) }).collect();
        let _ = estimator.update(4.1, Some(4), &absent, true, false);
        let _ = estimator.update(5.9, Some(4), &absent, true, false);
        let slow: Vec<_> = (0..3).map(|car_idx| Observation { car_idx, in_world: false, ..row(5, 180.0) }).collect();
        let _ = estimator.update(6.2, Some(5), &slow, true, false);
        let returned: Vec<_> = (0..3).map(|car_idx| Observation { car_idx, ..row(6, 120.0) }).collect();
        let ages = estimator.update(7.0, Some(6), &returned, true, false);
        assert!(ages.values().all(|age| *age == StintAge::Unknown));
    }

    #[test]
    fn staggered_scoring_publications_still_suppress_field_slowdown() {
        let mut estimator = Estimator::default();
        for lap in 1..=4 {
            let rows: Vec<_> = (0..3).map(|car_idx| Observation { car_idx, ..row(lap, 120.0) }).collect();
            let _ = estimator.update(f64::from(lap), Some(lap), &rows, true, false);
        }
        let absent: Vec<_> = (0..3).map(|car_idx| Observation { car_idx, in_world: false, ..row(4, 120.0) }).collect();
        let _ = estimator.update(4.1, Some(4), &absent, true, false);
        let _ = estimator.update(5.9, Some(4), &absent, true, false);
        for (revision, car_idx) in [(5, 0), (6, 1), (7, 2)] {
            let slow = Observation { car_idx, in_world: false, ..row(5, 180.0) };
            let _ = estimator.update(f64::from(revision) + 1.2, Some(revision), &[slow], true, false);
        }
        let returned = Observation { car_idx: 0, ..row(6, 120.0) };
        assert_eq!(estimator.update(9.0, Some(8), &[returned], true, false)[&0], StintAge::Unknown);
    }

    #[test]
    fn driver_change_spans_last_known_old_driver_lap() {
        let mut estimator = Estimator::default();
        let mut first = row(10, 120.0);
        first.driver_id = Some(11);
        one(&mut estimator, 1.0, 1, first);
        let mut changed = row(30, 120.0);
        changed.driver_id = Some(12);
        assert_eq!(
            one(&mut estimator, 1.5, 2, changed),
            StintAge::Estimated { min: 0, max: 20, basis: EstimateBasis::DriverChange }
        );
    }

    #[test]
    fn cold_attach_uses_only_a_bounded_strategy_prior() {
        let mut estimator = Estimator::default();
        let mut late = row(65, 120.0);
        late.range_hint = Some((27, 29));
        assert_eq!(
            one(&mut estimator, 1.0, 1, late),
            StintAge::Estimated { min: 7, max: 11, basis: EstimateBasis::StrategyPrior }
        );
    }

    #[test]
    fn expired_strategy_prior_reenumerates_instead_of_showing_an_impossible_old_age() {
        let mut estimator = Estimator::default();
        let mut late = row(65, 120.0);
        late.range_hint = Some((27, 29));
        one(&mut estimator, 1.0, 1, late);
        late.lap = 95;
        late.scored_lap = Some((95, 120.0));
        assert_eq!(
            one(&mut estimator, 2.0, 2, late),
            StintAge::Estimated { min: 8, max: 14, basis: EstimateBasis::StrategyPrior }
        );
    }

    #[test]
    fn newer_observed_boundary_wins_and_old_boundary_after_gap_cannot_revive() {
        let mut estimator = Estimator::default();
        let mut observed = row(20, 120.0);
        observed.observed_boundary_lap = Some(10);
        assert_eq!(one(&mut estimator, 1.0, 1, observed), StintAge::Observed(10));
        // A local receiver gap clears exact certainty. The same old start is
        // sent repeatedly by StintState and must stay ignored.
        observed.scored_lap = None;
        assert_eq!(estimator.update(4.1, Some(1), &[observed], true, false)[&7], StintAge::Unknown);
        assert_eq!(estimator.update(4.2, Some(1), &[observed], true, false)[&7], StintAge::Unknown);
        observed.lap = 21;
        observed.observed_boundary_lap = Some(21);
        assert_eq!(estimator.update(4.3, Some(2), &[observed], true, false)[&7], StintAge::Observed(0));
    }
}
