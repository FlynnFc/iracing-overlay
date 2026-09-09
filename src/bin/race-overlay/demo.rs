// Rust guideline compliant 2026-02-16

//! A fixed telemetry snapshot reproducing the design mockups' data, so the
//! widgets can be rendered and compared against `design mocks/` without
//! iRacing running.
//!
//! Every name, lap time, gap and rating here is read off the mockups
//! directly. That's the point: `race-overlay.exe --demo` should put the real
//! widgets side by side with the images they were designed from, which only
//! works if the data matches. Treat this as a visual fixture, not as
//! representative telemetry.

use std::sync::Arc;

use iracing_telem::flags::TrackLocation;

use crate::telemetry::pit::PitService;
use crate::telemetry::snapshot::{
    Approaching, CarAdjustments, CarSnapshot, ClassSection, EnduranceMeta, FasterClassSnapshot, Penalty, RadarCar,
    RadarSide, RadarSnapshot, RelativeMeta, Seat, SessionKind, StandingsEntry, TelemetrySnapshot, TrackWetness,
    TyreCompound, TyreInfo, TyreState, WeatherSnapshot,
};

/// The header's session-wide figures: a 45-minute race twenty minutes in,
/// under green, as the driver sees it.
fn relative_meta() -> RelativeMeta {
    RelativeMeta {
        sof: Some(2800),
        session_kind: SessionKind::Race,
        car_count: 36,
        // The demo shows the widgets as a driver sees them, under green.
        spectating: None,
        team_mate: None,
        under_caution: false,
        incidents: 6,
        incident_limit: Some(25),
        race_elapsed_secs: 20.0 * 60.0 + 8.0,
        race_remain_secs: Some(24.0 * 60.0 + 52.0),
        // Green flag out, so the clock is running rather than holding.
        racing_under_way: true,
        session_length_secs: Some(45.0 * 60.0),
        current_lap: 12,
        predicted_total_laps: Some(25),
        session_laps: None,
        grid: None,
    }
}

/// Class identifiers, colors and names, matching the mockups' three classes.
const DP: (i32, &str, &str) = (0, "DP", "0xE0A82E");
const GTE: (i32, &str, &str) = (1, "GTE", "0x00A9E0");
const GT3: (i32, &str, &str) = (2, "GT3", "0xE81E5B");

/// License colors iRacing uses for the A and B classes.
const LIC_A: &str = "0x0153DB";
const LIC_B: &str = "0x00C702";

/// Profile flags dealt round the mockup's drivers by car index, no two
/// neighbours sharing: Great Britain, Germany, the United States, Italy,
/// France, no-flag, Brazil and Japan, as iRacing's `FlairID`s (see
/// `ui::flags`). The `0` is deliberate — one driver with no flag on their
/// profile, so the world fallback stays visible in every layout preview.
const FLAIRS: [i32; 8] = [222, 77, 223, 101, 71, 0, 31, 104];

/// The flag for the `n`th demo car.
fn demo_flair(n: usize) -> i32 {
    FLAIRS[n % FLAIRS.len()]
}

/// Builds the mockups' session as a single snapshot.
#[must_use]
pub fn snapshot() -> TelemetrySnapshot {
    let standings = standings();
    let relative_rows = relative();
    let focus_index = relative_rows.iter().position(|car| car.is_focus).unwrap_or(0);
    TelemetrySnapshot {
        relative: relative_rows,
        focus_index,
        relative_meta: relative_meta(),
        focus_car_class_id: Some(GT3.0),
        class_sections: vec![
            ClassSection {
                car_class_id: DP.0,
                short_name: Arc::from("GTP"),
                color: Arc::from(DP.2),
                car_count: 1,
                sof: Some(1852),
            },
            ClassSection {
                car_class_id: GTE.0,
                short_name: Arc::from("LMP2"),
                color: Arc::from(GTE.2),
                car_count: 20,
                sof: Some(4690),
            },
            ClassSection {
                car_class_id: GT3.0,
                short_name: Arc::from(GT3.1),
                color: Arc::from(GT3.2),
                car_count: 15,
                sof: Some(2817),
            },
        ],
        standings,
        radar: radar(),
        faster_class: faster_class(),
        weather: WeatherSnapshot {
            track_temp_c: 54.0,
            air_temp_c: 23.0,
            // 3.22 km/h, the figure on the mockup.
            wind_speed_mps: 3.22 / 3.6,
            fog: Some(0.10),
            precip_chance: Some(0.23),
            // A shower in progress, on wets, so every rain-facing surface —
            // the live-rain reading, the footer's WET tag — shows up in a
            // `--demo` screenshot rather than needing a wet session to see.
            precip_now: Some(0.42),
            declared_wet: true,
            track_wetness: Some(TrackWetness::LightlyWet),
            on_wet_tyres: true,
            // Roughly north-west, matching the mockup's arrow.
            wind_dir_relative_to_car_rad: Some(-2.4),
        },
        endurance: EnduranceMeta {
            multi_stop_race: true,
            laps_remaining: Some(13),
            lap_driven_pct: Some(0.4),
            stops_remaining: Some(1),
            projected_class_position: Some(10),
            best_stops_in_class: Some(0),
        },
        pit_projection: None,
        pit_service: PitService {
            fuel_armed: true,
            fuel_amount_litres: 55.0,
            fuel_level_litres: 55.0,
            tank_capacity_litres: Some(110.0),
            // 55.0 / 2.94 == 18.7 laps, the figure on the mockup.
            fuel_per_lap_litres: Some(2.94),
            tyres_armed: [true; 4],
            tyre_pressures_kpa: [159.0; 4],
            pending_tyre_compound: Some(0),
            tearoff_armed: true,
            fast_repair_armed: false,
            fast_repairs_available: 0,
            in_car: true,
            on_pit_road: false,
            refuel_target_litres: None,
        },
        tyres: tyres(),
        pit_model: crate::telemetry::pit_model::PitModel::default(),
        fuel_use: crate::telemetry::snapshot::FuelUse { used_this_lap_litres: Some(1.4), lap_fraction: Some(0.5) },
        // The mockups are of a car on track, and demo mode has no garage to
        // be hidden by in any case.
        in_garage: false,
        seat: Seat::Driving,
        identity: crate::telemetry::snapshot::SessionIdentity::default(),
        session_time_secs: 0.0,
        course_flag: crate::telemetry::snapshot::CourseFlag::Green,
        box_this_lap: false,
        metres_to_pit: None,
        adjustments: CarAdjustments {
            brake_bias: Some(54.0),
            abs: Some(3.0),
            traction_control: Some(4.0),
            throttle_shape: Some(1.0),
            dash_page: Some(0.0),
        },
    }
}

/// A deliberately asymmetric radar reading, unlike the mockup's matched pair.
///
/// A symmetric pair exercises none of what the widget has to get right, and
/// the mockup's own version predates the widget being drawn in metres at all.
/// Left holds a car overlapping the player's door and still coming — the case
/// that lights the rail. Right holds one dropping away with about a car length
/// of air behind it, and one far enough up the road to draw as a pip rather
/// than a block.
fn radar() -> RadarSnapshot {
    RadarSnapshot {
        left: RadarSide {
            ahead: None,
            behind: Some(RadarCar { separation_m: -2.6, gap_ms: -47.0, closing_mps: Some(1.4) }),
        },
        right: RadarSide {
            ahead: Some(RadarCar { separation_m: 17.5, gap_ms: 318.0, closing_mps: Some(-0.2) }),
            behind: Some(RadarCar { separation_m: -9.9, gap_ms: -180.0, closing_mps: Some(-2.1) }),
        },
    }
}

/// The quicker traffic behind the focus car, as the Faster Class widget sees
/// it in layout mode: a GTP already 1.8 s back and closing at a rate that
/// projects an arrival, and an LMP2 at 2.6 s — close enough to sit inside a
/// typical warning span at its own position rather than pinned to the far
/// end. The two classes the plate design is sized against, in iRacing's
/// usual colours for them, with the LMP2 also giving the `+1` chip
/// something to count. Nearest first, as the telemetry thread sends them.
fn faster_class() -> FasterClassSnapshot {
    FasterClassSnapshot {
        approaching: vec![
            Approaching {
                car_idx: 4,
                behind_secs: 1.8,
                closing_rate: Some(0.12),
                driver_name: Arc::from("Driver 2"),
                car_number: Arc::from("5"),
                car_class_short_name: Arc::from("GTP"),
                car_class_color: Arc::from("0xFFDA59"),
            },
            Approaching {
                car_idx: 40,
                behind_secs: 2.6,
                closing_rate: Some(0.3),
                driver_name: Arc::from("Driver 20"),
                car_number: Arc::from("12"),
                car_class_short_name: Arc::from("LMP2"),
                car_class_color: Arc::from("0x33CEFF"),
            },
        ],
    }
}

/// The Tire Info mockup's readings (`design mocks/Screenshot_18.jpg`).
///
/// Each corner is `[left, middle, right]` in the sim's own ordering, which is
/// also the order the mockup prints them.
fn tyres() -> TyreInfo {
    let corner = |temps: [f32; 3], wear: [f32; 3], pressure_kpa: f32| TyreState {
        temps_c: temps,
        wear: wear.map(|percent| percent / 100.0),
        pressure_kpa,
    };
    TyreInfo {
        corners: [
            corner([68.0, 78.0, 83.0], [98.0, 97.0, 97.0], 177.0),
            corner([78.0, 72.0, 62.0], [98.0, 98.0, 99.0], 174.0),
            corner([70.0, 78.0, 81.0], [98.0, 98.0, 98.0], 174.0),
            corner([76.0, 72.0, 60.0], [98.0, 98.0, 99.0], 171.0),
        ],
    }
}

/// One row of the Standings mockup, before it's expanded into a full entry.
struct Row {
    class_position: i32,
    position: i32,
    name: &'static str,
    car: &'static str,
    irating: i32,
    gap: f32,
    best: f32,
    last: f32,
    laps_down: i32,
    location: TrackLocation,
}

/// Builds `M:SS.mmm` lap times from their parts, so the table below reads
/// the way the mockup does rather than as pre-summed seconds.
const fn lap(minutes: f32, seconds: f32) -> f32 {
    minutes * 60.0 + seconds
}

#[expect(
    clippy::too_many_lines,
    reason = "a verbatim transcription of the mockup's classification table; splitting it per class would scatter the data this fixture exists to hold in one readable place"
)]
fn standings() -> Vec<StandingsEntry> {
    let gtp = [Row {
        class_position: 1,
        position: 1,
        name: "Driver 1",
        car: "Ferrari 499P",
        irating: 1800,
        gap: 0.0,
        best: lap(1.0, 33.635),
        last: lap(1.0, 35.063),
        laps_down: 0,
        location: TrackLocation::OnTrack,
    }];
    let lmp2 = [
        Row {
            class_position: 1,
            position: 2,
            name: "Driver 2",
            car: "Dallara P217",
            irating: 8000,
            gap: 0.0,
            best: lap(1.0, 38.635),
            last: lap(1.0, 40.063),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 2,
            position: 3,
            name: "Driver 3",
            car: "Dallara P217",
            irating: 6800,
            gap: 4.5,
            best: lap(1.0, 39.281),
            last: lap(1.0, 39.638),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 3,
            position: 4,
            name: "Driver 4",
            car: "Dallara P217",
            irating: 6400,
            gap: 4.6,
            best: lap(1.0, 39.209),
            last: lap(1.0, 39.560),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
    ];
    let gt3 = [
        Row {
            class_position: 1,
            position: 22,
            name: "Driver 5",
            car: "Audi R8 LMS GT3 EVO II",
            irating: 6300,
            gap: 0.0,
            best: lap(1.0, 42.198),
            last: lap(1.0, 42.353),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 2,
            position: 23,
            name: "Driver 6",
            car: "BMW M4 GT3",
            irating: 6600,
            gap: 0.9,
            best: lap(1.0, 42.089),
            last: lap(1.0, 42.619),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 3,
            position: 24,
            name: "Driver 7",
            car: "Ferrari 296 GT3",
            irating: 4400,
            gap: 11.5,
            best: lap(1.0, 42.702),
            last: lap(1.0, 43.231),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 9,
            position: 30,
            name: "Driver 8",
            car: "McLaren 720S GT3 EVO",
            irating: 5200,
            gap: 32.0,
            best: lap(1.0, 42.278),
            last: lap(1.0, 43.334),
            laps_down: 0,
            location: TrackLocation::OffTrack,
        },
        Row {
            class_position: 10,
            position: 31,
            name: "Driver 9",
            car: "Porsche 911 GT3 R",
            irating: 2500,
            gap: 35.2,
            best: lap(1.0, 44.002),
            last: lap(1.0, 44.247),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 11,
            position: 32,
            name: "Driver 10",
            car: "Lamborghini Huracan GT3 EVO",
            irating: 1900,
            gap: 44.1,
            best: lap(1.0, 44.059),
            last: lap(1.0, 44.292),
            laps_down: 0,
            location: TrackLocation::InPitStall,
        },
        Row {
            class_position: 12,
            position: 33,
            name: "Driver 11",
            car: "Mercedes-AMG GT3 2020",
            irating: 2200,
            gap: 47.4,
            best: lap(1.0, 45.035),
            last: lap(1.0, 45.302),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 13,
            position: 34,
            name: "Driver 12",
            car: "Aston Martin Vantage GT3",
            irating: 1700,
            gap: 52.1,
            best: lap(1.0, 44.822),
            last: lap(1.0, 46.677),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 14,
            position: 35,
            name: "Driver 13",
            car: "Ford Mustang GT3",
            irating: 1900,
            gap: 53.5,
            best: lap(1.0, 44.754),
            last: lap(1.0, 46.184),
            laps_down: 0,
            location: TrackLocation::OnTrack,
        },
        Row {
            class_position: 15,
            position: 36,
            name: "Driver 14",
            car: "Acura NSX GT3 EVO 22",
            irating: 2400,
            gap: 0.0,
            best: lap(1.0, 43.111),
            last: lap(1.0, 44.601),
            laps_down: 11,
            location: TrackLocation::OnTrack,
        },
    ];

    let mut entries = Vec::new();
    let mut car_idx = 0;
    for (class, rows) in [((DP.0, "GTP", DP.2), &gtp[..]), ((GTE.0, "LMP2", GTE.2), &lmp2[..]), (GT3, &gt3[..])] {
        // The mockup's class-fastest cells: whoever holds the lowest best
        // lap in each class gets the highlighted timing cell and the
        // stopwatch chip in the gutter.
        let class_best = rows.iter().map(|r| r.best).fold(f32::INFINITY, f32::min);
        for row in rows {
            entries.push(StandingsEntry {
                position: row.position,
                class_position: row.class_position,
                car_idx,
                driver_name: Arc::from(row.name),
                car_screen_name: Arc::from(row.car),
                irating: row.irating,
                flair_id: demo_flair(usize::try_from(car_idx).unwrap_or(0)),
                car_class_id: class.0,
                car_class_short_name: Arc::from(class.1),
                car_class_color: Arc::from(class.2),
                best_lap_secs: row.best,
                last_lap_secs: row.last,
                gap_to_leader_secs: row.gap,
                pit_stops: 1,
                last_pit_secs: Some(28.4),
                avg_pit_secs: Some(26.9),
                stops_remaining: Some(1),
                projected_class_position: Some(row.class_position),
                current_stint_laps: 6,
                current_stint_secs: 9.0 * 60.0,
                avg_stint_laps: Some(7),
                avg_stint_secs: Some(11.0 * 60.0),
                track_location: row.location,
                is_class_fastest: (row.best - class_best).abs() < f32::EPSILON,
                laps_down: row.laps_down,
                is_focus: row.name == "Driver 11",
                off_tracks: if row.name == "Driver 12" { 2 } else { 0 },
                penalty: None,
                // One tow ticking and one car gambling on wets, so a
                // screenshot exercises both markers — on rows the standings
                // window actually shows.
                tow_secs: (row.name == "Driver 7").then_some(43.0),
                tyre: Some(TyreCompound {
                    letter: if row.name == "Driver 12" { 'W' } else { 'D' },
                    wet: row.name == "Driver 12",
                }),
                // A gainer, a loser, and a steady field, so the change
                // column shows all three faces.
                race_position_change: Some(match row.name {
                    "Driver 6" => 2,
                    "Driver 10" => -3,
                    "Driver 11" => 1,
                    _ => 0,
                }),
            });
            car_idx += 1;
        }
    }
    entries
}

/// One row of the Relative mockup.
struct Nearby {
    position: i32,
    name: &'static str,
    /// The car's display name, whose first word resolves the brand mark.
    car: &'static str,
    /// The number on the car.
    number: &'static str,
    class: (i32, &'static str, &'static str),
    license_color: &'static str,
    irating: i32,
    irating_change: f32,
    gap: f32,
    lap_diff: i32,
    recent_lap: f32,
    location: TrackLocation,
    fastest: bool,
    penalty: Option<Penalty>,
}

#[expect(clippy::too_many_lines, reason = "see `standings`: a verbatim transcription of the mockup's rows")]
fn relative() -> Vec<CarSnapshot> {
    let rows = [
        Nearby {
            position: 10,
            name: "Driver 9",
            car: "Porsche 911 GT3 R",
            number: "31",
            class: GT3,
            license_color: LIC_B,
            irating: 2500,
            irating_change: -14.0,
            gap: 12.0,
            lap_diff: 0,
            recent_lap: 104.2,
            location: TrackLocation::OnTrack,
            fastest: false,
            penalty: Some(Penalty::Slowdown),
        },
        Nearby {
            position: 1,
            name: "Driver 1",
            car: "Dallara P217",
            number: "8",
            class: DP,
            license_color: LIC_A,
            irating: 1800,
            irating_change: 1.0,
            gap: 7.4,
            lap_diff: 1,
            recent_lap: 98.7,
            location: TrackLocation::OnTrack,
            fastest: true,
            penalty: None,
        },
        Nearby {
            position: 11,
            name: "Driver 10",
            car: "Lamborghini Huracan GT3 EVO",
            number: "117",
            class: GT3,
            license_color: LIC_B,
            irating: 1900,
            irating_change: -11.0,
            gap: 2.7,
            lap_diff: 0,
            recent_lap: 104.3,
            location: TrackLocation::InPitStall,
            fastest: false,
            penalty: None,
        },
        Nearby {
            position: 12,
            name: "Driver 11",
            car: "Mercedes-AMG GT3 2020",
            number: "42",
            class: GT3,
            license_color: LIC_B,
            irating: 2200,
            irating_change: -30.0,
            gap: 0.0,
            lap_diff: 0,
            recent_lap: 105.0,
            location: TrackLocation::OffTrack,
            fastest: false,
            penalty: None,
        },
        Nearby {
            position: 1,
            name: "Driver 2",
            car: "Porsche 911 RSR",
            number: "5",
            class: GTE,
            license_color: LIC_A,
            irating: 8000,
            irating_change: 41.0,
            gap: -1.8,
            lap_diff: 1,
            recent_lap: 98.9,
            location: TrackLocation::OnTrack,
            fastest: true,
            penalty: None,
        },
        Nearby {
            position: 13,
            name: "Driver 12",
            car: "Aston Martin Vantage GT3",
            number: "66",
            class: GT3,
            license_color: LIC_B,
            irating: 1700,
            irating_change: -31.0,
            gap: -4.8,
            lap_diff: 0,
            recent_lap: 104.8,
            location: TrackLocation::OnTrack,
            fastest: false,
            penalty: Some(Penalty::BlackFlag),
        },
        Nearby {
            position: 14,
            name: "Driver 13",
            car: "Ford Mustang GT3",
            number: "23",
            class: GT3,
            license_color: LIC_A,
            irating: 1900,
            irating_change: -48.0,
            gap: -6.3,
            lap_diff: 0,
            recent_lap: 104.7,
            location: TrackLocation::OnTrack,
            fastest: false,
            penalty: None,
        },
    ];

    rows.into_iter()
        .enumerate()
        .map(|(i, row)| CarSnapshot {
            car_idx: i32::try_from(i).unwrap_or(0),
            cust_id: Some(1000 + u32::try_from(i).unwrap_or(0)),
            position: row.position,
            track_location: row.location,
            gap_to_player_secs: row.gap,
            driver_name: Arc::from(row.name),
            car_number: Arc::from(row.number),
            car_screen_name: Arc::from(row.car),
            irating: row.irating,
            flair_id: demo_flair(i),
            license_color: Arc::from(row.license_color),
            car_class_color: Arc::from(row.class.2),
            is_fastest_overall: row.fastest,
            irating_change_estimate: Some(row.irating_change),
            is_focus: row.name == "Driver 11",
            off_tracks: if row.name == "Driver 12" { 2 } else { 0 },
            lap_diff: row.lap_diff,
            best_recent_lap_secs: Some(row.recent_lap),
            recent_laps: [Some(row.recent_lap + 0.4), Some(row.recent_lap), Some(row.recent_lap + 0.2)],
            penalty: row.penalty,
        })
        .collect()
}

/// Applies a named variation to the demo snapshot — `--demo-state=<name>`.
///
/// Demo mode renders one fixed snapshot, which is the right default for
/// holding a widget up against its mockup but cannot show any of the states
/// that only exist part-way through a race: a caution, a box call, a marked
/// driver, a spectator's seat. Each name here nudges the snapshot into one of
/// those, so every state has a reproducible screenshot behind it. Unknown
/// names are reported and ignored rather than failing the run.
///
/// See `docs/features/shoot.ps1`, which drives these to build the feature
/// page's images.
pub fn apply_state(snapshot: &mut TelemetrySnapshot, state: &str) {
    use crate::telemetry::snapshot::{CourseFlag, GridStatus};

    match state {
        // --- The black box's status border ---
        "caution" => snapshot.course_flag = CourseFlag::Yellow,
        "lastlap" => snapshot.course_flag = CourseFlag::White,
        "finish" => snapshot.course_flag = CourseFlag::Checkered,
        // A steady box call: past half distance on the lap, out of fuel range.
        "box" => {
            snapshot.box_this_lap = true;
            snapshot.pit_service.fuel_level_litres = 4.0;
            snapshot.fuel_use.lap_fraction = Some(0.72);
        }
        // The same call inside the approach distance, where it pulses.
        "boxnear" => {
            snapshot.box_this_lap = true;
            snapshot.pit_service.fuel_level_litres = 4.0;
            snapshot.metres_to_pit = Some(180.0);
        }

        // --- Seats and session state ---
        "practice" => {
            snapshot.relative_meta.session_kind = SessionKind::Practice;
            snapshot.endurance = EnduranceMeta::default();
            for entry in &mut snapshot.standings {
                entry.stops_remaining = None;
                entry.projected_class_position = None;
                entry.race_position_change = None;
            }
        }
        "spectating" => {
            snapshot.seat = Seat::Spectating(Arc::from("Alex Holder"));
            snapshot.relative_meta.spectating = Some(Arc::from("Alex Holder"));
        }
        "teammate" => {
            snapshot.seat = Seat::TeamMate(Arc::from("Ben Whitfield"));
            snapshot.relative_meta.team_mate = Some(Arc::from("Ben Whitfield"));
        }
        "grid" => {
            snapshot.relative_meta.grid =
                Some(GridStatus { cars_gridded: 27, car_count: 36, countdown_secs: Some(92.0) });
        }
        "pitroad" | "inbox" => {
            snapshot.pit_service.on_pit_road = true;
        }

        // --- Pit service ---
        "autofuel" => snapshot.pit_service.refuel_target_litres = Some(72.0),
        "nofuel" => snapshot.pit_service.fuel_armed = false,
        "fastrepair" => {
            snapshot.pit_service.fast_repairs_available = 2;
            snapshot.pit_service.fast_repair_armed = true;
        }
        "notyres" => snapshot.pit_service.tyres_armed = [false; 4],
        // A stint's worth of wear on the left side, so the Tires page's bars
        // have something to say beyond "all fresh".
        "worntyres" => {
            for (corner, wear) in snapshot.tyres.corners.iter_mut().zip([0.62_f32, 0.88, 0.55, 0.84]) {
                corner.wear = [wear - 0.06, wear, wear + 0.03];
            }
        }

        // --- Strategy ---
        // A tank that cannot reach the flag, so the pit window has stops to
        // place and the skip hint has something to search.
        "needsstops" => {
            snapshot.endurance.laps_remaining = Some(58);
            snapshot.endurance.stops_remaining = Some(2);
            snapshot.pit_service.fuel_level_litres = 46.0;
            snapshot.pit_service.fuel_amount_litres = 46.0;
        }
        "raining" => {
            snapshot.weather.precip_now = Some(0.78);
            snapshot.weather.precip_chance = Some(0.9);
            snapshot.weather.track_wetness = Some(TrackWetness::VeryWet);
        }
        "dry" => {
            snapshot.weather.precip_now = None;
            snapshot.weather.precip_chance = Some(0.04);
            snapshot.weather.declared_wet = false;
            snapshot.weather.on_wet_tyres = false;
            snapshot.weather.track_wetness = Some(TrackWetness::Dry);
        }
        unknown => println!("note: --demo-state={unknown} is not a state; ignoring it"),
    }
}

/// The team-sync ledger a `--demo-state=sync` run starts with.
///
/// A team-mate a dozen laps into a stint, with the crew's two standing calls
/// already made: a fuel target to hold and a directive to double-stint the
/// tyres. Folded straight into the store by `sync::runtime::TeamSync::demo_seed`,
/// which is what unlocks the spectator's Fuel, Tyres and Strategy pages
/// without a relay.
#[must_use]
pub fn sync_events() -> Vec<(f64, crate::sync::protocol::Event)> {
    use crate::sync::protocol::{Event, TyrePolicy};

    let mut events = vec![(0.0, Event::StintBoundary { driver: "Alex Holder".to_owned() })];
    // A dozen closed laps, so the spectator's burn average has a window to
    // read and the store's lap history is populated.
    for lap in 8_u16..=20 {
        let fuel = 78.0 - f32::from(lap - 8) * 2.71;
        events.push((f64::from(lap) * 96.0, Event::LapClosed { lap, fuel_litres: fuel, used_litres: 2.71 }));
    }
    events.push((2020.0, Event::TyreReadings(tyres())));
    events.push((
        2021.0,
        Event::DriverScalars {
            fuel_litres: 43.8,
            service_fuel_litres: Some(64),
            tyres_armed: [true, true, false, false],
            tyre_pressures_kpa: [159.0, 159.0, 158.0, 158.0],
        },
    ));
    events.push((2022.0, Event::FuelTarget { requester: "Flynn".to_owned(), litres_per_lap: Some(2.62) }));
    events.push((
        2023.0,
        Event::TyrePolicySet {
            requester: "Flynn".to_owned(),
            policy: Some(TyrePolicy::BelowWear { threshold_pct: TyrePolicy::DEFAULT_WEAR_THRESHOLD_PCT }),
        },
    ));
    events
}
