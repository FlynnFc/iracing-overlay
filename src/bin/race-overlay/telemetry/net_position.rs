//! Live race deficits for NET position. Scoring-line gaps can still describe
//! the lap before a pit stop; current race distance includes the loss already
//! incurred. Unlike Relative, these gaps retain whole laps.

use super::relative::LapCurve;

/// Current lap number and distance fraction from the same telemetry tick.
#[derive(Clone, Copy, Debug)]
pub struct RaceProgress {
    pub lap: i32,
    pub pct: f32,
}

/// Estimated seconds behind a class leader on one shared lap-time scale.
///
/// Both cars must belong to the class whose curve and pace are supplied.
/// The measured curve accounts for corners and straights; before a clean lap
/// is available, distance fraction is the approximation. Missing/off-world
/// progress and a car clearly ahead of the supplied leader return `None`.
/// Small negative deficits within standings' half-thousandth-lap ordering
/// hysteresis are treated as level. This does not wrap to the nearest car:
/// a competitor two laps down remains two laps down.
#[must_use]
pub fn live_gap_secs(curve: &LapCurve, leader: RaceProgress, car: RaceProgress, lap_secs: f32) -> Option<f32> {
    let valid =
        |progress: RaceProgress| progress.lap >= 0 && progress.pct.is_finite() && (0.0..=1.0).contains(&progress.pct);
    if !valid(leader) || !valid(car) || !lap_secs.is_finite() || lap_secs <= 0.0 {
        return None;
    }
    let laps = f64::from(leader.lap) - f64::from(car.lap);
    let distance_deficit = laps + f64::from(leader.pct) - f64::from(car.pct);
    if distance_deficit < -0.000_5 {
        return None;
    }
    let fraction = |pct| f64::from(curve.lap_fraction(pct).unwrap_or(pct));
    let seconds = (laps + fraction(leader.pct) - fraction(car.pct)).max(0.0) * f64::from(lap_secs);
    #[expect(clippy::cast_possible_truncation, reason = "finite output is checked after conversion")]
    let seconds = seconds as f32;
    seconds.is_finite().then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::{RaceProgress, live_gap_secs};
    use crate::telemetry::relative::LapCurve;

    fn progress(lap: i32, pct: f32) -> RaceProgress {
        RaceProgress { lap, pct }
    }

    fn close(actual: Option<f32>, expected: f32) {
        assert!((actual.expect("valid live gap") - expected).abs() < 0.02);
    }

    #[test]
    fn crossing_the_line_preserves_the_gap() {
        let curve = LapCurve::default();
        close(live_gap_secs(&curve, progress(10, 0.99), progress(10, 0.97), 100.0), 2.0);
        close(live_gap_secs(&curve, progress(11, 0.01), progress(10, 0.99), 100.0), 2.0);
        close(live_gap_secs(&curve, progress(11, 0.03), progress(11, 0.01), 100.0), 2.0);
    }

    #[test]
    fn retains_whole_laps_and_more_than_half_a_lap() {
        let curve = LapCurve::default();
        close(live_gap_secs(&curve, progress(12, 0.75), progress(12, 0.1), 100.0), 65.0);
        close(live_gap_secs(&curve, progress(12, 0.75), progress(10, 0.1), 100.0), 265.0);
    }

    #[test]
    fn a_stationary_pitting_car_loses_time_each_tick() {
        let curve = LapCurve::default();
        let stopped = progress(10, 0.95);
        close(live_gap_secs(&curve, progress(10, 0.98), stopped, 100.0), 3.0);
        close(live_gap_secs(&curve, progress(11, 0.08), stopped, 100.0), 13.0);
        close(live_gap_secs(&curve, progress(11, 0.15), progress(11, 0.02), 100.0), 13.0);
    }

    #[test]
    fn invalid_telemetry_is_not_a_zero_gap() {
        let curve = LapCurve::default();
        let leader = progress(10, 0.5);
        for invalid in [progress(-1, 0.3), progress(10, -1.0), progress(10, 1.01), progress(10, f32::NAN)] {
            assert!(live_gap_secs(&curve, leader, invalid, 100.0).is_none());
            assert!(live_gap_secs(&curve, invalid, leader, 100.0).is_none());
        }
        for pace in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(live_gap_secs(&curve, leader, progress(10, 0.3), pace).is_none());
        }
        assert!(live_gap_secs(&curve, leader, progress(11, 0.3), 100.0).is_none());
        close(live_gap_secs(&curve, leader, progress(10, 0.500_1), 100.0), 0.0);
    }

    #[test]
    fn measured_curve_uses_time_instead_of_distance() {
        let mut curve = LapCurve::default();
        // A nonuniform lap: the first half of distance takes one quarter
        // of the time. Every bin is sampled on the following full lap.
        for lap in 0..3 {
            for tick in 0..1000 {
                let fraction = f64::from(tick) / 1000.0;
                #[expect(clippy::cast_possible_truncation, reason = "fraction is within zero and one")]
                let pct = fraction.sqrt() as f32;
                curve.observe(Some(pct), f64::from(lap) * 100.0 + fraction * 100.0, true);
            }
        }
        assert!(curve.measured_lap_secs().is_some());
        close(live_gap_secs(&curve, progress(12, 0.8), progress(10, 0.5), 100.0), 239.0);
    }
}
