// Rust guideline compliant 2026-02-16

//! Owned telemetry data that crosses from the background telemetry thread
//! to the UI thread over a channel. Deliberately decoupled from
//! `iracing_telem`'s borrowed [`iracing_telem::Value`], which only lives for
//! the duration of one telemetry tick.

use std::sync::Arc;

use iracing_telem::flags::TrackLocation;
use serde::{Deserialize, Serialize};

// Every per-car string below is an `Arc<str>` rather than a `String`, and
// deliberately so. All of them — a driver's name, their car, their license and
// class colors — are fixed for the whole session: they come from the
// session-info YAML, which `SessionInfoCache` already re-parses only when
// iRacing bumps its version counter.
//
// A snapshot, however, is rebuilt from scratch on every one of iRacing's 60
// ticks a second, and each car appears in two of these lists. As `String`s
// that was four heap allocations per car per list per tick — about thirty
// thousand allocations a second in a full field, every one of them copying
// bytes that had not changed since the session loaded. As `Arc<str>` the same
// clone is an atomic increment, the bytes are allocated once per YAML refresh,
// and `TelemetrySnapshot`'s own `Clone` gets cheap along with it.
//
// Nothing downstream had to change: `&Arc<str>` coerces to `&str` wherever one
// is wanted, and `Display` forwards.

/// One car's row in the relative widget — either an opponent or the car the
/// view is centred on (marked via `is_focus`), inserted at its natural
/// position between the closest cars ahead and closest cars behind.
#[derive(Debug, Clone)]
pub struct CarSnapshot {
    /// The car's full display name, e.g. `"McLaren 720S GT3 EVO"`. Its first
    /// word is the manufacturer, which is what the Relative's brand mark is
    /// resolved from.
    pub car_screen_name: Arc<str>,
    /// iRacing's `CarIdx` for this car — the one identifier that stays put
    /// while position, gap and name all move. Row ordering keys off it to hold
    /// a row still between ticks; see `telemetry::relative::RowOrder`.
    pub car_idx: i32,
    /// The driver's iRacing customer id — the one identifier stable *across*
    /// sessions, which is what the danger-driver marks are keyed by (see
    /// `plans/danger-drivers.md`). `None` where the YAML omits it.
    pub cust_id: Option<u32>,
    /// Position **within this car's own class** — the same number the
    /// Standings widget shows, and in a single-class race the overall
    /// position too. A multi-class field has every class racing its own race,
    /// so an overall number here reads as a driver being fourteenth in a race
    /// they are in fact leading.
    pub position: i32,
    pub track_location: TrackLocation,
    /// Positive means this car is ahead of the focus car in time. Zero for the
    /// focus car's own row. Still named for the player because that is what
    /// the focus car is in every session anyone drives; see `is_focus`.
    pub gap_to_player_secs: f32,
    pub driver_name: Arc<str>,
    /// The number on the car, as printed on it — `"007"` stays `"007"`.
    /// Empty where the session doesn't say.
    pub car_number: Arc<str>,
    pub irating: i32,
    /// The driver's profile flag, as iRacing's `FlairID`; `ui::flags` maps it
    /// to a country. Zero means no flag.
    pub flair_id: i32,
    /// The license class's color, hex string like `car_class_color`. Borders
    /// the Relative widget's iRating badge, which is where license class is
    /// shown now that safety rating no longer gets a chip of its own.
    pub license_color: Arc<str>,
    /// Hex color string from the session YAML (e.g. `"0xFF3333"`); parsed to
    /// a UI color at render time, not here. Drives the small class-indicator
    /// mark next to each row.
    pub car_class_color: Arc<str>,
    /// Whether this car holds the fastest lap of the whole session.
    pub is_fastest_overall: bool,
    /// A rough estimate of this driver's iRating change if the session
    /// ended right now — see `telemetry::irating` for the caveats. Rated
    /// against this car's own class, which is the pool iRacing rates it in.
    /// `None` until there's enough field data (standings) to compute it, and
    /// for a class with nobody else in it to be rated against.
    pub irating_change_estimate: Option<f32>,
    /// Whether this row is the car every gap here is measured from.
    ///
    /// The player's own car in every session they drive. While spectating it
    /// is the car the camera is watching instead, which is what centres this
    /// widget on the race being watched rather than on an empty grid slot —
    /// see `resolve_focus_car` in `telemetry::session`.
    pub is_focus: bool,
    /// How many times this car has gone off the track so far — see
    /// `telemetry::session::OffTrackCounter`.
    pub off_tracks: i32,
    /// This car's lap count minus the focus car's. Positive means this car is
    /// a lap up on the focus car (about to lap it); negative means the focus
    /// car has lapped this one. Drives the name/gap red-or-blue lap-status
    /// coloring; zero (same lap) gets no special color.
    pub lap_diff: i32,
    /// The fastest of this car's last few completed laps — a rolling
    /// "recent pace" figure, since a session best can be a single fluke lap
    /// from long ago. `None` until this car has completed a lap since it was
    /// first seen this session.
    pub best_recent_lap_secs: Option<f32>,
    /// Last three completed laps, newest first.
    pub recent_laps: [Option<f32>; 3],
    /// A black flag held against this car, if any — see [`Penalty`].
    pub penalty: Option<Penalty>,
}

/// A black flag against one car, from its `CarIdxSessionFlags` bits.
///
/// Ordered by severity, so that a car carrying more than one bit — a black
/// flag is usually raised with the pit-serviceable bit alongside it — can be
/// reduced to the one worth a marker. See `telemetry::relative::penalty_from_flags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Penalty {
    /// The furled black flag: a slowdown warning for exceeding track limits
    /// or gaining time off track, cleared by lifting.
    Slowdown,
    /// The meatball: the car must pit for repairs.
    Repair,
    /// A black flag with a penalty to serve in the pit lane — a stop-and-go
    /// or a drive-through — or a slowdown the driver has not yet taken.
    BlackFlag,
    /// Disqualified from the session.
    Disqualified,
}

/// The compound one car is running, for the Standings tyre column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TyreCompound {
    /// The compound's initial — `D`/`W` in a GT3 field, `H`/`S`/… where a
    /// series names more than one dry. From the session YAML's compound
    /// names where they exist, the dry/wet convention otherwise; see
    /// `telemetry::session::tyre_compound_of`.
    pub letter: char,
    /// Whether this is a wet, which is what earns the circle its blue ring.
    pub wet: bool,
}

/// One driver's row in the Standings widget, from the session's official
/// `ResultsPositions` classification plus a few live telemetry values.
#[derive(Debug, Clone)]
pub struct StandingsEntry {
    pub position: i32,
    /// Position within this car's own class — the number actually shown,
    /// per class-relative Standings (see `car_class_id`).
    pub class_position: i32,
    pub car_idx: i32,
    pub driver_name: Arc<str>,
    pub irating: i32,
    /// The driver's profile flag, as iRacing's `FlairID`; `ui::flags` maps it
    /// to a country. Zero means no flag.
    pub flair_id: i32,
    /// Groups entries into class sections; matched against other entries'
    /// `car_class_id`, never assumed to correlate with car/driver order.
    pub car_class_id: i32,
    pub car_class_short_name: Arc<str>,
    /// Hex color string from the session YAML (e.g. `"0xFF3333"`); parsed to
    /// a UI color at render time, not here.
    pub car_class_color: Arc<str>,
    pub best_lap_secs: f32,
    pub last_lap_secs: f32,
    /// Gap to the session leader, in seconds.
    pub gap_to_leader_secs: f32,
    pub pit_stops: i32,
    /// Laps completed since this car last left the pits.
    pub current_stint_laps: i32,
    /// Seconds since this car last left the pits.
    pub current_stint_secs: f64,
    /// Typical completed-stint length in laps this session — the median of
    /// its recent fuel stints, with damage and penalty stops excluded — or
    /// `None` if the car hasn't completed a plausible fuel stint yet.
    pub avg_stint_laps: Option<i32>,
    /// Typical completed-stint length in seconds this session, measured the
    /// same way as `avg_stint_laps`.
    pub avg_stint_secs: Option<f64>,
    pub track_location: TrackLocation,
    /// A black flag held against this car, if any — see [`Penalty`].
    pub penalty: Option<Penalty>,
    /// Whether this row holds the fastest lap of its own class. Drives the
    /// Standings widget's highlighted best-lap cell, which is per class
    /// because that's the classification the rest of the widget is built
    /// around — in a multi-class race, a session-wide fastest lap says
    /// nothing about most of the field's races.
    pub is_class_fastest: bool,
    /// How many laps behind this class's leader this car is, or zero when on
    /// the lead lap. Shown in place of a time gap, which is meaningless once
    /// a car has been lapped.
    pub laps_down: i32,
    /// The car's full display name, e.g. `"McLaren 720S GT3 EVO"`, used to
    /// pick a manufacturer mark (see `ui::logos`).
    pub car_screen_name: Arc<str>,
    /// How long this car was stationary at its last completed stop, or
    /// `None` if it hasn't pitted yet this session.
    pub last_pit_secs: Option<f64>,
    /// Its typical stationary time across recent completed stops — a median,
    /// so one penalty served in the box or a long repair doesn't inflate it.
    pub avg_pit_secs: Option<f64>,
    /// How many more stops this car needs to reach the end of the session —
    /// see `telemetry::endurance`. `None` before it has completed a stint.
    pub stops_remaining: Option<i32>,
    /// Where this car is projected to finish in its class once every
    /// remaining stop has been taken. `None` when the projection has no
    /// basis yet.
    pub projected_class_position: Option<i32>,
    /// Whether this row is the car the widget builds its window around — the
    /// player's own, or the one being watched while spectating. See
    /// [`CarSnapshot::is_focus`].
    pub is_focus: bool,
    /// How many times this car has gone off the track so far — see
    /// `telemetry::session::OffTrackCounter`.
    pub off_tracks: i32,
    /// How long this car has been mid-tow: gone from the world after being
    /// seen in it. iRacing publishes a tow clock for the player alone, so
    /// this is measured by the overlay from the moment the car vanished —
    /// time gone, not time left. `None` when the car isn't towing; see
    /// `telemetry::session::TowTracker`.
    pub tow_secs: Option<f64>,
    /// The compound this car is running, where the sim says
    /// (`CarIdxTireCompound`, with the player's own `PlayerTireCompound`
    /// standing in for their row). `None` leaves the tyre column's circle
    /// off this row.
    pub tyre: Option<TyreCompound>,
    /// Class places gained (positive) or lost since this car's race began —
    /// measured from the first position it was seen holding, its grid slot
    /// when it was there before the green. `None` outside races, and for a
    /// car not yet scored; see `telemetry::session::PositionChangeTracker`.
    pub race_position_change: Option<i32>,
}

/// One car class's summary, for the Standings widget's section headers.
///
/// Built by grouping [`StandingsEntry`] values by `car_class_id`, so a
/// single-class session yields exactly one of these.
#[derive(Debug, Clone)]
pub struct ClassSection {
    pub car_class_id: i32,
    pub short_name: Arc<str>,
    /// Hex color string from the session YAML, parsed at render time.
    pub color: Arc<str>,
    /// How many cars are in this class.
    pub car_count: i32,
    /// This class's own field-strength estimate — see `telemetry::sof`.
    /// `None` until the class's iRatings are known.
    pub sof: Option<i32>,
}

/// One car on the radar, as the widget needs to draw it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarCar {
    /// Signed separation in metres, positive ahead, measured centre to centre.
    ///
    /// The widget's whole axis is this number: a car length of it is a car
    /// length of bar, so overlap is drawn rather than described.
    pub separation_m: f32,
    /// The same separation in milliseconds of relative time, for the optional
    /// numeric readout and for continuity across ticks.
    pub gap_ms: f32,
    /// Metres per second the gap is closing; negative while it opens. `None`
    /// until enough history exists, which draws as "steady".
    pub closing_mps: Option<f32>,
}

/// The closest car ahead of and behind the player on one side, if any.
///
/// Both `None` means no car is currently detected on that side (iRacing's
/// `CarLeftRight` reads clear).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RadarSide {
    pub ahead: Option<RadarCar>,
    pub behind: Option<RadarCar>,
}

/// Radar Bars data: the nearest cars on each side of the player.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RadarSnapshot {
    pub left: RadarSide,
    pub right: RadarSide,
}

/// A car from a quicker class near the focus car, for the Faster Class widget.
///
/// Built by the telemetry thread for every such car inside
/// `super::faster_class::in_scan`; which of them is worth a warning, and how
/// loud, is decided on the UI thread against the driver's own thresholds —
/// see `super::faster_class::Alarm`.
#[derive(Debug, Clone)]
pub struct Approaching {
    pub car_idx: i32,
    /// Seconds of relative time behind the focus car; negative once the car
    /// has gone by. The Relative's gap with the sign flipped, so the figure a
    /// driver is warned about is a positive one.
    pub behind_secs: f32,
    /// Seconds of gap this car takes back per second; negative while it drops
    /// back. `None` until enough has been seen to say — see
    /// `super::faster_class::ClosingRates`.
    pub closing_rate: Option<f32>,
    pub driver_name: Arc<str>,
    /// The number on the car; empty where the session doesn't say.
    pub car_number: Arc<str>,
    pub car_class_short_name: Arc<str>,
    /// Hex color string from the session YAML, parsed at render time.
    pub car_class_color: Arc<str>,
}

/// Faster Class data: every quicker-class car within scan, nearest first.
///
/// Empty in a single-class session, and in every session until a quicker car
/// is actually within range.
#[derive(Debug, Clone, Default)]
pub struct FasterClassSnapshot {
    pub approaching: Vec<Approaching>,
}

/// The sim's estimate of overall track wetness — `TrackWetness`, an enum in
/// the shared memory. The raw variable also carries an "unknown" zero, which
/// maps to `None` at the reading site like the variable's absence does: both
/// mean "nothing honest to say".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackWetness {
    Dry,
    MostlyDry,
    VeryLightlyWet,
    LightlyWet,
    ModeratelyWet,
    VeryWet,
    ExtremelyWet,
}

impl TrackWetness {
    /// The wetness the raw telemetry value describes, or `None` where it
    /// describes nothing (unknown, or a value this build doesn't know).
    #[must_use]
    pub fn from_raw(raw: i32) -> Option<Self> {
        match raw {
            1 => Some(Self::Dry),
            2 => Some(Self::MostlyDry),
            3 => Some(Self::VeryLightlyWet),
            4 => Some(Self::LightlyWet),
            5 => Some(Self::ModeratelyWet),
            6 => Some(Self::VeryWet),
            7 => Some(Self::ExtremelyWet),
            _ => None,
        }
    }

    /// Whether there is standing water worth naming — anything above the two
    /// dry readings. `MostlyDry` counts as dry: it is what a drying line
    /// reports for a long time after the rain has gone.
    #[must_use]
    pub fn is_wet(self) -> bool {
        !matches!(self, Self::Dry | Self::MostlyDry)
    }

    /// The wetness in the sim's own words, lowercase so it can sit mid-phrase
    /// ("track lightly wet").
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dry => "dry",
            Self::MostlyDry => "mostly dry",
            Self::VeryLightlyWet => "very lightly wet",
            Self::LightlyWet => "lightly wet",
            Self::ModeratelyWet => "moderately wet",
            Self::VeryWet => "very wet",
            Self::ExtremelyWet => "extremely wet",
        }
    }
}

/// Live weather data, plus the angles the widget's compass needs.
#[derive(Debug, Clone, Copy, Default)]
pub struct WeatherSnapshot {
    pub track_temp_c: f32,
    pub air_temp_c: f32,
    pub wind_speed_mps: f32,
    /// Fog density as a fraction from 0.0 to 1.0. `None` if the session
    /// doesn't expose `FogLevel`.
    pub fog: Option<f32>,
    /// The session's declared chance of rain, 0.0 to 1.0. `None` until the
    /// session-info YAML has been read, or where it declares none — the
    /// Realistic-weather sessions that actually rain publish no
    /// `ChanceOfRain` at all.
    pub precip_chance: Option<f32>,
    /// Rain falling *now* at the start/finish line, 0.0 to 1.0. `None` where
    /// the session doesn't expose `Precipitation`; `Some(0.0)` is a real
    /// "not raining".
    pub precip_now: Option<f32>,
    /// The steward's declaration that wet tyres may be used
    /// (`WeatherDeclaredWet`). `false` where the variable is absent.
    pub declared_wet: bool,
    /// Overall track wetness — see [`TrackWetness`]. `None` where
    /// unpublished or unknown.
    pub track_wetness: Option<TrackWetness>,
    /// Whether the player's own car is on a wet compound, resolved from
    /// `PlayerTireCompound` against the YAML's compound names — see
    /// `session::on_wet_tyres`. `false` whenever that cannot be said
    /// honestly, so a surface showing it can never wrongly claim wets.
    pub on_wet_tyres: bool,
    /// Wind direction in radians, relative to the car's nose (0 = wind
    /// blowing toward the front of the car, increasing clockwise) —
    /// already adjusted for the car's heading, not a raw world-frame angle.
    /// `None` until both the wind-direction and car-heading vars are available.
    pub wind_dir_relative_to_car_rad: Option<f32>,
}

/// Header/footer data for the Relative widget: field strength, incidents,
/// and race clock/lap progress. Grouped separately from `CarSnapshot` since
/// it describes the session as a whole, not any one row.
///
/// Also feeds the Standings widget's header, which shows the same session
/// clock and field size.
#[derive(Debug, Clone, Default)]
pub struct RelativeMeta {
    /// An unofficial field-strength estimate — see `telemetry::sof`. `None`
    /// until the field's iRatings are known.
    pub sof: Option<i32>,
    /// What kind of session this is — see [`SessionKind`].
    pub session_kind: SessionKind,
    /// How many cars are in the session, across all classes.
    pub car_count: i32,
    /// Who is being watched, when the overlay is following a camera car
    /// instead of the player's own — `None` in every session the player
    /// drives. See [`CarSnapshot::is_focus`].
    ///
    /// A name rather than a bare flag, because a name is what the header has
    /// to show. A Relative quietly centred on somebody else is the one way
    /// spectator focus could actively mislead, and the only fix is to say
    /// whose race it is.
    pub spectating: Option<Arc<str>>,
    /// Who is driving the player's own car when it is not the player — a
    /// team-mate's stint in a team session. `None` whenever the player is in
    /// the car, and in every session that is not a team event.
    ///
    /// The team-race counterpart of [`RelativeMeta::spectating`]: both put a
    /// name in the header, but this one names the car's driver rather than
    /// whose race is being watched. Focus does not move for it and the pages
    /// about the race stay; see [`Seat`].
    pub team_mate: Option<Arc<str>>,
    /// Whether the racing has actually started.
    ///
    /// A race's clock is the racing, not the queueing: `SessionState` reads
    /// `ParadeLaps` behind the pace car and `GetInCar` on the grid, and a
    /// countdown that has been running through either is a countdown that
    /// lies about how much race is left. Always true where the sim doesn't
    /// publish the state, and in practice and qualifying, where there is no
    /// green flag to wait for and the clock simply runs.
    pub racing_under_way: bool,
    /// The session's scheduled length, for the clock to hold at before the
    /// green. `None` for a session with no fixed length.
    pub session_length_secs: Option<f64>,
    /// Whether a full-course caution is out.
    ///
    /// Only the course-wide bits of `SessionFlags`, not a local yellow: a
    /// yellow in one sector slows one corner, while a caution rewrites every
    /// gap in the race at once. The pit window refuses to project through one
    /// — see `telemetry::pit_window::Confidence::Void`.
    pub under_caution: bool,
    /// The player's own incident count this session (`PlayerCarMyIncidentCount`).
    pub incidents: i32,
    /// The session's incident limit, or `None` if unlimited/not yet known.
    pub incident_limit: Option<i32>,
    pub race_elapsed_secs: f64,
    /// `None` if the session has no fixed time length (e.g. lap-limited).
    pub race_remain_secs: Option<f64>,
    pub current_lap: i32,
    /// Estimated number of laps the race will run, from the remaining time and
    /// the player's recent pace — an approximation, not an official iRacing
    /// figure. `None` until both the remaining-time var and a lap time are
    /// available. See `telemetry::endurance::projected_total_laps`.
    pub predicted_total_laps: Option<i32>,
    /// The session's scheduled lap count, where it has one. `None` for a
    /// timed session, where the lap count is the estimate above instead.
    pub session_laps: Option<i32>,
    /// The state of the grid before a race starts — see [`GridStatus`].
    /// `None` once the field is rolling, and in every other kind of session.
    pub grid: Option<GridStatus>,
}

/// How far a race's grid has got, while the field is still forming up.
///
/// Shown in place of the held race clock: the clock has nothing to say until
/// the green, and what a driver on the grid wants to know is how long they
/// have and whether everyone is out yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridStatus {
    /// How many competitors have gridded — are in the world at all, rather
    /// than still in the garage or at the entry screen.
    pub cars_gridded: i32,
    /// How many competitors the session has.
    pub car_count: i32,
    /// Seconds until the grid closes and the field rolls, where the sim
    /// publishes it. `None` for a lap-limited race, where iRacing gives the
    /// grid no clock the SDK can see.
    pub countdown_secs: Option<f64>,
}

impl RelativeMeta {
    /// What the session clock should read, counting down.
    ///
    /// Time left, once there is a race to have time left in. Before the green
    /// it holds at the session's full length instead of counting the grid and
    /// the pace laps away — those are not the race, and a driver who glances
    /// down on the formation lap should not find minutes already gone.
    ///
    /// `None` for a session with no fixed length, where there is nothing to
    /// count down to.
    #[must_use]
    pub fn countdown_secs(&self) -> Option<f64> {
        if self.session_kind.is_race() && !self.racing_under_way {
            return self.session_length_secs;
        }
        self.race_remain_secs
    }

    /// Whether the clock is holding rather than running — see
    /// [`RelativeMeta::countdown_secs`].
    #[must_use]
    pub fn clock_is_holding(&self) -> bool {
        self.session_kind.is_race() && !self.racing_under_way
    }
}

/// Session-level endurance figures for the Standings widget's summary band.
///
/// Every field is a projection from observed stint and pit-stall history —
/// iRacing publishes no fuel or tyre data for other cars — so the UI labels
/// them as estimates. See `telemetry::endurance`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnduranceMeta {
    /// Laps left in the session: the sim's own count in a lap-limited
    /// session, and otherwise a projection from the clock at the player's
    /// current pace. `None` in a session with neither a lap count nor a time
    /// limit, and before a lap time is known.
    pub laps_remaining: Option<i32>,
    /// How much of the lap under way is already behind the player, 0 to 1.
    ///
    /// `laps_remaining` counts that lap whole — it is a count of line
    /// crossings — so anything charging fuel per lap must subtract this or
    /// pay for road already driven; see `pit::fuel_to_finish_litres`.
    pub lap_driven_pct: Option<f32>,
    /// Stops the player still owes.
    pub stops_remaining: Option<i32>,
    /// The player's projected class position once the field has completed
    /// its remaining stops.
    pub projected_class_position: Option<i32>,
    /// Whether this session has been seen to need more than one stop.
    ///
    /// Latched: once true it stays true for the rest of the session. A
    /// three-stop race drops to one stop remaining near the end, and a view
    /// that switched itself off at that point would take the strategy
    /// columns away exactly when the last stop is being planned.
    pub multi_stop_race: bool,
    /// The fewest stops anyone in the player's class is projected to make.
    /// When this is below the player's own count, a rival is on a longer
    /// strategy — the single most decisive fact in an endurance race.
    pub best_stops_in_class: Option<i32>,
}

/// An estimate of where the player would rank if they pitted right now.
///
/// This is a rough projection from current gaps plus a configurable assumed
/// pit-lane time loss — not an iRacing-provided figure — and is presented to
/// the user as an estimate, not a guarantee.
#[derive(Debug, Clone, Copy)]
pub struct PitProjection {
    #[expect(
        dead_code,
        reason = "the redesigned Standings widget has no row for a pit projection; the estimate is still computed and tested (see telemetry::standings::project_pit_position) so re-adding it is a UI change only"
    )]
    pub estimated_position: i32,
}

/// What kind of session this is.
///
/// iRacing writes the session type as free text (`"Race"`, `"Open Qualify"`,
/// `"Lone Qualify"`, `"Offline Testing"`, …), and several things read very
/// differently depending on which it is: a lap deficit is meaningless in
/// qualifying, and a pit strategy is meaningless outside a race. Classifying
/// the string once keeps every widget agreeing about which session it is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionKind {
    Practice,
    Qualifying,
    Race,
    Warmup,
    /// Anything unrecognized — treated as practice everywhere it matters,
    /// since that is the least presumptuous reading.
    #[default]
    Unknown,
}

/// Whether iRacing's session-type text names a *lone* qualifying session.
///
/// [`SessionKind`] deliberately folds `"Open Qualify"` and `"Lone Qualify"`
/// into one kind, because everything it drives — the badge, the clock label —
/// treats them alike. The Relative does not: in lone qualifying the player is
/// the only car on track and the rest of the field is parked, so there is no
/// running order to show. Hence the finer distinction here rather than a
/// second enum variant that every `match` would have to repeat.
#[must_use]
pub fn is_lone_qualifying(session_type: &str) -> bool {
    let name = session_type.to_ascii_lowercase();
    name.contains("qual") && name.contains("lone")
}

impl SessionKind {
    /// Classifies iRacing's own session-type text.
    ///
    /// Matched on substrings rather than equality: the sim writes
    /// `"Open Qualify"` and `"Lone Qualify"` for what is one kind of session
    /// here, and `"Offline Testing"` for practice.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let name = name.to_ascii_lowercase();
        if name.contains("race") {
            Self::Race
        } else if name.contains("qual") {
            Self::Qualifying
        } else if name.contains("warm") {
            Self::Warmup
        } else if name.contains("practice") || name.contains("test") {
            Self::Practice
        } else {
            Self::Unknown
        }
    }

    /// The heading this session gets, e.g. the Relative footer's clock label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Practice => "PRACTICE",
            Self::Qualifying => "QUALIFY",
            Self::Race => "RACE",
            Self::Warmup => "WARMUP",
            Self::Unknown => "SESSION",
        }
    }

    /// The single letter in the Standings badge.
    #[must_use]
    pub fn letter(self) -> &'static str {
        match self {
            Self::Practice => "P",
            Self::Qualifying => "Q",
            Self::Race => "R",
            Self::Warmup => "W",
            Self::Unknown => "?",
        }
    }

    /// Whether cars are racing each other, rather than the clock.
    ///
    /// Gates everything that only makes sense in a race: lap deficits, pit
    /// strategy, the endurance projections.
    #[must_use]
    pub fn is_race(self) -> bool {
        self == Self::Race
    }
}

#[cfg(test)]
mod session_kind_tests {
    use super::SessionKind;

    /// iRacing writes several names for the same kind of session.
    #[test]
    fn the_sims_own_names_all_classify() {
        for name in ["Race", "RACE", "Heat Race"] {
            assert_eq!(SessionKind::from_name(name), SessionKind::Race, "{name}");
        }
        for name in ["Qualify", "Open Qualify", "Lone Qualify"] {
            assert_eq!(SessionKind::from_name(name), SessionKind::Qualifying, "{name}");
        }
        for name in ["Practice", "Offline Testing"] {
            assert_eq!(SessionKind::from_name(name), SessionKind::Practice, "{name}");
        }
        assert_eq!(SessionKind::from_name("Warmup"), SessionKind::Warmup);
        assert_eq!(SessionKind::from_name(""), SessionKind::Unknown);
    }

    #[test]
    fn only_a_race_counts_as_racing() {
        assert!(SessionKind::Race.is_race());
        for kind in [SessionKind::Practice, SessionKind::Qualifying, SessionKind::Warmup, SessionKind::Unknown] {
            assert!(!kind.is_race(), "{kind:?}");
        }
    }

    #[test]
    fn every_kind_has_a_heading_and_a_letter() {
        for kind in [
            SessionKind::Practice,
            SessionKind::Qualifying,
            SessionKind::Race,
            SessionKind::Warmup,
            SessionKind::Unknown,
        ] {
            assert!(!kind.label().is_empty());
            assert_eq!(kind.letter().chars().count(), 1);
        }
    }
}

/// One tyre's condition as it came off the car at the last pit stop.
///
/// Every reading here is a last-stop snapshot, which is exactly what iRacing's
/// own Tire Info box shows and what its "data collected at last pit stop"
/// caption means. The sim does that latching itself: `LFwear*` and the carcass
/// temperatures `LFtempC*` change only when a stop refreshes them, and hold
/// unchanged for the whole stint in between.
///
/// The live families are published too — surface temperatures `LFtempL/M/R`
/// and `LFpressure`, both of which move every tick — but they belong to a live
/// page rather than to this one.
///
/// `[left, middle, right]` throughout, as the sim orders them.
///
/// `Serialize`/`Deserialize` so a team-mate's tyre life can cross the sync
/// wire to a spectator's Tyres page — see `plans/team-sync.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TyreState {
    pub temps_c: [f32; 3],
    /// Tread remaining, 0.0-1.0.
    pub wear: [f32; 3],
    /// Hot pressure as the tyre came off, kPa.
    ///
    /// Alone among these, pressure has no last-stop variable to read: the sim
    /// publishes only the live `LFpressure`. It is therefore sampled here at
    /// the moment the wear readings refresh — see
    /// `telemetry::session::StopPressures`.
    pub pressure_kpa: f32,
}

/// All four tyres, indexed by [`super::pit::corner_index`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TyreInfo {
    pub corners: [TyreState; 4],
}

impl TyreInfo {
    /// Whether any corner has a reading — i.e. a stop has latched values.
    ///
    /// The sim publishes all zeros until the first stop, so this is how a
    /// spectator's page tells "no data yet" from real tyre life.
    #[must_use]
    pub fn any_collected(&self) -> bool {
        self.corners.iter().any(|corner| corner.temps_c.iter().any(|t| *t > 0.0))
    }

    /// The worst corner's remaining tread across every measured edge, as a
    /// percentage (100 = fresh), or `None` before a stop has latched wear.
    ///
    /// Zero readings are the sim's "not measured yet" sentinel, not a tyre
    /// worn to the cords, so they are excluded rather than reported as the
    /// minimum. This is the number the standing tyre policy's wear threshold
    /// compares against.
    #[must_use]
    pub fn lowest_remaining_pct(&self) -> Option<f32> {
        let lowest =
            self.corners.iter().flat_map(|corner| corner.wear).filter(|wear| *wear > 0.0).fold(f32::INFINITY, f32::min);
        lowest.is_finite().then_some(lowest * 100.0)
    }
}

/// The driver-adjustable controls this car exposes.
///
/// Every field is `None` when the car doesn't publish that variable, which is
/// how the In-Car Adjustments page decides whether to draw a row. These are
/// read-only: iRacing accepts no broadcast message for any of them, so the
/// page reports rather than sets.
#[derive(Debug, Clone, Copy, Default)]
pub struct CarAdjustments {
    pub brake_bias: Option<f32>,
    pub abs: Option<f32>,
    pub traction_control: Option<f32>,
    pub throttle_shape: Option<f32>,
    pub dash_page: Option<f32>,
}

/// How far into this lap's fuel allowance the car is, for the Strategy page's
/// live save row.
///
/// Both fields are `None` until there is an honest answer: the burn needs a
/// start/finish crossing to measure from, and the fraction needs the lap curve
/// to have measured a lap. The row is absent rather than approximate, because a
/// fuel figure a driver cannot check is one they will act on anyway.
#[derive(Debug, Clone, Copy, Default)]
pub struct FuelUse {
    /// Litres burnt since the last start/finish crossing.
    pub used_this_lap_litres: Option<f32>,
    /// What fraction of a lap's *time* has passed, from
    /// [`super::relative::LapCurve::lap_fraction`].
    pub lap_fraction: Option<f32>,
}

/// Where the player is relative to the car the widgets are about.
///
/// Every player scalar iRacing publishes — `FuelLevel`, tyre temperatures,
/// `dcBrakeBias`, `LapLastLapTime` — describes the car whose physics the
/// player's sim is running. Which car that is, and whether there is one at
/// all, is this. Resolved once per tick by `session::resolve_seat` from
/// `IsOnTrack`, the session YAML and the focus car; see
/// `plans/team-endurance.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Seat {
    /// In the car with the physics running (`IsOnTrack`). Every player scalar
    /// is live. The ordinary case, and the demo's.
    #[default]
    Driving,
    /// The player's own car with a team-mate in it. Focus stays on the car;
    /// the pages about the seat step aside; the name is who is driving.
    TeamMate(Arc<str>),
    /// The player's own car with nobody else in it: the garage, a tow, or the
    /// wait between stints. In a solo session the scalars are still the
    /// player's own, so no page is withdrawn for it.
    OutOfCar,
    /// Somebody else's car, from the camera — see
    /// [`RelativeMeta::spectating`].
    Spectating(Arc<str>),
}

/// One telemetry tick's worth of data, ready for the UI to render.
#[derive(Debug, Clone)]
pub struct TelemetrySnapshot {
    /// The whole field in display order: farthest ahead first, down through
    /// the focus car's own row, to farthest behind.
    ///
    /// Not pre-sliced to what fits on screen, because the Relative page
    /// scrolls: the rows outside the visible window are what scrolling
    /// reveals. The widget slices this itself with
    /// [`super::relative::window`].
    pub relative: Vec<CarSnapshot>,
    /// Where the focus car sits in `relative`, which the visible window is
    /// centred on. See [`CarSnapshot::is_focus`].
    pub focus_index: usize,
    pub relative_meta: RelativeMeta,
    /// Full-field classification, sorted by position. Empty if the session
    /// doesn't expose `ResultsPositions` yet (e.g. just loaded in).
    pub standings: Vec<StandingsEntry>,
    /// One entry per car class present in `standings`, ordered by the
    /// class's leading position, so the Standings widget renders sections in
    /// the order the classes are actually running.
    pub class_sections: Vec<ClassSection>,
    /// Which class the focus car is entered in, straight from the session's
    /// driver list.
    ///
    /// Known from the moment the session loads, and in particular known
    /// without that car having been scored — which is what makes it worth
    /// carrying separately from its `standings` row. Standings opens the
    /// focus car's own class out into a window around it, and finding that
    /// class by looking for its row meant a driver who had not yet set a
    /// lap had no class at all, so every class collapsed to its leaders.
    pub focus_car_class_id: Option<i32>,
    pub radar: RadarSnapshot,
    /// Quicker-class cars coming up behind; see [`FasterClassSnapshot`].
    pub faster_class: FasterClassSnapshot,
    pub weather: WeatherSnapshot,
    /// Session-wide endurance projections; see [`EnduranceMeta`].
    pub endurance: EnduranceMeta,
    /// `None` until there's enough data (gap-to-leader for the player and
    /// the rest of the field) to estimate a post-pit position.
    #[expect(dead_code, reason = "see PitProjection::estimated_position")]
    pub pit_projection: Option<PitProjection>,
    /// What iRacing has armed for the next stop. Read back every tick and
    /// rendered as-is — see [`super::pit`].
    pub pit_service: super::pit::PitService,
    pub tyres: TyreInfo,
    pub adjustments: CarAdjustments,
    /// This lap's fuel burn against the clock; see [`FuelUse`].
    pub fuel_use: FuelUse,
    /// What a pit stop costs on this track, in this car — measured where it can
    /// be, defaults elsewhere. See [`super::pit_model::PitModel`].
    pub pit_model: super::pit_model::PitModel,
    /// `IsInGarage`: the car is in the garage with the setup screen up.
    ///
    /// Describes the sim, not the overlay. What it means for drawing is
    /// `app`'s decision, and a session that doesn't publish the variable
    /// reports `false` — the state the overlay already assumed.
    pub in_garage: bool,
    /// Where the player is relative to the focus car; see [`Seat`].
    pub seat: Seat,
    /// Who and which session this is, for team sync; see [`SessionIdentity`].
    pub identity: SessionIdentity,
    /// iRacing `SessionTime` for this tick, in seconds — the clock every
    /// member's sim shares, which team-sync events are stamped and ordered
    /// by. `0.0` before the session publishes it.
    pub session_time_secs: f64,
    /// The session-wide flag flying now, for the black box's status border;
    /// see [`CourseFlag`].
    pub course_flag: CourseFlag,
    /// Whether the fuel margin makes this the lap to pit on — the auto BOX BOX
    /// trigger. See [`super::endurance::box_this_lap`].
    pub box_this_lap: bool,
    /// Metres from the player to their own pit stall along the lap, for the
    /// BOX BOX border to start pulsing on the approach. `None` where the stall
    /// or the track length isn't known — the border still lights, it just
    /// never pulses.
    pub metres_to_pit: Option<f32>,
}

/// The session-wide flag flying now, reduced to what the status border shows.
///
/// From the global `SessionFlags` bitfield, not the per-car one the Relative's
/// penalty gutter reads. Only the four states the border has a colour for —
/// the many other bits (start-ready, ten-to-go, random waving) collapse to the
/// nearest of these or to `Green`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CourseFlag {
    /// Racing, or nothing worth a border.
    #[default]
    Green,
    /// A full-course yellow or local caution.
    Yellow,
    /// The white flag: one lap to the finish.
    White,
    /// The chequered flag: the session is over.
    Checkered,
}

/// The identity team sync needs to join a room and stamp its events.
///
/// Carried on the snapshot because the app owns the sync client but the
/// session thread owns the YAML these come from — the same reason the seat
/// rides along. Every field is optional: until the session info has been
/// read they are unknown, and the app simply does not connect yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionIdentity {
    /// `WeekendInfo.SubSessionID` — the room key.
    pub subsession: Option<u64>,
    /// The player's own iRacing customer id — their producer id on the wire.
    pub player_cust_id: Option<u32>,
    /// The player's display name, for "set by <name>" notes.
    pub player_name: Option<Arc<str>>,
}

/// Converts a raw `CarIdxTrackSurface` element into its enum, defaulting
/// unrecognized values to `NotInWorld` (the SDK's own "no car here" state).
#[must_use]
pub fn track_location_from_raw(raw: i32) -> TrackLocation {
    match raw {
        0 => TrackLocation::OffTrack,
        1 => TrackLocation::InPitStall,
        2 => TrackLocation::ApproachingPits,
        3 => TrackLocation::OnTrack,
        _ => TrackLocation::NotInWorld,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowest_remaining_pct_reads_the_worst_measured_edge() {
        let mut info = TyreInfo::default();
        assert_eq!(info.lowest_remaining_pct(), None, "all zeros is the not-measured sentinel");

        info.corners[0].wear = [0.95, 0.93, 0.90];
        info.corners[2].wear = [0.98, 0.97, 0.88];
        // Corner 1 and 3 still all-zero: unmeasured, not worn to the cords.
        let lowest = info.lowest_remaining_pct().expect("wear latched");
        assert!((lowest - 88.0).abs() < 0.01, "the worst edge across all corners: {lowest}");
    }

    #[test]
    fn track_location_maps_known_discriminants() {
        assert_eq!(track_location_from_raw(-1), TrackLocation::NotInWorld);
        assert_eq!(track_location_from_raw(0), TrackLocation::OffTrack);
        assert_eq!(track_location_from_raw(1), TrackLocation::InPitStall);
        assert_eq!(track_location_from_raw(2), TrackLocation::ApproachingPits);
        assert_eq!(track_location_from_raw(3), TrackLocation::OnTrack);
    }

    #[test]
    fn track_location_falls_back_to_not_in_world_for_unknown_values() {
        assert_eq!(track_location_from_raw(99), TrackLocation::NotInWorld);
    }
}
