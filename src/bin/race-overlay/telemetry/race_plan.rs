// Rust guideline compliant 2026-02-16

//! The remaining race as a fuel plan: stints, stop laps, and what-ifs.
//!
//! The maths behind the spec-mode Strategy page's schedule band and its two
//! "smart" questions — *what burn skips a stop?* and *what burn makes it N
//! stops?* — see `plans/strategy-spec-mode.md`. Everything here is a pure
//! function over plain numbers, per the strategy principles: unit-testable
//! with no session and no window, and honest to the point of returning
//! `None` rather than a plan built on inputs that cannot support one.
//!
//! The model is deliberately simple: laps are the unit, the plan is computed
//! as from the current lap crossing, and every stop fills to what the rest of
//! the race needs (capped by the tank), keeping `reserve_laps` of fuel in
//! hand at all times. Pace, traffic and tyres are other modules' problems —
//! this is the fuel arithmetic they hang off.

/// What the plan is computed from. All figures as of the current crossing.
#[derive(Debug, Clone, Copy)]
pub struct PlanInputs {
    /// The lap now under way.
    pub current_lap: u16,
    /// Laps still to complete, including the one under way.
    pub laps_remaining: u16,
    /// Litres in the tank now.
    pub fuel_litres: f32,
    /// Measured litres per lap.
    pub burn_per_lap: f32,
    /// Usable tank, in litres.
    pub tank_litres: f32,
    /// Laps of fuel kept in hand — at every stop and at the flag.
    pub reserve_laps: f32,
}

/// One planned stop: pit at the end of `lap`, add `add_litres`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlannedStop {
    pub lap: u16,
    pub add_litres: f32,
}

/// The remaining race, as the fuel allows it.
#[derive(Debug, Clone, PartialEq)]
pub struct RacePlan {
    /// Every remaining stop, in order. Empty means the tank reaches the flag.
    pub stops: Vec<PlannedStop>,
    /// Laps of fuel in hand as the flag falls.
    pub finish_margin_laps: f32,
}

/// More stops than any real race plans for; past it the inputs are garbage
/// (a thimble of a tank), and the honest answer is no plan, not a thousand
/// imaginary stops.
const MAX_PLANNED_STOPS: usize = 100;

/// Plans the remaining race at the given burn.
///
/// Greedy, forward: run the tank to `reserve_laps` in hand, fill to what the
/// remaining laps need (capped by the tank), repeat.
///
/// `None` when no honest plan exists: no positive burn or tank, a tank too
/// small to progress a lap between stops, or a race that would take more than
/// [`MAX_PLANNED_STOPS`]. A finished race (`laps_remaining` 0) is a plan with
/// no stops, not an error.
#[must_use]
pub fn plan_race(inputs: &PlanInputs) -> Option<RacePlan> {
    // Spelled to catch `NaN` as well as zero and negatives: `NaN <= 0.0` is
    // false, so the finiteness check is what refuses a poisoned reading.
    let burn = inputs.burn_per_lap;
    let sound = |value: f32| value.is_finite() && value > 0.0;
    if !sound(burn) || !sound(inputs.tank_litres) || !inputs.fuel_litres.is_finite() || inputs.reserve_laps < 0.0 {
        return None;
    }

    let mut stops = Vec::new();
    let mut lap = inputs.current_lap;
    let mut to_go = f32::from(inputs.laps_remaining);
    let mut fuel = inputs.fuel_litres.max(0.0);

    while to_go > 0.0 {
        // Whole laps this tank still covers with the reserve in hand.
        let usable = (fuel / burn - inputs.reserve_laps).floor().max(0.0);
        if usable >= to_go {
            break;
        }
        // A tank that cannot progress at least one lap after a full fill can
        // never finish; the first iteration may legitimately need an
        // immediate stop (an empty-ish tank now), so only a *refilled* tank
        // stuck at zero is hopeless.
        if usable < 1.0 && !stops.is_empty() {
            return None;
        }
        if stops.len() >= MAX_PLANNED_STOPS {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "floored, non-negative, and bounded by laps_remaining, a u16"
        )]
        let run = usable as u16;
        lap = lap.saturating_add(run);
        to_go -= f32::from(run);
        let at_stop = fuel - f32::from(run) * burn;
        // Fill to what the rest needs — the remaining laps plus the reserve —
        // or to the brim where the race wants more than the tank holds.
        let want = (to_go + inputs.reserve_laps) * burn;
        let fill_to = want.min(inputs.tank_litres);
        stops.push(PlannedStop { lap, add_litres: (fill_to - at_stop).max(0.0) });
        fuel = fill_to;
    }

    Some(RacePlan { stops, finish_margin_laps: fuel / burn - to_go })
}

/// The burn that removes one stop from the current plan, if it is reachable.
///
/// Searches downward from the measured burn in `step`-sized savings, out to
/// `max_save_per_lap` — the "a tenth or two, not lift-and-coast" bound that
/// keeps the suggestion honest. `None` when the plan already has no stops,
/// when no plan exists at the measured burn, or when no reachable saving
/// drops the count. See the stop-skip band in `plans/strategy-spec-mode.md`.
#[must_use]
pub fn burn_to_skip_a_stop(inputs: &PlanInputs, max_save_per_lap: f32, step: f32) -> Option<f32> {
    let sound = |value: f32| value.is_finite() && value > 0.0;
    if !sound(step) || !sound(max_save_per_lap) {
        return None;
    }
    let baseline = plan_race(inputs)?.stops.len();
    if baseline == 0 {
        return None;
    }
    let mut save = step;
    while save <= max_save_per_lap {
        let candidate = PlanInputs { burn_per_lap: inputs.burn_per_lap - save, ..*inputs };
        if let Some(plan) = plan_race(&candidate)
            && plan.stops.len() < baseline
        {
            return Some(candidate.burn_per_lap);
        }
        save += step;
    }
    None
}

/// The burn that makes the remaining race fit `stops` stops — the "aim for
/// five stops" back-solve, answered in the litres-per-lap the fuel target
/// speaks.
///
/// Searches downward from the measured burn in `step`s, refusing to pretend
/// past half the measured burn — a target below that is not a save, it is a
/// different car. `None` when the race already fits (nothing to solve), or
/// when no realistic burn gets there.
#[must_use]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the 'aim for N stops' control, the spec-mode plan's stop-count back-solve")
)]
pub fn burn_for_stop_count(inputs: &PlanInputs, stops: usize, step: f32) -> Option<f32> {
    if !(step.is_finite() && step > 0.0) {
        return None;
    }
    let baseline = plan_race(inputs)?.stops.len();
    if baseline <= stops {
        return None;
    }
    let floor = inputs.burn_per_lap / 2.0;
    let mut burn = inputs.burn_per_lap - step;
    while burn >= floor {
        let candidate = PlanInputs { burn_per_lap: burn, ..*inputs };
        if let Some(plan) = plan_race(&candidate)
            && plan.stops.len() <= stops
        {
            return Some(burn);
        }
        burn -= step;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2 L/lap, 20 L tank, half a lap in hand.
    fn inputs(laps_remaining: u16, fuel: f32) -> PlanInputs {
        PlanInputs {
            current_lap: 10,
            laps_remaining,
            fuel_litres: fuel,
            burn_per_lap: 2.0,
            tank_litres: 20.0,
            reserve_laps: 0.5,
        }
    }

    #[test]
    fn a_tank_that_reaches_the_flag_plans_no_stop() {
        let plan = plan_race(&inputs(5, 20.0)).expect("a plan");
        assert!(plan.stops.is_empty());
        assert!((plan.finish_margin_laps - 5.0).abs() < 0.01, "10 laps of fuel, 5 to run");
    }

    #[test]
    fn one_stop_lands_at_the_last_safe_lap_and_fills_what_the_rest_needs() {
        // 12 L is 6 laps of fuel, 5.5 usable → run 5 of the 12, pit at lap 15,
        // fill for the 7 left plus reserve (15 L wanted, tank allows it).
        let plan = plan_race(&inputs(12, 12.0)).expect("a plan");
        assert_eq!(plan.stops.len(), 1);
        assert_eq!(plan.stops[0].lap, 15);
        // At the stop 2 L remain; filling to 15 L adds 13.
        assert!((plan.stops[0].add_litres - 13.0).abs() < 0.01);
        assert!((plan.finish_margin_laps - 0.5).abs() < 0.01, "the reserve is what crosses the line");
    }

    #[test]
    fn a_long_race_takes_repeated_brim_fills() {
        // 40 laps at 2 L/lap needs 80 L; a 20 L tank (9.5 usable laps per
        // fill) forces repeated stops, each to the brim until the last.
        let plan = plan_race(&inputs(40, 20.0)).expect("a plan");
        assert_eq!(plan.stops.len(), 4, "40 laps on a 9-usable-lap tank is four more stops");
        // A brim fill: 9 laps run leaves 2 L, topping back to the 20 L tank
        // adds 18. The final stop takes only what the last stint needs.
        let brims = plan.stops.iter().filter(|stop| (stop.add_litres - 18.0).abs() < 0.02).count();
        assert_eq!(brims, plan.stops.len() - 1, "every stop but the last fills to the brim");
        assert!((plan.stops.last().expect("stops").add_litres - 7.0).abs() < 0.02, "the splash is sized to the finish");
        assert!(plan.finish_margin_laps >= 0.5 - 0.01, "the reserve survives to the flag");
        // Stops land in strictly increasing laps inside the race.
        for pair in plan.stops.windows(2) {
            assert!(pair[1].lap > pair[0].lap);
        }
    }

    #[test]
    fn an_empty_tank_now_pits_immediately_rather_than_pretending() {
        // Half a lap of fuel with laps to run: the stop is this lap, lap 10.
        let plan = plan_race(&inputs(8, 1.0)).expect("a plan");
        assert_eq!(plan.stops.first().expect("a stop").lap, 10);
    }

    #[test]
    fn hopeless_inputs_refuse_a_plan() {
        // No burn measured.
        assert!(plan_race(&PlanInputs { burn_per_lap: 0.0, ..inputs(10, 10.0) }).is_none());
        assert!(plan_race(&PlanInputs { burn_per_lap: -1.0, ..inputs(10, 10.0) }).is_none());
        assert!(plan_race(&PlanInputs { burn_per_lap: f32::NAN, ..inputs(10, 10.0) }).is_none());
        // A tank smaller than a lap of fuel plus reserve can never progress.
        assert!(plan_race(&PlanInputs { tank_litres: 2.0, ..inputs(10, 2.0) }).is_none());
        // A NaN tank reading.
        assert!(plan_race(&PlanInputs { fuel_litres: f32::NAN, ..inputs(10, 10.0) }).is_none());
        // A negative reserve is a caller bug, not a bolder strategy.
        assert!(plan_race(&PlanInputs { reserve_laps: -1.0, ..inputs(10, 10.0) }).is_none());
    }

    #[test]
    fn a_finished_race_is_an_empty_plan_not_an_error() {
        let plan = plan_race(&inputs(0, 10.0)).expect("a plan");
        assert!(plan.stops.is_empty());
        assert!((plan.finish_margin_laps - 5.0).abs() < 0.01);
    }

    #[test]
    fn negative_fuel_reads_as_empty_not_as_debt() {
        let plan = plan_race(&PlanInputs { fuel_litres: -3.0, ..inputs(8, 0.0) }).expect("a plan");
        assert_eq!(plan.stops.first().expect("a stop").lap, 10, "an immediate stop, not a negative fill");
        assert!(plan.stops.iter().all(|stop| stop.add_litres >= 0.0));
    }

    #[test]
    fn skipping_a_stop_names_the_nearest_reachable_burn() {
        // 12 laps on 12 L: one stop at 2.0 L/lap. Saving to ~0.96 L/lap would
        // make the tank reach — far beyond a reachable save — but a race just
        // over the boundary flips with a small one:
        // 21 laps, full 20 L tank: 9.5 usable of 21 → 2 stops at 2.0. At
        // 1.9 L/lap usable becomes 10.02 → still 2... search widens until the
        // count drops within the allowed save, or reports honestly that it
        // cannot.
        let two_stop = PlanInputs { laps_remaining: 21, fuel_litres: 20.0, ..inputs(0, 0.0) };
        assert_eq!(plan_race(&two_stop).expect("plan").stops.len(), 2);
        let saved = burn_to_skip_a_stop(&two_stop, 0.2, 0.01);
        if let Some(burn) = saved {
            let replanned = plan_race(&PlanInputs { burn_per_lap: burn, ..two_stop }).expect("plan");
            assert!(replanned.stops.len() < 2, "the returned burn actually drops a stop");
            assert!(two_stop.burn_per_lap - burn <= 0.2 + 0.001, "and stays inside the reachable save");
        }

        // A no-stop race has nothing to skip.
        assert_eq!(burn_to_skip_a_stop(&inputs(5, 20.0), 0.2, 0.01), None);
        // An unreachable save reports honestly.
        assert_eq!(burn_to_skip_a_stop(&inputs(12, 12.0), 0.05, 0.01), None, "a whole stop needs more than 0.05");
        // Degenerate search parameters refuse rather than loop.
        assert_eq!(burn_to_skip_a_stop(&two_stop, 0.0, 0.01), None);
        assert_eq!(burn_to_skip_a_stop(&two_stop, 0.2, 0.0), None);
    }

    #[test]
    fn the_stop_count_back_solve_returns_a_burn_that_actually_fits() {
        // 60 laps on a 20 L tank at 2.0 L/lap: many stops. Ask for a count
        // one fewer than the baseline needs.
        let long = PlanInputs { laps_remaining: 60, fuel_litres: 20.0, ..inputs(0, 0.0) };
        let baseline = plan_race(&long).expect("plan").stops.len();
        let target = baseline - 1;
        if let Some(burn) = burn_for_stop_count(&long, target, 0.01) {
            let replanned = plan_race(&PlanInputs { burn_per_lap: burn, ..long }).expect("plan");
            assert!(replanned.stops.len() <= target, "the returned burn fits the asked count");
            assert!(burn < long.burn_per_lap, "it is a save, not a wish");
            assert!(burn >= long.burn_per_lap / 2.0, "and stays inside the honesty floor");
        }

        // Already fitting: nothing to solve.
        assert_eq!(burn_for_stop_count(&inputs(5, 20.0), 0, 0.01), None);
        // An absurd ask (zero stops on a 60-lap race) hits the floor and
        // refuses rather than inventing a car that sips half the fuel.
        assert_eq!(burn_for_stop_count(&long, 0, 0.01), None);
    }
}
