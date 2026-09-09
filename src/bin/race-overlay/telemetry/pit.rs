// Rust guideline compliant 2026-02-16

//! Pit-service state and the commands that change it.
//!
//! Everything the black box's Fuel and Tyres pages read, and everything they
//! send. The rule the whole design turns on: **a page never renders its own
//! intent**. Pressing a button emits a [`PitRequest`], which becomes an
//! iRacing broadcast message; what appears on screen is whatever the sim
//! reports on the next tick. iRacing silently ignores pit commands in plenty
//! of states — not in the car, service already under way — and a panel that
//! showed the request rather than the result would confidently display 55 L
//! armed when nothing at all is armed, on the one lap where being wrong is
//! expensive.

use iracing_telem::flags::PitCommand;
use serde::{Deserialize, Serialize};

/// The four corners, in the order the widget lays them out.
///
/// A plain enum rather than four booleans threaded everywhere: the tyre
/// commands, the pressure reads and the mock's 2x2 grid all need to talk
/// about "a corner", and naming it once keeps them agreeing.
///
/// `Serialize`/`Deserialize` so a [`PitRequest`] can cross the team-sync wire
/// as a crew-chief write — see `plans/team-sync.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corner {
    LeftFront,
    RightFront,
    LeftRear,
    RightRear,
}

impl Corner {
    /// All four, in layout order.
    pub const ALL: [Self; 4] = [Self::LeftFront, Self::RightFront, Self::LeftRear, Self::RightRear];

    /// The two-letter label the mock uses.
    #[must_use]
    pub fn short_name(self) -> &'static str {
        match self {
            Self::LeftFront => "LF",
            Self::RightFront => "RF",
            Self::LeftRear => "LR",
            Self::RightRear => "RR",
        }
    }

    /// This corner's pit command, carrying an optional cold pressure in kPa.
    ///
    /// Passing `None` arms the corner at whatever pressure is already set,
    /// which is what iRacing does for a bare tyre tick.
    #[must_use]
    fn command(self, pressure_kpa: Option<i16>) -> PitCommand {
        match self {
            Self::LeftFront => PitCommand::LF(pressure_kpa),
            Self::RightFront => PitCommand::RF(pressure_kpa),
            Self::LeftRear => PitCommand::LR(pressure_kpa),
            Self::RightRear => PitCommand::RR(pressure_kpa),
        }
    }
}

/// What iRacing currently has armed for the next stop, read back every tick.
///
/// Pressures are kPa. Note that `dpLFTireColdPress` and friends are labelled
/// `[Pa]` in the SDK's own variable metadata, but read identically to
/// `PitSvLFP`, which is labelled `[kPa]` — they are kPa. Taking the unit
/// string at face value would be a thousandfold error on every pressure the
/// widget shows.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "these mirror iRacing's own pit-service checkboxes one for one; grouping them into a bitfield would only obscure which box each is"
)]
pub struct PitService {
    /// Whether the sim has fuelling ticked for the next stop.
    pub fuel_armed: bool,
    /// Litres iRacing will add, as the sim reports it.
    pub fuel_amount_litres: f32,
    /// Litres currently in the tank.
    pub fuel_level_litres: f32,
    /// Tank size in litres, derived from level and level-percent — there is
    /// no capacity variable. `None` while the tank is near empty, where that
    /// division is too noisy to trust.
    pub tank_capacity_litres: Option<f32>,
    /// Litres this car has actually been using per lap, averaged over the
    /// last few completed laps. `None` until one has been completed.
    ///
    /// Measured rather than derived from `FuelUsePerHour`: that is a
    /// kilograms-per-hour figure, so turning it into litres per lap needs both
    /// a lap time and the fuel's density, and gets the answer wrong for every
    /// car whose density isn't 1. Watching the tank drain needs neither.
    pub fuel_per_lap_litres: Option<f32>,
    /// Per-corner tyre-change ticks, indexed by [`Corner::ALL`].
    pub tyres_armed: [bool; 4],
    /// Per-corner cold pressures in kPa, indexed by [`Corner::ALL`].
    pub tyre_pressures_kpa: [f32; 4],
    /// Which compound the next stop will fit, as `PitSvTireCompound` reports
    /// it — `0` for the car's first compound, `1` for its second, and so on.
    /// `None` for a car that doesn't publish it.
    ///
    /// A readout and never a control: iRacing's pit-command broadcast has no
    /// compound mode (see [`PitCommand`] — every variant it has is here in
    /// [`commands_for`]), so this can be shown but not set. The driver changes
    /// it in the sim's own black box, and this says what they changed it to.
    pub pending_tyre_compound: Option<i32>,
    pub tearoff_armed: bool,
    pub fast_repair_armed: bool,
    /// Fast repairs left this session. `255` is iRacing's "unlimited".
    pub fast_repairs_available: i32,
    /// Whether the player is in the car — pit commands are ignored otherwise,
    /// so the widget can say why nothing is happening instead of appearing
    /// broken.
    pub in_car: bool,
    /// Whether the car is on pit road — approaching the pits or stopped in its
    /// box, as opposed to out on track.
    ///
    /// Auto Fuel stops recalculating here: a lap that is part pit lane says
    /// nothing about racing pace or racing fuel use, and this is the moment
    /// the answer gets committed to the tank.
    pub on_pit_road: bool,
    /// The level the running fuel rig will stop at, while one is pumping this
    /// visit to the lane — see [`RefuelWatch`]. `None` the rest of the time,
    /// including the approach to the box, where the armed load genuinely sits
    /// on top of a still-falling level.
    pub refuel_target_litres: Option<f32>,
}

impl PitService {
    /// Whether this corner is ticked for the next stop.
    #[must_use]
    pub fn tyre_armed(&self, corner: Corner) -> bool {
        self.tyres_armed[corner_index(corner)]
    }

    /// Whether all four corners are ticked.
    #[must_use]
    pub fn all_tyres_armed(&self) -> bool {
        self.tyres_armed.iter().all(|armed| *armed)
    }

    /// This corner's armed cold pressure, in kPa.
    #[must_use]
    pub fn tyre_pressure_kpa(&self, corner: Corner) -> f32 {
        self.tyre_pressures_kpa[corner_index(corner)]
    }
}

/// Index into [`PitService`]'s per-corner arrays.
#[must_use]
pub fn corner_index(corner: Corner) -> usize {
    match corner {
        Corner::LeftFront => 0,
        Corner::RightFront => 1,
        Corner::LeftRear => 2,
        Corner::RightRear => 3,
    }
}

/// A change the user asked for, before it becomes broadcast messages.
///
/// Kept separate from [`PitCommand`] because one intent is not always one
/// command — see [`commands_for`].
///
/// `Serialize`/`Deserialize` so a crew chief's adjustment can travel the
/// team-sync ledger to the seated driver's overlay, which turns it into the
/// sim's own commands there — see `plans/team-sync.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PitRequest {
    /// Add this many litres, arming fuelling. Whole litres only; see
    /// [`commands_for`].
    SetFuel(i16),
    /// Untick fuelling entirely.
    ClearFuel,
    /// Tick or untick one corner, leaving the other three as they are.
    SetTyre {
        corner: Corner,
        armed: bool,
    },
    /// Tick or untick all four corners at once.
    SetAllTyres(bool),
    /// Set one corner's cold pressure in kPa, which also arms that corner.
    SetTyrePressure {
        corner: Corner,
        kpa: i16,
    },
    SetTearoff(bool),
    SetFastRepair(bool),
}

/// Turns one user intent into the broadcast messages that carry it out.
///
/// Most map one-to-one. Unticking a single tyre does not: iRacing has
/// `ClearTires`, which unticks all four, and no per-corner clear. Removing one
/// corner therefore means clearing all four and re-arming the others — at
/// their current pressures, so the round trip doesn't quietly reset them.
///
/// `current` is the sim's last reported state, which is what the re-arm is
/// rebuilt from; passing stale state would re-arm the wrong corners.
#[must_use]
pub fn commands_for(request: PitRequest, current: &PitService) -> Vec<PitCommand> {
    match request {
        // `PitCommand::Fuel` also ticks the fuel box, so this is one message.
        PitRequest::SetFuel(litres) => vec![PitCommand::Fuel(Some(litres))],
        PitRequest::ClearFuel => vec![PitCommand::ClearFuel],
        PitRequest::SetTyre { corner, armed } => {
            if armed {
                vec![corner.command(None)]
            } else {
                rebuild_tyres_without(corner, current)
            }
        }
        // Arming keeps each corner's current pressure — a bare tick, the same
        // as arming one corner on its own. `ClearTires` is iRacing's own
        // all-four clear, so the other direction is a single message.
        PitRequest::SetAllTyres(true) => Corner::ALL.into_iter().map(|corner| corner.command(None)).collect(),
        PitRequest::SetAllTyres(false) => vec![PitCommand::ClearTires],
        PitRequest::SetTyrePressure { corner, kpa } => vec![corner.command(Some(kpa))],
        PitRequest::SetTearoff(true) => vec![PitCommand::TearOff],
        PitRequest::SetTearoff(false) => vec![PitCommand::ClearWS],
        PitRequest::SetFastRepair(true) => vec![PitCommand::FastRepair],
        PitRequest::SetFastRepair(false) => vec![PitCommand::ClearFR],
    }
}

/// Clears every tyre, then re-arms all but `dropped` at their current
/// pressures — the only way to untick one corner (see [`commands_for`]).
fn rebuild_tyres_without(dropped: Corner, current: &PitService) -> Vec<PitCommand> {
    let mut commands = vec![PitCommand::ClearTires];
    for corner in Corner::ALL {
        if corner != dropped && current.tyre_armed(corner) {
            commands.push(corner.command(Some(round_kpa(current.tyre_pressure_kpa(corner)))));
        }
    }
    commands
}

/// Rounds a pressure to the whole kPa the command API accepts.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "tyre pressures are a few hundred kPa, far inside i16, and are clamped below"
)]
pub fn round_kpa(kpa: f32) -> i16 {
    if kpa.is_finite() { kpa.round().clamp(0.0, f32::from(i16::MAX)) as i16 } else { 0 }
}

/// Litres that must be **on board** for the car to reach the finish with
/// `margin_laps` in hand.
///
/// iRacing's own Auto Fuel is a black box UI feature with no SDK equivalent —
/// nothing publishes it and nothing accepts it — so this computes the same
/// thing from data that *is* published, and [`fuel_to_add_litres`] turns it
/// into a load to arm through the ordinary fuel command.
///
/// Deliberately separate from the load to add, and deliberately independent of
/// the current tank level. This half of the sum can only be worked out from
/// *racing* pace and *racing* consumption, so it is computed on track and
/// latched; the level it is measured against changes every second of the pit
/// lane and belongs to the other half. Latching a load-to-add instead meant the
/// litres that went in were measured against a tank reading from before the
/// in-lap, and every metre driven after that was fuel the car never got back.
///
/// `laps_remaining` is a count of **line crossings** — it includes the lap
/// under way as a whole lap, by [`super::endurance::laps_remaining`]'s
/// convention — but the part of that lap already behind the car needs no
/// fuel. `lap_driven_pct` is that part, and is subtracted, so a zero margin
/// means dry across the line rather than dry plus however much of a lap the
/// car happened to have driven when the target was latched. This mattered
/// most exactly where it was latched: the pit entry, near the end of a lap,
/// where charging for the whole crossing put nearly a full spare lap in the
/// tank on every stop. An unknown fraction counts as zero, which errs long.
///
/// Returns `None` when the answer would be a guess: before consumption has
/// been measured, or before the session length is known. A fuel figure that is
/// wrong is worse than one that is absent, because it will be trusted.
#[must_use]
pub fn fuel_to_finish_litres(
    laps_remaining: Option<i32>,
    lap_driven_pct: Option<f32>,
    fuel_per_lap_litres: Option<f32>,
    margin_laps: f32,
) -> Option<f32> {
    let per_lap = fuel_per_lap_litres.filter(|l| *l > 0.0 && l.is_finite())?;
    let crossings = f32::from(i16::try_from(laps_remaining?).ok()?);
    if !margin_laps.is_finite() || margin_laps < 0.0 {
        return None;
    }
    let driven = lap_driven_pct.filter(|pct| pct.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
    Some(((crossings - driven).max(0.0) + margin_laps) * per_lap)
}

/// Litres to put in now so that `needed_litres` ends up on board.
///
/// Rounds **up**. A litre too many costs a fraction of a second in the pit
/// lane; a litre too few ends the race.
///
/// `tank_capacity_litres` caps the answer only so that the figure this app
/// shows and the figure the sim reports back can agree — iRacing clips an
/// over-large request to the tank itself, and a panel showing a number the sim
/// disagreed with would have the arming logic re-sending forever chasing it.
/// It is deliberately an inferred figure (`FuelLevel / FuelLevelPct`, and
/// absent below a tenth of a tank) and so is never allowed to *decide* the
/// load: an unknown capacity means no cap at all, and the sim's own clip
/// stands in. A cap that guessed low is the one way this function could
/// under-fill a car, which is the failure it exists to avoid.
///
/// Both the level and the capacity must be **live** readings taken at the
/// moment the load is armed, not carried down the pit lane from the last lap.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped below to i16's range, and fuel loads are a few hundred litres at most"
)]
pub fn fuel_to_add_litres(needed_litres: f32, fuel_level_litres: f32, tank_capacity_litres: Option<f32>) -> i16 {
    if !needed_litres.is_finite() || !fuel_level_litres.is_finite() {
        return 0;
    }
    let to_add = (needed_litres - fuel_level_litres).max(0.0);
    let room = tank_capacity_litres
        .filter(|capacity| capacity.is_finite() && *capacity > 0.0)
        .map_or(to_add, |capacity| (capacity - fuel_level_litres).max(0.0));
    to_add.min(room).ceil().clamp(0.0, f32::from(i16::MAX)) as i16
}

/// A tank rise bigger than this means the rig is pumping, not the level
/// sensor wobbling. Well under a second of iRacing's delivery rate, so the
/// latch engages at the very start of a fill; comfortably above any jitter
/// the level reading has shown.
const FUEL_FLOWING_LITRES: f32 = 0.3;

/// Watches one visit to the pit lane for the fuel rig starting to pump, and
/// latches the level it will stop at.
///
/// While fuel is going in, iRacing keeps `PitSvFuel` at the full armed load
/// and raises `FuelLevel` toward it — so a gauge drawn as "level now, plus
/// the armed load on top" has its far edge crawling away as the tank fills,
/// when the truthful picture is a fixed far edge that the solid fill eats
/// into. The latch is that fixed edge: the lowest level seen this visit plus
/// the load armed when the rise began, which is where the rig will stop.
///
/// Latches only once a rise is seen, so an approach to the box — where the
/// level is still falling and the armed load genuinely does sit on top of a
/// moving level — reads `None` and the gauge draws it live as before.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_field_names,
    reason = "the litres postfix is the unit, which every fuel quantity in this module carries in its name"
)]
pub struct RefuelWatch {
    /// The lowest tank reading this visit — the level the fill started from.
    lowest_in_lane_litres: Option<f32>,
    /// Where the rig will stop, once a rise has shown it is running.
    target_litres: Option<f32>,
    /// The armed load the latch was taken against, so a load topped up
    /// mid-fill can move the edge out while the unchanged figure during
    /// plain delivery cannot.
    armed_at_latch_litres: f32,
}

impl RefuelWatch {
    /// Feeds one tick's readings; returns the fill's end level while one is
    /// under way this visit.
    ///
    /// `armed_litres` is the load set to go in — zero when the fuel box is
    /// unticked. Leaving pit road resets the watch for the next visit.
    pub fn update(&mut self, on_pit_road: bool, fuel_level_litres: f32, armed_litres: f32) -> Option<f32> {
        if !on_pit_road || !fuel_level_litres.is_finite() {
            *self = Self::default();
            return None;
        }
        let armed = if armed_litres.is_finite() { armed_litres.max(0.0) } else { 0.0 };
        let lowest = self.lowest_in_lane_litres.get_or_insert(fuel_level_litres);
        *lowest = lowest.min(fuel_level_litres);
        match self.target_litres {
            None => {
                if fuel_level_litres > *lowest + FUEL_FLOWING_LITRES {
                    self.target_litres = Some(*lowest + armed);
                    self.armed_at_latch_litres = armed;
                }
            }
            Some(target) => {
                // A driver adding to the load mid-fill moves the edge out;
                // the constant armed figure during delivery must not.
                if armed > self.armed_at_latch_litres {
                    self.target_litres = Some(target.max(fuel_level_litres + armed));
                    self.armed_at_latch_litres = armed;
                }
            }
        }
        self.target_litres
    }
}

/// Rounds a fuel load to the whole litres the command API accepts.
///
/// `PitCommand::Fuel` is an `i16` of litres, so tenths cannot be sent however
/// they are displayed. The widget therefore shows and steps whole litres
/// rather than a decimal it could never honour.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "fuel loads are at most a few hundred litres, far inside i16, and are clamped below"
)]
pub fn round_litres(litres: f32) -> i16 {
    if litres.is_finite() { litres.round().clamp(0.0, f32::from(i16::MAX)) as i16 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(tyres: [bool; 4], pressures: [f32; 4]) -> PitService {
        PitService { tyres_armed: tyres, tyre_pressures_kpa: pressures, ..PitService::default() }
    }

    #[test]
    fn arming_one_tyre_is_a_single_command() {
        let service = armed([false; 4], [159.0; 4]);
        let commands = commands_for(PitRequest::SetTyre { corner: Corner::LeftFront, armed: true }, &service);
        assert!(matches!(commands.as_slice(), [PitCommand::LF(None)]));
    }

    #[test]
    fn unticking_one_tyre_clears_all_four_and_re_arms_the_rest() {
        // iRacing has no per-corner clear, so dropping the left front means
        // clearing everything and putting the other three back.
        let service = armed([true, true, true, true], [158.0, 159.0, 160.0, 161.0]);
        let commands = commands_for(PitRequest::SetTyre { corner: Corner::LeftFront, armed: false }, &service);

        assert!(matches!(commands.first(), Some(PitCommand::ClearTires)));
        assert_eq!(commands.len(), 4, "clear plus the three survivors");
        // Each survivor keeps the pressure it already had, rather than
        // silently reverting to the garage setting.
        assert!(matches!(commands[1], PitCommand::RF(Some(159))));
        assert!(matches!(commands[2], PitCommand::LR(Some(160))));
        assert!(matches!(commands[3], PitCommand::RR(Some(161))));
    }

    #[test]
    fn unticking_a_tyre_does_not_re_arm_corners_that_were_already_off() {
        let service = armed([true, false, true, false], [159.0; 4]);
        let commands = commands_for(PitRequest::SetTyre { corner: Corner::LeftFront, armed: false }, &service);

        assert_eq!(commands.len(), 2, "clear plus the one other armed corner");
        assert!(matches!(commands[1], PitCommand::LR(Some(159))));
    }

    #[test]
    fn unticking_the_last_tyre_just_clears() {
        let service = armed([true, false, false, false], [159.0; 4]);
        let commands = commands_for(PitRequest::SetTyre { corner: Corner::LeftFront, armed: false }, &service);
        assert!(matches!(commands.as_slice(), [PitCommand::ClearTires]));
    }

    #[test]
    fn arming_all_four_is_one_bare_tick_each() {
        let service = armed([false; 4], [159.0; 4]);
        let commands = commands_for(PitRequest::SetAllTyres(true), &service);
        // `None`, not the pressure read back: a bare tick leaves each corner
        // at whatever the sim already has, which is what the tyre row does.
        assert!(matches!(
            commands.as_slice(),
            [PitCommand::LF(None), PitCommand::RF(None), PitCommand::LR(None), PitCommand::RR(None)]
        ));
    }

    #[test]
    fn clearing_all_four_is_iracings_own_single_command() {
        let service = armed([true; 4], [159.0; 4]);
        let commands = commands_for(PitRequest::SetAllTyres(false), &service);
        assert!(matches!(commands.as_slice(), [PitCommand::ClearTires]));
    }

    #[test]
    fn all_four_are_armed_only_when_none_is_missing() {
        assert!(armed([true; 4], [159.0; 4]).all_tyres_armed());
        assert!(!armed([true, true, false, true], [159.0; 4]).all_tyres_armed());
        assert!(!PitService::default().all_tyres_armed());
    }

    #[test]
    fn setting_a_pressure_also_arms_that_corner() {
        let service = armed([false; 4], [159.0; 4]);
        let commands = commands_for(PitRequest::SetTyrePressure { corner: Corner::RightRear, kpa: 165 }, &service);
        assert!(matches!(commands.as_slice(), [PitCommand::RR(Some(165))]));
    }

    #[test]
    fn fuel_and_the_toggles_are_one_command_each() {
        let service = PitService::default();
        assert!(matches!(commands_for(PitRequest::SetFuel(40), &service).as_slice(), [PitCommand::Fuel(Some(40))]));
        assert!(matches!(commands_for(PitRequest::ClearFuel, &service).as_slice(), [PitCommand::ClearFuel]));
        assert!(matches!(commands_for(PitRequest::SetTearoff(true), &service).as_slice(), [PitCommand::TearOff]));
        assert!(matches!(commands_for(PitRequest::SetTearoff(false), &service).as_slice(), [PitCommand::ClearWS]));
        assert!(matches!(commands_for(PitRequest::SetFastRepair(true), &service).as_slice(), [PitCommand::FastRepair]));
        assert!(matches!(commands_for(PitRequest::SetFastRepair(false), &service).as_slice(), [PitCommand::ClearFR]));
    }

    /// The whole sum end to end, as Auto Fuel runs it: a target worked out on
    /// track, then turned into litres against the tank in the pit lane.
    fn auto(laps: Option<i32>, per_lap: Option<f32>, level: f32, margin: f32) -> Option<i16> {
        let target = fuel_to_finish_litres(laps, None, per_lap, margin)?;
        Some(fuel_to_add_litres(target, level, Some(120.0)))
    }

    #[test]
    fn auto_fuel_covers_the_race_plus_the_margin() {
        // 20 laps left at 3 l/lap plus one lap in hand = 63 on board, of which
        // 20 are already aboard.
        assert_eq!(fuel_to_finish_litres(Some(20), None, Some(3.0), 1.0), Some(63.0));
        assert_eq!(auto(Some(20), Some(3.0), 20.0, 1.0), Some(43));
    }

    /// The Le Mans over-fuel: the target is latched at the pit entry, near
    /// the end of a lap, and the crossings count charges that nearly-done lap
    /// whole. Zero margin has to mean dry across the line, so the part of the
    /// lap already behind the car is not fuelled for.
    #[test]
    fn the_driven_part_of_the_current_lap_is_not_fuelled_for() {
        // 4 crossings left, latched 90% of the way round: 3.1 laps of road.
        assert_eq!(fuel_to_finish_litres(Some(4), Some(0.9), Some(3.0), 0.0), Some((4.0 - 0.9_f32) * 3.0));
        // The margin still rides on top, untouched.
        assert_eq!(fuel_to_finish_litres(Some(4), Some(0.9), Some(3.0), 1.0), Some(((4.0 - 0.9_f32) + 1.0) * 3.0));
        // A garbage fraction counts as zero driven, which errs long.
        assert_eq!(fuel_to_finish_litres(Some(4), Some(f32::NAN), Some(3.0), 0.0), Some(12.0));
        assert_eq!(fuel_to_finish_litres(Some(4), Some(1.7), Some(3.0), 0.0), Some(9.0), "clamped to one lap");
        assert_eq!(fuel_to_finish_litres(Some(0), Some(0.9), Some(3.0), 0.0), Some(0.0), "never negative");
    }

    #[test]
    fn auto_fuel_rounds_up_because_running_dry_is_worse_than_a_heavy_car() {
        assert_eq!(auto(Some(10), Some(2.55), 0.0, 0.0), Some(26), "25.5 rounds to 26");
    }

    #[test]
    fn auto_fuel_asks_for_nothing_when_the_tank_already_covers_it() {
        assert_eq!(auto(Some(5), Some(2.0), 60.0, 1.0), Some(0));
    }

    #[test]
    fn auto_fuel_never_asks_for_more_than_will_fit() {
        // 60 laps at 3 l/lap is 180 litres; the tank holds 120 and has 20 in.
        assert_eq!(auto(Some(60), Some(3.0), 20.0, 0.0), Some(100));
    }

    /// The tank estimate is inferred (`FuelLevel / FuelLevelPct`) and absent
    /// below a tenth of a tank, so it is never allowed to decide the load. With
    /// no capacity to go on, the whole shortfall is asked for and iRacing's own
    /// clip — which is authoritative — decides what actually fits.
    #[test]
    fn an_unknown_tank_size_never_shrinks_the_load() {
        assert_eq!(fuel_to_add_litres(180.0, 5.0, None), 175);
        assert_eq!(fuel_to_add_litres(180.0, 5.0, Some(f32::NAN)), 175, "an unusable estimate is no estimate");
        assert_eq!(fuel_to_add_litres(180.0, 5.0, Some(0.0)), 175);
    }

    /// The failure this split exists to make impossible. A target set on the
    /// last lap of a stint is measured against a tank that keeps draining all
    /// the way to the box; pairing it with the level from when it was set
    /// leaves the car short by every litre burnt in between.
    #[test]
    fn the_load_is_measured_against_the_tank_at_the_moment_it_is_armed() {
        let target = fuel_to_finish_litres(Some(40), None, Some(3.0), 0.5).expect("a measurable stint");
        // 8 litres left on the in-lap, 5 by the time the rig is connected.
        let on_the_in_lap = fuel_to_add_litres(target, 8.0, Some(120.0));
        let in_the_box = fuel_to_add_litres(target, 5.0, Some(120.0));
        assert_eq!(in_the_box - on_the_in_lap, 3, "the litres burnt getting there have to be made up");
    }

    #[test]
    fn auto_fuel_stays_silent_until_it_can_answer() {
        assert_eq!(fuel_to_finish_litres(None, None, Some(3.0), 1.0), None, "session length unknown");
        assert_eq!(fuel_to_finish_litres(Some(20), None, None, 1.0), None, "consumption not yet measured");
        assert_eq!(fuel_to_finish_litres(Some(20), None, Some(0.0), 1.0), None);
        assert_eq!(fuel_to_finish_litres(Some(20), None, Some(3.0), f32::NAN), None);
        assert_eq!(fuel_to_finish_litres(Some(20), None, Some(3.0), -1.0), None, "a negative margin is not a margin");
    }

    /// The gauge's fixed far edge: nothing latches while the tank only falls,
    /// the fill's end level latches the moment it rises, and the constant
    /// armed figure during delivery cannot push it along.
    #[test]
    fn a_running_fill_latches_the_level_it_will_stop_at() {
        let mut watch = RefuelWatch::default();
        assert_eq!(watch.update(false, 20.0, 0.0), None, "on track");
        assert_eq!(watch.update(true, 10.0, 0.0), None, "into the lane, nothing armed yet");
        assert_eq!(watch.update(true, 9.5, 50.0), None, "still burning down to the box");
        // The rig starts: the tank rises, and the edge pins to where it began
        // plus the load armed — not to the moving level plus that load.
        assert_eq!(watch.update(true, 10.0, 50.0), Some(59.5));
        assert_eq!(watch.update(true, 30.0, 50.0), Some(59.5), "delivery does not move it");
        assert_eq!(watch.update(true, 59.5, 50.0), Some(59.5), "nor does the fill completing");
        assert_eq!(watch.update(false, 59.5, 0.0), None, "back on track, next visit starts clean");
    }

    #[test]
    fn a_load_topped_up_mid_fill_moves_the_edge_out() {
        let mut watch = RefuelWatch::default();
        watch.update(true, 9.5, 40.0);
        assert_eq!(watch.update(true, 10.0, 40.0), Some(49.5));
        // The driver asks for more while it pumps: the stop now ends higher.
        assert_eq!(watch.update(true, 20.0, 60.0), Some(80.0));
        // iRacing unticking the box at the end reads as armed zero, which is a
        // decrease and leaves the finished edge where it is.
        assert_eq!(watch.update(true, 80.0, 0.0), Some(80.0));
    }

    #[test]
    fn a_stop_with_no_fuel_never_latches() {
        let mut watch = RefuelWatch::default();
        // A tyres-only stop: the level only ever falls in the lane.
        assert_eq!(watch.update(true, 12.0, 0.0), None);
        assert_eq!(watch.update(true, 11.8, 0.0), None);
        assert_eq!(watch.update(true, 11.7, 0.0), None);
    }

    /// Garbage in one input must not become a confident load: zero is what the
    /// pit command API reads as "leave this alone", which is the safe answer.
    #[test]
    fn a_nonsense_reading_asks_for_nothing_rather_than_something_wrong() {
        assert_eq!(fuel_to_add_litres(f32::NAN, 10.0, Some(120.0)), 0);
        assert_eq!(fuel_to_add_litres(100.0, f32::NAN, Some(120.0)), 0);
    }

    /// A garbage reading resolves to zero, which the pit command API reads as
    /// "leave this as it is" — the safe answer. Clamping to `i16::MAX`
    /// instead would send the sim a nonsense maximum.
    #[test]
    fn rounding_survives_garbage_readings() {
        assert_eq!(round_litres(54.6), 55);
        assert_eq!(round_litres(f32::NAN), 0);
        assert_eq!(round_litres(-3.0), 0);
        assert_eq!(round_kpa(158.579), 159);
        assert_eq!(round_kpa(f32::INFINITY), 0);
        assert_eq!(round_kpa(f32::NAN), 0);
    }
}
