// Rust guideline compliant 2026-02-16

//! The settings window — every knob in `race-overlay.toml`, without the file.
//!
//! An egui window inside the overlay rather than a second program: the
//! overlay already has the context, the fonts and the panels to preview
//! against, and it flips itself to pointer-capturing whenever egui wants the
//! pointer, so a window with widgets in it is clickable with no new plumbing.
//! See `plans/settings-ui.md` for the decisions.
//!
//! Every change applies to the live config the moment it is made — the panel
//! behind the window is the preview — and the app writes the file once the
//! values stop changing. Nothing here touches the disk, with one exception:
//! the Launcher page (`ui::launcher_page`) reads and writes the launcher's
//! own list and starts programs, because that is what it is for.

use std::time::{Duration, Instant};

use egui::{Context, RichText, Slider, Ui};

use super::theme::Theme;
use super::{CAUTION, launcher_page, logos};
use crate::config::{
    BlackBoxConfig, EnduranceMode, FasterClassConfig, LogoConfig, LogoShape, LogoStyle, LogoVariant, OverlayConfig,
    PitStallConfig, RadarConfig, RelativeConfig, StandingsConfig, SyncConfig, TyreBars,
};
use crate::input::{Action, Actions};

/// How long **Bind…** waits for a press before giving the row back.
///
/// Long enough to reach for a button on the far side of the wheel; short
/// enough that a driver who changed their mind is not left with a row that
/// says "press a button" for half a minute.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(8);

/// The window's size at scale 1: wide enough for a label, a slider and its
/// number on one line, tall enough for the longest page without scrolling.
const WINDOW_SIZE: [f32; 2] = [640.0, 600.0];
/// The page rail down the left, and the gap between it and the page.
const RAIL_WIDTH: f32 = 140.0;
const RAIL_GAP: f32 = 16.0;
/// The least height a page takes, so the window stays one size from page
/// to page. The longest page (Radar Bars, at nine sliders and switches)
/// fits in this with room over.
const PAGE_HEIGHT: f32 = 460.0;

/// How far a panel may be scaled either way. Half size is still legible on a
/// 4K screen; double is the most a triple-screen seating position has asked
/// for. The bounds are on the value, not the geometry — a panel scaled off
/// the screen is what **Reset all positions** is for.
const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;
/// Cars ahead/behind on the Relative. One each way is the minimum that is
/// still a relative; ten each way is a taller panel than any screen wants.
const RELATIVE_COUNT_RANGE: std::ops::RangeInclusive<u8> = 1..=10;
/// Pit-lane time loss, in seconds. Ten is a short lane at a speed limit; ninety
/// is Le Mans with a stop-and-go in it.
const PIT_LOSS_RANGE: std::ops::RangeInclusive<f32> = 10.0..=90.0;
/// The tyre-change threshold, in seconds of stationary time.
const TYRE_SECS_RANGE: std::ops::RangeInclusive<f32> = 5.0..=60.0;
/// A car's length, in metres: a Mazda MX-5 to a prototype.
const CAR_LENGTH_RANGE: std::ops::RangeInclusive<f32> = 3.0..=6.0;
/// How many car lengths each half of a radar bar covers.
const RANGE_CARS_RANGE: std::ops::RangeInclusive<f32> = 1.0..=6.0;
/// How close in relative time a car must be to register on the radar at all.
const RANGE_MS_RANGE: std::ops::RangeInclusive<f32> = 100.0..=2000.0;
/// One radar capsule's width and height, in design pixels (the panel's own
/// Scale multiplies on top). The floors stay above the hand-edit guards in
/// `radar_bars`; the ceilings are as much bar as a 1440p screen wants.
const BAR_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 60.0..=160.0;
const BAR_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 300.0..=1000.0;
/// The channel between the two capsules, in design pixels: from touching to
/// wider than a triple-screen cockpit view.
const BAR_GAP_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1200.0;
/// What an empty Pit Stall bar means, in metres.
const PIT_STALL_RANGE_M: std::ops::RangeInclusive<f32> = 10.0..=200.0;
/// How far behind a quicker class is when the Faster Class card appears, in
/// seconds. One is already on the bumper; the top is as far as the telemetry
/// thread scans.
const WARN_SECS_RANGE: std::ops::RangeInclusive<f32> = 1.0..=crate::telemetry::faster_class::MAX_WARN_SECS;
/// The least the alert can be set to: any closer and the car is alongside,
/// which is a state of its own.
const ALERT_MIN_SECS: f32 = 0.5;
/// Laps of fuel in hand for Auto Fuel — the same ceiling the wheel stepper has.
const FUEL_MARGIN_RANGE: std::ops::RangeInclusive<f32> = 0.0..=super::blackbox::pages::MAX_MARGIN_LAPS;

/// A tile in the Logos page's brand grid: big enough that a wordmark reads,
/// small enough that nine fit a row. The mark is inset by the padding, on
/// the same ground colour the widgets draw on, so it looks as it will.
const LOGO_TILE: f32 = 44.0;
const LOGO_TILE_PADDING: f32 = 6.0;
const LOGO_TILE_GAP: f32 = 6.0;
/// Text size of a tile's abbreviation fallback.
const LOGO_TILE_TEXT: f32 = 12.0;
/// The grid scrolls inside this, leaving room for the style row above and
/// the selected brand's row below within the fixed page height.
const LOGO_GRID_HEIGHT: f32 = 190.0;

/// One page of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    General,
    Relative,
    Standings,
    Logos,
    RadarBars,
    FasterClass,
    PitStall,
    BlackBox,
    TeamSync,
    Binds,
    Launcher,
}

impl Page {
    const ALL: [Self; 11] = [
        Self::General,
        Self::Relative,
        Self::Standings,
        Self::Logos,
        Self::RadarBars,
        Self::FasterClass,
        Self::PitStall,
        Self::BlackBox,
        Self::TeamSync,
        Self::Binds,
        Self::Launcher,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Relative => "Relative",
            Self::Standings => "Standings",
            Self::Logos => "Logos",
            Self::RadarBars => "Radar Bars",
            Self::FasterClass => "Faster Class",
            Self::PitStall => "Pit Stall",
            Self::BlackBox => "Black Box",
            Self::TeamSync => "Team Sync",
            Self::Binds => "Binds",
            Self::Launcher => "Launcher",
        }
    }
}

/// The window's own state: whether it is up, and which page it is on.
#[derive(Debug, Default)]
pub struct SettingsWindow {
    /// Whether the window is on screen. Set by the tray; cleared by the
    /// window's own close button or Escape.
    pub open: bool,
    page: Page,
    /// The action whose **Bind…** is waiting for a press, and since when.
    capturing: Option<(Action, Instant)>,
    /// The brand whose row is open on the Logos page.
    selected_brand: Option<String>,
    /// The Launcher page's list and background work; the app asks it to
    /// write once the settings have settled.
    pub launcher: launcher_page::LauncherPage,
}

/// What the app reports about its embedded team-sync relay, shown on the
/// Team Sync page — the app owns the relay, this window only describes it.
#[derive(Debug, Clone, Default)]
pub enum HostStatus {
    /// Not hosting, or team sync is off.
    #[default]
    Off,
    /// The embedded relay is up, bound to localhost on this port.
    Running { port: u16 },
    /// Hosting was asked for but could not start — the port is taken, or the
    /// saved invite text isn't a code.
    Failed { error: String },
}

/// What a frame of the window did to the config.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    /// Something in the config changed this frame, so a write is due once
    /// the values settle.
    pub changed: bool,
    /// Every panel's position was put back to its default. The app has to
    /// forget where egui last had each panel for that to take.
    pub reset_positions: bool,
}

/// Draws the window if it is open, applying every change straight to `config`.
///
/// `actions` is the input thread, which is where a **Bind…** capture actually
/// happens — see `input::Actions::start_capture`.
/// `watching` says the watching layout is the one on screen (and the one a
/// drag would edit), so the General page can name it — see
/// `crate::config::RelativeConfig::watch_pos`.
pub fn draw(
    ctx: &Context,
    config: &mut OverlayConfig,
    window: &mut SettingsWindow,
    actions: &Actions,
    now: Instant,
    watching: bool,
    host: &HostStatus,
) -> Outcome {
    let mut outcome = Outcome::default();
    if !window.open {
        // A capture must not outlive the window it was started from.
        if window.capturing.take().is_some() {
            actions.cancel_capture();
        }
        return outcome;
    }
    let mut open = true;
    // Centred on first open; egui remembers where it is dragged after that
    // for as long as the overlay runs.
    let screen = ctx.screen_rect();
    let default_pos = screen.center() - egui::vec2(WINDOW_SIZE[0], WINDOW_SIZE[1]) / 2.0;
    egui::Window::new("Race Overlay settings")
        .open(&mut open)
        .default_pos(default_pos)
        .default_width(WINDOW_SIZE[0])
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            // The window sizes itself to its content, and its content is
            // this: a fixed width, and a page column with a fixed minimum
            // height so switching pages doesn't resize it. Nothing in here
            // may ask for "all the available height" — inside an auto-sized
            // window that is the whole screen, which is what a vertical
            // `ui.separator()` in a horizontal layout asks for.
            ui.set_width(WINDOW_SIZE[0]);
            ui.horizontal_top(|ui| {
                let rail = ui.vertical(|ui| {
                    ui.set_width(RAIL_WIDTH);
                    ui.set_min_height(PAGE_HEIGHT);
                    for page in Page::ALL {
                        if ui.selectable_label(window.page == page, page.label()).clicked() {
                            window.page = page;
                        }
                    }
                });
                // The rail's divider, drawn to the rail's own height rather
                // than allocated — see above.
                let rail_rect = rail.response.rect;
                ui.painter().vline(
                    rail_rect.right() + RAIL_GAP / 2.0,
                    rail_rect.y_range(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                ui.add_space(RAIL_GAP);
                ui.vertical(|ui| {
                    ui.set_min_height(PAGE_HEIGHT);
                    ui.set_width(WINDOW_SIZE[0] - RAIL_WIDTH - RAIL_GAP);
                    ui.heading(window.page.label());
                    ui.add_space(6.0);
                    let page_outcome = match window.page {
                        Page::General => general(ui, config, watching),
                        Page::Relative => relative(ui, &mut config.relative),
                        Page::Standings => standings(ui, &mut config.standings),
                        Page::Logos => logos_page(ui, &mut config.logos, window),
                        Page::RadarBars => radar(ui, &mut config.radar),
                        Page::FasterClass => faster_class(ui, &mut config.faster_class),
                        Page::PitStall => pit_stall(ui, &mut config.pit_stall),
                        Page::BlackBox => black_box(ui, &mut config.blackbox),
                        Page::TeamSync => team_sync(ui, &mut config.sync, host),
                        Page::Binds => binds(ui, config, window, actions, now),
                        Page::Launcher => launcher_page::draw(ui, &mut window.launcher, now),
                    };
                    outcome.changed |= page_outcome.changed;
                    outcome.reset_positions |= page_outcome.reset_positions;
                });
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Changes apply straight away and are saved on their own. \
                         Close this window to click through to iRacing again.",
                    )
                    .small(),
                )
                .wrap(),
            );
        });
    // Escape closes it, the same as its own button.
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    window.open = open;
    outcome
}

fn general(ui: &mut Ui, config: &mut OverlayConfig, watching: bool) -> Outcome {
    let mut outcome = Outcome::default();
    ui.horizontal(|ui| {
        ui.label("Theme");
        for theme in Theme::ALL {
            outcome.changed |=
                ui.selectable_value(&mut config.theme, theme, theme.label()).on_hover_text(theme.hint()).changed();
        }
    });
    ui.small(config.theme.hint());
    ui.add_space(12.0);
    outcome.changed |= ui
        .checkbox(&mut config.only_show_when_iracing_focused, "Only show while iRacing is focused")
        .on_hover_text("Hides every panel while you are alt-tabbed into another app.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.hide_in_garage, "Hide in the garage")
        .on_hover_text("Drops the panels while the car is in the garage, so they don't cover the setup screen.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_off_tracks, "Count off-tracks")
        .on_hover_text("Tallies each car's trips off the track in the Standings and Relative gutters.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_flags, "Show driver flags")
        .on_hover_text(
            "Each driver's national flag, from their iRacing profile, before their name in the Standings and Relative.",
        )
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.stream_mode, "Stream mode (capture in OBS)")
        .on_hover_text(
            "Lets OBS list \"Race Overlay\" as a Window Capture source — use the \
             \"Windows 10 (1903 and up)\" capture method, layered over your game capture. \
             While on, the overlay also appears in the taskbar and alt-tab.",
        )
        .changed();
    // Two layouts is invisible state, so it is named the moment it exists:
    // a panel that "won't stay where I put it" after a seat change would
    // otherwise read as a bug rather than as the other layout coming back.
    if watching {
        ui.small(
            "Panels are on the watching layout — drags save to it, and your driving layout comes back in the car.",
        );
    } else if config.has_watch_layout() {
        ui.small("Panels are on the driving layout — the watching layout you saved comes back while spectating.");
    }
    ui.add_space(12.0);
    if ui
        .button("Reset all positions")
        .on_hover_text(
            "Puts every panel back where a fresh install has it, and clears the watching layout. \
             For a panel dragged off-screen.",
        )
        .clicked()
    {
        config.reset_positions();
        outcome.changed = true;
        outcome.reset_positions = true;
    }
    ui.add_space(12.0);
    ui.small(format!("Settings file: {}", crate::config::config_path().display()));
    outcome
}

fn relative(ui: &mut Ui, config: &mut RelativeConfig) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui.checkbox(&mut config.visible, "Show").changed();
    outcome.changed |= scale(ui, &mut config.scale);
    outcome.changed |= ui
        .add(Slider::new(&mut config.width, super::relative::WIDTH_RANGE).step_by(10.0).suffix(" px").text("Width"))
        .on_hover_text(
            "The card's width before scaling; the design's own is 690. Narrower trims the name column — \
             long names gain an ellipsis — while the gap and badge columns keep their size.",
        )
        .changed();
    outcome.changed |= ui.add(Slider::new(&mut config.ahead_count, RELATIVE_COUNT_RANGE).text("Cars ahead")).changed();
    outcome.changed |=
        ui.add(Slider::new(&mut config.behind_count, RELATIVE_COUNT_RANGE).text("Cars behind")).changed();
    ui.add_space(8.0);
    ui.label("Columns");
    outcome.changed |= ui
        .checkbox(&mut config.show_car_number, "Car number")
        .on_hover_text("The #number between the class bar and the name. Off, names move left into its place.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_brand, "Manufacturer mark")
        .on_hover_text("The car maker's mark after the driver's name.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_recent_lap, "Recent lap")
        .on_hover_text("Each car's best recent lap time, after the name and mark.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_irating, "iRating badge")
        .on_hover_text("The white chip before the gap: iRating, its projected change, and the license class border.")
        .changed();
    if reset_page(ui) {
        // Both layouts' positions survive a page reset: the button is about
        // this page's settings, and where the panel sits isn't one of them.
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = RelativeConfig { pos, watch_pos, ..RelativeConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn standings(ui: &mut Ui, config: &mut StandingsConfig) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui.checkbox(&mut config.visible, "Show").changed();
    outcome.changed |= scale(ui, &mut config.scale);
    outcome.changed |= ui
        .add(
            Slider::new(&mut config.name_width, super::standings::NAME_WIDTH_RANGE)
                .step_by(10.0)
                .suffix(" px")
                .text("Name column"),
        )
        .on_hover_text(
            "The driver band's width before scaling; the design's own is 440. Narrower trims the room \
             names get — long names gain an ellipsis — while the timing columns keep their size.",
        )
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_other_classes, "Show other classes' leaders")
        .on_hover_text("A section per class you aren't in, each showing just its leader.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_stint_laps, "Stint laps beside the bar")
        .on_hover_text("Prints each car's current stint length, in laps, next to its stint bar in endurance mode.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_tyres, "Tyre compound column")
        .on_hover_text(
            "Each car's compound letter in a circle after the timing columns — ringed blue on wets. \
             Only appears when the sim publishes compounds for the session.",
        )
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.show_position_change, "Position change since start")
        .on_hover_text(
            "A \u{25B2}/\u{25BC} marker beside each position: places gained or lost since the race began. \
             Races only.",
        )
        .changed();
    ui.horizontal(|ui| {
        ui.label("Endurance columns");
        for (mode, label) in [(EnduranceMode::Auto, "Auto"), (EnduranceMode::On, "On"), (EnduranceMode::Off, "Off")] {
            outcome.changed |= ui.selectable_value(&mut config.endurance_mode, mode, label).changed();
        }
    });
    outcome.changed |= ui
        .add(Slider::new(&mut config.pit_loss_secs, PIT_LOSS_RANGE).suffix(" s").text("Pit-lane time loss"))
        .on_hover_text("Used to project positions after a stop. Takes effect the next time the overlay starts.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.tyre_change_secs, TYRE_SECS_RANGE).suffix(" s").text("Tyre-change threshold"))
        .on_hover_text("A stop at least this long is assumed to have included tyres.")
        .changed();
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = StandingsConfig { pos, watch_pos, ..StandingsConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn radar(ui: &mut Ui, config: &mut RadarConfig) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui.checkbox(&mut config.visible, "Show").changed();
    outcome.changed |= scale(ui, &mut config.scale);
    outcome.changed |= ui
        .add(Slider::new(&mut config.car_length_m, CAR_LENGTH_RANGE).suffix(" m").text("Car length"))
        .on_hover_text("The scale the bars are drawn at, and where overlap begins.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.range_cars, RANGE_CARS_RANGE).text("Range (car lengths)"))
        .on_hover_text("How much track each half of a bar covers.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.range_ms, RANGE_MS_RANGE).suffix(" ms").text("Range (time)"))
        .on_hover_text("How close in relative time a car must be to register at all. Takes effect the next time the overlay starts.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.bar_size[0], BAR_WIDTH_RANGE).step_by(5.0).suffix(" px").text("Bar width"))
        .on_hover_text("How thick each capsule is.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.bar_size[1], BAR_HEIGHT_RANGE).step_by(5.0).suffix(" px").text("Bar height"))
        .on_hover_text("How long each capsule is. The configured range still has to fit, so a shorter bar draws every car proportionally smaller.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.bar_gap, BAR_GAP_RANGE).step_by(5.0).suffix(" px").text("Bar gap"))
        .on_hover_text("The clear channel between the two capsules — set it to frame your car's width in your view.")
        .changed();
    outcome.changed |= ui.checkbox(&mut config.show_numbers, "Show gaps in metres").changed();
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = RadarConfig { pos, watch_pos, ..RadarConfig::default() };
        outcome.changed = true;
    }
    outcome
}

/// The Faster Class page: the two gaps that are the whole customisation.
///
/// Both apply as the slider moves — the thresholds are read on the UI thread
/// against a list the telemetry thread already sends — so the card behind the
/// window is the preview, in layout mode or with a quicker car actually there.
fn faster_class(ui: &mut Ui, config: &mut FasterClassConfig) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui.checkbox(&mut config.visible, "Show").changed();
    outcome.changed |= scale(ui, &mut config.scale);
    outcome.changed |= ui
        .add(Slider::new(&mut config.warn_secs, WARN_SECS_RANGE).step_by(0.1).suffix(" s").text("Warn at"))
        .on_hover_text("How far behind a car from a quicker class is, in seconds of gap, when the card appears.")
        .changed();
    // The alert escalates the warning, so its slider stops where the warning
    // is set; an alert already past that point is pulled back to it.
    let alert_max = config.warn_secs.max(ALERT_MIN_SECS);
    let clamped = config.alert_secs.clamp(ALERT_MIN_SECS, alert_max);
    if (clamped - config.alert_secs).abs() > f32::EPSILON {
        config.alert_secs = clamped;
        outcome.changed = true;
    }
    outcome.changed |= ui
        .add(Slider::new(&mut config.alert_secs, ALERT_MIN_SECS..=alert_max).step_by(0.1).suffix(" s").text("Alert at"))
        .on_hover_text("How close it is when the card turns red.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.flash, "Flash when alerting")
        .on_hover_text("Pulses the red between full and dimmed. Off, it holds steady.")
        .changed();
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = FasterClassConfig { pos, watch_pos, ..FasterClassConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn pit_stall(ui: &mut Ui, config: &mut PitStallConfig) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui.checkbox(&mut config.visible, "Show").changed();
    outcome.changed |= scale(ui, &mut config.scale);
    outcome.changed |= ui
        .add(Slider::new(&mut config.range_m, PIT_STALL_RANGE_M).suffix(" m").text("Range"))
        .on_hover_text("What an empty bar means, in metres from your marks. Lower magnifies the last metre.")
        .changed();
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = PitStallConfig { pos, watch_pos, ..PitStallConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn black_box(ui: &mut Ui, config: &mut BlackBoxConfig) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui
        .checkbox(&mut config.auto_fuel, "Auto Fuel")
        .on_hover_text("Keeps the fuel load set to whatever finishes the race, plus the margin below.")
        .changed();
    outcome.changed |= ui
        .add(Slider::new(&mut config.fuel_margin_laps, FUEL_MARGIN_RANGE).step_by(0.1).text("Fuel margin (laps)"))
        .changed();
    ui.horizontal(|ui| {
        ui.label("Tyre bars");
        for (bars, label) in [(TyreBars::Temps, "Temps"), (TyreBars::Wear, "Wear")] {
            outcome.changed |= ui.selectable_value(&mut config.tyre_bars, bars, label).changed();
        }
    })
    .response
    .on_hover_text("What the three bars on each wheel of the Tires page show, from the last stop.");
    if reset_page(ui) {
        *config = BlackBoxConfig::default();
        outcome.changed = true;
    }
    outcome
}

/// The Team Sync page: the connection, hosting, and the one consent that
/// matters.
///
/// One member ticks **Host** and the overlay runs the relay itself; everyone
/// else fills in that member's funnel URL. The invite code is one field
/// serving both sides — the host's relay verifies against it, a joiner's
/// client presents it. Pit control is the driver-side consent gate on
/// crew-chief writes, off by default and armed deliberately.
fn team_sync(ui: &mut Ui, config: &mut SyncConfig, host: &HostStatus) -> Outcome {
    let mut outcome = Outcome::default();
    outcome.changed |= ui
        .checkbox(&mut config.enabled, "Team sync")
        .on_hover_text(
            "Shares fuel, tyre and strategy data with teammates in the same session, \
             and catches you up after a disconnect. Needs the relay details below.",
        )
        .changed();
    ui.add_space(8.0);

    outcome.changed |= ui
        .checkbox(&mut config.host, "Host the relay from this PC")
        .on_hover_text(
            "Runs the team's relay inside this overlay — no terminal, no separate program. \
             It binds to this machine only; teammates reach it through your Tailscale Funnel.",
        )
        .changed();
    if config.host {
        ui.horizontal(|ui| {
            ui.label("Port");
            outcome.changed |= ui
                .add(egui::DragValue::new(&mut config.host_port).range(1024..=u16::MAX))
                .on_hover_text("Fixed so your funnel command keeps working across restarts.")
                .changed();
        });
        match host {
            HostStatus::Off => {
                if !config.enabled {
                    ui.colored_label(CAUTION, "Waiting for Team sync to be switched on above.");
                }
            }
            HostStatus::Running { port } => {
                ui.label(RichText::new(format!("Relay running on 127.0.0.1:{port}.")).small().color(super::SIGNAL));
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "Publish it once, from any terminal:  tailscale funnel {port}\n\
                             Teammates put the printed https address in their Relay URL as wss://, \
                             plus your invite code below. The relay runs until the overlay closes; \
                             moving it to another port takes a restart.",
                        ))
                        .small(),
                    )
                    .wrap(),
                );
            }
            HostStatus::Failed { error } => {
                ui.add(egui::Label::new(RichText::new(error).small().color(CAUTION)).wrap());
            }
        }
        ui.add_space(8.0);
    }

    // Hosting joins its own relay on localhost, so the URL field is the
    // joiners' side of the handshake only.
    ui.add_enabled_ui(!config.host, |ui| {
        ui.label("Relay URL");
        outcome.changed |= ui
            .text_edit_singleline(&mut config.relay_url)
            .on_hover_text("The host's Tailscale Funnel address as wss://, or ws://host:port on a LAN.")
            .on_disabled_hover_text("While hosting, this overlay connects to its own relay automatically.")
            .changed();
    });
    ui.label("Invite code");
    outcome.changed |= ui
        .text_edit_singleline(&mut config.invite)
        .on_hover_text(
            "Everyone on the team enters the same code. A host with this left empty gets one \
             generated the moment hosting starts — hand that one out.",
        )
        .changed();
    ui.add_space(8.0);
    outcome.changed |= ui
        .checkbox(&mut config.allow_team_pit_control, "Let my team adjust my pit box")
        .on_hover_text(
            "While you drive, spectating teammates may set your fuel and tyres. Every change lands in \
             the sim's own black box where you can see and override it, with a note saying who made it. \
             Off, their requests are ignored.",
        )
        .changed();
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(
            RichText::new(
                "One member ticks Host and shares their funnel URL and invite code; everyone else \
                 just fills them in here. Changes to the URL or code take hold when sync is toggled \
                 or the next session starts.",
            )
            .small(),
        )
        .wrap(),
    );
    if reset_page(ui) {
        *config = SyncConfig::default();
        outcome.changed = true;
    }
    outcome
}

/// The Binds page: one row per black box action, with press-to-capture.
///
/// The capture itself runs on the input thread; this page only asks for one,
/// shows that it is waiting, and collects the answer. A press already bound
/// to another action is taken anyway and the clash shown on both rows — a
/// driver reassigning a button wants the new binding, and can clear the old
/// one from the row that names it.
fn binds(
    ui: &mut Ui,
    config: &mut OverlayConfig,
    window: &mut SettingsWindow,
    actions: &Actions,
    now: Instant,
) -> Outcome {
    let mut outcome = Outcome::default();

    // Collect a finished capture, or notice one that was cancelled (Escape)
    // or has run out of time.
    if let Some((action, since)) = window.capturing {
        if let Some(bind) = actions.captured() {
            config.binds.set(action, bind);
            outcome.changed = true;
            window.capturing = None;
        } else if !actions.is_capturing() {
            window.capturing = None;
        } else if now.duration_since(since) >= CAPTURE_TIMEOUT {
            actions.cancel_capture();
            window.capturing = None;
        }
    }

    ui.label("Wheel buttons, button boxes and keyboard keys. Press Bind\u{2026}, then the control.");
    ui.add_space(6.0);
    egui::Grid::new("binds").num_columns(3).spacing([12.0, 6.0]).striped(true).show(ui, |ui| {
        for action in Action::ALL {
            ui.label(action.label());
            let capturing_this = window.capturing.is_some_and(|(capturing, _)| capturing == action);
            if capturing_this {
                ui.colored_label(CAUTION, "press a button or key\u{2026} (Esc cancels)");
                if ui.button("Cancel").clicked() {
                    actions.cancel_capture();
                    window.capturing = None;
                }
            } else {
                match config.binds.get(action) {
                    Some(bind) => {
                        ui.horizontal(|ui| {
                            ui.label(bind.describe());
                            // The same control on two actions fires both; say so
                            // on each of them.
                            let clash = Action::ALL
                                .into_iter()
                                .find(|other| *other != action && config.binds.get(*other) == Some(bind));
                            if let Some(other) = clash {
                                ui.colored_label(CAUTION, format!("also {}", other.label()));
                            }
                        });
                    }
                    None => {
                        ui.label(RichText::new("unbound").weak());
                    }
                }
                ui.horizontal(|ui| {
                    // One capture at a time: a second Bind… while one is
                    // waiting would leave two rows claiming the same press.
                    let idle = window.capturing.is_none();
                    if ui.add_enabled(idle, egui::Button::new("Bind\u{2026}")).clicked() {
                        actions.start_capture();
                        window.capturing = Some((action, now));
                    }
                    if config.binds.get(action).is_some() && ui.button("Clear").clicked() {
                        config.binds.clear(action);
                        outcome.changed = true;
                    }
                });
            }
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.add(
        egui::Label::new(
            RichText::new(
                "Whatever is already held when you press Bind\u{2026} doesn't count, so a shifter resting in gear \
                 can't be the answer. Wheels plugged in after the overlay started are picked up once nothing \
                 else is open, or on the next start.",
            )
            .small(),
        )
        .wrap(),
    );
    outcome
}

/// The Logos page: the global style, and every brand drawn as it will be.
///
/// The grid goes through the same resolution the widgets use, on the same
/// ground colour, so a faint colour file looks faint here too. Clicking a
/// tile opens that brand's row, where a per-brand override is set or
/// cleared.
fn logos_page(ui: &mut Ui, config: &mut LogoConfig, window: &mut SettingsWindow) -> Outcome {
    let mut outcome = Outcome::default();
    ui.horizontal(|ui| {
        ui.label("Style");
        for style in LogoStyle::ALL {
            outcome.changed |= ui.selectable_value(&mut config.style, style, style.label()).changed();
        }
    });
    outcome.changed |= ui
        .checkbox(&mut config.colour, "Colour where it reads")
        .on_hover_text(
            "Uses a brand's colour file where it is legible on the overlay's dark ground; the rest stay white.",
        )
        .changed();
    // The grid draws through the widgets' own resolution, so the config has
    // to be in force before the first tile.
    logos::apply(config);
    ui.add_space(6.0);

    let brands = logos::known_brands();
    egui::Frame::none().fill(super::PANEL_BG).inner_margin(6.0).rounding(4.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(LOGO_GRID_HEIGHT).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(LOGO_TILE_GAP, LOGO_TILE_GAP);
                for stem in brands {
                    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(LOGO_TILE), egui::Sense::click());
                    if response.clicked() {
                        window.selected_brand = Some(stem.clone());
                    }
                    ui.painter().rect_filled(rect, 4.0, super::TILE_BG);
                    logos::draw_brand(ui, rect.shrink(LOGO_TILE_PADDING), stem, LOGO_TILE_TEXT, false);
                    if window.selected_brand.as_deref() == Some(stem.as_str()) {
                        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.5_f32, super::SIGNAL));
                    }
                    if config.overrides.contains_key(stem) {
                        ui.painter().circle_filled(rect.right_top() + egui::vec2(-5.0, 5.0), 2.5, CAUTION);
                    }
                    response.on_hover_text(logos::display_name(stem));
                }
            });
        });
    });
    ui.add_space(6.0);

    if let Some(stem) = window.selected_brand.clone() {
        let (variant, reason) = logos::resolve(config, &stem);
        ui.horizontal(|ui| {
            ui.strong(logos::display_name(&stem));
            ui.label(format!("\u{2014} {}: {}", reason.label(), variant.label()));
        });
        ui.horizontal(|ui| {
            let overridden = config.overrides.get(&stem).copied();
            for shape in LogoShape::ALL {
                let picked = overridden.is_some_and(|variant| variant.shape == shape);
                if ui.selectable_label(picked, shape.label()).clicked() {
                    let colour = overridden.is_some_and(|variant| variant.colour);
                    config.overrides.insert(stem.clone(), LogoVariant { shape, colour });
                    outcome.changed = true;
                }
            }
            let mut colour = overridden.is_some_and(|variant| variant.colour);
            if ui.add_enabled(overridden.is_some(), egui::Checkbox::new(&mut colour, "Colour")).changed()
                && let Some(variant) = config.overrides.get_mut(&stem)
            {
                variant.colour = colour;
                outcome.changed = true;
            }
            if overridden.is_some() && ui.button("Use default").clicked() {
                config.overrides.remove(&stem);
                outcome.changed = true;
            }
        });
    } else {
        ui.label(RichText::new("Click a brand to give it a variant of its own.").weak());
    }

    if reset_page(ui) {
        *config = LogoConfig::default();
        window.selected_brand = None;
        outcome.changed = true;
    }
    outcome
}

/// The scale slider every panel page has.
fn scale(ui: &mut Ui, value: &mut f32) -> bool {
    ui.add(Slider::new(value, SCALE_RANGE).step_by(0.05).text("Scale"))
        .on_hover_text("1.0 is the design's own size. Text stays sharp at any scale.")
        .changed()
}

/// The **Reset page** button at the foot of every page.
fn reset_page(ui: &mut Ui) -> bool {
    ui.add_space(12.0);
    ui.button("Reset page to defaults").clicked()
}
