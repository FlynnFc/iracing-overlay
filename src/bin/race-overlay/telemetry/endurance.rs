// Rust guideline compliant 2026-02-16

//! Pure endurance-strategy math: how many laps are left, how many stops that
//! implies, and where the field lands once everyone has taken theirs.
//!
//! All of this is projection, not fact. iRacing publishes no fuel, tyre or
//! pit-service data for cars other than the player's, so every figure here is
//! inferred from what *is* observable — each car's completed stint lengths,
//! its measured time stationary in its pit box, and the session clock. The
//! UI presents these as estimates for that reason.
//!
//! Kept free of telemetry and UI types so it is cheap to unit test.

/// How many more laps a car running at `avg_lap_secs` still has to complete.
///
/// Includes the lap in progress, which `lap_dist_pct` says how much of is
/// already behind the car. That lap has to be finished and has to be fuelled
/// for, and leaving it out under-counted the race by one lap for all but the
/// instant a car sits on the start/finish line — a whole lap of fuel, on the
/// low side, every time.
///
/// Rounds up, and to the same rule as [`projected_total_laps`]: a race ends at
/// a line crossing, so a car part-way round when the clock expires still
/// completes that lap.
///
/// Returns `None` when the session has no fixed time (a lap-limited or
/// practice session, where remaining time is not the binding constraint) or
/// before a lap time is known.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "lap counts in a session are far below i32's range, and a non-positive remaining time returns early"
)]
pub fn laps_remaining(session_remain_secs: Option<f64>, avg_lap_secs: f32, lap_dist_pct: Option<f32>) -> Option<i32> {
    let remain = session_remain_secs?;
    if !avg_lap_secs.is_finite() || !remain.is_finite() || avg_lap_secs <= 0.0 || remain <= 0.0 {
        return None;
    }
    if lap_dist_pct.is_some_and(|pct| !pct.is_finite()) {
        return None;
    }
    let driven = f64::from(lap_dist_pct.unwrap_or(0.0).clamp(0.0, 1.0));
    Some((remain / f64::from(avg_lap_secs) + driven).ceil().max(1.0) as i32)
}

/// How many laps this car will have completed when it takes the flag.
///
/// `current_lap` is the lap the car is on now and `lap_dist_pct` how far
/// around it the car already is.
///
/// A timed race never stops mid-lap: the last lap is the one running when the
/// clock expires, whether you frame that as the leader being shown the white
/// flag at the crossing where less than a lap of time is left, or as the
/// leader finishing the lap it is on when the clock reaches zero. Both come to
/// the same count, so the projection rounds **up** to a whole lap — the number
/// a driver reads this as is how many laps the race runs, and there is no such
/// thing as 12.7 of them.
///
/// The part of the current lap already driven is added rather than the whole
/// lap counted, because that part is not in `session_remain_secs`; counting
/// `current_lap` whole *and* every lap the remaining time allows counts it
/// twice, which reads a lap high at the start of each lap.
///
/// Returns `None` when the session has no fixed clock or no lap time is known
/// yet, and never projects fewer laps than the car has already completed.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "lap counts in a session are far below i32's range, and the value is clamped below"
)]
pub fn projected_total_laps(
    current_lap: i32,
    lap_dist_pct: Option<f32>,
    session_remain_secs: Option<f64>,
    avg_lap_secs: f32,
) -> Option<i32> {
    let remain = session_remain_secs?;
    if !avg_lap_secs.is_finite() || !remain.is_finite() || avg_lap_secs <= 0.0 || remain < 0.0 {
        return None;
    }
    if lap_dist_pct.is_some_and(|pct| !pct.is_finite()) {
        return None;
    }
    // Clamped because `CarIdxLapDistPct` reads a little outside 0..1 either
    // side of the start/finish line.
    let driven = f64::from(lap_dist_pct.unwrap_or(0.0).clamp(0.0, 1.0));
    let laps_to_run = (remain / f64::from(avg_lap_secs) + driven).ceil() as i32;
    let laps_completed = current_lap.saturating_sub(1);
    Some(laps_completed.saturating_add(laps_to_run.max(1)))
}

/// How many more pit stops a car needs to reach the end of the session.
///
/// `laps_left` is the whole session's remaining laps; `stint_laps_so_far` is
/// how far into its current stint the car already is, and `avg_stint_laps`
/// how long its stints have historically run. Returns zero when the car can
/// finish on its current stint.
///
/// A car with no completed stint yet has no basis for a projection, so
/// `avg_stint_laps` of zero or less yields `None` rather than a guess.
#[must_use]
pub fn stops_remaining(laps_left: i32, stint_laps_so_far: i32, avg_stint_laps: i32) -> Option<i32> {
    if avg_stint_laps <= 0 || stint_laps_so_far < 0 || laps_left < 0 {
        return None;
    }
    // Laps this car can still cover before its current stint runs out.
    let laps_on_current_stint = (avg_stint_laps - stint_laps_so_far).max(0);
    let laps_needing_a_stop = laps_left - laps_on_current_stint;
    if laps_needing_a_stop <= 0 {
        return Some(0);
    }
    // Ceiling division: any remainder needs one more stop.
    Some(1 + (laps_needing_a_stop - 1) / avg_stint_laps)
}

/// Whether the fuel margin makes the current lap the one to pit on.
///
/// The "box this lap" trigger behind the black box's BOX BOX border. It is
/// true once you could not safely complete *another* lap after this one — so
/// this lap is the last to come in on — and only in the lap's back half, so
/// the call is a settled "turn in at the end of this lap" rather than a
/// first-corner guess there is still time to reconsider.
///
/// `fuel_litres` comes from the live tank and `burn_per_lap` is the highest
/// consumption among the five most recent racing laps. `lap_fraction` is the
/// time-weighted fraction of the current lap already driven (0.0 at the line,
/// 1.0 approaching it). Returns `false` with invalid inputs or no measured
/// burn — a made-up "box now" is the one false instruction that costs a race.
#[must_use]
pub fn box_this_lap(fuel_litres: f32, burn_per_lap: Option<f32>, lap_fraction: f32) -> bool {
    let Some(burn) = burn_per_lap.filter(|burn| burn.is_finite() && *burn > 0.0) else {
        return false;
    };
    if !fuel_litres.is_finite()
        || fuel_litres < 0.0
        || !lap_fraction.is_finite()
        || !(0.5..=1.0).contains(&lap_fraction)
    {
        return false;
    }
    let laps_of_fuel = fuel_litres / burn;
    // Fuel to finish this lap plus one complete lap after it. An exact fit can
    // run the extra lap, so only a genuine shortfall makes this the lap to pit.
    let laps_to_finish_the_next = (1.0 - lap_fraction) + 1.0;
    laps_of_fuel < laps_to_finish_the_next
}

/// The lap the tank runs dry at a given burn — the "make lap N" figure the
/// shared fuel target shows on the Relative header.
///
/// `current_lap` is the lap now under way; the result is the last lap that
/// finishes before the tank empties, so "make lap 96" means lap 96 is
/// completed and lap 97 is where fuel runs out. `None` with no positive burn
/// (nothing to divide by) or no fuel — there is no honest lap to name.
#[must_use]
pub fn projected_dry_lap(current_lap: u16, fuel_litres: f32, burn_per_lap: f32) -> Option<u16> {
    if burn_per_lap <= 0.0 || fuel_litres <= 0.0 {
        return None;
    }
    // Whole laps the tank still covers from the start of the current lap.
    let laps = (fuel_litres / burn_per_lap).floor();
    if !laps.is_finite() || laps < 0.0 {
        return None;
    }
    // Saturating so an implausibly large tank/tiny burn can't overflow the lap
    // number; a race is never near `u16::MAX` laps anyway.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "guarded finite and non-negative above"
    )]
    let laps = laps as u16;
    Some(current_lap.saturating_add(laps))
}

/// One car's inputs to the net-position projection.
#[derive(Debug, Clone, Copy)]
pub struct Contender {
    /// Where this car sits in its class right now.
    pub class_position: i32,
    /// Current total time deficit to the class leader, in seconds, including
    /// any whole laps behind. A within-lap gap is not sufficient.
    pub gap_to_leader_secs: f32,
    /// Stops this car still has to make, from [`stops_remaining`].
    pub stops_remaining: i32,
    /// Total time lost relative to staying on track for one stop, in seconds.
    /// This includes pit-lane transit loss as well as stationary service;
    /// stationary service time alone cannot substitute for total pit loss.
    pub pit_loss_secs: f32,
}

/// Projects each contender's finishing order once every remaining stop is
/// taken, returning each one's projected class position in input order.
///
/// The model is deliberately simple: a car's projected time behind the leader
/// is its current gap plus the cost of the stops it still owes, and the field
/// is re-sorted on that. It captures the thing that actually decides
/// endurance races — being on a different stop count from a rival — without
/// pretending to model traffic, safety cars or pace changes.
///
/// Ties keep their current running order, so a car ahead on the road stays
/// ahead when two projections land level.
///
/// An invalid input makes the entire class projection unavailable: excluding
/// one car would silently promote the others into incorrect net positions.
#[must_use]
pub fn projected_positions(contenders: &[Contender]) -> Vec<Option<i32>> {
    if contenders.iter().any(|c| {
        c.class_position <= 0
            || !c.gap_to_leader_secs.is_finite()
            || c.gap_to_leader_secs < 0.0
            || c.stops_remaining < 0
            || !c.pit_loss_secs.is_finite()
            || c.pit_loss_secs < 0.0
    }) {
        return vec![None; contenders.len()];
    }
    let projected_gap =
        |c: &Contender| f64::from(c.gap_to_leader_secs) + f64::from(c.stops_remaining) * f64::from(c.pit_loss_secs);

    let mut order: Vec<usize> = (0..contenders.len()).collect();
    order.sort_by(|&a, &b| {
        projected_gap(&contenders[a])
            .total_cmp(&projected_gap(&contenders[b]))
            .then(contenders[a].class_position.cmp(&contenders[b].class_position))
    });

    let mut positions = vec![None; contenders.len()];
    for (rank, &index) in order.iter().enumerate() {
        positions[index] = Some(i32::try_from(rank).unwrap_or(i32::MAX).saturating_add(1));
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_dry_lap_counts_whole_laps_of_fuel_from_here() {
        // 10 L at 2.5 L/lap is 4 whole laps; from lap 30 the tank makes lap 34.
        assert_eq!(projected_dry_lap(30, 10.0, 2.5), Some(34));
        // A part-lap's fuel doesn't add a lap: 11 L at 2.5 is 4.4 laps → 4.
        assert_eq!(projected_dry_lap(30, 11.0, 2.5), Some(34));
        // Exactly enough for one more.
        assert_eq!(projected_dry_lap(30, 2.5, 2.5), Some(31));
    }

    #[test]
    fn projected_dry_lap_needs_fuel_and_a_burn() {
        assert_eq!(projected_dry_lap(30, 10.0, 0.0), None, "no burn, no projection");
        assert_eq!(projected_dry_lap(30, 0.0, 2.5), None, "an empty tank has no lap to make");
    }

    #[test]
    fn box_this_lap_needs_a_measured_burn() {
        assert!(!box_this_lap(2.0, None, 0.9), "no burn, no call");
        assert!(!box_this_lap(2.0, Some(0.0), 0.9), "a zero burn is no basis");
    }

    #[test]
    fn box_this_lap_rejects_invalid_telemetry() {
        assert!(!box_this_lap(f32::NAN, Some(2.0), 0.9));
        assert!(!box_this_lap(f32::INFINITY, Some(2.0), 0.9));
        assert!(!box_this_lap(-1.0, Some(2.0), 0.9));
        assert!(!box_this_lap(2.0, Some(f32::NAN), 0.9));
        assert!(!box_this_lap(2.0, Some(f32::INFINITY), 0.9));
        assert!(!box_this_lap(2.0, Some(2.0), f32::NAN));
        assert!(!box_this_lap(2.0, Some(2.0), -0.1));
        assert!(!box_this_lap(2.0, Some(2.0), 1.1));
    }

    #[test]
    fn box_this_lap_waits_for_the_back_half_of_the_lap() {
        // One lap of fuel left (2 L at 2 L/lap): a definite stop this lap.
        assert!(!box_this_lap(2.0, Some(2.0), 0.4), "not yet — still the front half");
        assert!(box_this_lap(2.0, Some(2.0), 0.6), "past half, and can't do another lap");
    }

    #[test]
    fn box_this_lap_stays_dark_when_another_lap_exactly_fits() {
        // At 90% round, 2.2 L exactly covers the remaining 0.1 lap and the
        // complete lap after it at 2 L/lap.
        assert!(!box_this_lap(2.2, Some(2.0), 0.9));
    }

    #[test]
    fn box_this_lap_fires_when_the_next_lap_would_run_dry() {
        // At 90% round, anything below 2.2 L cannot finish this lap and one
        // complete lap after it.
        assert!(box_this_lap(2.19, Some(2.0), 0.9));
    }

    #[test]
    fn box_this_lap_advances_to_the_actual_last_viable_lap() {
        // With 2.6 L late in this lap, the car can still run the next lap, so
        // the call stays dark. After crossing the line and burning into that
        // final lap, it waits for the back-half gate and then calls the stop.
        assert!(!box_this_lap(2.6, Some(2.0), 0.9));
        assert!(!box_this_lap(2.2, Some(2.0), 0.1));
        assert!(box_this_lap(1.4, Some(2.0), 0.5));
    }

    #[test]
    fn laps_remaining_rounds_up_a_partial_lap() {
        assert_eq!(laps_remaining(Some(300.0), 90.0, Some(0.0)), Some(4));
        assert_eq!(laps_remaining(Some(180.0), 90.0, Some(0.0)), Some(2));
    }

    /// The lap in progress still has to be driven, and still has to be
    /// fuelled for. Two laps of time left, half way round: this lap plus two.
    #[test]
    fn laps_remaining_counts_the_lap_already_under_way() {
        assert_eq!(laps_remaining(Some(180.0), 90.0, Some(0.5)), Some(3));
        assert_eq!(laps_remaining(Some(180.0), 90.0, Some(0.99)), Some(3));
        // On the line, the lap ahead is whole and nothing is part-done.
        assert_eq!(laps_remaining(Some(180.0), 90.0, Some(0.0)), Some(2));
    }

    /// Seconds from the flag, there is still a lap to finish.
    #[test]
    fn laps_remaining_never_reads_zero_while_the_clock_is_running() {
        assert_eq!(laps_remaining(Some(0.5), 90.0, Some(0.9)), Some(1));
    }

    /// A practice session reports no fixed remaining time, and a session that
    /// hasn't produced a lap time yet has nothing to divide by.
    #[test]
    fn laps_remaining_needs_both_a_clock_and_a_lap_time() {
        assert_eq!(laps_remaining(None, 90.0, Some(0.5)), None);
        assert_eq!(laps_remaining(Some(300.0), 0.0, Some(0.5)), None);
    }

    /// The case the whole projection exists to get right: a 20-minute race at
    /// 80-second laps is 15 laps, and must read 15 from the green flag to the
    /// chequered rather than drifting across the race.
    #[test]
    fn a_timed_race_projects_the_same_total_throughout() {
        let at = |lap, pct, remain| projected_total_laps(lap, Some(pct), Some(remain), 80.0);
        assert_eq!(at(1, 0.0, 1200.0), Some(15), "on the green flag");
        assert_eq!(at(8, 0.5, 600.0), Some(15), "half distance");
        assert_eq!(at(15, 0.9, 8.0), Some(15), "final lap");
    }

    /// A race stops at a start/finish line, not at a stopwatch: 13 and a third
    /// laps' worth of time is a 14-lap race, because the lap running when the
    /// clock expires is completed.
    #[test]
    fn a_part_lap_of_time_left_is_a_whole_lap_of_racing() {
        assert_eq!(projected_total_laps(1, Some(0.0), Some(1200.0), 90.0), Some(14));
    }

    /// Counting the current lap whole while also counting every lap the
    /// remaining time allows counts the driven part of that lap twice, which
    /// is a full lap of over-reading at the start of each lap.
    #[test]
    fn the_driven_part_of_the_current_lap_is_not_counted_twice() {
        // Just across the line onto lap 5, so four are done and ten laps'
        // worth of time is left: fourteen.
        assert_eq!(projected_total_laps(5, Some(0.0), Some(800.0), 80.0), Some(14));
        // Same four completed and the same ten laps of time, but lap 5 is
        // nearly over — its last moments come out of the ten, so one more lap
        // fits.
        assert_eq!(projected_total_laps(5, Some(0.99), Some(800.0), 80.0), Some(15));
    }

    /// Without a lap-distance reading the car is treated as being at the line,
    /// which is the conservative end of the range.
    #[test]
    fn a_missing_lap_distance_assumes_the_start_of_the_lap() {
        assert_eq!(projected_total_laps(1, None, Some(1200.0), 80.0), Some(15));
    }

    #[test]
    fn a_projection_never_falls_below_the_lap_the_car_is_on() {
        // The clock has expired but the car still has most of its lap to run;
        // it is still shown the flag at the line, so that lap counts.
        assert_eq!(projected_total_laps(15, Some(0.1), Some(0.0), 80.0), Some(15));
    }

    #[test]
    fn projected_total_laps_needs_both_a_clock_and_a_lap_time() {
        assert_eq!(projected_total_laps(5, Some(0.5), None, 80.0), None);
        assert_eq!(projected_total_laps(5, Some(0.5), Some(600.0), 0.0), None);
    }

    #[test]
    fn a_car_that_can_reach_the_end_on_this_stint_owes_no_stops() {
        // 10 laps left, 4 laps into a 20-lap stint: 16 still available.
        assert_eq!(stops_remaining(10, 4, 20), Some(0));
    }

    #[test]
    fn stops_remaining_counts_a_partial_stint_as_a_whole_stop() {
        // 50 laps left, 5 into a 20-lap stint, so 15 free then 35 more:
        // two stints' worth, needing two stops.
        assert_eq!(stops_remaining(50, 5, 20), Some(2));
        // One lap beyond that still costs a third stop.
        assert_eq!(stops_remaining(56, 5, 20), Some(3));
    }

    /// This is the number the whole feature turns on: being a stop up on a
    /// rival is worth more than any plausible pace difference.
    ///
    /// 60 laps on 20-lap stints is 20 + 20 + 20, so two stops — the final
    /// stint runs to the flag and needs no stop after it.
    #[test]
    fn a_longer_stint_can_save_a_whole_stop() {
        let thirsty = stops_remaining(60, 0, 20).expect("has a stint history");
        let frugal = stops_remaining(60, 0, 30).expect("has a stint history");
        assert_eq!(thirsty, 2);
        assert_eq!(frugal, 1);
        assert_eq!(thirsty - frugal, 1, "the longer stint must be worth exactly one stop here");
    }

    /// Before a car has completed a stint there's nothing to project from,
    /// and inventing a stint length would put a confident wrong number on
    /// screen during the opening laps of every race.
    #[test]
    fn a_car_without_a_stint_history_gets_no_projection() {
        assert_eq!(stops_remaining(50, 0, 0), None);
    }

    #[test]
    fn an_extra_stop_drops_a_car_behind_a_rival_it_currently_leads() {
        let field = [
            // Leading on the road, but owes one more stop than the rival.
            Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 2, pit_loss_secs: 30.0 },
            Contender { class_position: 2, gap_to_leader_secs: 20.0, stops_remaining: 1, pit_loss_secs: 30.0 },
        ];
        // 0 + 60 = 60 versus 20 + 30 = 50, so the rival comes out ahead.
        assert_eq!(projected_positions(&field), vec![Some(2), Some(1)]);
    }

    #[test]
    fn equal_stop_counts_preserve_the_running_order() {
        let field = [
            Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 2, pit_loss_secs: 30.0 },
            Contender { class_position: 2, gap_to_leader_secs: 15.0, stops_remaining: 2, pit_loss_secs: 30.0 },
            Contender { class_position: 3, gap_to_leader_secs: 40.0, stops_remaining: 2, pit_loss_secs: 30.0 },
        ];
        assert_eq!(projected_positions(&field), vec![Some(1), Some(2), Some(3)]);
    }

    /// A car whose stops are measurably quicker — short fuel-only stops
    /// against a rival taking tyres — gains on the projection even at an
    /// identical stop count.
    #[test]
    fn a_quicker_stop_is_worth_track_position() {
        let field = [
            Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 2, pit_loss_secs: 32.0 },
            Contender { class_position: 2, gap_to_leader_secs: 5.0, stops_remaining: 2, pit_loss_secs: 12.0 },
        ];
        // 64.0 versus 29.0.
        assert_eq!(projected_positions(&field), vec![Some(2), Some(1)]);
    }

    #[test]
    fn an_empty_field_projects_nothing() {
        assert_eq!(projected_positions(&[]), Vec::<Option<i32>>::new());
    }

    #[test]
    fn paying_the_projected_pit_loss_preserves_net_position_after_exit() {
        let mut field = [
            Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 2, pit_loss_secs: 60.0 },
            Contender { class_position: 2, gap_to_leader_secs: 20.0, stops_remaining: 2, pit_loss_secs: 60.0 },
            Contender { class_position: 3, gap_to_leader_secs: 45.0, stops_remaining: 1, pit_loss_secs: 60.0 },
        ];
        let before = projected_positions(&field);
        // The first car pays its stop, drops behind both rivals and owes one
        // fewer stop. Rebase all live gaps onto the new on-road leader.
        field[0].gap_to_leader_secs = 40.0;
        field[0].class_position = 3;
        field[0].stops_remaining = 1;
        field[1].gap_to_leader_secs = 0.0;
        field[1].class_position = 1;
        field[2].gap_to_leader_secs = 25.0;
        field[2].class_position = 2;
        assert_eq!(before, vec![Some(2), Some(3), Some(1)]);
        assert_eq!(projected_positions(&field), before);
    }

    #[test]
    fn equal_projected_times_keep_running_order_even_with_shuffled_input() {
        let field = [
            Contender { class_position: 3, gap_to_leader_secs: 60.0, stops_remaining: 0, pit_loss_secs: 30.0 },
            Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 2, pit_loss_secs: 30.0 },
            Contender { class_position: 2, gap_to_leader_secs: 30.0, stops_remaining: 1, pit_loss_secs: 30.0 },
        ];
        assert_eq!(projected_positions(&field), vec![Some(3), Some(1), Some(2)]);
    }

    #[test]
    fn a_lap_down_car_only_gains_position_if_saved_stops_cover_the_total_deficit() {
        let mut field = [
            Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 1, pit_loss_secs: 60.0 },
            // A 90-second lap plus a 10-second gap, not a 10-second deficit.
            Contender { class_position: 2, gap_to_leader_secs: 100.0, stops_remaining: 0, pit_loss_secs: 60.0 },
        ];
        assert_eq!(projected_positions(&field), vec![Some(1), Some(2)]);
        field[0].stops_remaining = 2;
        assert_eq!(projected_positions(&field), vec![Some(2), Some(1)]);
    }

    #[test]
    fn invalid_contender_inputs_withhold_the_entire_class_projection() {
        let valid = Contender { class_position: 1, gap_to_leader_secs: 0.0, stops_remaining: 2, pit_loss_secs: 60.0 };
        for invalid in [
            Contender { gap_to_leader_secs: f32::NAN, ..valid },
            Contender { gap_to_leader_secs: f32::INFINITY, ..valid },
            Contender { gap_to_leader_secs: -1.0, ..valid },
            Contender { pit_loss_secs: f32::NAN, ..valid },
            Contender { pit_loss_secs: f32::INFINITY, ..valid },
            Contender { pit_loss_secs: -1.0, ..valid },
            Contender { stops_remaining: -1, ..valid },
            Contender { class_position: 0, ..valid },
        ] {
            assert_eq!(projected_positions(&[valid, invalid]), vec![None, None]);
        }
    }

    #[test]
    fn nonfinite_timing_inputs_do_not_produce_lap_or_stop_estimates() {
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(laps_remaining(Some(600.0), invalid, Some(0.5)), None);
            assert_eq!(laps_remaining(Some(f64::from(invalid)), 90.0, Some(0.5)), None);
            assert_eq!(laps_remaining(Some(600.0), 90.0, Some(invalid)), None);
            assert_eq!(projected_total_laps(10, Some(0.5), Some(600.0), invalid), None);
            assert_eq!(projected_total_laps(10, Some(0.5), Some(f64::from(invalid)), 90.0), None);
            assert_eq!(projected_total_laps(10, Some(invalid), Some(600.0), 90.0), None);
        }
    }

    #[test]
    fn stop_counts_reject_invalid_laps_and_do_not_overflow_ceiling_division() {
        assert_eq!(stops_remaining(10, -1, 20), None);
        assert_eq!(stops_remaining(-1, 10, 20), None);
        assert_eq!(stops_remaining(i32::MAX, 20, 20), Some(107_374_183));
        assert_eq!(stops_remaining(0, 20, 20), Some(0));
        assert_eq!(stops_remaining(20, 0, 20), Some(0));
        assert_eq!(stops_remaining(21, 0, 20), Some(1));
    }
}
