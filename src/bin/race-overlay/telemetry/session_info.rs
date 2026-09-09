// Rust guideline compliant 2026-02-16

//! Shape of the `Session::session_info()` YAML blob, reduced to the fields
//! this crate's widgets need.

use anyhow::Context;
use serde::{Deserialize, Deserializer};

/// Parsed session info: only the sections this app reads.
#[expect(
    clippy::struct_field_names,
    reason = "field names mirror the YAML section names (WeekendInfo/SessionInfo/DriverInfo) on purpose"
)]
#[derive(Debug, Deserialize)]
pub struct SessionInfoYaml {
    #[serde(rename = "WeekendInfo", default)]
    pub weekend_info: WeekendInfo,
    #[serde(rename = "SessionInfo", default)]
    pub session_info: SessionInfoSection,
    /// The event's qualifying classification — the grid, in effect. Present
    /// for the whole event, which makes it the one starting-order source
    /// that survives joining or restarting the overlay mid-race.
    #[serde(rename = "QualifyResultsInfo", default)]
    pub qualify_results_info: QualifyResultsInfo,
    #[serde(rename = "DriverInfo")]
    pub driver_info: DriverInfo,
}

/// The `QualifyResultsInfo` section: how the field qualified. Absent in
/// sessions with no qualifying (practice, or a race with none attached).
#[derive(Debug, Default, Deserialize)]
pub struct QualifyResultsInfo {
    #[serde(rename = "Results", default)]
    pub results: Vec<QualifyResult>,
}

/// One car's qualifying classification.
///
/// Unlike a session's `ResultsPositions`, both positions here are
/// **zero-based** — the pole sitter is `Position: 0`.
#[derive(Debug, Deserialize)]
pub struct QualifyResult {
    #[serde(rename = "Position", default)]
    pub position: i32,
    #[serde(rename = "ClassPosition", default)]
    pub class_position: i32,
    #[serde(rename = "CarIdx")]
    pub car_idx: i32,
}

/// The `WeekendInfo` section: the incident-limit chip's field, plus the
/// track's own length.
#[derive(Debug, Default, Deserialize)]
pub struct WeekendInfo {
    /// The lap length as iRacing writes it, e.g. `"4.5298 km"`; parse with
    /// [`parse_track_length`].
    ///
    /// `TrackLength` rather than `TrackLengthOfficial`, which is the
    /// advertised figure and disagrees — 4.5298 km against 4.57 km at Spa.
    /// Anything converting a `LapDistPct` into metres has to use the length
    /// the sim actually measures those percentages against.
    #[serde(rename = "TrackLength", default)]
    pub track_length: String,
    /// `1` in a team session, where `DriverCarIdx` names the team's car
    /// rather than whoever is in it and the incident limit is applied to the
    /// team's count. See `plans/team-endurance.md`.
    #[serde(rename = "TeamRacing", default)]
    pub team_racing: i32,
    /// iRacing's unique id for this specific running of the session — the
    /// room key team sync joins members by (see `plans/team-sync.md`).
    /// `None` where a YAML omits it.
    #[serde(rename = "SubSessionID", default)]
    pub sub_session_id: Option<u64>,
    #[serde(rename = "WeekendOptions", default)]
    pub weekend_options: WeekendOptions,
}

/// Metres from a track-length string like `"4.5298 km"` or `"1200 m"`.
///
/// iRacing writes kilometres in every session seen so far, but the unit is
/// published alongside the number rather than implied, so it is read rather
/// than assumed — a track silently taken as 4.5 km instead of 4500 m would
/// put every derived distance out by a thousand.
///
/// `None` for an empty, unparseable, or non-positive value, so a session that
/// doesn't publish it reads as "unknown" rather than as a zero-length lap.
#[must_use]
pub fn parse_track_length(text: &str) -> Option<f32> {
    let mut parts = text.split_whitespace();
    let number: f32 = parts.next()?.parse().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    match parts.next().unwrap_or("km") {
        "km" => Some(number * 1000.0),
        "m" => Some(number),
        _ => None,
    }
}

/// The `WeekendInfo.WeekendOptions` section.
#[derive(Debug, Default, Deserialize)]
pub struct WeekendOptions {
    /// e.g. `"4"` or `"unlimited"`; parse with [`parse_incident_limit`].
    #[serde(rename = "IncidentLimit", default)]
    pub incident_limit: String,
    /// e.g. `"23 %"`; parse with [`parse_percent`].
    #[serde(rename = "ChanceOfRain", default)]
    pub chance_of_rain: String,
}

/// Parses a percentage as iRacing writes it in session info — a number
/// followed by an optional `%`, e.g. `"23 %"` — into a 0.0-1.0 fraction.
///
/// Returns `None` for an empty or unparseable value, so a session that
/// doesn't publish the field reads as "unknown" rather than "zero".
#[must_use]
pub fn parse_percent(text: &str) -> Option<f32> {
    text.trim().trim_end_matches('%').trim().parse::<f32>().ok().map(|percent| percent / 100.0)
}

/// Parses `WeekendOptions.IncidentLimit`. iRacing writes this as either a
/// plain number or the literal string `"unlimited"`. Returns `None` for
/// `"unlimited"` (nothing to show a fraction against) or unparseable input.
#[must_use]
pub fn parse_incident_limit(text: &str) -> Option<i32> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("unlimited") {
        return None;
    }
    text.parse::<i32>().ok()
}

/// The `SessionInfo` section: one entry per practice/qualify/race session.
#[derive(Debug, Default, Deserialize)]
pub struct SessionInfoSection {
    #[serde(rename = "Sessions", default)]
    pub sessions: Vec<SessionResults>,
}

/// One session's classification data.
#[derive(Debug, Deserialize)]
pub struct SessionResults {
    /// Matched against the live `SessionNum` telemetry var — never assume
    /// array position equals session number.
    #[serde(rename = "SessionNum")]
    pub session_num: i32,
    /// e.g. `"Race"`, `"Qualify"`, `"Practice"`, `"Warmup"` — shown as its
    /// first letter in the Standings header.
    #[serde(rename = "SessionType", default)]
    pub session_type: String,
    /// This session's scheduled length as iRacing writes it — `"2400.0000
    /// sec"`, or `"unlimited"` for a session with no clock. Parsed by
    /// [`parse_session_seconds`].
    ///
    /// The race clock has to come from here rather than from adding the
    /// `SessionTime` and `SessionTimeRemain` telemetry vars together. Those
    /// two are measured from different places — `SessionTime` counts from the
    /// moment the session loaded, so it includes gridding and the pace laps,
    /// while `SessionTimeRemain` counts down the racing itself — and their sum
    /// is the race length plus however long the field spent forming up. That
    /// is how a 40-minute race came to be shown as a 44-minute one.
    #[serde(rename = "SessionTime", default)]
    pub session_time: String,
    /// This session's scheduled lap count as iRacing writes it — `"20"`, or
    /// `"unlimited"` for a session bounded by the clock alone. Parsed by
    /// [`parse_session_laps`].
    #[serde(rename = "SessionLaps", default)]
    pub session_laps: String,
    #[serde(rename = "ResultsPositions", default)]
    pub results_positions: Vec<ResultsPosition>,
}

/// Seconds from a session-length string like `"2400.0000 sec"`.
///
/// `None` for `"unlimited"`, for an empty field, and for anything else that
/// doesn't lead with a number — an unbounded session has no length to show,
/// and a guess would be worse than the dash the UI falls back to.
#[must_use]
pub fn parse_session_seconds(text: &str) -> Option<f64> {
    let number = text.split_whitespace().next()?;
    number.parse::<f64>().ok().filter(|secs| secs.is_finite() && *secs > 0.0)
}

/// A lap count from a session-length string like `"20"`.
///
/// `None` for `"unlimited"`, for an empty field, and for anything else that
/// isn't a positive whole number: a session with no lap limit has no count to
/// show, and the clock is what bounds it instead.
#[must_use]
pub fn parse_session_laps(text: &str) -> Option<i32> {
    text.trim().parse::<i32>().ok().filter(|laps| *laps > 0)
}

/// One driver's row in a session's official classification.
#[derive(Debug, Deserialize)]
pub struct ResultsPosition {
    #[serde(rename = "Position")]
    pub position: i32,
    #[serde(rename = "ClassPosition", default)]
    pub class_position: i32,
    #[serde(rename = "CarIdx")]
    pub car_idx: i32,
    #[serde(rename = "LapsComplete", default)]
    pub laps_complete: i32,
    #[serde(rename = "FastestTime", default)]
    pub fastest_time: f32,
    #[serde(rename = "LastTime", default)]
    pub last_time: f32,
    #[serde(rename = "PitStops", default)]
    pub pit_stops: i32,
}

/// The `DriverInfo` section: the player's own car plus the full driver list.
#[derive(Debug, Deserialize)]
pub struct DriverInfo {
    #[serde(rename = "DriverCarIdx")]
    pub driver_car_idx: i32,
    /// The player's own iRacing customer id. In a team session the car's
    /// `Drivers` entry carries whoever is in the seat, so comparing its
    /// `UserID` with this one is how "a team-mate is driving" is known.
    /// `None` where a YAML omits it.
    #[serde(rename = "DriverUserID", default)]
    pub driver_user_id: Option<i32>,
    /// Where the player's own pit stall sits, as a fraction of a lap — the
    /// same coordinate `CarIdxLapDistPct` reports positions in.
    ///
    /// A `DriverInfo` field rather than a per-driver one: iRacing publishes
    /// only your own stall, never anybody else's. Zero when the session has
    /// no pit lane.
    #[serde(rename = "DriverPitTrkPct", default)]
    pub driver_pit_trk_pct: f32,
    /// The player's car's tank, in litres, and the fraction of it this
    /// session allows — the two multiply to the fuel the car can actually
    /// carry. Both published by iRacing; `None` where a YAML omits them.
    #[serde(rename = "DriverCarFuelMaxLtr", default)]
    pub driver_car_fuel_max_ltr: Option<f32>,
    #[serde(rename = "DriverCarMaxFuelPct", default)]
    pub driver_car_max_fuel_pct: Option<f32>,
    /// The compounds the player's own car can fit, by index — the same index
    /// space `PlayerTireCompound` reports in. Empty where a YAML omits the
    /// list, which is how "the sim named no compounds" is known.
    #[serde(rename = "DriverTires", default)]
    pub driver_tires: Vec<DriverTire>,
    #[serde(rename = "Drivers", default)]
    pub drivers: Vec<Driver>,
}

/// One entry in `DriverInfo.DriverTires`: a compound the player's car can
/// fit, named — e.g. `TireIndex: 1, TireCompoundType: "Wet"`.
#[derive(Debug, Deserialize)]
pub struct DriverTire {
    #[serde(rename = "TireIndex", default)]
    pub tire_index: i32,
    #[serde(rename = "TireCompoundType", default)]
    pub tire_compound_type: String,
}

impl DriverInfo {
    /// The fuel the player's car can carry in this session, in litres: the
    /// tank scaled by the session's fuel limit, `None` where the YAML gives
    /// no tank.
    ///
    /// A limit outside `0..=1` is treated as no limit rather than trusted: it
    /// can only come from a YAML this parser does not understand.
    #[must_use]
    pub fn tank_capacity_litres(&self) -> Option<f32> {
        let tank = self.driver_car_fuel_max_ltr.filter(|litres| litres.is_finite() && *litres > 0.0)?;
        let limit = self.driver_car_max_fuel_pct.filter(|pct| *pct > 0.0 && *pct <= 1.0).unwrap_or(1.0);
        Some(tank * limit)
    }
}

/// One entry in `DriverInfo.Drivers`, matched by its own `CarIdx` field.
#[derive(Debug, Deserialize)]
pub struct Driver {
    #[serde(rename = "CarIdx")]
    pub car_idx: i32,
    /// Whoever is in the car right now: in a team session iRacing rewrites
    /// this entry at every driver swap.
    #[serde(rename = "UserName", default)]
    pub user_name: String,
    /// The current driver's customer id, matched against
    /// [`DriverInfo::driver_user_id`]. `None` where a YAML omits it.
    #[serde(rename = "UserID", default)]
    pub user_id: Option<i32>,
    /// The number on the car, as iRacing writes it — a string, because a
    /// number like `007` keeps its zeros.
    #[serde(rename = "CarNumber", default)]
    pub car_number: String,
    /// The car's full display name, e.g. `"McLaren 720S GT3 EVO"`; the
    /// Relative widget's "Car" column shows just its first word (the
    /// manufacturer).
    #[serde(rename = "CarScreenName", default)]
    pub car_screen_name: String,
    #[serde(rename = "IRating", default)]
    pub irating: i32,
    /// The license class's color; same dual string/integer encoding as
    /// `CarClassColor`, so it goes through the same normalizer.
    #[serde(rename = "LicColor", default, deserialize_with = "deserialize_color_as_hex")]
    pub license_color: String,
    /// Identifies which class this driver races in — matched against other
    /// drivers' `car_class_id` to group Standings, not display order.
    #[serde(rename = "CarClassID", default)]
    pub car_class_id: i32,
    #[serde(rename = "CarClassShortName", default)]
    pub car_class_short_name: String,
    /// Color string; iRacing writes this as either a bare decimal number or
    /// a hex string depending on session type, so it's normalized to a
    /// `"RRGGBB"` hex string here rather than assumed to match `LicColor`'s
    /// format.
    #[serde(rename = "CarClassColor", default, deserialize_with = "deserialize_color_as_hex")]
    pub car_class_color: String,
    /// iRacing's own ranking of how quick this driver's class is: an integer,
    /// equal across a class, higher being faster. `None` where a YAML omits
    /// it, so a missing figure reads as "unknown" rather than "slowest" — see
    /// `telemetry::faster_class::is_faster_class`.
    #[serde(rename = "CarClassRelSpeed", default)]
    pub car_class_rel_speed: Option<i32>,
    /// iRacing's estimated lap for the class, in seconds; lower is quicker.
    /// The fallback where the ranking above ties or is absent.
    #[serde(rename = "CarClassEstLapTime", default)]
    pub car_class_est_lap_time: Option<f32>,
    /// `1` for the pace car's own entry. iRacing leaves it out of
    /// `ResultsPositions`, so anything reconstructing the running order from
    /// live telemetry instead has to exclude it by hand.
    #[serde(rename = "CarIsPaceCar", default)]
    pub car_is_pace_car: i32,
    /// `1` for someone watching rather than driving; excluded for the same
    /// reason as [`Driver::car_is_pace_car`].
    #[serde(rename = "IsSpectator", default)]
    pub is_spectator: i32,
    /// The flag the member picked on their iRacing profile, as iRacing's own
    /// id — `ui::flags` turns it into a country. Zero where a YAML omits it;
    /// the pace car reports `2`; both mean "no flag".
    #[serde(rename = "FlairID", default)]
    pub flair_id: i32,
}

/// Accepts a color as either a YAML string (e.g. `"0xFF3333"`) or a bare
/// integer (e.g. `16711680`), normalizing both to a `"RRGGBB"` hex string.
fn deserialize_color_as_hex<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    struct ColorVisitor;
    impl serde::de::Visitor<'_> for ColorVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a color as a hex string or an integer")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_owned())
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<String, E> {
            Ok(format!("{v:06X}"))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<String, E> {
            Ok(format!("{v:06X}"))
        }
    }
    deserializer.deserialize_any(ColorVisitor)
}

impl SessionInfoYaml {
    /// Parses raw session-info YAML from `Session::session_info()`.
    ///
    /// # Errors
    /// Returns an error if the YAML doesn't match the expected shape.
    pub fn parse(yaml: &str) -> anyhow::Result<Self> {
        serde_yaml::from_str(yaml).context("parsing session_info YAML")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_drivers_by_car_idx_field_not_array_position() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 5
  Drivers:
  - CarIdx: 9
    UserName: Someone Else
    CarScreenName: BMW M4 GT3
  - CarIdx: 5
    UserName: Flynn
    CarScreenName: McLaren 720S GT3 EVO
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        assert_eq!(info.driver_info.driver_car_idx, 5);

        let me = info.driver_info.drivers.iter().find(|d| d.car_idx == 5).expect("car 5 must be present");
        assert_eq!(me.user_name, "Flynn");
        assert_eq!(me.car_screen_name, "McLaren 720S GT3 EVO");
    }

    /// The fields a team session adds: the flag, the player's own id and
    /// who is in each car right now. The car's entry names the team-mate
    /// while the player's id is their own, which is the swap detected.
    #[test]
    fn team_session_fields_parse() {
        let yaml = r"
WeekendInfo:
  TeamRacing: 1
DriverInfo:
  DriverCarIdx: 5
  DriverUserID: 111
  Drivers:
  - CarIdx: 5
    UserName: Istvan Fodor
    UserID: 222
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        assert_eq!(info.weekend_info.team_racing, 1);
        assert_eq!(info.driver_info.driver_user_id, Some(111));
        let car = &info.driver_info.drivers[0];
        assert_eq!(car.user_id, Some(222));

        let solo = SessionInfoYaml::parse(
            "DriverInfo:
  DriverCarIdx: 0
  Drivers:
  - CarIdx: 0
",
        )
        .expect("must parse");
        assert_eq!(solo.weekend_info.team_racing, 0);
        assert_eq!(solo.driver_info.driver_user_id, None);
        assert_eq!(solo.driver_info.drivers[0].user_id, None);
    }

    #[test]
    fn missing_drivers_list_still_parses() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 0
";
        let info = SessionInfoYaml::parse(yaml).expect("driver info without a Drivers list must parse");
        assert!(info.driver_info.drivers.is_empty());
    }

    #[test]
    fn driver_irating_and_license_parse() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 0
  Drivers:
  - CarIdx: 0
    UserName: Flynn
    IRating: 2453
    LicString: A 3.28
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        let me = &info.driver_info.drivers[0];
        assert_eq!(me.irating, 2453);
    }

    #[test]
    fn parses_percentages_with_and_without_a_sign() {
        assert!((parse_percent("23 %").expect("must parse") - 0.23).abs() < 1e-6);
        assert!((parse_percent("0").expect("must parse")).abs() < 1e-6);
        assert!(parse_percent("").is_none());
        assert!(parse_percent("n/a").is_none());
    }

    /// The unit is read, not assumed: taking `"4.5298 km"` at face value as
    /// metres puts every distance derived from it out by a thousandfold.
    #[test]
    fn track_length_reads_its_own_unit() {
        assert!((parse_track_length("4.5298 km").expect("must parse") - 4529.8).abs() < 0.01);
        assert!((parse_track_length("1200 m").expect("must parse") - 1200.0).abs() < 0.01);
        // No unit at all: kilometres, which is what iRacing publishes.
        assert!((parse_track_length("3.7").expect("must parse") - 3700.0).abs() < 0.01);
        assert_eq!(parse_track_length(""), None);
        assert_eq!(parse_track_length("unlimited"), None);
        assert_eq!(parse_track_length("0.0 km"), None);
        assert_eq!(parse_track_length("-2 km"), None);
        assert_eq!(parse_track_length("4.5 furlongs"), None);
    }

    /// A session with no pit lane publishes zero here, which has to survive
    /// parsing as zero rather than as an error that loses the rest of the
    /// driver info with it.
    #[test]
    fn driver_pit_position_parses_and_defaults_to_zero() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 32
  DriverPitTrkPct: 0.035053
  Drivers:
  - CarIdx: 32
    UserName: Flynn
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        assert!((info.driver_info.driver_pit_trk_pct - 0.035_053).abs() < 1e-6);

        let without = SessionInfoYaml::parse("DriverInfo:\n  DriverCarIdx: 0\n").expect("must parse");
        assert_eq!(without.driver_info.tank_capacity_litres(), None, "no tank published, no capacity");
        assert!(without.driver_info.driver_pit_trk_pct.abs() < f32::EPSILON);
    }

    #[test]
    fn parses_numeric_and_unlimited_lap_counts() {
        assert_eq!(parse_session_laps("20"), Some(20));
        assert_eq!(parse_session_laps(" 45 "), Some(45));
        assert_eq!(parse_session_laps("unlimited"), None);
        assert_eq!(parse_session_laps(""), None);
        assert_eq!(parse_session_laps("0"), None);
    }

    #[test]
    fn parses_numeric_and_unlimited_incident_limits() {
        assert_eq!(parse_incident_limit("25"), Some(25));
        assert_eq!(parse_incident_limit(" 4 "), Some(4));
        assert_eq!(parse_incident_limit("unlimited"), None);
        assert_eq!(parse_incident_limit("Unlimited"), None);
        assert_eq!(parse_incident_limit("bogus"), None);
    }

    #[test]
    fn car_class_color_accepts_a_quoted_hex_string() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 0
  Drivers:
  - CarIdx: 0
    CarClassID: 84
    CarClassShortName: GT3
    CarClassColor: '0xFF3333'
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        let me = &info.driver_info.drivers[0];
        assert_eq!(me.car_class_id, 84);
        assert_eq!(me.car_class_short_name, "GT3");
        assert_eq!(me.car_class_color, "0xFF3333");
    }

    /// The race clock is derived from this, so an unbounded session has to
    /// come back as "no length" rather than as some number of seconds.
    #[test]
    fn session_length_parses_the_seconds_and_rejects_the_unbounded() {
        assert_eq!(parse_session_seconds("2400.0000 sec"), Some(2400.0));
        assert_eq!(parse_session_seconds(" 900 sec "), Some(900.0));
        assert_eq!(parse_session_seconds("unlimited"), None);
        assert_eq!(parse_session_seconds(""), None);
        assert_eq!(parse_session_seconds("0.0000 sec"), None);
    }

    /// The Faster Class widget's whole basis for "quicker". Both fields are
    /// optional so a YAML without them reads as unknown rather than failing.
    #[test]
    fn class_speed_fields_parse_and_default_to_unknown() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 0
  Drivers:
  - CarIdx: 0
    CarClassID: 84
    CarClassRelSpeed: 55
    CarClassEstLapTime: 104.25
  - CarIdx: 1
    CarClassID: 85
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        let gt3 = &info.driver_info.drivers[0];
        assert_eq!(gt3.car_class_rel_speed, Some(55));
        assert!((gt3.car_class_est_lap_time.expect("an estimate was published") - 104.25).abs() < 1e-4);
        let other = &info.driver_info.drivers[1];
        assert_eq!(other.car_class_rel_speed, None);
        assert_eq!(other.car_class_est_lap_time, None);
    }

    #[test]
    fn car_class_color_accepts_a_bare_integer() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 0
  Drivers:
  - CarIdx: 0
    CarClassColor: 16711680
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        assert_eq!(info.driver_info.drivers[0].car_class_color, "FF0000");
    }

    #[test]
    fn results_positions_matched_by_session_num_field_not_array_position() {
        let yaml = r"
DriverInfo:
  DriverCarIdx: 0
SessionInfo:
  Sessions:
  - SessionNum: 1
    ResultsPositions:
    - Position: 1
      CarIdx: 9
  - SessionNum: 0
    ResultsPositions:
    - Position: 1
      CarIdx: 5
      LapsComplete: 3
      FastestTime: 91.5
      LastTime: 92.1
      PitStops: 1
";
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        let session0 =
            info.session_info.sessions.iter().find(|s| s.session_num == 0).expect("session 0 must be present");
        assert_eq!(session0.results_positions.len(), 1);
        let row = &session0.results_positions[0];
        assert_eq!(row.car_idx, 5);
        assert_eq!(row.laps_complete, 3);
        assert!((row.fastest_time - 91.5).abs() < f32::EPSILON);
        assert_eq!(row.pit_stops, 1);
    }

    /// The tank is the published litres scaled by the session's fuel limit.
    #[test]
    fn the_tank_is_the_published_litres_scaled_by_the_fuel_limit() {
        let yaml = "DriverInfo:\n  DriverCarIdx: 0\n  DriverCarFuelMaxLtr: 110.0\n  DriverCarMaxFuelPct: 0.75\n";
        let info = SessionInfoYaml::parse(yaml).expect("must parse");
        let capacity = info.driver_info.tank_capacity_litres().expect("a tank was published");
        assert!((capacity - 82.5).abs() < 1e-3, "got {capacity}");

        let unlimited = "DriverInfo:\n  DriverCarIdx: 0\n  DriverCarFuelMaxLtr: 110.0\n";
        let info = SessionInfoYaml::parse(unlimited).expect("must parse");
        assert!((info.driver_info.tank_capacity_litres().expect("a tank") - 110.0).abs() < 1e-3);
    }

    /// `DriverTires` names the player's compounds by index; a YAML without
    /// the list parses to an empty one, which is how callers know the sim
    /// named nothing.
    #[test]
    fn parses_the_named_compound_list() {
        let yaml = r#"
DriverInfo:
  DriverCarIdx: 0
  DriverTires:
  - TireIndex: 0
    TireCompoundType: "Hard"
  - TireIndex: 1
    TireCompoundType: "Wet"
"#;
        let info = SessionInfoYaml::parse(yaml).expect("valid session info must parse");
        let tires = &info.driver_info.driver_tires;
        assert_eq!(tires.len(), 2);
        assert_eq!((tires[0].tire_index, tires[0].tire_compound_type.as_str()), (0, "Hard"));
        assert_eq!((tires[1].tire_index, tires[1].tire_compound_type.as_str()), (1, "Wet"));

        let without = SessionInfoYaml::parse("DriverInfo:\n  DriverCarIdx: 0\n").expect("must parse");
        assert!(without.driver_info.driver_tires.is_empty());
    }
}
