// Rust guideline compliant 2026-09-13

//! Read-only evidence capture for investigating scoring while a car is absent.
//!
//! This deliberately talks only to the SDK's read surface.  It writes no
//! iRacing commands, names, roster details, or account identifiers: the CSV is
//! a numeric trace keyed only by `CarIdx`.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use iracing_telem::{Client, DataUpdateResult, Session, Var};

use super::session_info::{ResultsPosition, SessionInfoYaml};

const CONNECT_WAIT: Duration = Duration::from_secs(60);
const CONNECT_STEP: Duration = Duration::from_millis(500);
const DATA_WAIT: Duration = Duration::from_millis(500);
const SAMPLE_PERIOD: Duration = Duration::from_secs(1);
const FLUSH_PERIOD: Duration = Duration::from_secs(5);
const MAX_TICK_GAP: Duration = Duration::from_secs(2);
const LONG_ABSENCE_SECS: f64 = 30.0;

/// Files and counts produced by [`capture`].
#[derive(Debug)]
pub struct CaptureReport {
    pub data_path: PathBuf,
    pub summary_path: PathBuf,
    pub rows: usize,
}

#[derive(Debug, Clone, Copy)]
enum CaptureEnd {
    Duration,
    SessionExpired,
    Error,
}

impl CaptureEnd {
    const fn label(self) -> &'static str {
        match self {
            Self::Duration => "duration",
            Self::SessionExpired => "session_expired",
            Self::Error => "error",
        }
    }
}

/// Captures numeric scorer inputs for `duration`, without changing iRacing.
///
/// # Errors
/// Returns an error when either output already exists, iRacing is unavailable,
/// or either output cannot be written.
pub fn capture(path: &Path, duration: Duration) -> anyhow::Result<CaptureReport> {
    let summary_path = path.with_extension("summary.txt");
    let (data_file, summary_file) = create_outputs(path, &summary_path)?;
    let mut writer = BufWriter::new(data_file);
    writeln!(
        writer,
        "elapsed,SessionTime,SessionNum,SDKSessionInfoUpdate,CarIdx,TrackSurface,DirectOnPitRoad,Lap,LapCompleted,LapDistPct,LastLapTime,F2Time,YamlLapsComplete,YamlLastTime,YamlPosition,AbsentSeconds"
    )?;

    let mut client = Client::new();
    let session = wait_for_session(&mut client)?;
    let mut recorder = Recorder::new(&mut writer, summary_file, Instant::now());
    let end = match recorder.capture_session(session, duration) {
        Ok(end) => end,
        Err(error) => {
            let _ = recorder.finish(CaptureEnd::Error);
            return Err(error);
        }
    };
    let rows = recorder.rows;
    recorder.finish(end)?;
    Ok(CaptureReport { data_path: path.to_owned(), summary_path, rows })
}

fn create_outputs(path: &Path, summary_path: &Path) -> anyhow::Result<(File, File)> {
    // Create both with CreateNew before attaching to iRacing: a mistaken path
    // must never overwrite prior evidence.  If the second creation fails, the
    // empty first file is left as the explicit, visible result of that new path.
    let data = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating new capture {}", path.display()))?;
    let summary = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(summary_path)
        .with_context(|| format!("creating new summary {}", summary_path.display()))?;
    Ok((data, summary))
}

fn wait_for_session(client: &mut Client) -> anyhow::Result<Session> {
    let deadline = Instant::now() + CONNECT_WAIT;
    loop {
        // SAFETY: iRacing owns the shared-memory layout; this diagnostic keeps
        // the resulting session on this one thread and only reads it.
        if let Some(session) = unsafe { client.session() } {
            return Ok(session);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("iRacing did not become available within {CONNECT_WAIT:?}");
        }
        std::thread::sleep(remaining.min(CONNECT_STEP));
    }
}

#[derive(Debug)]
struct CaptureVars {
    session_time: Option<Var>,
    session_num: Option<Var>,
    track_surface: Option<Var>,
    direct_on_pit_road: Option<Var>,
    lap: Option<Var>,
    lap_completed: Option<Var>,
    lap_dist_pct: Option<Var>,
    last_lap_time: Option<Var>,
    f2_time: Option<Var>,
}

impl CaptureVars {
    /// # Safety
    /// `session` must be the live session used for the complete capture.
    unsafe fn find(session: &Session) -> Self {
        let find = |name| {
            // SAFETY: forwards this function's contract; it only reads SDK metadata.
            unsafe { session.find_var(name) }
        };
        Self {
            session_time: find("SessionTime"),
            session_num: find("SessionNum"),
            track_surface: find("CarIdxTrackSurface"),
            direct_on_pit_road: find("CarIdxOnPitRoad"),
            lap: find("CarIdxLap"),
            lap_completed: find("CarIdxLapCompleted"),
            lap_dist_pct: find("CarIdxLapDistPct"),
            last_lap_time: find("CarIdxLastLapTime"),
            f2_time: find("CarIdxF2Time"),
        }
    }

    fn missing(&self) -> Vec<&'static str> {
        [
            ("SessionTime", self.session_time.is_none()),
            ("SessionNum", self.session_num.is_none()),
            ("CarIdxTrackSurface", self.track_surface.is_none()),
            ("CarIdxOnPitRoad", self.direct_on_pit_road.is_none()),
            ("CarIdxLap", self.lap.is_none()),
            ("CarIdxLapCompleted", self.lap_completed.is_none()),
            ("CarIdxLapDistPct", self.lap_dist_pct.is_none()),
            ("CarIdxLastLapTime", self.last_lap_time.is_none()),
            ("CarIdxF2Time", self.f2_time.is_none()),
        ]
        .into_iter()
        .filter_map(|(name, absent)| absent.then_some(name))
        .collect()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct YamlCar {
    laps_complete: i32,
    last_time: f32,
    position: i32,
}

#[derive(Debug, Default)]
struct YamlState {
    update: Option<i32>,
    cars: HashMap<i32, YamlCar>,
    parse_failures: usize,
    parse_failed: bool,
}

impl YamlState {
    /// Refreshes precisely once for each SDK session-info revision.
    ///
    /// # Safety
    /// `session` must be live for the duration of this read.
    unsafe fn refresh(&mut self, session: &Session, session_num: Option<i32>) -> bool {
        // SAFETY: forwards this function's contract; the counter is an SDK read.
        let update = unsafe { session.session_info_update() };
        if self.update == Some(update) {
            return false;
        }
        self.update = Some(update);
        // SAFETY: forwards this function's contract; this copies the SDK YAML.
        let yaml = unsafe { session.session_info() };
        if let Ok(info) = SessionInfoYaml::parse(&yaml) {
            self.cars = classification(&info, session_num);
            self.parse_failed = false;
        } else {
            self.cars.clear();
            self.parse_failures += 1;
            self.parse_failed = true;
        }
        true
    }
}

fn classification(info: &SessionInfoYaml, session_num: Option<i32>) -> HashMap<i32, YamlCar> {
    let Some(session_num) = session_num else { return HashMap::new() };
    let Some(results) = info.session_info.sessions.iter().find(|entry| entry.session_num == session_num) else {
        return HashMap::new();
    };
    results.results_positions.iter().map(|entry| (entry.car_idx, yaml_car(entry))).collect()
}

fn yaml_car(entry: &ResultsPosition) -> YamlCar {
    YamlCar { laps_complete: entry.laps_complete, last_time: entry.last_time, position: entry.position }
}

#[derive(Debug, Default)]
struct Absence {
    started_session_time: f64,
    last_session_time: f64,
}

#[derive(Debug, Default)]
struct Continuity {
    active: HashMap<i32, Absence>,
    long_absences: usize,
    longest_absence: f64,
}

impl Continuity {
    fn reset(&mut self) {
        self.active.clear();
    }

    fn tick(&mut self, car_idx: i32, surface: Option<i32>, session_time: f64) {
        match surface {
            Some(-1) => {
                let absence = self
                    .active
                    .entry(car_idx)
                    .or_insert(Absence { started_session_time: session_time, last_session_time: session_time });
                absence.last_session_time = session_time;
            }
            Some(_) => self.finish(car_idx),
            None => self.invalidate(car_idx),
        }
    }

    fn absent_for(&self, car_idx: i32, session_time: f64) -> Option<f64> {
        self.active.get(&car_idx).map(|absence| (session_time - absence.started_session_time).max(0.0))
    }

    fn finish(&mut self, car_idx: i32) {
        let Some(absence) = self.active.remove(&car_idx) else { return };
        self.record_duration((absence.last_session_time - absence.started_session_time).max(0.0));
    }

    /// Drops an interrupted interval. Unlike an observed return, a missing
    /// SDK surface cannot prove that the car stayed absent through the gap.
    fn invalidate(&mut self, car_idx: i32) {
        self.active.remove(&car_idx);
    }

    fn close_all(&mut self) {
        for (_, absence) in std::mem::take(&mut self.active) {
            self.record_duration((absence.last_session_time - absence.started_session_time).max(0.0));
        }
    }

    fn record_duration(&mut self, duration: f64) {
        self.longest_absence = self.longest_absence.max(duration);
        if duration >= LONG_ABSENCE_SECS {
            self.long_absences += 1;
        }
    }
}

struct Recorder<'a> {
    writer: &'a mut BufWriter<File>,
    summary: File,
    started: Instant,
    next_sample: Instant,
    last_flush: Instant,
    rows: usize,
    ticks: usize,
    yaml_updates: usize,
    yaml: YamlState,
    previous_yaml_laps: HashMap<i32, i32>,
    continuity: Continuity,
    lap_advances_while_absent: usize,
    likely_genuine_lap_advances: usize,
    stale_or_ambiguous_lap_advances: usize,
    last_session_time: Option<f64>,
    last_session_num: Option<i32>,
    last_tick: Option<Instant>,
    resets_for_time: usize,
    resets_for_phase: usize,
    resets_for_gap: usize,
    resets_for_surface: usize,
    missing_vars: Vec<&'static str>,
}

impl<'a> Recorder<'a> {
    fn new(writer: &'a mut BufWriter<File>, summary: File, started: Instant) -> Self {
        Self {
            writer,
            summary,
            started,
            next_sample: started,
            last_flush: started,
            rows: 0,
            ticks: 0,
            yaml_updates: 0,
            yaml: YamlState::default(),
            previous_yaml_laps: HashMap::new(),
            continuity: Continuity::default(),
            lap_advances_while_absent: 0,
            likely_genuine_lap_advances: 0,
            stale_or_ambiguous_lap_advances: 0,
            last_session_time: None,
            last_session_num: None,
            last_tick: None,
            resets_for_time: 0,
            resets_for_phase: 0,
            resets_for_gap: 0,
            resets_for_surface: 0,
            missing_vars: Vec::new(),
        }
    }

    fn capture_session(&mut self, mut session: Session, duration: Duration) -> anyhow::Result<CaptureEnd> {
        // SAFETY: `session` stays on this thread; all later reads use only Vars
        // obtained from this exact session.
        let vars = unsafe { CaptureVars::find(&session) };
        self.missing_vars = vars.missing();
        let deadline = self.started + duration;
        while Instant::now() < deadline {
            // SAFETY: one bounded SDK wait, never longer than 500 ms.
            match unsafe { session.wait_for_data(DATA_WAIT) } {
                DataUpdateResult::Updated => self.updated(&session, &vars)?,
                DataUpdateResult::SessionExpired => return Ok(CaptureEnd::SessionExpired),
                DataUpdateResult::NoUpdate | DataUpdateResult::FailedToCopyRow => {}
            }
            if Instant::now().saturating_duration_since(self.last_flush) >= FLUSH_PERIOD {
                self.writer.flush()?;
                self.last_flush = Instant::now();
            }
        }
        self.writer.flush()?;
        Ok(CaptureEnd::Duration)
    }

    fn updated(&mut self, session: &Session, vars: &CaptureVars) -> anyhow::Result<()> {
        let now = Instant::now();
        self.ticks += 1;
        let session_time = read_scalar_f64(session, vars.session_time.as_ref()).filter(|time| time.is_finite());
        let session_num = read_scalar_i32(session, vars.session_num.as_ref());
        let surfaces = read_i32s(session, vars.track_surface.as_ref());
        let reset = self.should_reset(now, session_time, session_num, surfaces.is_some());
        if reset {
            self.continuity.reset();
            self.previous_yaml_laps.clear();
            self.yaml.update = None;
        }
        self.last_tick = Some(now);

        // Apply the current surface before a YAML revision is examined. A car
        // returning this tick is therefore never attributed to an absence.
        if let (Some(time), Some(surfaces)) = (session_time, surfaces) {
            for car_idx in self.yaml.cars.keys().copied().collect::<Vec<_>>() {
                self.continuity.tick(car_idx, array_at(Some(surfaces), car_idx), time);
            }
        }
        // SAFETY: `session` is live for this SDK tick; refresh only copies its YAML.
        let yaml_changed = unsafe { self.yaml.refresh(session, session_num) };
        if yaml_changed {
            self.yaml_updates += 1;
            if self.yaml.parse_failed {
                self.continuity.reset();
                self.previous_yaml_laps.clear();
            }
        }

        if let (Some(time), Some(surfaces)) = (session_time, surfaces) {
            for car_idx in self.yaml.cars.keys().copied().collect::<Vec<_>>() {
                let surface = array_at(Some(surfaces), car_idx);
                if surface.is_none() {
                    self.resets_for_surface += 1;
                }
                self.continuity.tick(car_idx, surface, time);
            }
        }
        if yaml_changed {
            self.observe_yaml_lap_advances(session_time);
        }

        if yaml_changed || now >= self.next_sample {
            self.write_sample(session, vars, session_time, session_num)?;
            self.next_sample = now + SAMPLE_PERIOD;
        }
        Ok(())
    }

    fn should_reset(
        &mut self,
        now: Instant,
        session_time: Option<f64>,
        session_num: Option<i32>,
        have_surfaces: bool,
    ) -> bool {
        let mut reset = false;
        if self.last_tick.is_some_and(|last| now.saturating_duration_since(last) > MAX_TICK_GAP) {
            self.resets_for_gap += 1;
            reset = true;
        }
        if session_time.is_none() || session_num.is_none() || !have_surfaces {
            self.resets_for_surface += usize::from(!have_surfaces);
            self.resets_for_time += usize::from(session_time.is_none());
            self.resets_for_phase += usize::from(session_num.is_none());
            reset = true;
        }
        if let (Some(previous), Some(current)) = (self.last_session_time, session_time)
            && current < previous
        {
            self.resets_for_time += 1;
            reset = true;
        }
        if let (Some(previous), Some(current)) = (self.last_session_num, session_num)
            && current != previous
        {
            self.resets_for_phase += 1;
            reset = true;
        }
        self.last_session_time = session_time;
        self.last_session_num = session_num;
        reset
    }

    fn observe_yaml_lap_advances(&mut self, session_time: Option<f64>) {
        let Some(session_time) = session_time else { return };
        for (&car_idx, yaml) in &self.yaml.cars {
            let old = self.previous_yaml_laps.insert(car_idx, yaml.laps_complete);
            let Some(old) = old else { continue };
            if yaml.laps_complete <= old || self.continuity.absent_for(car_idx, session_time).is_none() {
                continue;
            }
            self.lap_advances_while_absent += 1;
            let absent_for = self.continuity.absent_for(car_idx, session_time).unwrap_or_default();
            let strict = f64::from(yaml.last_time).is_finite()
                && yaml.last_time > 0.0
                && absent_for > f64::from(yaml.last_time) + LONG_ABSENCE_SECS;
            if strict {
                self.likely_genuine_lap_advances += 1;
            } else {
                self.stale_or_ambiguous_lap_advances += 1;
            }
        }
    }

    fn write_sample(
        &mut self,
        session: &Session,
        vars: &CaptureVars,
        session_time: Option<f64>,
        session_num: Option<i32>,
    ) -> anyhow::Result<()> {
        let surfaces = read_i32s(session, vars.track_surface.as_ref());
        let pit_road = read_bools(session, vars.direct_on_pit_road.as_ref());
        let lap = read_i32s(session, vars.lap.as_ref());
        let lap_completed = read_i32s(session, vars.lap_completed.as_ref());
        let lap_dist = read_f32s(session, vars.lap_dist_pct.as_ref());
        let last_lap = read_f32s(session, vars.last_lap_time.as_ref());
        let f2_time = read_f32s(session, vars.f2_time.as_ref());
        // SAFETY: read-only SDK counter, while this exact session remains live.
        let yaml_update = unsafe { session.session_info_update() };
        let elapsed = self.started.elapsed().as_secs_f64();
        for (&car_idx, yaml) in &self.yaml.cars {
            write_numeric_row(
                self.writer,
                [
                    Some(elapsed),
                    session_time,
                    session_num.map(f64::from),
                    Some(f64::from(yaml_update)),
                    Some(f64::from(car_idx)),
                    array_at(surfaces, car_idx).map(f64::from),
                    array_at(pit_road, car_idx).map(|value| if value { 1.0 } else { 0.0 }),
                    array_at(lap, car_idx).map(f64::from),
                    array_at(lap_completed, car_idx).map(f64::from),
                    array_at(lap_dist, car_idx).map(f64::from),
                    array_at(last_lap, car_idx).map(f64::from),
                    array_at(f2_time, car_idx).map(f64::from),
                    Some(f64::from(yaml.laps_complete)),
                    Some(f64::from(yaml.last_time)),
                    Some(f64::from(yaml.position)),
                    session_time.and_then(|time| self.continuity.absent_for(car_idx, time)),
                ],
            )?;
            self.rows += 1;
        }
        Ok(())
    }

    fn finish(&mut self, end: CaptureEnd) -> anyhow::Result<()> {
        self.continuity.close_all();
        self.writer.flush()?;
        let mut summary = BufWriter::new(&self.summary);
        writeln!(summary, "Scoring capture summary")?;
        writeln!(summary, "captured_duration_secs: {:.3}", self.started.elapsed().as_secs_f64())?;
        writeln!(summary, "end_reason: {}", end.label())?;
        writeln!(summary, "csv_rows: {}", self.rows)?;
        writeln!(summary, "sdk_ticks: {}", self.ticks)?;
        writeln!(summary, "yaml_updates: {}", self.yaml_updates)?;
        writeln!(summary, "yaml_parse_failures: {}", self.yaml.parse_failures)?;
        let missing_variables =
            if self.missing_vars.is_empty() { "none".to_owned() } else { self.missing_vars.join(",") };
        writeln!(summary, "missing_sdk_variables: {missing_variables}")?;
        writeln!(summary, "lap_advances_while_continuously_NotInWorld: {}", self.lap_advances_while_absent)?;
        writeln!(summary, "strict_likely_genuine_during_absence: {}", self.likely_genuine_lap_advances)?;
        writeln!(summary, "stale_or_ambiguous_publication: {}", self.stale_or_ambiguous_lap_advances)?;
        writeln!(summary, "long_absences_at_least_{LONG_ABSENCE_SECS:.0}s: {}", self.continuity.long_absences)?;
        writeln!(summary, "longest_continuous_absence_secs: {:.3}", self.continuity.longest_absence)?;
        writeln!(summary, "continuity_resets_time: {}", self.resets_for_time)?;
        writeln!(summary, "continuity_resets_phase: {}", self.resets_for_phase)?;
        writeln!(summary, "continuity_resets_local_gap: {}", self.resets_for_gap)?;
        writeln!(summary, "continuity_resets_missing_surface: {}", self.resets_for_surface)?;
        writeln!(
            summary,
            "strict rule: YAML LapsComplete advanced, every SDK tick remained NotInWorld, and absence exceeded YAML LastTime + 30 seconds"
        )?;
        writeln!(summary, "absence of a strict event is inconclusive; a strict event still needs the CSV reviewed")?;
        summary.flush()?;
        Ok(())
    }
}

fn write_numeric_row(writer: &mut BufWriter<File>, values: [Option<f64>; 16]) -> std::io::Result<()> {
    for (index, value) in values.into_iter().enumerate() {
        if index > 0 {
            write!(writer, ",")?;
        }
        if let Some(value) = value.filter(|value| value.is_finite()) {
            write!(writer, "{value}")?;
        }
    }
    writeln!(writer)
}

fn array_at<T: Copy>(values: Option<&[T]>, car_idx: i32) -> Option<T> {
    usize::try_from(car_idx).ok().and_then(|index| values.and_then(|values| values.get(index)).copied())
}

fn read_scalar_f64(session: &Session, var: Option<&Var>) -> Option<f64> {
    var.and_then(|var| {
        // SAFETY: `var` came from this session and the caller holds its live row.
        unsafe { session.value::<f64>(var).ok() }
    })
}

fn read_scalar_i32(session: &Session, var: Option<&Var>) -> Option<i32> {
    var.and_then(|var| {
        // SAFETY: `var` came from this session and the caller holds its live row.
        unsafe { session.value::<i32>(var).ok() }
    })
}

fn read_i32s<'a>(session: &'a Session, var: Option<&'a Var>) -> Option<&'a [i32]> {
    var.and_then(|var| {
        // SAFETY: `var` came from this session and values borrow only the current row.
        unsafe { session.value::<&[i32]>(var).ok() }
    })
}

fn read_f32s<'a>(session: &'a Session, var: Option<&'a Var>) -> Option<&'a [f32]> {
    var.and_then(|var| {
        // SAFETY: `var` came from this session and values borrow only the current row.
        unsafe { session.value::<&[f32]>(var).ok() }
    })
}

fn read_bools<'a>(session: &'a Session, var: Option<&'a Var>) -> Option<&'a [bool]> {
    var.and_then(|var| {
        // SAFETY: `var` came from this session and values borrow only the current row.
        unsafe { session.value::<&[bool]>(var).ok() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_or_missing_surface_ends_an_absence() {
        let mut continuity = Continuity::default();
        continuity.tick(7, Some(-1), 10.0);
        continuity.tick(7, Some(-1), 45.0);
        continuity.tick(7, Some(3), 46.0);
        assert!(continuity.absent_for(7, 46.0).is_none());
        assert_eq!(continuity.long_absences, 1);
        continuity.tick(7, Some(-1), 50.0);
        continuity.tick(7, None, 51.0);
        assert!(continuity.absent_for(7, 51.0).is_none());
    }

    #[test]
    fn reset_discards_an_interrupted_absence() {
        let mut continuity = Continuity::default();
        continuity.tick(2, Some(-1), 20.0);
        continuity.tick(2, Some(-1), 80.0);
        continuity.reset();
        assert!(continuity.absent_for(2, 81.0).is_none());
        assert_eq!(continuity.long_absences, 0);
    }

    #[test]
    fn missing_fields_are_reported_separately() {
        let vars = CaptureVars {
            session_time: None,
            session_num: None,
            track_surface: None,
            direct_on_pit_road: None,
            lap: None,
            lap_completed: None,
            lap_dist_pct: None,
            last_lap_time: None,
            f2_time: None,
        };
        let missing: std::collections::HashSet<_> = vars.missing().into_iter().collect();
        assert!(missing.contains("SessionTime"));
        assert!(missing.contains("CarIdxTrackSurface"));
        assert_eq!(missing.len(), 9);
    }
}
