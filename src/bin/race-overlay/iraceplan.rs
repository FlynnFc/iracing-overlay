//! Read-only iRacePlan integration. Network work never runs on the render thread.
//! A session join discovers the website's scheduled plan; renders only read its cache.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

mod discovery;
pub mod handover;
mod session;

const CREDENTIAL: windows::core::PCWSTR = windows::core::w!("race-overlay/iraceplan");
const POLL: Duration = Duration::from_secs(30);
static RECHECK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const STALE: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("race/iraceplan.toml"))
}

impl Config {
    fn load() -> Result<Self, &'static str> {
        let path = config_path().ok_or("Cannot locate user settings")?;
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|_error| "Invalid iRacePlan settings"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("Cannot read iRacePlan settings"),
        }
    }

    fn save(&self) -> Result<(), &'static str> {
        let path = config_path().ok_or("Cannot locate user settings")?;
        std::fs::create_dir_all(path.parent().ok_or("Invalid settings path")?)
            .map_err(|_error| "Cannot create settings folder")?;
        let text = toml::to_string(self).map_err(|_error| "Cannot encode settings")?;
        std::fs::write(path, text).map_err(|_error| "Cannot save iRacePlan settings")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Driver {
    pub iracing_id: u32,
    pub name: String,
    pub average_fuel_consumption_dry: Option<f64>,
    pub average_fuel_consumption_wet: Option<f64>,
    pub average_lap_time_dry: Option<f64>,
    pub average_lap_time_wet: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Stint {
    pub number: u32,
    pub driver: Option<Driver>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub setup: Option<String>,
    pub estimated_laps: Option<u32>,
    pub fuel_consumption: Option<f64>,
    pub pit_fuel_capacity: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Strategy {
    pub id: u64,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub drivers: Vec<Driver>,
    pub stints: Vec<Stint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Identity {
    pub iracing_id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventSession {
    #[serde(default)]
    pub name: String,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
}

impl EventSession {
    fn contains(&self, now: DateTime<Utc>) -> bool {
        self.end_at > self.start_at
            && now >= self.start_at - chrono::Duration::minutes(30)
            && now <= self.end_at + chrono::Duration::minutes(30)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    #[default]
    Team,
    Individual,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Planning {
    #[serde(rename = "type", default)]
    pub kind: Registration,
    #[serde(default)]
    pub session: Option<EventSession>,
    pub id: u64,
    pub car: String,
    pub registrable: Identity,
    pub track: Identity,
    pub strategies: Vec<Strategy>,
}

#[derive(Deserialize)]
struct Response {
    planning: Planning,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub planning: Planning,
    pub strategy: Strategy,
}

#[derive(Clone, Copy)]
enum StrategyChoice<'a> {
    Scheduled(&'a str),
    Id(u64),
}

impl Plan {
    fn in_event_window(&self, now: DateTime<Utc>) -> bool {
        self.planning.session.as_ref().map_or_else(
            || {
                self.strategy.stints.first().zip(self.strategy.stints.last()).is_some_and(|(first, last)| {
                    EventSession { name: String::new(), start_at: first.start_at, end_at: last.end_at }.contains(now)
                })
            },
            |session| session.contains(now),
        )
    }

    fn parse(body: &[u8], planning_id: u64, choice: StrategyChoice<'_>) -> Result<Self, &'static str> {
        let response: Response = serde_json::from_slice(body).map_err(|_error| "Unrecognised planning response")?;
        let planning = response.planning;
        if planning.id != planning_id {
            return Err("Planning identity did not match");
        }
        let mut candidates = planning.strategies.iter().filter(|strategy| match choice {
            StrategyChoice::Scheduled(name) => strategy.name == name,
            StrategyChoice::Id(id) => strategy.id == id,
        });
        let mut strategy = candidates.next().cloned().ok_or("Scheduled strategy not found")?;
        if candidates.next().is_some() {
            return Err("Multiple strategies share the scheduled name");
        }
        strategy.stints.sort_by_key(|s| s.number);
        if strategy.stints.is_empty() {
            return Err("Scheduled strategy has no stints");
        }
        if strategy.stints.iter().any(|s| s.end_at <= s.start_at)
            || strategy.stints.windows(2).any(|s| s[0].number >= s[1].number || s[0].end_at > s[1].start_at)
        {
            return Err("Strategy contains invalid stint times or numbers");
        }
        Ok(Self { planning, strategy })
    }

    /// Current scheduled stint, the next one in a pit gap, or the first before the race.
    /// Never guesses the active stint from a driver's name: double/triple stints repeat it.
    pub fn preview_index(&self, now: DateTime<Utc>) -> Option<usize> {
        self.strategy.stints.iter().position(|s| now < s.end_at)
    }

    pub fn fuel_target(&self, stint: &Stint, driver_id: u32) -> Option<f64> {
        let driver = self.strategy.drivers.iter().find(|d| d.iracing_id == driver_id)?;
        let value = match stint.setup.as_deref() {
            Some("dry") => driver.average_fuel_consumption_dry,
            Some("wet") => driver.average_fuel_consumption_wet,
            _ => None,
        };
        value.filter(|v| v.is_finite() && *v > 0.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Feed {
    pub subsession: Option<u64>,
    pub plan: Option<Arc<Plan>>,
    pub fetched: Option<Instant>,
    pub status: String,
    pub failed: bool,
}

impl Feed {
    pub fn stale(&self) -> bool {
        self.failed || self.fetched.is_none_or(|at| at.elapsed() >= STALE)
    }
}

static FEED: OnceLock<RwLock<Feed>> = OnceLock::new();
static WORKER: OnceLock<()> = OnceLock::new();

pub fn feed() -> Feed {
    FEED.get_or_init(|| RwLock::new(Feed::default())).read().map(|s| s.clone()).unwrap_or_default()
}

pub fn available(snapshot: Option<&crate::telemetry::snapshot::TelemetrySnapshot>) -> bool {
    let Some(context) = snapshot.and_then(session::Context::from_snapshot) else { return false };
    let feed = feed();
    feed.subsession == Some(context.key) && feed.plan.as_ref().is_some_and(|plan| context.matches(plan, Utc::now()))
}

pub fn observe_session(snapshot: Option<&crate::telemetry::snapshot::TelemetrySnapshot>) {
    session::observe(snapshot);
}

/// Explicit Stints demo only; never contacts the API or saves a race plan.
pub fn show_demo(states: &[String]) {
    let handover = states.iter().any(|state| state.starts_with("handover"));
    let delayed = states.iter().any(|state| state == "handover-delay");
    let stale = states.iter().any(|state| state == "handover-stale");
    let minutes = if delayed {
        132
    } else if handover {
        112
    } else {
        23
    };
    let plan = example_plan(Utc::now() - chrono::Duration::minutes(minutes));
    publish(|state| {
        *state = Feed {
            subsession: Some(1),
            plan: Some(Arc::new(plan)),
            fetched: Instant::now().checked_sub(std::time::Duration::from_secs(if stale { 120 } else { 0 })),
            status: if stale { "Demo connection interrupted" } else { "Demo" }.to_owned(),
            failed: stale,
        }
    });
}

pub fn example_plan(start: DateTime<Utc>) -> Plan {
    let drivers = vec![
        Driver {
            iracing_id: 1,
            name: "Alex Morgan".to_owned(),
            average_fuel_consumption_dry: Some(3.5),
            average_fuel_consumption_wet: Some(0.0),
            average_lap_time_dry: Some(120_000.0),
            average_lap_time_wet: None,
        },
        Driver {
            iracing_id: 2,
            name: "Sam Taylor".to_owned(),
            average_fuel_consumption_dry: Some(3.42),
            average_fuel_consumption_wet: None,
            average_lap_time_dry: Some(120_000.0),
            average_lap_time_wet: None,
        },
    ];
    let stints = [0, 0, 1, 1]
        .into_iter()
        .enumerate()
        .map(|(index, driver)| {
            let offset = chrono::Duration::minutes(i64::try_from(index).unwrap_or(0) * 60);
            Stint {
                number: u32::try_from(index + 1).unwrap_or(1),
                driver: Some(drivers[driver].clone()),
                start_at: start + offset,
                end_at: start + offset + chrono::Duration::minutes(58),
                setup: Some("dry".to_owned()),
                estimated_laps: Some(29),
                fuel_consumption: Some(101.5),
                pit_fuel_capacity: Some(102.0),
            }
        })
        .collect();
    let strategy = Strategy { id: 1, name: "DEMO plan".to_owned(), status: "up-to-date".to_owned(), drivers, stints };
    Plan {
        planning: Planning {
            kind: Registration::Team,
            session: Some(EventSession {
                name: "Demo endurance race".to_owned(),
                start_at: start - chrono::Duration::minutes(45),
                end_at: start + chrono::Duration::hours(4),
            }),
            id: 1,
            car: "Ferrari 296 GT3".to_owned(),
            registrable: Identity { iracing_id: 1, name: "Example Racing".to_owned() },
            track: Identity { iracing_id: 341, name: "Silverstone".to_owned() },
            strategies: Vec::new(),
        },
        strategy,
    }
}

fn publish(update: impl FnOnce(&mut Feed)) {
    if let Ok(mut state) = FEED.get_or_init(|| RwLock::new(Feed::default())).write() {
        update(&mut state);
    }
}

pub fn start() {
    WORKER.get_or_init(|| {
        let result = std::thread::Builder::new().name("iraceplan".to_owned()).spawn(worker);
        if result.is_err() {
            publish(|s| "Cannot start iRacePlan connection".clone_into(&mut s.status));
        }
    });
}

fn worker() {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("RaceOverlay/iRacePlan")
        .build();
    let Ok(client) = client else {
        publish(|s| "Cannot initialise iRacePlan connection".clone_into(&mut s.status));
        return;
    };
    let mut previous = Config::default();
    let mut refresh = session::Refresh::default();
    loop {
        match Config::load() {
            Ok(config) => {
                let recheck = RECHECK.swap(false, std::sync::atomic::Ordering::Relaxed);
                if config != previous || recheck {
                    publish(|s| *s = Feed::default());
                    refresh = session::Refresh::default();
                    previous = config.clone();
                }
                poll_session(&client, &config, &mut refresh);
            }
            Err(message) => {
                refresh = session::Refresh::default();
                publish(|s| *s = Feed { status: message.to_owned(), failed: true, ..Feed::default() });
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn poll_session(client: &reqwest::blocking::Client, config: &Config, refresh: &mut session::Refresh) {
    let context = config.enabled.then(session::current).flatten();
    let cached = feed();
    let matched = context.as_ref().is_some_and(|context| {
        cached.subsession == Some(context.key)
            && cached.plan.as_ref().is_some_and(|plan| context.matches(plan, Utc::now()))
    });
    let action = refresh.due(context.as_ref().map(|context| context.key), matched, Instant::now());
    let Some(context) = context else {
        publish(|s| {
            *s = Feed {
                status: if config.enabled { "Paused / waiting for a session" } else { "Not connected" }.to_owned(),
                ..Feed::default()
            }
        });
        return;
    };
    let Some(action) = action else { return };
    let result = match action {
        session::Action::Discover => {
            publish(|s| {
                *s = Feed {
                    subsession: Some(context.key),
                    status: "Checking iRacePlan for this session".to_owned(),
                    ..Feed::default()
                }
            });
            discovery::discover(&context, Utc::now(), |path| {
                if session::current().is_none_or(|current| current.key != context.key) {
                    return Err("Session changed");
                }
                read_api(client, path)
            })
        }
        session::Action::Update => cached.plan.as_ref().ok_or("No matching plan").and_then(|plan| {
            let body = read_api(client, &format!("plannings/{}", plan.planning.id))?;
            Plan::parse(&body, plan.planning.id, StrategyChoice::Id(plan.strategy.id)).map(Some)
        }),
    };
    // Ignore an old room's result and stop a multi-request lookup when the user leaves.
    let Some(current) = session::current().filter(|current| current.key == context.key) else {
        return;
    };
    publish(|s| match result {
        Ok(plan) => {
            let plan = plan.filter(|plan| current.matches(plan, Utc::now()));
            let status = plan.as_ref().map_or_else(
                || "No stint plan for this session".to_owned(),
                |plan| {
                    format!(
                        "Detected {}",
                        plan.planning.session.as_ref().map_or("race plan", |session| session.name.as_str())
                    )
                },
            );
            *s = Feed {
                subsession: Some(context.key),
                plan: plan.map(Arc::new),
                fetched: Some(Instant::now()),
                status,
                failed: false,
            }
        }
        Err(message) => {
            message.clone_into(&mut s.status);
            s.failed = true;
            if matches!(
                message,
                "Planning not found"
                    | "Scheduled strategy not found"
                    | "Scheduled strategy has no stints"
                    | "API key rejected or planning access denied"
            ) {
                s.plan = None;
            }
        }
    });
    let still_matches = feed().plan.is_some_and(|plan| current.matches(&plan, Utc::now()));
    refresh.completed(still_matches, Instant::now());
}

fn read_api(client: &reqwest::blocking::Client, path: &str) -> Result<Vec<u8>, &'static str> {
    use std::io::Read;
    let key = read_key().ok_or("Add API key in Black Box settings")?;
    let response = client
        .get(format!("https://iraceplan.com/api/v1/{path}"))
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send()
        .map_err(|_error| "Connection failed")?;
    match response.status().as_u16() {
        200 => (),
        401 | 403 => return Err("API key rejected or planning access denied"),
        404 => return Err("Planning not found"),
        429 => return Err("Rate limited by iRacePlan"),
        _ => return Err("iRacePlan unavailable"),
    }
    // A corrupt or unexpectedly large response must not exhaust memory.
    let mut body = Vec::new();
    response.take(2_000_001).read_to_end(&mut body).map_err(|_error| "Incomplete planning response")?;
    if body.len() > 2_000_000 {
        return Err("Planning response too large");
    }
    Ok(body)
}

fn read_key() -> Option<String> {
    if let Ok(key) = std::env::var("IRACEPLAN")
        && !key.trim().is_empty()
    {
        return Some(key.trim().to_owned());
    }
    read_saved_key().or_else(bundled_key)
}

fn bundled_key() -> Option<String> {
    #[cfg(feature = "bundled-iraceplan")]
    {
        option_env!("IRACEPLAN").filter(|key| !key.trim().is_empty()).map(|key| key.trim().to_owned())
    }
    #[cfg(not(feature = "bundled-iraceplan"))]
    {
        None
    }
}

fn read_saved_key() -> Option<String> {
    use windows::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW};
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: the target is a static terminated UTF-16 string and the output pointer is valid.
    unsafe { CredReadW(CREDENTIAL, CRED_TYPE_GENERIC, None, &raw mut credential).ok()? };
    // SAFETY: CredReadW succeeded; its allocation remains alive until CredFree below.
    let result = unsafe {
        let value = &*credential;
        if value.CredentialBlobSize == 0 || value.CredentialBlob.is_null() {
            None
        } else {
            String::from_utf8(
                std::slice::from_raw_parts(value.CredentialBlob, value.CredentialBlobSize as usize).to_vec(),
            )
            .ok()
        }
    };
    // SAFETY: free exactly the allocation returned by CredReadW, after copying its bytes.
    unsafe { CredFree(credential.cast()) };
    result.filter(|key| !key.trim().is_empty())
}

fn save_key(key: &str) -> Result<(), &'static str> {
    use windows::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredWriteW,
    };
    let mut bytes = key.trim().as_bytes().to_vec();
    if bytes.is_empty() || bytes.len() > 2048 {
        return Err("Enter a valid API key");
    }
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(CREDENTIAL.0.cast_mut()),
        CredentialBlobSize: u32::try_from(bytes.len()).map_err(|_error| "API key too long")?,
        CredentialBlob: bytes.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // SAFETY: target and blob outlive this call; CredWriteW copies them into the user's vault.
    let result = unsafe { CredWriteW(&raw const credential, 0) };
    bytes.fill(0);
    result.map_err(|_error| "Cannot save key in Windows Credential Manager")
}

#[derive(Clone, Default)]
struct Settings {
    config: Config,
    key: String,
    message: String,
}

/// The credential is entered as a password and never becomes part of OverlayConfig/debug output.
pub fn settings(ui: &mut egui::Ui) {
    let id = ui.id().with("iraceplan-settings");
    let mut edit = ui
        .data_mut(|data| data.get_temp::<Settings>(id))
        .unwrap_or_else(|| Settings { config: Config::load().unwrap_or_default(), ..Settings::default() });
    ui.separator();
    ui.strong("iRacePlan stints");
    let current = feed();
    ui.label(current.status);
    if let Some(plan) = current.plan {
        ui.small(format!(
            "{} · {} · {} stints",
            plan.planning.registrable.name,
            plan.strategy.name,
            plan.strategy.stints.len()
        ));
        ui.small(format!("{} · {}", plan.planning.track.name, plan.planning.car));
    }
    ui.checkbox(&mut edit.config.enabled, "Automatically detect iRacePlan stints");
    ui.small("Joining a session checks for its race plan. No match means no Stints page or background refresh.");
    ui.add(egui::TextEdit::singleline(&mut edit.key).password(true).hint_text("API key (blank keeps saved key)"));
    ui.small("Uses IRACEPLAN, a saved key, or the key bundled with this build. Updates every 30 seconds during a matching session.");
    if ui.button("Save iRacePlan connection").clicked() {
        let result =
            if edit.key.trim().is_empty() { Ok(()) } else { save_key(&edit.key) }.and_then(|()| edit.config.save());
        edit.key.clear();
        edit.message = result
            .map_or_else(str::to_owned, |()| "Saved; automatic detection runs when a session is joined".to_owned());
        RECHECK.store(true, std::sync::atomic::Ordering::Relaxed);
        start();
    }
    if !edit.message.is_empty() {
        ui.small(&edit.message);
    }
    ui.data_mut(|data| data.insert_temp(id, edit));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_boundaries_do_not_confuse_double_stints_or_pit_gaps() {
        let start = Utc::now();
        let plan = example_plan(start);
        assert_eq!(plan.preview_index(start - chrono::Duration::hours(1)), Some(0));
        assert_eq!(plan.preview_index(start + chrono::Duration::minutes(57)), Some(0));
        assert_eq!(plan.preview_index(start + chrono::Duration::minutes(58)), Some(1));
        assert_eq!(plan.preview_index(start + chrono::Duration::minutes(60)), Some(1));
        assert_eq!(plan.preview_index(start + chrono::Duration::minutes(238)), None);
    }

    #[test]
    fn unknown_or_zero_wet_target_is_not_replaced_with_dry_consumption() {
        let plan = example_plan(Utc::now());
        let mut stint = plan.strategy.stints[0].clone();
        assert_eq!(plan.fuel_target(&stint, 1), Some(3.5));
        assert_eq!(plan.fuel_target(&stint, 2), Some(3.42));
        stint.setup = Some("wet".to_owned());
        assert_eq!(plan.fuel_target(&stint, 1), None);
        assert_eq!(plan.fuel_target(&stint, 2), None);
        assert_eq!(plan.fuel_target(&stint, 999), None);
    }

    #[test]
    fn feed_failure_and_age_are_both_stale() {
        let mut feed = Feed { fetched: Some(Instant::now()), ..Feed::default() };
        assert!(!feed.stale());
        feed.failed = true;
        assert!(feed.stale());
        feed.failed = false;
        feed.fetched = Some(Instant::now() - STALE);
        assert!(feed.stale());
    }

    #[test]
    fn event_matching_prefers_api_session_dates_with_schedule_fallback() {
        let start = Utc::now();
        let mut plan = example_plan(start);
        plan.planning.session = Some(
            serde_json::from_value(serde_json::json!({
                "start_at": start - chrono::Duration::hours(1),
                "end_at": start + chrono::Duration::hours(4)
            }))
            .unwrap(),
        );
        assert!(plan.in_event_window(start - chrono::Duration::minutes(90)));
        assert!(!plan.in_event_window(start - chrono::Duration::minutes(91)));
        assert!(plan.in_event_window(start + chrono::Duration::minutes(270)));
        assert!(!plan.in_event_window(start + chrono::Duration::minutes(271)));
        plan.planning.session = None;
        assert!(plan.in_event_window(start - chrono::Duration::minutes(30)));
        assert!(!plan.in_event_window(start - chrono::Duration::minutes(31)));
        assert!(!plan.in_event_window(start + chrono::Duration::minutes(269)));
    }

    #[test]
    fn parsing_rejects_wrong_selection_and_overlapping_stints() {
        let mut body = serde_json::json!({"planning": {
            "id":42,"car":"Car","registrable":{"iracing_id":1,"name":"Team"},
            "track":{"iracing_id":341,"name":"Track"},"strategies":[{
                "id":7,"name":"Plan","status":"up-to-date","stints":[
                    {"number":1,"driver":{"iracing_id":1,"name":"A"},"start_at":"2026-09-19T12:00:00Z","end_at":"2026-09-19T13:00:00Z"},
                    {"number":2,"driver":null,"start_at":"2026-09-19T13:02:00Z","end_at":"2026-09-19T14:00:00Z"}
                ]
            }]
        }});
        assert!(Plan::parse(&serde_json::to_vec(&body).unwrap(), 42, StrategyChoice::Id(7)).is_ok());
        assert!(Plan::parse(&serde_json::to_vec(&body).unwrap(), 42, StrategyChoice::Id(8)).is_err());
        assert!(Plan::parse(&serde_json::to_vec(&body).unwrap(), 99, StrategyChoice::Id(7)).is_err());
        assert!(Plan::parse(&serde_json::to_vec(&body).unwrap(), 42, StrategyChoice::Scheduled("Plan")).is_ok());
        body["planning"]["strategies"][0]["stints"][1]["start_at"] = "2026-09-19T12:59:00Z".into();
        assert!(Plan::parse(&serde_json::to_vec(&body).unwrap(), 42, StrategyChoice::Id(7)).is_err());
    }

    #[test]
    fn automatic_detection_defaults_on_and_ignores_legacy_selection() {
        assert!(Config::default().enabled);
        let legacy: Config = toml::from_str("enabled = true\nplanning_id = 42\nstrategy_id = 7").unwrap();
        assert!(legacy.enabled);
        assert!(!toml::to_string(&legacy).unwrap().contains("planning_id"));
        assert!(!toml::from_str::<Config>("enabled = false").unwrap().enabled);
    }
}
