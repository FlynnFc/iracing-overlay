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

use egui::{Color32, Context, RichText, Slider, Stroke, Ui};

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

/// Settings use their own opaque graphite surfaces so the game behind the
/// window cannot interfere with controls. These are local to this window.
const WINDOW_SIZE: [f32; 2] = [1000.0, 680.0];
const RAIL_WIDTH: f32 = 166.0;
const RAIL_GAP: f32 = 20.0;
const PAGE_HEIGHT: f32 = 580.0;
pub(super) const WINDOW_BG: Color32 = Color32::from_rgb(21, 24, 29);
pub(super) const CONTENT_BG: Color32 = Color32::from_rgb(28, 32, 38);
const CONTROL_BG: Color32 = Color32::from_rgb(42, 47, 55);
pub(super) const TEXT: Color32 = Color32::from_rgb(231, 235, 241);
pub(super) const MUTED: Color32 = Color32::from_rgb(146, 156, 171);
const EDGE: Color32 = Color32::from_rgba_premultiplied(16, 16, 16, 16);

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
        Self::BlackBox,
        Self::Relative,
        Self::Standings,
        Self::RadarBars,
        Self::FasterClass,
        Self::PitStall,
        Self::Logos,
        Self::Binds,
        Self::TeamSync,
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
            Self::Binds => "Controls",
            Self::Launcher => "Launcher",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::General => "Make the overlay feel at home in your cockpit.",
            Self::Relative => "The cars around you, with just the detail you need.",
            Self::Standings => "Follow the field, your class and the race strategy.",
            Self::Logos => "Choose how each manufacturer appears on track.",
            Self::RadarBars => "Tune your view of the space beside the car.",
            Self::FasterClass => "Know when quicker traffic is approaching.",
            Self::PitStall => "A clear guide to your marks in the pit box.",
            Self::BlackBox => "Set your fuel strategy and pit information.",
            Self::TeamSync => "Keep your crew connected throughout the race.",
            Self::Binds => "Keep the controls you need within reach.",
            Self::Launcher => "Bring your race-day apps together.",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Relative => "relative",
            Self::Standings => "standings",
            Self::Logos => "logos",
            Self::RadarBars => "radar-bars",
            Self::FasterClass => "faster-class",
            Self::PitStall => "pit-stall",
            Self::BlackBox => "black-box",
            Self::TeamSync => "team-sync",
            Self::Binds => "binds",
            Self::Launcher => "launcher",
        }
    }
}

/// The window's own state: whether it is up, and which page it is on.
#[derive(Debug, Default)]
pub struct SettingsWindow {
    /// Re-anchor after the native overlay expands from its startup size.
    last_screen_size: Option<egui::Vec2>,
    /// Whether the window is on screen. Set by the tray; cleared by the
    /// window's own close button or Escape.
    pub open: bool,
    page: Page,
    /// Optional section for reproducible demo screenshots. Applied once.
    demo_tab: Option<PanelTab>,
    /// The action whose **Bind…** is waiting for a press, and since when.
    capturing: Option<(Action, Instant)>,
    /// The brand whose row is open on the Logos page.
    selected_brand: Option<String>,
    /// The Launcher page's list and background work; the app asks it to
    /// write once the settings have settled.
    pub launcher: launcher_page::LauncherPage,
}

impl SettingsWindow {
    pub fn preview_blackbox_page(&self) -> Option<super::blackbox::Page> {
        if !self.open {
            return None;
        }
        match self.page {
            Page::Relative => Some(super::blackbox::Page::Relative),
            Page::BlackBox => Some(super::blackbox::Page::Fuel),
            _ => None,
        }
    }

    pub fn preview_panel(&self) -> Option<crate::tray::Panel> {
        if !self.open {
            return None;
        }
        match self.page {
            Page::Relative | Page::BlackBox => Some(crate::tray::Panel::Relative),
            Page::Standings => Some(crate::tray::Panel::Standings),
            Page::RadarBars => Some(crate::tray::Panel::RadarBars),
            Page::FasterClass => Some(crate::tray::Panel::FasterClass),
            Page::PitStall => Some(crate::tray::Panel::PitStall),
            _ => None,
        }
    }

    /// Opens a settings page for a demo preview without changing configuration.
    /// Unknown names leave the existing window state untouched.
    pub fn open_demo_page(&mut self, name: &str) -> bool {
        let (name, section) = name.split_once('/').unwrap_or((name, "layout"));
        let tab = match section {
            "layout" => PanelTab::Layout,
            "content" => PanelTab::Content,
            "columns" | "pages" => PanelTab::Columns,
            _ => return false,
        };
        let Some(page) = Page::ALL.into_iter().find(|page| page.slug() == name) else {
            return false;
        };
        self.page = page;
        self.demo_tab = Some(tab);
        self.open = true;
        true
    }
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
#[expect(clippy::too_many_lines, reason = "the window shell keeps navigation, content and footer in one layout")]
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
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("settings-watching"), watching));
    if let Some(tab) = window.demo_tab.take() {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(("settings-tab", window.page.slug())), tab));
    }
    let mut open = true;
    let mut close_clicked = false;
    let screen = ctx.screen_rect();
    let width = WINDOW_SIZE[0].min((screen.width() - 64.0).max(280.0));
    let page_height = PAGE_HEIGHT.min((screen.height() - 224.0).max(100.0));
    let compact = width < 660.0;
    let default_pos = egui::pos2((screen.right() - width - 56.0).max(8.0), 40.0);
    let frame =
        egui::Frame::none().fill(WINDOW_BG).stroke(Stroke::new(1.0, EDGE)).rounding(16.0).inner_margin(20.0).shadow(
            egui::epaint::Shadow {
                offset: egui::vec2(0.0, 12.0),
                blur: 36.0,
                spread: 0.0,
                color: Color32::from_black_alpha(120),
            },
        );
    let screen_changed = window.last_screen_size != Some(screen.size());
    window.last_screen_size = Some(screen.size());
    let settings = egui::Window::new("Race Overlay settings");
    let settings = if screen_changed { settings.current_pos(default_pos) } else { settings };
    settings
        .open(&mut open)
        .default_pos(default_pos)
        .default_width(width)
        .frame(frame)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            settings_style(ui);
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("RACE OVERLAY").size(10.0).strong().color(MUTED));
                    ui.label(RichText::new("Settings").size(25.0).strong().color(TEXT));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close_clicked = ui.add(egui::Button::new("Close").min_size(egui::vec2(64.0, 30.0))).clicked();
                });
            });
            ui.add_space(12.0);
            if compact {
                egui::ComboBox::from_id_salt("settings-page-picker")
                    .selected_text(window.page.label())
                    .width(width - 16.0)
                    .show_ui(ui, |ui| {
                        for page in Page::ALL {
                            ui.selectable_value(&mut window.page, page, page.label());
                        }
                    });
                ui.add_space(8.0);
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if !compact {
                    ui.vertical(|ui| {
                        ui.set_width(RAIL_WIDTH);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        egui::ScrollArea::vertical()
                            .id_salt("settings-navigation")
                            .max_height(page_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for page in Page::ALL {
                                    let group = match page {
                                        Page::General => Some("PREFERENCES"),
                                        Page::BlackBox => Some("ON TRACK"),
                                        Page::Logos => Some("PERSONALISE"),
                                        Page::TeamSync => Some("CONNECTED"),
                                        _ => None,
                                    };
                                    if let Some(group) = group {
                                        if page != Page::General {
                                            ui.add_space(8.0);
                                        }
                                        ui.label(RichText::new(group).size(10.0).strong().color(MUTED));
                                        ui.add_space(2.0);
                                    }
                                    if navigation_item(ui, page, window.page == page).clicked() {
                                        window.page = page;
                                    }
                                }
                            });
                    });
                    ui.add_space(RAIL_GAP);
                }
                let content_width = if compact { width } else { width - RAIL_WIDTH - RAIL_GAP };
                egui::Frame::none()
                    .fill(CONTENT_BG)
                    .stroke(Stroke::new(1.0, EDGE))
                    .rounding(12.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width((content_width - 40.0).max(160.0));
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.label(RichText::new(window.page.label()).size(24.0).strong().color(TEXT));
                            ui.add(
                                egui::Label::new(RichText::new(window.page.subtitle()).size(13.0).color(MUTED)).wrap(),
                            );
                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(8.0);
                            let tab = match window.page {
                                Page::Relative | Page::Standings => panel_tabs(ui, window.page.slug(), Some("Columns")),
                                Page::BlackBox => panel_tabs(ui, window.page.slug(), Some("Pages")),
                                Page::RadarBars | Page::FasterClass | Page::PitStall => {
                                    panel_tabs(ui, window.page.slug(), None)
                                }
                                _ => PanelTab::Layout,
                            };
                            // Each page owns its scroll position. The fixed viewport keeps
                            // the close button and navigation stable on longer pages.
                            egui::ScrollArea::vertical()
                                .id_salt(("settings-content", window.page.slug(), tab))
                                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                                .max_height((page_height - 156.0).max(80.0))
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_width((content_width - 56.0).max(144.0));
                                    let page_outcome = ui
                                        .push_id((window.page.slug(), tab), |ui| match window.page {
                                            Page::General => general(ui, config, watching, window),
                                            Page::Relative => relative(ui, &mut config.relative, tab),
                                            Page::Standings => standings(ui, &mut config.standings, tab),
                                            Page::Logos => logos_page(ui, &mut config.logos, window),
                                            Page::RadarBars => radar(ui, &mut config.radar, tab),
                                            Page::FasterClass => faster_class(ui, &mut config.faster_class, tab),
                                            Page::PitStall => pit_stall(ui, &mut config.pit_stall, tab),
                                            Page::BlackBox => black_box(ui, config, tab),
                                            Page::TeamSync => team_sync(ui, &mut config.sync, host),
                                            Page::Binds => binds(ui, config, window, actions, now),
                                            Page::Launcher => launcher_page::draw(ui, &mut window.launcher, now),
                                        })
                                        .inner;
                                    outcome.changed |= page_outcome.changed;
                                    outcome.reset_positions |= page_outcome.reset_positions;
                                });
                        });
                    });
            });
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 2.5, super::SIGNAL);
                ui.label(RichText::new("Changes apply instantly and save automatically.").size(12.0).color(MUTED));
                ui.label(RichText::new("Close to return input to iRacing.").size(12.0).color(MUTED));
            });
        });
    open &= !close_clicked;
    // Escape closes it, the same as its own button.
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    window.open = open;
    outcome
}

/// Style mutations are inherited only by this window's child UIs.
pub(super) fn settings_style(ui: &mut Ui) {
    let style = ui.style_mut();
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.slider_width = 150.0;
    style.spacing.text_edit_width = 310.0;
    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.override_text_color = None;
    visuals.extreme_bg_color = WINDOW_BG;
    visuals.faint_bg_color = Color32::from_white_alpha(4);
    visuals.code_bg_color = WINDOW_BG;
    visuals.slider_trailing_fill = true;
    visuals.selection.bg_fill = Color32::from_rgb(64, 73, 85);
    visuals.selection.stroke = Stroke::new(1.0, TEXT);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.rounding = egui::Rounding::same(6.0);
        widget.bg_fill = CONTROL_BG;
        widget.weak_bg_fill = CONTROL_BG;
        widget.bg_stroke = Stroke::new(1.0, EDGE);
        widget.fg_stroke = Stroke::new(1.5, TEXT);
        widget.expansion = 0.0;
    }
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, EDGE);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(55, 62, 73);
    visuals.widgets.hovered.weak_bg_fill = visuals.widgets.hovered.bg_fill;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(45));
    visuals.widgets.active.bg_fill = Color32::from_rgb(69, 79, 93);
    visuals.widgets.active.weak_bg_fill = visuals.widgets.active.bg_fill;
}

fn navigation_item(ui: &mut Ui, page: Page, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(rect, 6.0, if selected { Color32::from_rgb(64, 73, 85) } else { CONTROL_BG });
    }
    let inset = if page == Page::Relative { 26.0 } else { 12.0 };
    ui.painter().text(
        egui::pos2(rect.left() + inset, rect.center().y),
        egui::Align2::LEFT_CENTER,
        page.label(),
        egui::FontId::proportional(14.0),
        TEXT,
    );
    response
}

fn section_label(ui: &mut Ui, label: &str) {
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(RichText::new(label).size(11.0).strong().color(MUTED));
    ui.add_space(2.0);
}

fn general(ui: &mut Ui, config: &mut OverlayConfig, watching: bool, window: &mut SettingsWindow) -> Outcome {
    let mut outcome = Outcome::default();
    section_label(ui, "YOUR WIDGETS");
    let enabled = crate::tray::Panel::ALL.into_iter().filter(|panel| config.panel_visible(*panel)).count();
    ui.small(format!("{enabled} of 5 enabled. Configure a widget to preview it on its own."));
    for panel in crate::tray::Panel::ALL {
        ui.push_id(panel.label(), |ui| {
            egui::Frame::none().fill(WINDOW_BG).rounding(6.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width((ui.available_width() - 16.0).max(160.0));
                ui.horizontal(|ui| {
                    outcome.changed |= ui.checkbox(config.panel_visible_mut(panel), panel.label()).changed();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Configure").clicked() {
                            window.page = match panel {
                                crate::tray::Panel::Relative => Page::BlackBox,
                                crate::tray::Panel::Standings => Page::Standings,
                                crate::tray::Panel::RadarBars => Page::RadarBars,
                                crate::tray::Panel::FasterClass => Page::FasterClass,
                                crate::tray::Panel::PitStall => Page::PitStall,
                            };
                        }
                    });
                });
            });
        });
    }
    section_label(ui, "VISIBILITY & DETAILS");
    outcome.changed |= ui
        .checkbox(&mut config.only_show_when_iracing_focused, "Only show while iRacing is focused")
        .on_hover_text("Hides every panel while you are alt-tabbed into another app.")
        .changed();
    outcome.changed |= ui
        .checkbox(&mut config.hide_in_garage, "Hide in the garage")
        .on_hover_text("Drops the panels while the car is in the garage, so they don't cover the setup screen.")
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
    section_label(ui, "LAYOUT");
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

fn position(ui: &mut Ui, driving: &mut [f32; 2], watching_pos: &mut Option<[f32; 2]>) -> bool {
    let watching = ui.ctx().data(|data| data.get_temp::<bool>(egui::Id::new("settings-watching"))).unwrap_or(false);
    let mut value = if watching { watching_pos.unwrap_or(*driving) } else { *driving };
    let pos = &mut value;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Position");
        changed |= ui.add(egui::DragValue::new(&mut pos[0]).prefix("X ").speed(1.0)).changed();
        changed |= ui.add(egui::DragValue::new(&mut pos[1]).prefix("Y ").speed(1.0)).changed();
    });
    ui.label(
        RichText::new(if watching {
            "Preview only this widget / Watching layout"
        } else {
            "Preview only this widget / Driving layout"
        })
        .small()
        .color(super::SIGNAL),
    );
    ui.label(
        egui::RichText::new(
            "Drag the preview on screen or enter coordinates. Its position is kept when you switch pages.",
        )
        .small()
        .color(MUTED),
    );
    if changed {
        if watching {
            *watching_pos = Some(value);
        } else {
            *driving = value;
        }
    }
    changed
}

/// Draw the arrows ourselves: embedded fonts need not contain arrow glyphs.
fn order_arrow(ui: &mut Ui, up: bool, enabled: bool) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let response = ui.add(egui::Button::new("").min_size(egui::vec2(30.0, 28.0)));
        let center = response.rect.center();
        let direction = if up { -1.0 } else { 1.0 };
        let stroke = ui.style().interact(&response).fg_stroke;
        ui.painter().line_segment([center + egui::vec2(0.0, -5.0), center + egui::vec2(0.0, 5.0)], stroke);
        ui.painter()
            .line_segment([center + egui::vec2(-4.0, -direction), center + egui::vec2(0.0, 5.0 * direction)], stroke);
        ui.painter()
            .line_segment([center + egui::vec2(4.0, -direction), center + egui::vec2(0.0, 5.0 * direction)], stroke);
        response.on_hover_text(if up { "Move up (earlier)" } else { "Move down (later)" })
    })
    .inner
}

fn reorder<T: Copy + PartialEq>(ui: &mut Ui, order: &mut [T], label: impl Fn(T) -> &'static str) -> bool {
    order_rows(ui, order, |ui, item| {
        ui.label(label(item));
        false
    })
}

fn order_rows<T: Copy>(ui: &mut Ui, order: &mut [T], mut content: impl FnMut(&mut Ui, T) -> bool) -> bool {
    let mut movement = None;
    let mut changed = false;
    for index in 0..order.len() {
        ui.push_id(index, |ui| {
            egui::Frame::none().fill(WINDOW_BG).rounding(6.0).inner_margin(6.0).show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 12.0).max(120.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{:02}", index + 1)).color(MUTED));
                    if order_arrow(ui, true, index > 0).clicked() {
                        movement = Some((index, index - 1));
                    }
                    if order_arrow(ui, false, index + 1 < order.len()).clicked() {
                        movement = Some((index, index + 1));
                    }
                    changed |= content(ui, order[index]);
                });
            });
        });
    }
    if let Some((from, to)) = movement {
        order.swap(from, to);
        changed = true;
    }
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
enum PanelTab {
    #[default]
    Layout,
    Content,
    Columns,
}

fn panel_tabs(ui: &mut Ui, page: &str, columns: Option<&str>) -> PanelTab {
    let id = egui::Id::new(("settings-tab", page));
    let mut tab = ui.ctx().data(|data| data.get_temp::<PanelTab>(id)).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut tab, PanelTab::Layout, "Layout");
        ui.selectable_value(&mut tab, PanelTab::Content, "Content");
        if let Some(label) = columns {
            ui.selectable_value(&mut tab, PanelTab::Columns, label);
        }
    });
    ui.ctx().data_mut(|data| data.insert_temp(id, tab));
    ui.add_space(12.0);
    tab
}

fn relative(ui: &mut Ui, config: &mut RelativeConfig, tab: PanelTab) -> Outcome {
    let mut outcome = Outcome::default();
    if tab == PanelTab::Layout {
        section_label(ui, "DISPLAY & POSITION");
        outcome.changed |= ui.checkbox(&mut config.visible, "Show during normal use").changed();
        outcome.changed |= scale(ui, &mut config.scale);
        outcome.changed |= position(ui, &mut config.pos, &mut config.watch_pos);
        outcome.changed |= ui
            .add(Slider::new(&mut config.width, super::relative::WIDTH_RANGE).step_by(10.0).suffix(" px").text("Width"))
            .on_hover_text(
                "The card's width before scaling; the design's own is 690. Narrower trims the name column — \
             long names gain an ellipsis — while the gap and badge columns keep their size.",
            )
            .changed();
    }
    if tab == PanelTab::Content {
        outcome.changed |=
            ui.add(Slider::new(&mut config.ahead_count, RELATIVE_COUNT_RANGE).text("Cars ahead")).changed();
        outcome.changed |=
            ui.add(Slider::new(&mut config.behind_count, RELATIVE_COUNT_RANGE).text("Cars behind")).changed();
        ui.add_space(8.0);
        section_label(ui, "DRIVER DETAILS");
        outcome.changed |= ui.checkbox(&mut config.show_flags, "Driver flags").changed();
        outcome.changed |= ui.checkbox(&mut config.show_off_tracks, "Off-track counts").changed();
        ui.label("Lap time calculation");
        egui::ComboBox::from_id_salt("relative-lap-metric").selected_text(config.lap_metric.label()).show_ui(
            ui,
            |ui| {
                for metric in crate::config::RelativeLapMetric::ALL {
                    outcome.changed |= ui.selectable_value(&mut config.lap_metric, metric, metric.label()).changed();
                }
            },
        );
    }
    if tab == PanelTab::Columns {
        ui.small("Order reads left to right on the overlay. Use the arrows to move a column.");
        section_label(ui, "COLUMN ORDER");
        ui.small("Tick optional columns to show them. Position, driver and gap are always shown.");
        let mut order = config.ordered_columns();
        outcome.changed |= order_rows(ui, &mut order, |ui, column| {
            use crate::config::RelativeColumn;
            let visible = match column {
                RelativeColumn::CarNumber => Some(&mut config.show_car_number),
                RelativeColumn::Manufacturer => Some(&mut config.show_brand),
                RelativeColumn::LapTime => Some(&mut config.show_recent_lap),
                RelativeColumn::Rating => Some(&mut config.show_irating),
                _ => None,
            };
            if let Some(visible) = visible {
                ui.checkbox(visible, column.label()).changed()
            } else {
                ui.label(column.label()).on_hover_text("Always shown");
                false
            }
        });
        config.column_order = order;
    }
    if reset_page(ui) {
        // Both layouts' positions survive a page reset: the button is about
        // this page's settings, and where the panel sits isn't one of them.
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = RelativeConfig { pos, watch_pos, ..RelativeConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn standings(ui: &mut Ui, config: &mut StandingsConfig, tab: PanelTab) -> Outcome {
    let mut outcome = Outcome::default();
    if tab == PanelTab::Layout {
        section_label(ui, "DISPLAY & POSITION");
        outcome.changed |= ui.checkbox(&mut config.visible, "Show during normal use").changed();
        outcome.changed |= scale(ui, &mut config.scale);
        outcome.changed |= position(ui, &mut config.pos, &mut config.watch_pos);
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
    }
    if tab == PanelTab::Content {
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
            for (mode, label) in [(EnduranceMode::Auto, "Auto"), (EnduranceMode::On, "On"), (EnduranceMode::Off, "Off")]
            {
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
        outcome.changed |= ui.checkbox(&mut config.show_flags, "Driver flags").changed();
        outcome.changed |= ui.checkbox(&mut config.show_off_tracks, "Off-track counts").changed();
    }
    if tab == PanelTab::Columns {
        ui.small("Order reads left to right on the overlay. Use the arrows to move a column.");
        section_label(ui, "TIMING COLUMN ORDER");
        for column in crate::config::StandingsColumn::ALL {
            if !config.column_order.contains(&column) {
                config.column_order.push(column);
            }
        }
        outcome.changed |= reorder(ui, &mut config.column_order, crate::config::StandingsColumn::label);
    }
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = StandingsConfig { pos, watch_pos, ..StandingsConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn radar(ui: &mut Ui, config: &mut RadarConfig, tab: PanelTab) -> Outcome {
    let mut outcome = Outcome::default();
    if tab == PanelTab::Layout {
        section_label(ui, "DISPLAY & POSITION");
        outcome.changed |= ui.checkbox(&mut config.visible, "Show during normal use").changed();
        outcome.changed |= scale(ui, &mut config.scale);
        outcome.changed |= position(ui, &mut config.pos, &mut config.watch_pos);
    }
    if tab == PanelTab::Content {
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
            .on_hover_text(
                "The clear channel between the two capsules — set it to frame your car's width in your view.",
            )
            .changed();
        outcome.changed |= ui.checkbox(&mut config.show_numbers, "Show gaps in metres").changed();
    }
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
fn faster_class(ui: &mut Ui, config: &mut FasterClassConfig, tab: PanelTab) -> Outcome {
    let mut outcome = Outcome::default();
    if tab == PanelTab::Layout {
        section_label(ui, "DISPLAY & POSITION");
        outcome.changed |= ui.checkbox(&mut config.visible, "Show during normal use").changed();
        outcome.changed |= scale(ui, &mut config.scale);
        outcome.changed |= position(ui, &mut config.pos, &mut config.watch_pos);
    }
    if tab == PanelTab::Content {
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
            .add(
                Slider::new(&mut config.alert_secs, ALERT_MIN_SECS..=alert_max)
                    .step_by(0.1)
                    .suffix(" s")
                    .text("Alert at"),
            )
            .on_hover_text("How close it is when the card turns red.")
            .changed();
        outcome.changed |= ui
            .checkbox(&mut config.flash, "Flash when alerting")
            .on_hover_text("Pulses the red between full and dimmed. Off, it holds steady.")
            .changed();
    }
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = FasterClassConfig { pos, watch_pos, ..FasterClassConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn pit_stall(ui: &mut Ui, config: &mut PitStallConfig, tab: PanelTab) -> Outcome {
    let mut outcome = Outcome::default();
    if tab == PanelTab::Layout {
        section_label(ui, "DISPLAY & POSITION");
        outcome.changed |= ui.checkbox(&mut config.visible, "Show during normal use").changed();
        outcome.changed |= scale(ui, &mut config.scale);
        outcome.changed |= position(ui, &mut config.pos, &mut config.watch_pos);
    }
    if tab == PanelTab::Content {
        outcome.changed |= ui
            .add(Slider::new(&mut config.range_m, PIT_STALL_RANGE_M).suffix(" m").text("Range"))
            .on_hover_text("What an empty bar means, in metres from your marks. Lower magnifies the last metre.")
            .changed();
    }
    if reset_page(ui) {
        let (pos, watch_pos) = (config.pos, config.watch_pos);
        *config = PitStallConfig { pos, watch_pos, ..PitStallConfig::default() };
        outcome.changed = true;
    }
    outcome
}

fn black_box(ui: &mut Ui, all: &mut OverlayConfig, tab: PanelTab) -> Outcome {
    let mut outcome = Outcome::default();
    if tab == PanelTab::Layout {
        ui.small("Black Box and Relative share one overlay and position.");
        outcome.changed |= ui.checkbox(&mut all.relative.visible, "Show during normal use").changed();
        outcome.changed |= scale(ui, &mut all.relative.scale);
        outcome.changed |= position(ui, &mut all.relative.pos, &mut all.relative.watch_pos);
    }
    let config = &mut all.blackbox;
    if tab == PanelTab::Content {
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
    }
    if tab == PanelTab::Columns {
        ui.small("Choose the pages available in the Black Box, in tab order. Keep at least one visible.");
        let mut order = Vec::new();
        for page in config.page_order.iter().copied().chain(super::blackbox::Page::ALL) {
            if !order.contains(&page) {
                order.push(page);
            }
        }
        outcome.changed |= order_rows(ui, &mut order, |ui, page| {
            let mut shown = !config.hidden_pages.contains(&page);
            let shown_count =
                super::blackbox::Page::ALL.into_iter().filter(|p| !config.hidden_pages.contains(p)).count();
            let can_hide = shown_count > 1 || !shown;
            if ui
                .add_enabled(can_hide, egui::Checkbox::new(&mut shown, page.tab_label()))
                .on_disabled_hover_text("At least one page must stay visible.")
                .changed()
            {
                config.hidden_pages.retain(|p| *p != page);
                if !shown {
                    config.hidden_pages.push(page);
                }
                true
            } else {
                false
            }
        });
        config.page_order = order;
    }
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
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(4.0);
    ui.button(RichText::new("Reset page to defaults").size(12.0).color(MUTED)).clicked()
}
