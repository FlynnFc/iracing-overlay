// Rust guideline compliant 2026-02-16

//! The overlay's [`EguiOverlay`] implementation: drains telemetry
//! snapshots, sizes the window to the primary monitor on the first frame,
//! draws the five independently draggable panels (Relative, Standings,
//! Radar Bars, Faster Class, Pit Stall), and toggles mouse
//! passthrough automatically based on whether the pointer is over interactive
//! content.

use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use egui_overlay::EguiOverlay;
use egui_overlay::egui_render_three_d::ThreeDBackend;
use egui_overlay::egui_window_glfw_passthrough::GlfwBackend;
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use crate::config::{BlackBoxConfig, OverlayConfig};
use crate::focus::FocusTracker;
use crate::input;
use crate::telemetry::pit::PitRequest;
use crate::telemetry::snapshot::TelemetrySnapshot;
use crate::tray::Tray;
use crate::ui::{blackbox, faster_class, pit_stall, radar_bars, settings, standings};

/// Fallback size if GLFW can't report a primary monitor (should not happen
/// in practice, but a blank/tiny window would be a confusing failure mode).
const FALLBACK_MONITOR_SIZE: [f32; 2] = [1920.0, 1080.0];

/// How often the settings file is checked for changes made by another
/// process — in practice, `race-overlay.exe --bind <action>`.
///
/// One `metadata` call on one small file: loose enough to cost nothing at
/// display rate, tight enough that a new bind works before the driver has
/// finished putting the wheel down.
const CONFIG_POLL: Duration = Duration::from_secs(2);

/// How long the black box's own settings must sit unchanged before they are
/// written to disk.
///
/// The fuel margin steps half a lap per rotary click, so setting three laps
/// is six presses inside a second or two. Without a delay each one would be
/// its own read-modify-write of the settings file.
const SETTINGS_WRITE_DELAY: Duration = Duration::from_secs(1);

/// How long the car must have been in the garage before the panels go.
///
/// Hiding waits, showing does not — see [`GarageHide`]. A quarter-second is
/// below noticing on the way into the garage, and asymmetry means the delay
/// can never cost the driver an overlay on the way out of the box.
const GARAGE_HIDE_DELAY: Duration = Duration::from_millis(250);

/// How long to wait between frames while the panels are on screen.
///
/// The telemetry thread publishes at iRacing's own 60 Hz, so a frame drawn any
/// sooner than this can only redraw numbers that have not changed. Left
/// unpaced — which is what an unconditional `request_repaint` does, since egui
/// then reports a repaint delay of zero and the windowing loop's
/// `wait_events_timeout(0)` returns immediately — the overlay renders and swaps
/// a full-screen transparent surface as fast as the GPU will take it, which
/// measured at 26% of a core with every panel hidden and nothing to draw.
///
/// Neither `egui_overlay` nor the GLFW backend beneath it ever calls
/// `glfwSwapInterval`, and GLFW defaults that to zero, so there is no vsync to
/// fall back on: the pacing has to come from here.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// How long to wait between frames while nothing is drawn.
///
/// `should_show` being false means the frame paints nothing at all, so the only
/// reason to wake at any particular rate is to notice when it becomes true
/// again. A quarter-second is far below noticing on the way back into the sim,
/// and it is a ceiling rather than a floor: GLFW waits on the thread's message
/// queue, so a tray click, a mouse move or any other window message wakes the
/// loop immediately regardless of this.
const IDLE_INTERVAL: Duration = Duration::from_millis(250);

/// How many frames to draw before `--screenshot` reads the framebuffer.
///
/// The SVG icons and manufacturer marks are decoded by `egui_extras`' image
/// loaders on a background thread, and a widget asking for one gets nothing
/// on the frames before it arrives. A second at the paced frame rate is far
/// more than the loaders need and still quick enough to sit through.
const SCREENSHOT_AFTER_FRAMES: u32 = 60;

/// How old the newest snapshot may be before its garage flag is ignored.
///
/// The telemetry thread returns on `SessionExpired` and sends nothing further,
/// while this side keeps the last snapshot it received. Leaving a session from
/// the garage would otherwise leave `in_garage` true forever, and the panels
/// hidden until the overlay was restarted. Four times the telemetry thread's
/// own 500 ms poll, so a live session cannot trip it.
const SNAPSHOT_STALE: Duration = Duration::from_secs(2);

/// The switches only a `--demo` or `--screenshot` run sets.
///
/// Grouped rather than passed one by one, because they are one idea — "this
/// run is looking at the widgets rather than driving" — and because
/// [`OverlayApp::new`] had collected enough of them to stop reading as a
/// parameter list.
#[derive(Debug, Default)]
pub struct DemoOptions {
    /// Draw the panels regardless of whether iRacing has focus, and never
    /// persist what this run changes — see [`OverlayApp::persist`].
    pub enabled: bool,
    /// Open the black box on this page, since every page but the first is
    /// reached by a wheel button that demo mode cannot press.
    pub page: Option<blackbox::Page>,
    /// Open a settings page for reproducible previews, only when `enabled`.
    /// Accepted names are defined by `SettingsWindow::open_demo_page`.
    pub settings_page: Option<String>,
    /// Write one rendered frame here, then quit — see
    /// [`OverlayApp::take_screenshot`].
    pub screenshot: Option<std::path::PathBuf>,
    /// The `--demo-state=` names this run was given. Most are applied to the
    /// snapshot by `demo::apply_state`; `sync` is handled here instead,
    /// because a synced team car lives in the store rather than the snapshot.
    pub states: Vec<String>,
}

/// Holds the panels on screen for [`GARAGE_HIDE_DELAY`] after the car enters
/// the garage, and drops them the instant it leaves.
///
/// The delay exists so a single chattering tick across the garage/track
/// transition cannot flicker every panel. Its state is one instant, so it is
/// kept out of [`OverlayApp`] itself and tested on its own.
#[derive(Debug, Default)]
struct GarageHide {
    /// When the car was first seen in the garage in this uninterrupted run of
    /// garage ticks; `None` whenever it isn't.
    since: Option<Instant>,
}

impl GarageHide {
    /// Advances by one frame and reports whether the panels should be hidden.
    fn update(&mut self, in_garage: bool, now: Instant) -> bool {
        if !in_garage {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.duration_since(since) >= GARAGE_HIDE_DELAY
    }
}

/// Which of the two saved panel arrangements is in force.
///
/// The panels keep one position for driving and another for watching,
/// resolved from the snapshot's seat every frame — see
/// `plans/seat-layouts.md` and `RelativeConfig::watch_pos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Layout {
    /// Panels where they keep out of the mirrors: the player is driving.
    /// Also the answer before any snapshot has said otherwise, and demo and
    /// layout mode's.
    #[default]
    Driving,
    /// The screen is being read like a broadcast: spectating, or watching a
    /// team-mate's stint.
    Watching,
}

/// The layout `seat` asks for, given the one in force.
///
/// Only a definite seat moves the panels. `OutOfCar` covers the garage, a
/// tow and the wait between stints — transitions, not places — so it keeps
/// the current layout rather than snapping every panel across the screen
/// mid-tow. `None` (no snapshot yet, or a session gone quiet) keeps it for
/// the same reason.
fn layout_for_seat(seat: Option<&crate::telemetry::snapshot::Seat>, current: Layout) -> Layout {
    use crate::telemetry::snapshot::Seat;
    match seat {
        Some(Seat::Driving) => Layout::Driving,
        Some(Seat::Spectating(_) | Seat::TeamMate(_)) => Layout::Watching,
        Some(Seat::OutOfCar) | None => current,
    }
}

/// Litres-per-lap the shared fuel target steps by, and its ceiling — a
/// generous cap no real save target reaches, just a guard on the stepper.
const FUEL_TARGET_STEP: f32 = 0.05;
const FUEL_TARGET_MAX: f32 = 20.0;

/// Percentage points the tyre-policy wear threshold steps by per turn, and
/// the range it is held to — below 5% every set qualifies and above 95%
/// nothing does, so the ends carry no meaning worth reaching.
const TYRE_WEAR_STEP_PCT: u8 = 5;
const TYRE_WEAR_MIN_PCT: u8 = 5;
const TYRE_WEAR_MAX_PCT: u8 = 95;

/// Whether a black-box pit request should travel the sync wire to the seated
/// driver's overlay, rather than go straight to the local sim.
///
/// True only from a spectator seat — the crew chief adjusting the team car.
/// From any other seat the request is the player's own and goes local. The
/// sim only accepts pit commands from the seated client, so this keeps a
/// spectator's intent a relay, never a direct command.
fn pit_request_routes_over_wire(seat: Option<&crate::telemetry::snapshot::Seat>) -> bool {
    matches!(seat, Some(crate::telemetry::snapshot::Seat::Spectating(_)))
}

/// Whether a received team pit write may be applied on this side.
///
/// Two gates, both required: the driver has armed consent for the session,
/// and this overlay is the one in the car — the only one whose commands the
/// sim will accept. A write failing either is dropped, not queued, so it can
/// never spring later.
fn team_write_applies(consent: bool, seat: Option<&crate::telemetry::snapshot::Seat>) -> bool {
    consent && matches!(seat, Some(crate::telemetry::snapshot::Seat::Driving))
}

/// The position slot the active layout reads and a drag writes.
///
/// Watching seeds from the driving position the first time it is looked at,
/// so the layouts only diverge once a panel is actually dragged while
/// watching — and an untouched config keeps serialising without the key.
fn active_pos<'a>(layout: Layout, pos: &'a mut [f32; 2], watch_pos: &'a mut Option<[f32; 2]>) -> &'a mut [f32; 2] {
    match layout {
        Layout::Driving => pos,
        Layout::Watching => watch_pos.get_or_insert(*pos),
    }
}

/// The overlay's [`EguiOverlay`] implementation.
pub struct OverlayApp {
    rx: Receiver<TelemetrySnapshot>,
    latest: Option<TelemetrySnapshot>,
    /// When `latest` arrived, so a session that has stopped sending cannot
    /// leave a stale garage flag standing — see [`SNAPSHOT_STALE`].
    latest_at: Option<Instant>,
    /// Debounces the garage rule; see [`GarageHide`].
    garage_hide: GarageHide,
    config: OverlayConfig,
    /// Whether the window has been sized to the monitor yet.
    sized: bool,
    /// Draw regardless of whether iRacing has focus, without touching the
    /// saved setting — see [`OverlayApp::new`].
    demo: bool,
    /// The notification-area icon, and the only way to quit the overlay now
    /// that it keeps out of the taskbar. `None` if it couldn't be
    /// registered.
    tray: Option<Tray>,
    focus_tracker: FocusTracker,
    /// The black box's page, cursor and scroll.
    black_box: blackbox::BlackBox,
    preview_black_box: blackbox::BlackBox,
    preview_blackbox_page: Option<blackbox::Page>,
    /// Wheel and keyboard binds, polled every frame.
    actions: input::Actions,
    /// Queued pit intents. Sent from the telemetry thread, which owns the
    /// `Session` — it is `!Send`, so it can't come here.
    pit_requests: Sender<PitRequest>,
    /// Layout mode: draw every widget, on the mockup snapshot, wherever it
    /// sits — see [`OverlayApp::layout_mode`].
    layout_mode: bool,
    /// The settings file's modification time when its binds were last read,
    /// so `--bind` takes effect without a restart — see
    /// [`OverlayApp::reload_binds_if_changed`].
    binds_read_at: Option<SystemTime>,
    /// When to next look at that modification time.
    next_config_check: Instant,
    /// The black box settings as of the last frame, so a run of rotary clicks
    /// becomes one write — see [`OverlayApp::save_blackbox_if_settled`].
    settling_blackbox: BlackBoxConfig,
    /// When they last changed, or `None` once they have been written.
    blackbox_changed_at: Option<Instant>,
    /// The settings window's state — see `ui::settings`.
    settings: settings::SettingsWindow,
    /// When the settings window last changed something, so the file is
    /// written once the values settle rather than on every slider frame.
    settings_changed_at: Option<Instant>,
    /// Clicks on the black box's controls, collected while it is drawn and
    /// applied with the wheel's actions — see `blackbox::Click`.
    pending_clicks: Vec<blackbox::Click>,
    /// The window's last reported compositing state, so a change is printed
    /// once — see [`WindowState`].
    window_state: Option<WindowState>,
    /// The last window other than ours seen holding foreground, to hand it
    /// back to — see [`OverlayApp::report_window_state`].
    last_other_foreground: Option<HWND>,
    /// The earliest the next frame may be drawn; see [`OverlayApp::run`].
    next_frame_at: Option<Instant>,
    /// The gap [`OverlayApp::gui_run`] last asked to be left before the next
    /// frame — [`FRAME_INTERVAL`] or [`IDLE_INTERVAL`]. Held here because
    /// [`OverlayApp::run`] is what enforces it.
    frame_interval: Duration,
    /// The cursor the last drawn frame asked for, so a skipped frame can ask
    /// for the same one rather than resetting it — see [`OverlayApp::run`].
    last_cursor: egui::CursorIcon,
    /// The window that had foreground before ours existed, to hand it back
    /// to on the first frame — see [`crate::focus::yield_foreground`].
    launched_from: Option<HWND>,
    /// Where to write one rendered frame before quitting, if `--screenshot`
    /// asked for one — see [`OverlayApp::take_screenshot`].
    screenshot: Option<std::path::PathBuf>,
    /// Frames drawn so far, so the screenshot waits for the ones that are
    /// still loading their assets.
    frames_drawn: u32,
    /// The Faster Class widget's level, held across frames so its thresholds
    /// can have hysteresis — see `telemetry::faster_class::Alarm`.
    faster_class_alarm: crate::telemetry::faster_class::Alarm,
    /// Keys typed into the settings window's number fields, polled because
    /// the window can never hold keyboard focus — see `input::typed`.
    typed: input::typed::TypedKeys,
    /// The stream-mode value the window styles were last applied with, so
    /// ticking it in the settings takes effect without a restart — `None`
    /// until the window exists. See [`apply_window_styles`].
    stream_mode_applied: Option<bool>,
    /// Which panel arrangement is in force — see [`Layout`] and
    /// [`layout_for_seat`].
    active_layout: Layout,
    /// Team sync: the connection, the event producer, and the store a
    /// spectator's pages read — see `crate::sync::runtime`.
    team_sync: crate::sync::runtime::TeamSync,
    /// The embedded team-sync relay while this member hosts. `Relay` has no
    /// shutdown — once spawned it serves until the process exits — so this
    /// is spawned at most once per run; see [`OverlayApp::ensure_relay`].
    hosted_relay: Option<crate::sync::relay::Relay>,
    /// Why the relay isn't running when hosting is on, for the settings page.
    host_error: Option<String>,
    /// The `(port, invite)` of the last failed spawn, so a taken port or a
    /// mangled invite isn't retried sixty times a second — only when the
    /// inputs change.
    host_failed_with: Option<(u16, String)>,
}

impl OverlayApp {
    /// Creates the overlay.
    ///
    /// `demo` forces the panels to stay visible while iRacing isn't focused,
    /// since the point of demo mode is to compare them against the mockup
    /// images in another window. It's kept as a field rather than applied by
    /// overwriting `config.only_show_when_iracing_focused`, because dragging
    /// a panel persists the config — which would silently write demo mode's
    /// override into the user's real settings.
    ///
    /// `demo_page` opens the black box on a chosen page, since every page but
    /// the first is reached by a wheel button that demo mode cannot press.
    ///
    /// `launched_from` is the window that had foreground before the overlay's
    /// existed; see [`crate::focus::yield_foreground`].
    #[must_use]
    pub fn new(
        rx: Receiver<TelemetrySnapshot>,
        pit_requests: Sender<PitRequest>,
        config: OverlayConfig,
        demo: DemoOptions,
        tray: Option<Tray>,
        launched_from: Option<HWND>,
    ) -> Self {
        let DemoOptions { enabled: demo, page: demo_page, settings_page, screenshot, states } = demo;
        let mut settings = settings::SettingsWindow::default();
        if demo
            && let Some(page) = settings_page
            && !settings.open_demo_page(&page)
        {
            println!("note: --demo-settings={page} is not a settings page; keeping settings closed");
        }
        // A demo run asked to show the team-sync surfaces starts with a
        // ledger already folded in — see `crate::sync::runtime::TeamSync::demo_seed`.
        let mut team_sync = crate::sync::runtime::TeamSync::default();
        if demo && states.iter().any(|state| state == "sync") {
            team_sync.demo_seed(&crate::demo::sync_events());
        }
        let settling_blackbox = config.blackbox.clone();
        Self {
            rx,
            latest: None,
            latest_at: None,
            garage_hide: GarageHide::default(),
            config,
            sized: false,
            demo,
            tray,
            focus_tracker: FocusTracker::new(),
            black_box: demo_page.map_or_else(blackbox::BlackBox::new, blackbox::BlackBox::showing),
            preview_black_box: blackbox::BlackBox::new(),
            preview_blackbox_page: None,
            actions: input::Actions::new(),
            pit_requests,
            layout_mode: false,
            binds_read_at: settings_modified_at(),
            next_config_check: Instant::now() + CONFIG_POLL,
            settling_blackbox,
            blackbox_changed_at: None,
            settings,
            settings_changed_at: None,
            pending_clicks: Vec::new(),
            window_state: None,
            last_other_foreground: None,
            next_frame_at: None,
            frame_interval: FRAME_INTERVAL,
            last_cursor: egui::CursorIcon::Default,
            launched_from,
            screenshot,
            frames_drawn: 0,
            faster_class_alarm: crate::telemetry::faster_class::Alarm::default(),
            typed: input::typed::TypedKeys::default(),
            stream_mode_applied: None,
            active_layout: Layout::default(),
            team_sync,
            hosted_relay: None,
            host_error: None,
            host_failed_with: None,
        }
    }

    /// Persists the settings this process owns — unless this is a demo run.
    ///
    /// Demo mode exists to hold the widgets up against the mockups, and doing
    /// that involves dragging them somewhere convenient. Every drag used to be
    /// written straight to the real settings file, so an afternoon spent
    /// looking at panels rearranged the panels a driver had spent a race
    /// positioning — and the same is true of the black box's own settings.
    /// Demo mode reads the real file and changes nothing in it.
    fn persist(&self) {
        if self.demo {
            return;
        }
        save_config(&self.config);
    }

    /// Re-reads the binds when the settings file changes underneath us.
    ///
    /// `race-overlay.exe --bind <action>` writes that file while this is
    /// running, and a bind that needs the overlay restarted before it works
    /// is a bind that looks broken — which is exactly how it looked.
    ///
    /// Only the binds are taken. Everything else in the file is owned by this
    /// process, which would be replacing its own live state with an older
    /// copy of it.
    fn reload_binds_if_changed(&mut self, now: Instant) {
        if now < self.next_config_check {
            return;
        }
        self.next_config_check = now + CONFIG_POLL;

        let modified = settings_modified_at();
        if modified.is_none() || modified == self.binds_read_at {
            return;
        }
        self.binds_read_at = modified;
        match OverlayConfig::load() {
            Ok(config) => self.config.binds = config.binds,
            Err(err) => println!("note: {err:#}; keeping the binds already loaded"),
        }
    }

    /// Writes the black box's own settings once they stop changing.
    ///
    /// Auto Fuel and its margin are set from the wheel rather than by
    /// dragging a panel, so they used to reach the disk only if a panel
    /// happened to be dragged afterwards: switch Auto Fuel on, restart the
    /// PC, and it was off again.
    fn save_blackbox_if_settled(&mut self, now: Instant) {
        if self.config.blackbox != self.settling_blackbox {
            self.settling_blackbox = self.config.blackbox.clone();
            self.blackbox_changed_at = Some(now);
            return;
        }
        let Some(changed_at) = self.blackbox_changed_at else {
            return;
        };
        if now.duration_since(changed_at) < SETTINGS_WRITE_DELAY {
            return;
        }
        self.blackbox_changed_at = None;
        self.persist();
    }

    /// Draws the settings window when it is open, and writes what it changed
    /// once the values have settled.
    ///
    /// A slider drag changes the config on every frame it moves; writing each
    /// of those would be sixty writes a second for one gesture. The same
    /// delay the black box's own settings use covers a drag comfortably.
    /// Spawns the embedded team-sync relay when the config asks this overlay
    /// to host — the settings-page face of `--sync-host`.
    ///
    /// The invite is the config's own: enabling hosting with none saved
    /// generates one and persists it, so the code the settings page shows is
    /// the code the relay verifies and the one this member's own client
    /// presents. A spawned relay serves until the process exits (`Relay` has
    /// no shutdown), so hosting toggled off and back on reuses it, and
    /// moving to another port takes a restart — the page says so.
    fn ensure_relay(&mut self) {
        use crate::sync::relay::{InviteCode, Relay};
        if !(self.config.sync.enabled && self.config.sync.host) || self.hosted_relay.is_some() {
            return;
        }
        let attempt = (self.config.sync.host_port, self.config.sync.invite.clone());
        if self.host_failed_with.as_ref() == Some(&attempt) {
            return;
        }
        let invite = if self.config.sync.invite.trim().is_empty() {
            match InviteCode::generate() {
                Ok(invite) => {
                    self.config.sync.invite = invite.to_string();
                    self.persist();
                    Ok(invite)
                }
                Err(err) => Err(format!("{err:#}")),
            }
        } else {
            InviteCode::parse(&self.config.sync.invite).map_err(|err| format!("{err:#}"))
        };
        (self.host_error, self.host_failed_with) = match invite
            .and_then(|invite| Relay::spawn(self.config.sync.host_port, invite).map_err(|err| format!("{err:#}")))
        {
            Ok(relay) => {
                println!("note: team sync relay hosting on {}", relay.local_addr());
                self.hosted_relay = Some(relay);
                (None, None)
            }
            // Keyed on the config as it stands now, not as it was: generating
            // an invite just changed it, and that new pair deserves its own try.
            Err(error) => (Some(error), Some((self.config.sync.host_port, self.config.sync.invite.clone()))),
        };
    }

    /// The sync config the client actually connects with: while hosting, the
    /// relay to join is this overlay's own on localhost, whatever the URL
    /// field says — the URL field is for teammates, who reach the same relay
    /// through the funnel.
    fn effective_sync_config(&self) -> crate::config::SyncConfig {
        let mut config = self.config.sync.clone();
        if config.host
            && let Some(relay) = &self.hosted_relay
        {
            config.relay_url = format!("ws://{}", relay.local_addr());
        }
        config
    }

    /// What the Team Sync settings page says about the embedded relay.
    fn host_status(&self) -> settings::HostStatus {
        if let Some(error) = &self.host_error {
            return settings::HostStatus::Failed { error: error.clone() };
        }
        match &self.hosted_relay {
            Some(relay) => settings::HostStatus::Running { port: relay.local_addr().port() },
            None => settings::HostStatus::Off,
        }
    }

    fn draw_settings(&mut self, egui_context: &egui::Context, now: Instant) {
        let watching = self.active_layout == Layout::Watching;
        let host = self.host_status();
        let outcome =
            settings::draw(egui_context, &mut self.config, &mut self.settings, &self.actions, now, watching, &host);
        if outcome.changed {
            self.settings_changed_at = Some(now);
        }
        // egui owns each panel's position once it has been seen, and only
        // reads `default_pos` for an area it has no memory of — so putting
        // the config back to its defaults takes only once that memory is
        // gone. Next frame every panel seeds from the config again.
        if outcome.reset_positions {
            egui_context.memory_mut(egui::Memory::reset_areas);
        }
        if let Some(changed_at) = self.settings_changed_at
            && now.duration_since(changed_at) >= SETTINGS_WRITE_DELAY
        {
            self.settings_changed_at = None;
            // The launcher's list is its own file, and not the demo's
            // business either way, so it is written regardless.
            self.settings.launcher.save_if_dirty();
            self.persist();
        }
    }

    /// Swaps the panel arrangement when the seat asks for the other one.
    ///
    /// On a switch, egui's memory of where every panel sits is dropped so
    /// each one re-seeds from the newly active slot next frame — the same
    /// mechanism Reset all positions uses; `default_pos` is only read for an
    /// area egui has no memory of. See [`layout_for_seat`].
    fn settle_layout(&mut self, egui_context: &egui::Context) {
        let seat_layout = layout_for_seat(self.latest.as_ref().map(|snapshot| &snapshot.seat), self.active_layout);
        if seat_layout != self.active_layout {
            self.active_layout = seat_layout;
            egui_context.memory_mut(egui::Memory::reset_areas);
        }
    }

    /// Re-applies the window styles the frame the Stream mode tick changes.
    ///
    /// It has to take effect while the settings window is still open — the
    /// point of ticking it is watching the window appear in OBS's picker.
    fn apply_stream_mode(&mut self, glfw_backend: &GlfwBackend) {
        if self.stream_mode_applied != Some(self.config.stream_mode) {
            apply_window_styles(glfw_backend, self.config.stream_mode);
            self.stream_mode_applied = Some(self.config.stream_mode);
        }
    }

    /// Whether every widget should be drawn on stand-in data right now.
    ///
    /// Toggled from the tray. Positioning a widget otherwise means waiting for
    /// the situation that makes it appear: the radar draws nothing until a car
    /// is alongside, and a widget switched off in the config draws nothing at
    /// all, so neither can be dragged where you want it. Layout mode ignores
    /// both conditions and feeds every widget the design-mockup snapshot, so
    /// there is always something on screen to take hold of.
    ///
    /// Deliberately not persisted: it is a thing you turn on for a minute in
    /// the garage, and a saved layout mode would mean a first race spent
    /// looking at a radar full of cars that are not there.
    #[must_use]
    const fn layout_mode(&self) -> bool {
        self.layout_mode
    }

    /// Draws every panel that should be on screen this frame.
    ///
    /// Split out of `gui_run` so that adding a widget is one block here rather
    /// than another few lines on an already long frame function.
    /// Applies crew-chief pit writes on the driver's side, behind consent.
    ///
    /// A spectator's adjustment becomes a local [`PitRequest`] here — the same
    /// channel the wheel feeds — so the sim only ever takes commands from the
    /// seated client, and this is a relay of intent, not remote control. Each
    /// applied change is announced rather than prompted: a driver mid-corner
    /// cannot answer dialogs. Writes are dropped, not queued, when consent is
    /// off or the player is not the one who can arm.
    fn apply_team_writes(&mut self) {
        let writes = self.team_sync.take_pit_writes();
        if writes.is_empty() {
            return;
        }
        if !team_write_applies(self.config.sync.allow_team_pit_control, self.latest.as_ref().map(|snap| &snap.seat)) {
            return;
        }
        for (requester, request) in writes {
            if self.pit_requests.send(request).is_ok() {
                println!("note: team pit control — {request:?} set by {requester}");
            }
        }
    }

    /// The shared fuel-target readout for the Relative footer, if a spec has
    /// set a target.
    ///
    /// Reads the target off the sync ledger and the current burn/fuel/lap off
    /// the snapshot — which is the driver's own while driving and the synced
    /// team car's while spectating (the black box's adjusted snapshot), so the
    /// driver chasing the number and the crew watching them chase it see the
    /// same thing. See `crate::ui::FuelTargetReadout`.
    fn fuel_target_readout(&self, snapshot: Option<&TelemetrySnapshot>) -> Option<crate::ui::FuelTargetReadout> {
        let target_lpl = self.team_sync.fuel_target()?;
        let snapshot = snapshot?;
        let current_lpl = snapshot.pit_service.fuel_per_lap_litres.filter(|burn| *burn > 0.0);
        let lap = u16::try_from(snapshot.relative_meta.current_lap.max(0)).unwrap_or(0);
        let hit_lap = current_lpl.and_then(|burn| {
            crate::telemetry::endurance::projected_dry_lap(lap, snapshot.pit_service.fuel_level_litres, burn)
        });
        Some(crate::ui::FuelTargetReadout { target_lpl, current_lpl, hit_lap })
    }

    /// The value the Strategy page's fuel-target stepper shows, or `None` to
    /// hide it — a spec sets the target, so it appears only while spectating.
    ///
    /// Seeded at the current target, or the measured burn if none is set yet,
    /// so the first nudge starts from where the car actually is.
    fn fuel_target_control(&self, snapshot: Option<&TelemetrySnapshot>) -> Option<f32> {
        let snapshot = snapshot?;
        if !matches!(snapshot.seat, crate::telemetry::snapshot::Seat::Spectating(_)) {
            return None;
        }
        Some(self.team_sync.fuel_target().or(snapshot.pit_service.fuel_per_lap_litres).unwrap_or(0.0))
    }

    /// The spec-side sync controls the Strategy page shows — the fuel-target
    /// stepper and the standing tyre-policy control, both spectating-only.
    fn sync_controls(&self, snapshot: Option<&TelemetrySnapshot>) -> blackbox::SyncControls {
        let spectating =
            snapshot.is_some_and(|snap| matches!(snap.seat, crate::telemetry::snapshot::Seat::Spectating(_)));
        blackbox::SyncControls {
            fuel_target: self.fuel_target_control(snapshot),
            tyre_policy: spectating.then(|| {
                self.team_sync
                    .tyre_policy()
                    .map_or(blackbox::TyreDirective::DriversCall, |(policy, _)| blackbox::TyreDirective::Set(policy))
            }),
        }
    }

    /// Cycles or adjusts the standing tyre directive from a Strategy-page
    /// action, published to team sync.
    ///
    /// A press walks the policies — off, every stop, never, wear-threshold —
    /// and a turn moves the threshold by [`TYRE_WEAR_STEP_PCT`] while the
    /// wear policy is the one standing; turns elsewhere do nothing rather
    /// than publish a no-op.
    fn adjust_tyre_policy(&self, action: input::Action) {
        use crate::sync::protocol::TyrePolicy;
        let current = self.team_sync.tyre_policy().map(|(policy, _)| policy);
        let next = match action {
            input::Action::Toggle => match current {
                None => Some(TyrePolicy::EveryStop),
                Some(TyrePolicy::EveryStop) => Some(TyrePolicy::Never),
                Some(TyrePolicy::Never) => {
                    Some(TyrePolicy::BelowWear { threshold_pct: TyrePolicy::DEFAULT_WEAR_THRESHOLD_PCT })
                }
                Some(TyrePolicy::BelowWear { .. }) => None,
            },
            input::Action::Increment | input::Action::Decrement => {
                let Some(TyrePolicy::BelowWear { threshold_pct }) = current else { return };
                let stepped = if action == input::Action::Increment {
                    threshold_pct.saturating_add(TYRE_WEAR_STEP_PCT)
                } else {
                    threshold_pct.saturating_sub(TYRE_WEAR_STEP_PCT)
                };
                let clamped = stepped.clamp(TYRE_WEAR_MIN_PCT, TYRE_WEAR_MAX_PCT);
                if clamped == threshold_pct {
                    return;
                }
                Some(TyrePolicy::BelowWear { threshold_pct: clamped })
            }
            _ => return,
        };
        self.team_sync.set_tyre_policy(next);
    }

    /// Steps or clears the shared fuel target from a Strategy-page action.
    ///
    /// A turn moves it by [`FUEL_TARGET_STEP`] from the current target (or the
    /// measured burn, if none is set yet); a press clears it. Published to team
    /// sync, where the driver's header picks it up.
    fn adjust_fuel_target(&self, action: input::Action, seed_burn: Option<f32>) {
        let current = self.team_sync.fuel_target().or(seed_burn).unwrap_or(0.0);
        let next = match action {
            input::Action::Increment => Some((current + FUEL_TARGET_STEP).min(FUEL_TARGET_MAX)),
            input::Action::Decrement => Some((current - FUEL_TARGET_STEP).max(0.0)),
            // A press clears the target — back to the driver's own judgement.
            input::Action::Toggle => None,
            _ => return,
        };
        self.team_sync.set_fuel_target(next.filter(|litres| *litres > 0.0));
    }

    /// Sets or clears a driver's danger mark and persists it.
    ///
    /// Written straight to disk because a mark is set once and wanted in every
    /// future session — see `plans/danger-drivers.md`.
    fn apply_danger_mark(&mut self, cust_id: u32, level: Option<crate::config::DangerLevel>) {
        match level {
            Some(level) => {
                self.config.danger.insert(cust_id, level);
            }
            None => {
                self.config.danger.remove(&cust_id);
            }
        }
        self.persist();
    }

    /// The team car's synced fuel while spectating, else `None`.
    ///
    /// The one place the "watching, and the ledger has data" test lives, so
    /// the Fuel page's availability and its contents can't disagree. Owned,
    /// so a caller can hold it across the `self.black_box` borrow.
    fn spectator_synced(&self) -> Option<crate::sync::store::SyncedCar> {
        self.latest
            .as_ref()
            .filter(|snap| matches!(snap.seat, crate::telemetry::snapshot::Seat::Spectating(_)))
            .and_then(|_| self.team_sync.synced_car())
    }

    /// The snapshot the black box should read while spectating: a clone with
    /// the team car's synced fuel written over this sim's empty pit service.
    ///
    /// This is what lets the ordinary Fuel page and its request-builder work
    /// unchanged from a spectator seat — the page reads `pit_service`, so the
    /// override makes its steppers count from the driver's real armed load
    /// rather than from an empty tank. `None` unless watching with synced
    /// data, where the real snapshot is used as-is.
    fn synced_blackbox_snapshot(&self) -> Option<TelemetrySnapshot> {
        let synced = self.spectator_synced()?;
        let mut snapshot = self.latest.as_ref()?.clone();
        // Tyre life first, before the pit-service borrow below.
        if let Some(tyres) = synced.tyres {
            snapshot.tyres = tyres;
        }
        // The BOX BOX border's fuel call, recomputed from the driver's real
        // tank and burn so a crew chief sees it fire on the driver's fuel, not
        // on this sim's empty one. The lap fraction is the followed car's.
        snapshot.box_this_lap = snapshot.fuel_use.lap_fraction.is_some_and(|fraction| {
            crate::telemetry::endurance::box_this_lap(synced.fuel_litres, synced.burn_per_lap, fraction)
        });
        let service = &mut snapshot.pit_service;
        service.fuel_level_litres = synced.fuel_litres;
        service.fuel_per_lap_litres = synced.burn_per_lap;
        service.fuel_amount_litres = synced.service_fuel_litres.map_or(0.0, f32::from);
        service.fuel_armed = synced.service_fuel_litres.is_some();
        // The armed tyre state, so the spectator's Tyres controls step from
        // the driver's real ticks and pressures rather than from zero.
        service.tyres_armed = synced.tyres_armed;
        service.tyre_pressures_kpa = synced.tyre_pressures_kpa;
        Some(snapshot)
    }

    /// Routes a pit request from the black box.
    ///
    /// To the sim when driving — the same channel the wheel feeds. From a
    /// spectator seat it travels the sync wire to the seated driver's overlay
    /// instead, which applies it behind that driver's consent: the sim only
    /// takes commands from the seated client, so this is a relay of intent.
    fn dispatch_pit_request(&self, request: crate::telemetry::pit::PitRequest) {
        if pit_request_routes_over_wire(self.latest.as_ref().map(|snap| &snap.seat)) {
            self.team_sync.send_pit_write(request);
        } else {
            // A full queue means the telemetry thread has stopped; there is
            // nothing useful to do about it here.
            let _ = self.pit_requests.send(request);
        }
    }

    #[expect(clippy::too_many_lines, reason = "each overlay draws with its own config and saved position")]
    fn draw_panels(&mut self, egui_context: &egui::Context) -> bool {
        // Layout mode substitutes the mockup snapshot for whatever the sim
        // is saying, so every widget has something to draw while it is
        // being positioned. Built per frame rather than cached: this runs
        // only while the menu item is ticked, and never during a race.
        let selected_preview = self.settings.preview_panel();
        let preview_page = self.settings.preview_blackbox_page();
        if preview_page != self.preview_blackbox_page {
            if let Some(page) = preview_page {
                self.preview_black_box = blackbox::BlackBox::showing(page);
            }
            self.preview_blackbox_page = preview_page;
        }
        let preview = (self.layout_mode() || selected_preview.is_some()).then(crate::demo::snapshot);
        // While spectating, the black box (and only it) reads the team car's
        // synced fuel over this sim's empty tank — the override touches only
        // the fuel fields no other panel reads, so it is safe to share. Owned,
        // so it outlives the `self.black_box` borrow below.
        let bb_override = self.synced_blackbox_snapshot();
        let latest = preview.as_ref().or(bb_override.as_ref()).or(self.latest.as_ref());
        let layout_mode = self.layout_mode();
        let synced = self.spectator_synced();
        // Which position slot every drag below reads and writes; resolved
        // once per frame in `gui_run`. Layout mode inherits whatever seat the
        // real session put in force, so panels can be arranged for watching
        // by turning it on while actually watching.
        let seat_layout = self.active_layout;
        // Cloned so each closure below captures an independent value
        // instead of re-borrowing `self.config`, which is already
        // mutably borrowed (its `.pos` field) by the same
        // `draggable_panel` call.
        let relative_config = self.config.relative.clone();
        // Cloned so the draw closure can read it without borrowing `self.config`
        // alongside the mutable panel-position borrows below; the map is a
        // handful of entries.
        let danger = self.config.danger.clone();
        let standings_config = self.config.standings.clone();
        let radar_config = self.config.radar.clone();
        let pit_stall_config = self.config.pit_stall.clone();
        let faster_class_config = self.config.faster_class.clone();

        // Every panel is drawn bare: each widget owns its own card
        // chrome, because the designs are not one card each. Standings
        // is two cards plus a status gutter outside them, and Relative a
        // gutter plus a card; Radar Bars and Pit Stall have deliberately
        // never had any.
        //
        // The Relative is page one of the black box rather than a panel
        // of its own, so this one draggable area is the whole box.
        let sync_controls = self.sync_controls(latest);
        // Computed before the `self.black_box` borrow below, as it reads
        // `self.team_sync`.
        let fuel_target_readout = self.fuel_target_readout(latest);
        let mut preview_config = self.config.blackbox.clone();
        if let Some(page) = preview_page {
            preview_config.hidden_pages.retain(|hidden| *hidden != page);
        }
        let pages = blackbox::configured_pages(latest, synced.as_ref(), &preview_config);
        let black_box = if preview_page.is_some() { &mut self.preview_black_box } else { &mut self.black_box };
        black_box.settle_page(pages);
        let mut layout = blackbox::layout_for(
            black_box.page(),
            latest,
            &self.config.blackbox,
            black_box.auto_fuel_litres(),
            black_box.box_called(),
            synced.as_ref(),
            sync_controls,
        );
        if let blackbox::Shape::Corners { wear_threshold_pct, .. } = &mut layout.shape {
            *wear_threshold_pct = self.team_sync.tyre_policy().and_then(|(policy, _)| match policy {
                crate::sync::protocol::TyrePolicy::BelowWear { threshold_pct } => Some(threshold_pct),
                _ => None,
            });
        }
        let clicks = &mut self.pending_clicks;
        let row_options = crate::ui::RowOptions {
            show_off_tracks: self.config.relative.show_off_tracks,
            show_flags: self.config.relative.show_flags,
            danger: &danger,
            fuel_target: fuel_target_readout,
        };
        let mut held = false;
        if panel_visible(selected_preview, crate::tray::Panel::Relative, layout_mode, self.config.relative.visible) {
            let pos = active_pos(seat_layout, &mut self.config.relative.pos, &mut self.config.relative.watch_pos);
            let synced_ref = synced.as_ref();
            let drag = draggable_panel(egui_context, "relative", pos, |ui| {
                let drawn_clicks =
                    blackbox::draw(ui, black_box, latest, &relative_config, &layout, row_options, synced_ref, pages);
                if preview_page.is_some() {
                    for click in drawn_clicks {
                        let _ = black_box.click(
                            click,
                            &layout.controls,
                            latest,
                            &mut preview_config,
                            &relative_config,
                            pages,
                        );
                    }
                } else {
                    clicks.extend(drawn_clicks);
                }
            });
            held |= drag.held;
            if drag.stopped {
                self.persist();
            }
        }

        if panel_visible(selected_preview, crate::tray::Panel::Standings, layout_mode, self.config.standings.visible) {
            let pos = active_pos(seat_layout, &mut self.config.standings.pos, &mut self.config.standings.watch_pos);
            let drag = draggable_panel(egui_context, "standings", pos, |ui| {
                standings::draw(
                    ui,
                    latest,
                    &standings_config,
                    crate::ui::RowOptions {
                        show_off_tracks: standings_config.show_off_tracks,
                        show_flags: standings_config.show_flags,
                        ..row_options
                    },
                );
            });
            held |= drag.held;
            if drag.stopped {
                self.persist();
            }
        }

        if panel_visible(selected_preview, crate::tray::Panel::RadarBars, layout_mode, self.config.radar.visible) {
            let pos = active_pos(seat_layout, &mut self.config.radar.pos, &mut self.config.radar.watch_pos);
            let drag = draggable_panel(egui_context, "radar_bars", pos, |ui| {
                radar_bars::draw(ui, latest, &radar_config);
            });
            held |= drag.held;
            if drag.stopped {
                self.persist();
            }
        }

        // The alarm is the widget's own memory of its level, so a threshold
        // can be held through a gap that breathes across it. Layout mode
        // draws the nearest car at the warning level whatever the thresholds
        // say, so the panel can be positioned — and a demo run is the same
        // kind of showing, so its screenshots match what layout mode shows.
        let faster_preview = layout_mode || self.demo || selected_preview == Some(crate::tray::Panel::FasterClass);
        if panel_visible(
            selected_preview,
            crate::tray::Panel::FasterClass,
            layout_mode,
            self.config.faster_class.visible,
        ) {
            let alarm = &mut self.faster_class_alarm;
            let pos =
                active_pos(seat_layout, &mut self.config.faster_class.pos, &mut self.config.faster_class.watch_pos);
            let drag = draggable_panel(egui_context, "faster_class", pos, |ui| {
                faster_class::draw(ui, latest, &faster_class_config, alarm, faster_preview);
            });
            held |= drag.held;
            if drag.stopped {
                self.persist();
            }
        }

        if panel_visible(selected_preview, crate::tray::Panel::PitStall, layout_mode, self.config.pit_stall.visible) {
            let pos = active_pos(seat_layout, &mut self.config.pit_stall.pos, &mut self.config.pit_stall.watch_pos);
            let drag = draggable_panel(egui_context, "pit_stall", pos, |ui| {
                pit_stall::draw(ui, latest, &pit_stall_config);
            });
            held |= drag.held;
            if drag.stopped {
                self.persist();
            }
        }
        held
    }
}

impl EguiOverlay for OverlayApp {
    /// Draws a frame, or skips one entirely when the next is not due yet.
    ///
    /// Overridden rather than left to the trait's default because that default
    /// runs a whole frame — an egui pass, tessellation, an upload and a
    /// `swap_buffers` — on *every* wake of the windowing loop, and the loop
    /// wakes on any message the thread receives, not only when the repaint
    /// delay has elapsed. Pacing [`FRAME_INTERVAL`] through
    /// `request_repaint_after` alone therefore set a floor on the frame rate
    /// without setting a ceiling: measured idle, stray messages still drove
    /// four to eighteen frames a second when four were asked for, and each one
    /// swapped a full-screen 4K surface through the desktop compositor, which
    /// is the expensive part whether or not anything was drawn into it.
    ///
    /// So the deadline is enforced here instead. A wake that arrives early
    /// returns immediately with the time still to run, which the loop uses as
    /// its next timeout, and no work is done at all.
    ///
    /// The body of the drawing path mirrors the trait's default implementation,
    /// which is the coupling this buys the saving with: if `egui_overlay` ever
    /// changes what a frame consists of, this has to change with it.
    fn run(
        &mut self,
        egui_context: &egui::Context,
        default_gfx_backend: &mut ThreeDBackend,
        glfw_backend: &mut GlfwBackend,
    ) -> Option<(egui::PlatformOutput, Duration)> {
        let now = Instant::now();
        if let Some(due) = self.next_frame_at
            && now < due
        {
            // Not due. Nothing is drawn and nothing is swapped; the cursor is
            // repeated rather than defaulted so a skipped frame mid-drag
            // cannot flicker it back to an arrow.
            let output = egui::PlatformOutput { cursor_icon: self.last_cursor, ..Default::default() };
            return Some((output, due - now));
        }

        let mut input = wheel_in_lines(glfw_backend.take_raw_input());
        // Typed keys reach egui only while one of the settings window's own
        // fields is focused; everywhere else the keyboard stays the sim's.
        // `wants_keyboard_input` is last frame's answer, which is the freshest
        // one there is before this frame begins.
        self.typed.poll(self.settings.open && egui_context.wants_keyboard_input(), &mut input.events);
        if self.screenshot.is_some() {
            // Hidden previews must not inherit the desktop pointer or a held
            // key: hover tooltips would obscure settings and make captures vary.
            input.events.clear();
            input.events.push(egui::Event::PointerGone);
        }
        if !self.sized {
            // Before the first pass rather than inside it: `set_fonts` only
            // takes effect at the next `begin_pass`, so a first frame that
            // already drew in the readout family — demo mode, or a live launch
            // with iRacing focused — panicked on a family bound to nothing.
            install_fonts(egui_context);
            // Lets `egui::Image` resolve the `file://` URIs the Standings
            // widget builds for manufacturer marks (see `ui::logos`).
            egui_extras::install_image_loaders(egui_context);
        }
        default_gfx_backend.prepare_frame(|| {
            let latest_size = glfw_backend.window.get_framebuffer_size();
            [latest_size.0.cast_unsigned(), latest_size.1.cast_unsigned()]
        });
        egui_context.begin_pass(input);
        self.gui_run(egui_context, default_gfx_backend, glfw_backend);
        let egui::FullOutput { platform_output, textures_delta, shapes, pixels_per_point, viewport_output } =
            egui_context.end_pass();
        let meshes = egui_context.tessellate(shapes, pixels_per_point);
        drop(viewport_output);
        default_gfx_backend.render_egui(meshes, textures_delta, glfw_backend.window_size_logical);
        // Read back before the buffers are swapped, which is when what was
        // just drawn is still the one being read from.
        self.frames_drawn = self.frames_drawn.saturating_add(1);
        if self.screenshot.is_some() && self.frames_drawn >= SCREENSHOT_AFTER_FRAMES {
            self.take_screenshot(default_gfx_backend);
            glfw_backend.window.set_should_close(true);
        }
        if glfw_backend.is_opengl() {
            use egui_overlay::egui_window_glfw_passthrough::glfw::Context as _;
            glfw_backend.window.swap_buffers();
        }

        // The pace comes from `gui_run`'s own decision, not from egui's
        // `repaint_delay`. egui asks for an immediate repaint whenever input
        // reaches it, and this window covers the whole screen, so ordinary
        // desktop mouse movement had it asking for one continuously — which
        // pushed the measured idle rate back up past a hundred frames a second
        // even with the deadline above in place. What the sim is showing does
        // not depend on where the mouse is, so the overlay's own interval wins.
        let pace = self.frame_interval;
        self.last_cursor = platform_output.cursor_icon;
        self.next_frame_at = Some(Instant::now() + pace);
        Some((platform_output, pace))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the per-frame body: input, sync, focus, panels, clicks and settings in sequence"
    )]
    fn gui_run(
        &mut self,
        egui_context: &egui::Context,
        _default_gfx_backend: &mut ThreeDBackend,
        glfw_backend: &mut GlfwBackend,
    ) {
        let now = Instant::now();
        while let Ok(snapshot) = self.rx.try_recv() {
            self.latest = Some(snapshot);
            self.latest_at = Some(now);
        }
        self.reload_binds_if_changed(now);
        self.settle_layout(egui_context);
        // A hand-called BOX BOX clears itself once the car is on pit road: the
        // stop it reminded of is being made. `false` (no snapshot) is a no-op.
        self.black_box
            .clear_box_call_on_pit_road(self.latest.as_ref().is_some_and(|snap| snap.pit_service.on_pit_road));
        // Team sync runs off the latest snapshot: it publishes the driver's
        // events and folds in everyone else's. Inert unless enabled and the
        // session has an identity — see `crate::sync::runtime`. The hosting
        // member's relay is raised first so their own client has something
        // to join.
        self.ensure_relay();
        let sync_config = self.effective_sync_config();
        self.team_sync.update(&sync_config, self.latest.as_ref(), now);
        self.apply_team_writes();
        // Every mark drawn this frame is chosen by the logo config in
        // force; see `ui::logos::apply`.
        crate::ui::logos::apply(&self.config.logos);
        // Every widget reads the theme while drawing, so it is published once
        // here rather than threaded through every draw call — see `ui::theme`.
        crate::ui::theme::apply(crate::ui::theme::Theme::Panel);

        if !self.sized {
            let size = primary_monitor_size(glfw_backend);
            glfw_backend.set_window_size([size[0] + WINDOW_OVERSCAN, size[1] + WINDOW_OVERSCAN]);
            glfw_backend.window.set_pos(0, 0);
            // GLFW names the window "g" by default, which is what Task
            // Manager and any window-inspector tool then shows.
            glfw_backend.window.set_title("Race Overlay");
            // A screenshot run renders to the framebuffer and reads it back,
            // which a hidden window does just as well — and it is the whole
            // screen for a second or two otherwise. Shooting a page of them
            // in a row strobes the desktop; hiding makes the run silent.
            if self.screenshot.is_some() {
                glfw_backend.window.hide();
            }
            apply_window_styles(glfw_backend, self.config.stream_mode);
            self.stream_mode_applied = Some(self.config.stream_mode);
            crate::focus::focus_iracing_window(&self.config.iracing_process_name);
            crate::focus::yield_foreground(HWND(glfw_backend.window.get_win32_window()), self.launched_from);
            self.sized = true;
        }

        // The sim's own word that the car is in the garage, ignored once the
        // snapshot carrying it has gone stale — a session left from the
        // garage sends no further ticks to say otherwise.
        let in_garage = self.config.hide_in_garage
            && self.latest_at.is_some_and(|at| now.duration_since(at) < SNAPSHOT_STALE)
            && self.latest.as_ref().is_some_and(|snapshot| snapshot.in_garage);
        // Advanced every frame, not just when the panels might be drawn, so
        // the delay measures garage time rather than drawn time.
        let garage_hidden = self.garage_hide.update(in_garage, now);

        // Skip drawing entirely while iRacing isn't focused: still
        // click-through regardless (nothing interactive gets drawn either
        // way), but this avoids cluttering whatever app the user tabbed
        // into instead. The garage does the same for the setup screen.
        //
        // Demo and layout mode override both. Layout mode especially: it is
        // the thing you turn on in the garage to drag the panels around, so a
        // garage rule that hid them would take that with it.
        // The settings window forces the panels on too: changing a setting
        // means looking at the panel it changes, and the window itself has to
        // be reachable from whatever app the driver opened it from.
        let should_show = self.demo
            || self.layout_mode()
            || self.settings.open
            || ((!self.config.only_show_when_iracing_focused
                || self.focus_tracker.is_focused(&self.config.iracing_process_name))
                && !garage_hidden);

        // Whether a panel is holding the pointer decides one thing only: that
        // foreground must not be handed back this frame — see
        // [`OverlayApp::report_window_state`].
        let panel_held = should_show && self.draw_panels(egui_context);
        self.draw_settings(egui_context, now);
        self.apply_stream_mode(glfw_backend);

        // Binds are polled every frame regardless of whether the panels are
        // drawn, so paging while the box is hidden can't leave it on a page
        // the driver didn't choose.
        // Auto Fuel keeps the load topped up on its own, whether or not the
        // Fuel page is the one showing.
        if let Some(request) =
            self.black_box.auto_fuel_request(self.latest.as_ref(), &self.config.blackbox, Instant::now())
        {
            let _ = self.pit_requests.send(request);
        }

        // Settled once a frame, before anything reads the page: the set of
        // pages changes with the session — Strategy appears when a race is seen
        // to need more than one stop — and a driver parked on a page that has
        // just gone away has to end up somewhere sensible rather than nowhere.
        // The same synced fuel that draws the page decides whether it is on
        // offer, so a spectator's Fuel page and its rail tab agree.
        let synced = self.spectator_synced();
        // While spectating, the black box reads the synced-adjusted snapshot
        // so its controls build correct requests from the driver's real fuel.
        let bb_snapshot = self.synced_blackbox_snapshot();
        let bb = bb_snapshot.as_ref().or(self.latest.as_ref());
        let pages = blackbox::configured_pages(bb, synced.as_ref(), &self.config.blackbox);
        self.black_box.settle_page(pages);

        let binds = self.config.binds.pairs();
        let clicks = std::mem::take(&mut self.pending_clicks);
        // Danger marks are config, not a pit action; collected here and applied
        // after the black-box loop below, so they don't fight its borrow of the
        // snapshot. See `plans/danger-drivers.md`.
        let danger_marks: Vec<(u32, Option<crate::config::DangerLevel>)> = clicks
            .iter()
            .filter_map(|click| match click {
                blackbox::Click::Danger { cust_id, level } => Some((*cust_id, *level)),
                _ => None,
            })
            .collect();
        if !binds.is_empty() || !clicks.is_empty() {
            let page = self.black_box.page();
            let fuel = self.black_box.auto_fuel_litres();
            let box_called = self.black_box.box_called();
            let fuel_seed = self.fuel_target_control(bb);
            let sync_controls = self.sync_controls(bb);
            let rows =
                blackbox::layout_for(page, bb, &self.config.blackbox, fuel, box_called, synced.as_ref(), sync_controls)
                    .controls;
            let relative = self.config.relative.clone();
            // Sync-control steps (fuel target, tyre policy) go to team sync,
            // not the sim; collected here and applied after the loop so they
            // don't fight `bb`'s borrow.
            let mut fuel_target_actions: Vec<input::Action> = Vec::new();
            let mut tyre_policy_actions: Vec<input::Action> = Vec::new();
            let control_at = |index: usize| rows.get(index).and_then(|row| row.kind.control());
            for action in self.actions.poll(&binds) {
                if let Some(request) =
                    self.black_box.apply(action, &rows, bb, &mut self.config.blackbox, &relative, pages)
                {
                    self.dispatch_pit_request(request);
                }
            }
            // A click is the same action the bind would have sent, landed on
            // the control it was made on — see `BlackBox::click`.
            for click in clicks {
                if let blackbox::Click::Control { index, action } = click {
                    match control_at(index) {
                        Some(blackbox::Control::FuelTarget) => {
                            // App-owned actions still select their control so the
                            // next wheel input targets the row that was clicked.
                            let _ = self.black_box.click(click, &rows, bb, &mut self.config.blackbox, &relative, pages);
                            fuel_target_actions.push(action);
                            continue;
                        }
                        Some(blackbox::Control::TyrePolicy) => {
                            let _ = self.black_box.click(click, &rows, bb, &mut self.config.blackbox, &relative, pages);
                            tyre_policy_actions.push(action);
                            continue;
                        }
                        _ => {}
                    }
                }
                if let Some(request) =
                    self.black_box.click(click, &rows, bb, &mut self.config.blackbox, &relative, pages)
                {
                    self.dispatch_pit_request(request);
                }
            }
            for action in fuel_target_actions {
                self.adjust_fuel_target(action, fuel_seed);
            }
            for action in tyre_policy_actions {
                self.adjust_tyre_policy(action);
            }
        }
        for (cust_id, level) in danger_marks {
            self.apply_danger_mark(cust_id, level);
        }
        self.save_blackbox_if_settled(Instant::now());

        // `is_anything_being_dragged` is checked explicitly (not just
        // `wants_pointer_input`) so passthrough can never re-engage
        // mid-drag: if the OS ever routes even one intermediate mouse-move
        // to the window beneath instead of this overlay, the drag loses its
        // delta stream and the panel snaps back instead of following the
        // cursor.
        // Tray menu clicks arrive on this thread's message queue, which the
        // window's own event pump drains, so they're polled here.
        let tray_actions = self.tray.as_ref().map(Tray::poll).unwrap_or_default();
        if tray_actions.open_settings {
            self.settings.open = true;
        }
        if tray_actions.quit {
            glfw_backend.window.set_should_close(true);
        }

        let capture_pointer = egui_context.wants_pointer_input()
            || egui_context.wants_keyboard_input()
            || egui_context.dragged_id().is_some();
        glfw_backend.set_passthrough(!capture_pointer);
        // A widget being held counts the same as a panel: yielding foreground
        // under a held button makes GLFW synthesise a release, which is what
        // dropped every slider drag in the settings window one frame in —
        // the same snap-back the panels' own drags used to have.
        self.report_window_state(glfw_backend, panel_held || egui_context.is_using_pointer());
        // Paced rather than unconditional — see [`FRAME_INTERVAL`]. egui hands
        // this delay to the windowing loop as its `wait_events_timeout`, so it
        // is what actually puts the thread to sleep between frames; asking for
        // an immediate repaint instead is what left it spinning. A frame the
        // driver is interacting with must not be paced down to the idle rate,
        // so a held pointer counts as showing.
        // Recorded for [`OverlayApp::run`], which is what actually holds the
        // frame back; the request keeps egui's own bookkeeping in step so its
        // repaint delay never reads as "never".
        self.frame_interval = if should_show || capture_pointer { FRAME_INTERVAL } else { IDLE_INTERVAL };
        egui_context.request_repaint_after(self.frame_interval);
    }
}

/// A selected settings preview is exclusive, even when layout mode is on.
fn panel_visible(selected: Option<crate::tray::Panel>, panel: crate::tray::Panel, layout: bool, visible: bool) -> bool {
    selected.map_or(layout || visible, |selected| selected == panel)
}

/// Draws live panels and settings previews from the same saved position and
/// Area identity. Selecting a settings page changes content, never geometry.
fn draggable_panel(
    ctx: &egui::Context,
    id: &str,
    pos: &mut [f32; 2],
    add_contents: impl FnOnce(&mut egui::Ui),
) -> PanelDrag {
    let area = egui::Area::new(egui::Id::new(id))
        .current_pos(egui::pos2(pos[0], pos[1]))
        .movable(true)
        // egui keeps an `Area` inside the screen rect by default, and this
        // window is not the screen yet on the frame it is first drawn: GLFW
        // creates it at its own 800x600 and the resize to the monitor only
        // lands on the frame after. Every panel drawn in that first pass was
        // shoved into that corner — a Standings wider and taller than it
        // came out at exactly [0, 0] — and the shoved position was then
        // written back over the saved one, so the next thing to save wrote
        // the ruined layout to disk. That is the layout that "doesn't stay
        // put" across a restart. A panel dragged off-screen is what **Reset
        // all positions** is for; it is not worth this.
        .constrain(false)
        .show(ctx, add_contents);

    // Only a drag moves a panel, so only a drag writes the position back.
    // Any other frame that copied the rect across was a frame that could
    // overwrite a saved position with wherever egui had happened to put the
    // panel that frame — see `constrain` above.
    if area.response.dragged() || area.response.drag_stopped() {
        let top_left = area.response.rect.min;
        pos[0] = top_left.x;
        pos[1] = top_left.y;
    }
    PanelDrag { stopped: area.response.drag_stopped(), held: area.response.is_pointer_button_down_on() }
}

/// What one panel's `Area` did with the pointer this frame.
#[derive(Debug, Clone, Copy)]
struct PanelDrag {
    /// A drag just ended, so the caller can persist the new position without
    /// writing to disk on every dragged frame.
    stopped: bool,
    /// A mouse button is down on this panel — true from the press itself,
    /// before any movement has made it a drag. That is the window in which
    /// foreground must not be handed back; see [`OverlayApp::report_window_state`].
    held: bool,
}

/// Inter (proportional) and IBM Plex Mono (monospace) — the exact same
/// pairing the design mockups were built with, embedded at compile time
/// (both OFL-licensed; see `assets/fonts/*-OFL.txt`) rather than read from
/// the system font directory. egui's built-in font is deliberately
/// minimal — fine for a debug UI, but it was the single biggest reason
/// this app's actual rendering looked nothing like its design mockups.
pub(super) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut install = |family: egui::FontFamily, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), egui::FontData::from_static(bytes));
        if let Some(family_fonts) = fonts.families.get_mut(&family) {
            family_fonts.insert(0, name.to_owned());
        }
    };
    install(egui::FontFamily::Proportional, "inter", include_bytes!("../../../assets/fonts/Inter.ttf"));
    install(
        egui::FontFamily::Monospace,
        "ibm-plex-mono",
        include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf"),
    );
    // The readout face, under its own family so a call site asks for it by
    // role rather than by name — see `ui::readout`. Inter follows it in the
    // family so any glyph Barlow lacks still renders, in the text face.
    fonts.font_data.insert(
        "barlow-condensed".to_owned(),
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/BarlowCondensed-SemiBold.ttf")),
    );
    fonts.families.insert(
        egui::FontFamily::Name(crate::ui::READOUT_FAMILY.into()),
        vec!["barlow-condensed".to_owned(), "inter".to_owned()],
    );
    ctx.set_fonts(fonts);
}

/// The settings file's modification time, if it exists and can be read.
fn settings_modified_at() -> Option<SystemTime> {
    std::fs::metadata(crate::config::config_path()).and_then(|meta| meta.modified()).ok()
}

/// Writes the live settings to disk.
///
/// The binds are written back, even though `--bind` also owns them: the
/// overlay re-reads them whenever the file changes underneath it, so what
/// goes out here is what that run saved.
///
/// Written as "all of it" rather than as a list of fields to copy across,
/// because the list was the bug. It was the list of settings that saved, and
/// every setting added after it was written was quietly missing from it —
/// stream mode, the focus and garage rules, the flag and off-track columns,
/// team sync. Each one worked for the session it was ticked in and was gone
/// by the next launch, because the file it was meant to be in was never told
/// about it.
fn save_config(config: &OverlayConfig) {
    let mine = config.clone();
    let saved = OverlayConfig::update(|on_disk| {
        *on_disk = mine;
    });
    if let Err(err) = saved {
        println!("note: could not save race-overlay.toml: {err:#}");
    }
}

/// Relabels mouse-wheel events as lines, so a notch scrolls a line, not a point.
///
/// The GLFW backend reports each wheel notch as `MouseWheelUnit::Point` with
/// the raw offset GLFW gives it — ±1 — so egui moved a scroll area one pixel
/// per notch, which is what made the settings pages crawl. Called lines
/// instead, egui multiplies by its line speed (40 points by default), which
/// is the scrolling every other Windows app does.
fn wheel_in_lines(mut input: egui::RawInput) -> egui::RawInput {
    for event in &mut input.events {
        if let egui::Event::MouseWheel { unit, .. } = event
            && *unit == egui::MouseWheelUnit::Point
        {
            *unit = egui::MouseWheelUnit::Line;
        }
    }
    input
}

/// The facts about the overlay window that decide whether the desktop
/// compositor keeps its alpha — see [`apply_window_styles`].
///
/// Printed whenever they change. That is how the settings-window case was
/// caught: the log showed the window *layered throughout* and yet becoming
/// foreground the moment the settings window was clicked, `WS_EX_NOACTIVATE`
/// notwithstanding — and the screen going black on that line. Foreground on
/// its own is enough, so it is now given back whenever it lands here.
///
/// `no_activate` is printed alongside for that last clause: a window
/// carrying `WS_EX_NOACTIVATE` is one Windows does not bring to the front
/// when it is clicked, so if this window keeps arriving at foreground, the
/// log now says whether the style was on it at the time or had been taken
/// off by something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one line of a log: four window flags as the OS reports them, not a state to model"
)]
struct WindowState {
    layered: bool,
    click_through: bool,
    foreground: bool,
    no_activate: bool,
}

impl OverlayApp {
    /// Prints the window's compositing state when it changes, and hands
    /// foreground back the moment this window has it.
    ///
    /// Nothing about the overlay ever wants foreground: its binds are polled
    /// rather than delivered as key events, and its settings window is
    /// sliders and checkboxes. Foreground is given back to whichever window
    /// held it last — the sim, in practice — every frame it is found here,
    /// because a monitor-sized topmost window that is also the foreground
    /// window is what the compositor stops compositing.
    ///
    /// Not, however, while a panel has the pointer held. Deactivating a
    /// window under a held button makes GLFW synthesise a button release, and
    /// a panel being dragged is dropped a few pixels into the drag — the
    /// jitter layout mode used to have. The yield waits for the button to come
    /// up; [`restore_overlay_styles`] covers the alpha until it does.
    fn report_window_state(&mut self, glfw_backend: &GlfwBackend, panel_held: bool) {
        let hwnd = HWND(glfw_backend.window.get_win32_window());
        // SAFETY: `hwnd` is this app's own live top-level window, just
        // obtained from GLFW; reading its style has no preconditions.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let has = |flag: u32| style & isize::try_from(flag).unwrap_or(0) != 0;
        let foreground = crate::focus::foreground_window();
        let state = WindowState {
            layered: has(WS_EX_LAYERED.0),
            click_through: has(WS_EX_TRANSPARENT.0),
            foreground: foreground == Some(hwnd),
            no_activate: has(WS_EX_NOACTIVATE.0),
        };
        // Both styles, not just the layering: a window that has lost
        // `WS_EX_NOACTIVATE` is one the next click brings to the front, which
        // is the other half of what stops the compositor compositing it.
        if !state.layered || !state.no_activate {
            restore_overlay_styles(hwnd);
        }
        match foreground {
            Some(other) if other != hwnd => self.last_other_foreground = Some(other),
            Some(_) if !panel_held => crate::focus::yield_foreground(hwnd, self.last_other_foreground),
            Some(_) | None => {}
        }
        if self.window_state != Some(state) {
            println!(
                "note: overlay window layered={} click-through={} foreground={} no-activate={}",
                state.layered, state.click_through, state.foreground, state.no_activate
            );
            self.window_state = Some(state);
        }
    }
}

/// Applies the extended window styles the overlay needs: never activate, and
/// never appear in the taskbar or alt-tab.
///
/// `WS_EX_TOOLWINDOW` (with `WS_EX_APPWINDOW` cleared) is what keeps it out
/// of the taskbar. There is nothing to switch to — the window is a
/// transparent, click-through sheet covering the whole screen — so a taskbar
/// button for it is purely noise. The tray icon is the app's handle instead.
/// Stream mode leaves this one style off: OBS's Window Capture picker skips
/// tool windows, so hiding from the taskbar was also hiding from OBS — see
/// `OverlayConfig::stream_mode`. Everything else here applies either way.
///
/// `WS_EX_NOACTIVATE` means the window can never gain OS focus/activation —
/// not even for the one frame a click might land on it before click-through
/// catches up.
///
/// Without this, a click that briefly lands on the overlay (e.g. right
/// after the cursor crosses from a panel onto the game beneath it, before
/// the next frame re-enables click-through) activates the window as a
/// normal side effect of Windows handling that click. DWM's blur-behind
/// transparency can then flash its raw, opaque backing surface for a frame
/// during that activation transition — the "screen goes black for an
/// instant" symptom. `WS_EX_NOACTIVATE` prevents the activation entirely,
/// so that transition (and the flash) can't happen.
///
/// `WS_EX_LAYERED` is made permanent here, at full opacity. GLFW's mouse
/// passthrough adds that style while the pointer is over nothing and takes it
/// away again while it is over a panel — and a monitor-sized, topmost,
/// *non-layered* window that is also the foreground window is one the
/// desktop compositor scans out directly instead of compositing, which
/// throws its alpha away: the whole screen goes black behind the panels
/// until a click somewhere hands foreground to another window. That is the
/// state a fresh launch is in (the new window has foreground, and the pointer
/// is wherever it was left), and the one layout mode is in (every panel on
/// screen, so the pointer is over one). A layered window is never scanned
/// out directly. GLFW keeps the style across passthrough changes for as long
/// as the window has an `LWA_ALPHA` attribute, so it is set once here.
///
/// `SetWindowLongPtr` alone leaves some of what it changes cached until the
/// window's frame is next recomputed; `SWP_FRAMECHANGED` does that now.
/// The whole window's opacity: one step below opaque, deliberately.
///
/// A layered window at full `LWA_ALPHA` opacity is still one the compositor
/// may hand the screen to directly when it is monitor-sized, topmost and
/// foreground — which throws its per-pixel alpha away and flashes the screen
/// black. That is exactly the state a click on the settings window puts the
/// overlay in for the length of the press: foreground is deliberately *held*
/// while a button is down (yielding it mid-press makes GLFW synthesise a
/// release and drop the drag), so every click blinked the desktop black
/// until the release gave foreground back. At 254 the window is never
/// opaque, so the compositor must always blend it and the bypass can never
/// engage — and 1/255 of dimming on an overlay's own pixels is invisible.
const WINDOW_ALPHA: u8 = 254;

/// How far past the monitor the window is stretched, in pixels.
///
/// The other half of the same defence as [`WINDOW_ALPHA`], from the geometry
/// side. What the compositor may hand the screen to directly is a topmost
/// window whose bounds are *the monitor's* — an exact match is part of the
/// test — so a window a pixel larger than the display is never a candidate
/// for it, whatever it does with foreground. The pixel is spent off the
/// bottom-right corner, where the overlay draws nothing.
///
/// Cheap insurance rather than a measured fix: the alpha above was supposed
/// to settle this on its own, and the screen still blinks black on a click
/// in the settings window. Both together cost one pixel and one step of
/// dimming.
const WINDOW_OVERSCAN: f32 = 1.0;

fn apply_window_styles(glfw_backend: &GlfwBackend, stream_mode: bool) {
    let hwnd = HWND(glfw_backend.window.get_win32_window());
    // SAFETY: `hwnd` is this app's own live top-level window, just obtained
    // from GLFW.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let mut add = WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0;
        let mut remove = 0;
        if stream_mode {
            // `WS_EX_APPWINDOW` is added, not merely left alone: a
            // `WS_EX_NOACTIVATE` window gets no taskbar button unless it is
            // forced with this style, and the taskbar button is the visible
            // confirmation stream mode took effect at all.
            add |= WS_EX_APPWINDOW.0;
            remove |= WS_EX_TOOLWINDOW.0;
        } else {
            add |= WS_EX_TOOLWINDOW.0;
            remove |= WS_EX_APPWINDOW.0;
        }
        let add = isize::try_from(add).unwrap_or(0);
        let remove = isize::try_from(remove).unwrap_or(0);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style | add) & !remove);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), WINDOW_ALPHA, LWA_ALPHA);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

impl OverlayApp {
    /// Writes the frame just rendered to the `--screenshot` path as a PNG.
    ///
    /// The overlay's window is layered, topmost and drawn by OpenGL, and none
    /// of the ordinary capture routes can read it: a screen `BitBlt` leaves it
    /// out, the same blit with `CAPTUREBLT` returns it at a fraction of its
    /// alpha, and `PrintWindow` — even with `PW_RENDERFULLCONTENT` — returns
    /// black, because there is no GDI content to print. Asking the renderer
    /// for its own framebuffer is the one route that sees what the driver
    /// sees, which makes it the only way to check a widget against its mockup.
    ///
    /// The transparent parts of the overlay are composited onto black, which
    /// is what the design mockups themselves sit on, so the file can be
    /// compared with one directly.
    ///
    /// Failures are reported and otherwise ignored: this is a diagnostic, and
    /// a screenshot that could not be written is not a reason to take the
    /// overlay down with it.
    fn take_screenshot(&self, backend: &ThreeDBackend) {
        if let Some(path) = &self.screenshot {
            capture_screenshot(path, backend);
        }
    }
}

/// Captures the current GL framebuffer for overlay and first-run design previews.
pub(super) fn capture_screenshot(path: &std::path::Path, backend: &ThreeDBackend) {
    use egui_overlay::egui_render_three_d::glow::{HasContext as _, PixelPackData, RGBA, UNSIGNED_BYTE};

    let [width, height] = backend.glow_backend.framebuffer_size;
    let (Ok(w), Ok(h)) = (usize::try_from(width), usize::try_from(height)) else {
        println!("note: could not screenshot: framebuffer is {width}x{height}");
        return;
    };
    let mut rgba = vec![0_u8; w * h * 4];
    let gl = &backend.glow_backend.glow_context;
    // SAFETY: `glow` marks every GL entry point unsafe. This one reads the
    // bound framebuffer into `rgba`, which is exactly `w * h * 4` bytes —
    // the size the format and dimensions passed here ask for — and runs on
    // the thread that owns the context, mid-frame.
    //
    // The semicolon has to sit outside the block for the crate's
    // `semicolon_outside_block` lint, which is what the other lint objects
    // to; they cannot both be satisfied on a one-call block.
    #[expect(clippy::semicolon_if_nothing_returned, reason = "conflicts with semicolon_outside_block")]
    unsafe {
        gl.read_pixels(
            0,
            0,
            width.cast_signed(),
            height.cast_signed(),
            RGBA,
            UNSIGNED_BYTE,
            PixelPackData::Slice(&mut rgba),
        )
    };
    // OpenGL's origin is bottom-left and a PNG's is top-left, so the rows
    // come back in the opposite order; each is composited onto black on
    // the way into the output.
    let mut opaque = Vec::with_capacity(w * h * 3);
    for row in (0..h).rev() {
        for pixel in rgba[row * w * 4..(row + 1) * w * 4].as_chunks::<4>().0 {
            let alpha = f32::from(pixel[3]) / 255.0;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a channel times a 0..=1 alpha stays inside u8"
            )]
            opaque.extend(pixel[..3].iter().map(|channel| (f32::from(*channel) * alpha) as u8));
        }
    }
    match write_png(path, &opaque, width, height) {
        Ok(()) => println!("note: wrote {}", path.display()),
        Err(err) => println!("note: could not write {}: {err}", path.display()),
    }
}

/// Writes `rgb` to `path` as an 8-bit RGB PNG.
///
/// # Errors
/// Returns an error if the file cannot be created or the encoder rejects the
/// data.
pub fn write_png(path: &std::path::Path, rgb: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgb)?;
    Ok(())
}

/// Puts `WS_EX_LAYERED` and `WS_EX_NOACTIVATE` back on the window, with the
/// alpha attribute that keeps the first of them there.
///
/// GLFW strips the layering whenever mouse passthrough is turned *off* and
/// the window has no `LWA_ALPHA` attribute at that moment
/// (`_glfwPlatformSetWindowMousePassthrough`), and passthrough is turned off
/// every time the pointer moves over a panel or the settings window. A
/// full-screen topmost window that is not layered is one the desktop
/// compositor scans out directly instead of compositing, which throws its
/// alpha away and blacks out the whole screen — the failure this is the
/// defence against.
///
/// `WS_EX_NOACTIVATE` is re-asserted alongside it because the same call is
/// what would drop it: the style is read, edited and written back whole, so
/// anything that read the style before this app first set it would put a
/// copy without it back. Without that style a click brings this window to
/// the front, and foreground is the other half of the same failure.
///
/// `apply_window_styles` sets all of it once at startup; this re-asserts it
/// on any frame a style is found missing, which is cheap because the style
/// is already being read there to report it.
fn restore_overlay_styles(hwnd: HWND) {
    let restore = isize::try_from(WS_EX_LAYERED.0 | WS_EX_NOACTIVATE.0).unwrap_or(0);
    // SAFETY: `hwnd` is this app's own live top-level window, obtained from
    // GLFW by the caller.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | restore);
        // The attribute is what makes GLFW keep the style next time round;
        // just under opaque so the compositor can never bypass — see
        // [`WINDOW_ALPHA`].
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), WINDOW_ALPHA, LWA_ALPHA);
    }
}

/// Reads the primary monitor's resolution via GLFW, so the overlay window
/// covers the whole screen. Falls back to a common resolution if GLFW can't
/// find a monitor.
///
/// The window is sized to this plus [`WINDOW_OVERSCAN`], not to this exactly.
#[expect(clippy::cast_precision_loss, reason = "screen resolutions are far below f32's exact-integer range")]
fn primary_monitor_size(glfw_backend: &mut GlfwBackend) -> [f32; 2] {
    let video_mode = glfw_backend.glfw.with_primary_monitor(|_, monitor| monitor.and_then(|m| m.get_video_mode()));
    match video_mode {
        Some(mode) => [mode.width as f32, mode.height as f32],
        None => FALLBACK_MONITOR_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garage_hide_waits_out_the_delay_before_hiding() {
        let start = Instant::now();
        let mut hide = GarageHide::default();

        assert!(!hide.update(true, start), "hiding on the first garage tick would flicker");
        assert!(!hide.update(true, start + GARAGE_HIDE_DELAY / 2), "still inside the wait");
        assert!(hide.update(true, start + GARAGE_HIDE_DELAY));
    }

    #[test]
    fn garage_hide_shows_again_without_delay() {
        let start = Instant::now();
        let mut hide = GarageHide::default();
        hide.update(true, start);
        assert!(hide.update(true, start + GARAGE_HIDE_DELAY), "the panels should be hidden by now");

        assert!(!hide.update(false, start + GARAGE_HIDE_DELAY), "leaving the garage must show the panels at once");
    }

    /// A tick that says "not in garage" restarts the wait, so a car that dips
    /// in and straight back out never reaches the delay.
    #[test]
    fn garage_hide_restarts_the_wait_after_a_gap() {
        let start = Instant::now();
        let mut hide = GarageHide::default();

        assert!(!hide.update(true, start));
        assert!(!hide.update(false, start + Duration::from_millis(100)));
        assert!(!hide.update(true, start + Duration::from_millis(200)));
        assert!(!hide.update(true, start + GARAGE_HIDE_DELAY), "the wait should run from the second entry");
        assert!(hide.update(true, start + Duration::from_millis(200) + GARAGE_HIDE_DELAY));
    }

    #[test]
    fn garage_hide_never_hides_while_out_of_the_garage() {
        let start = Instant::now();
        let mut hide = GarageHide::default();

        for step in 0..10 {
            assert!(!hide.update(false, start + Duration::from_secs(step)));
        }
    }

    #[test]
    fn a_definite_seat_picks_its_layout() {
        use crate::telemetry::snapshot::Seat;
        assert_eq!(layout_for_seat(Some(&Seat::Driving), Layout::Watching), Layout::Driving);
        assert_eq!(layout_for_seat(Some(&Seat::Spectating("Istvan Fodor".into())), Layout::Driving), Layout::Watching);
        assert_eq!(layout_for_seat(Some(&Seat::TeamMate("Istvan Fodor".into())), Layout::Driving), Layout::Watching);
    }

    /// The garage, a tow and a quiet session are transitions, not places: the
    /// panels stay wherever the last definite seat put them, and before any
    /// snapshot at all that is the driving layout.
    #[test]
    fn out_of_car_and_silence_keep_the_layout_in_force() {
        use crate::telemetry::snapshot::Seat;
        assert_eq!(layout_for_seat(Some(&Seat::OutOfCar), Layout::Watching), Layout::Watching);
        assert_eq!(layout_for_seat(Some(&Seat::OutOfCar), Layout::Driving), Layout::Driving);
        assert_eq!(layout_for_seat(None, Layout::Watching), Layout::Watching);
        assert_eq!(layout_for_seat(None, Layout::default()), Layout::Driving);
    }

    #[test]
    fn pit_writes_route_over_the_wire_only_from_a_spectator_seat() {
        use crate::telemetry::snapshot::Seat;
        use std::sync::Arc;
        assert!(pit_request_routes_over_wire(Some(&Seat::Spectating(Arc::from("Ben")))), "a spec adjusts the team car");
        assert!(!pit_request_routes_over_wire(Some(&Seat::Driving)), "a driver's own request goes local");
        assert!(
            !pit_request_routes_over_wire(Some(&Seat::TeamMate(Arc::from("Ben")))),
            "a team-mate's own car is local"
        );
        assert!(!pit_request_routes_over_wire(Some(&Seat::OutOfCar)), "the garage is local");
        assert!(!pit_request_routes_over_wire(None), "no session, no wire");
    }

    #[test]
    fn a_team_write_applies_only_to_a_consenting_seated_driver() {
        use crate::telemetry::snapshot::Seat;
        use std::sync::Arc;
        assert!(team_write_applies(true, Some(&Seat::Driving)), "consent armed and in the car");
        assert!(!team_write_applies(false, Some(&Seat::Driving)), "no consent, no remote hand on the box");
        assert!(!team_write_applies(true, Some(&Seat::Spectating(Arc::from("Ben")))), "a spectator can't arm the sim");
        assert!(!team_write_applies(true, Some(&Seat::OutOfCar)), "nobody is in the car to arm it");
        assert!(!team_write_applies(true, None), "no session");
    }

    /// Watching reads the driving position until a drag makes its own, and
    /// the seeding itself counts as having one — that is what lets a drag's
    /// write land in the right slot.
    #[test]
    fn the_watching_layout_seeds_from_the_driving_one() {
        // Integer-valued positions, so exact equality is honest here; a helper
        // keeps clippy's float-array-comparison lint happy in test code.
        let eq = |a: [f32; 2], b: [f32; 2]| a[0].to_bits() == b[0].to_bits() && a[1].to_bits() == b[1].to_bits();
        let mut pos = [10.0, 20.0];
        let mut watch_pos = None;

        assert!(eq(*active_pos(Layout::Watching, &mut pos, &mut watch_pos), [10.0, 20.0]));
        assert!(watch_pos.is_some_and(|w| eq(w, [10.0, 20.0])));

        *active_pos(Layout::Watching, &mut pos, &mut watch_pos) = [300.0, 40.0];
        assert!(eq(pos, [10.0, 20.0]), "a watching drag must not move the driving layout");
        assert!(eq(*active_pos(Layout::Driving, &mut pos, &mut watch_pos), [10.0, 20.0]));
        assert!(watch_pos.is_some_and(|w| eq(w, [300.0, 40.0])));
    }

    /// The first pass of every launch is drawn before the window has been
    /// resized to the monitor, so the screen rect is still GLFW's own
    /// 800x600 — and a saved position is a position on the real screen.
    /// Nothing about that pass may reach the config: the panels came back
    /// from the corner they were shoved into, but the corner is what the
    /// next save wrote to disk.
    #[test]
    fn a_panel_keeps_its_saved_position_on_a_screen_too_small_for_it() {
        let eq = |a: [f32; 2], b: [f32; 2]| a[0].to_bits() == b[0].to_bits() && a[1].to_bits() == b[1].to_bits();
        let saved = [2813.0, 995.0];
        let mut pos = saved;

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        // Wider and taller than that screen, as the Standings is: the panel
        // this landed on hardest, at exactly [0, 0].
        let drawn = ctx.run(input, |ctx| {
            draggable_panel(ctx, "standings", &mut pos, |ui| {
                ui.allocate_space(egui::vec2(900.0, 700.0));
            });
        });
        drop(drawn);

        assert!(eq(pos, saved), "a pass with no drag in it must leave the saved position alone");
    }
    #[test]
    fn settings_preview_is_exclusive_and_leaves_normal_visibility_intact() {
        use crate::tray::Panel;
        let panels = [Panel::Relative, Panel::Standings, Panel::RadarBars, Panel::FasterClass, Panel::PitStall];
        for selected in panels {
            for panel in panels {
                for layout in [false, true] {
                    for visible in [false, true] {
                        assert_eq!(panel_visible(Some(selected), panel, layout, visible), selected == panel);
                        assert_eq!(panel_visible(None, panel, layout, visible), layout || visible);
                    }
                }
            }
        }
    }

    #[test]
    fn dragged_panel_stays_put_after_selecting_another_panel() {
        let ctx = egui::Context::default();
        let mut relative = [120.0, 140.0];
        let mut standings = [500.0, 140.0];
        let frame = |id: &str, pos: &mut [f32; 2], events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
                events,
                ..Default::default()
            };
            let mut rect = egui::Rect::NOTHING;
            drop(ctx.run(input, |ctx| {
                draggable_panel(ctx, id, pos, |ui| {
                    rect = ui.allocate_exact_size(egui::vec2(180.0, 80.0), egui::Sense::hover()).0;
                });
            }));
            rect.min
        };
        frame("relative", &mut relative, vec![]);
        frame("relative", &mut relative, vec![]);
        let press = egui::pos2(160.0, 170.0);
        let release = egui::pos2(300.0, 300.0);
        frame(
            "relative",
            &mut relative,
            vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame("relative", &mut relative, vec![egui::Event::PointerMoved(release)]);
        frame(
            "relative",
            &mut relative,
            vec![egui::Event::PointerButton {
                pos: release,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let saved = relative;
        assert!(saved[0] > 200.0 && saved[1] > 200.0, "the test must actually drag the panel");
        frame("standings", &mut standings, vec![]);
        frame("standings", &mut standings, vec![]);
        let returned = frame("relative", &mut relative, vec![]);
        assert!((returned - egui::pos2(saved[0], saved[1])).length() < 1.0);
        // Numeric edits and a position reset also take effect after a preview.
        relative = [410.0, 360.0];
        let edited = frame("relative", &mut relative, vec![]);
        assert!((edited - egui::pos2(410.0, 360.0)).length() < 1.0);
    }
}
