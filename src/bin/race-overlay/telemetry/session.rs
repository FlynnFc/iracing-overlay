// Rust guideline compliant 2026-02-16

//! Owns the `iracing-telem` connection lifecycle on a background thread and
//! turns each telemetry tick into an owned [`TelemetrySnapshot`].
//!
//! `Client` and `Session` hold an internal `Rc`, so they are `!Send`: both
//! must be constructed and used on the same thread that calls [`run`], never
//! moved from or shared with the UI thread.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use iracing_telem::flags::{BroadcastMsg, CarLeftRight, Flags, SessionState, TrackLocation};
use iracing_telem::{Client, DataUpdateResult, Session, Value, Var};

use super::session_info::{
    ResultsPosition, SessionInfoYaml, SessionResults, parse_incident_limit, parse_percent, parse_session_laps,
    parse_session_seconds, parse_track_length,
};
use super::snapshot::{Approaching, EnduranceMeta, FasterClassSnapshot, Seat, SessionKind, is_lone_qualifying};
use super::snapshot::{
    CarAdjustments, CarSnapshot, ClassSection, GridStatus, PitProjection, RadarCar, RadarSide, RadarSnapshot,
    RelativeMeta, StandingsEntry, TelemetrySnapshot, TrackWetness, TyreInfo, TyreState, WeatherSnapshot,
    track_location_from_raw,
};
use super::{endurance, faster_class, irating, pit, pit_model, radar, relative, sof, standings, weather};

/// How long to wait for iRacing to appear before checking again.
const SESSION_WAIT: Duration = Duration::from_secs(5);

/// Slightly above the 60 Hz telemetry rate so updates are never missed.
const DATA_WAIT: Duration = Duration::from_millis(500);

/// How long to pause between one session ending and looking for the next.
///
/// Only a floor on how often a connection may be re-established; see the loop
/// in [`run`] for why one is needed at all.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(1);

/// iRacing reports `SessionTimeRemain` as exactly one week (604800s) for
/// sessions with no fixed time limit (practice, and some qualify/time trial
/// setups) — a sentinel, not a real countdown. Treating it as a real value
/// produced a "RACE 0:58 / 10080:58" footer and wildly wrong
/// predicted-final-lap estimates; anything within a few seconds of a week
/// is that sentinel, since a real timed session is never scheduled that long.
const UNLIMITED_SESSION_SENTINEL_SECS: f64 = 604_800.0;

/// iRacing reports `SessionLapsRemainEx` as `i16::MAX` in a session with no
/// lap limit — every timed race — the way `SessionTimeRemain` reports a week:
/// a sentinel, not a count. Anything at or past it is that sentinel.
const UNLIMITED_LAPS_SENTINEL: i32 = 32_767;

/// The longest a grid countdown is taken to be, in seconds.
///
/// The SDK has no clock dedicated to the grid. What it has is
/// `SessionTimeRemain`, which in a timed race counts the grid down before it
/// starts counting the race — so the same variable is a grid clock in one
/// state and a race clock in the next, and this is how the two are told
/// apart on top of `SessionState`. iRacing grids close inside a few minutes;
/// no race is shorter than ten, so a reading above this while the field is
/// still forming up is not the grid's.
const GRID_COUNTDOWN_MAX_SECS: f64 = 600.0;

/// How far around the lap the player must be before their own progress can be
/// used to estimate the lap's length.
///
/// The estimate divides time-from-the-line by fraction-of-lap-covered; a few
/// metres past the line that is a tiny number over a tiny number, and the
/// result swings wildly. A twentieth of a lap is far enough in to settle.
const LAP_ESTIMATE_MIN_PCT: f32 = 0.05;

/// How much slower than the player's best lap their rolling pace may read
/// before it stops being taken as pace.
///
/// Relative gaps are the player's lap fraction turned back into seconds, so
/// whatever this resolves to scales every gap on the panel at once. Race pace
/// sits a few percent off a best lap; a caution lap, an in-lap or a spin sits
/// tens of percent off it, and taking one of those at face value would inflate
/// the whole column. Fifteen percent is comfortably above the first and well
/// below the second.
const PACE_OVER_BEST_LIMIT: f32 = 1.15;

/// How full the tank must be before its capacity can be inferred from
/// `FuelLevel / FuelLevelPct`. Below a tenth, both numbers are small enough
/// that their ratio swings by tens of litres between ticks.
const TANK_ESTIMATE_MIN_PCT: f32 = 0.10;

/// Tuning the background telemetry loop needs from `OverlayConfig`, grouped
/// so `run`/`run_session`/`build_snapshot` don't grow an ever-longer
/// positional parameter list as more widgets are added.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotTuning {
    /// Assumed pit-lane time loss used to project a post-pit position.
    pub pit_loss_secs: f32,
    /// How close (in seconds of relative time) a car must be to register on
    /// the Radar Bars widget.
    pub radar_range_secs: f32,
}

/// Runs forever: connects to iRacing, streams snapshots to `tx`, and
/// reconnects automatically whenever iRacing was not yet running or
/// restarts mid-session.
///
/// `on_update` is called after every snapshot is sent, so the caller can
/// wake a paint loop (e.g. `egui::Context::request_repaint`).
pub fn run(
    tx: &Sender<TelemetrySnapshot>,
    requests: &std::sync::mpsc::Receiver<pit::PitRequest>,
    tuning: SnapshotTuning,
    on_update: impl Fn(),
) {
    // This thread builds a snapshot on every one of iRacing's ticks; keep it
    // off the sim's cores — see `crate::perf`.
    crate::perf::mark_background_thread();
    let mut client = Client::new();
    loop {
        // SAFETY: iracing-telem's documented contract: `Client`/`Session`
        // read Windows shared memory whose layout is defined by the iRacing
        // SDK, not verified by the type system. Used only on this thread,
        // matching the crate's `!Send` design.
        let Some(mut session) = (unsafe { client.wait_for_session(SESSION_WAIT) }) else {
            continue;
        };
        run_session(&mut session, tx, requests, tuning, &on_update);
        // A session that ends immediately must not become a hot loop. It can:
        // `run_session` returns straight away when the session is missing a
        // variable this widget needs, and `wait_for_session` returns straight
        // back with the same session — which spun this thread at full tilt,
        // reprobing every candidate variable and printing a note each time, for
        // as long as iRacing stayed in that state. A session genuinely starting
        // is a once-a-race event, so a second's pause costs nothing.
        std::thread::sleep(RECONNECT_BACKOFF);
    }
}

/// Connects to iRacing, waits up to `wait` for a session, and returns the
/// raw `session_info()` YAML once. Used by `--dump-session-info` to capture
/// a real sample for confirming YAML field names against actual data.
///
/// # Errors
/// Returns an error if iRacing doesn't become available within `wait`.
pub fn dump_session_info(wait: Duration) -> anyhow::Result<String> {
    let mut client = Client::new();
    // SAFETY: see `run`.
    let Some(session) = (unsafe { client.wait_for_session(wait) }) else {
        anyhow::bail!(
            "iRacing did not become available within {wait:?}; make sure it's running and you're in a session"
        );
    };
    // SAFETY: see `run`.
    Ok(unsafe { session.session_info() })
}

/// Prints every variable the live session publishes, whatever its name.
///
/// [`dump_vars`] answers "does this session have the variables I expected?";
/// this answers "what has it got at all?", which is the question worth asking
/// when something expected turns out to be absent and the next move depends on
/// whether an alternative exists. `iracing-telem`'s own `Session::dump_vars`
/// walks the shared memory's variable headers, so nothing here is guessed.
///
/// # Errors
/// Returns an error if iRacing doesn't become available within `wait`.
pub fn dump_all_vars(wait: Duration) -> anyhow::Result<()> {
    let mut client = Client::new();
    // SAFETY: see `run`.
    let Some(mut session) = (unsafe { client.wait_for_session(wait) }) else {
        anyhow::bail!(
            "iRacing did not become available within {wait:?}; make sure it's running and you're in a session"
        );
    };
    // A variable's header exists as soon as the session does, but its value is
    // whatever was last written to the shared memory, so wait for one real tick.
    // SAFETY: see `run`.
    unsafe { session.wait_for_data(DATA_WAIT) };
    // SAFETY: see `run`; this only walks the header list and reads each value.
    unsafe { session.dump_vars() };
    Ok(())
}

/// Every telemetry variable the black box's pages could want, probed by name.
///
/// A candidate list rather than an enumeration because this reports on the
/// names the widgets actually ask for, including the ones that turn out to be
/// absent — which is the answer a page needs when deciding whether to draw a
/// row. See [`dump_all_vars`] for the full list of what a session carries.
/// Which is what the pages need anyway: [`Session::find_var`] returning `None`
/// is exactly how a row decides to hide itself on a car without that control.
///
/// Grouped by the page each belongs to, and deliberately over-inclusive —
/// names that turn out not to exist simply report as absent, which is the
/// answer [`dump_vars`] is being asked for.
const CANDIDATE_VARS: &[(&str, &[&str])] = &[
    (
        "fuel service",
        &["FuelLevel", "FuelLevelPct", "FuelUsePerHour", "dpFuelFill", "dpFuelAddKg", "PitSvFuel", "PitSvFlags"],
    ),
    (
        "tyre service",
        &[
            "dpLFTireChange",
            "dpRFTireChange",
            "dpLRTireChange",
            "dpRRTireChange",
            "dpLFTireColdPress",
            "dpRFTireColdPress",
            "dpLRTireColdPress",
            "dpRRTireColdPress",
            "PitSvLFP",
            "PitSvRFP",
            "PitSvLRP",
            "PitSvRRP",
            "dpTireCompound",
            "PitSvTireCompound",
        ],
    ),
    (
        "other pit service",
        &[
            "dpWindshieldTearoff",
            "dpFastRepair",
            "FastRepairAvailable",
            "FastRepairUsed",
            "PitRepairLeft",
            "PitOptRepairLeft",
        ],
    ),
    (
        "tyre temps (live carcass)",
        &[
            "LFtempCL",
            "LFtempCM",
            "LFtempCR",
            "RFtempCL",
            "RFtempCM",
            "RFtempCR",
            "LRtempCL",
            "LRtempCM",
            "LRtempCR",
            "RRtempCL",
            "RRtempCM",
            "RRtempCR",
            "LFtempCA",
            "RFtempCA",
            "LRtempCA",
            "RRtempCA",
            "LFpressure",
            "RFpressure",
            "LRpressure",
            "RRpressure",
        ],
    ),
    (
        "tyre info (last stop)",
        &[
            "LFtempL",
            "LFtempM",
            "LFtempR",
            "RFtempL",
            "RFtempM",
            "RFtempR",
            "LRtempL",
            "LRtempM",
            "LRtempR",
            "RRtempL",
            "RRtempM",
            "RRtempR",
            "LFwearL",
            "LFwearM",
            "LFwearR",
            "RFwearL",
            "RFwearM",
            "RFwearR",
            "LRwearL",
            "LRwearM",
            "LRwearR",
            "RRwearL",
            "RRwearM",
            "RRwearR",
            "LFcoldPressure",
            "RFcoldPressure",
            "LRcoldPressure",
            "RRcoldPressure",
        ],
    ),
    (
        "in-car adjustments",
        &[
            "dcBrakeBias",
            "dcABS",
            "dcTractionControl",
            "dcTractionControl2",
            "dcThrottleShape",
            "dcDashPage",
            "dcAntiRollFront",
            "dcAntiRollRear",
        ],
    ),
    (
        "pit-stop adjustments",
        &[
            "dpRearWing",
            "dpWingFront",
            "dpWingRear",
            "dpQtape",
            "dpLrWedgeAdj",
            "dpRrWedgeAdj",
            "dcAntiRollBarFront",
            "dcAntiRollBarRear",
            "dcFrontARB",
            "dcRearARB",
            "dcARBFront",
            "dcARBRear",
            "dcFrontWing",
            "dcRearWing",
            "dcWingSetting",
            "dpFNOMKnobSetting",
            "dpRRDamperPerchOffsetm",
        ],
    ),
    (
        "weather",
        &[
            "AirTemp",
            "TrackTempCrew",
            "FogLevel",
            "Precipitation",
            "TrackWetness",
            "WindVel",
            "WindDir",
            "RelativeHumidity",
            "WeatherDeclaredWet",
            "SolarAltitude",
            "SessionTimeOfDay",
        ],
    ),
    ("state", &["IsOnTrack", "IsInGarage", "PlayerCarInPitStall", "OnPitRoad", "PlayerTrackSurface", "SessionFlags"]),
    ("tyres", &["PlayerTireCompound"]),
    // Which player scalars survive climbing out of the car in a team session.
    // Run from the spotter seat while a team-mate drives, then again from the
    // car; the two dumps decide every open question in
    // `plans/team-endurance.md`.
    (
        "team (dump from the spotter seat)",
        &[
            "IsOnTrackCar",
            "PlayerCarIdx",
            "CamCarIdx",
            "LapLastLapTime",
            "LapBestLapTime",
            "Speed",
            "PlayerCarMyIncidentCount",
            "PlayerCarTeamIncidentCount",
            "PlayerCarDriverIncidentCount",
            "PlayerCarTowTime",
            "CarIdxLastLapTime",
            "CarIdxBestLapTime",
        ],
    ),
];

/// Reports which of [`CANDIDATE_VARS`] this session publishes, with each one's
/// type, unit, description and current value.
///
/// A one-off diagnostic behind `--dump-vars`: run it on track in the car whose
/// black box is being built, so the pages are written against the variables
/// that car actually has rather than against recollection of the SDK.
///
/// # Errors
/// Returns an error if iRacing doesn't become available within `wait`.
pub fn dump_vars(wait: Duration) -> anyhow::Result<String> {
    let mut client = Client::new();
    // SAFETY: see `run`.
    let Some(mut session) = (unsafe { client.wait_for_session(wait) }) else {
        anyhow::bail!(
            "iRacing did not become available within {wait:?}; make sure it's running and you're in a session"
        );
    };
    // A variable's header exists as soon as the session does, but its value is
    // whatever was last written to the shared memory — so wait for one real
    // tick before reading, or every number below reads zero.
    // SAFETY: see `run`.
    unsafe { session.wait_for_data(DATA_WAIT) };

    // The player's own slot in every per-car array, so the dump shows their
    // car's entry as well as the first few. Read from the YAML because the
    // `PlayerCarIdx` variable is itself one of the things being checked.
    // SAFETY: see `run`.
    let yaml = unsafe { session.session_info() };
    let player_idx =
        SessionInfoYaml::parse(&yaml).ok().and_then(|info| usize::try_from(info.driver_info.driver_car_idx).ok());

    let mut lines: Vec<String> = Vec::new();
    lines.push(match player_idx {
        Some(idx) => format!("player car slot: {idx}"),
        None => "player car slot: unknown (session info did not parse)".to_owned(),
    });
    let mut present = 0_usize;
    let mut missing = 0_usize;
    for (group, names) in CANDIDATE_VARS {
        lines.push(format!("\n== {group} =="));
        for name in *names {
            // SAFETY: see `run`; `session` is live and this reads only the
            // variable header list.
            let found = unsafe { session.find_var(name) };
            if let Some(var) = found {
                present += 1;
                // SAFETY: `var` was just looked up against this `session`.
                let value = unsafe { describe_value(&session, &var, player_idx) };
                let unit = var.unit();
                let unit = if unit.is_empty() { String::new() } else { format!(" [{unit}]") };
                lines.push(format!("{name:<24} = {value:<28}{unit}  {}", var.desc()));
            } else {
                missing += 1;
                lines.push(format!("{name:<24}   (absent)"));
            }
        }
    }
    Ok(format!("{present} variables present, {missing} absent\n{}\n", lines.join("\n")))
}

/// Renders one variable's current value, whatever its type and arity.
///
/// Per-car arrays also show the entry at `player_idx`, the player's own
/// slot, since that is the one worth comparing against the player scalars.
///
/// # Safety
/// `session` must be live and `var` must have been looked up against it.
unsafe fn describe_value(session: &Session, var: &Var, player_idx: Option<usize>) -> String {
    use iracing_telem::VarType;

    // Arrays here are the per-car ones (`CarIdx*`); a handful of entries shows
    // the shape without pages of zeros for empty car slots.
    const ARRAY_SAMPLE: usize = 4;

    fn sample<T: std::fmt::Debug>(values: &[T], player_idx: Option<usize>) -> String {
        let head = &values[..values.len().min(ARRAY_SAMPLE)];
        let mine =
            player_idx.and_then(|idx| values.get(idx).map(|value| format!(" me[{idx}]={value:?}"))).unwrap_or_default();
        format!("{head:?}..[{}]{mine}", values.len())
    }

    // SAFETY: forwarding this function's own contract — `session` is live and
    // `var` was looked up against it — for every read in this match.
    unsafe {
        match (var.var_type(), var.count() == 1) {
            (VarType::Bool, true) => session.value::<bool>(var).map_or_else(|e| format!("<{e:?}>"), |v| v.to_string()),
            (VarType::Int | VarType::Bitfield, true) => {
                session.value::<i32>(var).map_or_else(|e| format!("<{e:?}>"), |v| v.to_string())
            }
            (VarType::Float, true) => {
                session.value::<f32>(var).map_or_else(|e| format!("<{e:?}>"), |v| format!("{v:.3}"))
            }
            (VarType::Double, true) => {
                session.value::<f64>(var).map_or_else(|e| format!("<{e:?}>"), |v| format!("{v:.3}"))
            }
            (VarType::Float, false) => {
                session.value::<&[f32]>(var).map_or_else(|e| format!("<{e:?}>"), |v| sample(v, player_idx))
            }
            (VarType::Int | VarType::Bitfield, false) => {
                session.value::<&[i32]>(var).map_or_else(|e| format!("<{e:?}>"), |v| sample(v, player_idx))
            }
            (kind, _) => format!("<{kind:?} x{}>", var.count()),
        }
    }
}

/// Polls one iRacing session until it expires (iRacing closed or restarted).
fn run_session(
    session: &mut Session,
    tx: &Sender<TelemetrySnapshot>,
    requests: &std::sync::mpsc::Receiver<pit::PitRequest>,
    tuning: SnapshotTuning,
    on_update: &impl Fn(),
) {
    // SAFETY: see `run`.
    let Some(vars) = (unsafe { TelemetryVars::find_all(session) }) else {
        println!("note: iRacing session is missing telemetry vars this widget needs; waiting for the next session");
        return;
    };
    let mut info_cache = SessionInfoCache::default();
    let mut trackers = SessionTrackers::default();

    loop {
        // SAFETY: see `run`.
        match unsafe { session.wait_for_data(DATA_WAIT) } {
            DataUpdateResult::SessionExpired => return,
            DataUpdateResult::Updated => {
                // SAFETY: see `run`.
                unsafe { info_cache.refresh(session) };
                // SAFETY: `vars` were looked up against this exact `session`.
                let snapshot = unsafe { build_snapshot(session, &vars, &info_cache, &mut trackers, tuning) };
                let service = snapshot.pit_service;
                if tx.send(snapshot).is_err() {
                    return; // UI thread is gone; stop polling
                }
                // Pit commands are sent from this thread because `Session` is
                // `!Send` — it cannot be handed to the UI. The UI queues
                // intents and they are carried out here, against the state
                // this same tick just read.
                // SAFETY: `session` is live and connected.
                unsafe { send_pit_requests(session, requests, &service) };
                on_update();
            }
            DataUpdateResult::NoUpdate | DataUpdateResult::FailedToCopyRow => {}
        }
    }
}

/// Drains the UI's queued pit intents and broadcasts them to iRacing.
///
/// Each intent may expand into several commands — unticking one tyre means
/// clearing all four and re-arming the rest; see [`pit::commands_for`]. A
/// failed send is reported once and then ignored: iRacing accepts these only
/// in certain states, and a driver mid-stint does not need a message every
/// frame telling them so.
///
/// # Safety
/// `session` must be a live, currently connected `Session`.
unsafe fn send_pit_requests(
    session: &Session,
    requests: &std::sync::mpsc::Receiver<pit::PitRequest>,
    service: &pit::PitService,
) {
    while let Ok(request) = requests.try_recv() {
        for command in pit::commands_for(request, service) {
            // SAFETY: forwarding this function's contract; `broadcast_msg`
            // only posts a Windows message and touches no shared memory.
            if let Err(err) = unsafe { session.broadcast_msg(BroadcastMsg::PitCommand(command)) } {
                println!("note: iRacing did not accept a pit command ({err:?})");
            }
        }
    }
}

/// The mutable state `build_snapshot` carries across ticks, grouped so its
/// parameter list doesn't grow one entry per feature.
#[derive(Debug, Default)]
struct SessionTrackers {
    radar_smoothing: RadarSmoothing,
    /// How fast each quicker-class car behind is closing, for the Faster
    /// Class widget. A few seconds of history per car, so it is simply
    /// dropped with the rest on a change of session.
    faster_class_rates: faster_class::ClosingRates,
    stint: StintTracker,
    /// The player's own stops, counted from their own car's telemetry rather
    /// than the field-wide inference — see [`PlayerStops`].
    player_stops: PlayerStops,
    /// How often each car has left the track — see [`OffTrackCounter`].
    off_tracks: OffTrackCounter,
    /// Each car's last real lap number — see [`LapLatch`].
    laps: LapLatch,
    /// The previous tick's distance-based running order — see [`RaceOrder`].
    race_order: RaceOrder,
    /// When each absent car left the world — see [`TowTracker`].
    tow: TowTracker,
    /// Where each car's race — and current lap — began, for the
    /// position-change markers; see [`PositionChangeTracker`].
    position_change: PositionChangeTracker,
    /// The tank's size, once it has been seen — see [`TankCapacity`].
    tank_capacity: TankCapacity,
    recent_laps: RecentLapsTracker,
    /// Latched for the session — see `EnduranceMeta::multi_stop_race`.
    multi_stop_race: bool,
    stop_pressures: StopPressures,
    fuel: FuelTracker,
    /// Whether a fuel rig is pumping this visit to the lane, and the level it
    /// will stop at — see [`pit::RefuelWatch`].
    refuel: pit::RefuelWatch,
    /// This lap's fuel burn against the clock, for the Strategy page.
    lap_fuel_use: LapFuelUse,
    /// What the pit lane costs here, measured from every visit to it.
    pit_loss: pit_model::PitLossTracker,
    /// The player's rolling pace, which every lap and stop projection divides
    /// by.
    player_pace: LapPace,
    /// The last tick's Relative row order, so rows hold still between ticks.
    row_order: relative::RowOrder,
    /// The player's own time-around-the-lap curve, which every relative gap
    /// is read off.
    lap_curve: relative::LapCurve,
    /// The measured lap already announced for `lap_curve`, so the note is
    /// printed once per improvement rather than once a tick.
    curve_reported_secs: Option<f64>,
    /// The reference lap iRacing's own est-times are projected against, which
    /// `lap_curve`'s fractions are turned back into seconds by.
    reference_lap: relative::ReferenceLap,
    /// Whether the note for `reference_lap` has been printed, so it is said
    /// once on recovery rather than once a tick.
    reference_reported: bool,
    /// Which car `lap_curve` and `reference_lap` were measured from, and that
    /// car's class; see [`SessionTrackers::sync_to_focus`]. `None` until the
    /// first tick of a connection.
    focus_car_idx: Option<i32>,
    /// See `focus_car_idx`. `None` where the focus car has no driver entry and
    /// its class is therefore unknown.
    focus_class_id: Option<i32>,
    /// Field strength per class, recomputed only when a class's ratings move.
    sof: SofCache,
    /// The field's iRating change estimates; see [`IratingCache`].
    irating: IratingCache,
    /// Which `SessionNum` everything above describes; see
    /// [`SessionTrackers::sync_to_session`].
    session_num: Option<i32>,
}

/// Remembers each class's field strength so it is computed only when it moves.
///
/// [`sof::estimate_sof`] bisects forty times over the whole class, calling
/// `exp` twice per driver per step — about 4,800 `exp` calls for a full class,
/// and it is asked once for the focus car's class plus once per class in
/// [`build_class_sections`]. Run on every telemetry tick that was over a
/// million `exp` calls a second in a multi-class field.
///
/// What it computes is a function of one thing: the list of iRatings in the
/// class. Those come from the session-info YAML, which changes when a driver
/// joins or leaves and not otherwise — a few times an hour against sixty times
/// a second. So the inputs are kept alongside the answer and compared, which
/// costs a slice comparison of a few dozen `i32`s and is exact: there is no
/// staleness window, the answer is recomputed the moment its inputs differ.
#[derive(Debug, Default)]
struct SofCache {
    /// One entry per class seen, as `(class id, the ratings it was computed
    /// from, the answer)`. A `Vec` rather than a map because a session has a
    /// handful of classes and scanning them beats hashing.
    entries: Vec<(i32, Vec<i32>, Option<i32>)>,
}

impl SofCache {
    /// This class's field strength, recomputing only if `iratings` has changed.
    fn sof(&mut self, class_id: i32, iratings: &[i32]) -> Option<i32> {
        if let Some((_, known, answer)) = self.entries.iter().find(|(id, _, _)| *id == class_id)
            && known == iratings
        {
            return *answer;
        }
        let answer = sof::estimate_sof(iratings);
        match self.entries.iter_mut().find(|(id, _, _)| *id == class_id) {
            Some(entry) => {
                entry.1.clear();
                entry.1.extend_from_slice(iratings);
                entry.2 = answer;
            }
            None => self.entries.push((class_id, iratings.to_vec(), answer)),
        }
        answer
    }
}

/// How long an iRating estimate may stand before the field is re-examined.
///
/// Unlike [`SofCache`], this one's inputs include finishing order, which in a
/// busy race genuinely does move most ticks — so input comparison alone would
/// not bound the work. [`irating::estimate_changes`] is O(n²) with two `exp`
/// calls per pair, about 7,200 of them for a sixty-car field.
///
/// A second's delay is invisible on a figure the UI already labels an estimate
/// of a session that has not finished, and it turns a per-tick cost into a
/// per-second one.
const IRATING_REFRESH_SECS: f64 = 1.0;

/// Remembers the field's iRating change estimates between recomputes.
///
/// Recomputed when the field's positions or ratings have actually changed *and*
/// [`IRATING_REFRESH_SECS`] has passed since the last one — see there for why
/// both conditions are wanted.
#[derive(Debug, Default)]
struct IratingCache {
    /// What the last estimate was computed from, to notice when it moves:
    /// each entry's `car_idx`, its class, and its result.
    inputs: Vec<(i32, i32, irating::RaceResult)>,
    /// This tick's field, built here so the comparison against `inputs`
    /// allocates nothing on the ticks — nearly all of them — where it matches.
    scratch: Vec<(i32, i32, irating::RaceResult)>,
    /// One class's bare results, handed to the estimator a class at a time.
    results: Vec<irating::RaceResult>,
    /// That estimate, keyed by `car_idx` as the callers look it up.
    changes: HashMap<i32, f32>,
    /// The session clock when it was computed; `None` before the first one.
    computed_at_secs: Option<f64>,
}

impl IratingCache {
    /// The field's estimated iRating changes, keyed by `car_idx`.
    ///
    /// Empty outside a race: there is no result to rate, and rating a practice
    /// session's cars as they arrive gave every one of them a four-figure
    /// change.
    fn changes(
        &mut self,
        standings: &[StandingsEntry],
        drivers: &HashMap<i32, DriverMeta>,
        racing_under_way: bool,
        is_race: bool,
        session_time_secs: f64,
    ) -> &HashMap<i32, f32> {
        if !is_race {
            self.inputs.clear();
            self.changes.clear();
            return &self.changes;
        }
        let due = self.computed_at_secs.is_none_or(|at| (session_time_secs - at).abs() >= IRATING_REFRESH_SECS);
        // Cheap test first: an unchanged field needs nothing however long it
        // has been, and that is the whole of practice and every green-flag
        // stint where nobody is passing.
        field_results(standings, drivers, racing_under_way, &mut self.scratch);
        if self.scratch == self.inputs || !due {
            return &self.changes;
        }

        std::mem::swap(&mut self.inputs, &mut self.scratch);
        self.changes.clear();
        // One pool per class. iRacing rates a multi-class race as several
        // separate races, so an LMP2 driver's rating never turns on how the
        // GT3s finished — pooled together, beating a slower class's whole
        // field read as beating the session, and every class but the quickest
        // was rated as if it had lost to cars it was never racing.
        //
        // It is also cheaper: `estimate_changes` is O(n²), and the squares of
        // the parts are always less than the square of the whole.
        for class_id in class_ids_of(&self.inputs) {
            self.results.clear();
            self.results
                .extend(self.inputs.iter().filter(|(_, class, _)| *class == class_id).map(|(_, _, result)| *result));
            // A class with nobody else in it has no one to be rated against,
            // and a one-car duel is a number with no meaning behind it.
            if self.results.len() < 2 {
                continue;
            }
            let changes = irating::estimate_changes(&self.results);
            let rated = self.inputs.iter().filter(|(_, class, _)| *class == class_id);
            self.changes.extend(rated.zip(changes).map(|((car_idx, _, _), change)| (*car_idx, change)));
        }
        self.computed_at_secs = Some(session_time_secs);
        &self.changes
    }
}

/// Every class present in `field`, once each, in a stable order.
fn class_ids_of(field: &[(i32, i32, irating::RaceResult)]) -> Vec<i32> {
    let mut ids: Vec<i32> = field.iter().map(|(_, class_id, _)| *class_id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Every competitor in the session as the estimate wants them, into `out`:
/// the scored field in its running order, then everyone registered but not
/// yet in it. Each is tagged with its class, and ranked **within that class**,
/// because that is the pool it is rated in — see [`IratingCache::changes`].
///
/// The entry list rather than the cars on track, because on the grid cars
/// arrive one at a time, and a field that grows by one every few seconds is
/// an estimate that moves by a few points every few seconds with nobody
/// having done anything. iRacing rates the session against everyone who
/// registered, so the estimate does too. A registered driver not yet in the
/// world is placed behind the scored field — as a starter until the race is
/// under way, and as a non-starter once it is, which is what iRacing makes of
/// a no-show. Only competitors count: the pace car and spectators are on the
/// entry list too, and rate nothing.
///
/// A car the sim has not scored yet (class position `0`) is placed behind
/// every scored car in its class, in the order it holds in the standings —
/// fed to the estimate as rank zero it read as better than first, which is
/// what made a full grid of unscored cars all "win". A driver with no iRating
/// published is left out altogether: there is nothing to rate them from, and a
/// zero rating makes every duel against them a certainty.
fn field_results(
    standings: &[StandingsEntry],
    drivers: &HashMap<i32, DriverMeta>,
    racing_under_way: bool,
    out: &mut Vec<(i32, i32, irating::RaceResult)>,
) {
    out.clear();
    // The last rank handed out in each class, so unscored and absent cars
    // queue up behind that class's scored field rather than behind the
    // session's.
    let mut next_rank: HashMap<i32, u32> = HashMap::new();
    for entry in standings.iter().filter(|entry| entry.class_position > 0) {
        let slot = next_rank.entry(entry.car_class_id).or_insert(0);
        *slot = (*slot).max(entry.class_position.unsigned_abs());
    }
    for entry in standings.iter().filter(|entry| entry.irating > 0) {
        let finish_rank = if entry.class_position > 0 {
            entry.class_position.unsigned_abs()
        } else {
            let slot = next_rank.entry(entry.car_class_id).or_insert(0);
            *slot += 1;
            *slot
        };
        let result = irating::RaceResult { finish_rank, start_irating: entry.irating.unsigned_abs(), started: true };
        out.push((entry.car_idx, entry.car_class_id, result));
    }

    // Sorted so the field reads identically from one tick to the next: the
    // driver map iterates in no particular order, and a reordering would
    // count as a change and force a recompute for nothing.
    let mut absent: Vec<(i32, i32, u32)> = drivers
        .iter()
        .filter(|(car_idx, driver)| {
            driver.is_competitor && driver.irating > 0 && !standings.iter().any(|e| e.car_idx == **car_idx)
        })
        .map(|(car_idx, driver)| (*car_idx, driver.car_class_id, driver.irating.unsigned_abs()))
        .collect();
    absent.sort_unstable();
    for (car_idx, class_id, start_irating) in absent {
        let slot = next_rank.entry(class_id).or_insert(0);
        *slot += 1;
        let result = irating::RaceResult { finish_rank: *slot, start_irating, started: !racing_under_way };
        out.push((car_idx, class_id, result));
    }
}

impl SessionTrackers {
    /// Discards every session-derived figure when iRacing moves on to the next
    /// session.
    ///
    /// One connection covers practice, then qualifying, then the race, and
    /// every tracker here holds a fact about one of them: stint lengths, stop
    /// counts, measured pace, fuel per lap. Carried across, a race inherits
    /// the pit lane of the practice session before it — which is how a sprint
    /// with no stops in it comes to report several, and why each car ends up
    /// apparently on a different strategy from the next.
    fn sync_to_session(&mut self, session_num: Option<i32>) {
        // A missing `SessionNum` is an unavailable sample, not a new session.
        // Resetting here and again when the same number returned erased stint
        // history during ordinary telemetry gaps.
        let Some(session_num) = session_num else {
            return;
        };
        let session_num = Some(session_num);
        if self.session_num == session_num {
            return;
        }
        // Radar smoothing is a sub-second filter over the last few frames
        // rather than a session history, and the cars beside you do not move
        // when the session number changes; keeping it avoids a visible jump.
        //
        // The lap curve is kept for a different reason: it describes the
        // track and the car, neither of which changes between the practice
        // session and the race after it. Thrown away, every race would spend
        // its opening lap on the coarser fallback gap for no reason. The
        // reference lap the curve is scaled by describes the same pair, and
        // thrown away would leave the opening lap's gaps sized off the
        // player's own pace — the error this measures its way around.
        let radar_smoothing = std::mem::take(&mut self.radar_smoothing);
        let lap_curve = std::mem::take(&mut self.lap_curve);
        let curve_reported_secs = self.curve_reported_secs;
        let reference_lap = std::mem::take(&mut self.reference_lap);
        let reference_reported = self.reference_reported;
        // Which car the curve above was measured from travels with it, or
        // [`Self::sync_to_focus`] would read the next tick as a change of focus
        // and throw away the very curve this went to the trouble of keeping.
        let focus_car_idx = self.focus_car_idx;
        let focus_class_id = self.focus_class_id;
        *self = Self {
            radar_smoothing,
            lap_curve,
            curve_reported_secs,
            reference_lap,
            reference_reported,
            focus_car_idx,
            focus_class_id,
            session_num,
            ..Self::default()
        };
    }

    /// Discards what the gap machinery holds about one car when the view moves
    /// to another.
    ///
    /// Only ever reached while spectating, where the camera changing car
    /// changes whose samples arrive. Two different amounts are thrown away.
    ///
    /// The lap being timed always goes. A camera switch mid-lap splices two
    /// cars' track positions into one lap, and `CURVE_MAX_STEP` only rejects
    /// the splices that happen to jump far enough to look wrong.
    ///
    /// The measured curve and the reference lap it is scaled by go only when
    /// the class changes. The curve holds what fraction of a lap's *time* a car
    /// has used at each point on it, and a GT3 and an LMP2 do not spend a lap
    /// the same way. Within one class the shape is the same, so it is kept —
    /// which is what leaves gaps sharp through a switch between team-mates.
    /// A class that isn't known is never treated as a change; there is nothing
    /// to be gained by discarding on a missing driver entry.
    fn sync_to_focus(&mut self, focus_car_idx: i32, class_id: Option<i32>) {
        let previous_class_id = self.focus_class_id;
        self.focus_class_id = class_id;
        if self.focus_car_idx == Some(focus_car_idx) {
            return;
        }
        // The first focus of a connection is not a change of focus: there is
        // nothing measured yet to spoil, and spoiling would discard the curve
        // `sync_to_session` above deliberately carried over.
        let first_focus = self.focus_car_idx.is_none();
        self.focus_car_idx = Some(focus_car_idx);
        if first_focus {
            return;
        }
        self.lap_curve.spoil_lap();
        if matches!((previous_class_id, class_id), (Some(before), Some(now)) if before != now) {
            self.lap_curve = relative::LapCurve::default();
            self.reference_lap = relative::ReferenceLap::default();
            self.curve_reported_secs = None;
            self.reference_reported = false;
        }
    }
}

/// `Var` handles cached once per `Session`; each is valid only for that
/// session's lifetime.
#[derive(Debug)]
struct TelemetryVars {
    car_idx_position: Var,
    car_idx_class_position: Var,
    car_idx_track_surface: Var,
    car_idx_est_time: Var,
    car_idx_lap: Var,
    lap_last_lap_time: Option<Var>,
    lap_best_lap_time: Option<Var>,
    /// Standings/pit-projection vars. All optional: a session missing these
    /// still gets a working relative widget, just an empty Standings list.
    car_idx_best_lap_time: Option<Var>,
    car_idx_last_lap_time: Option<Var>,
    car_idx_f2_time: Option<Var>,
    session_num: Option<Var>,
    /// The car the active camera is watching, which is what every view
    /// centres on while spectating — see [`resolve_focus_car`].
    ///
    /// Optional like the rest: without it the overlay simply stays centred on
    /// the player's own car, which is what it did before this existed.
    cam_car_idx: Option<Var>,
    /// Full-course caution state, for the pit window — see
    /// [`RelativeMeta::under_caution`].
    session_flags: Option<Var>,
    /// Each car's own flags — the black flags among them are what the
    /// Relative's gutter marks; see `snapshot::Penalty`. Optional because
    /// it is a newer variable than the rest, and an overlay without it just
    /// has no penalty markers.
    car_idx_session_flags: Option<Var>,
    /// The sim's own laps-to-go count, meaningful only in a lap-limited
    /// session — see [`UNLIMITED_LAPS_SENTINEL`].
    session_laps_remain_ex: Option<Var>,
    /// Gridding, pace laps, racing or done — what decides whether a race's
    /// clock has started. See [`RelativeMeta::racing_under_way`].
    session_state: Option<Var>,
    /// Radar's side signal and distance source; optional for the same
    /// reason as the standings vars above.
    car_left_right: Option<Var>,
    car_idx_lap_dist_pct: Option<Var>,
    /// Each car's current compound index, for the Standings tyre column.
    /// Optional: a build without it simply leaves the column's circles off
    /// every row but the player's own, which falls back to
    /// `PlayerTireCompound`.
    car_idx_tire_compound: Option<Var>,
    /// Session clock, for stint-length tracking.
    session_time: Option<Var>,
    /// Seconds left in the session, for the Relative footer's race clock and
    /// predicted-final-lap estimate.
    session_time_remain: Option<Var>,
    /// The player's own incident count, for the Relative header's incident chip.
    player_incidents: Option<Var>,
    /// `PlayerCarTeamIncidentCount`: the whole team's, which is what the
    /// limit is applied to in a team event. Equal to the player's own
    /// everywhere else.
    team_incidents: Option<Var>,
    /// Weather vars; optional for the same reason as the standings vars above.
    track_temp: Option<Var>,
    air_temp: Option<Var>,
    fog_level: Option<Var>,
    wind_vel: Option<Var>,
    wind_dir: Option<Var>,
    /// Rain falling now, 0–1, at the start/finish line.
    precipitation: Option<Var>,
    /// The steward's wet-tyre declaration.
    declared_wet: Option<Var>,
    /// The sim's overall track-wetness estimate — see
    /// [`TrackWetness::from_raw`]. Documented but not yet seen in a live
    /// dump, hence read as strictly optional.
    track_wetness: Option<Var>,
    /// The player's own car heading, so wind direction can be shown
    /// relative to the car instead of an absolute (and less useful) bearing.
    yaw: Option<Var>,
    /// Ground speed in m/s, used to distinguish a stopped pit service from a
    /// drive-through.
    speed: Option<Var>,
    /// Whether the car is sitting in the garage with the setup screen up, so
    /// the overlay can get out of the way of it.
    ///
    /// `IsInGarage` rather than `IsOnTrack`, which is also false on the grid
    /// before a rolling start, under tow, and in replays — every one of them a
    /// moment the panels should still be there.
    in_garage: Option<Var>,
    /// Everything the black box's pages read. All optional, and deliberately
    /// so: which of these a car publishes is a property of the car, and a
    /// missing one is how a page decides not to draw that row.
    black_box: BlackBoxVars,
    /// `PlayerTireCompound`: the compound on the player's car now, as
    /// distinct from the one armed for the next stop
    /// ([`BlackBoxVars::tyre_compound`]).
    ///
    /// Read for the standings' tyre column and for the wet-tyre tell in the
    /// weather: the sim always publishes the player's own compound, so their
    /// row is filled even where the per-car array is absent.
    player_tyre_compound: Option<Var>,
}

/// The pit-service, tyre and driver-adjustment variables, grouped so
/// [`TelemetryVars`] doesn't grow forty more fields.
#[derive(Debug, Default)]
struct BlackBoxVars {
    fuel_level: Option<Var>,
    fuel_level_pct: Option<Var>,
    fuel_armed: Option<Var>,
    fuel_amount: Option<Var>,
    /// Per corner, in [`pit::Corner::ALL`] order.
    tyre_change: [Option<Var>; 4],
    tyre_pressure: [Option<Var>; 4],
    tearoff: Option<Var>,
    fast_repair_armed: Option<Var>,
    fast_repairs_available: Option<Var>,
    in_car: Option<Var>,
    /// Last-stop carcass temperatures, `[corner][left, middle, right]`.
    ///
    /// Frozen between stops by the sim, like `tyre_wear` — see [`TyreState`].
    tyre_temps: [[Option<Var>; 3]; 4],
    tyre_wear: [[Option<Var>; 3]; 4],
    /// Live hot pressure, sampled at each stop by [`StopPressures`].
    tyre_hot_pressure: [Option<Var>; 4],
    /// The garage setting, which stands in until the first stop of the session.
    tyre_cold_pressure: [Option<Var>; 4],
    brake_bias: Option<Var>,
    abs: Option<Var>,
    traction_control: Option<Var>,
    throttle_shape: Option<Var>,
    dash_page: Option<Var>,
    player_in_pit_stall: Option<Var>,
    /// Which compound the next stop will fit; a readout only, see
    /// [`PitService::pending_tyre_compound`].
    tyre_compound: Option<Var>,
}

/// Measures how much fuel this car actually uses on a racing lap.
///
/// Drawn from the last few laps run wholly on track. Any lap that touched pit
/// road is thrown away rather than averaged in: an in-lap and an out-lap are
/// part pit lane, use markedly less fuel than a racing lap, and are the two
/// laps most likely to be in the window at the exact moment a fuel load is
/// being worked out. Letting them in biases the figure low precisely when
/// being low is expensive.
///
/// A lap markedly slower than the window's quickest is the same problem in
/// slow motion: the seconds it lost were spent lifting behind traffic, so its
/// burn reads low, and a stretch of such laps used to wash the honest figures
/// out of the window one by one — which is how a fill planned mid-traffic at
/// Le Mans came up a lap short of the flag. Off-pace laps are therefore kept
/// out of the window (see [`FuelTracker::record`]) unless enough arrive in a
/// row to be the new pace rather than an anomaly.
#[derive(Debug, Default)]
struct FuelTracker {
    last_lap: i32,
    last_level: f32,
    /// Whether the lap under way has had the car on pit road at any point, so
    /// its usage is discarded when it completes.
    lap_touched_pits: bool,
    /// Litres used on each recent racing lap, with the lap time that vouches
    /// for it; bounded to [`FUEL_WINDOW`].
    recent: std::collections::VecDeque<FuelLap>,
    /// Completed racing laps in a row rejected as off-pace. At
    /// [`FUEL_WINDOW`] of them the slower pace is believed and the window
    /// reseeded — rain or deliberate saving, not an anomaly.
    slow_streak: u8,
}

/// One racing lap's fuel burn, tagged with the lap time that vouches for it.
#[derive(Debug)]
struct FuelLap {
    burn_litres: f32,
    /// Zero when no lap time was published, in which case the lap is taken at
    /// face value rather than judged.
    lap_secs: f32,
}

/// How much of each tick's fuel reading is taken as the new smoothed level.
///
/// The tank sloshes: braking and cornering move the level by a good fraction of
/// a litre, which is the whole quantity the Strategy page's live row is trying
/// to show. About half a second of averaging at iRacing's 60 Hz, which is short
/// enough that a start/finish crossing is not visibly late and long enough that
/// a corner does not read as a saving.
///
/// Both ends of the subtraction are smoothed the same way, so the lag largely
/// cancels rather than biasing the burn.
const FUEL_SMOOTHING: f32 = 0.03;

/// This lap's fuel burn so far, for the Strategy page's live save row.
#[derive(Debug, Default)]
struct LapFuelUse {
    /// The tank level with [`FUEL_SMOOTHING`] applied.
    smoothed_litres: Option<f32>,
    /// That smoothed level as the car crossed the start/finish line, which the
    /// burn is measured against. `None` on a lap that touched the pit lane,
    /// where the reading would include whatever the rig put in.
    at_line_litres: Option<f32>,
    last_lap: Option<i32>,
}

impl LapFuelUse {
    /// Litres burnt since the last start/finish crossing, or `None` before one
    /// has been seen or on a lap the pit lane spoiled.
    fn update(&mut self, lap: i32, level_litres: f32, on_pit_road: bool) -> Option<f32> {
        let smoothed = match self.smoothed_litres {
            Some(previous) => previous + (level_litres - previous) * FUEL_SMOOTHING,
            // Seeded rather than started from zero, or the first half-second of
            // every session reads as an enormous burn.
            None => level_litres,
        };
        self.smoothed_litres = Some(smoothed);
        if on_pit_road {
            self.at_line_litres = None;
        }
        if self.last_lap != Some(lap) {
            self.last_lap = Some(lap);
            if !on_pit_road {
                self.at_line_litres = Some(smoothed);
            }
            return None;
        }
        // Never negative: a splash mid-lap is not a negative burn.
        Some((self.at_line_litres? - smoothed).max(0.0))
    }
}

/// How many completed racing laps the fuel figure is drawn from. Long enough
/// that a single lap in traffic doesn't decide a fuel load; a genuine change
/// of pace — a wetter track, a driver saving fuel — reaches the figure after
/// this many laps in a row of it, via the reseed in [`FuelTracker::record`].
const FUEL_WINDOW: usize = 5;

/// How much slower than the window's quickest lap a lap may run and still
/// have its burn believed.
///
/// A driver's clean laps scatter by a few tenths — well under a percent —
/// while a lap compromised by traffic loses whole seconds, spent lifting, so
/// its burn reads low. Two percent sits between the two: a 2:04 among 2:01s,
/// the reported shape of the laps that starved a Le Mans fill, is two and a
/// half percent off and lands outside. Much tighter than
/// [`PACE_OUTLIER_RATIO`], deliberately: the pace mean only needs
/// stop-inflated laps out, while this figure must be drawn solely from laps
/// burnt at racing effort.
const FUEL_LAP_OUTLIER_RATIO: f32 = 1.02;

impl FuelTracker {
    /// Records `level_litres` at each lap change and returns the litres to
    /// plan a lap on, or `None` before a racing lap has completed.
    ///
    /// The **highest** of the recent laps, not their mean. Consumption varies
    /// by several percent with traffic, tow and how much lifting a lap
    /// involved, and the two errors are not equal: carrying a spare litre
    /// costs a fraction of a second in the pit lane, while being a litre short
    /// ends the race — the same asymmetry [`super::pit::auto_fuel_litres`]
    /// rounds up for. The mean was short about half the time by construction.
    ///
    /// A lap where the tank got *fuller* is skipped rather than recorded as
    /// negative use: that is a pit stop, not a lap of running.
    ///
    /// `last_lap_secs` is the just-completed lap's time as `LapLastLapTime`
    /// reads at the crossing — the same pairing [`LapPace::update`] relies on
    /// — and zero or negative when none has been published.
    fn update(&mut self, lap: i32, level_litres: f32, last_lap_secs: f32, on_pit_road: bool) -> Option<f32> {
        if lap != self.last_lap {
            if self.last_lap > 0 && !self.lap_touched_pits && self.last_level > level_litres {
                self.record(self.last_level - level_litres, last_lap_secs);
            }
            self.last_lap = lap;
            self.last_level = level_litres;
            // The new lap starts tainted if the car is on pit road as it
            // begins, which is exactly the case for an out-lap at a track
            // whose pit exit is past the line.
            self.lap_touched_pits = on_pit_road;
        } else if on_pit_road {
            self.lap_touched_pits = true;
        }
        self.recent.iter().map(|l| l.burn_litres).max_by(f32::total_cmp)
    }

    /// Admits one completed racing lap to the window, or rejects it as off-pace.
    ///
    /// Rejection needs all three of: a published lap time, that time more than
    /// [`FUEL_LAP_OUTLIER_RATIO`] over the quickest timed lap in the window,
    /// and a burn below the window's current figure. The last leg is the
    /// asymmetry every fuel decision here carries: a slow lap that *out-burnt*
    /// the window — a lap spent fighting — raises the plan, and raising the
    /// plan is the safe direction, so the clock is not allowed to veto it.
    ///
    /// [`FUEL_WINDOW`] rejections in a row mean the slower pace is the pace;
    /// the window restarts on the lap that proved it.
    fn record(&mut self, burn_litres: f32, lap_secs: f32) {
        let quickest = self.recent.iter().map(|l| l.lap_secs).filter(|secs| *secs > 0.0).min_by(f32::total_cmp);
        let figure = self.recent.iter().map(|l| l.burn_litres).max_by(f32::total_cmp);
        let off_pace = lap_secs > 0.0
            && quickest.is_some_and(|q| lap_secs > q * FUEL_LAP_OUTLIER_RATIO)
            && figure.is_some_and(|f| burn_litres < f);
        if off_pace {
            self.slow_streak = self.slow_streak.saturating_add(1);
            if usize::from(self.slow_streak) < FUEL_WINDOW {
                return;
            }
            self.recent.clear();
        }
        self.slow_streak = 0;
        if self.recent.len() >= FUEL_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(FuelLap { burn_litres, lap_secs });
    }
}

/// Samples tyre pressure at the instant the sim publishes a new stop snapshot.
///
/// Wear and carcass temperature need no help: the sim freezes them itself
/// between stops, which is the whole meaning of its "data collected at last
/// pit stop" caption. Reading them straight through is both simpler and more
/// correct than latching them again here, because any latch of our own has to
/// guess when the snapshot is ready. `PlayerCarInPitStall` is the obvious
/// guess and the wrong one: it goes true whenever the car merely sits in its
/// box — on the grid, after a tow, or rolling in ahead of the crew — while the
/// refresh itself rides with `PitstopActive`. A latch keyed to the stall flag
/// captures the untouched session defaults, closes, and shows 100% tread for
/// the rest of the race.
///
/// Pressure is the one reading with no last-stop variable behind it. Only the
/// live `LFpressure` is published, and it climbs all stint as the tyre heats,
/// so printing it raw next to last-stop wear would put two different moments
/// in one tile. The refresh of the wear readings marks the stop, and is
/// therefore used as the trigger to sample it.
#[derive(Debug, Default)]
struct StopPressures {
    /// The wear readings the previous tick carried, to spot the refresh.
    previous_wear: Option<[[f32; 3]; 4]>,
    captured_kpa: Option<[f32; 4]>,
}

impl StopPressures {
    /// Returns the pressures to print, or `None` before the first stop.
    ///
    /// `None` also covers joining a session that has already had a stop: the
    /// refresh happened before the overlay was watching, and the pressure that
    /// went with it is not recoverable. Callers fall back to the garage's cold
    /// pressure, which is what the sim itself shows before a stop.
    fn update(&mut self, wear: [[f32; 3]; 4], live_kpa: [f32; 4]) -> Option<[f32; 4]> {
        // Exact inequality is the point: these are the sim's own bytes
        // republished wholesale at a stop, not a computed quantity that could
        // drift by an ulp.
        if self.previous_wear.is_some_and(|previous| previous != wear) {
            self.captured_kpa = Some(live_kpa);
        }
        self.previous_wear = Some(wear);
        self.captured_kpa
    }
}

impl BlackBoxVars {
    /// Looks up every black box variable, tolerating any of them being absent.
    ///
    /// # Safety
    /// `session` must be a live, currently connected `Session`.
    unsafe fn find_all(session: &Session) -> Self {
        /// The sim's own prefixes for the four corners, in `Corner::ALL` order.
        const PREFIXES: [&str; 4] = ["LF", "RF", "LR", "RR"];
        /// Across each tyre: inner, middle, outer as the sim names them.
        const EDGES: [&str; 3] = ["L", "M", "R"];

        // SAFETY: forwarding this function's own contract.
        let find = |name: &str| unsafe { session.find_var(name) };

        // `{}` stands in for the corner prefix, so each pattern reads the way
        // the SDK spells it.
        let per_corner = |pattern: &str| PREFIXES.map(|p| find(&pattern.replace("{}", p)));
        let per_edge =
            |pattern: &str| PREFIXES.map(|p| EDGES.map(|edge| find(&pattern.replace("{}", p).replace("[]", edge))));

        Self {
            fuel_level: find("FuelLevel"),
            fuel_level_pct: find("FuelLevelPct"),
            fuel_armed: find("dpFuelFill"),
            // `PitSvFuel` in preference to `dpFuelAddKg`: the latter is
            // labelled kilograms, so reading it as litres would be wrong the
            // moment a car's fuel density isn't 1.
            fuel_amount: find("PitSvFuel").or_else(|| find("dpFuelAddKg")),
            tyre_change: per_corner("dp{}TireChange"),
            tyre_pressure: per_corner("PitSv{}P"),
            tearoff: find("dpWindshieldTearoff"),
            fast_repair_armed: find("dpFastRepair"),
            fast_repairs_available: find("FastRepairAvailable"),
            in_car: find("IsOnTrack"),
            tyre_temps: per_edge("{}tempC[]"),
            tyre_wear: per_edge("{}wear[]"),
            tyre_hot_pressure: per_corner("{}pressure"),
            tyre_cold_pressure: per_corner("{}coldPressure"),
            brake_bias: find("dcBrakeBias"),
            abs: find("dcABS"),
            traction_control: find("dcTractionControl"),
            throttle_shape: find("dcThrottleShape"),
            dash_page: find("dcDashPage"),
            player_in_pit_stall: find("PlayerCarInPitStall"),
            tyre_compound: find("PitSvTireCompound"),
        }
    }
}

impl TelemetryVars {
    /// Looks up every telemetry variable the dash/relative widget needs.
    ///
    /// Returns `None` if a variable this widget cannot function without is
    /// missing from the current session (e.g. an unsupported sim mode).
    /// Optional vars (driver aids, lap-time fallbacks) are allowed to be
    /// absent; the fields they feed are simply hidden or estimated instead.
    ///
    /// # Safety
    /// `session` must be a live, currently connected `Session`.
    unsafe fn find_all(session: &Session) -> Option<Self> {
        let find = |name: &str| -> Option<Var> {
            // SAFETY: forwarding `find_all`'s own contract; reads only the
            // session's live variable header list.
            unsafe { session.find_var(name) }
        };
        Some(Self {
            car_idx_position: find("CarIdxPosition")?,
            car_idx_class_position: find("CarIdxClassPosition")?,
            car_idx_track_surface: find("CarIdxTrackSurface")?,
            car_idx_est_time: find("CarIdxEstTime")?,
            car_idx_lap: find("CarIdxLap")?,
            lap_last_lap_time: find("LapLastLapTime"),
            lap_best_lap_time: find("LapBestLapTime"),
            car_idx_best_lap_time: find("CarIdxBestLapTime"),
            car_idx_last_lap_time: find("CarIdxLastLapTime"),
            car_idx_f2_time: find("CarIdxF2Time"),
            session_num: find("SessionNum"),
            cam_car_idx: find("CamCarIdx"),
            session_flags: find("SessionFlags"),
            car_idx_session_flags: find("CarIdxSessionFlags"),
            session_laps_remain_ex: find("SessionLapsRemainEx"),
            session_state: find("SessionState"),
            car_left_right: find("CarLeftRight"),
            car_idx_lap_dist_pct: find("CarIdxLapDistPct"),
            car_idx_tire_compound: find("CarIdxTireCompound"),
            session_time: find("SessionTime"),
            session_time_remain: find("SessionTimeRemain"),
            player_incidents: find("PlayerCarMyIncidentCount"),
            team_incidents: find("PlayerCarTeamIncidentCount"),
            track_temp: find("TrackTempCrew"),
            air_temp: find("AirTemp"),
            fog_level: find("FogLevel"),
            wind_vel: find("WindVel"),
            wind_dir: find("WindDir"),
            precipitation: find("Precipitation"),
            declared_wet: find("WeatherDeclaredWet"),
            track_wetness: find("TrackWetness"),
            yaw: find("Yaw"),
            speed: find("Speed"),
            in_garage: find("IsInGarage"),
            player_tyre_compound: find("PlayerTireCompound"),
            // SAFETY: forwarding `find_all`'s own contract.
            black_box: unsafe { BlackBoxVars::find_all(session) },
        })
    }
}

/// The stand-in for a name the session has not published yet.
///
/// `Arc<str>` has no `Default`, and the empty `String`s this replaced were
/// allocated afresh at every use. One shared empty allocation instead, so a
/// grid of cars whose driver entries have not arrived costs nothing per tick.
fn unnamed() -> Arc<str> {
    static EMPTY: OnceLock<Arc<str>> = OnceLock::new();
    Arc::clone(EMPTY.get_or_init(|| Arc::from("")))
}

/// Metadata about one driver, cached from the slower-updating session-info YAML.
///
/// The strings are `Arc<str>` because every snapshot clones them out of here
/// sixty times a second while they change only when the YAML does; see the note
/// at the top of [`super::snapshot`].
#[derive(Debug, Clone)]
struct DriverMeta {
    user_name: Arc<str>,
    /// The customer id of whoever is in the car — in a team session, the
    /// current driver rather than the entry's owner. `None` where the YAML
    /// omits it.
    user_id: Option<i32>,
    /// The number on the car — see `session_info::Driver::car_number`.
    car_number: Arc<str>,
    car_screen_name: Arc<str>,
    irating: i32,
    license_color: Arc<str>,
    car_class_id: i32,
    car_class_short_name: Arc<str>,
    car_class_color: Arc<str>,
    /// iRacing's ranking of the class's speed, higher being faster; `None`
    /// where the YAML omits it — see `session_info::Driver`.
    car_class_rel_speed: Option<i32>,
    /// iRacing's estimated lap for the class, in seconds; `None` where absent.
    car_class_est_lap_secs: Option<f32>,
    /// Whether this entry is a real competitor — i.e. not the pace car and
    /// not a spectator.
    ///
    /// The official `ResultsPositions` list already excludes both, so only the
    /// two places that work from live telemetry instead need this:
    /// [`live_classification`], and the Relative row list.
    is_competitor: bool,
    /// The driver's profile flag — see `session_info::Driver::flair_id`.
    flair_id: i32,
}

/// Caches the parsed session-info YAML, re-parsing only when iRacing's own
/// `session_info_update` counter changes; that YAML updates far slower than
/// the 60 Hz telemetry tick.
#[derive(Debug, Default)]
struct SessionInfoCache {
    last_update: Option<i32>,
    player_car_idx: Option<i32>,
    /// `WeekendInfo.TeamRacing`: whether this is a team session, where the
    /// car's driver entry can name somebody other than the player.
    team_racing: bool,
    /// `DriverInfo.DriverUserID`, the player's own customer id.
    driver_user_id: Option<i32>,
    /// `WeekendInfo.SubSessionID`, the team-sync room key. `None` until read.
    sub_session_id: Option<u64>,
    drivers: HashMap<i32, DriverMeta>,
    /// The full per-session classification list, refreshed alongside
    /// everything else; `build_snapshot` picks the entry matching the
    /// live `SessionNum` telemetry value.
    sessions: Vec<SessionResults>,
    /// `None` if the session has no incident limit ("unlimited") or hasn't
    /// been parsed yet.
    incident_limit: Option<i32>,
    /// The session's declared chance of rain as a 0.0-1.0 fraction, or
    /// `None` until the YAML has been read.
    precip_chance: Option<f32>,
    /// Which of the player's compound indices name a wet tyre, from
    /// `DriverInfo.DriverTires` — the index space `PlayerTireCompound`
    /// reports in. Empty where the YAML names no compounds, which sends
    /// [`on_wet_tyres`] to its index-convention fallback.
    tyre_is_wet_by_index: HashMap<i32, bool>,
    /// Each named compound's initial (`Wet` → `W`, `Hard` → `H`), from the
    /// same `DriverTires` list, for the Standings tyre column. Empty where
    /// the YAML names none, which sends [`tyre_compound_of`] to the D/W
    /// convention instead.
    tyre_letter_by_index: HashMap<i32, char>,
    /// Each car's qualifying result, `car_idx` → (overall slot, class slot),
    /// both 1-based. The starting grid, in effect: it orders the field while
    /// it forms up, and it is the change baseline that survives joining
    /// mid-race. Empty where the event had no qualifying.
    qualify_grid: HashMap<i32, (i32, i32)>,
    /// The player's own pit stall as a fraction of a lap, or `0.0` where the
    /// session publishes none — see [`session_info::DriverInfo`].
    pit_stall_pct: f32,
    /// The lap length in metres, or `None` where it wasn't published or
    /// couldn't be parsed.
    track_length_m: Option<f32>,
    /// The fuel the player's car can carry this session, from the YAML — see
    /// [`session_info::DriverInfo::tank_capacity_litres`]. `None` until read,
    /// or where the YAML gives no tank.
    tank_capacity_litres: Option<f32>,
}

impl SessionInfoCache {
    /// Re-parses the session-info YAML if iRacing has published a newer version.
    ///
    /// # Safety
    /// `session` must be a live, currently connected `Session`.
    unsafe fn refresh(&mut self, session: &Session) {
        // SAFETY: forwarding `refresh`'s own contract.
        let update = unsafe { session.session_info_update() };
        if self.last_update == Some(update) {
            return;
        }
        self.last_update = Some(update);

        // SAFETY: forwarding `refresh`'s own contract.
        let yaml = unsafe { session.session_info() };
        match SessionInfoYaml::parse(&yaml) {
            Ok(info) => {
                self.player_car_idx = Some(info.driver_info.driver_car_idx);
                self.team_racing = info.weekend_info.team_racing != 0;
                self.driver_user_id = info.driver_info.driver_user_id;
                self.sub_session_id = info.weekend_info.sub_session_id;
                self.drivers = info
                    .driver_info
                    .drivers
                    .iter()
                    .map(|driver| {
                        // The one place these strings are allocated: once per
                        // driver per YAML version, rather than per tick.
                        let meta = DriverMeta {
                            user_name: Arc::from(driver.user_name.as_str()),
                            user_id: driver.user_id,
                            car_number: Arc::from(driver.car_number.as_str()),
                            car_screen_name: Arc::from(driver.car_screen_name.as_str()),
                            irating: driver.irating,
                            license_color: Arc::from(driver.license_color.as_str()),
                            car_class_id: driver.car_class_id,
                            car_class_short_name: Arc::from(class_short_name(&driver.car_class_short_name)),
                            car_class_color: Arc::from(driver.car_class_color.as_str()),
                            car_class_rel_speed: driver.car_class_rel_speed,
                            car_class_est_lap_secs: driver.car_class_est_lap_time,
                            is_competitor: driver.car_is_pace_car == 0 && driver.is_spectator == 0,
                            flair_id: driver.flair_id,
                        };
                        (driver.car_idx, meta)
                    })
                    .collect();
                self.sessions = info.session_info.sessions;
                self.incident_limit = parse_incident_limit(&info.weekend_info.weekend_options.incident_limit);
                self.precip_chance = parse_percent(&info.weekend_info.weekend_options.chance_of_rain);
                self.tyre_is_wet_by_index = info
                    .driver_info
                    .driver_tires
                    .iter()
                    .map(|tire| (tire.tire_index, tire.tire_compound_type.to_ascii_lowercase().contains("wet")))
                    .collect();
                self.tyre_letter_by_index = info
                    .driver_info
                    .driver_tires
                    .iter()
                    .filter_map(|tire| {
                        let letter = tire.tire_compound_type.chars().find(char::is_ascii_alphabetic)?;
                        Some((tire.tire_index, letter.to_ascii_uppercase()))
                    })
                    .collect();
                // The YAML's qualifying positions are zero-based — pole is
                // `Position: 0` — unlike every other position it publishes.
                self.qualify_grid = info
                    .qualify_results_info
                    .results
                    .iter()
                    .map(|result| (result.car_idx, (result.position + 1, result.class_position + 1)))
                    .collect();
                self.pit_stall_pct = info.driver_info.driver_pit_trk_pct;
                self.track_length_m = parse_track_length(&info.weekend_info.track_length);
                self.tank_capacity_litres = info.driver_info.tank_capacity_litres();
            }
            Err(err) => println!("note: could not parse session info: {err:#}"),
        }
    }
}

/// How long a vanished car keeps its tow marker, in seconds.
///
/// An iRacing tow runs a minute or two. A car gone longer than this has
/// retired, disconnected or is sitting in the garage between stints, and a
/// tow clock still ticking under it would be a claim about something else.
const TOW_SHOW_MAX_SECS: f64 = 180.0;

/// When each car left the world, for the Standings tow marker.
///
/// iRacing publishes a tow timer for the player alone (`PlayerCarTowTime`);
/// for everyone else the only visible fact is the car vanishing from
/// `CarIdxTrackSurface` mid-session. The clock here is therefore *time since
/// it vanished*, counted up locally — how long they have been gone, not how
/// long the sim will hold them, which nothing published can say.
#[derive(Debug, Default)]
struct TowTracker {
    /// Cars seen in the world at least once this session, so a grid slot
    /// whose driver never gridded is not "towing".
    seen: std::collections::HashSet<i32>,
    /// `SessionTime` when each currently-absent car vanished.
    vanished_at: HashMap<i32, f64>,
}

impl TowTracker {
    /// Seconds this car has been gone from the world, or `None` where that
    /// isn't a tow: still in the world, never seen in it, or gone longer
    /// than [`TOW_SHOW_MAX_SECS`].
    fn tow_secs(&mut self, car_idx: i32, in_world: bool, now: f64) -> Option<f64> {
        if in_world {
            self.seen.insert(car_idx);
            self.vanished_at.remove(&car_idx);
            return None;
        }
        if !self.seen.contains(&car_idx) {
            return None;
        }
        let since = now - *self.vanished_at.entry(car_idx).or_insert(now);
        (since <= TOW_SHOW_MAX_SECS).then_some(since)
    }
}

#[cfg(test)]
mod tow_tests {
    use super::TowTracker;

    #[test]
    fn a_car_that_vanishes_mid_session_ticks_a_tow_clock() {
        let mut tow = TowTracker::default();
        assert_eq!(tow.tow_secs(4, true, 100.0), None, "in the world is not a tow");
        assert_eq!(tow.tow_secs(4, false, 130.0), Some(0.0), "the clock starts when the car vanishes");
        assert_eq!(tow.tow_secs(4, false, 172.5), Some(42.5));
        assert_eq!(tow.tow_secs(4, true, 200.0), None, "back in the world ends the tow");
        assert_eq!(tow.tow_secs(4, false, 210.0), Some(0.0), "a second tow starts a fresh clock");
    }

    /// A car that has never gridded is absent, not towing; and one gone past
    /// the cap has retired or disconnected, which is not a tow either.
    #[test]
    fn absence_alone_is_not_a_tow() {
        let mut tow = TowTracker::default();
        assert_eq!(tow.tow_secs(9, false, 50.0), None, "never seen in the world");
        tow.tow_secs(9, true, 60.0);
        tow.tow_secs(9, false, 70.0);
        assert_eq!(tow.tow_secs(9, false, 70.0 + super::TOW_SHOW_MAX_SECS + 1.0), None, "gone too long to be a tow");
    }
}

/// Where each car's race began, so the Standings' change column can say how
/// the race has treated it.
///
/// The baseline, in order of preference (see `plans/position-change.md`):
/// the last class position a car held while the field was still forming up —
/// its grid slot — and otherwise its qualifying result, the one source that
/// survives joining or restarting the overlay mid-race. A car with neither
/// gets no figure at all: no number is better than one measured from a wrong
/// baseline.
#[derive(Debug, Default)]
struct PositionChangeTracker {
    race_start: HashMap<i32, i32>,
}

impl PositionChangeTracker {
    /// Places gained since the race began, positive = gained. `None` before
    /// the green — the grid is being captured, not measured — and for a car
    /// with no baseline.
    fn change(&mut self, car_idx: i32, class_position: i32, racing: bool, quali_slot: Option<i32>) -> Option<i32> {
        if class_position < 1 {
            return None;
        }
        if !racing {
            // The forming grid: keep the last slot seen — a re-grid after a
            // red flag re-latches.
            self.race_start.insert(car_idx, class_position);
            return None;
        }
        self.race_start.get(&car_idx).copied().or(quali_slot).map(|start| {
            self.race_start.entry(car_idx).or_insert(start);
            start - class_position
        })
    }
}

#[cfg(test)]
mod position_change_tests {
    use super::PositionChangeTracker;

    #[test]
    fn changes_measure_from_the_grid() {
        let mut tracker = PositionChangeTracker::default();
        assert_eq!(tracker.change(3, 5, false, None), None, "the grid is captured, not measured");
        assert_eq!(tracker.change(3, 4, true, None), Some(1), "measured from the gridded slot");
        assert_eq!(tracker.change(3, 2, true, None), Some(3), "gains count");
        assert_eq!(tracker.change(3, 6, true, None), Some(-1), "losses go negative");
    }

    /// The overlay restarted mid-race: no grid was watched, so the
    /// qualifying result is the baseline — and it latches, so a stale YAML
    /// can't move it later.
    #[test]
    fn a_mid_race_join_falls_back_to_qualifying() {
        let mut tracker = PositionChangeTracker::default();
        assert_eq!(tracker.change(4, 3, true, Some(6)), Some(3));
        assert_eq!(tracker.change(4, 2, true, Some(9)), Some(4), "the first slot stands");
        assert_eq!(tracker.change(9, 3, true, None), None, "no baseline, no number");
    }

    /// A car the scorer hasn't placed has no baseline to be measured from.
    #[test]
    fn unscored_cars_stay_blank() {
        let mut tracker = PositionChangeTracker::default();
        assert_eq!(tracker.change(7, 0, true, None), None);
        assert_eq!(tracker.change(7, 3, true, Some(3)), Some(0));
    }
}

/// The compound a car is running, resolved for the Standings tyre column.
///
/// The letter is the initial of the compound's name where the session YAML
/// names one (`Wet` → `W`, `Hard` → `H`). The named list is the player's
/// car's — the only one the YAML gives — so other models' indices are read
/// against it; right whenever the field shares the dry-at-zero convention,
/// which is every road series with wets. Unnamed indices fall back to that
/// convention outright: `D` at index zero, and anything else is only called
/// a wet on a track that is actually wet — the same guard [`on_wet_tyres`]
/// applies, so a second dry compound is never badged as the wet.
fn tyre_compound_of(
    compound: Option<i32>,
    letters: &HashMap<i32, char>,
    wet_by_index: &HashMap<i32, bool>,
    track_wet: bool,
) -> Option<super::snapshot::TyreCompound> {
    let index = compound.filter(|index| *index >= 0)?;
    let wet = on_wet_tyres(Some(index), wet_by_index, track_wet);
    let letter = letters.get(&index).copied().unwrap_or(if wet { 'W' } else { 'D' });
    Some(super::snapshot::TyreCompound { letter, wet })
}

#[cfg(test)]
mod tyre_compound_tests {
    use super::*;

    #[test]
    fn named_compounds_take_their_initial() {
        let letters = HashMap::from([(0, 'H'), (1, 'W')]);
        let wets = HashMap::from([(0, false), (1, true)]);
        let hard = tyre_compound_of(Some(0), &letters, &wets, false).expect("a named compound resolves");
        assert_eq!((hard.letter, hard.wet), ('H', false));
        let wet = tyre_compound_of(Some(1), &letters, &wets, false).expect("a named compound resolves");
        assert_eq!((wet.letter, wet.wet), ('W', true));
    }

    /// No names published: index zero is the dry, and the wet convention only
    /// stands on a wet track — a second dry must never be badged as a wet.
    #[test]
    fn unnamed_compounds_fall_back_to_the_dry_wet_convention() {
        let no_letters = HashMap::new();
        let no_wets = HashMap::new();
        let dry = tyre_compound_of(Some(0), &no_letters, &no_wets, true).expect("index zero resolves");
        assert_eq!((dry.letter, dry.wet), ('D', false));
        let on_dry_track = tyre_compound_of(Some(1), &no_letters, &no_wets, false).expect("index one resolves");
        assert_eq!((on_dry_track.letter, on_dry_track.wet), ('D', false));
        let on_wet_track = tyre_compound_of(Some(1), &no_letters, &no_wets, true).expect("index one resolves");
        assert_eq!((on_wet_track.letter, on_wet_track.wet), ('W', true));
    }

    #[test]
    fn an_unknown_compound_resolves_to_nothing() {
        assert_eq!(tyre_compound_of(None, &HashMap::new(), &HashMap::new(), true), None);
        assert_eq!(tyre_compound_of(Some(-1), &HashMap::new(), &HashMap::new(), true), None);
    }
}

/// The class short name every tag shows, normalized once at the YAML.
///
/// iRacing names a one-car class after its car — the Dallara P217's class
/// arrives as a "P217" variant rather than as LMP2 — but the category is
/// what a driver thinks in, and it is what keeps the tags narrow. Applied
/// where `DriverMeta` is built, so the Standings, the Relative and the
/// Faster Class plates all say the same thing.
fn class_short_name(published: &str) -> &str {
    if published.to_ascii_lowercase().contains("p217") { "LMP2" } else { published }
}

#[cfg(test)]
mod class_name_tests {
    use super::class_short_name;

    #[test]
    fn the_p217_class_reads_as_the_category() {
        assert_eq!(class_short_name("P217"), "LMP2");
        assert_eq!(class_short_name("Dallara P217"), "LMP2");
        assert_eq!(class_short_name("GT3"), "GT3", "everything else passes through untouched");
        assert_eq!(class_short_name("LMP2"), "LMP2");
    }
}

/// Rain heavier than this counts as "the track is wet" for compound naming.
///
/// A per-two-hundred trace of drizzle should not flip an unnamed index-1
/// compound from `ALT` to wet; one percent matches the test iFL03 applies
/// to the same variable. See `plans/weather-rain.md`.
const WET_TRACK_PRECIP_MIN: f32 = 0.01;

/// Whether the player's car is on a wet compound.
///
/// By name where the session names its compounds (`DriverInfo.DriverTires`);
/// where it doesn't, by the index convention iFL03 uses — 1 and 3 are the wet
/// slots — and then only while the track is actually wet, so a second dry
/// compound at index 1 in a dry session is never called a wet. Unknown
/// (`-1`, or the variable absent) is `false`: a surface showing this must
/// never wrongly claim wets, and staying quiet is the honest failure.
fn on_wet_tyres(compound: Option<i32>, tyre_is_wet_by_index: &HashMap<i32, bool>, track_wet: bool) -> bool {
    let Some(index) = compound.filter(|index| *index >= 0) else {
        return false;
    };
    match tyre_is_wet_by_index.get(&index) {
        Some(named_wet) => *named_wet,
        None => track_wet && (index == 1 || index == 3),
    }
}

#[cfg(test)]
mod wet_tyre_tests {
    use super::*;

    fn named() -> HashMap<i32, bool> {
        HashMap::from([(0, false), (1, true)])
    }

    #[test]
    fn a_named_compound_answers_by_name_whatever_the_track_is_doing() {
        assert!(on_wet_tyres(Some(1), &named(), false), "a named wet needs no wet track");
        assert!(!on_wet_tyres(Some(0), &named(), true), "a named dry stays dry in the rain");
    }

    /// No names published: the index convention stands in, gated on the
    /// track being wet so a second dry compound is never called a wet.
    #[test]
    fn unnamed_indices_fall_back_to_the_convention_only_on_a_wet_track() {
        let none = HashMap::new();
        assert!(on_wet_tyres(Some(1), &none, true));
        assert!(on_wet_tyres(Some(3), &none, true));
        assert!(!on_wet_tyres(Some(1), &none, false));
        assert!(!on_wet_tyres(Some(2), &none, true));
    }

    #[test]
    fn unknown_compounds_are_never_called_wet() {
        assert!(!on_wet_tyres(Some(-1), &named(), true));
        assert!(!on_wet_tyres(None, &named(), true));
        // Named list exists but doesn't cover this index: fall back, gated.
        assert!(on_wet_tyres(Some(3), &named(), true));
        assert!(!on_wet_tyres(Some(3), &named(), false));
    }
}

/// Which car every view is centred on: the player's, or the one being watched.
///
/// The player's own car whenever they are a real entry in this session, which
/// is every session they drive — including a team endurance race, where
/// `DriverCarIdx` names the team's car rather than whoever is currently in the
/// seat, so watching a team-mate drive already centres on the right car.
///
/// When they are *not* a real entry — a spectator entry, the pace car, or no
/// entry at all — the camera's focus car stands in. Without it Relative would
/// measure every gap against a car that is not in the world and Standings
/// would have no row to build its window around, which is what spectating
/// looked like before this existed.
///
/// `previous` is held on to whenever `cam_car_idx` names nothing that races:
/// it reads `-1` during camera transitions, and it names the pace car whenever
/// the camera is on that. Holding still beats jumping to a car nobody chose.
///
/// The test is `is_competitor`, a property of the session rather than of the
/// tick, so focus cannot flap. A live test like "the player is `NotInWorld`"
/// would hand focus to the camera every time they sat in the garage waiting to
/// join, and Standings would wander off while they waited.
fn resolve_focus_car(
    drivers: &HashMap<i32, DriverMeta>,
    player_car_idx: i32,
    cam_car_idx: Option<i32>,
    previous: Option<i32>,
) -> i32 {
    let races = |idx: &i32| drivers.get(idx).is_some_and(|driver| driver.is_competitor);
    if races(&player_car_idx) {
        return player_car_idx;
    }
    cam_car_idx.filter(races).or(previous).unwrap_or(player_car_idx)
}

/// What [`resolve_seat`] decides from: one tick's worth.
#[derive(Clone, Copy)]
struct SeatInputs<'a> {
    drivers: &'a HashMap<i32, DriverMeta>,
    player_car_idx: i32,
    focus_car_idx: i32,
    /// `IsOnTrack`: the player is in the car with the physics running.
    driving: bool,
    /// `WeekendInfo.TeamRacing`.
    team_racing: bool,
    /// `DriverInfo.DriverUserID`, the player's own customer id.
    driver_user_id: Option<i32>,
    /// The player's own car's `CarIdxTrackSurface`.
    player_track_location: TrackLocation,
}

/// Where the player is relative to the focus car — see [`Seat`].
///
/// In order:
///
/// 1. Focus on somebody else's car is spectating, whatever else is true.
/// 2. `IsOnTrack` is driving. Checked before the YAML so that the moment the
///    player is back in the car they are driving, even while the YAML still
///    names the team-mate who just climbed out.
/// 3. A team session whose car entry carries a customer id other than the
///    player's, with the car in the world, is a team-mate's stint. It is the
///    YAML that is compared, not a live scalar, so this cannot flap.
/// 4. Anything else is the player's own car with nobody driving it.
///
/// `IsOnTrackCar` is deliberately not consulted: the SDK documents it as
/// staying false while another driver has the car in a team event.
fn resolve_seat(inputs: SeatInputs<'_>) -> Seat {
    let car = inputs.drivers.get(&inputs.focus_car_idx);
    if inputs.focus_car_idx != inputs.player_car_idx {
        return Seat::Spectating(car.map_or_else(unnamed, |driver| Arc::clone(&driver.user_name)));
    }
    if inputs.driving {
        return Seat::Driving;
    }
    let in_the_world = !matches!(inputs.player_track_location, TrackLocation::NotInWorld);
    let somebody_else = match (car.and_then(|driver| driver.user_id), inputs.driver_user_id) {
        (Some(theirs), Some(mine)) => theirs != mine,
        _ => false,
    };
    match car {
        Some(driver) if inputs.team_racing && in_the_world && somebody_else => {
            Seat::TeamMate(Arc::clone(&driver.user_name))
        }
        _ => Seat::OutOfCar,
    }
}

/// Reads the current telemetry row and builds an owned snapshot.
///
/// # Safety
/// `session` must be a live, currently connected `Session`, and every `Var`
/// in `vars` must have been looked up against this exact `session`.
#[expect(
    clippy::too_many_lines,
    reason = "one telemetry tick's worth of scalar/slice reads for every widget; already split into build_radar/build_standings/build_pit_projection, further slicing would fragment the reader closures that all share `session`'s borrow"
)]
unsafe fn build_snapshot(
    session: &Session,
    vars: &TelemetryVars,
    info: &SessionInfoCache,
    trackers: &mut SessionTrackers,
    tuning: SnapshotTuning,
) -> TelemetrySnapshot {
    // Every closure below forwards `build_snapshot`'s own contract: `session`
    // is live and connected, and each `Var` was looked up against it.
    let f32_of_opt = |var: &Option<Var>| -> Option<f32> {
        var.as_ref().and_then(|v| {
            // SAFETY: see above.
            unsafe { session.value::<f32>(v) }.ok()
        })
    };
    let i32_of_opt = |var: &Option<Var>| -> Option<i32> {
        var.as_ref().and_then(|v| {
            // SAFETY: see above.
            unsafe { session.value::<i32>(v) }.ok()
        })
    };
    let f32_slice_of = |var: &Var| -> &[f32] {
        // SAFETY: see above.
        unsafe { session.value::<&[f32]>(var) }.unwrap_or(&[])
    };
    let i32_slice_of = |var: &Var| -> &[i32] {
        // SAFETY: see above.
        unsafe { session.value::<&[i32]>(var) }.unwrap_or(&[])
    };
    let f32_slice_of_opt = |var: &Option<Var>| -> &[f32] {
        match var {
            // SAFETY: see above.
            Some(v) => unsafe { session.value::<&[f32]>(v) }.unwrap_or(&[]),
            None => &[],
        }
    };
    let i32_slice_of_opt = |var: &Option<Var>| -> &[i32] {
        match var {
            // SAFETY: see above.
            Some(v) => unsafe { session.value::<&[i32]>(v) }.unwrap_or(&[]),
            None => &[],
        }
    };
    let f64_of_opt = |var: &Option<Var>| -> Option<f64> {
        var.as_ref().and_then(|v| {
            // SAFETY: see above.
            unsafe { session.value::<f64>(v) }.ok()
        })
    };
    let car_left_right = vars
        .car_left_right
        .as_ref()
        .and_then(|v| {
            // SAFETY: see above.
            unsafe { session.value::<CarLeftRight>(v) }.ok()
        })
        .unwrap_or(CarLeftRight::Off);

    let positions = i32_slice_of(&vars.car_idx_position);
    let class_positions = i32_slice_of(&vars.car_idx_class_position);
    let track_surfaces = i32_slice_of(&vars.car_idx_track_surface);
    let est_times = f32_slice_of(&vars.car_idx_est_time);
    let laps = i32_slice_of(&vars.car_idx_lap);
    let best_laps = f32_slice_of_opt(&vars.car_idx_best_lap_time);
    let last_laps = f32_slice_of_opt(&vars.car_idx_last_lap_time);
    let f2_times = f32_slice_of_opt(&vars.car_idx_f2_time);
    let lap_dist_pcts = f32_slice_of_opt(&vars.car_idx_lap_dist_pct);
    // A bitfield array, which the crate's typed slice accessor refuses — it
    // only knows plain ints — so this one is read raw and takes either shape.
    let session_flags_per_car: &[i32] = match &vars.car_idx_session_flags {
        // SAFETY: see above.
        Some(v) => match unsafe { session.var_value(v) } {
            Value::Bitfields(bits) | Value::Ints(bits) => bits,
            _ => &[],
        },
        None => &[],
    };

    // Every projection on screen is this number divided into the session
    // clock, so it is read once here and shared. `SessionNum` is picked up
    // first because a change of session invalidates the pace window, the stint
    // history and the fuel average alike.
    let session_num = i32_of_opt(&vars.session_num);
    trackers.sync_to_session(session_num);

    // Which car everything below is centred on: the player's own in every
    // session they drive, and the one the camera is watching when they are
    // only watching. See [`resolve_focus_car`].
    let player_car_idx = info.player_car_idx.unwrap_or(-1);
    let focus_car_idx =
        resolve_focus_car(&info.drivers, player_car_idx, i32_of_opt(&vars.cam_car_idx), trackers.focus_car_idx);
    let focus_is_player = focus_car_idx == player_car_idx;
    let my_car_class_id = info.drivers.get(&focus_car_idx).map(|d| d.car_class_id);
    trackers.sync_to_focus(focus_car_idx, my_car_class_id);

    // "me" from here on is the focus car — the player themselves unless they
    // are spectating, which is the only case where the two differ.
    let focus_idx = usize::try_from(focus_car_idx).ok();
    let me_est_time = focus_idx.and_then(|i| est_times.get(i)).copied().unwrap_or(0.0);
    let me_lap = focus_idx.and_then(|i| laps.get(i)).copied().unwrap_or(0);
    let me_lap_dist_pct = focus_idx.and_then(|i| lap_dist_pcts.get(i)).copied();
    let me_track_location = focus_idx
        .and_then(|i| track_surfaces.get(i))
        .copied()
        .map_or(TrackLocation::NotInWorld, track_location_from_raw);

    // Where the player is relative to that car. `IsOnTrack` is the sim's word
    // for "in it, physics running"; the YAML says whether the car is a team's
    // and who is in it; the player's own slot in the track-surface array says
    // whether the car is out at all. See [`resolve_seat`].
    let driving = vars
        .black_box
        .in_car
        .as_ref()
        // SAFETY: see the closures at the top of this function.
        .and_then(|v| unsafe { session.value::<bool>(v) }.ok())
        .unwrap_or(false);
    let player_idx = usize::try_from(player_car_idx).ok();
    let player_track_location = player_idx
        .and_then(|i| track_surfaces.get(i))
        .copied()
        .map_or(TrackLocation::NotInWorld, track_location_from_raw);
    let seat = resolve_seat(SeatInputs {
        drivers: &info.drivers,
        player_car_idx,
        focus_car_idx,
        driving,
        team_racing: info.team_racing,
        driver_user_id: info.driver_user_id,
        player_track_location,
    });

    let this_session = session_num.and_then(|num| info.sessions.iter().find(|s| s.session_num == num));
    let session_type = this_session.map(|s| s.session_type.clone()).filter(|s| !s.is_empty());
    // In lone qualifying the track is yours alone: every other car is sitting
    // in its box waiting its turn. Rows for them are rows about nothing, and
    // a gap to a parked car is meaningless.
    let lone_qualifying = session_type.as_deref().is_some_and(is_lone_qualifying);

    // An average over the player's recent green-flag laps rather than the
    // single most recent one: `LapLastLapTime` on its own means the whole of
    // lap two is projected from a standing-start opening lap, and every
    // caution lap, in-lap and lap spent in traffic moves the projection for as
    // long as it stands as the last lap. The session best is the fallback for
    // the opening lap, where there is nothing yet to average.
    //
    // Both lap times are read per car rather than from `LapLastLapTime` and
    // `LapBestLapTime` whenever the focus is not the player, because those two
    // scalars describe the player alone. Taken literally while spectating —
    // where the player has not driven a lap and never will — they leave every
    // gap below scaled by zero.
    let best_lap_secs = if focus_is_player {
        f32_of_opt(&vars.lap_best_lap_time)
    } else {
        focus_idx.and_then(|i| best_laps.get(i)).copied()
    }
    .filter(|t| *t > 0.0);
    let last_lap_secs = if focus_is_player {
        f32_of_opt(&vars.lap_last_lap_time)
    } else {
        focus_idx.and_then(|i| last_laps.get(i)).copied()
    }
    .unwrap_or(0.0);
    // `player_pace` is the player's own, and what the fuel and stop
    // projections — which never leave them — divide the clock by. A spectated
    // car's laps go to the per-car window instead of being written into it.
    let on_pit_road = matches!(me_track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits);
    let avg_lap_secs = if focus_is_player {
        trackers.player_pace.update(me_lap, last_lap_secs, on_pit_road)
    } else {
        trackers.recent_laps.update(focus_car_idx, me_lap, last_lap_secs)
    }
    .or(best_lap_secs)
    .unwrap_or(0.0);

    // The period relative gaps wrap around: one lap. A completed lap time
    // where there is one, and otherwise the player's own position read off
    // the lap in progress — `CarIdxEstTime` is time-from-the-line and
    // `CarIdxLapDistPct` the matching fraction, so their ratio is the lap.
    // Without that second source the opening lap of a race, where nobody has
    // a time yet, would be the one lap with no wrapping at all.
    //
    // The player's *best* lap in preference to their last: every gap on the
    // panel is scaled by this, so one traffic-bound or in-lap would drift the
    // whole column at once.
    let lap_secs = best_lap_secs
        .or(Some(avg_lap_secs).filter(|t| *t > 0.0))
        .or_else(|| me_lap_dist_pct.filter(|pct| *pct > LAP_ESTIMATE_MIN_PCT).map(|pct| me_est_time / pct))
        .unwrap_or(0.0);

    // The car with the lowest positive best-lap time holds the session's
    // fastest lap; used to badge that row in Relative and Standings.
    let fastest_overall_idx =
        best_laps.iter().enumerate().filter(|&(_, &t)| t > 0.0).min_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);

    let session_time_secs = f64_of_opt(&vars.session_time).unwrap_or(0.0);

    // One more point on the curve every relative gap below is read off: where
    // the player is on the lap, and what the session clock says. Only laps
    // driven wholly on track qualify, so the pit lane and the garage — which
    // share their track positions with the main straight — are excluded here
    // rather than inside the curve.
    let curve_was_ready = trackers.lap_curve.measured_lap_secs().is_some();
    trackers.lap_curve.observe(
        me_lap_dist_pct,
        session_time_secs,
        matches!(me_track_location, TrackLocation::OnTrack | TrackLocation::OffTrack),
    );
    // Said once per improvement, and never in the ordinary case: whether the
    // curve is live is the difference between gaps good to a hundredth and gaps
    // that breathe through every corner, and it is otherwise invisible from
    // the driver's seat.
    if let Some(measured) = trackers.lap_curve.measured_lap_secs()
        && trackers.curve_reported_secs != Some(measured)
    {
        trackers.curve_reported_secs = Some(measured);
        let note = if curve_was_ready { "sharpened" } else { "ready" };
        println!("note: relative gap curve {note} from a {measured:.3}s lap");
    }

    // The player's own pace: what the lap and stop projections divide the
    // session clock by, and the fallback scale for relative gaps until the
    // reference lap below has been measured.
    //
    // Clamped to a band above their best lap — see [`race_pace_secs`].
    let pace_secs = race_pace_secs(best_lap_secs, avg_lap_secs, lap_secs);

    // How many seconds a lap of the curve's shape is worth, which is what turns
    // a fraction of it into a gap. Measured off iRacing's own est-times rather
    // than taken from the player's pace, so the column reads the size the
    // in-sim Relative reads; see `relative::ReferenceLap` for why the two are
    // not the same number and what taking the wrong one costs.
    let me_lap_fraction = me_lap_dist_pct.and_then(|pct| trackers.lap_curve.lap_fraction(pct));
    let reference_lap_secs = trackers.reference_lap.observe(me_est_time, me_lap_fraction, pace_secs);
    let gap_scale_secs = reference_lap_secs.unwrap_or(pace_secs);
    // Said once, like the curve's own note: the difference between this and the
    // player's pace is the difference between a column that matches the in-sim
    // Relative and one that reads every row ten percent long, and there is no
    // way to tell which you are looking at from the driver's seat.
    if let Some(reference) = reference_lap_secs
        && !trackers.reference_reported
    {
        trackers.reference_reported = true;
        println!("note: relative gaps scaled by a {reference:.3}s reference lap");
    }

    let session_state = vars.session_state.as_ref().and_then(|v| {
        // SAFETY: see the closures at the top of this function.
        unsafe { session.value::<SessionState>(v) }.ok()
    });
    // A race's clock is the racing, not the queueing: gridding and the pace
    // laps come before any of it counts. Anything from the green onwards is
    // under way, the flag included — a race in its cool-down lap has
    // certainly started. Always true where the sim publishes no state.
    let racing_under_way = session_state
        .is_none_or(|state| matches!(state, SessionState::Racing | SessionState::Checkered | SessionState::CoolDown));
    // Rated only in a race. Practice and qualifying have no finishing order
    // — every car is unscored, and cars come and go — and rating them produced
    // four-figure swings as people joined.
    let session_kind = session_type.as_deref().map(SessionKind::from_name).unwrap_or_default();
    let is_race = session_kind.is_race();
    // Which order the field is ranked in — see [`StandingsOrder`]:
    // - Between the green and the checkered, how far each car has driven.
    //   `Racing` (which cautions stay inside) is exactly the stretch where a
    //   mid-lap pass is a change of position; see [`apply_distance_order`].
    // - While a race's field forms up, the grid qualifying set — iRacing's
    //   own slots fill in one by one as cars grid, so whoever grids first
    //   reads P1 until then.
    // - In practice and qualifying, quickest lap first.
    // - Otherwise — post-checkered, or a state the sim didn't publish — the
    //   scorer's order, which can never invent a position.
    let order = if is_race && matches!(session_state, Some(SessionState::Racing)) {
        StandingsOrder::Distance(&mut trackers.race_order)
    } else if is_race
        && !info.qualify_grid.is_empty()
        && matches!(session_state, Some(SessionState::GetInCar | SessionState::Warmup | SessionState::ParadeLaps))
    {
        StandingsOrder::Grid
    } else if matches!(session_kind, SessionKind::Practice | SessionKind::Qualifying | SessionKind::Warmup) {
        StandingsOrder::Quickest
    } else {
        StandingsOrder::Scored
    };
    let mut standings = build_standings(
        info,
        session_num,
        focus_car_idx,
        StandingsRawArrays {
            best_laps,
            last_laps,
            f2_times,
            track_surfaces,
            laps,
            positions,
            lap_dist_pcts,
            session_flags: session_flags_per_car,
        },
        &mut trackers.stint,
        &mut trackers.off_tracks,
        &mut trackers.laps,
        session_time_secs,
        order,
    );
    // How the race has treated each car. Races only: in practice and
    // qualifying "places since the start" would measure the order lap times
    // happened to arrive in, which isn't a story worth a column.
    if is_race {
        for entry in &mut standings {
            let grid_slot = info.qualify_grid.get(&entry.car_idx).map(|(_, class_slot)| *class_slot);
            entry.race_position_change =
                trackers.position_change.change(entry.car_idx, entry.class_position, racing_under_way, grid_slot);
        }
    }
    // Built from the whole field's live standings (not just the nearby
    // subset shown in Relative), since the estimate is more meaningful with
    // full-field context; see `telemetry::irating` for what this is and isn't.
    //
    // Borrowed from the tracker rather than returned owned, so a tick that
    // changes nothing rebuilds nothing. The borrow is of one field, which
    // leaves the rest of `trackers` free for the loop below to use.
    let irating_changes =
        trackers.irating.changes(&standings, &info.drivers, racing_under_way, is_race, session_time_secs);
    // The number every Relative row shows. In a single-class race it is the
    // overall position; in a multi-class one it is the only position a driver
    // is actually racing for, and showing the overall number instead puts a
    // GT3 driver's "P14" next to the LMP2 car lapping them. Taken from
    // `standings`, which derives a contiguous per-class order from live
    // telemetry rather than trusting the YAML's own class numbering — see
    // `build_standings`.
    let class_positions_by_car: HashMap<i32, i32> =
        standings.iter().map(|entry| (entry.car_idx, entry.class_position)).collect();
    // Reuses the same buffer both SOF calls below read from, so the focus
    // car's class is not collected twice per tick.
    let mut class_iratings: Vec<i32> = Vec::new();
    let sof = my_car_class_id.and_then(|class_id| {
        class_iratings.extend(standings.iter().filter(|e| e.car_class_id == class_id).map(|e| e.irating));
        trackers.sof.sof(class_id, &class_iratings)
    });
    // In a team event the limit is applied to the team's count, which is the
    // one that ends the race; the counters are equal everywhere else.
    let incidents = if info.team_racing {
        i32_of_opt(&vars.team_incidents).or_else(|| i32_of_opt(&vars.player_incidents))
    } else {
        i32_of_opt(&vars.player_incidents)
    }
    .unwrap_or(0);

    let count =
        positions.len().min(class_positions.len()).min(track_surfaces.len()).min(est_times.len()).min(laps.len());
    let mut cars = Vec::with_capacity(count);
    // Radar reuses the relative-time gaps computed in this loop rather than
    // deriving its own from track-distance percentages: one gap definition for
    // both widgets means the bars can never disagree with the row a driver is
    // reading three inches away. It is collected separately because it takes
    // every car that can be hit, including the ones Relative leaves out.
    let mut radar_contacts: Vec<radar::Contact> = Vec::with_capacity(count);
    // Which classes are quicker than the focus car's, for the Faster Class
    // widget: iRacing's own ranking where the YAML gives one, the quickest lap
    // seen per class otherwise — see `faster_class::is_faster_class`. Every
    // sighting is `(car index, seconds behind)`; the widget applies its own
    // thresholds to the list, so nothing here depends on a setting.
    let class_best_laps = class_best_laps(&info.drivers, best_laps);
    let my_class_pace = info.drivers.get(&focus_car_idx).map(|driver| class_pace(driver, &class_best_laps));
    // In the pit lane, past your own stall, you are on the way out — about
    // to merge into exactly the traffic the faster-class warning is for, so
    // the lane's silence (see below) is lifted for that stretch. Half a lap
    // is generous for any pit lane, and the stall fraction wraps at the
    // line. The stall is the player's own — spectated cars' stalls aren't
    // published, and their lane stays quiet throughout.
    let leaving_pit_lane = me_track_location == TrackLocation::ApproachingPits
        && focus_is_player
        && info.pit_stall_pct > 0.0
        && me_lap_dist_pct.is_some_and(|pct| (pct - info.pit_stall_pct).rem_euclid(1.0) < 0.5);
    let mut faster_sightings: Vec<(i32, f32)> = Vec::new();
    for i in 0..count {
        let car_idx = i32::try_from(i).unwrap_or(i32::MAX);
        if car_idx == focus_car_idx {
            continue;
        }
        let Some(driver) = info.drivers.get(&car_idx) else {
            continue;
        };
        let track_location = track_location_from_raw(track_surfaces[i]);
        if track_location == TrackLocation::NotInWorld {
            continue; // no car in this slot
        }
        let gap_to_player_secs = relative::gap_seconds(
            &trackers.lap_curve,
            me_lap_dist_pct,
            lap_dist_pcts.get(i).copied(),
            me_est_time,
            est_times[i],
            gap_scale_secs,
        );
        radar_contacts.push(radar::Contact {
            gap_secs: gap_to_player_secs,
            gap_m: radar_separation_metres(
                me_lap_dist_pct,
                lap_dist_pcts.get(i).copied(),
                info.track_length_m,
                gap_to_player_secs,
            ),
        });
        // The pace car and any spectator are on track but not in the race.
        // Radar above still wants them — a pace car is as solid as anything
        // else — and so does it want the field during lone qualifying, where
        // a car pulling out of its box is exactly what you need warning of.
        // Neither belongs in the running order below.
        if !driver.is_competitor || lone_qualifying {
            continue;
        }
        // A quicker class coming up behind. Not from the pit lane: a car in
        // its box is not arriving, however quick its class. Lap difference is
        // deliberately no criterion — a prototype a lap down after a repair is
        // still a prototype, and still coming through.
        //
        // And not *to* the pit lane either: while the focus car is in it, the
        // whole field streams past the pit wall a few metres away, and every
        // quicker car reads as "arriving" at a car that is parked. Nobody
        // needs warning of traffic they are not in — until the exit stretch,
        // where they are seconds from being in it; see `leaving_pit_lane`.
        let behind_secs = -gap_to_player_secs;
        if let Some(mine) = my_class_pace
            && (matches!(me_track_location, TrackLocation::OnTrack | TrackLocation::OffTrack) || leaving_pit_lane)
            && matches!(track_location, TrackLocation::OnTrack | TrackLocation::OffTrack)
            && faster_class::in_scan(behind_secs)
            && faster_class::is_faster_class(class_pace(driver, &class_best_laps), mine)
        {
            faster_sightings.push((car_idx, behind_secs));
        }
        // Being in the world is the only test for whether a car belongs in
        // Relative. There used to be a second one — skip anything iRacing
        // hasn't scored — because an unscored car's lap count could send its
        // gap to several hundred seconds of nonsense. `gap_seconds` can't
        // produce that any more (it is bounded to half a lap by
        // construction), and the test was throwing away real cars: every car
        // on a grid, and any car whose position iRacing zeroes while it sits
        // in the pits.
        cars.push(CarSnapshot {
            car_idx,
            cust_id: driver.user_id.and_then(|id| u32::try_from(id).ok()),
            position: class_positions_by_car.get(&car_idx).copied().unwrap_or(positions[i]),
            track_location,
            gap_to_player_secs,
            driver_name: Arc::clone(&driver.user_name),
            car_number: Arc::clone(&driver.car_number),
            car_screen_name: Arc::clone(&driver.car_screen_name),
            irating: driver.irating,
            license_color: Arc::clone(&driver.license_color),
            car_class_color: Arc::clone(&driver.car_class_color),
            flair_id: driver.flair_id,
            is_fastest_overall: fastest_overall_idx == Some(i),
            irating_change_estimate: irating_changes.get(&car_idx).copied(),
            is_focus: false,
            off_tracks: trackers.off_tracks.count(car_idx),
            lap_diff: relative::lap_difference(me_lap, me_lap_dist_pct, laps[i], lap_dist_pcts.get(i).copied()),
            best_recent_lap_secs: trackers.recent_laps.update(
                car_idx,
                laps[i],
                last_laps.get(i).copied().unwrap_or(0.0),
            ),
            recent_laps: trackers.recent_laps.samples(car_idx),
            penalty: session_flags_per_car.get(i).copied().and_then(relative::penalty_from_flags),
        });
    }

    if let (Some(i), Some(driver)) = (focus_idx, info.drivers.get(&focus_car_idx)) {
        cars.push(CarSnapshot {
            car_idx: focus_car_idx,
            cust_id: driver.user_id.and_then(|id| u32::try_from(id).ok()),
            position: class_positions_by_car
                .get(&focus_car_idx)
                .copied()
                .unwrap_or_else(|| positions.get(i).copied().unwrap_or(0)),
            track_location: track_surfaces.get(i).copied().map_or(TrackLocation::NotInWorld, track_location_from_raw),
            gap_to_player_secs: 0.0,
            driver_name: Arc::clone(&driver.user_name),
            car_number: Arc::clone(&driver.car_number),
            car_screen_name: Arc::clone(&driver.car_screen_name),
            irating: driver.irating,
            license_color: Arc::clone(&driver.license_color),
            car_class_color: Arc::clone(&driver.car_class_color),
            flair_id: driver.flair_id,
            is_fastest_overall: focus_idx == fastest_overall_idx,
            irating_change_estimate: irating_changes.get(&focus_car_idx).copied(),
            is_focus: true,
            off_tracks: trackers.off_tracks.count(focus_car_idx),
            lap_diff: 0,
            best_recent_lap_secs: trackers.recent_laps.update(
                focus_car_idx,
                me_lap,
                last_laps.get(i).copied().unwrap_or(0.0),
            ),
            recent_laps: trackers.recent_laps.samples(focus_car_idx),
            penalty: session_flags_per_car.get(i).copied().and_then(relative::penalty_from_flags),
        });
    }
    // Ordered after the focus car is added, so its own row takes its place in
    // the field rather than being spliced into the middle afterwards.
    let relative_rows = relative::order_by_gap(&cars, &mut trackers.row_order);
    let focus_index = relative_rows.iter().position(|car| car.is_focus).unwrap_or(0);

    let session_time_remain_secs =
        f64_of_opt(&vars.session_time_remain).filter(|&remain| (remain - UNLIMITED_SESSION_SENTINEL_SECS).abs() > 1.0);

    // An approximation (hence "~" in the UI), not an official lap count.
    //
    // `pace_secs`, not the raw `avg_lap_secs` — see `laps_remaining` below for
    // what taking the unclamped average here costs.
    let predicted_total_laps =
        endurance::projected_total_laps(me_lap, me_lap_dist_pct, session_time_remain_secs, pace_secs);
    // Elapsed is derived from the session's scheduled length rather than read
    // from `SessionTime`, so that it and `race_remain_secs` measure the same
    // race. `SessionTime` counts from when the session loaded, which puts
    // gridding and the pace laps into it; pairing that with a remaining time
    // that counts only the racing made a 40-minute race read as 44. Falls back
    // to the raw counter for a session with no scheduled length, where there
    // is nothing better and no total is shown anyway.
    let session_length_secs = this_session.and_then(|s| parse_session_seconds(&s.session_time));
    let race_elapsed_secs = match (session_length_secs, session_time_remain_secs) {
        (Some(length), Some(remain)) => (length - remain).max(0.0),
        _ => session_time_secs,
    };
    let session_kind = session_type.as_deref().map(SessionKind::from_name).unwrap_or_default();
    // The sim's own laps-to-go where the session has a lap limit, alongside
    // the projection from the clock. `SessionLapsRemainEx` counts the lap in
    // progress — it reads 1 on the last lap, not 0 — which is the same
    // convention as [`endurance::laps_remaining`], so the two can be compared
    // directly, and a session bounded by both takes whichever runs out first.
    let laps_by_count =
        i32_of_opt(&vars.session_laps_remain_ex).filter(|laps| (0..UNLIMITED_LAPS_SENTINEL).contains(laps));
    // `pace_secs` rather than `avg_lap_secs`, and this is load-bearing: the
    // clamp that `pace_secs` carries is the whole reason a single in-lap cannot
    // move a projection, and this call — the one every fuel load in the race is
    // built on — was the one place still reading the raw rolling average.
    //
    // A lap with a pit stop standing in it reads two or three times a green
    // lap, and dividing the session clock by that halves the laps left: a
    // three-hour race at Hockenheim read 36 laps remaining where it had 90-odd,
    // so Auto Fuel worked out a load for a third of the race and put half a tank
    // in. Erring the other way is free — `pace_secs` can never read quicker than
    // the driver's own best lap, so the laps left can never come out short.
    let laps_by_clock = endurance::laps_remaining(session_time_remain_secs, pace_secs, me_lap_dist_pct);
    let laps_remaining = race_laps_remaining(session_kind, laps_by_count, laps_by_clock);
    let relative_meta = RelativeMeta {
        sof,
        session_kind,
        car_count: i32::try_from(standings.len()).unwrap_or(i32::MAX),
        racing_under_way,
        session_length_secs,
        // The course-wide bits only. A local yellow slows one corner; a
        // caution rewrites every gap in the race at once, which is the one
        // the pit window has to refuse to project through.
        under_caution: vars
            .session_flags
            .as_ref()
            .and_then(|v| {
                // SAFETY: see the closures at the top of this function.
                unsafe { session.value::<Flags>(v) }.ok()
            })
            .is_some_and(|flags| flags.intersects(Flags::CAUTION | Flags::CAUTION_WAVING)),
        // Both names come from the seat, resolved once above, so the header
        // and the page set can never disagree about who is where.
        spectating: match &seat {
            Seat::Spectating(name) => Some(Arc::clone(name)),
            _ => None,
        },
        team_mate: match &seat {
            Seat::TeamMate(name) => Some(Arc::clone(name)),
            _ => None,
        },
        incidents,
        incident_limit: info.incident_limit,
        race_elapsed_secs,
        race_remain_secs: session_time_remain_secs,
        current_lap: me_lap,
        predicted_total_laps,
        session_laps: this_session.and_then(|s| parse_session_laps(&s.session_laps)),
        grid: build_grid_status(
            session_state,
            session_kind.is_race(),
            &info.drivers,
            track_surfaces,
            session_time_remain_secs,
        ),
    };

    let radar = build_radar(
        car_left_right,
        &radar_contacts,
        tuning.radar_range_secs,
        session_time_secs,
        &mut trackers.radar_smoothing,
    );
    let faster_class =
        build_faster_class(&mut faster_sightings, info, session_time_secs, &mut trackers.faster_class_rates);
    let pit_projection = build_pit_projection(&standings, tuning.pit_loss_secs, my_car_class_id);
    let bool_of = |var: &Option<Var>| {
        var.as_ref()
            // SAFETY: see the closures at the top of this function.
            .and_then(|v| unsafe { session.value::<bool>(v) }.ok())
            .unwrap_or(false)
    };
    let car_yaw_rad = f32_of_opt(&vars.yaw);
    let precip_now = f32_of_opt(&vars.precipitation);
    let track_wetness = i32_of_opt(&vars.track_wetness).and_then(TrackWetness::from_raw);
    // "The track is wet" for compound naming: either the sim's own wetness
    // estimate says so, or rain is measurably falling.
    let track_wet =
        track_wetness.is_some_and(TrackWetness::is_wet) || precip_now.is_some_and(|p| p > WET_TRACK_PRECIP_MIN);
    // The per-row facts that need this tick-wide context: the tow marker
    // (the tracker lives across ticks) and the tyre column (whose wet ring
    // depends on the track's own wetness, read just above).
    let tire_compounds = i32_slice_of_opt(&vars.car_idx_tire_compound);
    // The player's own stall flag, read here rather than with the rest of the
    // black box below because their stop count is settled before the per-row
    // pass — see [`PlayerStops`].
    let player_in_pit_stall = bool_of(&vars.black_box.player_in_pit_stall);
    let player_stops = trackers.player_stops.update(PlayerStopTick {
        session_time_secs,
        // Pit road whole, not the box alone: a drive-through is a visit to it
        // too, and telling those apart is the whole job of the tracker.
        on_pit_road: matches!(player_track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits),
        in_box: vars.black_box.player_in_pit_stall.as_ref().map(|_| player_in_pit_stall),
        speed_mps: f32_of_opt(&vars.speed),
        in_world: player_track_location != TrackLocation::NotInWorld,
        official_stops: this_session
            .and_then(|results| results.results_positions.iter().find(|row| row.car_idx == player_car_idx))
            .map(|row| row.pit_stops),
    });
    for entry in &mut standings {
        // Their own row takes the count measured from their own car; every
        // other row keeps the inference drawn from track position.
        if entry.car_idx == player_car_idx {
            entry.pit_stops = player_stops;
        }
        entry.tow_secs =
            trackers.tow.tow_secs(entry.car_idx, entry.track_location != TrackLocation::NotInWorld, session_time_secs);
        let compound = usize::try_from(entry.car_idx)
            .ok()
            .and_then(|i| tire_compounds.get(i))
            .copied()
            // The sim always says which compound the *player's* car wears,
            // so their own row never goes blank even where the per-car
            // array isn't published.
            .or_else(|| (entry.car_idx == player_car_idx).then(|| i32_of_opt(&vars.player_tyre_compound)).flatten());
        entry.tyre = tyre_compound_of(compound, &info.tyre_letter_by_index, &info.tyre_is_wet_by_index, track_wet);
    }
    let weather = WeatherSnapshot {
        track_temp_c: f32_of_opt(&vars.track_temp).unwrap_or(0.0),
        air_temp_c: f32_of_opt(&vars.air_temp).unwrap_or(0.0),
        wind_speed_mps: f32_of_opt(&vars.wind_vel).unwrap_or(0.0),
        fog: f32_of_opt(&vars.fog_level),
        precip_chance: info.precip_chance,
        precip_now,
        declared_wet: bool_of(&vars.declared_wet),
        track_wetness,
        on_wet_tyres: on_wet_tyres(i32_of_opt(&vars.player_tyre_compound), &info.tyre_is_wet_by_index, track_wet),
        wind_dir_relative_to_car_rad: weather::wind_direction_relative_to_car(f32_of_opt(&vars.wind_dir), car_yaw_rad),
    };
    let class_sections = build_class_sections(&standings, &mut trackers.sof);
    // Settle the lane measurement before NET consumes it on the exit tick.
    let phase = match me_track_location {
        TrackLocation::InPitStall => pit_model::LanePhase::Stall,
        TrackLocation::ApproachingPits => pit_model::LanePhase::Lane,
        TrackLocation::OnTrack | TrackLocation::OffTrack => pit_model::LanePhase::OnTrack,
        TrackLocation::NotInWorld => pit_model::LanePhase::Away,
    };
    if let Some(transit) = trackers.pit_loss.update(phase, session_time_secs, me_lap_fraction, pace_secs) {
        println!("note: pit lane transit measured at {transit:.1}s");
    }
    let pit_model = trackers.pit_loss.model();
    let net_gaps = live_net_gaps(&standings, laps, lap_dist_pcts, &trackers.lap_curve, my_car_class_id);
    let mut endurance_meta = annotate_endurance(
        &mut standings,
        laps_remaining,
        tuning.pit_loss_secs,
        my_car_class_id,
        &mut trackers.multi_stop_race,
        pit_model,
        &net_gaps,
    );
    // `laps_remaining` counts the lap under way whole; this is the part of it
    // already behind the car, which the fuel target subtracts — see
    // `EnduranceMeta::lap_driven_pct`.
    endurance_meta.lap_driven_pct = me_lap_dist_pct.map(|pct| pct.clamp(0.0, 1.0));

    // Every black box read goes through the same optional-var accessors, so a
    // car that doesn't publish one simply reports a default rather than
    // failing the whole tick.
    let bb = &vars.black_box;
    let flag_of = |var: &Option<Var>| f32_of_opt(var).is_some_and(|v| v > 0.5);
    let fuel_level_litres = f32_of_opt(&bb.fuel_level).unwrap_or(0.0);
    let fuel_armed = flag_of(&bb.fuel_armed);
    let fuel_amount_litres = f32_of_opt(&bb.fuel_amount).unwrap_or(0.0);
    let pit_service = pit::PitService {
        fuel_armed,
        fuel_amount_litres,
        fuel_level_litres,
        // The YAML's own figure first; the inference from level and percent
        // only stands in where a session publishes no tank, and is guarded
        // against a near-empty one, where dividing two small numbers swings
        // wildly.
        tank_capacity_litres: trackers.tank_capacity.latch(info.tank_capacity_litres.or_else(|| {
            f32_of_opt(&bb.fuel_level_pct).filter(|pct| *pct > TANK_ESTIMATE_MIN_PCT).map(|pct| fuel_level_litres / pct)
        })),
        // Pit road, not just the box: an in-lap or out-lap is part pit lane at
        // pit-lane speed, and its fuel use is no guide to a racing lap's.
        // The lap time comes straight from `LapLastLapTime` rather than the
        // focus-aware `last_lap_secs` above, because the tank being measured
        // is the player's car whatever the camera is doing.
        fuel_per_lap_litres: trackers.fuel.update(
            me_lap,
            fuel_level_litres,
            f32_of_opt(&vars.lap_last_lap_time).unwrap_or(0.0),
            on_pit_road,
        ),
        tyres_armed: std::array::from_fn(|i| flag_of(&bb.tyre_change[i])),
        tyre_pressures_kpa: std::array::from_fn(|i| f32_of_opt(&bb.tyre_pressure[i]).unwrap_or(0.0)),
        pending_tyre_compound: i32_of_opt(&bb.tyre_compound),
        tearoff_armed: flag_of(&bb.tearoff),
        fast_repair_armed: flag_of(&bb.fast_repair_armed),
        fast_repairs_available: i32_of_opt(&bb.fast_repairs_available).unwrap_or(0),
        in_car: driving,
        on_pit_road,
        // An unticked fuel box means nothing is set to go in, whatever stale
        // figure `PitSvFuel` still reads.
        refuel_target_litres: trackers.refuel.update(
            on_pit_road,
            fuel_level_litres,
            if fuel_armed { fuel_amount_litres } else { 0.0 },
        ),
    };

    // Wear and carcass temperature are read straight through: the sim already
    // holds them at their last-stop values, so a second latch here could only
    // ever freeze something staler. See [`StopPressures`].
    let wear: [[f32; 3]; 4] =
        std::array::from_fn(|i| std::array::from_fn(|e| f32_of_opt(&bb.tyre_wear[i][e]).unwrap_or(0.0)));
    let stop_kpa = trackers
        .stop_pressures
        .update(wear, std::array::from_fn(|i| f32_of_opt(&bb.tyre_hot_pressure[i]).unwrap_or(0.0)));
    let tyres = TyreInfo {
        corners: std::array::from_fn(|i| TyreState {
            temps_c: std::array::from_fn(|e| f32_of_opt(&bb.tyre_temps[i][e]).unwrap_or(0.0)),
            wear: wear[i],
            pressure_kpa: stop_kpa.map_or_else(|| f32_of_opt(&bb.tyre_cold_pressure[i]).unwrap_or(0.0), |kpa| kpa[i]),
        }),
    };
    let adjustments = CarAdjustments {
        brake_bias: f32_of_opt(&bb.brake_bias),
        abs: f32_of_opt(&bb.abs),
        traction_control: f32_of_opt(&bb.traction_control),
        throttle_shape: f32_of_opt(&bb.throttle_shape),
        dash_page: f32_of_opt(&bb.dash_page),
    };

    // The player's own identity for team sync. The car-index lookup gives
    // the entry's current driver name; in a team race that is whoever is in
    // the seat, which is exactly who a "set by" note should read.
    let identity = crate::telemetry::snapshot::SessionIdentity {
        subsession: info.sub_session_id,
        player_cust_id: info.driver_user_id.and_then(|id| u32::try_from(id).ok()),
        player_name: info.drivers.get(&player_car_idx).map(|meta| Arc::clone(&meta.user_name)),
    };

    // The status border's inputs: the course flag, the fuel-based box call,
    // and how far the player is from their own stall (for the approach pulse).
    let course_flag = crate::telemetry::relative::course_flag_from_bits(i32_of_opt(&vars.session_flags).unwrap_or(0));
    // Only while driving: out of the car, `FuelLevel` and the burn tracker
    // describe a tank that isn't ours (spectating follows the camera car's
    // laps against the player's dead fuel var), which called BOX BOX every
    // lap. Spectators watching a teammate get the call recomputed from the
    // team-synced tank instead.
    let box_this_lap = seat == Seat::Driving
        && me_lap_fraction.is_some_and(|fraction| {
            crate::telemetry::endurance::box_this_lap(
                pit_service.fuel_level_litres,
                pit_service.fuel_per_lap_litres,
                fraction,
            )
        });
    // Forward distance along the lap to the player's stall. `rem_euclid` wraps
    // the fraction so a stall just past the line still reads as ahead, not as
    // most of a lap behind.
    let metres_to_pit = match (me_lap_dist_pct, info.track_length_m) {
        (Some(pct), Some(length)) if info.pit_stall_pct > 0.0 && length > 0.0 => {
            Some((info.pit_stall_pct - pct).rem_euclid(1.0) * length)
        }
        _ => None,
    };

    TelemetrySnapshot {
        in_garage: bool_of(&vars.in_garage),
        seat,
        identity,
        session_time_secs,
        course_flag,
        box_this_lap,
        metres_to_pit,
        relative: relative_rows,
        focus_index,
        relative_meta,
        standings,
        class_sections,
        focus_car_class_id: my_car_class_id,
        endurance: endurance_meta,
        radar,
        faster_class,
        weather,
        pit_projection,
        pit_service,
        tyres,
        adjustments,
        fuel_use: crate::telemetry::snapshot::FuelUse {
            // Measured against the *lap curve*, not track position: fuel burns
            // with time under power, so a distance-based comparison reads rich
            // down every straight and lean through every corner — the same error
            // that made track-position relative gaps breathe.
            used_this_lap_litres: trackers.lap_fuel_use.update(
                me_lap,
                pit_service.fuel_level_litres,
                pit_service.on_pit_road,
            ),
            lap_fraction: me_lap_fraction,
        },
        pit_model,
    }
}

/// The seconds-per-lap every projection in this app divides the session clock
/// by: the player's rolling average, held to a band above their own best lap.
///
/// **Nothing that projects forward may use the raw average.** The rolling
/// window takes in whatever laps the driver has just done, and one of those is
/// regularly a lap with a pit stop standing in it — two or three times a green
/// lap. Divide a session clock by that and the laps remaining halve, which is
/// not a cosmetic error: it is the number the fuel load is worked out from, so
/// the car goes back out with a third of the fuel the race needs. A three-hour
/// race at Hockenheim read 36 laps left where it had ninety-odd, for exactly
/// this reason.
///
/// The clamp is one-sided in the direction that is safe. `best` is a floor as
/// well as a ceiling band, so the pace can never read *quicker* than a lap the
/// driver has actually turned, and the laps remaining can therefore never come
/// out short. Being a few laps long costs a couple of litres of ballast.
///
/// `fallback_secs` covers the opening laps, where neither a best lap nor a
/// completed lap exists and the only estimate is from iRacing's est-times.
#[must_use]
fn race_pace_secs(best_lap_secs: Option<f32>, avg_lap_secs: f32, fallback_secs: f32) -> f32 {
    match best_lap_secs {
        Some(best) if avg_lap_secs > 0.0 => avg_lap_secs.clamp(best, best * PACE_OVER_BEST_LIMIT),
        Some(best) => best,
        None if avg_lap_secs > 0.0 => avg_lap_secs,
        None => fallback_secs,
    }
}

/// Stops still owed at which a session counts as an endurance race. Two,
/// because a one-stop sprint needs none of the strategy columns while
/// anything longer turns on stop count rather than pace.
const MULTI_STOP_THRESHOLD: i32 = 2;

/// The grid's state before a race, or `None` once the field is rolling.
///
/// `GetInCar` is the gridding itself and `Warmup` the moment after it closes,
/// before the pace car pulls away; both are "the grid" to a driver waiting on
/// it. A car has gridded when it exists in the world at all — the track
/// surface reads `NotInWorld` for one still in the garage or at the entry
/// screen. Only competitors count: the pace car is always out first and never
/// gridding.
///
/// The countdown is `SessionTimeRemain` taken only while it is plausibly one
/// — see [`GRID_COUNTDOWN_MAX_SECS`]. A lap-limited race reads the unlimited
/// sentinel instead, which the caller has already filtered to `None`, so that
/// race shows the count of cars and no clock.
fn build_grid_status(
    state: Option<SessionState>,
    is_race: bool,
    drivers: &HashMap<i32, DriverMeta>,
    track_surfaces: &[i32],
    time_remain_secs: Option<f64>,
) -> Option<GridStatus> {
    if !is_race || !matches!(state, Some(SessionState::GetInCar | SessionState::Warmup)) {
        return None;
    }
    let mut car_count = 0;
    let mut cars_gridded = 0;
    for (&car_idx, driver) in drivers {
        if !driver.is_competitor {
            continue;
        }
        car_count += 1;
        let in_world = usize::try_from(car_idx)
            .ok()
            .and_then(|i| track_surfaces.get(i))
            .is_some_and(|&raw| track_location_from_raw(raw) != TrackLocation::NotInWorld);
        if in_world {
            cars_gridded += 1;
        }
    }
    let countdown_secs = time_remain_secs.filter(|secs| (0.0..=GRID_COUNTDOWN_MAX_SECS).contains(secs));
    Some(GridStatus { cars_gridded, car_count, countdown_secs })
}

/// NET needs current race distance, not the last scoring-line gap. A missing
/// reading stays missing; zero would invent a car alongside its class leader.
fn live_net_gaps(
    standings: &[StandingsEntry],
    laps: &[i32],
    pcts: &[f32],
    curve: &relative::LapCurve,
    curve_class_id: Option<i32>,
) -> HashMap<i32, f32> {
    use super::net_position::{RaceProgress, live_gap_secs};
    let progress = |entry: &StandingsEntry| {
        let i = usize::try_from(entry.car_idx).ok()?;
        Some(RaceProgress { lap: *laps.get(i)?, pct: *pcts.get(i)? })
    };
    let fallback_curve = relative::LapCurve::default();
    let mut gaps = HashMap::new();
    for representative in standings.iter().filter(|entry| entry.class_position == 1) {
        // Use the furthest active car as the common origin. The displayed
        // order intentionally holds close passes with hysteresis and can
        // therefore name a leader a little behind another car on this tick.
        let leader = standings
            .iter()
            .filter(|entry| {
                entry.car_class_id == representative.car_class_id && entry.track_location != TrackLocation::NotInWorld
            })
            .filter_map(|entry| {
                progress(entry)
                    .filter(|p| p.lap >= 0 && p.pct.is_finite() && (0.0..=1.0).contains(&p.pct))
                    .map(|p| (entry, p))
            })
            .max_by(|(_, a), (_, b)| a.lap.cmp(&b.lap).then(a.pct.total_cmp(&b.pct)))
            .map(|(entry, _)| entry);
        let Some(leader) = leader else { continue };
        let Some(leader_progress) = progress(leader) else { continue };
        let class_pace = standings
            .iter()
            .filter(|entry| entry.car_class_id == leader.car_class_id)
            .map(|entry| entry.best_lap_secs)
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .min_by(f32::total_cmp);
        let Some(pace) = class_pace else { continue };
        // A different class may spend very different fractions of its lap in
        // each corner; do not apply the focus car's measured curve to it.
        let class_curve = if curve_class_id == Some(leader.car_class_id) { curve } else { &fallback_curve };
        for entry in standings.iter().filter(|entry| entry.car_class_id == leader.car_class_id) {
            if let Some(gap) = progress(entry).and_then(|car| live_gap_secs(class_curve, leader_progress, car, pace)) {
                gaps.insert(entry.car_idx, gap);
            }
        }
    }
    gaps
}

/// The race's remaining laps, using whichever finish limit arrives first.
/// A practice or qualifying clock is access to the track, not a race distance
/// every car must complete. It must never create fuel or stop-to-finish plans.
fn race_laps_remaining(
    session_kind: SessionKind,
    laps_by_count: Option<i32>,
    laps_by_clock: Option<i32>,
) -> Option<i32> {
    if !session_kind.is_race() {
        return None;
    }
    match (laps_by_count, laps_by_clock) {
        (Some(count), Some(clock)) => Some(count.min(clock)),
        (count, clock) => count.or(clock),
    }
}

/// Fills in every entry's endurance projection and returns the session-level
/// summary for the player.
///
/// Each car's remaining stops come from its own stint history — or, where it
/// has none yet, from its class's typical stint — and its projected position
/// from re-sorting its class on current gap plus the cost of the stops it
/// still owes. Measured service time is added to lane transit, never used
/// as a replacement for the total loss. Gaps include whole laps and update
/// with track position, independently of the scorer's F2 timing updates.
fn annotate_endurance(
    standings: &mut [StandingsEntry],
    laps_remaining: Option<i32>,
    default_pit_loss_secs: f32,
    my_car_class_id: Option<i32>,
    multi_stop_race: &mut bool,
    pit_model: pit_model::PitModel,
    net_gaps: &HashMap<i32, f32>,
) -> EnduranceMeta {
    for entry in standings.iter_mut() {
        entry.stops_remaining = None;
        entry.projected_class_position = None;
    }
    let Some(laps_left) = laps_remaining else {
        return EnduranceMeta { multi_stop_race: *multi_stop_race, ..EnduranceMeta::default() };
    };

    for entry in standings.iter_mut() {
        entry.stops_remaining =
            entry.avg_stint_laps.and_then(|avg| endurance::stops_remaining(laps_left, entry.current_stint_laps, avg));
    }

    // Projections are class-relative, matching the rest of the widget.
    let class_ids: Vec<i32> = {
        let mut ids: Vec<i32> = standings.iter().map(|e| e.car_class_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    for class_id in class_ids {
        // NET ranks the active field. An absent car has no forward strategy
        // to project; leave its NET unknown without blanking every rival.
        // If it rejoins, it participates again on the next valid reading.
        let indices: Vec<usize> = standings
            .iter()
            .enumerate()
            .filter(|(_, e)| e.car_class_id == class_id && e.track_location != TrackLocation::NotInWorld)
            .map(|(i, _)| i)
            .collect();
        // A class where nobody has a stint history yet has nothing to
        // project; leaving the field `None` keeps the column blank rather
        // than showing everyone their current position as a "projection".
        if indices.iter().all(|&i| standings[i].stops_remaining.is_none()) {
            continue;
        }
        // A car without a stint history of its own — it hasn't pitted yet, or
        // its only stop was for damage — is projected on the class's typical
        // stint instead. Projecting it with zero stops ranked every unpitted
        // car as if it would never pit, which is the one answer known to be
        // wrong: everyone in a class runs broadly the same tank.
        let class_typical_stint = lower_median_i32(indices.iter().filter_map(|&i| standings[i].avg_stint_laps));
        for &i in &indices {
            let entry = &mut standings[i];
            entry.stops_remaining = entry
                .avg_stint_laps
                .or(class_typical_stint)
                .and_then(|avg| endurance::stops_remaining(laps_left, entry.current_stint_laps, avg));
        }
        // An ongoing visit has already lost part of its time, but its stint
        // has not settled yet. Publishing a rank would charge it twice.
        // Resume as soon as every car in this class has a usable exit reading.
        if indices.iter().any(|&i| {
            let entry = &standings[i];
            !matches!(entry.track_location, TrackLocation::OnTrack | TrackLocation::OffTrack)
                || !net_gaps.contains_key(&entry.car_idx)
        }) {
            continue;
        }
        let class_service = lower_median_f64(
            indices.iter().filter_map(|&i| standings[i].avg_pit_secs).filter(|secs| secs.is_finite() && *secs > 0.0),
        );
        let contenders: Vec<endurance::Contender> = indices
            .iter()
            .map(|&i| {
                let entry = &standings[i];
                endurance::Contender {
                    class_position: entry.class_position,
                    gap_to_leader_secs: net_gaps[&entry.car_idx],
                    stops_remaining: entry
                        .stops_remaining
                        .or_else(|| {
                            class_typical_stint
                                .and_then(|avg| endurance::stops_remaining(laps_left, entry.current_stint_laps, avg))
                        })
                        .unwrap_or(0),
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a pit stop's duration is a handful of seconds, far inside f32"
                    )]
                    pit_loss_secs: entry
                        .avg_pit_secs
                        .filter(|secs| secs.is_finite() && *secs > 0.0)
                        .or(class_service)
                        .map_or(default_pit_loss_secs, |secs| {
                            if pit_model.transit_runs > 0 {
                                pit_model.transit_loss_secs + secs as f32
                            } else {
                                // This setting is a TOTAL stop-loss estimate
                                // (also used by the pit-position preview).
                                // Adding service again would double-charge it.
                                default_pit_loss_secs
                            }
                        }),
                }
            })
            .collect();
        for (&index, position) in indices.iter().zip(endurance::projected_positions(&contenders)) {
            standings[index].projected_class_position = position;
        }
    }

    // Latch the multi-stop flag off the whole class, not just the player:
    // early in a race nobody has a stint history yet, but as soon as any car
    // has pitted twice the shape of the race is known.
    let class_stops =
        |class_id: i32| standings.iter().filter(move |e| e.car_class_id == class_id).filter_map(|e| e.stops_remaining);
    if my_car_class_id.is_some_and(|class_id| class_stops(class_id).any(|s| s >= MULTI_STOP_THRESHOLD)) {
        *multi_stop_race = true;
    }

    let player = standings.iter().find(|e| e.is_focus);
    EnduranceMeta {
        multi_stop_race: *multi_stop_race,
        laps_remaining: Some(laps_left),
        // Filled in by the caller, which has the player's lap position.
        lap_driven_pct: None,
        stops_remaining: player.and_then(|e| e.stops_remaining),
        projected_class_position: player.and_then(|e| e.projected_class_position),
        best_stops_in_class: my_car_class_id.and_then(|class_id| {
            standings.iter().filter(|e| e.car_class_id == class_id).filter_map(|e| e.stops_remaining).min()
        }),
    }
}

/// Summarizes each car class present in `standings` into a [`ClassSection`].
///
/// Ordered by each class's leading overall position, so the Standings widget
/// draws the class that's actually winning first — which for a multi-class
/// race is the order a driver expects to read them in.
fn build_class_sections(standings: &[StandingsEntry], sof_cache: &mut SofCache) -> Vec<ClassSection> {
    let mut by_class: HashMap<i32, Vec<&StandingsEntry>> = HashMap::new();
    for entry in standings {
        by_class.entry(entry.car_class_id).or_default().push(entry);
    }

    // Reused across classes so the per-class rating list is one allocation for
    // the whole function rather than one per class per tick.
    let mut iratings: Vec<i32> = Vec::new();
    let mut sections: Vec<(i32, ClassSection)> = by_class
        .into_iter()
        .map(|(car_class_id, entries)| {
            // A class's leading overall position; `i32::MAX` only if every
            // entry is unclassified, which sorts the class last rather than
            // panicking on an empty fold.
            let lead_position = entries.iter().map(|e| e.position).filter(|p| *p > 0).min().unwrap_or(i32::MAX);
            iratings.clear();
            iratings.extend(entries.iter().map(|e| e.irating));
            let first = entries.first();
            let section = ClassSection {
                car_class_id,
                short_name: first.map_or_else(unnamed, |e| Arc::clone(&e.car_class_short_name)),
                color: first.map_or_else(unnamed, |e| Arc::clone(&e.car_class_color)),
                car_count: i32::try_from(entries.len()).unwrap_or(i32::MAX),
                sof: sof_cache.sof(car_class_id, &iratings),
            };
            (lead_position, section)
        })
        .collect();
    sections.sort_by_key(|(lead_position, _)| *lead_position);
    sections.into_iter().map(|(_, section)| section).collect()
}

/// Centre-to-centre metres between the player and one other car.
///
/// Measured from the two cars' track positions against the track's own
/// length, which is exact and — unlike a relative-time gap multiplied by the
/// player's speed — stays right at a standing start and through the slowest
/// corner, the moments a driver most needs the radar.
///
/// Falls back to the relative-time gap at a nominal racing speed when the
/// session has not published a track length, or when either car's position is
/// missing. That reading is an estimate rather than a measurement, but the
/// widget showing a car in roughly the right place beats it showing nothing.
///
/// In the pit lane the fallback is arguably the better source: the lane shares
/// its track positions with the main straight while being a different physical
/// length, so metres-per-percent differs there. Not corrected for, because
/// `CarLeftRight` — which gates the widget entirely — is a racing signal, and
/// the error is a fraction of a car length at pit-lane separations.
fn radar_separation_metres(
    me_lap_dist_pct: Option<f32>,
    other_lap_dist_pct: Option<f32>,
    track_length_m: Option<f32>,
    gap_secs: f32,
) -> f32 {
    /// Speed assumed when metres have to be estimated from seconds, in m/s —
    /// about 145 km/h, a middling racing speed across a lap.
    const NOMINAL_SPEED_MPS: f32 = 40.0;

    let measured = me_lap_dist_pct.zip(other_lap_dist_pct).zip(track_length_m).map(|((me, other), length)| {
        // Percentages wrap at the start/finish line, so the raw difference
        // between a car just before it and one just after is nearly a whole
        // lap. The short way round is always the right one at radar range.
        let mut delta = other - me;
        if delta > 0.5 {
            delta -= 1.0;
        } else if delta < -0.5 {
            delta += 1.0;
        }
        delta * length
    });
    measured.filter(|m| m.is_finite()).unwrap_or(gap_secs * NOMINAL_SPEED_MPS)
}

/// Combines `CarLeftRight`'s side signal with the nearest cars into a
/// [`RadarSnapshot`], smoothed across ticks so signal flicker doesn't make the
/// markers snap in and out.
///
/// `CarLeftRight` says only *that* a car is on a side, never which one. With
/// one side occupied that is enough: the nearest car ahead and the nearest
/// behind are the two that can be hit, and they go to the side that reported.
/// With both sides occupied the field holds at least two cars that can be hit,
/// so the two nearest are split one per bar — see [`radar::assign_sides`] for
/// how each is placed. Offering the same pair to both bars, as this once did,
/// draws two cars as four, and a count a driver reacts to must not be wrong.
fn build_radar(
    car_left_right: CarLeftRight,
    contacts: &[radar::Contact],
    radar_range_secs: f32,
    session_time_secs: f64,
    smoothing: &mut RadarSmoothing,
) -> RadarSnapshot {
    let nearby = radar::nearest_ahead_behind(contacts, radar_range_secs);
    let none = (None, None);
    let (raw_left, raw_right) = match car_left_right {
        CarLeftRight::CarLeft | CarLeftRight::TwoCarsLeft => (nearby, none),
        CarLeftRight::CarRight | CarLeftRight::TwoCarsRight => (none, nearby),
        CarLeftRight::CarLeftRight => {
            let pair = radar::two_nearest(contacts, radar_range_secs);
            let (left, right) =
                radar::assign_sides(pair, smoothing.left.last_signed_secs(), smoothing.right.last_signed_secs());
            (split_by_direction(left), split_by_direction(right))
        }
        CarLeftRight::Off | CarLeftRight::Clear => (none, none),
    };
    RadarSnapshot {
        left: smoothing.left.update(raw_left, session_time_secs),
        right: smoothing.right.update(raw_right, session_time_secs),
    }
}

/// The quickest lap seen in each class so far, as `(class id, seconds)`.
///
/// The last-resort measure of class speed, for a session whose YAML ranks no
/// classes. A `Vec` scanned linearly rather than a map: a grid has a handful
/// of classes, and this is rebuilt every tick.
fn class_best_laps(drivers: &HashMap<i32, DriverMeta>, best_laps: &[f32]) -> Vec<(i32, f32)> {
    let mut out: Vec<(i32, f32)> = Vec::new();
    for (car_idx, driver) in drivers {
        let Some(secs) = usize::try_from(*car_idx).ok().and_then(|i| best_laps.get(i)).copied() else {
            continue;
        };
        if !secs.is_finite() || secs <= 0.0 {
            continue;
        }
        match out.iter_mut().find(|(class_id, _)| *class_id == driver.car_class_id) {
            Some((_, best)) => *best = best.min(secs),
            None => out.push((driver.car_class_id, secs)),
        }
    }
    out
}

/// What is known about how quick `driver`'s class is.
///
/// A ranking of zero is treated as unpublished: iRacing writes zero where it
/// has nothing to say, and zero would otherwise rank below every real class.
fn class_pace(driver: &DriverMeta, class_best_laps: &[(i32, f32)]) -> faster_class::ClassPace {
    faster_class::ClassPace {
        class_id: driver.car_class_id,
        rel_speed: driver.car_class_rel_speed.filter(|speed| *speed > 0),
        est_lap_secs: driver.car_class_est_lap_secs,
        best_lap_secs: class_best_laps.iter().find(|(id, _)| *id == driver.car_class_id).map(|(_, secs)| *secs),
    }
}

/// Orders the tick's quicker-class sightings nearest first and attaches each
/// car's closing rate, for the Faster Class widget.
fn build_faster_class(
    sightings: &mut [(i32, f32)],
    info: &SessionInfoCache,
    session_time_secs: f64,
    rates: &mut faster_class::ClosingRates,
) -> FasterClassSnapshot {
    sightings.sort_by(|a, b| a.1.total_cmp(&b.1));
    rates.update(sightings, session_time_secs);
    let approaching = sightings
        .iter()
        .filter_map(|&(car_idx, behind_secs)| {
            let driver = info.drivers.get(&car_idx)?;
            Some(Approaching {
                car_idx,
                behind_secs,
                closing_rate: rates.rate(car_idx),
                driver_name: Arc::clone(&driver.user_name),
                car_number: Arc::clone(&driver.car_number),
                car_class_short_name: Arc::clone(&driver.car_class_short_name),
                car_class_color: Arc::clone(&driver.car_class_color),
            })
        })
        .collect();
    FasterClassSnapshot { approaching }
}

/// Files one car into the ahead or behind slot of the side it was given to.
fn split_by_direction(car: Option<radar::Contact>) -> (Option<radar::Contact>, Option<radar::Contact>) {
    match car {
        Some(car) if car.gap_secs >= 0.0 => (Some(car), None),
        Some(car) => (None, Some(car)),
        None => (None, None),
    }
}

/// Smooths one Radar Bars side's pair of readings across ticks.
///
/// iRacing's `CarLeftRight` can flicker off for a tick or two even while a
/// car is still genuinely alongside, which without smoothing snaps the
/// marker straight off the bar and back. This holds the last known gap
/// through brief dropouts and eases new readings in instead of jumping to
/// them, so the marker slides instead of flickering.
#[derive(Debug, Default)]
struct SideSmoothing {
    ahead: GapSmoothing,
    behind: GapSmoothing,
}

impl SideSmoothing {
    /// Takes this tick's raw contacts and returns what the side should draw.
    fn update(&mut self, raw: (Option<radar::Contact>, Option<radar::Contact>), session_time_secs: f64) -> RadarSide {
        let (raw_ahead, raw_behind) = raw;
        RadarSide {
            ahead: self.ahead.update(raw_ahead, session_time_secs),
            behind: self.behind.update(raw_behind, session_time_secs),
        }
    }

    /// The signed gap in seconds this side is currently showing, if any.
    ///
    /// Used to keep a car on the bar it was already on when the second side
    /// lights up; ahead wins over behind because a car being drawn ahead is
    /// the reading a swap would most visibly contradict.
    fn last_signed_secs(&self) -> Option<f32> {
        self.ahead.smoothed_secs.or(self.behind.smoothed_secs)
    }
}

/// Smooths a single slot's reading — see [`SideSmoothing`], which holds one of
/// these per direction.
#[derive(Debug, Default)]
struct GapSmoothing {
    smoothed_secs: Option<f32>,
    smoothed_m: Option<f32>,
    /// Consecutive ticks this direction has read "no car".
    empty_ticks: u32,
    /// The recent separations this slot's closing rate is measured over.
    history: radar::SeparationHistory,
}

/// How many consecutive empty ticks before a bar actually clears. `wait_for_data`
/// polls faster than the signal needs to hold, so a handful of ticks is a
/// small fraction of a second — enough to ride out flicker, not enough to
/// noticeably lag a car that's genuinely pulled away.
const RADAR_EMPTY_HOLD_TICKS: u32 = 5;

/// How much weight a new reading gets each tick (0.0-1.0). Lower means
/// smoother but slower to react.
const RADAR_SMOOTHING_ALPHA: f32 = 0.35;

impl GapSmoothing {
    /// Eases this tick's contact in, and reports what the slot should draw.
    ///
    /// The closing rate is measured from the smoothed metres rather than the
    /// raw ones, so the tail describes the marker the driver can actually see
    /// moving rather than the noisier number underneath it.
    fn update(&mut self, raw: Option<radar::Contact>, session_time_secs: f64) -> Option<RadarCar> {
        /// Seconds-to-milliseconds.
        const MS_PER_SEC: f32 = 1000.0;

        let ease = |previous: Option<f32>, new: f32| -> f32 {
            previous.map_or(new, |prev| prev + (new - prev) * RADAR_SMOOTHING_ALPHA)
        };
        if let Some(contact) = raw {
            self.empty_ticks = 0;
            self.smoothed_secs = Some(ease(self.smoothed_secs, contact.gap_secs));
            let metres = ease(self.smoothed_m, contact.gap_m);
            self.smoothed_m = Some(metres);
            self.history.push(session_time_secs, metres);
        } else {
            self.empty_ticks += 1;
            if self.empty_ticks >= RADAR_EMPTY_HOLD_TICKS {
                self.smoothed_secs = None;
                self.smoothed_m = None;
                self.history.clear();
            }
        }
        Some(RadarCar {
            separation_m: self.smoothed_m?,
            gap_ms: self.smoothed_secs? * MS_PER_SEC,
            closing_mps: self.history.closing_mps(),
        })
    }
}

/// Both Radar Bars sides' smoothing state, held across ticks for one session.
#[derive(Debug, Default)]
struct RadarSmoothing {
    left: SideSmoothing,
    right: SideSmoothing,
}

/// Per-`CarIdx` telemetry arrays [`build_standings`] blends with the
/// session's official `ResultsPositions`. Grouped into one struct purely to
/// keep `build_standings`'s own parameter list short; a bag of slice
/// references, so cheap to pass by value.
#[derive(Clone, Copy)]
struct StandingsRawArrays<'a> {
    best_laps: &'a [f32],
    last_laps: &'a [f32],
    f2_times: &'a [f32],
    track_surfaces: &'a [i32],
    laps: &'a [i32],
    /// Live overall position. Zero for a car iRacing hasn't scored yet, which
    /// is every car before the first start/finish crossing.
    positions: &'a [i32],
    /// Fraction of the lap each car has covered, used to order the field
    /// while nobody has a position yet.
    lap_dist_pcts: &'a [f32],
    /// Each car's `CarIdxSessionFlags` bits; empty where the sim has none.
    session_flags: &'a [i32],
}

/// One car's place in the running order, from either the session's official
/// classification or the live-telemetry fallback.
///
/// Mirrors the [`ResultsPosition`] fields this crate reads so
/// [`build_standings`] can be written once against both sources.
#[derive(Debug, Clone, Copy)]
struct ClassifiedCar {
    car_idx: i32,
    position: i32,
    class_position: i32,
    laps_complete: i32,
    fastest_time: f32,
    last_time: f32,
    pit_stops: i32,
    /// Present only for a real scorer row. Live fallback rows use zero for
    /// display but must not establish an official-stop baseline.
    official_pit_stops: Option<i32>,
}

impl From<&ResultsPosition> for ClassifiedCar {
    fn from(row: &ResultsPosition) -> Self {
        Self {
            car_idx: row.car_idx,
            position: row.position,
            class_position: row.class_position,
            laps_complete: row.laps_complete,
            fastest_time: row.fastest_time,
            last_time: row.last_time,
            pit_stops: row.pit_stops,
            official_pit_stops: Some(row.pit_stops.max(0)),
        }
    }
}

/// Reconstructs the running order from live telemetry, for the window before
/// iRacing has scored anybody.
///
/// Cars are ranked by live position where the sim publishes one, and by track
/// order — furthest lap first, then furthest around that lap — for the cars it
/// still reports as position `0`. On a race grid that is every car, and track
/// order is exactly the grid order.
///
/// Positions are the ranks derived here rather than the sim's own, so they
/// stay contiguous (`1..=n`) even when part of the field is unscored; the
/// Standings widget looks rows up by position, and a gap would drop them.
fn live_classification(info: &SessionInfoCache, arrays: StandingsRawArrays<'_>) -> Vec<ClassifiedCar> {
    let position_of = |i: usize| arrays.positions.get(i).copied().filter(|&p| p > 0).unwrap_or(i32::MAX);
    let lap_of = |i: usize| arrays.laps.get(i).copied().unwrap_or(0);
    let pct_of = |i: usize| arrays.lap_dist_pcts.get(i).copied().unwrap_or(0.0);

    let mut order: Vec<(i32, usize)> = info
        .drivers
        .iter()
        .filter(|(_, driver)| driver.is_competitor)
        .filter_map(|(&car_idx, _)| usize::try_from(car_idx).ok().map(|i| (car_idx, i)))
        .filter(|&(_, i)| {
            arrays
                .track_surfaces
                .get(i)
                .copied()
                .is_some_and(|raw| track_location_from_raw(raw) != TrackLocation::NotInWorld)
        })
        .collect();
    // `car_idx` breaks ties last so the order is stable frame to frame; two
    // cars genuinely side by side would otherwise swap rows every tick.
    order.sort_by(|&(a_car, a), &(b_car, b)| {
        position_of(a)
            .cmp(&position_of(b))
            .then_with(|| lap_of(b).cmp(&lap_of(a)))
            .then_with(|| pct_of(b).total_cmp(&pct_of(a)))
            .then_with(|| a_car.cmp(&b_car))
    });

    let mut class_ranks: HashMap<i32, i32> = HashMap::new();
    order
        .into_iter()
        .enumerate()
        .map(|(rank, (car_idx, i))| {
            let class_id = info.drivers.get(&car_idx).map_or(0, |d| d.car_class_id);
            let class_rank = class_ranks.entry(class_id).or_insert(0);
            *class_rank += 1;
            ClassifiedCar {
                car_idx,
                position: i32::try_from(rank).unwrap_or(i32::MAX).saturating_add(1),
                class_position: *class_rank,
                laps_complete: lap_of(i),
                // Lap times and stop counts have live telemetry sources that
                // `build_standings` prefers over these anyway; zero reads as
                // "not set yet" everywhere they surface.
                fastest_time: 0.0,
                last_time: 0.0,
                pit_stops: 0,
                official_pit_stops: None,
            }
        })
        .collect()
}

/// Re-orders and renumbers the field from the live per-tick position arrays.
///
/// The running order in `ResultsPositions` comes from the session-info YAML,
/// which iRacing republishes far more slowly than it updates telemetry. Left
/// as the source of the order, Standings lags the Relative — the same two cars
/// swap on one panel seconds before the other, which reads as one of them
/// being broken. `CarIdxPosition` is the same figure the Relative is built
/// from, so taking the order from it means the two cannot disagree.
///
/// Class positions are then *derived* from that order rather than read from
/// the YAML. That is the stronger reason to do this: it makes them contiguous
/// from one by construction, so Standings — which looks rows up by class
/// position, counting from one — can no longer be handed a numbering that
/// starts at zero, hides the class leader, and puts every driver on a
/// neighbour's row.
///
/// Cars the sim has not scored keep the order they arrived in, behind the ones
/// it has: a car with no position is placed by track order upstream, and
/// nothing here knows better.
/// How [`build_standings`] orders the field this tick.
enum StandingsOrder<'a> {
    /// The scorer's own order (`CarIdxPosition`): practice, qualifying, and
    /// a race once the checkered flies.
    Scored,
    /// The grid as qualifying set it, while a race's field forms up — see
    /// [`apply_grid_order`].
    Grid,
    /// Each car's best lap, quickest first: practice and qualifying, where
    /// that is the only order anyone means — see [`apply_quickest_order`].
    Quickest,
    /// The live distance order between the green and the checkered,
    /// carrying the held ranks its hysteresis needs — see
    /// [`apply_distance_order`].
    Distance(&'a mut RaceOrder),
}

/// Orders practice and qualifying by each car's best lap, quickest first.
///
/// The scorer agrees in principle, but `CarIdxPosition` moves on the slow
/// session-info cadence, so the lap that just demoted you was showing the
/// old order for seconds. The live best-lap array is per-tick; the official
/// fastest stands in for a car the array has nothing for, and cars with no
/// lap at all keep the scorer's order at the back.
fn apply_quickest_order(classified: &mut [ClassifiedCar], info: &SessionInfoCache, arrays: StandingsRawArrays<'_>) {
    let best = |car: &ClassifiedCar| -> f32 {
        usize::try_from(car.car_idx)
            .ok()
            .and_then(|i| arrays.best_laps.get(i))
            .copied()
            .filter(|secs| *secs > 0.0)
            .or_else(|| (car.fastest_time > 0.0).then_some(car.fastest_time))
            .unwrap_or(f32::MAX)
    };
    classified.sort_by(|a, b| {
        best(a).total_cmp(&best(b)).then_with(|| a.position.cmp(&b.position)).then_with(|| a.car_idx.cmp(&b.car_idx))
    });
    renumber(classified, info);
}

/// Orders a race's forming field by its qualifying result.
///
/// iRacing hands out `CarIdxPosition` as cars grid up, so the first driver
/// to grid reads P1 regardless of where they qualified, and their number
/// then walks down as the cars ahead of them appear. The qualifying
/// classification is published for the whole event, so the panel can show
/// the real starting order from the moment the race session loads. Cars
/// that did not qualify sort to the back, in the order the scorer has them.
fn apply_grid_order(classified: &mut [ClassifiedCar], info: &SessionInfoCache) {
    let grid_slot = |car_idx: i32| info.qualify_grid.get(&car_idx).map_or(i32::MAX, |(overall, _)| *overall);
    classified.sort_by(|a, b| {
        grid_slot(a.car_idx)
            .cmp(&grid_slot(b.car_idx))
            .then_with(|| a.position.cmp(&b.position))
            .then_with(|| a.car_idx.cmp(&b.car_idx))
    });
    renumber(classified, info);
}

fn apply_live_order(classified: &mut [ClassifiedCar], info: &SessionInfoCache, arrays: StandingsRawArrays<'_>) {
    let live_position = |car_idx: i32| {
        usize::try_from(car_idx)
            .ok()
            .and_then(|i| arrays.positions.get(i))
            .copied()
            .filter(|position| *position > 0)
            .unwrap_or(i32::MAX)
    };
    classified.sort_by(|a, b| {
        live_position(a.car_idx)
            .cmp(&live_position(b.car_idx))
            // The order they came in with breaks ties, so unscored cars keep
            // the track order they were placed in rather than shuffling.
            .then_with(|| a.position.cmp(&b.position))
            .then_with(|| a.car_idx.cmp(&b.car_idx))
    });
    renumber(classified, info);
}

/// Rewrites every car's overall and class position from the order it now
/// stands in, contiguous from one — see [`apply_live_order`] for why the
/// numbering is derived rather than trusted.
fn renumber(classified: &mut [ClassifiedCar], info: &SessionInfoCache) {
    let mut class_ranks: HashMap<i32, i32> = HashMap::new();
    for (rank, car) in classified.iter_mut().enumerate() {
        car.position = i32::try_from(rank).unwrap_or(i32::MAX).saturating_add(1);
        let class_id = info.drivers.get(&car.car_idx).map_or(0, |driver| driver.car_class_id);
        let slot = class_ranks.entry(class_id).or_insert(0);
        *slot = slot.saturating_add(1);
        car.class_position = *slot;
    }
}

/// How much further round the track a car must be than the one it is passing
/// before the two swap positions, in laps of track position.
///
/// The Standings counterpart of `relative::ORDER_HYSTERESIS_SECS`, and there
/// for the same reason: two cars genuinely side by side sit inside
/// `CarIdxLapDistPct`'s tick-to-tick noise, and without a band their
/// positions would trade every frame. Half a thousandth of a lap is two to
/// three metres on a typical circuit — under a car length, so any pass a
/// driver would call complete clears it at once, while two cars drag-racing
/// down a straight hold their numbers until one is genuinely by.
const ORDER_HYSTERESIS_LAPS: f32 = 0.000_5;

/// The previous tick's distance-based running order, so side-by-side cars
/// hold their positions rather than trading them every tick — see
/// [`apply_distance_order`] and [`ORDER_HYSTERESIS_LAPS`].
#[derive(Debug, Default)]
struct RaceOrder {
    /// Each car's place in the last order produced, by `CarIdx`.
    ranks: HashMap<i32, usize>,
}

/// Re-orders the field by how far each car has actually driven: whole laps
/// plus the fraction of the current one.
///
/// This is what makes a race position *live*. `CarIdxPosition` moves only
/// when the scorer says so — at timing checkpoints, in practice the
/// start/finish line — so a driver who passes the car ahead down the back
/// straight watched the panel say the old number for the rest of the lap.
/// Race position between the green and the checkered *is* track position,
/// laps included, and that is published every tick.
///
/// Only between those flags, though: the caller keeps this to
/// `SessionState::Racing`, because before the green the order is the grid's
/// and after the checkered it is the result's, and neither is a fact about
/// where the cars are. Distance also cannot see a penalty the scorer applies
/// on paper, which stands until the scorer's own order returns at the flag.
///
/// Each car is handicapped by the place it already held, exactly as the
/// Relative's `order_by_gap` does, so the comparison stays a total order and
/// stationary neighbours don't swap on noise — see [`ORDER_HYSTERESIS_LAPS`].
fn apply_distance_order(
    classified: &mut [ClassifiedCar],
    info: &SessionInfoCache,
    arrays: StandingsRawArrays<'_>,
    lap_latch: &mut LapLatch,
    held: &mut RaceOrder,
) {
    #[expect(clippy::cast_precision_loss, reason = "lap counts are far inside f32's exact-integer range")]
    let mut distance_of = |car: &ClassifiedCar| -> f32 {
        let idx = usize::try_from(car.car_idx).ok();
        // The latch, not the raw lap: a towed car reads `-1` and would fall
        // to the back of the field mid-tow, then leap forward on rejoin.
        let lap = lap_latch.resolve(car.car_idx, idx.and_then(|i| arrays.laps.get(i)).copied(), car.laps_complete);
        let pct = idx
            .and_then(|i| arrays.lap_dist_pcts.get(i))
            .copied()
            .filter(|pct| pct.is_finite() && *pct >= 0.0)
            .map_or(0.0, |pct| pct.min(1.0));
        lap as f32 + pct
    };
    let distances: HashMap<i32, f32> = classified.iter().map(|car| (car.car_idx, distance_of(car))).collect();
    let distance = |car: &ClassifiedCar| distances.get(&car.car_idx).copied().unwrap_or(0.0);

    // Where each car would sit on distance alone — the answer for a car the
    // last tick knew nothing about, which has no held place of its own.
    classified.sort_by(|a, b| distance(b).total_cmp(&distance(a)).then_with(|| a.car_idx.cmp(&b.car_idx)));
    #[expect(clippy::cast_precision_loss, reason = "a field is tens of cars, exact in f32")]
    let keys: Vec<f32> = classified
        .iter()
        .enumerate()
        .map(|(rank, car)| {
            let held_rank = held.ranks.get(&car.car_idx).copied().unwrap_or(rank);
            distance(car) - held_rank as f32 * ORDER_HYSTERESIS_LAPS
        })
        .collect();
    let mut places: Vec<usize> = (0..classified.len()).collect();
    places
        .sort_by(|&a, &b| keys[b].total_cmp(&keys[a]).then_with(|| classified[a].car_idx.cmp(&classified[b].car_idx)));

    held.ranks = places.iter().enumerate().map(|(rank, &i)| (classified[i].car_idx, rank)).collect();
    // Applies the permutation in place: `places[rank]` names the row that
    // belongs at `rank`, so the rows are cloned out in that order and copied
    // back. A `ClassifiedCar` is `Copy`-sized, so this is cheap.
    let ordered: Vec<ClassifiedCar> = places.into_iter().map(|i| classified[i]).collect();
    classified.copy_from_slice(&ordered);
    renumber(classified, info);
}

/// Appends every competitor the official classification hasn't scored yet.
///
/// They go behind the scored cars in track order — which is all that can
/// honestly be said about a car with no lap time — keeping both the overall
/// and the per-class numbering contiguous, since Standings looks rows up by
/// position and a gap would drop them.
fn append_unscored(classified: &mut Vec<ClassifiedCar>, info: &SessionInfoCache, arrays: StandingsRawArrays<'_>) {
    let scored: std::collections::HashSet<i32> = classified.iter().map(|car| car.car_idx).collect();
    let class_of = |car_idx: i32| info.drivers.get(&car_idx).map_or(0, |driver| driver.car_class_id);

    let mut next_position = classified.iter().map(|car| car.position).max().unwrap_or(0);
    let mut next_in_class: HashMap<i32, i32> = HashMap::new();
    for car in classified.iter() {
        let slot = next_in_class.entry(class_of(car.car_idx)).or_insert(0);
        *slot = (*slot).max(car.class_position);
    }

    for mut car in live_classification(info, arrays) {
        if scored.contains(&car.car_idx) {
            continue;
        }
        next_position = next_position.saturating_add(1);
        let slot = next_in_class.entry(class_of(car.car_idx)).or_insert(0);
        *slot = slot.saturating_add(1);
        car.position = next_position;
        car.class_position = *slot;
        classified.push(car);
    }
}

/// The last real lap number seen for each car.
///
/// `CarIdxLap` reads `-1` for a car that is not in the world — towed, in the
/// garage between drivers, mid-repair, or disconnected, all of them routine
/// in an endurance race. Subtracting that raw `-1` from the class leader's
/// lap is how a car two laps down came to read as ten laps down: the figure
/// was the leader's lap count plus one, not a gap at all. The latch keeps
/// the car at the lap it was genuinely on when it left the world.
///
/// Laps only count up within a session, so taking the max is safe, and the
/// latch is dropped with the rest of the trackers on a change of session.
#[derive(Debug, Default)]
struct LapLatch(HashMap<i32, i32>);

impl LapLatch {
    /// The car's lap as far as anything honest knows: the live value while
    /// the sim publishes one, else the last it published, else `fallback` —
    /// the YAML's `LapsComplete`, which for a parked car is stale but true.
    fn resolve(&mut self, car_idx: i32, live: Option<i32>, fallback: i32) -> i32 {
        match live.filter(|lap| *lap >= 0) {
            Some(lap) => {
                let latched = self.0.entry(car_idx).or_insert(lap);
                *latched = (*latched).max(lap);
                *latched
            }
            None => self.0.get(&car_idx).copied().unwrap_or(fallback),
        }
    }
}

#[cfg(test)]
mod lap_latch_tests {
    use super::LapLatch;

    /// The tow case: a car that leaves the world keeps the lap it was on,
    /// rather than reading `-1` and turning its gap into the leader's lap
    /// count.
    #[test]
    fn a_car_out_of_the_world_keeps_its_last_real_lap() {
        let mut latch = LapLatch::default();
        assert_eq!(latch.resolve(7, Some(5), 0), 5);
        assert_eq!(latch.resolve(7, Some(-1), 0), 5, "towed: the latch answers");
        assert_eq!(latch.resolve(7, Some(6), 0), 6, "back in the world: live again");
    }

    /// A car never seen in the world falls back to the YAML's scored count —
    /// stale, but a real number rather than `-1`.
    #[test]
    fn a_car_never_seen_falls_back_to_the_scored_count() {
        let mut latch = LapLatch::default();
        assert_eq!(latch.resolve(3, Some(-1), 4), 4);
        assert_eq!(latch.resolve(3, None, 4), 4, "no live array at all: same fallback");
    }
}

/// Builds the Standings list from the current session's `ResultsPositions`,
/// filled in with driver metadata and live per-tick telemetry where available.
///
/// Falls back to [`live_classification`] while `ResultsPositions` is empty.
///
/// `order` picks how the field is ranked this tick — see [`StandingsOrder`]
/// for what each variant is right for.
#[expect(clippy::too_many_arguments, reason = "called from one place; a struct would only rename the list")]
fn build_standings(
    info: &SessionInfoCache,
    session_num: Option<i32>,
    focus_car_idx: i32,
    arrays: StandingsRawArrays<'_>,
    stint_tracker: &mut StintTracker,
    off_tracks: &mut OffTrackCounter,
    lap_latch: &mut LapLatch,
    session_time_secs: f64,
    order: StandingsOrder<'_>,
) -> Vec<StandingsEntry> {
    let official: Vec<ClassifiedCar> = session_num
        .and_then(|num| info.sessions.iter().find(|s| s.session_num == num))
        .into_iter()
        .flat_map(|session_results| &session_results.results_positions)
        .map(ClassifiedCar::from)
        .collect();
    // iRacing publishes `ResultsPositions` only once cars have been scored,
    // which for a race means the first start/finish crossing. Reading nothing
    // else left Standings — and everything derived from it, class sections,
    // SOF, car count — blank on the grid, through the pace laps and for the
    // whole opening lap, the stretch where the field is closest together.
    //
    // The two sources are merged rather than chosen between. Taking the
    // official list whole as soon as it had a single row in it dropped every
    // car the sim had not scored yet — which in qualifying is everyone who
    // has not set a lap, the player included. Their own row vanishing is what
    // collapsed the widget to three leaders: with no player row there is no
    // player class, and every class falls back to its leaders-only view.
    let mut classified: Vec<ClassifiedCar> = official.into_iter().filter(|row| row.position > 0).collect();
    if classified.is_empty() {
        classified = live_classification(info, arrays);
    } else {
        append_unscored(&mut classified, info, arrays);
    }
    // Ordered and numbered from live telemetry, so this panel and the Relative
    // are never a session-info update apart. Mid-race the order comes from
    // how far each car has driven, so a pass changes the numbers the moment
    // it happens rather than at the next timing checkpoint.
    match order {
        StandingsOrder::Distance(held) => apply_distance_order(&mut classified, info, arrays, lap_latch, held),
        StandingsOrder::Grid => apply_grid_order(&mut classified, info),
        StandingsOrder::Quickest => apply_quickest_order(&mut classified, info, arrays),
        StandingsOrder::Scored => apply_live_order(&mut classified, info, arrays),
    }

    let mut entries: Vec<StandingsEntry> = classified
        .iter()
        .map(|row| {
            let idx = usize::try_from(row.car_idx).ok();
            let driver = info.drivers.get(&row.car_idx);
            let track_location = idx
                .and_then(|i| arrays.track_surfaces.get(i))
                .copied()
                .map_or(TrackLocation::NotInWorld, track_location_from_raw);
            let lap = lap_latch.resolve(row.car_idx, idx.and_then(|i| arrays.laps.get(i)).copied(), row.laps_complete);
            // The whole pit lane, not the stall alone: for a car other than the
            // player, `InPitStall` is the one value the sim can decline to
            // publish, and hanging the stint on it left rivals running stints
            // that never ended. What separates a stop from a drive-through is
            // then whether the car actually stood still in there, which its own
            // track position answers — see [`StintState::update`].
            let stint = stint_tracker.update(
                row.car_idx,
                session_time_secs,
                lap,
                row.official_pit_stops,
                track_location != TrackLocation::NotInWorld,
                matches!(track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits),
                track_location == TrackLocation::InPitStall,
                idx.and_then(|i| arrays.lap_dist_pcts.get(i)).copied().filter(|pct| pct.is_finite() && *pct >= 0.0),
            );
            let penalty = idx.and_then(|i| arrays.session_flags.get(i)).copied().and_then(relative::penalty_from_flags);
            StandingsEntry {
                penalty,
                position: row.position,
                class_position: row.class_position,
                car_idx: row.car_idx,
                driver_name: driver.map_or_else(unnamed, |d| Arc::clone(&d.user_name)),
                car_screen_name: driver.map_or_else(unnamed, |d| Arc::clone(&d.car_screen_name)),
                irating: driver.map_or(0, |d| d.irating),
                flair_id: driver.map_or(0, |d| d.flair_id),
                car_class_id: driver.map_or(0, |d| d.car_class_id),
                car_class_short_name: driver.map_or_else(unnamed, |d| Arc::clone(&d.car_class_short_name)),
                car_class_color: driver.map_or_else(unnamed, |d| Arc::clone(&d.car_class_color)),
                best_lap_secs: idx
                    .and_then(|i| arrays.best_laps.get(i))
                    .copied()
                    .filter(|&t| t > 0.0)
                    .unwrap_or(row.fastest_time),
                last_lap_secs: idx
                    .and_then(|i| arrays.last_laps.get(i))
                    .copied()
                    .filter(|&t| t > 0.0)
                    .unwrap_or(row.last_time),
                gap_to_leader_secs: idx.and_then(|i| arrays.f2_times.get(i)).copied().unwrap_or(0.0),
                // Whichever source has seen more stops. The measured count is
                // the reliable one — it is what this widget watched happen —
                // but it can only count stops made since the overlay was
                // looking, so a car that pitted before it started is caught by
                // the official figure instead.
                pit_stops: row.pit_stops.max(stint.completed_stops),
                last_pit_secs: stint.last_pit_secs,
                avg_pit_secs: stint.avg_pit_secs,
                stops_remaining: None,
                projected_class_position: None,
                current_stint_laps: stint.current_laps,
                current_stint_secs: stint.current_secs,
                avg_stint_laps: stint.avg_laps,
                avg_stint_secs: stint.avg_secs,
                track_location,
                // Both filled in by `annotate_classes` below, which needs
                // the whole field to compare against.
                is_class_fastest: false,
                laps_down: lap,
                is_focus: row.car_idx == focus_car_idx,
                off_tracks: off_tracks.update(
                    row.car_idx,
                    track_location == TrackLocation::OffTrack,
                    session_time_secs,
                ),
                // All filled in by the caller, which has the tick-wide
                // context — the tow tracker, the track's wetness, the
                // position baselines — that this builder deliberately
                // doesn't.
                tow_secs: None,
                tyre: None,
                race_position_change: None,
            }
        })
        .collect();
    annotate_classes(&mut entries);
    entries
}

/// Fills in the per-class comparisons each entry can't compute alone.
///
/// On entry, `laps_down` holds each car's raw completed-lap count; this
/// converts it to laps behind that car's own class leader. On entry,
/// `gap_to_leader_secs` is `CarIdxF2Time` — measured to the *overall*
/// leader — and this rebases it to the class leader's. Also marks the
/// fastest lap within each class.
///
/// All three are class-relative rather than session-relative because the
/// Standings widget classifies by class throughout — in a multi-class race,
/// a GT3 car's twenty seconds to the LMP2 leader isn't a fact about its
/// race, and it drowned the gaps that are.
fn annotate_classes(entries: &mut [StandingsEntry]) {
    let mut leader_laps: HashMap<i32, i32> = HashMap::new();
    let mut leader_gap: HashMap<i32, f32> = HashMap::new();
    let mut best_lap: HashMap<i32, f32> = HashMap::new();
    for entry in entries.iter() {
        let laps = leader_laps.entry(entry.car_class_id).or_insert(entry.laps_down);
        *laps = (*laps).max(entry.laps_down);
        if entry.class_position == 1 {
            leader_gap.insert(entry.car_class_id, entry.gap_to_leader_secs);
        }
        if entry.best_lap_secs > 0.0 {
            let best = best_lap.entry(entry.car_class_id).or_insert(entry.best_lap_secs);
            *best = best.min(entry.best_lap_secs);
        }
    }
    for entry in entries {
        let leader = leader_laps.get(&entry.car_class_id).copied().unwrap_or(entry.laps_down);
        entry.laps_down = (leader - entry.laps_down).max(0);
        // Clamped at zero: the panel's order and the sim's F2 clock can
        // briefly disagree about a side-by-side pair, and a leader shown a
        // negative gap to themselves reads as a bug.
        if let Some(gap) = leader_gap.get(&entry.car_class_id) {
            entry.gap_to_leader_secs = (entry.gap_to_leader_secs - gap).max(0.0);
        }
        entry.is_class_fastest =
            entry.best_lap_secs > 0.0 && best_lap.get(&entry.car_class_id) == Some(&entry.best_lap_secs);
    }
}

/// Stints shorter than this never enter a car's stint history.
///
/// A car that boxes three laps after its last stop stopped for damage, a
/// penalty or a top-up — none of which says anything about how far its tank
/// goes, and a projection built on it expects the car back in the lane every
/// few laps for the rest of the race. Even the shortest genuine fuel stints
/// in iRacing run well past five laps, so a stint below this is still counted
/// as a stop but kept out of the history the stop projection is built on.
const MIN_FUEL_STINT_LAPS: i32 = 5;

/// How many completed stints and stops the per-car history keeps.
///
/// Enough that one odd visit can never be the median, and short enough that a
/// race whose stint length genuinely changes — a wet spell, a fuel-saving
/// phase — is followed rather than averaged with its own past forever.
const STINT_SAMPLES: usize = 8;

/// How far a car must move along the lap for the pit lane to call it rolling.
///
/// Set clear of `CarIdxLapDistPct`'s own noise, which is the whole difficulty:
/// the figure wobbles a couple of metres tick to tick, enough that
/// [`ORDER_HYSTERESIS_LAPS`] exists to stop side-by-side cars trading places
/// on it. A threshold inside that band would read a parked car's jitter as
/// movement, and a car that never reads as stopped never ends a stint — the
/// very fault this detection is here to fix. Twice the hysteresis band, so
/// noise cannot clear it.
///
/// The cost of being this coarse is bounded because it is a distance to
/// cover, not a speed, and it is measured against the last position the car
/// was seen to leave rather than the previous tick. A thousandth of a lap is
/// 13 m at Le Mans and 4 m at a short circuit; a car at the 60 kph limiter
/// covers even the longer of those in under a second, well inside
/// [`STOPPED_CONFIRM_SECS`].
const PIT_MOVED_PCT: f32 = 0.001;

/// How long a car must sit still before the pit lane calls it stopped.
///
/// Only load-bearing where the sim declines to publish `InPitStall` for
/// another car, which is the case this exists for. Two seconds is a fraction
/// of any service — the quickest splash is twenty — and longer than a car
/// serving a drive-through pauses on its way through.
const STOPPED_CONFIRM_SECS: f64 = 2.0;

/// Stationary time a pit-lane visit needs before it counts as a stop.
///
/// A drive-through penalty is a lane transit at the limiter with no stop in
/// it. Counted as a pit stop it ends a fuel stint that is still running —
/// nothing went into the car — and adds a stop to every projection built on
/// the count. Three seconds clears the longest pause a car makes queueing
/// behind another on its way through, and is a fraction of the shortest real
/// service.
const MIN_STOP_SECS: f64 = 3.0;

/// Missing in-world position time that makes a continuous lane visit more
/// likely to contain an unseen stop than an observed drive-through.
///
/// It permits a stint reset and an exit-to-exit range for sparse rival
/// telemetry, but never supplies a stop duration. `NotInWorld` time is
/// excluded because that may be a tow or garage stay rather than missing lane
/// position.
const POSITION_BLIND_TOLERANCE_SECS: f64 = 1.0;

/// Per-car pit-lane transition tracking, so Standings can show current and
/// typical stint length without a direct SDK var for either.
#[derive(Debug, Default, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "orthogonal observations: world presence, lane continuity, stall presence and complete stint boundary"
)]
struct StintState {
    /// `SessionTime`/lap count when the car most recently left the pits, if
    /// it's been seen doing so at least once this session.
    current_start_secs: Option<f64>,
    current_start_lap: Option<i32>,
    /// Recently completed fuel stints as `(laps, secs)`, oldest first;
    /// bounded to [`STINT_SAMPLES`], and stints under
    /// [`MIN_FUEL_STINT_LAPS`] never enter.
    completed: std::collections::VecDeque<(i32, f64)>,
    was_in_pit_lane: bool,
    /// Whether any trustworthy surface has been seen for this car.
    seen_in_world: bool,
    /// A `NotInWorld` tick interrupted the last trustworthy transition.
    ///
    /// It is not an off-track or pit-exit observation. In particular, a car
    /// that vanishes from the circuit and next appears in its box was towed;
    /// treating that as a normal lane visit invents a stop and a fresh stint.
    observation_interrupted: bool,
    /// `SessionTime` and lap at which the current pit-lane visit began, so a
    /// visit that turns out to be a stop closes its stint at the lane entry
    /// rather than wherever the car came to rest.
    lane_entered: Option<(f64, i32)>,
    /// The lap fraction this car was last seen to move past, and when it did.
    ///
    /// The stand-in for a stall reading the sim may never publish for another
    /// car: in the pit lane a car that is not moving is being serviced. See
    /// [`PIT_MOVED_PCT`].
    anchor_pct: Option<f32>,
    last_moved_secs: f64,
    /// When the current stationary spell began, and the stationary time this
    /// visit has banked before it. Their sum is what tells a stop from a
    /// drive-through — see [`MIN_STOP_SECS`].
    stopped_since: Option<f64>,
    stopped_total_secs: f64,
    /// `SessionTime` at the previous trustworthy tick of the current visit.
    last_tick_secs: f64,
    /// False when the visit began outside observable telemetry. Such a visit
    /// may be a tow or garage stay and cannot close a racing stint.
    lane_visit_observed: bool,
    /// In-world lane time for which no position was published.
    blind_secs: f64,
    /// Whether the sim directly placed the car in its pit stall this visit.
    stall_observed: bool,
    /// Whether the visit crossed a `NotInWorld` gap. Service observed outside
    /// the gap remains real, but the gap makes its duration and range unsafe
    /// to learn from.
    lane_visit_interrupted: bool,
    /// Whether the current stint was observed from its beginning. An overlay
    /// attached halfway through a race must not learn a fuel range from the
    /// tail of the first stint it happens to see.
    current_start_observed: bool,
    /// Recent completed stops' stationary times, oldest first; bounded to
    /// [`STINT_SAMPLES`].
    pit_secs: std::collections::VecDeque<f64>,
    pit_count: u32,
    last_pit_secs: Option<f64>,
    /// Highest genuine scorer count seen. The first value establishes a
    /// historical offset; later increases reconcile lane visits.
    official_stops_seen: Option<i32>,
    /// Stops the scorer knew about before this tracker measured them.
    official_offset: i32,
    /// Scorer increases not yet paired with a local lane outcome.
    official_unmatched: u32,
    /// Locally detected stops the scorer has not caught up with yet.
    local_unscored: u32,
    /// Latest exit whose evidence was insufficient to call either a stop or
    /// a drive-through. A later scorer increase may confirm its boundary.
    /// The flag records whether confirmation should still move the current
    /// boundary; a later local reset supersedes that part while retaining the
    /// pending count reconciliation.
    pending_uncertain_exit: Option<(f64, i32, bool)>,
    /// Fully observed nonstop visits awaiting any scorer count they may
    /// receive. Matching them changes the displayed total, never the stint.
    pending_drive_throughs: u32,
}

/// One car's current and typical stint length, in both time and laps.
///
/// The `avg_*` figures are medians of the recent history rather than means —
/// see [`StintState::update`] for why.
#[derive(Debug, Clone, Copy)]
struct StintReading {
    current_secs: f64,
    current_laps: i32,
    avg_secs: Option<f64>,
    avg_laps: Option<i32>,
    /// How long this car was stationary at its last completed stop.
    last_pit_secs: Option<f64>,
    /// Its typical stationary time across recent completed stops.
    avg_pit_secs: Option<f64>,
    /// Stops completed in this session, combining the scorer's historical
    /// baseline with visits measured since this started watching.
    completed_stops: i32,
}

#[derive(Debug, Clone, Copy)]
enum LaneVisitOutcome {
    Stop,
    DriveThrough,
    Uncertain { exit_secs: f64, exit_lap: i32 },
    Ignored,
}

impl StintState {
    /// Folds one tick of this car's pit-stall state into its stint history.
    ///
    /// The typical figures that come out are **medians** of the recent
    /// history, not means, for the same reason the player's own lane transit
    /// is (see `pit_model`): one visit for damage, a penalty or a repair is
    /// wildly unlike the rest, and a mean carries a share of it into every
    /// projection for the rest of the race. A median throws it away — while
    /// keeping what is genuine: a car that stops every 16 laps where the rest
    /// of its class runs 17 really is going a lap early, and reads as 16.
    #[cfg(test)]
    fn update(
        &mut self,
        session_time_secs: f64,
        lap: i32,
        in_pit_lane: bool,
        in_pit_stall: bool,
        lap_dist_pct: Option<f32>,
    ) -> StintReading {
        self.update_with_presence(session_time_secs, lap, true, in_pit_lane, in_pit_stall, lap_dist_pct)
    }

    /// The standings integration variant of [`Self::update`], where an
    /// absent surface is kept distinct from a real observation off pit road.
    #[cfg(test)]
    fn update_with_presence(
        &mut self,
        session_time_secs: f64,
        lap: i32,
        in_world: bool,
        in_pit_lane: bool,
        in_pit_stall: bool,
        lap_dist_pct: Option<f32>,
    ) -> StintReading {
        self.update_with_official(
            session_time_secs,
            lap,
            None,
            in_world,
            in_pit_lane,
            in_pit_stall,
            lap_dist_pct,
        )
    }

    #[expect(clippy::too_many_arguments, reason = "one normalized per-car telemetry sample plus its scorer count")]
    fn update_with_official(
        &mut self,
        session_time_secs: f64,
        lap: i32,
        official_stops: Option<i32>,
        in_world: bool,
        in_pit_lane: bool,
        in_pit_stall: bool,
        lap_dist_pct: Option<f32>,
    ) -> StintReading {
        self.observe_official_stops(official_stops);
        if !in_world {
            // Once the car leaves observable telemetry, a later official
            // increase cannot safely be assigned to an older drive-through.
            self.pending_drive_throughs = 0;
            self.observation_interrupted |= self.seen_in_world;
            if self.was_in_pit_lane {
                // Do not let an unobserved garage/driver-swap interval become
                // stationary service time when the car next appears.
                if let Some(since) = self.stopped_since.take() {
                    self.stopped_total_secs += (self.last_tick_secs - since).max(0.0);
                }
                self.lane_visit_interrupted = true;
            }
            self.last_tick_secs = session_time_secs;
            return self.reading(session_time_secs, lap);
        }

        if in_pit_lane && !self.was_in_pit_lane {
            // Any scorer increment after a new visit begins belongs to newer
            // activity; an older observed drive-through cannot claim it.
            self.pending_drive_throughs = 0;
        }
        let resumed_after_gap = std::mem::take(&mut self.observation_interrupted);
        // A delayed scorer update normally arrives after pit exit. Settle it
        // against an earlier local outcome before this tick can begin another
        // lane visit.
        self.reconcile_official_stops(
            session_time_secs,
            lap,
            in_pit_lane || self.was_in_pit_lane,
            false,
            resumed_after_gap,
        );
        let left_pit_lane = !in_pit_lane && self.was_in_pit_lane;
        let mut visit_outcome = None;
        if in_pit_lane {
            let since_last_tick =
                if self.was_in_pit_lane { (session_time_secs - self.last_tick_secs).max(0.0) } else { 0.0 };
            if !self.was_in_pit_lane {
                self.lane_entered = Some((session_time_secs, lap));
                self.anchor_pct = lap_dist_pct;
                self.last_moved_secs = session_time_secs;
                self.stopped_since = None;
                self.stopped_total_secs = 0.0;
                self.lane_visit_observed = self.seen_in_world && !resumed_after_gap;
                self.blind_secs = 0.0;
                self.stall_observed = in_pit_stall;
                self.lane_visit_interrupted = false;
                if resumed_after_gap {
                    // Track -> absent -> box is a tow/garage transition, not
                    // a witnessed stint boundary. Do not let a later genuine
                    // stop turn the whole span into a fuel-range sample.
                    self.current_start_observed = false;
                }
            } else if resumed_after_gap {
                // Start motion/stationary inference afresh after the gap, so
                // the unseen interval itself can never become service time.
                self.anchor_pct = lap_dist_pct;
                self.last_moved_secs = session_time_secs;
            }
            self.last_tick_secs = session_time_secs;
            if lap_dist_pct.is_none() && !resumed_after_gap {
                self.blind_secs += since_last_tick;
            }
            self.stall_observed |= in_pit_stall;
            match (lap_dist_pct, self.anchor_pct) {
                (Some(pct), Some(anchor)) if (pct - anchor).abs() > PIT_MOVED_PCT => {
                    self.anchor_pct = Some(pct);
                    self.last_moved_secs = session_time_secs;
                }
                (Some(pct), None) => {
                    self.anchor_pct = Some(pct);
                    self.last_moved_secs = session_time_secs;
                }
                _ => {}
            }
            // Where the sim publishes a stall the car is stopped by
            // definition; where it does not, a track position that has not
            // moved for [`STOPPED_CONFIRM_SECS`] says the same thing. The
            // frozen-position case is backdated to the moment the car came to
            // rest, so the confirming wait is not lost from the measured stop.
            let frozen = lap_dist_pct.is_some() && session_time_secs - self.last_moved_secs >= STOPPED_CONFIRM_SECS;
            if frozen {
                self.stopped_since.get_or_insert(self.last_moved_secs);
            } else if in_pit_stall {
                self.stopped_since.get_or_insert(session_time_secs);
            } else if let Some(since) = self.stopped_since.take() {
                self.stopped_total_secs += (session_time_secs - since).max(0.0);
            }
        } else {
            if self.was_in_pit_lane {
                if let Some(since) = self.stopped_since.take() {
                    self.stopped_total_secs += (session_time_secs - since).max(0.0);
                }
                visit_outcome = Some(self.close_lane_visit(session_time_secs, lap));
            }
            if self.current_start_secs.is_none() {
                self.current_start_secs = Some(session_time_secs);
                self.current_start_lap = Some(lap);
                // A lap-zero baseline is the beginning of a race. Otherwise
                // it is complete only when this tick is an observed exit from
                // the initial garage, rather than the first tick received
                // halfway through somebody's stint.
                self.current_start_observed = lap <= 0 || (left_pit_lane && !resumed_after_gap);
            }
        }
        self.was_in_pit_lane = in_pit_lane;
        self.seen_in_world = true;

        if let Some(outcome) = visit_outcome {
            self.record_lane_outcome(outcome);
            self.reconcile_official_stops(
                session_time_secs,
                lap,
                false,
                matches!(outcome, LaneVisitOutcome::Stop),
                false,
            );
        }

        self.reading(session_time_secs, lap)
    }

    fn reading(&self, session_time_secs: f64, lap: i32) -> StintReading {
        StintReading {
            current_secs: self.current_start_secs.map_or(0.0, |s| (session_time_secs - s).max(0.0)),
            current_laps: self.current_start_lap.map_or(0, |l| (lap - l).max(0)),
            avg_secs: lower_median_f64(self.completed.iter().map(|&(_, secs)| secs)),
            avg_laps: lower_median_i32(self.completed.iter().map(|&(laps, _)| laps)),
            last_pit_secs: self.last_pit_secs,
            avg_pit_secs: lower_median_f64(self.pit_secs.iter().copied()),
            completed_stops: self.completed_stop_count(),
        }
    }

    /// Records only genuine scorer rows. The first is a baseline, not a fresh
    /// stop; later monotonic increases are reconciled with lane outcomes.
    fn observe_official_stops(&mut self, stops: Option<i32>) {
        let Some(stops) = stops.map(|stops| stops.max(0)) else { return };
        match self.official_stops_seen {
            None => {
                let local = i32::try_from(self.pit_count).unwrap_or(i32::MAX);
                self.official_offset = (stops - local).max(0);
                self.local_unscored = u32::try_from((local - stops).max(0)).unwrap_or(u32::MAX);
                self.official_stops_seen = Some(stops);
            }
            Some(previous) if stops > previous => {
                self.official_unmatched = self
                    .official_unmatched
                    .saturating_add(u32::try_from(stops - previous).unwrap_or(u32::MAX));
                self.official_stops_seen = Some(stops);
            }
            _ => {}
        }
    }

    fn record_lane_outcome(&mut self, outcome: LaneVisitOutcome) {
        match outcome {
            LaneVisitOutcome::Stop => {
                // This boundary is newer than any earlier uncertain exit, so
                // later confirmation may affect the count but must not rewind
                // the current stint to that older boundary.
                if let Some((_, _, reset_boundary)) = self.pending_uncertain_exit.as_mut() {
                    *reset_boundary = false;
                }
                if self.official_unmatched > 0 {
                    self.official_unmatched -= 1;
                } else {
                    self.local_unscored = self.local_unscored.saturating_add(1);
                }
            }
            LaneVisitOutcome::DriveThrough => {
                self.pending_drive_throughs = self.pending_drive_throughs.saturating_add(1);
            }
            LaneVisitOutcome::Uncertain { exit_secs, exit_lap } => {
                self.pending_uncertain_exit = Some((exit_secs, exit_lap, true));
            }
            LaneVisitOutcome::Ignored => {}
        }
    }

    /// Matches scorer increases to already-known events before treating one
    /// as an otherwise invisible stop. Official-only events reset the current
    /// boundary but never add range or service samples.
    fn reconcile_official_stops(
        &mut self,
        session_time_secs: f64,
        lap: i32,
        lane_visit_active: bool,
        current_boundary_just_reset: bool,
        returned_after_gap: bool,
    ) {
        let locally_matched = self.official_unmatched.min(self.local_unscored);
        self.official_unmatched -= locally_matched;
        self.local_unscored -= locally_matched;

        let drives_matched = self.official_unmatched.min(self.pending_drive_throughs);
        self.official_unmatched -= drives_matched;
        self.pending_drive_throughs -= drives_matched;
        self.add_official_offset(drives_matched);

        if self.official_unmatched > 0
            && let Some((exit_secs, exit_lap, reset_boundary)) = self.pending_uncertain_exit.take()
        {
            self.official_unmatched -= 1;
            self.add_official_offset(1);
            if reset_boundary && !current_boundary_just_reset {
                self.reset_from_official(exit_secs, exit_lap, true);
            }
        }

        if self.official_unmatched == 0 || lane_visit_active {
            return;
        }
        let unmatched = std::mem::take(&mut self.official_unmatched);
        self.add_official_offset(unmatched);
        if returned_after_gap && !current_boundary_just_reset {
            self.reset_from_official(session_time_secs, lap, false);
        }
    }

    fn add_official_offset(&mut self, stops: u32) {
        self.official_offset = self
            .official_offset
            .saturating_add(i32::try_from(stops).unwrap_or(i32::MAX));
    }

    fn reset_from_official(&mut self, session_time_secs: f64, lap: i32, exit_observed: bool) {
        self.current_start_secs = Some(session_time_secs);
        self.current_start_lap = Some(lap);
        self.current_start_observed = exit_observed;
        self.last_pit_secs = None;
    }

    fn completed_stop_count(&self) -> i32 {
        let local = self
            .official_offset
            .saturating_add(i32::try_from(self.pit_count).unwrap_or(i32::MAX));
        self.official_stops_seen.map_or(local, |official| official.max(local))
    }

    /// Settles a finished pit-lane visit: a stop where the car spent
    /// [`MIN_STOP_SECS`] of it stationary, a drive-through where it was
    /// watched throughout and did not.
    ///
    /// A drive-through is left to run through the stint deliberately. The
    /// penalty puts no fuel in the car, so the stint it interrupts is still
    /// running: ending it there would report a range the car does not have and
    /// add a stop to every projection counted off it.
    ///
    /// A stop only counts where there was a stint to end. Every car begins a
    /// session parked in its own box, and driving out of it to take the grid
    /// is not a pit stop — counted as one it gives a sprint race with no stops
    /// in it a stop for every car, a first "stint" one out-lap long, and from
    /// that a remaining-stop projection for the whole field that is pure
    /// noise.
    fn close_lane_visit(&mut self, session_time_secs: f64, lap: i32) -> LaneVisitOutcome {
        let stopped_secs = self.stopped_total_secs;
        let entered = self.lane_entered.take();
        self.stopped_total_secs = 0.0;
        self.anchor_pct = None;
        let service_observed = stopped_secs >= MIN_STOP_SECS;
        let blind_stop_inferred = self.blind_secs > POSITION_BLIND_TOLERANCE_SECS && !self.lane_visit_interrupted;
        let service_time_uncertain =
            self.lane_visit_interrupted || (self.blind_secs > POSITION_BLIND_TOLERANCE_SECS && !self.stall_observed);
        self.blind_secs = 0.0;
        // A visit first seen after the car vanished may be a tow or garage
        // stay. A visit entered normally is still a stop when service was
        // observed, or (as a conservative fallback for sparse rival data)
        // when its whole continuous transit was position-blind.
        if !self.lane_visit_observed && self.current_start_secs.is_none() {
            return LaneVisitOutcome::Ignored;
        }
        if !self.lane_visit_observed {
            return LaneVisitOutcome::Uncertain { exit_secs: session_time_secs, exit_lap: lap };
        }
        if !service_observed && !blind_stop_inferred {
            return if self.lane_visit_interrupted {
                LaneVisitOutcome::Uncertain { exit_secs: session_time_secs, exit_lap: lap }
            } else {
                LaneVisitOutcome::DriveThrough
            };
        }
        let (Some((start_secs, start_lap)), Some((lane_secs, _))) =
            (self.current_start_secs.zip(self.current_start_lap), entered)
        else {
            return LaneVisitOutcome::Uncertain { exit_secs: session_time_secs, exit_lap: lap };
        };
        // Use exit-to-exit lap counts, matching the current stint's reset
        // below. Where the lane crosses the timing line, measuring to entry
        // but restarting at exit silently removes one lap from every tank.
        let stint_laps = (lap - start_lap).max(0);
        // A stop a few laps in was for damage or a penalty, not fuel — see
        // `MIN_FUEL_STINT_LAPS`. It stays a stop; it just says nothing about
        // the car's range.
        if !self.lane_visit_interrupted && self.current_start_observed && stint_laps >= MIN_FUEL_STINT_LAPS {
            if self.completed.len() >= STINT_SAMPLES {
                self.completed.pop_front();
            }
            self.completed.push_back((stint_laps, (lane_secs - start_secs).max(0.0)));
        }
        // An unwatched visit can reset the stint without supplying a service
        // measurement. Recording zero here makes NET price later stops as
        // drive-throughs immediately after the first such visit.
        self.last_pit_secs = (service_observed && !service_time_uncertain).then_some(stopped_secs);
        if let Some(measured_secs) = self.last_pit_secs {
            if self.pit_secs.len() >= STINT_SAMPLES {
                self.pit_secs.pop_front();
            }
            self.pit_secs.push_back(measured_secs);
        }
        self.pit_count = self.pit_count.saturating_add(1);
        self.current_start_secs = Some(session_time_secs);
        self.current_start_lap = Some(lap);
        self.current_start_observed = true;
        LaneVisitOutcome::Stop
    }
}

/// The lower median, or `None` of an empty history.
///
/// Lower rather than upper on an even count, deliberately: a car whose recent
/// stints read `[17, 16]` is tightening — going a lap early — and the shorter
/// figure is the one its next stop will look like. For the stop times the
/// same choice discounts the long side, which is where the outliers live
/// (penalties served in the box, repairs, driver swaps).
fn lower_median_i32(values: impl Iterator<Item = i32>) -> Option<i32> {
    let mut sorted: Vec<i32> = values.collect();
    sorted.sort_unstable();
    sorted.get(sorted.len().checked_sub(1)? / 2).copied()
}

/// The lower median, or `None` of an empty history — see [`lower_median_i32`].
fn lower_median_f64(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sorted: Vec<f64> = values.collect();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len().checked_sub(1)? / 2).copied()
}

/// All cars' stint-tracking state, held across ticks for one session.
/// The tank's size, held once it has been seen.
///
/// The session YAML publishes it (`DriverCarFuelMaxLtr`, scaled by the
/// session's `DriverCarMaxFuelPct`), and that is what is used wherever it is
/// there. Where it is not, the size is inferred from `FuelLevel /
/// FuelLevelPct`, which is only trusted with a reasonably full tank — see
/// `TANK_ESTIMATE_MIN_PCT`. Read afresh every tick, that inference vanished
/// for the whole of a low-fuel stint, and the Fuel page drew a tank with no
/// scale exactly when the driver was deciding a load. A tank does not change
/// size mid-session, so the last good figure stands until the session does.
#[derive(Debug, Default)]
struct TankCapacity {
    latched_litres: Option<f32>,
}

impl TankCapacity {
    /// Takes this tick's estimate, if there is one, and returns the best
    /// known size.
    fn latch(&mut self, estimate_litres: Option<f32>) -> Option<f32> {
        if let Some(litres) = estimate_litres.filter(|litres| litres.is_finite() && *litres > 0.0) {
            self.latched_litres = Some(litres);
        }
        self.latched_litres
    }
}

/// Counts each car's trips off the track, from the sim's own surface flag.
///
/// The Standings gutter already lights while a car is off; a count is what
/// turns that flash into a fact — this driver has been off four times — which
/// is what a car behind wants to know before committing to a move on it. An
/// excursion has to last [`OFF_TRACK_MIN_SECS`] to count, and a car that
/// is back off within [`OFF_TRACK_MERGE_SECS`] of its last one is still on
/// the same excursion: the sim's surface flag flickers over a kerb and across
/// a gravel trap, and read literally it counted one trip through the grass
/// as three. Even so this is the surface flag, not the sim's incident count —
/// iRacing's `1x` needs all four wheels off, the flag does not — so the tally
/// runs a little ahead of the incident points.
#[derive(Debug, Default)]
pub struct OffTrackCounter {
    cars: HashMap<i32, OffTrack>,
}

/// How long a car has to be off before it counts, and how soon after coming
/// back a second trip off is the same one.
const OFF_TRACK_MIN_SECS: f64 = 0.5;
const OFF_TRACK_MERGE_SECS: f64 = 3.0;

/// One car's off-track history.
#[derive(Debug, Default, Clone, Copy)]
struct OffTrack {
    /// When the current run of off-track ticks began, while it lasts.
    off_since_secs: Option<f64>,
    /// Whether the current run has been counted (or merged into the last).
    counted: bool,
    /// When the car last came back onto the track.
    back_at_secs: Option<f64>,
    count: i32,
}

impl OffTrackCounter {
    /// Feeds one tick for `car_idx` at `now_secs` and returns its count so far.
    pub fn update(&mut self, car_idx: i32, off_track: bool, now_secs: f64) -> i32 {
        let car = self.cars.entry(car_idx).or_default();
        match (off_track, car.off_since_secs) {
            (true, None) => {
                car.off_since_secs = Some(now_secs);
                // Straight back off after coming on: the same excursion.
                car.counted = car.back_at_secs.is_some_and(|back| now_secs - back < OFF_TRACK_MERGE_SECS);
            }
            (true, Some(since)) => {
                if !car.counted && now_secs - since >= OFF_TRACK_MIN_SECS {
                    car.count = car.count.saturating_add(1);
                    car.counted = true;
                }
            }
            (false, Some(_)) => {
                car.off_since_secs = None;
                // A run too short to count leaves nothing to merge into.
                car.back_at_secs = car.counted.then_some(now_secs);
            }
            (false, None) => {}
        }
        car.count
    }

    /// How often `car_idx` has been off so far.
    #[must_use]
    pub fn count(&self, car_idx: i32) -> i32 {
        self.cars.get(&car_idx).map_or(0, |car| car.count)
    }
}

#[derive(Debug, Default)]
struct StintTracker {
    cars: HashMap<i32, StintState>,
}

impl StintTracker {
    #[expect(clippy::too_many_arguments, reason = "one normalized per-car telemetry sample")]
    fn update(
        &mut self,
        car_idx: i32,
        session_time_secs: f64,
        lap: i32,
        official_stops: Option<i32>,
        in_world: bool,
        in_pit_lane: bool,
        in_pit_stall: bool,
        lap_dist_pct: Option<f32>,
    ) -> StintReading {
        self.cars.entry(car_idx).or_default().update_with_official(
            session_time_secs,
            lap,
            official_stops,
            in_world,
            in_pit_lane,
            in_pit_stall,
            lap_dist_pct,
        )
    }
}

/// Ground speed below which the player's car counts as standing still, in m/s.
///
/// The player's own `Speed` is published directly and exactly, so this only
/// has to clear the sim's noise around zero — not the metre-scale wobble
/// [`PIT_MOVED_PCT`] exists for on another car's track position. Half a metre
/// a second is 1.8 km/h: a car still rolling into its box reads well above
/// it, a serviced car reads zero.
const PLAYER_STOPPED_MPS: f32 = 0.5;

/// One tick of the player's own car, as [`PlayerStops`] reads it.
#[derive(Debug, Clone, Copy)]
struct PlayerStopTick {
    session_time_secs: f64,
    /// Whether the car is anywhere on pit road, box or lane.
    on_pit_road: bool,
    /// `PlayerCarInPitStall`: the sim's own word that the car is in its box.
    /// `None` where the session does not publish the variable at all, which
    /// widens the stationary test to pit road as a whole rather than losing
    /// the count altogether.
    in_box: Option<bool>,
    /// `Speed` in m/s, `None` where the session does not publish it.
    speed_mps: Option<f32>,
    /// Whether the sim is simulating the car at all this tick.
    in_world: bool,
    /// This car's `ResultsPositions` stop count, where the session has scored
    /// it yet.
    official_stops: Option<i32>,
}

/// The player's own pit stops, counted from their own car's telemetry.
///
/// Separate from [`StintState`], which counts every car's stops from the one
/// signal every car publishes: a track position that stops moving. That
/// inference is the best available for a rival and the wrong tool for the
/// player, whose car publishes both `PlayerCarInPitStall` and an exact
/// `Speed` — no confirmation delay, no movement threshold, and no way to
/// mistake a crawl down the lane for a car at rest.
///
/// The official figure is still used, but only as a starting offset for stops
/// made before the overlay was watching, and only until this has measured a
/// stop of its own. Taking the larger of the two instead — what every other
/// row still does — lets anything the sim counts differently, a drive-through
/// or a tow or a garage exit, stand as the count with no way to come back
/// down.
#[derive(Debug, Default)]
struct PlayerStops {
    /// Stops made before this started watching; see the type docs.
    baseline: i32,
    /// Set once a countable visit begins, after which `baseline` holds still:
    /// from that moment on the sim's count and this one describe the same
    /// stops, and a baseline still tracking the sim would report them twice.
    baseline_locked: bool,
    /// Whether the car was on pit road at the previous tick.
    was_on_pit_road: bool,
    /// Whether the car has been seen off pit road since this session began.
    ///
    /// Every session starts with the car parked in its own box, and driving
    /// out of it to take the grid is not a pit stop — the same case
    /// [`StintState::close_lane_visit`] guards for every other car.
    has_raced: bool,
    /// Seconds the car has spent stationary in its box this visit.
    stationary_secs: f64,
    /// `SessionTime` at the previous tick, for the interval above.
    last_tick_secs: f64,
    /// Stops measured here since this session began.
    measured: i32,
}

impl PlayerStops {
    /// Folds one tick in and returns the player's stop count for this session.
    fn update(&mut self, tick: PlayerStopTick) -> i32 {
        if !self.baseline_locked
            && let Some(official) = tick.official_stops
        {
            self.baseline = official;
        }
        // A car the sim is not simulating — sitting in the garage, or in the
        // moments around a session change — publishes neither a surface nor a
        // speed worth folding in. The visit is held open rather than closed,
        // so a trip to the garage in the middle of a stop stays one stop
        // instead of becoming two.
        if !tick.in_world {
            self.last_tick_secs = tick.session_time_secs;
            return self.total();
        }
        let elapsed = (tick.session_time_secs - self.last_tick_secs).max(0.0);
        self.last_tick_secs = tick.session_time_secs;
        if tick.on_pit_road {
            if self.was_on_pit_road {
                // Time is only ever banked from the second tick of a visit
                // onwards, which is what keeps the gap between joining a
                // session and its first tick out of the total.
                let at_rest =
                    tick.in_box.unwrap_or(true) && tick.speed_mps.is_some_and(|mps| mps.abs() < PLAYER_STOPPED_MPS);
                if at_rest {
                    self.stationary_secs += elapsed;
                }
            } else {
                self.stationary_secs = 0.0;
            }
            // The visit about to be measured here is the last one the official
            // figure is allowed to speak for.
            self.baseline_locked |= self.has_raced;
        } else {
            // Leaving pit road settles the visit: a stop where the car stood
            // still in its box for [`MIN_STOP_SECS`] of it, a drive-through or
            // a lane transit where it did not.
            if self.was_on_pit_road && self.has_raced && self.stationary_secs >= MIN_STOP_SECS {
                self.measured = self.measured.saturating_add(1);
            }
            self.stationary_secs = 0.0;
            self.has_raced = true;
        }
        self.was_on_pit_road = tick.on_pit_road;
        self.total()
    }

    /// The stops made before this was watching plus the ones it has measured.
    fn total(&self) -> i32 {
        self.baseline.saturating_add(self.measured)
    }
}

/// How many of a car's most recently completed laps to keep for the
/// "recent pace" figure. Four is short enough that an out-lap or a lap spent
/// in traffic ages out quickly, which is the point of preferring it over a
/// session best.
const RECENT_LAPS_WINDOW: usize = 4;

/// One car's rolling window of recent lap times.
#[derive(Debug, Default)]
struct RecentLaps {
    /// The lap count last time this car's window was updated, so a new lap
    /// completion (`lap` increasing) is only counted once.
    last_seen_lap: Option<i32>,
    /// Oldest lap is at the front; bounded to [`RECENT_LAPS_WINDOW`].
    laps: std::collections::VecDeque<f32>,
}

impl RecentLaps {
    /// Records a newly completed lap's time when `lap` has increased since
    /// the last call, and returns the fastest lap currently in the window
    /// (or `None` if no lap has completed yet).
    fn update(&mut self, lap: i32, last_lap_secs: f32) -> Option<f32> {
        let is_new_lap = self.last_seen_lap.is_none_or(|last| lap > last);
        self.last_seen_lap = Some(lap);
        if is_new_lap && last_lap_secs > 0.0 {
            if self.laps.len() >= RECENT_LAPS_WINDOW {
                self.laps.pop_front();
            }
            self.laps.push_back(last_lap_secs);
        }
        self.laps.iter().copied().min_by(f32::total_cmp)
    }
}

/// How many of the player's most recently completed laps feed the pace figure
/// the lap and stop projections divide the session clock by.
///
/// Five is long enough that one lap in traffic doesn't move the answer, and
/// short enough to follow a real change of pace — tyres going away, a track
/// getting wet — within a couple of laps.
const PACE_WINDOW: usize = 5;

/// How much slower than the window's quickest lap a lap may be and still count
/// as representative pace.
///
/// A driver's green-flag laps vary by a few percent; an out-lap, an in-lap, a
/// lap behind the safety car or one that ran wide are all slower than this by
/// a wide margin. Tighter than this and the figure tracks the best lap rather
/// than the honest average, over-reading the lap count; looser and the
/// standing-start opening lap of a race drags the estimate for the rest of it.
const PACE_OUTLIER_RATIO: f32 = 1.10;

/// The player's own representative pace, as the projections need it.
///
/// Deliberately not `LapLastLapTime` on its own. That is a single sample, and
/// in a sprint race the one lap guaranteed to be unrepresentative — the first,
/// off a standing start and through the first-corner scramble — is the sample
/// the whole of lap two is projected from. A 20-minute race read off it comes
/// out two to three laps short, and every caution or traffic lap afterwards
/// moves the answer again.
#[derive(Debug, Default)]
struct LapPace {
    /// The lap count at the last update, so one completed lap is recorded once.
    last_seen_lap: Option<i32>,
    /// Oldest lap at the front; bounded to [`PACE_WINDOW`].
    laps: std::collections::VecDeque<f32>,
    /// Whether the lap under way has had the car on pit road at any point, so
    /// its time is discarded when it completes — the same rule, and for the
    /// same reason, as [`FuelTracker::lap_touched_pits`].
    lap_touched_pits: bool,
}

impl LapPace {
    /// Records a newly completed lap and returns the pace estimate.
    ///
    /// The mean of the laps in the window within [`PACE_OUTLIER_RATIO`] of the
    /// quickest of them, so traffic laps and caution laps are discarded rather
    /// than averaged in. `None` until a racing lap has been completed.
    ///
    /// Any lap that touched pit road is thrown away before it can reach the
    /// window at all, rather than left to the outlier filter. The filter is
    /// relative to the *quickest lap in the window*, so it only works while
    /// there is a clean lap in there to measure against — and the one moment
    /// there might not be is the moment after a stop, when the window holds an
    /// in-lap, a lap with the crew's twenty seconds standing in it, and an
    /// out-lap. That is also the moment the next fuel load is worked out, so
    /// the failure and the consequence arrive together: a stop-inflated average
    /// halves the laps remaining and the car goes out on a fraction of the fuel
    /// the race needs.
    fn update(&mut self, lap: i32, last_lap_secs: f32, on_pit_road: bool) -> Option<f32> {
        let is_new_lap = self.last_seen_lap.is_none_or(|last| lap > last);
        if is_new_lap {
            if last_lap_secs > 0.0 && !self.lap_touched_pits {
                if self.laps.len() >= PACE_WINDOW {
                    self.laps.pop_front();
                }
                self.laps.push_back(last_lap_secs);
            }
            // The new lap starts tainted if the car is on pit road as it
            // begins, which is exactly an out-lap at a track whose pit exit is
            // past the start/finish line.
            self.lap_touched_pits = on_pit_road;
        } else if on_pit_road {
            self.lap_touched_pits = true;
        }
        self.last_seen_lap = Some(lap);

        let cutoff = self.laps.iter().copied().min_by(f32::total_cmp)? * PACE_OUTLIER_RATIO;
        // The quickest lap always clears its own cutoff, so having found one
        // above, the count below is at least one.
        let (total, count) = self
            .laps
            .iter()
            .filter(|&&secs| secs <= cutoff)
            .fold((0.0_f32, 0_u16), |(sum, n), &secs| (sum + secs, n.saturating_add(1)));
        (count > 0).then(|| total / f32::from(count))
    }
}

/// All cars' recent-lap windows, held across ticks for one session.
#[derive(Debug, Default)]
struct RecentLapsTracker {
    cars: HashMap<i32, RecentLaps>,
}

impl RecentLapsTracker {
    fn samples(&self, car_idx: i32) -> [Option<f32>; 3] {
        let mut samples = [None; 3];
        if let Some(car) = self.cars.get(&car_idx) {
            for (slot, lap) in samples.iter_mut().zip(car.laps.iter().rev()) {
                *slot = Some(*lap);
            }
        }
        samples
    }
    fn update(&mut self, car_idx: i32, lap: i32, last_lap_secs: f32) -> Option<f32> {
        self.cars.entry(car_idx).or_default().update(lap, last_lap_secs)
    }
}

/// Estimates a post-pit *class* position from the current standings, or
/// `None` if there isn't enough data yet (no standings row for the focus
/// car, or the player's class isn't known yet). Restricted to the player's
/// own class so the projected position lines up with Standings'
/// class-relative numbering.
///
/// The player's gap comes from their standings row rather than the raw
/// `CarIdxF2Time` var, because every gap in `standings` has been rebased to
/// the class leader — see [`annotate_classes`] — and the projection has to
/// compare on one base.
fn build_pit_projection(
    standings: &[StandingsEntry],
    pit_loss_secs: f32,
    my_car_class_id: Option<i32>,
) -> Option<PitProjection> {
    let my_car_class_id = my_car_class_id?;
    let me = standings.iter().find(|e| e.is_focus)?;
    let field: Vec<f32> = standings
        .iter()
        .filter(|e| !e.is_focus && e.car_class_id == my_car_class_id)
        .map(|e| e.gap_to_leader_secs)
        .collect();
    Some(PitProjection {
        estimated_position: standings::project_pit_position(me.gap_to_leader_secs, pit_loss_secs, &field),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this clamp exists for, in the numbers it actually happened in.
    ///
    /// Hockenheim, GT3s, a three-hour race with two and a half hours left. A
    /// 97-second lap and one stop makes the rolling average read like a
    /// four-minute lap, and the raw average turns 9000 seconds of racing into
    /// 36 laps instead of 93 — which is what Auto Fuel then loaded for.
    #[test]
    fn a_lap_with_a_pit_stop_in_it_cannot_halve_the_laps_remaining() {
        let best = 97.0;
        let poisoned_average = 250.0;
        let remaining_secs = 9000.0;

        let raw = endurance::laps_remaining(Some(remaining_secs), poisoned_average, Some(0.0));
        assert_eq!(raw, Some(36), "the reading that was actually shown, from the unclamped average");

        let clamped = race_pace_secs(Some(best), poisoned_average, best);
        let honest = endurance::laps_remaining(Some(remaining_secs), clamped, Some(0.0));
        assert_eq!(honest, Some(81), "held to 115% of a 97s best, which is 111.55s a lap");
        assert!(
            honest.expect("a clamped pace always projects") > raw.expect("so does an unclamped one") * 2,
            "the whole failure was projecting a third of the race"
        );
    }

    /// The clamp is a floor as well as a ceiling: a pace quicker than any lap
    /// the driver has turned would project *more* laps than the race has and
    /// load fuel for them, so the best lap is the fastest the projection may
    /// ever assume.
    #[test]
    fn race_pace_is_never_quicker_than_a_lap_actually_turned() {
        assert!((race_pace_secs(Some(97.0), 90.0, 97.0) - 97.0).abs() < f32::EPSILON);
        // And an ordinary race lap, a few percent off the best, passes through
        // untouched — the clamp must not quietly turn every projection into a
        // qualifying simulation.
        assert!((race_pace_secs(Some(97.0), 100.0, 97.0) - 100.0).abs() < f32::EPSILON);
    }

    /// Before either figure exists there is still a race to project, so the
    /// est-time estimate stands in rather than the projection going blank.
    #[test]
    fn race_pace_falls_back_while_no_lap_has_been_completed() {
        assert!((race_pace_secs(None, 0.0, 105.0) - 105.0).abs() < f32::EPSILON);
        assert!((race_pace_secs(None, 99.0, 105.0) - 99.0).abs() < f32::EPSILON);
        assert!((race_pace_secs(Some(97.0), 0.0, 105.0) - 97.0).abs() < f32::EPSILON);
    }

    /// A contact at `secs`, with the metres a racing speed would put it at.
    fn contact(secs: f32) -> radar::Contact {
        radar::Contact { gap_secs: secs, gap_m: secs * 55.0 }
    }

    /// Cars on both sides once drew the *same* nearest pair on both bars, so
    /// two cars rendered as four. A driver reacts to the count before anything
    /// else on this widget, and a count that is wrong is worse than no widget.
    #[test]
    fn cars_on_both_sides_are_split_one_per_bar() {
        let mut smoothing = RadarSmoothing::default();
        let contacts = [contact(0.20), contact(-0.15)];
        let radar = build_radar(CarLeftRight::CarLeftRight, &contacts, 0.5, 0.0, &mut smoothing);

        let drawn = [radar.left.ahead, radar.left.behind, radar.right.ahead, radar.right.behind];
        assert_eq!(drawn.iter().filter(|car| car.is_some()).count(), 2, "two cars must draw as two markers");
        assert!(!is_side_empty(radar.left) && !is_side_empty(radar.right), "each occupied side must hold one");
    }

    /// One side reporting is the unambiguous case: both the car ahead and the
    /// car behind can be hit, and they belong to the side that reported.
    #[test]
    fn one_side_reporting_keeps_both_of_its_cars_and_leaves_the_other_bar_empty() {
        let mut smoothing = RadarSmoothing::default();
        let contacts = [contact(0.20), contact(-0.15)];
        let radar = build_radar(CarLeftRight::CarLeft, &contacts, 0.5, 0.0, &mut smoothing);

        assert!(radar.left.ahead.is_some() && radar.left.behind.is_some());
        assert!(is_side_empty(radar.right));
    }

    #[test]
    fn a_clear_signal_draws_nothing_however_close_the_field_is() {
        let mut smoothing = RadarSmoothing::default();
        let radar = build_radar(CarLeftRight::Clear, &[contact(0.02)], 0.5, 0.0, &mut smoothing);
        assert!(is_side_empty(radar.left) && is_side_empty(radar.right));
    }

    /// The widget's whole axis is metres, so the conversion is the reading.
    /// Track positions against the track's own length, not seconds times
    /// speed, which collapses exactly where a driver most needs the radar.
    #[test]
    fn separation_comes_from_track_positions_when_the_length_is_known() {
        // Half a percent of a 4000 m lap is 20 m up the road.
        let metres = radar_separation_metres(Some(0.100), Some(0.105), Some(4000.0), 0.36);
        assert!((metres - 20.0).abs() < 0.01, "expected 20 m, got {metres}");
    }

    #[test]
    fn separation_takes_the_short_way_round_the_start_finish_line() {
        // One car just before the line, one just after: 8 m apart, not a lap.
        let metres = radar_separation_metres(Some(0.999), Some(0.001), Some(4000.0), 0.14);
        assert!((metres - 8.0).abs() < 0.01, "expected 8 m across the line, got {metres}");
        let behind = radar_separation_metres(Some(0.001), Some(0.999), Some(4000.0), -0.14);
        assert!((behind + 8.0).abs() < 0.01, "expected -8 m across the line, got {behind}");
    }

    /// Without a track length there is nothing to measure against, so the
    /// estimate has to come from the gap — an estimate, but a car drawn
    /// roughly right beats a bar that stays empty next to a car that is there.
    #[test]
    fn separation_falls_back_to_the_gap_when_the_track_length_is_unknown() {
        let metres = radar_separation_metres(Some(0.1), Some(0.2), None, 0.25);
        assert!(metres > 0.0 && metres.is_finite(), "a fallback must still place the car, got {metres}");
        assert!((metres + radar_separation_metres(None, None, None, -0.25)).abs() < 0.01, "and stay symmetric");
    }

    /// Whether a side is showing anything, for the radar tests above.
    fn is_side_empty(side: RadarSide) -> bool {
        side.ahead.is_none() && side.behind.is_none()
    }

    /// Laps down and class-fastest are both class-relative: in a
    /// multi-class race, being a lap behind a quicker class's leader — or
    /// slower than their best lap — says nothing about your own race.
    #[test]
    fn class_annotations_compare_within_a_class_only() {
        let mut entries = vec![
            test_entry(0, 10, 98.0), // GT3 leader
            test_entry(0, 8, 97.5),  // GT3, two laps down, and class-fastest
            test_entry(1, 12, 90.0), // GTE leader, quicker in absolute terms
        ];
        annotate_classes(&mut entries);

        assert_eq!(entries[0].laps_down, 0);
        assert_eq!(entries[1].laps_down, 2, "laps down must count from this car's own class leader");
        assert_eq!(entries[2].laps_down, 0);

        assert!(!entries[0].is_class_fastest);
        assert!(entries[1].is_class_fastest, "the quickest GT3 lap wins its class even while laps down");
        assert!(entries[2].is_class_fastest, "a lone GTE entry holds its own class's fastest lap");
    }

    /// A car that hasn't set a lap yet reads `0.0`, which must not win the
    /// class-fastest comparison against cars that have actually lapped.
    #[test]
    fn a_car_without_a_lap_time_is_not_class_fastest() {
        let mut entries = vec![test_entry(0, 5, 0.0), test_entry(0, 5, 99.0)];
        annotate_classes(&mut entries);
        assert!(!entries[0].is_class_fastest);
        assert!(entries[1].is_class_fastest);
    }

    /// A slow lap must push the oldest lap out of the window rather than
    /// being ignored, so the figure tracks recent pace instead of latching
    /// onto one good lap forever.
    /// One run of off-track ticks is one excursion, however long it lasts;
    /// a flicker over a kerb is none; straight back off is still the same one.
    #[test]
    fn an_excursion_is_counted_once_and_only_once_it_has_lasted() {
        let mut counter = OffTrackCounter::default();
        assert_eq!(counter.update(3, false, 0.0), 0);
        assert_eq!(counter.update(3, true, 1.0), 0, "not yet long enough to be an excursion");
        assert_eq!(counter.update(3, true, 1.2), 0);
        assert_eq!(counter.update(3, true, 1.6), 1, "half a second off is a trip off");
        assert_eq!(counter.update(3, true, 3.0), 1, "however long it goes on");
        assert_eq!(counter.update(3, false, 3.5), 1);
        assert_eq!(counter.update(3, true, 4.0), 1, "back off within the merge window is the same trip");
        assert_eq!(counter.update(3, true, 6.0), 1);
        assert_eq!(counter.update(3, false, 6.5), 1);
        assert_eq!(counter.update(3, true, 20.0), 1);
        assert_eq!(counter.update(3, true, 20.6), 2, "a fresh trip, well clear of the last");
        assert_eq!(counter.count(3), 2);
        assert_eq!(counter.count(4), 0, "a car never seen has no history");
    }

    /// A kerb strike that shows as a single off-track tick counts nothing.
    #[test]
    fn a_flicker_off_the_track_is_not_an_excursion() {
        let mut counter = OffTrackCounter::default();
        assert_eq!(counter.update(1, true, 10.0), 0);
        assert_eq!(counter.update(1, false, 10.1), 0);
        assert_eq!(counter.update(1, true, 10.2), 0);
        assert_eq!(counter.update(1, false, 10.3), 0);
        assert_eq!(counter.count(1), 0);
    }

    #[test]
    fn recent_laps_keep_only_the_last_four() {
        let mut recent = RecentLaps::default();
        for (lap, time) in [(1, 100.0), (2, 99.0), (3, 98.0), (4, 97.0)] {
            recent.update(lap, time);
        }
        assert_eq!(recent.update(4, 97.0), Some(97.0), "a repeated lap number must not re-record");
        // Window is now [99, 98, 97, 150]: the 100.0 lap has aged out.
        assert_eq!(recent.update(5, 150.0), Some(97.0));
        for (lap, time) in [(6, 150.0), (7, 150.0), (8, 150.0)] {
            recent.update(lap, time);
        }
        assert_eq!(recent.update(9, 150.0), Some(150.0), "once every recent lap is slow, so is the figure");
    }

    #[test]
    fn a_car_with_no_completed_laps_has_no_recent_pace() {
        assert_eq!(RecentLaps::default().update(1, 0.0), None);
    }

    /// A lap run wholly on track, with no lap time published.
    fn racing_lap(fuel: &mut FuelTracker, lap: i32, level: f32) -> Option<f32> {
        fuel.update(lap, level, 0.0, false)
    }

    /// A lap run wholly on track; `secs` is the time of the lap just completed.
    fn timed_lap(fuel: &mut FuelTracker, lap: i32, level: f32, secs: f32) -> Option<f32> {
        fuel.update(lap, level, secs, false)
    }

    #[test]
    fn fuel_use_needs_a_completed_lap_before_it_says_anything() {
        let mut fuel = FuelTracker::default();
        assert_eq!(racing_lap(&mut fuel, 1, 60.0), None);
        assert_eq!(racing_lap(&mut fuel, 1, 58.0), None, "mid-lap drain is not a measurement");
        assert_eq!(racing_lap(&mut fuel, 2, 57.0), Some(3.0), "60 at the start of lap 1, 57 at the start of lap 2");
    }

    #[test]
    fn refuelling_is_not_counted_as_a_lap_of_use() {
        let mut fuel = FuelTracker::default();
        racing_lap(&mut fuel, 1, 60.0);
        racing_lap(&mut fuel, 2, 57.0);
        // Pitted: the tank is fuller at the next lap change than before it.
        assert_eq!(racing_lap(&mut fuel, 3, 90.0), Some(3.0), "the stop leaves the figure alone");
        assert_eq!(racing_lap(&mut fuel, 4, 87.0), Some(3.0));
    }

    /// The number this exists to protect. An in-lap is part pit lane and uses
    /// a fraction of a racing lap's fuel; taken as a lap of running it drags
    /// the figure down at the one moment it is being used to fill the tank —
    /// which is how a load that read 22 litres all race went in as 11.
    #[test]
    fn a_lap_that_touched_pit_road_is_not_a_lap_of_racing() {
        let mut fuel = FuelTracker::default();
        racing_lap(&mut fuel, 1, 100.0);
        assert_eq!(racing_lap(&mut fuel, 2, 97.0), Some(3.0));

        // Lap 2 runs normally until the car turns into the pit lane.
        fuel.update(2, 95.0, 0.0, true);
        // It completes the lap in the pit lane having used one litre.
        assert_eq!(fuel.update(3, 96.0, 0.0, true), Some(3.0), "the in-lap is discarded, not averaged in");
    }

    /// The out-lap is the same problem the other way round: at a track whose
    /// pit exit is past the line, the new lap starts on pit road.
    #[test]
    fn an_out_lap_is_discarded_too() {
        let mut fuel = FuelTracker::default();
        racing_lap(&mut fuel, 1, 100.0);
        assert_eq!(racing_lap(&mut fuel, 2, 97.0), Some(3.0));
        // Lap 3 begins on pit road, and uses a litre.
        fuel.update(3, 96.0, 0.0, true);
        assert_eq!(racing_lap(&mut fuel, 4, 95.0), Some(3.0), "the out-lap is discarded");
    }

    /// The thirstiest recent lap, not the mean of them. Consumption swings
    /// with traffic, tow and lifting, and the two errors are not equal: a
    /// spare litre costs a moment in the pit lane, a missing one ends the
    /// race. A mean is short about half the time by construction.
    #[test]
    fn fuel_use_plans_on_the_thirstiest_recent_lap() {
        let mut fuel = FuelTracker::default();
        racing_lap(&mut fuel, 1, 100.0);
        for (lap, level) in [(2, 98.0), (3, 96.0), (4, 94.0)] {
            racing_lap(&mut fuel, lap, level);
        }
        assert_eq!(racing_lap(&mut fuel, 5, 86.0), Some(8.0), "an 8-litre lap sets the figure, not a 2-litre one");
    }

    /// The window has to move on, or one thirsty lap early in a stint fuels
    /// the rest of the race.
    #[test]
    fn fuel_use_forgets_laps_beyond_the_window() {
        let mut fuel = FuelTracker::default();
        racing_lap(&mut fuel, 1, 100.0);
        racing_lap(&mut fuel, 2, 92.0); // an 8-litre lap
        let mut level = 92.0_f32;
        let window = i32::try_from(FUEL_WINDOW).expect("a handful of laps");
        for lap in 3..=(3 + window) {
            level -= 2.0;
            racing_lap(&mut fuel, lap, level);
        }
        assert_eq!(fuel.recent.len(), FUEL_WINDOW);
        assert_eq!(racing_lap(&mut fuel, 20, level - 2.0), Some(2.0), "the 8-litre lap has aged out");
    }

    /// The GT3-at-Le-Mans failure: a stretch of 2:04s in traffic, each a
    /// lifted lap-and-a-half short on burn, used to wash the honest 2:01
    /// figures out of the window one by one — and the fill planned on what
    /// was left came up short of the flag.
    #[test]
    fn an_off_pace_lap_cannot_drag_the_fuel_figure_down() {
        let mut fuel = FuelTracker::default();
        timed_lap(&mut fuel, 1, 100.0, 0.0);
        // Three clean 2:01s at three litres a lap.
        timed_lap(&mut fuel, 2, 97.0, 121.0);
        timed_lap(&mut fuel, 3, 94.0, 121.0);
        assert_eq!(timed_lap(&mut fuel, 4, 91.0, 121.0), Some(3.0));
        // Four 2:04s stuck in traffic, a litre and a half each.
        let mut level = 91.0_f32;
        for lap in 5..=8 {
            level -= 1.5;
            assert_eq!(timed_lap(&mut fuel, lap, level, 124.0), Some(3.0), "an off-pace lap is not believed");
        }
    }

    /// Rain, or a driver stretching a stint: when every lap is the slow kind,
    /// it is the pace, and planning fuel for a pace no longer being driven
    /// would overfill every stop from there on.
    #[test]
    fn a_sustained_change_of_pace_is_eventually_believed() {
        let mut fuel = FuelTracker::default();
        timed_lap(&mut fuel, 1, 100.0, 0.0);
        timed_lap(&mut fuel, 2, 97.0, 121.0);
        assert_eq!(timed_lap(&mut fuel, 3, 94.0, 121.0), Some(3.0));
        let mut level = 94.0_f32;
        let window = i32::try_from(FUEL_WINDOW).expect("a handful of laps");
        for lap in 4..(3 + window) {
            level -= 1.5;
            assert_eq!(timed_lap(&mut fuel, lap, level, 124.0), Some(3.0), "not believed yet");
        }
        level -= 1.5;
        assert_eq!(
            timed_lap(&mut fuel, 3 + window, level, 124.0),
            Some(1.5),
            "a full window of slow laps in a row is the new pace, not an anomaly"
        );
    }

    /// The asymmetry rejection carries: a slow lap that out-burnt the window
    /// was a lap spent fighting, and raising the plan is the safe direction,
    /// so the clock does not get to veto it.
    #[test]
    fn a_slow_but_thirsty_lap_still_raises_the_figure() {
        let mut fuel = FuelTracker::default();
        timed_lap(&mut fuel, 1, 100.0, 0.0);
        assert_eq!(timed_lap(&mut fuel, 2, 97.0, 121.0), Some(3.0));
        // A 2:04 spent wheel-to-wheel: five litres went through the engine.
        assert_eq!(timed_lap(&mut fuel, 3, 92.0, 124.0), Some(5.0), "thirsty counts, whatever the clock says");
    }

    /// One clean lap between two stretches of traffic starts the count over;
    /// only an unbroken run of slow laps may reseed the window.
    #[test]
    fn a_clean_lap_resets_the_off_pace_count() {
        let mut fuel = FuelTracker::default();
        timed_lap(&mut fuel, 1, 100.0, 0.0);
        assert_eq!(timed_lap(&mut fuel, 2, 97.0, 121.0), Some(3.0));
        // Four slow laps, one short of a full window of them.
        let mut level = 97.0_f32;
        for lap in 3..=6 {
            level -= 1.5;
            timed_lap(&mut fuel, lap, level, 124.0);
        }
        // A clear lap at pace, then traffic again.
        level -= 3.0;
        assert_eq!(timed_lap(&mut fuel, 7, level, 121.0), Some(3.0));
        for lap in 8..=11 {
            level -= 1.5;
            assert_eq!(timed_lap(&mut fuel, lap, level, 124.0), Some(3.0), "the count restarted at the clear lap");
        }
    }

    #[test]
    fn pressure_falls_back_to_cold_until_the_sim_publishes_a_stop() {
        let mut pressures = StopPressures::default();
        // Wear holding still is the whole stint: nothing to sample yet.
        assert_eq!(pressures.update(flat_wear(1.0), [180.0; 4]), None);
        assert_eq!(pressures.update(flat_wear(1.0), [185.0; 4]), None);
    }

    #[test]
    fn the_stop_refresh_samples_the_pressure_that_came_off() {
        let mut pressures = StopPressures::default();
        pressures.update(flat_wear(1.0), [180.0; 4]);
        // Wear jumping to its post-stop values is the sim republishing, and
        // the pressure riding along with it is the one the tyre came off at.
        assert_eq!(pressures.update(flat_wear(0.807), [178.7; 4]), Some([178.7; 4]));
        // It must then hold all the way through the next stint, rather than
        // tracking the fresh set warming up.
        for live in [160.0, 172.0, 185.0] {
            assert_eq!(pressures.update(flat_wear(0.807), [live; 4]), Some([178.7; 4]));
        }
        // And the next stop replaces it.
        assert_eq!(pressures.update(flat_wear(0.62), [181.4; 4]), Some([181.4; 4]));
    }

    /// A stop is refreshed wholesale, so every corner moves on the same tick.
    fn flat_wear(value: f32) -> [[f32; 3]; 4] {
        [[value; 3]; 4]
    }

    /// A field of three: the player at 0, a rival at 1, and the pace car at 2.
    fn spectated_field(player_races: bool) -> HashMap<i32, DriverMeta> {
        HashMap::from([(0, test_driver(10, player_races)), (1, test_driver(10, true)), (2, test_driver(10, false))])
    }

    #[test]
    fn a_driver_stays_centred_on_their_own_car_whatever_the_camera_is_doing() {
        let drivers = spectated_field(true);
        assert_eq!(resolve_focus_car(&drivers, 0, Some(1), None), 0);
        assert_eq!(resolve_focus_car(&drivers, 0, None, Some(1)), 0);
    }

    #[test]
    fn a_spectator_follows_the_camera() {
        let drivers = spectated_field(false);
        assert_eq!(resolve_focus_car(&drivers, 0, Some(1), None), 1);
    }

    #[test]
    fn a_player_with_no_entry_at_all_follows_the_camera() {
        let drivers = HashMap::from([(1, test_driver(10, true))]);
        assert_eq!(resolve_focus_car(&drivers, -1, Some(1), None), 1);
    }

    #[test]
    fn a_camera_on_nothing_that_races_holds_the_car_it_had() {
        let drivers = spectated_field(false);
        // Between cameras, on the pace car, and on a slot with no entry: all
        // three would otherwise throw the view onto a car nobody chose.
        assert_eq!(resolve_focus_car(&drivers, 0, Some(-1), Some(1)), 1);
        assert_eq!(resolve_focus_car(&drivers, 0, Some(2), Some(1)), 1);
        assert_eq!(resolve_focus_car(&drivers, 0, Some(9), Some(1)), 1);
        assert_eq!(resolve_focus_car(&drivers, 0, None, Some(1)), 1);
    }

    #[test]
    fn a_spectator_with_nothing_to_follow_yet_falls_back_to_their_own_entry() {
        let drivers = spectated_field(false);
        assert_eq!(resolve_focus_car(&drivers, 0, None, None), 0);
    }

    /// The player's own customer id in the seat tests below.
    const ME: i32 = 111;

    /// A team of two: the player's car at 0 with `driver` in it, a rival at 1.
    fn team_field(driver: Option<i32>) -> HashMap<i32, DriverMeta> {
        let ours = DriverMeta { user_name: Arc::from("Istvan Fodor"), user_id: driver, ..test_driver(10, true) };
        HashMap::from([(0, ours), (1, test_driver(10, true))])
    }

    /// The seat inputs for a team session, centred on the player's own car
    /// with it out in the world.
    fn team_inputs(drivers: &HashMap<i32, DriverMeta>, driving: bool) -> SeatInputs<'_> {
        SeatInputs {
            drivers,
            player_car_idx: 0,
            focus_car_idx: 0,
            driving,
            team_racing: true,
            driver_user_id: Some(ME),
            player_track_location: TrackLocation::OnTrack,
        }
    }

    #[test]
    fn in_the_car_is_driving_whatever_the_yaml_still_says() {
        // The YAML lags the swap by a few seconds; the seat must not.
        let drivers = team_field(Some(222));
        assert_eq!(resolve_seat(team_inputs(&drivers, true)), Seat::Driving);
    }

    #[test]
    fn a_team_mate_in_the_car_is_named() {
        let drivers = team_field(Some(222));
        assert_eq!(resolve_seat(team_inputs(&drivers, false)), Seat::TeamMate(Arc::from("Istvan Fodor")));
    }

    #[test]
    fn a_team_car_out_of_the_world_is_nobodys() {
        let drivers = team_field(Some(222));
        let inputs = SeatInputs { player_track_location: TrackLocation::NotInWorld, ..team_inputs(&drivers, false) };
        assert_eq!(resolve_seat(inputs), Seat::OutOfCar);
    }

    #[test]
    fn the_yaml_still_naming_the_player_is_out_of_car_rather_than_a_team_mate() {
        // Climbing out: `IsOnTrack` drops before the YAML names who is next.
        let drivers = team_field(Some(ME));
        assert_eq!(resolve_seat(team_inputs(&drivers, false)), Seat::OutOfCar);
    }

    #[test]
    fn a_yaml_without_customer_ids_never_reports_a_team_mate() {
        let drivers = team_field(None);
        assert_eq!(resolve_seat(team_inputs(&drivers, false)), Seat::OutOfCar);
    }

    #[test]
    fn a_solo_session_is_never_a_team_mates_whatever_the_entry_says() {
        let drivers = team_field(Some(222));
        let inputs = SeatInputs { team_racing: false, ..team_inputs(&drivers, false) };
        assert_eq!(resolve_seat(inputs), Seat::OutOfCar);
    }

    #[test]
    fn watching_another_car_is_spectating_even_in_a_team_session() {
        let drivers = team_field(Some(222));
        let inputs = SeatInputs { focus_car_idx: 1, ..team_inputs(&drivers, true) };
        assert_eq!(resolve_seat(inputs), Seat::Spectating(Arc::from("")));
    }

    #[test]
    fn the_first_focus_of_a_connection_keeps_the_curve_carried_across_sessions() {
        let mut trackers = SessionTrackers::default();
        let curve = drive_a_measured_lap();
        let measured = curve.measured_lap_secs().expect("the test lap must qualify");
        trackers.lap_curve = curve;

        trackers.sync_to_focus(0, Some(10));

        assert_eq!(trackers.lap_curve.measured_lap_secs(), Some(measured), "nothing has changed focus yet");
    }

    #[test]
    fn following_another_car_in_the_same_class_keeps_the_measured_curve() {
        let mut trackers = SessionTrackers::default();
        trackers.sync_to_focus(0, Some(10));
        trackers.lap_curve = drive_a_measured_lap();
        let measured = trackers.lap_curve.measured_lap_secs();

        trackers.sync_to_focus(1, Some(10));

        assert_eq!(trackers.lap_curve.measured_lap_secs(), measured, "one class spends a lap the same way");
        assert_eq!(trackers.focus_car_idx, Some(1));
    }

    #[test]
    fn following_a_car_in_another_class_throws_the_curve_away() {
        let mut trackers = SessionTrackers::default();
        trackers.sync_to_focus(0, Some(10));
        trackers.lap_curve = drive_a_measured_lap();
        trackers.curve_reported_secs = Some(90.0);
        trackers.reference_reported = true;

        trackers.sync_to_focus(1, Some(20));

        assert_eq!(trackers.lap_curve.measured_lap_secs(), None, "a GT3 and an LMP2 do not");
        assert_eq!(trackers.curve_reported_secs, None, "so the note is worth saying again");
        assert!(!trackers.reference_reported);
    }

    #[test]
    fn a_focus_car_with_no_class_throws_nothing_away() {
        let mut trackers = SessionTrackers::default();
        trackers.sync_to_focus(0, Some(10));
        trackers.lap_curve = drive_a_measured_lap();
        let measured = trackers.lap_curve.measured_lap_secs();

        trackers.sync_to_focus(1, None);

        assert_eq!(trackers.lap_curve.measured_lap_secs(), measured, "an unknown class is not a changed one");
    }

    /// A curve with one clean lap in it, for the tests above to watch survive
    /// or not.
    fn drive_a_measured_lap() -> relative::LapCurve {
        /// Fine enough that no step comes near `CURVE_MAX_STEP`.
        const STEPS: i32 = 200;
        /// Any plausible lap; the tests only compare it against itself.
        const LAP_SECS: f64 = 90.0;

        let mut curve = relative::LapCurve::default();
        let sample = |curve: &mut relative::LapCurve, lap: i32, step: i32| {
            let fraction = f64::from(step) / f64::from(STEPS);
            let secs = (f64::from(lap) + fraction) * LAP_SECS;
            #[expect(clippy::cast_possible_truncation, reason = "a fraction of a lap is far inside f32's range")]
            curve.observe(Some(fraction as f32), secs, true);
        };
        // Two laps: the first only starts the timer at the line, the second is
        // the one that can qualify. Sampled through to the line itself and not
        // merely up to the last step before it — the bins between the two are
        // real bins, and a lap missing any of them does not qualify.
        for lap in 0..2 {
            for step in 0..=STEPS {
                sample(&mut curve, lap, step);
            }
        }
        sample(&mut curve, 2, 0);
        curve
    }

    fn test_driver(car_class_id: i32, is_competitor: bool) -> DriverMeta {
        DriverMeta {
            user_name: Arc::from(""),
            user_id: None,
            car_number: Arc::from(""),
            car_screen_name: Arc::from(""),
            irating: 0,
            license_color: Arc::from(""),
            car_class_id,
            car_class_short_name: Arc::from(""),
            car_class_color: Arc::from(""),
            car_class_rel_speed: None,
            car_class_est_lap_secs: None,
            is_competitor,
            flair_id: 0,
        }
    }

    /// Per-car arrays for a field where nothing but position, lap, track
    /// surface and lap fraction matters — everything [`live_classification`]
    /// reads.
    fn test_arrays<'a>(
        positions: &'a [i32],
        laps: &'a [i32],
        track_surfaces: &'a [i32],
        lap_dist_pcts: &'a [f32],
    ) -> StandingsRawArrays<'a> {
        StandingsRawArrays {
            best_laps: &[],
            last_laps: &[],
            f2_times: &[],
            track_surfaces,
            laps,
            positions,
            lap_dist_pcts,
            session_flags: &[],
        }
    }

    /// On the grid the estimate rates the whole entry list, not the cars that
    /// happen to have gridded so far, so it does not creep as they arrive.
    #[test]
    fn the_irating_field_is_the_entry_list_not_the_cars_on_track() {
        let mut drivers = HashMap::new();
        drivers.insert(3, test_driver(10, true));
        drivers.insert(7, test_driver(10, true));
        drivers.insert(0, test_driver(10, false)); // the pace car
        for driver in drivers.values_mut() {
            driver.irating = 2000;
        }
        let mut on_track = test_entry(10, 0, 0.0);
        on_track.car_idx = 7;
        on_track.position = 1;
        on_track.class_position = 1;
        on_track.irating = 2000;
        let standings = vec![on_track];

        let mut out = Vec::new();
        field_results(&standings, &drivers, false, &mut out);
        let ids: Vec<i32> = out.iter().map(|(idx, _, _)| *idx).collect();
        assert_eq!(ids, vec![7, 3], "the scored car first, then the absent competitor; never the pace car");
        assert_eq!(out[1].2.finish_rank, 2, "an absent car is placed behind the scored field");
        assert!(out[1].2.started, "before the green an absent car is still expected to grid");

        field_results(&standings, &drivers, true, &mut out);
        assert!(!out[1].2.started, "once the race is under way an absent car is a no-show");
    }

    /// Position zero is "not scored yet", not "ahead of first": such cars go
    /// behind the scored field, in standings order, and a driver with no
    /// published rating is not in the field at all.
    #[test]
    fn unscored_cars_rank_behind_the_scored_field_and_unrated_ones_are_left_out() {
        let mut drivers = HashMap::new();
        for car_idx in 1..=3 {
            let mut driver = test_driver(10, true);
            driver.irating = 2000;
            drivers.insert(car_idx, driver);
        }
        let mut scored = test_entry(10, 0, 0.0);
        scored.car_idx = 1;
        scored.position = 1;
        scored.class_position = 1;
        scored.irating = 2000;
        let mut unscored = test_entry(10, 0, 0.0);
        unscored.car_idx = 2;
        unscored.position = 0;
        unscored.class_position = 0;
        unscored.irating = 2000;
        let mut unrated = test_entry(10, 0, 0.0);
        unrated.car_idx = 3;
        unrated.position = 0;
        unrated.class_position = 0;
        unrated.irating = 0;

        let mut out = Vec::new();
        field_results(&[scored, unscored, unrated], &drivers, true, &mut out);
        let ranks: Vec<(i32, u32)> = out.iter().map(|(idx, _, r)| (*idx, r.finish_rank)).collect();
        assert_eq!(ranks, vec![(1, 1), (2, 2)], "unscored behind scored; the unrated car is not rated");
    }

    /// A multi-class race is several races run at once, and iRacing rates it
    /// as such. Pooled into one, a class's winner was rated on where it came
    /// overall — so the slower class was rated as if it had lost to cars it
    /// was never racing, and its own winner could take a loss for winning.
    #[test]
    fn each_class_is_ranked_from_first_within_itself() {
        let mut drivers = HashMap::new();
        for (car_idx, class_id) in [(0, 10), (1, 10), (2, 20), (3, 20)] {
            let mut driver = test_driver(class_id, true);
            driver.irating = 2000;
            drivers.insert(car_idx, driver);
        }
        // The quicker class fills the top two overall places; the slower one
        // the bottom two, but leads its own race from P1.
        let entry = |car_idx, class_id, position, class_position| {
            let mut e = test_entry(class_id, 0, 0.0);
            e.car_idx = car_idx;
            e.position = position;
            e.class_position = class_position;
            e.irating = 2000;
            e
        };
        let standings = vec![entry(0, 10, 1, 1), entry(1, 10, 2, 2), entry(2, 20, 3, 1), entry(3, 20, 4, 2)];

        let mut out = Vec::new();
        field_results(&standings, &drivers, true, &mut out);
        let ranks: Vec<(i32, i32, u32)> = out.iter().map(|(idx, class, r)| (*idx, *class, r.finish_rank)).collect();
        assert_eq!(ranks, vec![(0, 10, 1), (1, 10, 2), (2, 20, 1), (3, 20, 2)], "each class counts from one");
    }

    /// The consequence of the above, through the estimator: two identical
    /// two-car classes must produce identical changes, and both class leaders
    /// must gain — the slower class's winner is a winner, not a third place.
    #[test]
    fn a_slower_classs_winner_is_rated_as_a_winner() {
        let mut drivers = HashMap::new();
        for (car_idx, class_id) in [(0, 10), (1, 10), (2, 20), (3, 20)] {
            let mut driver = test_driver(class_id, true);
            driver.irating = 2000;
            drivers.insert(car_idx, driver);
        }
        let entry = |car_idx, class_id, position, class_position| {
            let mut e = test_entry(class_id, 0, 0.0);
            e.car_idx = car_idx;
            e.position = position;
            e.class_position = class_position;
            e.irating = 2000;
            e
        };
        let standings = vec![entry(0, 10, 1, 1), entry(1, 10, 2, 2), entry(2, 20, 3, 1), entry(3, 20, 4, 2)];

        let mut cache = IratingCache::default();
        let changes = cache.changes(&standings, &drivers, true, true, 0.0).clone();

        assert!(changes[&2] > changes[&3], "the slower class's winner must beat its own runner-up");
        assert!(
            (changes[&0] - changes[&2]).abs() < 0.001,
            "two identical classes must rate identically: {:?} vs {:?}",
            changes[&0],
            changes[&2]
        );
    }

    #[test]
    fn an_unscored_grid_is_ordered_by_track_position() {
        let mut info = SessionInfoCache::default();
        for car_idx in 0..3 {
            info.drivers.insert(car_idx, test_driver(10, true));
        }
        // Nobody has crossed the line yet, so every position reads 0 and the
        // order has to come from how far around the lap each car sits.
        let order = live_classification(&info, test_arrays(&[0, 0, 0], &[0, 0, 0], &[3, 3, 3], &[0.10, 0.30, 0.20]));

        let by_position: Vec<(i32, i32)> = order.iter().map(|c| (c.position, c.car_idx)).collect();
        assert_eq!(by_position, vec![(1, 1), (2, 2), (3, 0)]);
        assert_eq!(order.iter().map(|c| c.class_position).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn scored_cars_lead_unscored_ones_and_class_positions_count_per_class() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(20, true));
        info.drivers.insert(2, test_driver(10, true));
        // Car 1 is scored P1; cars 0 and 2 are not scored at all, so they sort
        // behind it on track order regardless of how far around they are.
        let order = live_classification(&info, test_arrays(&[0, 1, 0], &[0, 1, 0], &[3, 3, 3], &[0.90, 0.10, 0.95]));

        assert_eq!(order.iter().map(|c| c.car_idx).collect::<Vec<_>>(), vec![1, 2, 0]);
        assert_eq!(order.iter().map(|c| c.position).collect::<Vec<_>>(), vec![1, 2, 3]);
        // Class 20 has only car 1; class 10 has cars 2 then 0.
        assert_eq!(order.iter().map(|c| c.class_position).collect::<Vec<_>>(), vec![1, 1, 2]);
    }

    #[test]
    fn the_pace_car_spectators_and_cars_out_of_the_world_are_left_out() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, false)); // pace car or spectator
        info.drivers.insert(2, test_driver(10, true)); // in the garage
        let order = live_classification(&info, test_arrays(&[0, 0, 0], &[0, 0, 0], &[3, 3, -1], &[0.10, 0.50, 0.90]));

        assert_eq!(order.iter().map(|c| c.car_idx).collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn cars_the_sim_has_not_scored_are_kept_behind_the_ones_it_has() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, true));
        info.sessions.push(SessionResults {
            session_num: 0,
            session_type: "Race".to_owned(),
            session_time: "2400.0000 sec".to_owned(),
            session_laps: "unlimited".to_owned(),
            results_positions: vec![ResultsPosition {
                position: 1,
                class_position: 1,
                car_idx: 1,
                laps_complete: 3,
                fastest_time: 90.0,
                last_time: 91.0,
                pit_stops: 0,
            }],
        });
        let arrays = test_arrays(&[0, 1], &[0, 3], &[3, 3], &[0.90, 0.10]);
        let entries = build_standings(
            &info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Scored,
        );

        // Car 1 is scored; car 0 is out there but has no lap yet. Taking the
        // official list alone dropped car 0 entirely — and when car 0 is the
        // player, as it is through every qualifying session until their first
        // lap, losing that row collapses the whole widget to class leaders.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].car_idx, 1, "the scored car leads");
        assert_eq!(entries[0].class_position, 1);
        assert_eq!(entries[1].car_idx, 0, "the unscored car follows it");
        assert_eq!(entries[1].class_position, 2, "numbering stays contiguous, so the window can find it");
    }

    /// The Le Mans tow case: a car out of the world reads `CarIdxLap` `-1`,
    /// and subtracting that raw value from the class leader's lap once showed
    /// a car two laps down as ten laps down — the leader's lap count plus
    /// one, not a gap. The YAML's scored count stands in until the car has
    /// been seen; after that the latch remembers what it saw.
    #[test]
    fn a_towed_car_keeps_its_real_lap_deficit() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, true));
        let scored = |car_idx: i32, position: i32, class_position: i32, laps_complete: i32| ResultsPosition {
            position,
            class_position,
            car_idx,
            laps_complete,
            fastest_time: 90.0,
            last_time: 91.0,
            pit_stops: 0,
        };
        info.sessions.push(SessionResults {
            session_num: 0,
            session_type: "Race".to_owned(),
            session_time: "2400.0000 sec".to_owned(),
            session_laps: "unlimited".to_owned(),
            results_positions: vec![scored(1, 1, 1, 9), scored(0, 2, 2, 7)],
        });

        // Car 0 is on the hook: not in world, live lap -1. The leader is on
        // lap 9. Before the latch, car 0 read 9 - (-1) = ten laps down.
        let towed = test_arrays(&[2, 1], &[-1, 9], &[-1, 3], &[0.50, 0.50]);
        let mut latch = LapLatch::default();
        let entries = build_standings(
            &info,
            Some(0),
            1,
            towed,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut latch,
            0.0,
            StandingsOrder::Scored,
        );
        let aston = entries.iter().find(|entry| entry.car_idx == 0).expect("the towed car keeps its row");
        assert_eq!(aston.laps_down, 2, "the scored count stands in, not the raw -1");

        // Seen in the world on lap 8, then towed again: the latch answers.
        let racing = test_arrays(&[2, 1], &[8, 9], &[3, 3], &[0.50, 0.50]);
        let entries = build_standings(
            &info,
            Some(0),
            1,
            racing,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut latch,
            1.0,
            StandingsOrder::Scored,
        );
        assert_eq!(entries.iter().find(|e| e.car_idx == 0).expect("still rowed").laps_down, 1);
        let towed_again = test_arrays(&[2, 1], &[-1, 9], &[-1, 3], &[0.50, 0.50]);
        let entries = build_standings(
            &info,
            Some(0),
            1,
            towed_again,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut latch,
            2.0,
            StandingsOrder::Scored,
        );
        assert_eq!(
            entries.iter().find(|e| e.car_idx == 0).expect("still rowed").laps_down,
            1,
            "the lap it was genuinely on outranks the stale scored count"
        );
    }

    /// Class positions are derived from the running order, never read from
    /// the YAML — so a `ClassPosition` counting from zero, which iRacing has
    /// been seen writing, cannot hide the class leader or shift every driver
    /// onto a neighbour's row.
    #[test]
    fn class_positions_are_derived_rather_than_trusted() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, true));
        let scored = |car_idx: i32, position: i32, class_position: i32| ResultsPosition {
            position,
            class_position,
            car_idx,
            laps_complete: 3,
            fastest_time: 90.0,
            last_time: 91.0,
            pit_stops: 0,
        };
        info.sessions.push(SessionResults {
            session_num: 0,
            session_type: "Race".to_owned(),
            session_time: "2400.0000 sec".to_owned(),
            session_laps: "unlimited".to_owned(),
            results_positions: vec![scored(1, 1, 0), scored(0, 2, 1)],
        });
        // Live positions agree with the YAML's overall order: car 1 leads.
        let arrays = test_arrays(&[2, 1], &[3, 3], &[3, 3], &[0.10, 0.90]);
        let entries = build_standings(
            &info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Scored,
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].car_idx, 1);
        assert_eq!(entries[0].class_position, 1, "the class leader is P1, not P0");
        assert_eq!(entries[1].class_position, 2);
    }

    /// The Relative is built from `CarIdxPosition` every tick while the YAML's
    /// own order lags a session-info update behind it. Taking the order from
    /// the same live array is what stops the two panels disagreeing about who
    /// is ahead for seconds at a time.
    #[test]
    fn the_running_order_follows_live_telemetry_not_the_slower_yaml() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, true));
        let scored = |car_idx: i32, position: i32| ResultsPosition {
            position,
            class_position: position,
            car_idx,
            laps_complete: 3,
            fastest_time: 90.0,
            last_time: 91.0,
            pit_stops: 0,
        };
        info.sessions.push(SessionResults {
            session_num: 0,
            session_type: "Race".to_owned(),
            session_time: "2400.0000 sec".to_owned(),
            session_laps: "unlimited".to_owned(),
            results_positions: vec![scored(1, 1), scored(0, 2)],
        });
        // The YAML still has car 1 leading; telemetry says car 0 has gone by.
        let arrays = test_arrays(&[1, 2], &[3, 3], &[3, 3], &[0.90, 0.10]);
        let entries = build_standings(
            &info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Scored,
        );

        assert_eq!(entries[0].car_idx, 0, "the pass shows immediately");
        assert_eq!(entries[0].class_position, 1);
        assert_eq!(entries[1].car_idx, 1);
        assert_eq!(entries[1].class_position, 2);
    }

    /// A race field two cars big, scored with car 1 leading, for the
    /// distance-order tests below.
    fn racing_pair() -> SessionInfoCache {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, true));
        let scored = |car_idx: i32, position: i32| ResultsPosition {
            position,
            class_position: position,
            car_idx,
            laps_complete: 3,
            fastest_time: 90.0,
            last_time: 91.0,
            pit_stops: 0,
        };
        info.sessions.push(SessionResults {
            session_num: 0,
            session_type: "Race".to_owned(),
            session_time: "2400.0000 sec".to_owned(),
            session_laps: "unlimited".to_owned(),
            results_positions: vec![scored(1, 1), scored(0, 2)],
        });
        info
    }

    /// The order the distance-based standings produce, by `CarIdx`.
    fn distance_order_of(info: &SessionInfoCache, arrays: StandingsRawArrays<'_>, held: &mut RaceOrder) -> Vec<i32> {
        let entries = build_standings(
            info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Distance(held),
        );
        entries.iter().map(|e| e.car_idx).collect()
    }

    /// The complaint this exists to fix: pass the car ahead down the back
    /// straight and the panel says the old position until the line, because
    /// `CarIdxPosition` moves only at timing checkpoints. Distance order
    /// changes the number the moment the car is by.
    #[test]
    fn a_mid_lap_pass_changes_the_position_before_the_line() {
        let info = racing_pair();
        // The scorer still says car 1 leads (`CarIdxPosition` [2, 1]), but
        // car 0 has driven past: same lap, further round it.
        let arrays = test_arrays(&[2, 1], &[3, 3], &[3, 3], &[0.60, 0.55]);
        let order = distance_order_of(&info, arrays, &mut RaceOrder::default());
        assert_eq!(order, vec![0, 1], "the pass counts at the moment it happens, not at the checkpoint");
    }

    /// A lap in hand outranks being behind on the current lap's fraction:
    /// distance is laps plus the fraction, not the fraction alone.
    #[test]
    fn a_car_a_lap_up_stays_ahead_of_one_further_round_the_lap() {
        let info = racing_pair();
        let arrays = test_arrays(&[2, 1], &[3, 4], &[3, 3], &[0.90, 0.10]);
        let order = distance_order_of(&info, arrays, &mut RaceOrder::default());
        assert_eq!(order, vec![1, 0]);
    }

    /// Two cars genuinely side by side sit inside `CarIdxLapDistPct`'s noise;
    /// without the hysteresis band their positions would trade every tick.
    #[test]
    fn side_by_side_cars_hold_their_positions_through_the_noise() {
        let info = racing_pair();
        let mut held = RaceOrder::default();
        let first = distance_order_of(&info, test_arrays(&[2, 1], &[3, 3], &[3, 3], &[0.500_1, 0.500_0]), &mut held);
        assert_eq!(first, vec![0, 1]);
        // The difference between them flips sign by less than the band, as it
        // does every few ticks while they run wheel to wheel.
        for wobble in [-0.000_1_f32, 0.000_2, -0.000_05] {
            let pcts = [0.500_0 + wobble, 0.500_0];
            let arrays = test_arrays(&[2, 1], &[3, 3], &[3, 3], &pcts);
            let now = distance_order_of(&info, arrays, &mut held);
            assert_eq!(now, vec![0, 1], "positions traded on a {wobble} wobble");
        }
        // A genuine pass clears the band and moves at once.
        let passed = distance_order_of(&info, test_arrays(&[2, 1], &[3, 3], &[3, 3], &[0.499_0, 0.502_0]), &mut held);
        assert_eq!(passed, vec![1, 0]);
    }

    /// The forming grid is ordered by qualifying, not by who gridded first:
    /// iRacing's own slots fill in one by one as cars grid, so the first
    /// driver to grid reads P1 until the cars ahead of them appear.
    #[test]
    fn the_forming_grid_is_ordered_by_qualifying() {
        let mut info = racing_pair();
        // The scorer has only seen car 1 grid, so it reads P1 — but
        // qualifying put car 0 on pole.
        info.qualify_grid.insert(0, (1, 1));
        info.qualify_grid.insert(1, (2, 2));
        let arrays = test_arrays(&[2, 1], &[0, 0], &[3, 3], &[0.0, 0.0]);
        let entries = build_standings(
            &info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Grid,
        );
        assert_eq!(entries[0].car_idx, 0, "pole is pole from the moment the session loads");
        assert_eq!(entries[1].car_idx, 1);
    }

    /// Practice and qualifying order by the quickest lap, per tick: the lap
    /// that just demoted you must not wait for the scorer's next update.
    #[test]
    fn practice_orders_by_the_quickest_lap() {
        let info = racing_pair();
        let best_laps = [95.0_f32, 92.0];
        let arrays = StandingsRawArrays {
            best_laps: &best_laps,
            last_laps: &[],
            f2_times: &[],
            track_surfaces: &[3, 3],
            laps: &[3, 3],
            positions: &[1, 2],
            lap_dist_pcts: &[0.5, 0.5],
            session_flags: &[],
        };
        let entries = build_standings(
            &info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Quickest,
        );
        assert_eq!(entries[0].car_idx, 1, "the quicker lap leads regardless of the scored order");
        assert_eq!(entries[0].class_position, 1);
        assert_eq!(entries[1].car_idx, 0);
    }

    /// The Gap column measures to the class leader. `CarIdxF2Time` carries
    /// the overall leader's offset, which in a multi-class field drowned
    /// every slower class's own gaps.
    #[test]
    fn gaps_rebase_to_each_class_leader() {
        let mut lmp2_leader = test_entry(1, 0, 90.0);
        lmp2_leader.gap_to_leader_secs = 0.0;
        let mut gt3_leader = test_entry(2, 0, 100.0);
        gt3_leader.car_idx = 1;
        gt3_leader.gap_to_leader_secs = 25.0;
        let mut gt3_second = test_entry(2, 0, 101.0);
        gt3_second.car_idx = 2;
        gt3_second.class_position = 2;
        gt3_second.gap_to_leader_secs = 31.5;
        let mut entries = vec![lmp2_leader, gt3_leader, gt3_second];
        annotate_classes(&mut entries);
        assert!(entries[1].gap_to_leader_secs.abs() < f32::EPSILON, "a class leader has no gap to themselves");
        assert!((entries[2].gap_to_leader_secs - 6.5).abs() < 1e-4, "the class-mate's gap is to their own leader");
    }

    #[test]
    fn an_unscored_race_still_produces_standings() {
        let mut info = SessionInfoCache::default();
        info.drivers.insert(0, test_driver(10, true));
        info.drivers.insert(1, test_driver(10, true));
        info.sessions.push(SessionResults {
            session_num: 0,
            session_type: "Race".to_owned(),
            session_time: "2400.0000 sec".to_owned(),
            session_laps: "unlimited".to_owned(),
            results_positions: Vec::new(),
        });
        let arrays = test_arrays(&[0, 0], &[0, 0], &[3, 3], &[0.10, 0.90]);
        let entries = build_standings(
            &info,
            Some(0),
            0,
            arrays,
            &mut StintTracker::default(),
            &mut OffTrackCounter::default(),
            &mut LapLatch::default(),
            0.0,
            StandingsOrder::Scored,
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].car_idx, 1, "the car furthest around the lap leads");
        assert!(entries.iter().any(|e| e.is_focus), "the player must be classified from the grid too");
    }

    fn test_entry(car_class_id: i32, laps_complete: i32, best_lap_secs: f32) -> StandingsEntry {
        StandingsEntry {
            position: 1,
            class_position: 1,
            car_idx: 0,
            driver_name: Arc::from(""),
            car_screen_name: Arc::from(""),
            irating: 0,
            flair_id: 0,
            car_class_id,
            car_class_short_name: Arc::from(""),
            car_class_color: Arc::from(""),
            best_lap_secs,
            last_lap_secs: 0.0,
            gap_to_leader_secs: 0.0,
            pit_stops: 0,
            current_stint_laps: 0,
            current_stint_secs: 0.0,
            avg_stint_laps: None,
            avg_stint_secs: None,
            track_location: TrackLocation::OnTrack,
            // `annotate_classes` reads this as the raw completed-lap count
            // and rewrites it in place as laps behind the class leader.
            laps_down: laps_complete,
            is_class_fastest: false,
            last_pit_secs: None,
            avg_pit_secs: None,
            stops_remaining: None,
            projected_class_position: None,
            is_focus: false,
            off_tracks: 0,
            penalty: None,
            tow_secs: None,
            tyre: None,
            race_position_change: None,
        }
    }

    /// Drives a car out of its box, round a lap and back in, returning the
    /// reading after each tick's `in_pit_stall` state.
    fn run_stint(stint: &mut StintState, ticks: &[(f64, i32, bool)]) -> StintReading {
        let mut reading = stint.update(0.0, 0, true, true, None);
        for &(secs, lap, in_pit_stall) in ticks {
            reading = stint.update(secs, lap, in_pit_stall, in_pit_stall, None);
        }
        reading
    }

    /// Drives a car through a pit lane the sim never reports a stall for,
    /// which is the case the position readings exist to cover. Each tick is
    /// `(secs, lap, in_pit_lane, lap_dist_pct)`.
    fn run_lane(stint: &mut StintState, ticks: &[(f64, i32, bool, f32)]) -> StintReading {
        let mut reading = stint.update(0.0, 0, true, true, None);
        for &(secs, lap, in_pit_lane, pct) in ticks {
            reading = stint.update(secs, lap, in_pit_lane, false, Some(pct));
        }
        reading
    }

    /// Drives the player's car through a session as [`PlayerStops`] reads it.
    /// Each tick is `(secs, on_pit_road, in_box, speed_mps)`, and the sim has
    /// scored no stops of its own.
    fn run_player(stops: &mut PlayerStops, ticks: &[(f64, bool, bool, f32)]) -> i32 {
        let mut count = 0;
        for &(session_time_secs, on_pit_road, in_box, speed_mps) in ticks {
            count = stops.update(PlayerStopTick {
                session_time_secs,
                on_pit_road,
                in_box: Some(in_box),
                speed_mps: Some(speed_mps),
                in_world: true,
                official_stops: None,
            });
        }
        count
    }

    /// The player starts the session parked in their own box. Driving out of
    /// it to take the grid is not a pit stop.
    #[test]
    fn leaving_the_box_at_the_start_is_not_one_of_the_players_stops() {
        let mut stops = PlayerStops::default();
        let count = run_player(
            &mut stops,
            &[
                (0.0, true, true, 0.0),     // sitting in the box before the start
                (60.0, true, true, 0.0),    // still sitting there
                (65.0, true, false, 8.0),   // rolling down the lane
                (70.0, false, false, 40.0), // out on track
                (200.0, false, false, 60.0),
            ],
        );

        assert_eq!(count, 0);
    }

    /// A real stop: down the lane, stationary in the box, back out.
    #[test]
    fn a_stop_in_the_players_own_box_is_counted_once() {
        let mut stops = PlayerStops::default();
        let count = run_player(
            &mut stops,
            &[
                (0.0, false, false, 60.0), // racing from the first tick
                (400.0, false, false, 55.0),
                (405.0, true, false, 15.0), // entering the lane
                (410.0, true, true, 0.0),   // stationary in the box
                (435.0, true, true, 0.0),   // 25s of service
                (440.0, true, false, 12.0), // pulling out
                (445.0, false, false, 30.0),
                (600.0, false, false, 60.0),
            ],
        );

        assert_eq!(count, 1);
    }

    /// A drive-through penalty is a lane transit at the limiter with no stop
    /// in it. Counted as a stop it adds one to a race-long projection.
    #[test]
    fn a_drive_through_is_not_one_of_the_players_stops() {
        let mut stops = PlayerStops::default();
        let count = run_player(
            &mut stops,
            &[
                (0.0, false, false, 60.0),
                (400.0, true, false, 16.0), // enters the lane at the limiter
                (410.0, true, false, 16.0), // never stops, never in the box
                (420.0, true, false, 16.0),
                (430.0, false, false, 40.0), // straight back out
            ],
        );

        assert_eq!(count, 0);
    }

    /// A car queueing behind another is standing still, but in the lane and
    /// not in its box, so the visit is still a transit.
    #[test]
    fn queueing_in_the_lane_is_not_a_stop_of_its_own() {
        let mut stops = PlayerStops::default();
        let count = run_player(
            &mut stops,
            &[
                (0.0, false, false, 60.0),
                (400.0, true, false, 10.0),
                (405.0, true, false, 0.0), // held behind a car being serviced
                (425.0, true, false, 0.0),
                (430.0, false, false, 40.0),
            ],
        );

        assert_eq!(count, 0);
    }

    /// The sim stops publishing a surface for a car in the garage. Read as
    /// leaving pit road it closed the visit and opened another, turning one
    /// stop into two — which is the shape of the count being wrong on the
    /// player's own row.
    #[test]
    fn a_gap_in_the_world_does_not_split_one_stop_into_two() {
        let mut stops = PlayerStops::default();
        let tick = |secs: f64, on_pit_road: bool, in_box: bool, speed_mps: f32, in_world: bool| PlayerStopTick {
            session_time_secs: secs,
            on_pit_road,
            in_box: Some(in_box),
            speed_mps: Some(speed_mps),
            in_world,
            official_stops: None,
        };
        stops.update(tick(0.0, false, false, 60.0, true));
        stops.update(tick(400.0, false, false, 55.0, true));
        stops.update(tick(405.0, true, false, 15.0, true));
        stops.update(tick(410.0, true, true, 0.0, true));
        stops.update(tick(420.0, true, true, 0.0, true));
        // Towed to the garage mid-stop: no surface, no speed, for a while.
        stops.update(tick(425.0, false, false, 0.0, false));
        stops.update(tick(455.0, false, false, 0.0, false));
        // Back in the box, then away.
        stops.update(tick(460.0, true, true, 0.0, true));
        stops.update(tick(480.0, true, true, 0.0, true));
        let count = stops.update(tick(490.0, false, false, 30.0, true));

        assert_eq!(count, 1, "one visit to the pit lane is one stop, whatever the sim stopped publishing during it");
    }

    /// Joining a race already under way: the sim's own count stands in for
    /// the stops made before the overlay was watching, and stops moving once
    /// this starts measuring, so the same stop is never counted twice.
    #[test]
    fn the_official_count_is_a_baseline_the_first_measured_stop_takes_over_from() {
        let mut stops = PlayerStops::default();
        let tick = |secs: f64, on_pit_road: bool, in_box: bool, speed_mps: f32, official: i32| PlayerStopTick {
            session_time_secs: secs,
            on_pit_road,
            in_box: Some(in_box),
            speed_mps: Some(speed_mps),
            in_world: true,
            official_stops: Some(official),
        };
        // Joined mid-race, two stops already made and scored.
        assert_eq!(stops.update(tick(1000.0, false, false, 60.0, 2)), 2);
        stops.update(tick(1100.0, true, false, 15.0, 2));
        stops.update(tick(1110.0, true, true, 0.0, 2));
        // The sim scores the stop while the car is still in the box; the
        // baseline is already frozen, so it does not land twice.
        stops.update(tick(1135.0, true, true, 0.0, 3));
        let count = stops.update(tick(1140.0, false, false, 30.0, 3));

        assert_eq!(count, 3);
    }

    /// The bug this splits the lane from the stall for: a rival whose
    /// `CarIdxTrackSurface` never reads `InPitStall` through the whole stop.
    /// Keyed on the stall alone its stint never ended — it ran on counting
    /// through stop after stop for a whole race — and nothing on the panel
    /// said it had pitted at all. The car standing still in the lane is what
    /// gives it away.
    #[test]
    fn a_stop_the_sim_never_places_in_a_stall_is_seen_by_the_car_standing_still() {
        let mut stint = StintState::default();
        let reading = run_lane(
            &mut stint,
            &[
                (30.0, 0, false, 0.05),  // leaves the box for the grid
                (800.0, 8, false, 0.90), // eight laps of racing
                (810.0, 8, true, 0.97),  // pit entry, still rolling
                (815.0, 8, true, 0.99),  // rolls to a halt at its box
                (830.0, 8, true, 0.99),  // stationary, being serviced
                (850.0, 8, true, 0.99),
                (852.0, 8, true, 0.995), // pulls away
                (855.0, 8, false, 0.01), // back on track
                (900.0, 9, false, 0.30),
            ],
        );

        assert_eq!(reading.completed_stops, 1, "standing still in the lane is a stop, stall reading or not");
        assert_eq!(reading.avg_laps, Some(8), "the stint it ended measures the car's range");
        assert_eq!(reading.current_laps, 1, "and the new stint runs from the stop, not from the green");
        assert_eq!(
            reading.last_pit_secs,
            Some(37.0),
            "stationary from 815 to 852, not the whole 810-to-855 lane visit"
        );
    }

    /// A drive-through is a lane transit at the limiter with nothing put in
    /// the car, so the fuel stint it interrupts is still running. Counted as
    /// a stop it would report a range the car has not got, and add a stop to
    /// every projection built on the count.
    #[test]
    fn a_drive_through_penalty_is_not_a_pit_stop() {
        let mut stint = StintState::default();
        let reading = run_lane(
            &mut stint,
            &[
                (30.0, 0, false, 0.05),  // leaves the box for the grid
                (800.0, 8, false, 0.90), // racing
                (810.0, 8, true, 0.97),  // into the lane, never stopping
                (815.0, 8, true, 0.98),
                (820.0, 8, true, 0.99),
                (825.0, 8, true, 0.995),
                (830.0, 8, false, 0.01), // straight back out
                (1000.0, 10, false, 0.5),
            ],
        );

        assert_eq!(reading.completed_stops, 0, "nothing was put in the car");
        assert_eq!(reading.avg_laps, None, "no stint ended, so nothing was learnt about its range");
        assert_eq!(reading.current_laps, 10, "the stint carries on straight through the penalty");
    }

    /// The safe default, and the reason the drive-through test cannot bring
    /// the original fault back: with no position readings to judge the visit
    /// by, it is taken as the stop a pit-lane visit nearly always is. Getting
    /// this the other way round would leave the stint running for the rest of
    /// the race — which is exactly what was being fixed.
    #[test]
    fn a_visit_the_sim_gave_no_position_for_is_taken_as_a_stop() {
        let mut stint = StintState::default();
        // Out of the box for the grid, eight laps of racing, then a lane visit
        // with neither a stall reading nor a position to watch it with.
        let mut reading = stint.update(0.0, 0, true, true, None);
        for &(secs, lap, lane) in &[
            (30.0_f64, 0_i32, false),
            (800.0, 8, false),
            (810.0, 8, true),
            (830.0, 8, true),
            (850.0, 8, true),
            (855.0, 8, false),
            (900.0, 9, false),
        ] {
            reading = stint.update(secs, lap, lane, false, None);
        }

        assert_eq!(reading.completed_stops, 1, "unwatched, so trusted as a stop rather than ruled a drive-through");
        assert_eq!(reading.current_laps, 1, "and the stint restarted, which is the whole point");
        assert_eq!(reading.avg_laps, Some(8));
        assert_eq!(reading.last_pit_secs, None, "an inferred stop has no measured service duration");
        assert_eq!(reading.avg_pit_secs, None, "missing service must not become a zero-second stop");
    }

    /// Per-car position can disappear only while the rival is at its box.
    /// The known lane entry and exit still make this one continuous visit;
    /// dropping it would leave the old stint running indefinitely.
    #[test]
    fn a_continuous_lane_visit_with_position_missing_during_service_still_resets_once() {
        let mut stint = StintState::default();
        stint.update_with_presence(0.0, 0, true, true, true, None);
        stint.update_with_presence(30.0, 0, true, false, false, Some(0.05));
        stint.update_with_presence(800.0, 8, true, false, false, Some(0.90));
        stint.update_with_presence(810.0, 8, true, true, false, Some(0.97));
        stint.update_with_presence(815.0, 8, true, true, false, None);
        stint.update_with_presence(835.0, 8, true, true, false, None);
        let reading = stint.update_with_presence(845.0, 8, true, false, false, Some(0.01));

        assert_eq!(reading.completed_stops, 1);
        assert_eq!(reading.current_laps, 0);
        assert_eq!(reading.avg_laps, Some(8), "known boundaries still provide an exit-to-exit range");
        assert_eq!(reading.last_pit_secs, None, "blind time cannot become a measured service duration");
    }

    /// `NotInWorld` is absence, not an off-pit-road tick. Closing on it used
    /// to split one driver-swap/repair visit and count it twice.
    #[test]
    fn a_not_in_world_gap_inside_an_observed_stop_does_not_split_or_time_the_gap() {
        let mut stint = StintState::default();
        stint.update_with_presence(0.0, 0, true, true, true, None);
        stint.update_with_presence(30.0, 0, true, false, false, Some(0.05));
        stint.update_with_presence(800.0, 8, true, false, false, Some(0.90));
        stint.update_with_presence(810.0, 8, true, true, false, Some(0.97));
        stint.update_with_presence(815.0, 8, true, true, true, Some(0.99));
        stint.update_with_presence(825.0, 8, true, true, true, Some(0.99));
        stint.update_with_presence(830.0, 8, false, false, false, None);
        stint.update_with_presence(850.0, 8, true, true, true, Some(0.99));
        let reading = stint.update_with_presence(860.0, 8, true, false, false, Some(0.01));

        assert_eq!(reading.completed_stops, 1, "the one entered visit settles once");
        assert_eq!(reading.current_laps, 0);
        assert_eq!(reading.avg_laps, None, "an interrupted visit cannot teach a fuel range");
        assert_eq!(reading.last_pit_secs, None, "partial observed time is not the full service time");
    }

    /// A tow has no witnessed pit entry. Reappearing in the box and later
    /// leaving it must not make the car look freshly out of a racing stop.
    #[test]
    fn a_tow_from_track_to_garage_does_not_reset_or_poison_the_next_range() {
        let mut stint = StintState::default();
        stint.update_with_presence(0.0, 0, true, false, false, Some(0.10));
        stint.update_with_presence(800.0, 8, true, false, false, Some(0.80));
        stint.update_with_presence(805.0, 8, false, false, false, None);
        stint.update_with_presence(900.0, 8, true, true, true, Some(0.99));
        stint.update_with_presence(930.0, 8, true, true, true, Some(0.99));
        let after_tow = stint.update_with_presence(940.0, 8, true, false, false, Some(0.01));

        assert_eq!(after_tow.completed_stops, 0);
        assert_eq!(after_tow.current_laps, 8, "garage departure is not a racing pit-exit reset");

        stint.update_with_presence(1700.0, 17, true, true, true, Some(0.99));
        let after_real_stop = stint.update_with_presence(1730.0, 17, true, false, false, Some(0.01));
        assert_eq!(after_real_stop.completed_stops, 1, "the later observed stop still counts");
        assert_eq!(after_real_stop.avg_laps, None, "the span across the tow is not a fuel stint");
    }

    /// Attaching the overlay midway through somebody's stint gives only its
    /// tail. It can count the first observed stop, but not call that tail the
    /// car's typical fuel range.
    #[test]
    fn a_first_partial_mid_race_stint_is_excluded_from_range_history() {
        let mut stint = StintState::default();
        stint.update_with_presence(1000.0, 10, true, false, false, Some(0.50));
        stint.update_with_presence(1600.0, 17, true, true, true, Some(0.99));
        let reading = stint.update_with_presence(1630.0, 17, true, false, false, Some(0.01));

        assert_eq!(reading.completed_stops, 1);
        assert_eq!(reading.avg_laps, None, "seven observed laps are not necessarily a seven-lap tank");
        assert_eq!(reading.current_laps, 0);
    }

    #[test]
    fn a_late_attach_baseline_plus_a_local_stop_is_shown_before_the_scorer_catches_up() {
        let mut stint = StintState::default();
        stint.update_with_official(1000.0, 10, Some(2), true, false, false, Some(0.5));
        stint.update_with_official(1600.0, 17, Some(2), true, true, true, Some(0.99));
        let exit = stint.update_with_official(1630.0, 17, Some(2), true, false, false, Some(0.01));

        assert_eq!(exit.completed_stops, 3, "two historical stops plus the one just observed");
        assert_eq!(exit.current_laps, 0);

        let scored = stint.update_with_official(1700.0, 18, Some(3), true, false, false, Some(0.5));
        assert_eq!(scored.completed_stops, 3, "the delayed scorer update acknowledges rather than duplicates it");
        assert_eq!(scored.current_laps, 1, "the delayed count must not reset the stint twice");
    }

    #[test]
    fn a_stop_scored_while_in_the_stall_still_resets_once_at_exit() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, Some(0), true, false, false, Some(0.1));
        stint.update_with_official(800.0, 8, Some(0), true, true, false, Some(0.97));
        stint.update_with_official(810.0, 8, Some(1), true, true, true, Some(0.99));
        stint.update_with_official(830.0, 8, Some(1), true, true, true, Some(0.99));
        let exit = stint.update_with_official(840.0, 8, Some(1), true, false, false, Some(0.01));

        assert_eq!(exit.completed_stops, 1);
        assert_eq!(exit.current_laps, 0);
        assert_eq!(exit.avg_laps, Some(8), "direct evidence still teaches the observed range");
    }

    #[test]
    fn a_delayed_official_count_confirms_an_interrupted_exit_without_teaching_history() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, Some(0), true, false, false, Some(0.1));
        stint.update_with_official(800.0, 8, Some(0), true, true, false, Some(0.97));
        stint.update_with_official(810.0, 8, Some(0), false, false, false, None);
        stint.update_with_official(830.0, 8, Some(0), true, true, false, Some(0.99));
        let uncertain = stint.update_with_official(840.0, 8, Some(0), true, false, false, Some(0.01));
        assert_eq!(uncertain.completed_stops, 0);
        assert_eq!(uncertain.current_laps, 8, "the interrupted visit is not guessed before confirmation");

        let confirmed = stint.update_with_official(900.0, 9, Some(1), true, false, false, Some(0.5));
        assert_eq!(confirmed.completed_stops, 1);
        assert_eq!(confirmed.current_laps, 1, "the known exit boundary is applied when the scorer confirms it");
        assert_eq!(confirmed.avg_laps, None);
        assert_eq!(confirmed.last_pit_secs, None);
    }

    #[test]
    fn an_offline_official_stop_resets_current_state_but_cannot_teach_the_next_range() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, Some(0), true, false, false, Some(0.1));
        stint.update_with_official(800.0, 8, Some(1), false, false, false, None);
        let returned = stint.update_with_official(900.0, 9, Some(1), true, false, false, Some(0.5));
        assert_eq!(returned.completed_stops, 1);
        assert_eq!(returned.current_laps, 0);

        stint.update_with_official(1600.0, 17, Some(1), true, true, true, Some(0.99));
        let next_exit = stint.update_with_official(1630.0, 17, Some(1), true, false, false, Some(0.01));
        assert_eq!(next_exit.completed_stops, 2);
        assert_eq!(next_exit.avg_laps, None, "an approximate post-offline start cannot become a fuel-range sample");
    }

    #[test]
    fn a_first_official_count_below_the_local_count_does_not_erase_the_stop() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, None, true, false, false, Some(0.1));
        stint.update_with_official(700.0, 8, None, true, true, true, Some(0.99));
        let local = stint.update_with_official(730.0, 8, None, true, false, false, Some(0.01));
        assert_eq!(local.completed_stops, 1);

        let stale = stint.update_with_official(800.0, 9, Some(0), true, false, false, Some(0.5));
        assert_eq!(stale.completed_stops, 1, "a first stale scorer row is a baseline, not a rollback");
        let caught_up = stint.update_with_official(900.0, 10, Some(1), true, false, false, Some(0.7));
        assert_eq!(caught_up.completed_stops, 1);
        assert_eq!(caught_up.current_laps, 2, "catch-up must not reset the already observed boundary");
    }

    #[test]
    fn an_observed_drive_through_never_resets_the_fuel_stint_when_officially_counted() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, Some(0), true, false, false, Some(0.1));
        stint.update_with_official(700.0, 8, Some(0), true, true, false, Some(0.97));
        stint.update_with_official(710.0, 8, Some(0), true, true, false, Some(0.98));
        stint.update_with_official(720.0, 8, Some(0), true, true, false, Some(0.99));
        stint.update_with_official(730.0, 8, Some(0), true, false, false, Some(0.01));
        let scored = stint.update_with_official(800.0, 9, Some(1), true, false, false, Some(0.5));

        assert_eq!(scored.completed_stops, 1, "the official convention still controls the displayed count");
        assert_eq!(scored.current_laps, 9, "a known nonstop transit does not start a new fuel stint");
        assert_eq!(scored.avg_laps, None);
    }

    #[test]
    fn an_old_drive_through_cannot_claim_the_score_for_a_later_real_stop() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, Some(0), true, false, false, Some(0.1));
        stint.update_with_official(700.0, 8, Some(0), true, true, false, Some(0.97));
        stint.update_with_official(710.0, 8, Some(0), true, true, false, Some(0.98));
        stint.update_with_official(720.0, 8, Some(0), true, false, false, Some(0.01));

        // The next pit entry expires that earlier drive-through credit. This
        // scorer increase belongs to the real stop now in progress.
        stint.update_with_official(1500.0, 17, Some(1), true, true, true, Some(0.99));
        stint.update_with_official(1525.0, 17, Some(1), true, true, true, Some(0.99));
        let exit = stint.update_with_official(1530.0, 17, Some(1), true, false, false, Some(0.01));

        assert_eq!(exit.completed_stops, 1, "one scored real stop must not become drive-through offset plus local stop");
        assert_eq!(exit.current_laps, 0);
        assert_eq!(exit.avg_laps, Some(17));
    }

    #[test]
    fn an_official_correction_while_continuously_on_track_preserves_the_stint_boundary() {
        let mut stint = StintState::default();
        stint.update_with_official(0.0, 0, Some(0), true, false, false, Some(0.1));
        stint.update_with_official(700.0, 8, Some(0), true, true, true, Some(0.99));
        stint.update_with_official(730.0, 8, Some(0), true, false, false, Some(0.01));

        let corrected = stint.update_with_official(800.0, 9, Some(2), true, false, false, Some(0.5));
        assert_eq!(corrected.completed_stops, 2, "the scorer still controls the completed total");
        assert_eq!(corrected.current_laps, 1, "an on-track correction is not a fresh pit exit");
        assert_eq!(corrected.avg_laps, Some(8), "existing observed range history is retained");
    }

    #[test]
    fn a_blind_stop_preserves_the_previous_service_estimate() {
        let mut stint = StintState::default();
        run_stint(&mut stint, &[(30.0, 0, false), (700.0, 8, true), (725.0, 8, false)]);
        stint.update(1400.0, 16, true, false, None);
        stint.update(1430.0, 16, true, false, None);
        let reading = stint.update(1440.0, 16, false, false, None);

        assert_eq!(reading.completed_stops, 2);
        assert_eq!(reading.last_pit_secs, None);
        assert_eq!(reading.avg_pit_secs, Some(25.0), "an unmeasured stop cannot undercut a measured one");
    }

    #[test]
    fn pit_lane_line_crossings_do_not_shorten_every_completed_stint() {
        let mut stint = StintState::default();
        let reading = run_stint(
            &mut stint,
            &[
                (30.0, 0, false),
                (1500.0, 16, true),
                (1530.0, 17, false),
                (3000.0, 33, true),
                (3030.0, 34, false),
                (4500.0, 50, true),
                (4530.0, 51, false),
            ],
        );

        assert_eq!(reading.completed_stops, 3);
        assert_eq!(reading.avg_laps, Some(17), "17 laps between exits must remain a 17-lap range");
        assert_eq!(reading.current_laps, 0, "the current stint uses the same exit boundary");
    }

    /// A car held for a moment behind another on its way through has still
    /// not stopped — see [`MIN_STOP_SECS`].
    #[test]
    fn a_brief_hold_on_the_way_through_is_not_a_stop() {
        let mut stint = StintState::default();
        let reading = run_lane(
            &mut stint,
            &[
                (30.0, 0, false, 0.05),
                (800.0, 8, false, 0.90),
                (810.0, 8, true, 0.9700),
                (810.5, 8, true, 0.9740), // rolling down the lane
                (811.0, 8, true, 0.9780),
                (811.5, 8, true, 0.9780), // held behind a car leaving its box
                (812.0, 8, true, 0.9780),
                (813.0, 8, true, 0.9780),
                (813.5, 8, true, 0.9820), // away again, 2.5s after it stopped
                (820.0, 8, true, 0.9950),
                (825.0, 8, false, 0.0100),
                (1000.0, 10, false, 0.5000),
            ],
        );

        assert_eq!(reading.completed_stops, 0, "a couple of seconds is a queue, not a service");
        assert_eq!(reading.current_laps, 10, "so the stint runs on");
    }

    /// Where the sim does publish the stall, it is believed on the spot: the
    /// lane transit either side of the box is not part of the stop.
    #[test]
    fn the_lane_transit_is_not_counted_as_time_stopped() {
        let mut stint = StintState::default();
        let mut reading = stint.update(0.0, 0, true, true, None);
        for &(secs, lap, lane, stall) in &[
            (30.0_f64, 0_i32, false, false), // out for the grid
            (400.0, 6, false, false),        // racing
            (410.0, 6, true, false),         // pit entry, still rolling
            (420.0, 6, true, true),          // settled in the box
            (450.0, 6, true, true),          // being serviced
            (455.0, 6, true, false),         // pulled away, still in the lane
            (465.0, 6, false, false),        // back on track
        ] {
            reading = stint.update(secs, lap, lane, stall, None);
        }

        assert_eq!(reading.completed_stops, 1);
        assert_eq!(reading.last_pit_secs, Some(35.0), "420 to 455 in the box, not 410 to 465 in the lane");
    }

    /// Every car begins a session parked in its own box. Driving out of it to
    /// take the grid is not a pit stop, and counting it as one gave a sprint
    /// race with no stops in it a stop for every car on the timing screen.
    #[test]
    fn leaving_the_box_at_the_start_of_a_session_is_not_a_pit_stop() {
        let mut stint = StintState::default();
        let reading = run_stint(&mut stint, &[(30.0, 0, false), (110.0, 1, false), (190.0, 2, false)]);

        assert_eq!(reading.completed_stops, 0);
        assert_eq!(reading.last_pit_secs, None);
        assert_eq!(reading.avg_pit_secs, None, "no stop has happened, so there is no stop time to average");
        assert_eq!(reading.current_laps, 2);
    }

    /// A real stop — out of the box, racing, back in, back out — is counted,
    /// and its stationary time measured.
    #[test]
    fn a_stop_taken_mid_race_is_counted_and_timed() {
        let mut stint = StintState::default();
        let reading = run_stint(
            &mut stint,
            &[
                (30.0, 0, false),  // leaves the box for the grid
                (400.0, 5, false), // racing
                (410.0, 5, true),  // stationary in the box
                (435.0, 5, false), // rejoins after 25s
                (600.0, 7, false),
            ],
        );

        assert_eq!(reading.completed_stops, 1);
        assert_eq!(reading.last_pit_secs, Some(25.0));
        assert_eq!(reading.avg_pit_secs, Some(25.0));
        assert_eq!(reading.current_laps, 2, "the new stint starts at the lap the car rejoined on");
    }

    /// A car that boxes three laps after its last stop stopped for damage or
    /// a penalty, not fuel. Projected literally, it pits every three laps for
    /// the rest of the race — the outlier deciding the strategy column.
    #[test]
    fn a_stop_a_few_laps_in_sets_no_stint_length() {
        let mut stint = StintState::default();
        let reading = run_stint(
            &mut stint,
            &[
                (30.0, 0, false),  // out for the grid
                (300.0, 3, true),  // boxes three laps in
                (330.0, 3, false), // rejoins
            ],
        );

        assert_eq!(reading.completed_stops, 1, "the stop itself is real");
        assert_eq!(reading.avg_laps, None, "but it says nothing about the car's range");
    }

    /// One cut-short stint — a flat tyre nine laps in — must not drag the
    /// typical figure down. A mean read 17, 17, 9 as 14 and projected a stop
    /// three laps before the car would actually take one.
    #[test]
    fn one_short_stint_does_not_drag_the_typical_stint_down() {
        let mut stint = StintState::default();
        let reading = run_stint(
            &mut stint,
            &[
                (30.0, 0, false),
                (1500.0, 17, true), // 17-lap stint
                (1530.0, 17, false),
                (3000.0, 34, true), // 17 again
                (3030.0, 34, false),
                (3800.0, 43, true), // cut short at 9
                (3830.0, 43, false),
            ],
        );

        assert_eq!(reading.avg_laps, Some(17));
    }

    /// The other direction has to survive: a car genuinely stopping a lap
    /// earlier than before is tightening, and the projection should say 16,
    /// not smooth it back up to 17.
    #[test]
    fn a_car_stopping_a_lap_earlier_reads_a_lap_earlier() {
        let mut stint = StintState::default();
        let reading = run_stint(
            &mut stint,
            &[
                (30.0, 0, false),
                (1500.0, 17, true), // 17-lap stint
                (1530.0, 17, false),
                (2950.0, 33, true), // then a lap early: 16
                (2980.0, 33, false),
            ],
        );

        assert_eq!(reading.avg_laps, Some(16));
    }

    /// A stop-and-hold served in the box is a fact about a penalty, not about
    /// what this car's service takes; a mean carried a third of it into every
    /// projected stop for the rest of the race.
    #[test]
    fn a_penalty_served_in_the_box_does_not_inflate_the_typical_stop() {
        let mut stint = StintState::default();
        let reading = run_stint(
            &mut stint,
            &[
                (30.0, 0, false),
                (700.0, 8, true), // a normal 25 s stop
                (725.0, 8, false),
                (1400.0, 16, true), // another
                (1425.0, 16, false),
                (2100.0, 24, true), // 95 s: a penalty plus service
                (2195.0, 24, false),
            ],
        );

        assert_eq!(reading.last_pit_secs, Some(95.0), "the last stop is reported as it was");
        assert_eq!(reading.avg_pit_secs, Some(25.0), "the typical stop is not");
    }

    /// A seven-lap run followed by one service: the point at which practice
    /// used to acquire a fictitious nine-stop race strategy for the field.
    fn field_after_seven_lap_stop() -> [StandingsEntry; 2] {
        let mut stint = StintState::default();
        let reading = run_stint(&mut stint, &[(0.0, 0, false), (700.0, 7, true), (725.0, 7, true), (730.0, 7, false)]);
        assert_eq!(reading.completed_stops, 1);
        assert_eq!(reading.avg_laps, Some(7));
        let mut pitted = test_entry(10, 0, 100.0);
        pitted.is_focus = true;
        pitted.pit_stops = reading.completed_stops;
        pitted.current_stint_laps = reading.current_laps;
        pitted.avg_stint_laps = reading.avg_laps;
        let mut rival = test_entry(10, 0, 100.0);
        rival.car_idx = 1;
        rival.class_position = 2;
        rival.current_stint_laps = 7;
        [pitted, rival]
    }

    #[test]
    fn a_practice_pit_exit_never_creates_race_stop_projections() {
        for kind in [SessionKind::Practice, SessionKind::Qualifying, SessionKind::Warmup, SessionKind::Unknown] {
            let mut standings = field_after_seven_lap_stop();
            let clock = endurance::laps_remaining(Some(7000.0), 100.0, Some(0.0));
            let mut multi_stop = false;
            let meta = annotate_endurance(
                &mut standings,
                race_laps_remaining(kind, None, clock),
                30.0,
                Some(10),
                &mut multi_stop,
                pit_model::PitModel::default(),
                &HashMap::from([(0, 0.0), (1, 10.0)]),
            );
            assert!(standings.iter().all(|entry| entry.stops_remaining.is_none()), "{kind:?}: no stops to a race finish");
            assert!(standings.iter().all(|entry| entry.projected_class_position.is_none()));
            assert!(meta.laps_remaining.is_none());
            assert!(!multi_stop);
            assert_eq!(standings[0].pit_stops, 1, "the completed stop is still counted");
            assert_eq!(standings[0].avg_stint_laps, Some(7), "practice still measures stints");
        }
    }

    #[test]
    fn a_race_still_projects_stops_from_the_same_seven_lap_stint() {
        let mut standings = field_after_seven_lap_stop();
        let clock = endurance::laps_remaining(Some(7000.0), 100.0, Some(0.0));
        annotate_endurance(
            &mut standings,
            race_laps_remaining(SessionKind::Race, None, clock),
            30.0,
            Some(10),
            &mut false,
            pit_model::PitModel::default(),
            &HashMap::from([(0, 0.0), (1, 10.0)]),
        );
        assert_eq!(standings[0].stops_remaining, Some(9));
        assert_eq!(standings[1].stops_remaining, Some(10));
        assert_eq!(standings[0].pit_stops, 1);
        assert_eq!(standings[1].pit_stops, 0);
    }

    #[test]
    fn race_laps_respect_lap_limits_and_the_earlier_of_both_limits() {
        for (count, clock, expected) in [
            (Some(20), None, Some(20)),
            (Some(12), Some(20), Some(12)),
            (Some(20), Some(12), Some(12)),
            (Some(0), Some(1), Some(0)),
            (None, None, None),
        ] {
            assert_eq!(race_laps_remaining(SessionKind::Race, count, clock), expected);
        }
    }

    /// A car that hasn't pitted yet has no history, but projecting it with
    /// zero remaining stops ranks it as if it will never pit — the one answer
    /// known to be wrong. It borrows the class's typical stint instead.
    #[test]
    fn a_car_that_has_not_pitted_is_not_projected_to_never_pit() {
        let mut unpitted = test_entry(10, 0, 100.0);
        unpitted.class_position = 1;
        unpitted.gap_to_leader_secs = 0.0;
        unpitted.current_stint_laps = 18; // deep into a long first stint

        let mut pitted = test_entry(10, 0, 100.0);
        pitted.car_idx = 1;
        pitted.class_position = 2;
        pitted.gap_to_leader_secs = 10.0;
        pitted.current_stint_laps = 1; // fresh out of the box
        pitted.avg_stint_laps = Some(17);

        let mut standings = [unpitted, pitted];
        let mut multi_stop = false;
        annotate_endurance(
            &mut standings,
            Some(20),
            30.0,
            Some(10),
            &mut multi_stop,
            pit_model::PitModel::default(),
            &HashMap::from([(0, 0.0), (1, 10.0)]),
        );

        // The unpitted leader owes two stops (nothing left of a typical
        // stint, then 20 laps over 17-lap tanks); the rival owes one. 60 s of
        // stops against 10 + 30 puts the rival ahead.
        assert_eq!(standings[1].projected_class_position, Some(1));
        assert_eq!(standings[0].projected_class_position, Some(2));
    }

    #[test]
    fn net_uses_total_stop_loss_and_live_gap_after_pit_exit() {
        let mut leader = test_entry(10, 0, 100.0);
        leader.avg_stint_laps = Some(17);
        leader.current_stint_laps = 17;
        leader.avg_pit_secs = Some(20.0);
        let mut pitted = leader.clone();
        pitted.car_idx = 1;
        pitted.class_position = 2;
        pitted.current_stint_laps = 0;
        // F2 still has the old gap. Live track position says forty seconds.
        pitted.gap_to_leader_secs = 0.0;
        let mut standings = [leader, pitted];
        let curve = relative::LapCurve::default();
        let gaps = live_net_gaps(&standings, &[20, 20], &[0.7, 0.3], &curve, Some(10));
        let model = pit_model::PitModel { transit_loss_secs: 30.0, transit_runs: 1, ..Default::default() };
        annotate_endurance(&mut standings, Some(10), 30.0, Some(10), &mut false, model, &gaps);
        assert_eq!(standings[0].stops_remaining, Some(1));
        assert_eq!(standings[1].stops_remaining, Some(0));
        assert_eq!(standings[1].projected_class_position, Some(1), "40 seconds paid beats 50 seconds owed");
        // At sixty seconds behind, the same car really has lost NET P1.
        let gaps = live_net_gaps(&standings, &[20, 20], &[0.9, 0.3], &curve, Some(10));
        annotate_endurance(&mut standings, Some(10), 30.0, Some(10), &mut false, model, &gaps);
        assert_eq!(standings[1].projected_class_position, Some(2), "stale F2 must not keep it ahead");
    }

    #[test]
    fn net_waits_for_a_complete_pit_exit_reading_then_recovers() {
        let mut leader = test_entry(10, 0, 100.0);
        leader.avg_stint_laps = Some(17);
        let mut pitted = leader.clone();
        pitted.car_idx = 1;
        pitted.class_position = 2;
        pitted.track_location = TrackLocation::ApproachingPits;
        let mut standings = [leader, pitted];
        let model = pit_model::PitModel::default();
        let gaps = HashMap::from([(0, 0.0), (1, 20.0)]);
        annotate_endurance(&mut standings, Some(10), 30.0, Some(10), &mut false, model, &gaps);
        assert!(standings.iter().all(|entry| entry.projected_class_position.is_none()));
        standings[1].track_location = TrackLocation::OnTrack;
        annotate_endurance(&mut standings, Some(10), 30.0, Some(10), &mut false, model, &HashMap::from([(0, 0.0)]));
        assert!(standings.iter().all(|entry| entry.projected_class_position.is_none()));
        annotate_endurance(&mut standings, Some(10), 30.0, Some(10), &mut false, model, &gaps);
        assert_eq!(standings[1].projected_class_position, Some(2));
        annotate_endurance(&mut standings, None, 30.0, Some(10), &mut false, model, &gaps);
        assert!(
            standings.iter().all(|entry| entry.projected_class_position.is_none()),
            "no stale rank without a horizon"
        );
    }

    #[test]
    fn net_does_not_add_service_to_an_unmeasured_total_loss() {
        let mut leader = test_entry(10, 0, 100.0);
        leader.avg_stint_laps = Some(17);
        leader.current_stint_laps = 17;
        leader.avg_pit_secs = Some(20.0);
        let mut rival = leader.clone();
        rival.car_idx = 1;
        rival.class_position = 2;
        rival.current_stint_laps = 0;
        let mut standings = [leader, rival];
        annotate_endurance(
            &mut standings,
            Some(10),
            30.0,
            Some(10),
            &mut false,
            pit_model::PitModel::default(),
            &HashMap::from([(0, 0.0), (1, 40.0)]),
        );
        assert_eq!(standings[0].projected_class_position, Some(1), "configured 30 is total, not 30 plus service");
    }

    #[test]
    fn net_ranks_active_cars_when_a_competitor_leaves_the_world() {
        let mut absent = test_entry(10, 0, 100.0);
        absent.avg_stint_laps = Some(17);
        absent.track_location = TrackLocation::NotInWorld;
        let mut active = absent.clone();
        active.car_idx = 1;
        active.class_position = 2;
        active.track_location = TrackLocation::OnTrack;
        let mut standings = [absent, active];
        let gaps = live_net_gaps(&standings, &[-1, 20], &[-1.0, 0.3], &relative::LapCurve::default(), Some(10));
        annotate_endurance(&mut standings, Some(10), 30.0, Some(10), &mut false, pit_model::PitModel::default(), &gaps);
        assert_eq!(standings[0].projected_class_position, None);
        assert_eq!(standings[1].projected_class_position, Some(1));
    }

    #[test]
    fn net_gap_origin_does_not_depend_on_held_standings_order() {
        let leader = test_entry(10, 0, 100.0);
        let mut passing = leader.clone();
        passing.car_idx = 1;
        passing.class_position = 2;
        let gaps =
            live_net_gaps(&[leader, passing], &[20, 20], &[0.5, 0.501], &relative::LapCurve::default(), Some(10));
        assert_eq!(gaps.len(), 2);
        assert!((gaps[&0] - 0.1).abs() < 0.001);
        assert!(gaps[&1].abs() < f32::EPSILON);
    }

    #[test]
    fn net_live_gaps_keep_lap_deficits_and_class_leaders_separate() {
        let mut fast = test_entry(1, 0, 80.0);
        fast.car_idx = 0;
        let mut slow = test_entry(2, 0, 100.0);
        slow.car_idx = 1;
        let mut lapped = slow.clone();
        lapped.car_idx = 2;
        lapped.class_position = 2;
        let gaps = live_net_gaps(
            &[fast, slow, lapped],
            &[30, 25, 23],
            &[0.8, 0.2, 0.9],
            &relative::LapCurve::default(),
            Some(2),
        );
        assert!(gaps[&0].abs() < f32::EPSILON);
        assert!(gaps[&1].abs() < f32::EPSILON);
        assert!((gaps[&2] - 130.0).abs() < 0.001);
    }

    /// The trackers live for one iRacing connection, which covers practice,
    /// then qualifying, then the race. Every figure in them is a fact about
    /// one of those, and a race that inherits practice's pit lane reports
    /// stops that never happened.
    #[test]
    fn a_new_session_starts_from_a_clean_history() {
        let mut trackers = SessionTrackers::default();
        trackers.sync_to_session(Some(1));
        trackers.stint.update(0, 0.0, 0, None, true, true, true, None);
        trackers.stint.update(0, 30.0, 0, None, true, false, false, None);
        trackers.stint.update(0, 300.0, 3, None, true, true, true, None);
        trackers.stint.update(0, 340.0, 3, None, true, false, false, None);
        trackers.multi_stop_race = true;
        assert_eq!(
            trackers.stint.update(0, 400.0, 4, None, true, false, false, None).completed_stops,
            1,
            "the practice stop is real"
        );

        trackers.sync_to_session(Some(2));
        assert_eq!(trackers.stint.update(0, 0.0, 0, None, true, false, false, None).completed_stops, 0);
        assert!(!trackers.multi_stop_race);
        assert_eq!(trackers.player_pace.update(1, 80.0, false), Some(80.0), "pace is measured afresh too");
    }

    #[test]
    fn a_temporarily_missing_session_number_preserves_the_current_session() {
        let mut trackers = SessionTrackers::default();
        trackers.sync_to_session(Some(2));
        trackers.stint.update(0, 0.0, 0, None, true, false, false, Some(0.1));
        trackers.stint.update(0, 700.0, 8, None, true, true, true, Some(0.99));
        trackers.stint.update(0, 730.0, 8, None, true, false, false, Some(0.01));

        trackers.sync_to_session(None);
        trackers.sync_to_session(Some(2));

        assert_eq!(
            trackers.stint.update(0, 800.0, 9, None, true, false, false, Some(0.5)).completed_stops,
            1,
            "an unavailable SessionNum sample is not a session change"
        );
    }

    /// The projection divides the session clock by this, so a single slow lap
    /// left standing as "the last lap" quietly rewrites how long the race is.
    #[test]
    fn pace_ignores_laps_that_are_not_representative() {
        let mut pace = LapPace::default();
        assert_eq!(pace.update(1, 0.0, false), None, "no lap completed yet");
        // A standing-start opening lap, then honest green-flag pace.
        assert_eq!(pace.update(2, 95.0, false), Some(95.0), "with one lap there is nothing to compare it against");
        assert_eq!(pace.update(3, 79.0, false), Some(79.0), "the opening lap is now visibly an outlier");
        assert_eq!(pace.update(4, 81.0, false), Some(80.0), "the two green laps average");
    }

    /// The one the fuel load turns on. A stop puts twenty seconds of standing
    /// still inside a lap time, and the lap after a stop is exactly when the
    /// next load is worked out — so if that lap reaches the window, the pace
    /// the whole race is projected from is a stop.
    #[test]
    fn a_lap_spent_in_the_pit_lane_never_reaches_the_pace_window() {
        let mut pace = LapPace::default();
        pace.update(1, 0.0, false);
        pace.update(2, 97.0, false);
        assert_eq!(pace.update(3, 98.0, false), Some(97.5), "two green laps");

        // Lap 3 is the in-lap: the car reaches pit road part-way round it.
        pace.update(3, 98.0, true);
        // It completes as lap 4, timed at a green lap plus the whole stop, and
        // lap 4 begins with the car still on pit road, so it is tainted too.
        assert_eq!(pace.update(4, 250.0, true), Some(97.5), "the stop must not become the pace");
        // Lap 4 completes as a slow out-lap; also discarded.
        assert_eq!(pace.update(5, 130.0, false), Some(97.5), "nor the out-lap behind it");
        // And a clean lap afterwards is taken normally.
        assert_eq!(pace.update(6, 97.0, false), Some((97.0 + 98.0 + 97.0) / 3.0), "back to green laps");
    }

    /// Averaging, not taking the best: the question is how long a lap takes,
    /// not how quick the car can be.
    #[test]
    fn pace_averages_the_laps_it_keeps() {
        let mut pace = LapPace::default();
        for (lap, secs) in [(1, 80.0), (2, 82.0), (3, 81.0)] {
            pace.update(lap, secs, false);
        }
        assert_eq!(pace.update(4, 81.0, false), Some(81.0));
    }

    /// A lap only counts once, however many ticks are read while the car sits
    /// on it.
    #[test]
    fn pace_records_each_lap_once() {
        let mut pace = LapPace::default();
        pace.update(1, 80.0, false);
        for _ in 0..5 {
            pace.update(1, 80.0, false);
        }
        assert_eq!(pace.update(2, 90.0, false), Some(80.0), "80 and 90 in the window, 90 outside the cutoff");
        assert_eq!(pace.laps.len(), 2);
    }

    /// The window is short enough to follow a real change of pace rather than
    /// averaging the whole session.
    #[test]
    fn pace_forgets_laps_beyond_the_window() {
        let mut pace = LapPace::default();
        for lap in 1..=(PACE_WINDOW + 3) {
            pace.update(i32::try_from(lap).expect("small"), 80.0, false);
        }
        assert_eq!(pace.laps.len(), PACE_WINDOW);
    }
}
