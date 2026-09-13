//! Driver-strength evidence for a team entry.
//!
//! Session-info YAML names only the driver currently seated in each car.  It
//! does not contain a team's full roster, so this tracker deliberately builds
//! a *known* roster from drivers it has encountered and marks every result
//! provisional.  It never groups `TeamID: 0`: recorded solo entries all use
//! that value.
//!
//! Rating and on-track pace remain separate bases.  A driver first compares by
//! iRating.  Once at least two known drivers have each finished a fully
//! observed, clean stint, drivers with measured pace compare by clean average
//! lap time; an active driver without that evidence remains on the rating
//! basis.  No output compares an iRating with a lap time.

use std::collections::HashMap;

/// A driver can remain in for many hours. Pace is deliberately recent-stint
/// bounded rather than retaining an unbounded vector until the next stop.
const MAX_CLEAN_LAPS_PER_CANDIDATE: usize = 128;

/// The source used for one driver's relative-strength rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrengthBasis {
    /// The driver's current session-info iRating, larger being stronger.
    Rating,
    /// Clean completed-stint average, smaller being faster.
    CleanPace,
}

/// Whether a newly published scored lap can contribute to a clean stint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LapDisposition {
    /// A normal racing lap. Its time is still subjected to a robust outlier
    /// filter when the stint closes.
    Clean,
    /// A pit, caution, missing, or already-detected outlier lap.
    #[allow(dead_code, reason = "the session bridge will supply this case when it wires the tracker")]
    Exclude,
}

/// An integration's evidence about a stint boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StintBoundary {
    /// No boundary was established this tick.
    None,
    /// The current driver's first lap was witnessed. Only this begins a stint
    /// eligible for pace measurement; joining in the middle is not enough.
    ObservedStart,
    /// The current driver's exit was witnessed and confirmed. This is the
    /// only signal which can publish a measured stint.
    ConfirmedCompletion,
    /// A possible boundary, including an estimator's blind-stop inference.
    /// It cannot complete or terminate a candidate; the integration excludes
    /// the associated pit-like lap and keeps the same driver's double stint.
    UnconfirmedBoundary,
}

/// Current information about one active entry row.
///
/// One value is supplied on every telemetry tick for each active team car.
/// `scored_lap` and `last_lap_secs` are considered only once for a strictly
/// newer, successfully parsed session-info revision.
#[derive(Debug, Clone, Copy)]
pub struct TeamDriverObservation<'a> {
    /// The stable iRacing car slot. A driver swap keeps this value.
    pub car_idx: i32,
    /// A positive `Driver.TeamID`. Do not pass the SDK's zero solo-entry
    /// value, because it would put unrelated drivers on one team.
    pub team_id: Option<i32>,
    /// The current `Driver.UserID`; a swap changes it while `car_idx` stays.
    pub user_id: Option<i32>,
    pub driver_name: &'a str,
    /// A positive iRating, if the current YAML row publishes one.
    pub irating: Option<i32>,
    pub class_id: i32,
    /// `ResultsPosition.LapsComplete` for this car from this YAML revision.
    pub scored_lap: Option<i32>,
    /// The just-published last-lap time for `scored_lap`.
    pub last_lap_secs: Option<f32>,
    pub lap_disposition: LapDisposition,
    /// False if the SDK receiver or identity stream had a gap. This is about
    /// telemetry continuity, never whether the car was rendered or in-world:
    /// verified session-info laps may qualify while a car is `NotInWorld`.
    pub continuously_observed: bool,
    pub boundary: StintBoundary,
}

/// Inputs to one [`TeamDriverPaceTracker::update`] call.
#[derive(Debug, Clone, Copy)]
pub struct TeamDriverTick<'a> {
    /// Monotonic session seconds, retained for future presentation policy.
    pub now_secs: f64,
    /// A successful session-info revision. `None`, repeats, and regressions do
    /// not create lap evidence.
    pub scoring_revision: Option<u64>,
    pub active_cars: &'a [TeamDriverObservation<'a>],
}

/// The relative-strength result for one currently active car.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamDriverStrength {
    pub car_idx: i32,
    pub team_id: i32,
    pub user_id: i32,
    pub basis: StrengthBasis,
    /// One is strongest. `None` means this driver has no usable value for the
    /// selected basis yet.
    pub rank: Option<usize>,
    /// Number of known team drivers with a comparable value for this basis.
    pub compared_count: usize,
    /// Signed, presentation-ready grade from -3 through +3. Positive means
    /// stronger/faster; zero means no comparative evidence or the midpoint.
    pub chevrons: i8,
    /// Number of unique drivers encountered for this team during this run.
    pub known_count: usize,
    /// Always true for SDK-only data: inactive team members are not published
    /// in the `Drivers` list, so this is a discovered roster, not a complete
    /// one.
    pub provisional: bool,
}

/// One row for the strength hover's known-team list.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownTeamDriver {
    pub team_id: i32,
    pub user_id: i32,
    pub driver_name: String,
    pub irating: Option<i32>,
    pub clean_average_lap_secs: Option<f32>,
    pub completed_clean_stints: usize,
    pub active: bool,
    /// See [`TeamDriverStrength::provisional`].
    pub provisional: bool,
}

/// Values the snapshot builder can copy into the standings rows and hover.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamDriverFrame {
    pub strengths: Vec<TeamDriverStrength>,
    pub known_drivers: Vec<KnownTeamDriver>,
}

/// Caches encountered drivers and admits completed clean stints as pace
/// evidence. Construct one tracker per subsession; a session reset creates a
/// fresh tracker rather than carrying ratings or pace into a new race.
#[derive(Debug, Default)]
pub struct TeamDriverPaceTracker {
    drivers: HashMap<(i32, i32), DriverRecord>,
    active_cars: HashMap<i32, ActiveDriver>,
    stints: HashMap<i32, CandidateStint>,
    last_scoring_revision: Option<u64>,
}

#[derive(Debug, Clone)]
struct DriverRecord {
    name: String,
    irating: Option<i32>,
    class_id: i32,
    pace_sum_secs: f64,
    pace_laps: usize,
    completed_clean_stints: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveDriver {
    team_id: i32,
    user_id: i32,
}

#[derive(Debug, Clone)]
struct CandidateStint {
    team_id: i32,
    user_id: i32,
    class_id: i32,
    last_scored_lap: Option<i32>,
    continuously_observed: bool,
    clean_laps: Vec<f32>,
}

impl TeamDriverPaceTracker {
    /// Incorporates a telemetry tick and returns only active-car strength plus
    /// the cached known-driver hover rows.
    #[must_use]
    pub fn update(&mut self, tick: TeamDriverTick<'_>) -> TeamDriverFrame {
        if !tick.now_secs.is_finite() {
            return self.frame();
        }
        let fresh_scoring =
            tick.scoring_revision.is_some_and(|revision| self.last_scoring_revision.is_none_or(|last| revision > last));
        if fresh_scoring {
            self.last_scoring_revision = tick.scoring_revision;
        }

        for observation in tick.active_cars {
            let Some((team_id, user_id)) = valid_identity(*observation) else {
                self.active_cars.remove(&observation.car_idx);
                self.stints.remove(&observation.car_idx);
                continue;
            };
            let identity = ActiveDriver { team_id, user_id };
            let previous = self.active_cars.get(&observation.car_idx).copied();
            let changed_driver = previous.is_some_and(|old| old != identity);
            if changed_driver {
                // The rewritten current-driver row is a confirmed swap. Close
                // the outgoing candidate before taking the new row's lap as a
                // baseline: that lap cannot belong to the incoming driver.
                self.finish_stint(observation.car_idx);
            }
            self.active_cars.insert(observation.car_idx, identity);
            self.remember_driver(team_id, user_id, observation);

            if changed_driver || matches!(observation.boundary, StintBoundary::ObservedStart) {
                self.start_stint(*observation, team_id, user_id);
            }

            if let Some(stint) = self.stints.get_mut(&observation.car_idx) {
                stint.continuously_observed &= observation.continuously_observed;
                if fresh_scoring {
                    observe_lap(stint, observation);
                }
            }

            match observation.boundary {
                StintBoundary::ConfirmedCompletion if !changed_driver => {
                    self.finish_stint(observation.car_idx);
                    // A same-driver stop has ended one confirmed stint, not
                    // the driver's race. The scoring lap is the new
                    // candidate's baseline and is never counted twice.
                    self.start_stint(*observation, team_id, user_id);
                }
                // A blind stop can make this lap non-clean, but does not end a
                // driver's stint: the same driver may refuel and double stint.
                StintBoundary::None
                | StintBoundary::ObservedStart
                | StintBoundary::UnconfirmedBoundary
                | StintBoundary::ConfirmedCompletion => {}
            }
        }

        self.frame()
    }

    fn remember_driver(&mut self, team_id: i32, user_id: i32, observation: &TeamDriverObservation<'_>) {
        let driver = self.drivers.entry((team_id, user_id)).or_insert_with(|| DriverRecord {
            name: observation.driver_name.to_owned(),
            irating: observation.irating.filter(|rating| *rating > 0),
            class_id: observation.class_id,
            pace_sum_secs: 0.0,
            pace_laps: 0,
            completed_clean_stints: 0,
        });
        if !observation.driver_name.is_empty() && driver.name != observation.driver_name {
            observation.driver_name.clone_into(&mut driver.name);
        }
        if let Some(rating) = observation.irating.filter(|rating| *rating > 0) {
            driver.irating = Some(rating);
        }
        if driver.class_id != observation.class_id {
            // A session's team entry should not change class. If malformed
            // input says it did, do not blend incompatible lap-time scales.
            driver.class_id = observation.class_id;
            driver.pace_sum_secs = 0.0;
            driver.pace_laps = 0;
            driver.completed_clean_stints = 0;
        }
    }

    fn start_stint(&mut self, observation: TeamDriverObservation<'_>, team_id: i32, user_id: i32) {
        self.stints.insert(
            observation.car_idx,
            CandidateStint {
                team_id,
                user_id,
                class_id: observation.class_id,
                last_scored_lap: observation.scored_lap,
                continuously_observed: observation.continuously_observed,
                clean_laps: Vec::new(),
            },
        );
    }

    fn finish_stint(&mut self, car_idx: i32) {
        let Some(stint) = self.stints.remove(&car_idx) else { return };
        if !stint.continuously_observed {
            return;
        }
        let Some((sum_secs, laps)) = robust_clean_laps(&stint.clean_laps) else { return };
        let Some(driver) = self.drivers.get_mut(&(stint.team_id, stint.user_id)) else { return };
        if driver.class_id != stint.class_id {
            return;
        }
        driver.pace_sum_secs += sum_secs;
        driver.pace_laps += laps;
        driver.completed_clean_stints += 1;
    }

    fn frame(&self) -> TeamDriverFrame {
        let mut strengths =
            self.active_cars.iter().map(|(&car_idx, &active)| self.strength(car_idx, active)).collect::<Vec<_>>();
        strengths.sort_unstable_by_key(|strength| strength.car_idx);

        let mut known_drivers = self
            .drivers
            .iter()
            .map(|(&(team_id, user_id), record)| KnownTeamDriver {
                team_id,
                user_id,
                driver_name: record.name.clone(),
                irating: record.irating,
                clean_average_lap_secs: display_pace(record),
                completed_clean_stints: record.completed_clean_stints,
                active: self.active_cars.values().any(|active| active.team_id == team_id && active.user_id == user_id),
                provisional: true,
            })
            .collect::<Vec<_>>();
        known_drivers.sort_unstable_by(|left, right| {
            left.team_id
                .cmp(&right.team_id)
                .then_with(|| left.driver_name.cmp(&right.driver_name))
                .then_with(|| left.user_id.cmp(&right.user_id))
        });
        TeamDriverFrame { strengths, known_drivers }
    }

    fn strength(&self, car_idx: i32, active: ActiveDriver) -> TeamDriverStrength {
        let known_count = self.drivers.keys().filter(|(team_id, _)| *team_id == active.team_id).count();
        let paced_drivers = self
            .drivers
            .iter()
            .filter_map(|(&(team_id, user_id), record)| {
                (team_id == active.team_id).then_some((user_id, average_pace(record)))
            })
            .filter_map(|(user_id, pace)| pace.map(|pace| (user_id, pace)))
            .collect::<Vec<_>>();
        let active_pace = self.drivers.get(&(active.team_id, active.user_id)).and_then(average_pace);
        let (basis, rank, compared_count, grade_rank, grade_count) =
            if paced_drivers.len() >= 2 && active_pace.is_some() {
                let (rank, grade_rank, grade_count) = rank_pace(active.user_id, &paced_drivers).unwrap_or((0, 0, 0));
                (StrengthBasis::CleanPace, (rank != 0).then_some(rank), paced_drivers.len(), grade_rank, grade_count)
            } else {
                let rated_drivers = self
                    .drivers
                    .iter()
                    .filter_map(|(&(team_id, user_id), record)| {
                        (team_id == active.team_id).then_some((user_id, record.irating))
                    })
                    .filter_map(|(user_id, rating)| rating.map(|rating| (user_id, rating)))
                    .collect::<Vec<_>>();
                let (rank, grade_rank, grade_count) = rank_rating(active.user_id, &rated_drivers).unwrap_or((0, 0, 0));
                (StrengthBasis::Rating, (rank != 0).then_some(rank), rated_drivers.len(), grade_rank, grade_count)
            };
        TeamDriverStrength {
            car_idx,
            team_id: active.team_id,
            user_id: active.user_id,
            basis,
            rank,
            compared_count,
            chevrons: rank.map_or(0, |_| strength_grade(grade_rank, grade_count)),
            known_count,
            provisional: true,
        }
    }
}

fn valid_identity(observation: TeamDriverObservation<'_>) -> Option<(i32, i32)> {
    Some((observation.team_id.filter(|team_id| *team_id > 0)?, observation.user_id.filter(|user_id| *user_id > 0)?))
}

fn observe_lap(stint: &mut CandidateStint, observation: &TeamDriverObservation<'_>) {
    let Some(lap) = observation.scored_lap else {
        stint.continuously_observed = false;
        return;
    };
    let Some(previous) = stint.last_scored_lap else {
        stint.last_scored_lap = Some(lap);
        return;
    };
    if lap <= previous {
        return;
    }
    if lap != previous + 1 {
        stint.continuously_observed = false;
        stint.last_scored_lap = Some(lap);
        return;
    }
    stint.last_scored_lap = Some(lap);
    if observation.lap_disposition == LapDisposition::Clean
        && let Some(seconds) = observation.last_lap_secs.filter(|seconds| seconds.is_finite() && *seconds > 1.0)
    {
        if stint.clean_laps.len() == MAX_CLEAN_LAPS_PER_CANDIDATE {
            stint.clean_laps.remove(0);
        }
        stint.clean_laps.push(seconds);
    }
}

/// Retains laps within 15% of the median. The caller has already excluded
/// pit/caution/missing laps; this only catches a remaining timing outlier.
fn robust_clean_laps(laps: &[f32]) -> Option<(f64, usize)> {
    let mut sorted = laps.iter().copied().filter(|lap| lap.is_finite() && *lap > 1.0).collect::<Vec<_>>();
    sorted.sort_unstable_by(f32::total_cmp);
    let median = *sorted.get(sorted.len() / 2)?;
    let lower = median * 0.85;
    let upper = median * 1.15;
    let retained = sorted.into_iter().filter(|lap| *lap >= lower && *lap <= upper).collect::<Vec<_>>();
    (retained.len() >= 5).then(|| (retained.iter().map(|lap| f64::from(*lap)).sum(), retained.len()))
}

fn average_pace(driver: &DriverRecord) -> Option<f64> {
    if driver.pace_laps < 5 {
        return None;
    }
    let laps = u32::try_from(driver.pace_laps).ok()?;
    Some(driver.pace_sum_secs / f64::from(laps))
}

fn display_pace(driver: &DriverRecord) -> Option<f32> {
    // A lap time is presented alongside the SDK's f32 timings. The f64 sum
    // avoids accumulating error across stints; converting its final ordinary
    // race-lap result back to f32 loses no meaningful display precision.
    #[expect(clippy::cast_possible_truncation, reason = "display uses the SDK's f32 lap-time precision")]
    average_pace(driver).map(|seconds| seconds as f32)
}

fn rank_pace(user_id: i32, drivers: &[(i32, f64)]) -> Option<(usize, usize, usize)> {
    let value = drivers.iter().find(|(candidate, _)| *candidate == user_id)?.1;
    let rank = 1 + drivers.iter().filter(|(_, candidate)| candidate.total_cmp(&value).is_lt()).count();
    let mut values = drivers.iter().map(|(_, candidate)| *candidate).collect::<Vec<_>>();
    values.sort_unstable_by(f64::total_cmp);
    values.dedup_by(|left, right| left.total_cmp(right).is_eq());
    let grade_rank = values.iter().position(|candidate| candidate.total_cmp(&value).is_eq())? + 1;
    Some((rank, grade_rank, values.len()))
}

fn rank_rating(user_id: i32, drivers: &[(i32, i32)]) -> Option<(usize, usize, usize)> {
    let value = drivers.iter().find(|(candidate, _)| *candidate == user_id)?.1;
    let rank = 1 + drivers.iter().filter(|(_, candidate)| *candidate > value).count();
    let mut values = drivers.iter().map(|(_, candidate)| *candidate).collect::<Vec<_>>();
    values.sort_unstable_by(|left, right| right.cmp(left));
    values.dedup();
    let grade_rank = values.iter().position(|candidate| *candidate == value)? + 1;
    Some((rank, grade_rank, values.len()))
}

/// Maps a rank to a symmetric -3..3 grade. With six or more comparable
/// drivers, ranks 1-3 are green up values and the final three are red down
/// values. Smaller teams use the same scale without assigning a driver both.
fn strength_grade(rank: usize, count: usize) -> i8 {
    if count < 2 || rank == 0 || rank > count {
        return 0;
    }
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    let rank = i64::try_from(rank).unwrap_or(i64::MAX);
    let denominator = count - 1;
    let numerator = 3 * (count + 1 - 2 * rank);
    let rounded = if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        -((-numerator + denominator / 2) / denominator)
    };
    i8::try_from(rounded.clamp(-3, 3)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        car_idx: i32,
        user_id: i32,
        irating: i32,
        lap: i32,
        seconds: f32,
        boundary: StintBoundary,
    ) -> TeamDriverObservation<'static> {
        TeamDriverObservation {
            car_idx,
            team_id: Some(42),
            user_id: Some(user_id),
            driver_name: if user_id == 1 { "Fast" } else { "Steady" },
            irating: Some(irating),
            class_id: 7,
            scored_lap: Some(lap),
            last_lap_secs: Some(seconds),
            lap_disposition: LapDisposition::Clean,
            continuously_observed: true,
            boundary,
        }
    }

    #[test]
    fn measured_drivers_switch_to_clean_pace_only_after_two_completed_stints() {
        let mut tracker = TeamDriverPaceTracker::default();
        let start = [
            observation(3, 1, 1200, 0, 90.0, StintBoundary::ObservedStart),
            observation(4, 2, 2400, 0, 100.0, StintBoundary::ObservedStart),
        ];
        let before = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &start });
        assert!(before.strengths.iter().all(|strength| strength.basis == StrengthBasis::Rating));

        for lap in 1..=5 {
            let cars = [
                observation(3, 1, 1200, lap, 90.0, StintBoundary::None),
                observation(4, 2, 2400, lap, 100.0, StintBoundary::None),
            ];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap + 1).expect("positive")),
                active_cars: &cars,
            });
        }
        let complete = [
            observation(3, 1, 1200, 5, 90.0, StintBoundary::ConfirmedCompletion),
            observation(4, 2, 2400, 5, 100.0, StintBoundary::ConfirmedCompletion),
        ];
        let frame = tracker.update(TeamDriverTick { now_secs: 6.0, scoring_revision: Some(7), active_cars: &complete });
        let fast = frame.strengths.iter().find(|strength| strength.car_idx == 3).expect("fast row");
        let steady = frame.strengths.iter().find(|strength| strength.car_idx == 4).expect("steady row");
        assert_eq!(fast.basis, StrengthBasis::CleanPace);
        assert_eq!(fast.rank, Some(1));
        assert_eq!(fast.chevrons, 3);
        assert_eq!(steady.rank, Some(2));
        assert_eq!(steady.chevrons, -3);
        assert!(frame.known_drivers.iter().all(|driver| driver.provisional));
    }

    #[test]
    fn unconfirmed_boundary_does_not_end_a_same_driver_double_stint() {
        let mut tracker = TeamDriverPaceTracker::default();
        let start = [observation(3, 1, 1200, 0, 90.0, StintBoundary::ObservedStart)];
        let _ = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &start });
        for lap in 1..=5 {
            let car = [observation(3, 1, 1200, lap, 90.0, StintBoundary::None)];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap + 1).expect("positive")),
                active_cars: &car,
            });
        }
        let boundary = [observation(3, 1, 1200, 5, 90.0, StintBoundary::UnconfirmedBoundary)];
        let pending =
            tracker.update(TeamDriverTick { now_secs: 6.0, scoring_revision: Some(7), active_cars: &boundary });
        assert_eq!(pending.known_drivers[0].clean_average_lap_secs, None);
        let complete = [observation(3, 1, 1200, 5, 90.0, StintBoundary::ConfirmedCompletion)];
        let frame = tracker.update(TeamDriverTick { now_secs: 7.0, scoring_revision: Some(8), active_cars: &complete });
        assert_eq!(frame.known_drivers[0].clean_average_lap_secs, Some(90.0));
        assert_eq!(frame.known_drivers[0].completed_clean_stints, 1);
    }

    #[test]
    fn verified_scoring_laps_complete_the_outgoing_driver_on_a_swap() {
        let mut tracker = TeamDriverPaceTracker::default();
        let start = [observation(3, 1, 1200, 10, 90.0, StintBoundary::ObservedStart)];
        let _ = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &start });
        for lap in 11..=15 {
            let car = [observation(3, 1, 1200, lap, 90.0, StintBoundary::None)];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap - 9).expect("positive")),
                active_cars: &car,
            });
        }
        // The current-driver identity is the confirmed boundary. The current
        // scored lap belongs to the outgoing driver and starts the incoming
        // driver's candidate as a baseline, even if this car was NotInWorld.
        let swapped = [observation(3, 2, 2400, 15, 100.0, StintBoundary::None)];
        let frame = tracker.update(TeamDriverTick { now_secs: 6.0, scoring_revision: Some(7), active_cars: &swapped });
        let outgoing = frame.known_drivers.iter().find(|driver| driver.user_id == 1).expect("outgoing driver");
        assert_eq!(outgoing.completed_clean_stints, 1);
        assert_eq!(outgoing.clean_average_lap_secs, Some(90.0));
        assert!(frame.known_drivers.iter().any(|driver| driver.user_id == 2 && driver.active));
    }

    #[test]
    fn scoring_lap_jump_taints_the_stint() {
        let mut tracker = TeamDriverPaceTracker::default();
        let start = [observation(3, 1, 1200, 0, 90.0, StintBoundary::ObservedStart)];
        let _ = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &start });
        let jumped = [observation(3, 1, 1200, 3, 90.0, StintBoundary::None)];
        let _ = tracker.update(TeamDriverTick { now_secs: 1.0, scoring_revision: Some(2), active_cars: &jumped });
        for lap in 4..=8 {
            let car = [observation(3, 1, 1200, lap, 90.0, StintBoundary::None)];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap).expect("positive")),
                active_cars: &car,
            });
        }
        let complete = [observation(3, 1, 1200, 8, 90.0, StintBoundary::ConfirmedCompletion)];
        let frame = tracker.update(TeamDriverTick { now_secs: 9.0, scoring_revision: Some(9), active_cars: &complete });
        assert_eq!(frame.known_drivers[0].clean_average_lap_secs, None);
    }

    #[test]
    fn confirmed_same_driver_stint_restarts_from_its_completion_lap() {
        let mut tracker = TeamDriverPaceTracker::default();
        let start = [observation(3, 1, 1200, 0, 90.0, StintBoundary::ObservedStart)];
        let _ = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &start });
        for lap in 1..=5 {
            let car = [observation(3, 1, 1200, lap, 90.0, StintBoundary::None)];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap + 1).expect("positive")),
                active_cars: &car,
            });
        }
        let first_finish = [observation(3, 1, 1200, 5, 90.0, StintBoundary::ConfirmedCompletion)];
        let _ = tracker.update(TeamDriverTick { now_secs: 6.0, scoring_revision: Some(7), active_cars: &first_finish });
        for lap in 6..=10 {
            let car = [observation(3, 1, 1200, lap, 90.0, StintBoundary::None)];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap + 2).expect("positive")),
                active_cars: &car,
            });
        }
        let second_finish = [observation(3, 1, 1200, 10, 90.0, StintBoundary::ConfirmedCompletion)];
        let frame =
            tracker.update(TeamDriverTick { now_secs: 11.0, scoring_revision: Some(13), active_cars: &second_finish });
        assert_eq!(frame.known_drivers[0].completed_clean_stints, 2);
    }

    #[test]
    fn equal_ratings_share_rank_and_neutral_chevrons() {
        let mut tracker = TeamDriverPaceTracker::default();
        let active = [
            observation(3, 1, 1800, 0, 90.0, StintBoundary::None),
            observation(4, 2, 1800, 0, 90.0, StintBoundary::None),
        ];
        let frame = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &active });
        assert!(frame.strengths.iter().all(|strength| strength.rank == Some(1) && strength.chevrons == 0));
    }

    #[test]
    fn candidate_retains_only_the_latest_clean_laps() {
        let mut tracker = TeamDriverPaceTracker::default();
        let start = [observation(3, 1, 1200, 0, 90.0, StintBoundary::ObservedStart)];
        let _ = tracker.update(TeamDriverTick { now_secs: 0.0, scoring_revision: Some(1), active_cars: &start });
        for lap in 1..=133 {
            let seconds = if lap <= 5 { 90.0 } else { 100.0 };
            let car = [observation(3, 1, 1200, lap, seconds, StintBoundary::None)];
            let _ = tracker.update(TeamDriverTick {
                now_secs: f64::from(lap),
                scoring_revision: Some(u64::try_from(lap + 1).expect("positive")),
                active_cars: &car,
            });
        }
        let finish = [observation(3, 1, 1200, 133, 100.0, StintBoundary::ConfirmedCompletion)];
        let frame =
            tracker.update(TeamDriverTick { now_secs: 134.0, scoring_revision: Some(135), active_cars: &finish });
        assert_eq!(frame.known_drivers[0].clean_average_lap_secs, Some(100.0));
    }
}
