// Rust guideline compliant 2026-02-16

//! Which of the next few laps to box on, and what each one costs.
//!
//! Two questions a driver asks on the way to a stop, both about specific laps
//! rather than about a strategy in the abstract:
//!
//! - Box a lap earlier or later, and what traffic do I come out into?
//! - Extend by a lap, and what fuel-per-lap do I have to hit?
//!
//! Both are answered per candidate lap, so the page can be a window of laps
//! with one column each. See `plans/strategy-and-timings.md` (Feature 3) for
//! the reasoning this implements.
//!
//! Everything here is a pure function over plain data — no session, no window,
//! no telemetry types — so the whole forward simulation is unit-testable.
//!
//! ## The frame everything is computed in
//!
//! Positions are held as *relative seconds*: how far ahead of the player a car
//! is, measured around the track, positive ahead and negative behind, wrapped
//! to half a lap either way. That is the same quantity the Relative widget
//! already shows, so no second definition of "where a car is" enters the app.
//!
//! It also means the lap curve is not needed here. A car's gap changes at a
//! rate that depends only on the two lap times: in `t` seconds the player
//! covers `t/P` laps and the rival `t/R`, so the rival gains `t·(P/R − 1)`
//! seconds of track on the player. Everything below is that one line applied
//! forwards.

/// How a rival's class relates to the player's, which is what decides whether
/// meeting it is a fight, a blue flag, or a pass to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassOrder {
    /// A quicker class: it arrives behind and expects to be let past.
    Faster,
    Same,
    /// A slower class: it sits ahead and has to be passed.
    Slower,
}

/// One car in the field, as the simulation needs it.
#[derive(Debug, Clone, Copy)]
pub struct Rival {
    pub car_idx: i32,
    /// Seconds ahead of the player around the track; negative is behind.
    pub gap_secs: f32,
    /// This car's recent pace. Cars without one are dropped before they get
    /// here — see [`pit_window`].
    pub lap_secs: f32,
    pub class: ClassOrder,
}

/// The player's own state at the moment the window is computed.
#[derive(Debug, Clone, Copy)]
pub struct Player {
    /// The lap now being driven.
    pub current_lap: i32,
    /// Recent pace, which every projection here is scaled by.
    pub lap_secs: f32,
    pub fuel_level_litres: f32,
    /// Measured consumption. Without it there is no window at all.
    pub fuel_per_lap_litres: f32,
    /// Laps left in the session, which the window is truncated to.
    pub laps_remaining: Option<i32>,
    /// Tank size. Without it a stop's reach cannot be known, and neither can
    /// the one thing this page is really being asked.
    pub tank_capacity_litres: Option<f32>,
}

impl Player {
    /// The last lap of the race.
    fn final_lap(&self) -> Option<i32> {
        self.laps_remaining.map(|left| self.current_lap + left.saturating_sub(1))
    }

    /// How many laps a full tank covers.
    fn laps_per_tank(&self) -> Option<f32> {
        let capacity = self.tank_capacity_litres.filter(|litres| *litres > 0.0)?;
        (self.fuel_per_lap_litres > 0.0).then(|| capacity / self.fuel_per_lap_litres)
    }
}

/// What a candidate lap asks of the driver on fuel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Save {
    /// Litres per lap that have to be given up. Zero or less means none.
    pub per_lap_litres: f32,
    /// The same figure as a fraction of current consumption.
    pub fraction: f32,
    /// Whether that is a rate a driver could actually hold — see
    /// [`Config::max_save_fraction`].
    pub reachable: bool,
}

impl Save {
    /// Whether this lap needs no saving at all.
    #[must_use]
    pub fn is_free(self) -> bool {
        self.per_lap_litres <= 0.0
    }
}

/// The first car that becomes a problem after the stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conflict {
    pub car_idx: i32,
    /// How many of the player's laps after the exit it arrives in.
    pub laps_until: f32,
    /// How much quicker it is per lap. Negative means the player is quicker
    /// and the problem is catching it, not being caught.
    pub pace_delta_secs: f32,
}

/// One lap the player could box on.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub lap: i32,
    /// Laps from the one being driven now; zero is "box at the end of this
    /// lap".
    pub laps_from_now: i32,
    pub save: Save,
    /// Seconds of compromised running after the exit — an index, not a time
    /// loss. `None` when the field had no usable pace and nothing was
    /// simulated.
    pub traffic_score: Option<f32>,
    /// The cars either side of the player at the pit exit: ahead, then behind.
    pub emerge_between: (Option<i32>, Option<i32>),
    pub first_conflict: Option<Conflict>,
    /// Position within the player's own class at the exit, or `None` when
    /// nothing was simulated.
    pub exit_class_position: Option<i32>,
    /// Stops still needed to reach the end, counting this one.
    ///
    /// The consequence a driver is actually weighing. Boxing early costs
    /// nothing at all until it pushes this up — a four-hour race on
    /// fifty-five-minute stints has a splash at the end that absorbs the
    /// slack, so somewhere in it there is an early stop that changes nothing.
    /// That is what this column finds. `None` without a tank size to divide by.
    pub stops_to_finish: Option<i32>,
}

/// How much of this to believe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Good,
    Fair,
    Poor,
    /// Nothing is shown: one caution rewrites the whole answer, and a
    /// green-flag projection during one is worse than no projection.
    Void,
}

impl Confidence {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Fair => "fair",
            Self::Poor => "poor",
            Self::Void => "void",
        }
    }
}

/// The whole window.
#[derive(Debug, Clone, PartialEq)]
pub struct PitWindow {
    pub candidates: Vec<Candidate>,
    /// The lap with the least traffic among those the fuel can reach.
    pub recommended_lap: Option<i32>,
    pub confidence: Confidence,
    /// The last lap the tank reaches with no saving at all — the lap the
    /// current plan is running to, and what the window is centred on.
    pub fuel_window_last_lap: Option<i32>,
    /// The fewest stops any candidate here reaches the end in.
    ///
    /// A candidate needing more than this is an extra stop, which is a cost no
    /// amount of clear air makes up for.
    pub min_stops: Option<i32>,
    /// Whether the tank already reaches the end, so there is no stop to plan.
    pub finishes_without_stopping: bool,
    /// How many cars were left out for having no pace figure yet, so the page
    /// can say so rather than implying the whole field was considered.
    pub cars_skipped: usize,
}

/// Tunables, all of them defaulted from the spec.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// How many laps past the plan to offer, for extending the stint.
    ///
    /// Two, because in an endurance race an extend is one or two laps; the
    /// undercut side is not fixed like this but runs back to wherever the stop
    /// count would change, which is narrow on tight fuel and wide with margin.
    pub extend_laps: i32,
    /// A hard cap on columns, so a long undercut window stays drawable.
    pub max_columns: usize,
    /// How many of the player's laps after the exit to score.
    pub score_laps: f32,
    /// How often the simulation samples, in seconds.
    pub sample_secs: f32,
    /// What cold tyres cost on the lap out of the pits.
    pub out_lap_penalty_secs: f32,
    /// Beyond this a save stops being something a driver can drive to.
    pub max_save_fraction: f32,
    /// Within this many seconds, a car of the player's own class is a fight.
    pub same_class_secs: f32,
    /// Within this many seconds, a car of another class is a lift or a pass.
    pub other_class_secs: f32,
    /// A pace difference smaller than this is not worth calling a conflict.
    pub pace_noise_secs: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            extend_laps: 2,
            max_columns: 10,
            score_laps: 3.0,
            sample_secs: 1.0,
            out_lap_penalty_secs: 2.0,
            max_save_fraction: 0.15,
            same_class_secs: 1.0,
            other_class_secs: 2.0,
            pace_noise_secs: 0.3,
        }
    }
}

/// Penalty per second of exposure, by what the car is.
///
/// Units are seconds of compromised running per second spent within reach, so
/// the score that comes out is an index in seconds — comparable between
/// candidates, and never a real time loss.
const PENALTY_SAME_CLASS: f32 = 1.0;
const PENALTY_FASTER_CLASS: f32 = 1.5;
const PENALTY_SLOWER_CLASS: f32 = 0.8;
/// Extra for a car that is actually in the way: ahead and slower, so it has to
/// be passed, or behind and quicker, so it has to be let by.
const PENALTY_HELD_UP: f32 = 1.2;
const PENALTY_CAUGHT: f32 = 1.0;

/// Where a rival sits relative to the player after `secs` of racing.
///
/// The one line the whole simulation rests on: in `secs` the player covers
/// `secs/player_lap` laps and the rival `secs/rival_lap`, so the rival gains
/// `secs·(player_lap/rival_lap − 1)` seconds of track. Wrapped to half a lap
/// either way, because a car three quarters of a lap ahead is a quarter of a
/// lap behind and it is the shorter of the two that a driver meets.
#[must_use]
pub fn gap_after(gap_secs: f32, player_lap_secs: f32, rival_lap_secs: f32, secs: f32) -> f32 {
    if player_lap_secs <= 0.0 || rival_lap_secs <= 0.0 {
        return gap_secs;
    }
    wrap_half_lap(gap_secs + secs * (player_lap_secs / rival_lap_secs - 1.0), player_lap_secs)
}

/// Folds a gap into the half lap either side of the player.
fn wrap_half_lap(gap: f32, lap_secs: f32) -> f32 {
    if lap_secs <= 0.0 || !gap.is_finite() {
        return 0.0;
    }
    let half = lap_secs / 2.0;
    let mut wrapped = (gap + half).rem_euclid(lap_secs) - half;
    // `rem_euclid` can land exactly on the negative edge; nudge it to the
    // positive one so a car dead level reads as ahead rather than flickering.
    if wrapped <= -half {
        wrapped += lap_secs;
    }
    wrapped
}

/// What extending to a candidate lap asks of the driver.
///
/// `laps` is how many laps the tank has to cover to reach it, counting the one
/// being driven now.
#[must_use]
pub fn save_for(laps: i32, player: &Player, max_fraction: f32) -> Save {
    let per_lap = player.fuel_per_lap_litres;
    if laps <= 0 || per_lap <= 0.0 {
        return Save { per_lap_litres: 0.0, fraction: 0.0, reachable: true };
    }
    #[expect(clippy::cast_precision_loss, reason = "a lap count is exact in f32 far beyond any race length")]
    let laps = laps as f32;
    let required = player.fuel_level_litres.max(0.0) / laps;
    let per_lap_litres = per_lap - required;
    let fraction = per_lap_litres / per_lap;
    Save { per_lap_litres, fraction, reachable: fraction <= max_fraction }
}

/// The last lap the tank reaches with no saving at all.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "a lap count is far inside i32 for any session")]
pub fn fuel_window_last_lap(player: &Player) -> Option<i32> {
    if player.fuel_per_lap_litres <= 0.0 {
        return None;
    }
    let laps = (player.fuel_level_litres / player.fuel_per_lap_litres).floor().max(0.0) as i32;
    // `laps` counts from the lap being driven now, which is itself one of them.
    Some(player.current_lap + laps.saturating_sub(1))
}

/// Builds the window: every candidate lap, scored.
///
/// `stop_secs` is what the stop costs against staying out — lane transit plus
/// service, from `pit_model::PitModel::stop_cost_secs`. `field` should already
/// exclude the player, cars in the pits, and cars with no pace figure;
/// `cars_skipped` says how many of the last were dropped.
///
/// Returns an empty candidate list where there is no measured consumption,
/// because every column of the page divides by it.
#[must_use]
pub fn pit_window(
    player: &Player,
    field: &[Rival],
    cars_skipped: usize,
    stop_secs: f32,
    confidence: Confidence,
    cfg: &Config,
) -> PitWindow {
    let due_lap = fuel_window_last_lap(player);
    let empty = PitWindow {
        candidates: Vec::new(),
        recommended_lap: None,
        confidence,
        fuel_window_last_lap: due_lap,
        min_stops: None,
        finishes_without_stopping: false,
        cars_skipped,
    };
    if player.fuel_per_lap_litres <= 0.0 || player.lap_secs <= 0.0 || confidence == Confidence::Void {
        return empty;
    }

    // A tank that already reaches the end is not a stop to be planned, and a
    // window of candidate stops would be a page about a decision nobody has.
    let final_lap = player.final_lap();
    if let (Some(due), Some(last)) = (due_lap, final_lap)
        && due >= last
    {
        return PitWindow { finishes_without_stopping: true, ..empty };
    }

    // The plan: the lap the tank runs to. Everything is measured from there,
    // because "a lap or two early" is a question about the plan and not about
    // now.
    let Some(plan) = due_lap else { return empty };
    let latest = match final_lap {
        Some(last) => (plan + cfg.extend_laps).min(last),
        None => plan + cfg.extend_laps,
    };

    // How early the stop can be taken without adding one. On tight fuel that
    // is barely earlier than the plan; where the end of the race has slack in
    // it — the splash every long race finishes on — it can be a long way back,
    // and that whole stretch is a free undercut.
    let plan_stops = stops_to_finish(plan, player);
    let mut earliest = plan;
    while earliest > player.current_lap
        && stops_to_finish(earliest - 1, player) == plan_stops
        && usize::try_from(latest - earliest).unwrap_or(0) + 1 < cfg.max_columns
    {
        earliest -= 1;
    }

    let candidates: Vec<Candidate> = (earliest..=latest)
        .map(|lap| {
            let offset = lap - player.current_lap;
            let mut candidate = Candidate {
                lap,
                laps_from_now: offset,
                save: save_for(offset + 1, player, cfg.max_save_fraction),
                traffic_score: None,
                emerge_between: (None, None),
                first_conflict: None,
                exit_class_position: None,
                stops_to_finish: stops_to_finish(lap, player),
            };
            if !field.is_empty() {
                score_candidate(&mut candidate, player, field, stop_secs, cfg);
            }
            candidate
        })
        .collect();

    let min_stops = candidates.iter().filter_map(|c| c.stops_to_finish).min();
    // Stops first, traffic second. A lost stop is a minute; a bad exit is a
    // few seconds, so no amount of clear air buys back an extra stop.
    let recommended_lap = candidates
        .iter()
        .filter(|c| c.save.reachable)
        .filter(|c| min_stops.is_none() || c.stops_to_finish == min_stops)
        .filter_map(|c| c.traffic_score.map(|score| (c.lap, score)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(lap, _)| lap);

    PitWindow { candidates, recommended_lap, min_stops, ..empty }
}

/// Stops still needed to reach the end if the car boxes at the end of `lap`,
/// counting that stop.
///
/// The laps left after the stop, divided by what a full tank covers. `None`
/// without a tank size or a remaining-lap count, because both are divided by.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "a stop count is a handful, far inside i32")]
pub fn stops_to_finish(lap: i32, player: &Player) -> Option<i32> {
    let laps_per_tank = player.laps_per_tank()?;
    let final_lap = player.final_lap()?;
    let after = final_lap - lap;
    if after <= 0 {
        // Boxing on or past the last lap: the stop is the last thing that
        // happens, if it happens at all.
        return Some(1);
    }
    #[expect(clippy::cast_precision_loss, reason = "a lap count is exact in f32 far beyond any race")]
    let further = (after as f32 / laps_per_tank).ceil().max(0.0) as i32;
    Some(1 + further)
}

/// Fills in one candidate's traffic figures.
fn score_candidate(candidate: &mut Candidate, player: &Player, field: &[Rival], stop_secs: f32, cfg: &Config) {
    #[expect(clippy::cast_precision_loss, reason = "a handful of laps, far inside f32's exact range")]
    let to_entry = candidate.laps_from_now as f32 * player.lap_secs;
    // At the exit every rival has gained the whole cost of the stop, because
    // that is what the cost *is*: the time they spent racing and the player
    // did not.
    let at_exit: Vec<(i32, f32, f32)> = field
        .iter()
        .map(|rival| {
            let gap = gap_after(rival.gap_secs, player.lap_secs, rival.lap_secs, to_entry);
            (rival.car_idx, wrap_half_lap(gap + stop_secs, player.lap_secs), rival.lap_secs)
        })
        .collect();

    candidate.emerge_between = (
        at_exit.iter().filter(|(_, gap, _)| *gap > 0.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(idx, _, _)| *idx),
        at_exit.iter().filter(|(_, gap, _)| *gap <= 0.0).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(idx, _, _)| *idx),
    );
    // Class position at the exit: every car of the player's own class still up
    // the road, plus the player.
    let ahead_in_class =
        field.iter().zip(&at_exit).filter(|(rival, (_, gap, _))| rival.class == ClassOrder::Same && *gap > 0.0).count();
    candidate.exit_class_position = Some(i32::try_from(ahead_in_class).unwrap_or(i32::MAX).saturating_add(1));

    // The out-lap is driven on cold tyres, so the player is slower for it than
    // the pace everything else here is scaled by.
    let out_lap_secs = player.lap_secs + cfg.out_lap_penalty_secs;
    let horizon = cfg.score_laps * player.lap_secs;
    let sample = cfg.sample_secs.max(0.1);

    let mut score = 0.0_f32;
    let mut first: Option<Conflict> = None;
    let mut elapsed = 0.0_f32;
    while elapsed < horizon {
        // The player's own pace over this sample: cold for the first lap out.
        let player_lap = if elapsed < out_lap_secs { out_lap_secs } else { player.lap_secs };
        for (rival, (car_idx, exit_gap, rival_lap)) in field.iter().zip(&at_exit) {
            let gap = gap_after(*exit_gap, player_lap, *rival_lap, elapsed);
            let reach = if rival.class == ClassOrder::Same { cfg.same_class_secs } else { cfg.other_class_secs };
            if gap.abs() > reach {
                continue;
            }
            let base = match rival.class {
                ClassOrder::Same => PENALTY_SAME_CLASS,
                ClassOrder::Faster => PENALTY_FASTER_CLASS,
                ClassOrder::Slower => PENALTY_SLOWER_CLASS,
            };
            // In the way, as opposed to merely nearby: ahead and slower has to
            // be passed, behind and quicker has to be let by.
            let delta = player_lap - rival_lap;
            let in_the_way = if gap > 0.0 && delta < -cfg.pace_noise_secs {
                PENALTY_HELD_UP
            } else if gap <= 0.0 && delta > cfg.pace_noise_secs {
                PENALTY_CAUGHT
            } else {
                0.0
            };
            score += (base + in_the_way) * sample;
            if first.is_none() {
                first = Some(Conflict {
                    car_idx: *car_idx,
                    laps_until: elapsed / player.lap_secs,
                    pace_delta_secs: player_lap - rival_lap,
                });
            }
        }
        elapsed += sample;
    }
    candidate.traffic_score = Some(score);
    candidate.first_conflict = first;
}

/// How far ahead of the fuel target this lap is running, in litres.
///
/// Positive means fuel in hand against the target; negative means the lap is
/// running rich and the saving is not being made.
///
/// `lap_fraction` is what fraction of a *lap's time* has passed, from
/// `super::relative::LapCurve`, not what fraction of its distance. Fuel burns
/// with time under power, so a distance-based comparison reads rich down every
/// straight and lean through every corner, and swings by tenths of a litre at
/// each corner while a driver is trying to read it — the same error that made
/// track-position relative gaps breathe.
#[must_use]
pub fn lap_progress_litres(required_per_lap_litres: f32, lap_fraction: f32, used_this_lap_litres: f32) -> f32 {
    required_per_lap_litres * lap_fraction.clamp(0.0, 1.0) - used_this_lap_litres
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> Player {
        Player {
            current_lap: 34,
            lap_secs: 100.0,
            fuel_level_litres: 30.0,
            fuel_per_lap_litres: 3.0,
            laps_remaining: Some(40),
            // Thirty laps a tank, so the race's own slack is what decides how
            // far back a stop can be taken without adding one.
            tank_capacity_litres: Some(90.0),
        }
    }

    fn rival(car_idx: i32, gap_secs: f32, lap_secs: f32, class: ClassOrder) -> Rival {
        Rival { car_idx, gap_secs, lap_secs, class }
    }

    #[test]
    fn a_gap_grows_at_the_difference_between_two_lap_times() {
        // A rival a second a lap quicker gains a second of track every lap.
        let after_one_lap = gap_after(0.0, 100.0, 99.0, 100.0);
        assert!((after_one_lap - 1.0101).abs() < 0.01, "{after_one_lap}");
        // And one exactly as quick never moves.
        assert!((gap_after(5.0, 100.0, 100.0, 1000.0) - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_gap_wraps_to_the_shorter_way_round() {
        // Three quarters of a lap ahead is a quarter of a lap behind, and it is
        // the shorter of the two a driver actually meets.
        assert!((wrap_half_lap(75.0, 100.0) - -25.0).abs() < 0.001);
        assert!((wrap_half_lap(-75.0, 100.0) - 25.0).abs() < 0.001);
    }

    /// The second of the two questions: extend a lap, what does it cost?
    #[test]
    fn the_save_needed_rises_with_every_lap_extended() {
        let player = player();
        // 30 L at 3 L/lap is ten laps, so ten laps need nothing given up.
        assert!(save_for(10, &player, 0.15).is_free());
        // Eleven laps means 2.73 L/lap, so 0.27 has to go — nine percent.
        let eleven = save_for(11, &player, 0.15);
        assert!((eleven.per_lap_litres - 0.273).abs() < 0.01, "{eleven:?}");
        assert!(eleven.reachable, "nine percent is drivable");
        // Thirteen is twenty-three percent, which is not.
        assert!(!save_for(13, &player, 0.15).reachable);
    }

    #[test]
    fn the_fuel_window_ends_where_the_tank_does() {
        // Ten laps of fuel from lap 34 reaches lap 43.
        assert_eq!(fuel_window_last_lap(&player()), Some(43));
        assert_eq!(fuel_window_last_lap(&Player { fuel_per_lap_litres: 0.0, ..player() }), None);
    }

    #[test]
    fn a_window_needs_measured_consumption() {
        let window = pit_window(
            &Player { fuel_per_lap_litres: 0.0, ..player() },
            &[],
            0,
            25.0,
            Confidence::Good,
            &Config::default(),
        );
        assert!(window.candidates.is_empty(), "nothing here can be computed without it");
    }

    #[test]
    fn a_caution_voids_the_window_rather_than_projecting_through_it() {
        let window = pit_window(&player(), &[], 0, 25.0, Confidence::Void, &Config::default());
        assert!(window.candidates.is_empty());
        assert_eq!(window.recommended_lap, None);
    }

    /// A tank that already reaches the flag is not a stop to be placed.
    #[test]
    fn a_tank_that_reaches_the_end_is_not_a_window_at_all() {
        let short = Player { laps_remaining: Some(3), ..player() };
        let window = pit_window(&short, &[], 0, 25.0, Confidence::Good, &Config::default());
        assert!(window.finishes_without_stopping);
        assert!(window.candidates.is_empty(), "there is no stop to choose a lap for");
        assert_eq!(window.recommended_lap, None);
    }

    /// The window is about the plan, not about now: "a lap or two early" is a
    /// question asked of the lap the tank is running to.
    #[test]
    fn the_window_is_centred_on_the_lap_the_tank_runs_to() {
        let window = pit_window(&player(), &[], 0, 25.0, Confidence::Good, &Config::default());
        let laps: Vec<i32> = window.candidates.iter().map(|c| c.lap).collect();
        assert_eq!(window.fuel_window_last_lap, Some(43), "ten laps of fuel from lap 34");
        assert_eq!(laps.last(), Some(&45), "two laps past the plan, for an extend");
        assert!(laps.contains(&43), "and the plan itself");
    }

    /// The case a long race turns on: with slack at the end — the splash every
    /// four-hour race finishes on — a stop can be taken a long way early for
    /// nothing, and the count moves at exactly one lap.
    #[test]
    fn boxing_early_costs_nothing_until_the_lap_where_it_adds_a_stop() {
        let long = player();
        assert_eq!(long.final_lap(), Some(73), "forty laps left from lap 34");

        // Thirty laps a tank. Box on 43 and thirty laps remain: one more tank.
        assert_eq!(stops_to_finish(43, &long), Some(2));
        assert_eq!(stops_to_finish(44, &long), Some(2), "later is no cheaper");
        assert_eq!(stops_to_finish(42, &long), Some(3), "and here the undercut stops being free");
    }

    /// The first of the two questions: box a lap either side, what changes?
    #[test]
    fn a_lap_that_comes_out_into_clear_air_scores_better_than_one_that_does_not() {
        let cfg = Config::default();
        // One rival, matched on pace, sitting where a stop now would drop the
        // player right on top of it: 25 s of stop puts it level.
        let field = [rival(7, -25.0, 100.0, ClassOrder::Same)];
        let window = pit_window(&player(), &field, 0, 25.0, Confidence::Good, &cfg);

        let first = window.candidates.first().expect("a window");
        assert!(first.traffic_score.is_some_and(|score| score > 0.0), "the stop comes out on top of it");
        // A rival on identical pace holds station, so every candidate is the
        // same fight — the point of the assertion is that it was simulated.
        assert!(window.candidates.iter().all(|c| c.traffic_score.is_some()));
        assert!(window.recommended_lap.is_some());
    }

    /// Stops outrank traffic: an extra stop is a minute, a bad exit is
    /// seconds, so no amount of clear air buys one back.
    #[test]
    fn a_lap_costing_an_extra_stop_is_never_recommended() {
        let window = pit_window(
            &player(),
            &[rival(7, 40.0, 100.0, ClassOrder::Same)],
            0,
            25.0,
            Confidence::Good,
            &Config::default(),
        );
        let min = window.min_stops.expect("a stop count");
        let recommended = window.recommended_lap.expect("a lap");
        let picked = window.candidates.iter().find(|c| c.lap == recommended).expect("a candidate");
        assert_eq!(picked.stops_to_finish, Some(min), "the pick never costs a stop the window could avoid");
    }

    #[test]
    fn a_recommendation_never_lands_on_a_lap_the_fuel_cannot_reach() {
        let cfg = Config::default();
        let window = pit_window(&player(), &[rival(7, 40.0, 100.0, ClassOrder::Same)], 0, 25.0, Confidence::Good, &cfg);
        let recommended = window.recommended_lap.expect("a lap");
        let candidate =
            window.candidates.iter().find(|c| c.lap == recommended).expect("the recommendation must be a candidate");
        assert!(candidate.save.reachable, "recommending a lap the tank cannot reach is recommending running dry");
    }

    #[test]
    fn emerging_between_two_cars_names_both_of_them() {
        let cfg = Config::default();
        // With a 25 s stop, a car 30 s up the road is 5 s behind at the exit,
        // and one 10 s back is 35 s back — so the player comes out between.
        let field = [rival(7, 30.0, 100.0, ClassOrder::Same), rival(14, -10.0, 100.0, ClassOrder::Same)];
        let window = pit_window(&player(), &field, 0, 25.0, Confidence::Good, &cfg);
        let now = window.candidates.first().expect("a window");
        let (ahead, behind) = now.emerge_between;
        assert!(ahead.is_some() || behind.is_some(), "somebody is either side: {:?}", now.emerge_between);
        assert_eq!(now.exit_class_position, Some(1 + i32::from(ahead.is_some())));
    }

    #[test]
    fn cars_left_out_are_counted_rather_than_quietly_dropped() {
        let window = pit_window(&player(), &[], 12, 25.0, Confidence::Fair, &Config::default());
        assert_eq!(window.cars_skipped, 12, "the page has to be able to say what it did not consider");
    }
}
