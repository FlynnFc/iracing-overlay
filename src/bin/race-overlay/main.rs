// Rust guideline compliant 2026-02-16

// Built as a GUI binary so double-clicking it — or having `race-launcher`
// start it — doesn't leave a console window sitting in the taskbar. Run from
// a terminal it still prints normally, because `attach_parent_console` below
// reattaches this process's standard streams to the calling console.
#![windows_subsystem = "windows"]

//! Entry point: connects to iRacing on a background thread and shows the
//! GT3 relative overlay window.
//!
//! Reads `%APPDATA%\race\race-overlay.toml`, migrating a legacy file beside
//! the executable or in the working directory when needed.
//!
//! Windowing is handled by `egui_overlay`, which creates a transparent,
//! always-on-top, undecorated, OpenGL-rendered window and lets us toggle
//! mouse passthrough at runtime — OpenGL is used deliberately (not wgpu)
//! because Windows' desktop compositor supports real per-pixel window
//! transparency far more reliably for GL surfaces than for DXGI swapchains.

mod app;
mod config;
mod demo;
mod focus;
mod input;
#[cfg(feature = "licence")]
mod licence;
mod perf;
mod sync;
mod telemetry;
mod tray;
mod ui;

use std::sync::mpsc;
use std::time::Duration;

use app::OverlayApp;
use config::OverlayConfig;
use telemetry::session::SnapshotTuning;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// File name the `--dump-session-info` diagnostic mode writes beside the exe.
const SESSION_INFO_DUMP_FILE: &str = "session_info_dump.yaml";

/// File name the `--dump-vars` diagnostic mode writes beside the exe.
const VARS_DUMP_FILE: &str = "vars_dump.txt";

/// How long the diagnostic dump modes wait for iRacing before giving up.
const DUMP_WAIT: Duration = Duration::from_secs(60);

fn main() {
    attach_parent_console();
    if run_early_diagnostic() {
        return;
    }
    if let Some(spec) = std::env::args().find_map(|arg| arg.strip_prefix("--sync-host=").map(str::to_owned)) {
        sync_host(&spec);
        return;
    }
    if let Some(spec) = std::env::args().find_map(|arg| arg.strip_prefix("--sync-join=").map(str::to_owned)) {
        sync_join(&spec);
        return;
    }
    if std::env::args().any(|arg| arg == "--list-devices") {
        for line in input::device::describe_all() {
            println!("{line}");
        }
        return;
    }
    if let Some(action) = bind_argument() {
        capture_bind(&action);
        return;
    }
    start_overlay();
}

/// Runs the early, non-interactive diagnostics and reports whether one ran.
fn run_early_diagnostic() -> bool {
    if std::env::args().any(|arg| arg == "--check-config") {
        check_config();
        return true;
    }
    if let Some(path) =
        std::env::args().find_map(|arg| arg.strip_prefix("--capture-scoring=").map(std::path::PathBuf::from))
    {
        capture_scoring(&path);
        return true;
    }
    if std::env::args().any(|arg| arg == "--dump-session-info") {
        dump_session_info();
        return true;
    }
    if std::env::args().any(|arg| arg == "--dump-all-vars") {
        println!("waiting up to {DUMP_WAIT:?} for iRacing (be on track, in the car, for meaningful values)...");
        if let Err(err) = telemetry::session::dump_all_vars(DUMP_WAIT) {
            println!("error: {err:#}");
        }
        return true;
    }
    if std::env::args().any(|arg| arg == "--dump-vars") {
        dump_vars();
        return true;
    }
    false
}

/// Starts the ordinary overlay after no command-line diagnostic was selected.
fn start_overlay() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    let demo_states = demo_states();
    // Reproducible settings previews, e.g. --demo --demo-settings=standings
    // --screenshot=settings.png. This flag never opens settings in a live run.
    let demo_settings = std::env::args().find_map(|arg| arg.strip_prefix("--demo-settings=").map(str::to_owned));
    // `--demo-page=fuel` and friends, so a black box page can be held up
    // against its mockup; without it demo mode can only ever show page one.
    let demo_page =
        std::env::args().find_map(|arg| arg.strip_prefix("--demo-page=").map(str::to_owned)).and_then(|name| {
            let page = ui::blackbox::Page::from_arg(&name);
            if page.is_none() {
                println!("note: --demo-page={name} is not a page; showing the Relative");
            }
            page
        });

    // `--screenshot=<path>` writes one rendered frame to a PNG and quits.
    // The overlay's window is layered and drawn by OpenGL, which every
    // ordinary screen-capture route reads as either black or nearly
    // transparent, so the only way to see what a widget actually draws is to
    // have the renderer hand its own framebuffer over.
    let screenshot = std::env::args().find_map(|arg| arg.strip_prefix("--screenshot=").map(std::path::PathBuf::from));

    let config = OverlayConfig::load().unwrap_or_else(|err| {
        println!("note: {err:#}; using default overlay settings");
        OverlayConfig::default()
    });
    // The temporary first-run flow completes before companion apps, telemetry,
    // controls or the tray are started. Preview runs never persist completion.
    // Built without the `licence` feature there is no setup step at all: that
    // build carries straight on into the normal startup below.
    #[cfg(feature = "licence")]
    {
        let licence_preview = std::env::args().any(|arg| arg == "--license-preview");
        if licence_preview {
            licence::show_first_start(true, screenshot);
            return;
        }
        if !demo && !licence::show_first_start(false, screenshot.clone()) {
            return;
        }
    }
    // The session programs, if the Launcher page asked for them to come up
    // with the overlay. Not in demo mode: looking at mockups is not a session.
    if !demo {
        race_tools::launcher::start_with_overlay_if_set();
    }

    let (tx, rx) = mpsc::channel();
    // The UI queues pit intents here; the telemetry thread carries them out,
    // because iRacing's `Session` is `!Send` and cannot leave that thread.
    let (request_tx, request_rx) = mpsc::channel();
    if demo {
        if !send_demo_snapshot(&tx, &demo_states) {
            return;
        }
    } else {
        // How many rows the Relative shows is no longer a telemetry concern:
        // the whole field is sent and the widget slices its own window, so it
        // can scroll.
        let tuning = SnapshotTuning {
            pit_loss_secs: config.standings.pit_loss_secs,
            radar_range_secs: config.radar.range_ms / 1000.0,
        };
        // The telemetry connection is !Send (internal Rc), so `Client` and
        // `Session` must be created inside this thread's closure. The overlay
        // repaints continuously (see `OverlayApp::gui_run`), so no repaint
        // callback is needed here.
        std::thread::spawn(move || telemetry::session::run(&tx, &request_rx, tuning, || {}));
    }

    // The tray is built before the window so a failure to register it is
    // reported plainly rather than leaving a running overlay with no way to
    // quit it.
    let tray = match tray::Tray::new() {
        Ok(tray) => Some(tray),
        Err(err) => {
            println!("note: {err:#}; the overlay will run without a tray icon");
            None
        }
    };

    // Captured before the overlay window exists, so it can be handed back —
    // see `focus::yield_foreground`.
    let launched_from = focus::foreground_window();
    let demo = app::DemoOptions {
        enabled: demo,
        page: demo_page,
        settings_page: demo_settings,
        screenshot,
        states: demo_states,
    };
    egui_overlay::start(OverlayApp::new(rx, request_tx, config, demo, tray, launched_from));
}

/// Diagnoses saved settings without opening the UI or exposing sync credentials.
fn check_config() {
    match OverlayConfig::load() {
        Ok(config) => {
            println!("Settings loaded: {}", config::config_path().display());
            println!(
                "Relative: width {}, scale {}, {} ahead / {} behind",
                config.relative.width, config.relative.scale, config.relative.ahead_count, config.relative.behind_count
            );
            println!("Bound controls: {} / {}", config.binds.pairs().len(), input::Action::ALL.len());
            println!("Danger marks: {}", config.danger.len());
        }
        Err(error) => {
            eprintln!("Settings could not be loaded: {error:#}");
            std::process::exit(1);
        }
    }
}

/// Captures numeric scorer evidence without opening the overlay or writing to
/// iRacing. `--capture-seconds` is intentionally bounded so this diagnostic
/// cannot accidentally become an unbounded background recorder.
fn capture_scoring(path: &std::path::Path) {
    let seconds = match scoring_capture_seconds() {
        Ok(seconds) => seconds,
        Err(message) => {
            println!("error: {message}");
            return;
        }
    };
    println!("capturing scorer telemetry for {seconds}s into {}...", path.display());
    match telemetry::scoring_capture::capture(path, Duration::from_secs(seconds)) {
        Ok(report) => println!(
            "wrote {} numeric rows to {}; summary: {}",
            report.rows,
            report.data_path.display(),
            report.summary_path.display()
        ),
        Err(error) => println!("error: {error:#}"),
    }
}

fn scoring_capture_seconds() -> Result<u64, String> {
    let value = std::env::args()
        .find_map(|arg| arg.strip_prefix("--capture-seconds=").map(str::to_owned))
        .unwrap_or_else(|| "300".to_owned());
    let seconds =
        value.parse::<u64>().map_err(|_error| "--capture-seconds must be a whole number from 1 to 3600".to_owned())?;
    if !(1..=3600).contains(&seconds) {
        return Err("--capture-seconds must be from 1 to 3600".to_owned());
    }
    Ok(seconds)
}

/// Queues the one snapshot a demo run renders, in whatever states were asked
/// for. Returns whether it went; a failure here leaves nothing to draw.
///
/// Each `--demo-state=` name nudges the fixed snapshot into a state that only
/// exists part-way through a race — a caution, a box call, a spectator's seat
/// — so every one of them has a reproducible screenshot behind it. See
/// `demo::apply_state`.
fn send_demo_snapshot(tx: &mpsc::Sender<telemetry::snapshot::TelemetrySnapshot>, states: &[String]) -> bool {
    println!("demo mode: rendering the design-mockup snapshot; iRacing is not needed");
    let mut snapshot = demo::snapshot();
    for state in states {
        demo::apply_state(&mut snapshot, state);
    }
    if tx.send(snapshot).is_err() {
        println!("error: could not queue the demo snapshot");
        return false;
    }
    true
}

/// The `--demo-state=` names this run was given.
///
/// Repeatable and comma-separated, so `--demo-state=spectating,sync` and two
/// separate flags mean the same thing. Each name is a state the fixed demo
/// snapshot cannot otherwise be in — see `demo::apply_state`.
fn demo_states() -> Vec<String> {
    std::env::args()
        .filter_map(|arg| arg.strip_prefix("--demo-state=").map(str::to_owned))
        .flat_map(|arg| arg.split(',').map(|state| state.trim().to_owned()).collect::<Vec<_>>())
        .filter(|state| !state.is_empty())
        .collect()
}

/// Reattaches this process's standard streams to the console it was launched
/// from, if there is one.
///
/// A `windows_subsystem = "windows"` binary starts with no console, which is
/// the point — but it also means `println!` output vanishes when the tool is
/// run from a terminal, including the diagnostics from
/// `--dump-session-info`. Attaching to the parent console restores that
/// without ever creating a console window of its own.
fn attach_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    // SAFETY: `AttachConsole` takes only a process id and is safe to call
    // with no console attached; it fails harmlessly when the parent has none
    // (e.g. launched from Explorer), which is exactly the case we ignore.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// Connects to iRacing, waits for a session, and writes the raw
/// `session_info()` YAML to [`SESSION_INFO_DUMP_FILE`] beside the exe.
///
/// This is a one-off diagnostic: run `race-overlay.exe --dump-session-info`
/// while on track to capture real session-info YAML for confirming the
/// field names the rest of this crate's YAML parsing relies on.
fn dump_session_info() {
    println!("waiting up to {DUMP_WAIT:?} for iRacing (make sure you're in a session)...");
    match telemetry::session::dump_session_info(DUMP_WAIT) {
        Ok(yaml) => write_dump(SESSION_INFO_DUMP_FILE, &yaml),
        Err(err) => println!("error: {err:#}"),
    }
}

/// Reports which of the black box's candidate telemetry variables the current
/// car publishes, and what each currently reads.
///
/// Run this on track, in the car, in the class being built for: which
/// variables exist is a property of the car, not of the SDK, so the pages are
/// written against this rather than against assumption.
fn dump_vars() {
    println!("waiting up to {DUMP_WAIT:?} for iRacing (be on track, in the car, for meaningful values)...");
    match telemetry::session::dump_vars(DUMP_WAIT) {
        Ok(report) => {
            print!("{report}");
            write_dump(VARS_DUMP_FILE, &report);
        }
        Err(err) => println!("error: {err:#}"),
    }
}

/// The action name following `--bind`, if that's how this run was invoked.
fn bind_argument() -> Option<String> {
    let mut args = std::env::args().skip_while(|arg| arg != "--bind");
    args.next()?;
    args.next()
}

/// How long `--bind` waits for a button before giving up.
const BIND_WAIT: Duration = Duration::from_secs(30);

/// How often `--bind` samples the wheel while waiting.
///
/// Deliberately much faster than the display rate: a rotary encoder's detent
/// often reports as a pulse only a few tens of milliseconds long, and a
/// leisurely poll can step straight over one.
const BIND_POLL: Duration = Duration::from_millis(4);

/// Records whichever wheel button the user presses as the bind for `action`.
///
/// Button numbering differs between iRacing's own control list and what an
/// input library reports — iRacing resolves bindings through its own device
/// enumeration, so its "Btn 22" is not a number anything else can look up.
/// Asking for one press sidesteps that entirely and works on any device.
fn capture_bind(action_name: &str) {
    let Some(action) = input::Action::from_slug(action_name) else {
        println!("error: unknown action {action_name:?}");
        println!("       try one of: {}", input::Action::ALL.map(input::Action::slug).join(", "));
        return;
    };

    let mut devices = input::device::Devices::new();
    devices.pump(&[]);
    let names = devices.names();
    println!("detected: {}", if names.is_empty() { "keyboard only".to_owned() } else { names.join(", ") });
    println!("press the control for {:?} (waiting {BIND_WAIT:?})...", action.slug());

    // Whatever is already held when this starts — a shifter resting in gear,
    // a key still down from the last step — must not be captured as the
    // answer, so only a fresh press counts.
    let held_buttons: Vec<(String, u32)> = devices.pressed();
    let held_key = input::keys::pressed();
    let deadline = std::time::Instant::now() + BIND_WAIT;
    while std::time::Instant::now() < deadline {
        devices.pump(&[]);
        if let Some((device, button)) = devices.pressed().into_iter().find(|held| !held_buttons.contains(held)) {
            println!("bound {} to {device} button {button}", action.slug());
            save_bind(action, input::Bind::Button { device, button });
            return;
        }
        if let Some(vk) = input::keys::pressed().filter(|vk| Some(*vk) != held_key) {
            println!("bound {} to {}", action.slug(), input::keys::key_name(vk));
            save_bind(action, input::Bind::Key { vk });
            return;
        }
        std::thread::sleep(BIND_POLL);
    }
    println!("nothing pressed; leaving {} as it was", action.slug());
}

/// Writes one captured bind into `race-overlay.toml`, preserving everything else.
///
/// Goes through [`OverlayConfig::update`] so a running overlay's panel
/// positions, saved since this process loaded the file, survive the write —
/// and so this bind survives the overlay's next save.
fn save_bind(action: input::Action, bind: input::Bind) {
    match OverlayConfig::update(|config| config.binds.set(action, bind)) {
        Ok(()) => println!("saved to {}", config::config_path().display()),
        Err(err) => println!("error: could not save race-overlay.toml: {err:#}"),
    }
}

/// Runs the team-sync relay on its own — `--sync-host=<port>`.
///
/// The diagnostic face of the relay the overlay will embed in phase 2:
/// starts it, prints the invite code and the funnel command, and stays up
/// until closed. What a host runs today to try sync with a teammate before
/// any of it has a UI.
fn sync_host(spec: &str) {
    let Ok(port) = spec.trim().parse::<u16>() else {
        println!("error: --sync-host wants a port, e.g. --sync-host=41230");
        return;
    };
    let invite = match sync::relay::InviteCode::generate() {
        Ok(invite) => invite,
        Err(err) => {
            println!("error: {err:#}");
            return;
        }
    };
    let relay = match sync::relay::Relay::spawn(port, invite.clone()) {
        Ok(relay) => relay,
        Err(err) => {
            println!("error: {err:#}");
            return;
        }
    };
    println!("team sync relay on {}", relay.local_addr());
    println!("invite code: {invite}");
    println!();
    println!("to publish it for the team (once, from any terminal):");
    println!("    tailscale funnel {}", relay.local_addr().port());
    println!("teammates then join with the printed https URL as wss://, e.g.");
    println!("    race-overlay.exe --sync-join=wss://<machine>.<tailnet>.ts.net,<subsession>,{invite},<name>,<custid>");
    println!("or on this machine directly:");
    println!("    race-overlay.exe --sync-join=ws://{},<subsession>,{invite},<name>,<custid>", relay.local_addr());
    println!();
    println!("running until this window is closed...");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Joins a team-sync relay and talks — `--sync-join=<url>,<subsession>,<code>,<name>,<custid>`.
///
/// Prints everything the relay sends, and publishes one small burst of
/// made-up events on join so two of these pointed at the same relay can
/// watch each other's traffic arrive — including the backlog on whichever
/// joined second. The made-up numbers are obviously fake (lap 999); this
/// exercises the transport, phase 2 wires the real telemetry.
fn sync_join(spec: &str) {
    let parts: Vec<&str> = spec.split(',').collect();
    let [url, subsession, invite, name, cust_id] = parts[..] else {
        println!("error: --sync-join wants <url>,<subsession>,<invite>,<name>,<custid>");
        return;
    };
    let (Ok(subsession), Ok(cust_id)) = (subsession.trim().parse::<u64>(), cust_id.trim().parse::<u32>()) else {
        println!("error: subsession and custid must be numbers");
        return;
    };
    let member = sync::protocol::Member { cust_id, name: name.trim().to_owned() };
    // The diagnostic room is phase zero; normal overlays take SessionNum
    // from iRacing and never mix practice, qualifying and race histories.
    let client =
        sync::client::SyncClient::start(url.trim().to_owned(), subsession, 0, invite.trim().to_owned(), member);

    let burst = [
        sync::protocol::Event::StintBoundary { driver: name.trim().to_owned() },
        sync::protocol::Event::LapClosed { lap: 999, fuel_litres: 42.0, used_litres: 2.5 },
        sync::protocol::Event::PitStopObserved { car_idx: 7, stationary_secs: 31.5, took_tyres: true },
        sync::protocol::Event::OffTrack { car_idx: 7, tally: 1 },
        sync::protocol::Event::DriverScalars {
            car_idx: Some(7),
            fuel_litres: 42.0,
            service_fuel_litres: Some(30),
            tyres_armed: [true, true, false, false],
            tyre_pressures_kpa: [165.0, 165.0, 165.0, 165.0],
        },
    ];
    for (offset, event) in (0_u32..).zip(burst) {
        let outgoing = sync::client::Outgoing { session_time: f64::from(offset), event };
        if client.publish.send(outgoing).is_err() {
            println!("error: the sync thread is gone");
            return;
        }
    }

    println!("joined; waiting for traffic (close this window to leave)...");
    while let Ok(frame) = client.incoming.recv() {
        println!("{frame:?}");
    }
    println!("sync ended");
}

fn write_dump(file_name: &str, contents: &str) {
    let path = std::path::Path::new(file_name);
    match std::fs::write(path, contents) {
        Ok(()) => println!("wrote {} ({} bytes)", path.display(), contents.len()),
        Err(err) => println!("error: could not write {}: {err:#}", path.display()),
    }
}
