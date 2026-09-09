// Rust guideline compliant 2026-02-16

//! Loading, saving, and shape of `race-overlay.toml`.
//!
//! The file lives in `%APPDATA%\race\`, not beside the executable. The exe
//! sits in `target\release`, which is build output: a `cargo clean`, or
//! deleting `target\` to force a rebuild, took every bind, panel position and
//! scale with it. An older file found beside the exe or in the working
//! directory is migrated there once, on the next run.
//!
//! Two processes write this file — the overlay itself, and
//! `race-overlay.exe --bind <action>` — and each loads its own copy at
//! startup. Every write therefore goes through [`OverlayConfig::update`],
//! which re-reads the file first, so neither can revert what the other saved
//! in the meantime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use race_tools::config::find_beside_exe_or_cwd;
use serde::{Deserialize, Serialize};

use crate::input::{Action, Bind};
use crate::tray::Panel;

/// File name this binary reads and writes.
const CONFIG_FILE_NAME: &str = "race-overlay.toml";

/// Folder under `%APPDATA%` these tools keep their settings in.
///
/// Shared with anything else in this repo that needs a per-user file, hence
/// the repo's name rather than this binary's.
const CONFIG_DIR_NAME: &str = "race";

/// Configuration for `race-overlay`, persisted across runs.
///
/// `Clone` so a save can take the whole live copy and write it over the file
/// it re-read a moment earlier — see `app::save_config`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a user-facing on/off setting in the file; a state machine would misdescribe them"
)]
pub struct OverlayConfig {
    /// Settings for the Relative widget.
    #[serde(default)]
    pub relative: RelativeConfig,
    /// Settings for the Standings widget.
    #[serde(default)]
    pub standings: StandingsConfig,
    /// Settings for the Radar Bars widget.
    #[serde(default)]
    pub radar: RadarConfig,
    /// Settings for the Pit Stall widget.
    #[serde(default)]
    pub pit_stall: PitStallConfig,
    /// Settings for the Faster Class widget.
    #[serde(default)]
    pub faster_class: FasterClassConfig,
    /// Only draw the panels while iRacing's own window has focus, so they
    /// don't visually clutter other apps (browser, Discord, etc.) while
    /// you're alt-tabbed away. The overlay stays click-through either way.
    #[serde(default = "default_true")]
    pub only_show_when_iracing_focused: bool,
    /// Process name (case-insensitive) checked against the foreground
    /// window when `only_show_when_iracing_focused` is set.
    #[serde(default = "default_iracing_process_name")]
    pub iracing_process_name: String,
    /// Hide the panels while the car is in the garage, so they don't cover
    /// the setup screen the driver is reading and clicking.
    ///
    /// Driven by `IsInGarage`; layout and demo mode override it, since
    /// positioning a widget is something you do from the garage.
    #[serde(default = "default_true")]
    pub hide_in_garage: bool,
    /// Let OBS capture the overlay's window directly.
    ///
    /// The overlay normally carries `WS_EX_TOOLWINDOW`, which keeps it out of
    /// the taskbar and alt-tab — and, as a side effect, out of OBS's Window
    /// Capture picker, which skips tool windows. Stream mode leaves that
    /// style off so a "Race Overlay" entry appears there (capture method
    /// "Windows 10 (1903 and up)"; the `BitBlt` method cannot read this
    /// window). Off by default because the same change puts the window in
    /// the taskbar and alt-tab, which is noise for anyone not streaming.
    #[serde(default)]
    pub stream_mode: bool,
    /// Wheel and keyboard binds driving the black box; see [`BindsConfig`].
    #[serde(default)]
    pub binds: BindsConfig,
    /// The black box's own settings; see [`BlackBoxConfig`].
    #[serde(default)]
    pub blackbox: BlackBoxConfig,
    /// How manufacturer marks are chosen; see [`LogoConfig`].
    #[serde(default)]
    pub logos: LogoConfig,
    /// Team sync connection settings; see [`SyncConfig`].
    #[serde(default)]
    pub sync: SyncConfig,
    /// Drivers marked as dangerous, by iRacing customer id — see
    /// [`DangerLevel`]. Persisted so a mark made in one session warns of the
    /// same driver in every future one. Keyed by cust-id, not name or
    /// car-index, because that is the one identifier stable across sessions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub danger: BTreeMap<u32, DangerLevel>,
}

/// How dangerous a marked driver is, escalating — see `plans/danger-drivers.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DangerLevel {
    /// "Give them room" — a hollow amber marker.
    Caution,
    /// "Watch this one" — a filled amber `!`.
    Warning,
    /// "Expect the worst" — a filled red `!`.
    Severe,
}

impl DangerLevel {
    /// The levels in escalating order, for the right-click menu.
    pub const ALL: [Self; 3] = [Self::Caution, Self::Warning, Self::Severe];

    /// What the context menu calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Caution => "Caution",
            Self::Warning => "Warning",
            Self::Severe => "Severe",
        }
    }

    /// The mark after picking `picked` from a menu that currently shows
    /// `current`: picking the level already set clears it, so the same click
    /// both marks and un-marks — one control, no separate "off".
    #[must_use]
    pub fn toggled(current: Option<Self>, picked: Self) -> Option<Self> {
        if current == Some(picked) { None } else { Some(picked) }
    }
}

/// Team sync settings — see `plans/team-sync.md`.
///
/// Off by default and inert until switched on: an overlay with no team to
/// sync with should never touch the network. The subsession, the player's
/// name and their customer id all come from the sim, not from here — only
/// the relay address and the invite the host handed out are configured.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncConfig {
    /// Whether to connect to a team relay at all.
    #[serde(default)]
    pub enabled: bool,
    /// The relay URL: the host's `wss://…ts.net` Funnel address for the
    /// team, or `ws://127.0.0.1:<port>` on the host's own machine.
    #[serde(default)]
    pub relay_url: String,
    /// The invite code the host handed out, matched case- and separator-
    /// insensitively by the relay.
    #[serde(default)]
    pub invite: String,
    /// Let teammates set this car's pit service — fuel and tyres — from their
    /// overlay while spectating. Off by default: a remote hand on the pit box
    /// is opted into, not discovered. Armed once per session; each applied
    /// change shows a passive "set by <name>" note rather than a prompt, so a
    /// driver mid-corner is never answering dialogs. See `plans/team-sync.md`.
    #[serde(default)]
    pub allow_team_pit_control: bool,
    /// Run the relay inside this overlay — the one member who hosts ticks
    /// this instead of running `--sync-host` in a terminal. The relay binds
    /// localhost only; teammates reach it through the host's Tailscale
    /// Funnel. While hosting, this overlay connects to its own relay and
    /// `relay_url` is ignored.
    #[serde(default)]
    pub host: bool,
    /// The port the embedded relay listens on — fixed rather than ephemeral
    /// so the `tailscale funnel <port>` command keeps working across
    /// restarts.
    #[serde(default = "default_sync_host_port")]
    pub host_port: u16,
}

/// See [`SyncConfig::host_port`] — the port `--sync-host`'s own example uses.
fn default_sync_host_port() -> u16 {
    41230
}

/// Hand-written so `host_port` defaults to the real port both here and in
/// serde's missing-field path, instead of the derive's zero.
impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay_url: String::new(),
            invite: String::new(),
            allow_team_pit_control: false,
            host: false,
            host_port: default_sync_host_port(),
        }
    }
}

impl OverlayConfig {
    /// Whether one panel is on screen, for the tray menu's tick.
    #[must_use]
    pub fn panel_visible(&self, panel: Panel) -> bool {
        match panel {
            Panel::Relative => self.relative.visible,
            Panel::Standings => self.standings.visible,
            Panel::RadarBars => self.radar.visible,
            Panel::PitStall => self.pit_stall.visible,
            Panel::FasterClass => self.faster_class.visible,
        }
    }

    /// Puts every panel back where a fresh install has it.
    ///
    /// For a panel dragged off-screen, or left on a monitor that is no longer
    /// plugged in — the one case where a saved position is worse than none.
    /// The watching layout is cleared for the same reason it exists: a
    /// `watch_pos` dragged off-screen is the same failure as a `pos`, and
    /// "back to a fresh install" means back to one layout.
    pub fn reset_positions(&mut self) {
        self.relative.pos = default_pos();
        self.standings.pos = default_standings_pos();
        self.radar.pos = default_radar_pos();
        self.pit_stall.pos = default_pit_stall_pos();
        self.faster_class.pos = default_faster_class_pos();
        for watch_pos in [
            &mut self.relative.watch_pos,
            &mut self.standings.watch_pos,
            &mut self.radar.watch_pos,
            &mut self.pit_stall.watch_pos,
            &mut self.faster_class.watch_pos,
        ] {
            *watch_pos = None;
        }
    }

    /// Whether any panel has a watching-layout position saved.
    ///
    /// What the settings window uses to decide the second layout is worth
    /// mentioning at all — until a panel has been dragged while watching,
    /// there is only one layout and nothing to explain.
    #[must_use]
    pub fn has_watch_layout(&self) -> bool {
        [
            self.relative.watch_pos,
            self.standings.watch_pos,
            self.radar.watch_pos,
            self.pit_stall.watch_pos,
            self.faster_class.watch_pos,
        ]
        .iter()
        .any(Option::is_some)
    }

    /// The `visible` flag of one panel, for the tray menu to flip.
    pub fn panel_visible_mut(&mut self, panel: Panel) -> &mut bool {
        match panel {
            Panel::Relative => &mut self.relative.visible,
            Panel::Standings => &mut self.standings.visible,
            Panel::RadarBars => &mut self.radar.visible,
            Panel::PitStall => &mut self.pit_stall.visible,
            Panel::FasterClass => &mut self.faster_class.visible,
        }
    }
}

/// Settings the black box owns rather than reads from the sim.
///
/// `PartialEq` so the app can tell whether a press changed anything worth
/// writing to disk — these are the only settings changed from the wheel
/// rather than by dragging a panel.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct BlackBoxConfig {
    /// Keep the fuel load set to whatever finishes the race, plus
    /// [`BlackBoxConfig::fuel_margin_laps`].
    ///
    /// Ours, not iRacing's: the sim's own Auto Fuel is a black box UI feature
    /// with no SDK equivalent, so this computes the same figure from measured
    /// consumption and arms it through the ordinary fuel command. Off by
    /// default — it takes the fuel decision out of the driver's hands, which
    /// should be opted into rather than discovered mid-stint.
    #[serde(default)]
    pub auto_fuel: bool,
    /// Extra laps of fuel Auto Fuel adds beyond the finish.
    #[serde(default = "default_fuel_margin_laps")]
    pub fuel_margin_laps: f32,
    /// What the three bars on each wheel of the Tires page show.
    #[serde(default)]
    pub tyre_bars: TyreBars,
    /// Page order used by both the rail and wheel navigation. Missing or
    /// duplicate entries are normalised before use.
    #[serde(default = "default_blackbox_page_order")]
    pub page_order: Vec<crate::ui::blackbox::Page>,
    /// Pages the driver has hidden. Session availability still applies;
    /// Relative is restored if no configured page can be shown.
    #[serde(default)]
    pub hidden_pages: Vec<crate::ui::blackbox::Page>,
}

impl Default for BlackBoxConfig {
    fn default() -> Self {
        Self {
            auto_fuel: false,
            fuel_margin_laps: default_fuel_margin_laps(),
            tyre_bars: TyreBars::default(),
            page_order: default_blackbox_page_order(),
            hidden_pages: Vec::new(),
        }
    }
}

fn default_blackbox_page_order() -> Vec<crate::ui::blackbox::Page> {
    crate::ui::blackbox::Page::ALL.to_vec()
}

/// What the three bars across each wheel on the Tires page read.
///
/// The sim publishes both, per tread position, latched at the last stop.
/// Which one a driver wants beside the pressure control is a matter of what
/// they are managing that stint, so it is a setting rather than a choice
/// made for them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TyreBars {
    /// Carcass temperature, judged against the tyre's own mean: which edge
    /// is doing the work.
    #[default]
    Temps,
    /// Tread remaining: how much of each position is left.
    Wear,
}

/// One lap in hand, matching the mockup and covering the usual reasons a
/// race takes longer than projected: a safety car, a slow final lap, a
/// splash of fuel burnt behind traffic.
fn default_fuel_margin_laps() -> f32 {
    1.0
}

/// What each black box action is bound to.
///
/// Every entry is optional: an unbound action simply can't be triggered,
/// which is better than refusing to start. Written by
/// `race-overlay.exe --bind <action>` rather than by hand — button numbers
/// are not something anyone should have to look up.
///
/// The keyboard defaults mirror iRacing's own: `F3` through `F8` select the
/// black box pages there, so the same keys reaching the same pages here means
/// one less thing to relearn.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BindsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_page: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub increment: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decrement: Option<Bind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle: Option<Bind>,
}

impl BindsConfig {
    /// Every bound action, in the form the input poller wants.
    #[must_use]
    pub fn pairs(&self) -> Vec<(Action, Bind)> {
        Action::ALL.into_iter().filter_map(|action| self.get(action).cloned().map(|bind| (action, bind))).collect()
    }

    /// This action's bind, if it has one.
    #[must_use]
    pub fn get(&self, action: Action) -> Option<&Bind> {
        match action {
            Action::NextPage => self.next_page.as_ref(),
            Action::PrevPage => self.prev_page.as_ref(),
            Action::Next => self.next.as_ref(),
            Action::Prev => self.prev.as_ref(),
            Action::Increment => self.increment.as_ref(),
            Action::Decrement => self.decrement.as_ref(),
            Action::Toggle => self.toggle.as_ref(),
        }
    }

    /// Removes one action's bind, leaving it unbound.
    pub fn clear(&mut self, action: Action) {
        let slot = match action {
            Action::NextPage => &mut self.next_page,
            Action::PrevPage => &mut self.prev_page,
            Action::Next => &mut self.next,
            Action::Prev => &mut self.prev,
            Action::Increment => &mut self.increment,
            Action::Decrement => &mut self.decrement,
            Action::Toggle => &mut self.toggle,
        };
        *slot = None;
    }

    /// Replaces one action's bind, for `--bind` to save.
    pub fn set(&mut self, action: Action, bind: Bind) {
        let slot = match action {
            Action::NextPage => &mut self.next_page,
            Action::PrevPage => &mut self.prev_page,
            Action::Next => &mut self.next,
            Action::Prev => &mut self.prev,
            Action::Increment => &mut self.increment,
            Action::Decrement => &mut self.decrement,
            Action::Toggle => &mut self.toggle,
        };
        *slot = Some(bind);
    }
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            relative: RelativeConfig::default(),
            standings: StandingsConfig::default(),
            radar: RadarConfig::default(),
            pit_stall: PitStallConfig::default(),
            faster_class: FasterClassConfig::default(),
            only_show_when_iracing_focused: default_true(),
            iracing_process_name: default_iracing_process_name(),
            hide_in_garage: default_true(),
            stream_mode: false,
            binds: BindsConfig::default(),
            blackbox: BlackBoxConfig::default(),
            logos: LogoConfig::default(),
            sync: SyncConfig::default(),
            danger: BTreeMap::new(),
        }
    }
}

/// Which of the manufacturer mark variants the widgets draw.
///
/// `assets/logos/` holds every brand in four shapes and two inks (see its
/// README and `plans/logo-styles.md`). `style` picks a shape for every
/// brand at once — or `Curated`, the hand-picked mix in the manifest —
/// and `colour` asks for the brand-colour file wherever the manifest says
/// it reads on the overlay's dark ground. A per-brand entry in `overrides`
/// wins over both. Keys are the file names in `assets/logos/` (`bmw`,
/// `alfa-romeo`, `mb`), values a [`LogoVariant`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct LogoConfig {
    /// The shape every brand gets, or the curated mix.
    #[serde(default)]
    pub style: LogoStyle,
    /// Prefer the brand-colour file where it is legible.
    #[serde(default)]
    pub colour: bool,
    /// Per-brand picks that win over `style` and `colour`.
    ///
    /// An entry whose value isn't a known variant is dropped on load with a
    /// note rather than failing the whole file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", deserialize_with = "deserialize_overrides")]
    pub overrides: BTreeMap<String, LogoVariant>,
}

/// The global choice on the Logos page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogoStyle {
    /// The manifest's hand-picked variant per brand — what ships.
    #[default]
    Curated,
    Icon,
    Badge,
    Horizontal,
    Wordmark,
}

impl LogoStyle {
    /// Every style, in the order the page lists them.
    pub const ALL: [Self; 5] = [Self::Curated, Self::Icon, Self::Badge, Self::Horizontal, Self::Wordmark];

    /// The shape this style forces, or `None` for the curated mix.
    #[must_use]
    pub fn shape(self) -> Option<LogoShape> {
        match self {
            Self::Curated => None,
            Self::Icon => Some(LogoShape::Icon),
            Self::Badge => Some(LogoShape::Badge),
            Self::Horizontal => Some(LogoShape::Horizontal),
            Self::Wordmark => Some(LogoShape::Wordmark),
        }
    }

    /// What the page calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Curated => "Curated",
            Self::Icon => "Icon",
            Self::Badge => "Badge",
            Self::Horizontal => "Horizontal",
            Self::Wordmark => "Wordmark",
        }
    }
}

/// The four shapes upstream draws every brand in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogoShape {
    /// The emblem alone.
    Icon,
    /// Emblem over the wordmark (upstream's "Logo").
    Badge,
    /// Emblem beside the wordmark (upstream's "Logo Horizontal").
    Horizontal,
    /// The name alone.
    Wordmark,
}

impl LogoShape {
    /// Every shape, in the order the page lists them.
    pub const ALL: [Self; 4] = [Self::Icon, Self::Badge, Self::Horizontal, Self::Wordmark];

    /// The folder under `assets/logos/` holding the white files of this shape.
    #[must_use]
    pub fn folder(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::Badge => "badge",
            Self::Horizontal => "horizontal",
            Self::Wordmark => "wordmark",
        }
    }

    /// What the page calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Icon => "Icon",
            Self::Badge => "Badge",
            Self::Horizontal => "Horizontal",
            Self::Wordmark => "Wordmark",
        }
    }

    /// Whether marks of this shape are wider than they are tall.
    #[must_use]
    pub fn is_wide(self) -> bool {
        matches!(self, Self::Horizontal | Self::Wordmark)
    }
}

/// A shape in one ink: the folder a mark file lives in.
///
/// Written as the folder name — `icon`, `icon-colour`, `badge`, … — in
/// both the manifest and the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogoVariant {
    pub shape: LogoShape,
    /// Brand colours rather than white.
    pub colour: bool,
}

/// Suffix on a folder name for the brand-colour files.
const COLOUR_SUFFIX: &str = "-colour";

impl LogoVariant {
    /// The white file of `shape`.
    #[must_use]
    pub const fn mono(shape: LogoShape) -> Self {
        Self { shape, colour: false }
    }

    /// The folder under `assets/logos/` this variant's files live in.
    #[must_use]
    pub fn folder(self) -> String {
        if self.colour { format!("{}{COLOUR_SUFFIX}", self.shape.folder()) } else { self.shape.folder().to_owned() }
    }

    /// This variant's white twin (itself, if already white).
    #[must_use]
    pub const fn as_mono(self) -> Self {
        Self { shape: self.shape, colour: false }
    }

    /// What the page calls it.
    #[must_use]
    pub fn label(self) -> String {
        if self.colour { format!("{} (colour)", self.shape.label()) } else { format!("{} (white)", self.shape.label()) }
    }

    /// Parses a folder name; `None` if it names no variant.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (shape, colour) = match text.strip_suffix(COLOUR_SUFFIX) {
            Some(shape) => (shape, true),
            None => (text, false),
        };
        let shape = LogoShape::ALL.into_iter().find(|candidate| candidate.folder() == shape)?;
        Some(Self { shape, colour })
    }
}

impl std::fmt::Display for LogoVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.folder())
    }
}

impl Serialize for LogoVariant {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.folder())
    }
}

impl<'de> Deserialize<'de> for LogoVariant {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom(format!("unknown logo variant `{text}`")))
    }
}

/// Reads `[logos.overrides]`, dropping entries that name no variant.
///
/// A typo in one brand's line is a note on the console, not every setting
/// in the file lost.
fn deserialize_overrides<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, LogoVariant>, D::Error> {
    let raw: BTreeMap<String, String> = BTreeMap::deserialize(deserializer)?;
    let mut overrides = BTreeMap::new();
    for (brand, text) in raw {
        match LogoVariant::parse(&text) {
            Some(variant) => {
                overrides.insert(brand, variant);
            }
            None => println!("note: ignoring logo override `{brand} = \"{text}\"`: not a variant"),
        }
    }
    Ok(overrides)
}

/// iRacing's current 64-bit DX11 sim executable. Configurable in case a
/// different build/name is ever used.
fn default_iracing_process_name() -> String {
    "iracingsim64dx11.exe".to_owned()
}

/// Settings for the Relative widget.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a user-facing column switch in the file; a state machine would misdescribe them"
)]
pub struct RelativeConfig {
    /// Top-left window position, in screen pixels.
    #[serde(default = "default_pos")]
    pub pos: [f32; 2],
    /// Top-left position while watching rather than driving, in screen pixels.
    ///
    /// The second layout `plans/seat-layouts.md` describes: spectating and a
    /// team-mate's stint read the screen differently from driving it, so the
    /// panels keep a place for each. `None` until this panel is first dragged
    /// while watching — [`Self::pos`] stands in until then, so the feature is
    /// invisible until used and older files load unchanged. Only positions
    /// fork; scale, visibility and every other setting are shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_pos: Option<[f32; 2]>,
    /// Whether to draw the Relative — and with it the whole black box, of
    /// which the Relative is page one. The wheel binds keep working while it
    /// is hidden; they just have nothing to show.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// How many cars ahead of the player to show.
    #[serde(default = "default_count")]
    pub ahead_count: u8,
    /// How many cars behind the player to show.
    #[serde(default = "default_count")]
    pub behind_count: u8,
    /// Size multiplier for this panel — see [`default_scale`].
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// The card's width at scale 1, in pixels — see `ui::relative`.
    ///
    /// The gap and badge columns keep their measured widths whatever this is
    /// set to; what grows and shrinks is the run the driver's name sits in,
    /// which is where the design's 690 spends most of its room. Clamped to
    /// `ui::relative::WIDTH_RANGE` at use.
    #[serde(default = "default_relative_width")]
    pub width: f32,
    /// How far the field can be scrolled from the player's own row before it
    /// stops, in rows. Generous enough to reach a class leader from mid-pack,
    /// bounded so a stuck rotary can't send the panel into empty space.
    #[serde(default = "default_scroll_limit")]
    pub scroll_limit: u8,
    /// Show the car number between the class bar and the driver's name.
    #[serde(default = "default_true")]
    pub show_car_number: bool,
    /// Show the manufacturer's mark after the driver's name.
    #[serde(default = "default_true")]
    pub show_brand: bool,
    /// Show each car's best recent lap after the name and mark.
    #[serde(default = "default_true")]
    pub show_recent_lap: bool,
    /// Show the iRating badge (with its license-color border) before the gap.
    #[serde(default = "default_true")]
    pub show_irating: bool,
    /// Show profile flags in this panel only.
    #[serde(default = "default_true")]
    pub show_flags: bool,
    /// Show off-track counts in this panel's gutter only.
    #[serde(default = "default_true")]
    pub show_off_tracks: bool,
    /// Which recent lap statistic the lap-time column shows.
    #[serde(default)]
    pub lap_metric: RelativeLapMetric,
    /// Column order, with omitted entries appended and duplicates ignored.
    #[serde(default = "default_relative_columns")]
    pub column_order: Vec<RelativeColumn>,
}

impl Default for RelativeConfig {
    fn default() -> Self {
        Self {
            pos: default_pos(),
            watch_pos: None,
            visible: true,
            ahead_count: default_count(),
            behind_count: default_count(),
            scale: default_scale(),
            width: default_relative_width(),
            scroll_limit: default_scroll_limit(),
            show_car_number: true,
            show_brand: true,
            show_recent_lap: true,
            show_irating: true,
            show_flags: true,
            show_off_tracks: true,
            lap_metric: RelativeLapMetric::default(),
            column_order: default_relative_columns(),
        }
    }
}

/// The lap-time statistic shown alongside a driver's name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelativeLapMetric {
    #[default]
    AverageLast3,
    BestLast3,
    LastLap,
}

impl RelativeLapMetric {
    pub const ALL: [Self; 3] = [Self::AverageLast3, Self::BestLast3, Self::LastLap];

    pub fn label(self) -> &'static str {
        match self {
            Self::AverageLast3 => "Average of last 3",
            Self::BestLast3 => "Best of last 3",
            Self::LastLap => "Last completed lap",
        }
    }

    /// Samples are newest first. Missing, non-finite and non-positive lap
    /// times never participate; early in a session the available laps count.
    pub fn value(self, laps: [Option<f32>; 3]) -> Option<f32> {
        let mut valid = laps.into_iter().flatten().filter(|secs| secs.is_finite() && *secs > 0.0);
        match self {
            Self::LastLap => valid.next(),
            Self::BestLast3 => valid.min_by(f32::total_cmp),
            Self::AverageLast3 => {
                let (sum, count) = valid.fold((0.0_f32, 0_u8), |(sum, count), secs| (sum + secs, count + 1));
                (count > 0).then(|| sum / f32::from(count))
            }
        }
    }
}

/// Columns that can be arranged in the Relative. Visibility remains a
/// separate choice, so hiding a column does not forget its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelativeColumn {
    Position,
    CarNumber,
    Driver,
    Manufacturer,
    LapTime,
    Rating,
    Gap,
}

impl RelativeColumn {
    pub const ALL: [Self; 7] =
        [Self::Position, Self::CarNumber, Self::Driver, Self::Manufacturer, Self::LapTime, Self::Rating, Self::Gap];

    pub fn label(self) -> &'static str {
        match self {
            Self::Position => "Position & class",
            Self::CarNumber => "Car number",
            Self::Driver => "Driver name & flag",
            Self::Manufacturer => "Manufacturer",
            Self::LapTime => "Lap time",
            Self::Rating => "iRating",
            Self::Gap => "Gap",
        }
    }
}

fn default_relative_columns() -> Vec<RelativeColumn> {
    RelativeColumn::ALL.to_vec()
}

impl RelativeConfig {
    pub fn ordered_columns(&self) -> Vec<RelativeColumn> {
        let mut order = Vec::with_capacity(RelativeColumn::ALL.len());
        for column in self.column_order.iter().copied().chain(RelativeColumn::ALL) {
            if !order.contains(&column) {
                order.push(column);
            }
        }
        order
    }

    pub fn column_visible(&self, column: RelativeColumn) -> bool {
        match column {
            RelativeColumn::CarNumber => self.show_car_number,
            RelativeColumn::Manufacturer => self.show_brand,
            RelativeColumn::LapTime => self.show_recent_lap,
            RelativeColumn::Rating => self.show_irating,
            RelativeColumn::Position | RelativeColumn::Driver | RelativeColumn::Gap => true,
        }
    }
}

/// The width the Relative's design mockup measures — see `ui::relative`.
fn default_relative_width() -> f32 {
    crate::ui::relative::DESIGN_WIDTH
}

/// Far enough to walk from mid-pack to the front of a full grid.
fn default_scroll_limit() -> u8 {
    60
}

/// Every widget is laid out at the exact pixel dimensions of its design
/// mockup, so `1.0` reproduces the intended design. This scales all of a
/// panel's sizes — text, padding, badges, icons — by the same factor, for
/// displays or seating positions where the intended size doesn't suit.
///
/// Applied by multiplying each dimension at layout time (see
/// `ui::Metrics`), not by transforming the rendered panel, so text stays
/// crisply rasterized at whatever size it ends up.
fn default_scale() -> f32 {
    1.0
}

fn default_pos() -> [f32; 2] {
    [60.0, 60.0]
}

/// Four ahead and four behind, matching the compact relative style this
/// widget mirrors.
fn default_count() -> u8 {
    4
}

fn default_true() -> bool {
    true
}

/// Reorderable timing columns. Driver identity stays in its own band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StandingsColumn {
    Gap,
    Fastest,
    Last,
}
impl StandingsColumn {
    pub const ALL: [Self; 3] = [Self::Gap, Self::Fastest, Self::Last];
    pub fn label(self) -> &'static str {
        match self {
            Self::Gap => "Gap",
            Self::Fastest => "Fastest",
            Self::Last => "Last",
        }
    }
}
fn default_standings_columns() -> Vec<StandingsColumn> {
    StandingsColumn::ALL.to_vec()
}

/// Settings for the Standings widget.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a user-facing column switch in the file; a state machine would misdescribe them"
)]
pub struct StandingsConfig {
    /// Top-left window position, in screen pixels.
    #[serde(default = "default_standings_pos")]
    pub pos: [f32; 2],
    /// Position while watching — see [`RelativeConfig::watch_pos`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_pos: Option<[f32; 2]>,
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Assumed pit-lane time loss, in seconds, used to estimate a post-pit
    /// position. Varies a lot by track and car class — tune to match yours.
    #[serde(default = "default_pit_loss_secs")]
    pub pit_loss_secs: f32,
    /// Size multiplier for this panel — see [`default_scale`].
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Whether to show the endurance columns — current stint, stops still
    /// owed, and measured pit-stop times. The strategy line across the
    /// card's foot — laps left, stops to go, net position — shows in every
    /// race whatever this is set to.
    #[serde(default)]
    pub endurance_mode: EnduranceMode,
    /// Stationary time, in seconds, at or above which a stop is assumed to
    /// have included tyres — see [`default_tyre_change_secs`].
    #[serde(default = "default_tyre_change_secs")]
    pub tyre_change_secs: f32,
    /// Whether classes other than the player's get a section — just their
    /// leader each. Off by default: the other classes are context, not the
    /// race the player is running, and every section is rows of height the
    /// panel spends over the track. The player's own class is always shown.
    #[serde(default)]
    pub show_other_classes: bool,
    /// Print each car's current stint length, in laps, beside its stint bar.
    #[serde(default = "default_true")]
    pub show_stint_laps: bool,
    /// Show each car's tyre compound at the end of the timing band: the
    /// compound's letter in a circle, ringed blue for a wet. On by default
    /// but self-hiding: the column only appears when the sim publishes a
    /// compound for at least one car in the session.
    #[serde(default = "default_true")]
    pub show_tyres: bool,
    /// Show how many places each car has gained or lost since the race
    /// began, in a slot after the position plate. Races only — the column
    /// stays out of practice and qualifying either way.
    #[serde(default = "default_true")]
    pub show_position_change: bool,
    /// The driver band's width at scale 1, in pixels — see `ui::standings`.
    ///
    /// The timing and strategy bands keep their measured widths; this is the
    /// band the names sit in, which is where the panel's spare room lives.
    /// Clamped to `ui::standings::NAME_WIDTH_RANGE` at use.
    #[serde(default = "default_standings_name_width")]
    pub name_width: f32,
    #[serde(default = "default_true")]
    pub show_flags: bool,
    #[serde(default = "default_true")]
    pub show_off_tracks: bool,
    #[serde(default = "default_standings_columns")]
    pub column_order: Vec<StandingsColumn>,
}

impl Default for StandingsConfig {
    fn default() -> Self {
        Self {
            pos: default_standings_pos(),
            watch_pos: None,
            visible: true,
            pit_loss_secs: default_pit_loss_secs(),
            scale: default_scale(),
            endurance_mode: EnduranceMode::default(),
            tyre_change_secs: default_tyre_change_secs(),
            show_other_classes: false,
            show_stint_laps: true,
            show_tyres: true,
            show_position_change: true,
            name_width: default_standings_name_width(),
            show_flags: true,
            show_off_tracks: true,
            column_order: default_standings_columns(),
        }
    }
}

/// The width the Standings' design mockup gives its driver band — see
/// `ui::standings`.
fn default_standings_name_width() -> f32 {
    crate::ui::standings::DESIGN_NAME_WIDTH
}

/// iRacing exposes no pit-service detail for cars other than the player's,
/// so whether a rival took tyres has to be inferred from how long they sat
/// still. A fuel-only splash is typically under ten seconds; adding four
/// tyres pushes a stop past twenty. Twenty seconds separates the two cleanly
/// for most GT and prototype classes — lower it for classes with quicker
/// tyre changes.
fn default_tyre_change_secs() -> f32 {
    20.0
}

/// When the Standings widget shows its endurance columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EnduranceMode {
    /// Show them once the session is seen to need more than one stop, and
    /// keep them for the rest of it.
    ///
    /// The default, because the columns are dead width in a sprint race and
    /// essential in anything longer — and which of those you're in is
    /// something the telemetry already knows.
    #[default]
    Auto,
    On,
    Off,
}

/// Settings for the Radar Bars widget.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RadarConfig {
    /// Top-left window position, in screen pixels.
    #[serde(default = "default_radar_pos")]
    pub pos: [f32; 2],
    /// Position while watching — see [`RelativeConfig::watch_pos`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_pos: Option<[f32; 2]>,
    #[serde(default = "default_true")]
    pub visible: bool,
    /// How far apart (in milliseconds of relative time) a car can be and
    /// still register on a bar. This is the span from the middle of a bar to
    /// either end.
    #[serde(default = "default_radar_range_ms")]
    pub range_ms: f32,
    /// How long a car is, in metres — see [`default_car_length_m`].
    #[serde(default = "default_car_length_m")]
    pub car_length_m: f32,
    /// How many car lengths of track each half of a bar covers — see
    /// [`default_range_cars`].
    #[serde(default = "default_range_cars")]
    pub range_cars: f32,
    /// Whether to print each marker's clear gap in metres beside it.
    ///
    /// Off by default: the whole point of drawing your own car on the bar is
    /// that the answer is a shape rather than a number, and a number invites
    /// the eye to stop and read it at exactly the wrong moment.
    #[serde(default)]
    pub show_numbers: bool,
    /// One capsule's width and height, in design pixels (multiplied by
    /// `scale` like every other dimension) — see [`default_radar_bar_size`].
    #[serde(default = "default_radar_bar_size")]
    pub bar_size: [f32; 2],
    /// The clear channel between the two capsules, in design pixels — see
    /// [`default_radar_bar_gap`].
    #[serde(default = "default_radar_bar_gap")]
    pub bar_gap: f32,
    /// Size multiplier for this panel — see [`default_scale`].
    #[serde(default = "default_scale")]
    pub scale: f32,
}

impl Default for RadarConfig {
    fn default() -> Self {
        Self {
            pos: default_radar_pos(),
            watch_pos: None,
            visible: true,
            range_ms: default_radar_range_ms(),
            car_length_m: default_car_length_m(),
            range_cars: default_range_cars(),
            show_numbers: false,
            bar_size: default_radar_bar_size(),
            bar_gap: default_radar_bar_gap(),
            scale: default_scale(),
        }
    }
}

/// One radar capsule's [width, height] in design pixels, measured off the
/// mockup. Height is also the drawing's scale: `range_cars` car lengths
/// have to fit in each half of it, so a shorter bar draws everything
/// proportionally smaller.
fn default_radar_bar_size() -> [f32; 2] {
    [100.0, 705.0]
}

/// The mockup's channel between the capsules — wide enough to read as
/// "either side of the car" rather than as a single split gauge, and to
/// leave the driver's actual car visible between them.
fn default_radar_bar_gap() -> f32 {
    465.0
}

/// Settings for the Pit Stall widget.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PitStallConfig {
    /// Top-left window position, in screen pixels.
    #[serde(default = "default_pit_stall_pos")]
    pub pos: [f32; 2],
    /// Position while watching — see [`RelativeConfig::watch_pos`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_pos: Option<[f32; 2]>,
    /// There is nothing to opt out of in the usual case — the widget hides
    /// itself for all but a few seconds a stop — so this defaults on.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// What an empty bar means, in metres from the perfect stopping point.
    /// See [`DEFAULT_RANGE_M`].
    #[serde(default = "default_pit_stall_range_m")]
    pub range_m: f32,
    /// Size multiplier for this panel — see [`default_scale`].
    #[serde(default = "default_scale")]
    pub scale: f32,
}

impl Default for PitStallConfig {
    fn default() -> Self {
        Self {
            pos: default_pit_stall_pos(),
            watch_pos: None,
            visible: true,
            range_m: default_pit_stall_range_m(),
            scale: default_scale(),
        }
    }
}

/// Settings for the Faster Class widget — see `plans/faster-class.md`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FasterClassConfig {
    /// Top-left window position, in screen pixels.
    #[serde(default = "default_faster_class_pos")]
    pub pos: [f32; 2],
    /// Position while watching — see [`RelativeConfig::watch_pos`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_pos: Option<[f32; 2]>,
    /// The widget hides itself outside the moments it is for, and never
    /// appears at all in a single-class session, so there is nothing to opt
    /// out of in the usual case: this defaults on.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Size multiplier for this panel — see [`default_scale`].
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// How many seconds behind a quicker-class car is when the card appears —
    /// see `telemetry::faster_class::DEFAULT_WARN_SECS`.
    #[serde(default = "default_warn_secs")]
    pub warn_secs: f32,
    /// How many seconds behind it is when the card turns red. Clamped to
    /// `warn_secs` at use: the alert escalates the warning, so it cannot come
    /// first — see `telemetry::faster_class::DEFAULT_ALERT_SECS`.
    #[serde(default = "default_alert_secs")]
    pub alert_secs: f32,
    /// Whether the red state flashes. Between full and dimmed rather than on
    /// and off, so it can be turned off by a driver it distracts without the
    /// alternative ever being a blank card.
    #[serde(default = "default_true")]
    pub flash: bool,
}

impl Default for FasterClassConfig {
    fn default() -> Self {
        Self {
            pos: default_faster_class_pos(),
            watch_pos: None,
            visible: true,
            scale: default_scale(),
            warn_secs: default_warn_secs(),
            alert_secs: default_alert_secs(),
            flash: true,
        }
    }
}

/// See `telemetry::faster_class::DEFAULT_WARN_SECS`.
fn default_warn_secs() -> f32 {
    crate::telemetry::faster_class::DEFAULT_WARN_SECS
}

/// See `telemetry::faster_class::DEFAULT_ALERT_SECS`.
fn default_alert_secs() -> f32 {
    crate::telemetry::faster_class::DEFAULT_ALERT_SECS
}

/// Top of a common 1920x1080 display, in the
/// eyeline, where a warning belongs, and clear of every other panel's seed.
/// A one-time seed like every panel position: wherever it is dragged to is
/// what persists.
fn default_faster_class_pos() -> [f32; 2] {
    [900.0, 60.0]
}

/// How far from the marks an empty Pit Stall bar means, in metres.
///
/// Shorter than the range the widget appears at, deliberately. The bar is a
/// closeness gauge rather than a position on the lane, and the fill is linear,
/// so this figure is the zoom: the box's own band is its half-width against
/// this — the top twenty-fifth of the capsule at the default. Stretched to the
/// full appearance range instead, that band would be a sixtieth of the bar and
/// the last few metres, which are the only ones a driver is placing the car in,
/// would be a sliver.
///
/// Set against `pit_stall::APPEAR_RANGE_M` so the level is already moving
/// through most of the time the bar is on screen: the widget arrives empty and
/// settled, then starts rising well before the braking point rather than
/// waiting until the car is nearly parked. Raise it for a bar that starts
/// moving sooner still, lower it for one that magnifies the final metre.
///
/// The bar's own height does not change with this, so raising it slows the
/// fill: the same capsule now spans twice the lane it used to, and the level
/// creeps up rather than jumping through the last stalls.
pub const DEFAULT_RANGE_M: f32 = 50.0;

/// See [`DEFAULT_RANGE_M`].
fn default_pit_stall_range_m() -> f32 {
    DEFAULT_RANGE_M
}

/// Right of centre on a common 1920x1080 display, clear of the other panels.
///
/// A tall capsule needs a column rather than a strip, and the right-hand side
/// keeps it off the Relative and Standings stack on the left while staying
/// inboard of the screen edge, where a driver looking at their crew would
/// still catch it. A one-time seed like every panel position: wherever it is
/// dragged to is what persists.
fn default_pit_stall_pos() -> [f32; 2] {
    [1680.0, 240.0]
}

// Relative and Standings are stacked in the same left-hand column with a
// generous vertical gap, since both can grow tall (many rows) but stay
// fairly narrow. None of
// this can be exact — these pure default-value functions can't know actual
// rendered content sizes ahead of time — and every position is just a
// one-time seed anyway, overwritten by wherever the user actually drags
// each panel.
fn default_standings_pos() -> [f32; 2] {
    [60.0, 520.0]
}

/// Roughly bottom-center for a common 1920x1080 display — near where a
/// driver's peripheral vision would naturally check for side traffic,
/// rather than tucked off to one side. Just a one-time seed: like every
/// panel, wherever it gets dragged to is what actually persists.
fn default_radar_pos() -> [f32; 2] {
    [860.0, 760.0]
}

/// A GT3 splash-and-go is typically ~25-35s door-to-door; 30s is a
/// reasonable class-agnostic default.
fn default_pit_loss_secs() -> f32 {
    30.0
}

/// Half a second of relative time, which is roughly a few car lengths at
/// racing speed but — unlike a fixed distance — stays meaningful through
/// slow corners and long straights alike. Chosen over meters because the
/// gap the bars plot is already a time — read off the player's own lap curve,
/// which carries the track's speed profile — so no track-length estimate sits
/// between the data and the display.
fn default_radar_range_ms() -> f32 {
    500.0
}

/// A GT3 is a little over 4.5 m; 4.7 m is a fair figure across the classes
/// this overlay is used in, and the SDK publishes no car length to do better.
///
/// It sets both the scale of the bar and where overlap begins, so raising it
/// makes the widget more cautious and lowering it less. Prototypes are longer
/// than this, Miatas shorter.
fn default_car_length_m() -> f32 {
    4.7
}

/// How much track each half of a bar covers, in car lengths.
///
/// Three is enough to see a car arrive with time to react while still leaving
/// half a car length as a movement of roughly sixty pixels — big enough to
/// judge in peripheral vision, which is the entire point. Cars beyond this
/// draw as a pip at the end of the bar rather than being clamped to it, so
/// widening this trades the magnification that makes overlap readable for
/// warning of cars that are not yet a factor.
fn default_range_cars() -> f32 {
    3.0
}

/// Where settings are read from and written to.
///
/// `%APPDATA%\race\race-overlay.toml`, falling back to beside the executable
/// on the unlikely machine with no `APPDATA` — a path is needed either way,
/// and a portable one is a better failure than none.
#[must_use]
pub fn config_path() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(appdata).join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME);
    }
    let beside_exe = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(CONFIG_FILE_NAME)));
    beside_exe.unwrap_or_else(|| PathBuf::from(CONFIG_FILE_NAME))
}

/// Suffix given to a settings file after it has been migrated out of the
/// build folder, so an edit to the stale copy can't look like it did nothing.
const MIGRATED_SUFFIX: &str = "moved";

impl OverlayConfig {
    /// Loads the settings file, migrating an older one into place if needed.
    ///
    /// Returns [`Self::default`] if no settings file exists yet; this app
    /// creates one on first save rather than requiring a hand-authored file.
    ///
    /// # Errors
    /// Returns an error if a settings file exists but is not valid TOML for
    /// this schema. Deliberately not defaulted past: overwriting a file that
    /// merely failed to parse would discard every setting in it.
    pub fn load() -> anyhow::Result<Self> {
        let path = config_path();
        if path.is_file() {
            return Self::load_from(&path);
        }
        match find_beside_exe_or_cwd(CONFIG_FILE_NAME) {
            Some(legacy) => {
                let config = Self::load_from(&legacy)?;
                migrate(&legacy, &path, &config);
                Ok(config)
            }
            None => Ok(Self::default()),
        }
    }

    /// Loads configuration from an explicit TOML file path.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Imports the old global row switches without overriding an explicit
    /// per-panel choice. Obsolete theme keys are accepted and discarded.
    fn parse(text: &str) -> anyhow::Result<Self> {
        let mut value: toml::Value = toml::from_str(text)?;
        if let Some(root) = value.as_table_mut() {
            for key in ["show_flags", "show_off_tracks"] {
                if let Some(legacy) = root.remove(key) {
                    for panel in ["relative", "standings"] {
                        let table = root.entry(panel).or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                        if let Some(table) = table.as_table_mut() {
                            table.entry(key).or_insert_with(|| legacy.clone());
                        }
                    }
                }
            }
            root.remove("theme");
        }
        Ok(value.try_into()?)
    }

    /// Applies `change` to the settings on disk, leaving every other field.
    ///
    /// The overlay and `--bind` both write this file, each from a copy loaded
    /// when it started. Writing that whole copy back reverted anything the
    /// other had saved since — which is how a bind captured while the overlay
    /// was running disappeared the moment a panel was dragged. Re-reading
    /// immediately before writing keeps both.
    ///
    /// # Errors
    /// Returns an error if the file exists but can't be read or parsed, or if
    /// the result can't be written. Nothing is written in the first case: a
    /// file that failed to parse is a file to look at, not to replace.
    pub fn update(change: impl FnOnce(&mut Self)) -> anyhow::Result<()> {
        let mut config = Self::load()?;
        change(&mut config);
        config.save()
    }

    /// Writes this configuration to [`config_path`].
    ///
    /// # Errors
    /// Returns an error if the folder can't be created or the file written.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&config_path())
    }

    /// Writes this configuration to an explicit TOML file path.
    ///
    /// Writes a temporary file beside the target and renames it over the top,
    /// which is atomic: a Windows shutdown that kills this process mid-write
    /// would otherwise leave a truncated file, and a settings file that no
    /// longer parses is every setting in it gone.
    ///
    /// # Errors
    /// Returns an error if the value can't be serialized, the parent folder
    /// can't be created, or either the write or the rename fails.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self).context("serializing race-overlay.toml")?;
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("renaming {} into place", temp.display()))
    }
}

/// Copies a settings file found beside the exe into [`config_path`].
///
/// Best-effort by design: a migration that fails is a note on the console and
/// a set of settings that still work for this run, not a reason to refuse to
/// start. The old file is renamed rather than deleted, both because deleting
/// a user's file to tidy up is not this program's call and because the new
/// name says plainly that editing it will no longer do anything.
fn migrate(from: &Path, to: &Path, config: &OverlayConfig) {
    if let Err(err) = config.save_to(to) {
        println!("note: could not move {} to {}: {err:#}", from.display(), to.display());
        return;
    }
    let retired = from.with_extension(format!("toml.{MIGRATED_SUFFIX}"));
    if std::fs::rename(from, &retired).is_err() {
        println!("note: settings copied to {}; {} is now unused", to.display(), from.display());
        return;
    }
    println!("note: settings moved to {} (was {})", to.display(), from.display());
}

#[cfg(test)]
mod tests {
    #[test]
    fn recent_metric_uses_three_samples_and_defaults_to_average() {
        use super::RelativeLapMetric as M;
        let laps = [Some(102.0), Some(100.0), Some(101.0)];
        assert_eq!(M::default(), M::AverageLast3);
        assert_eq!(M::AverageLast3.value(laps), Some(101.0));
        assert_eq!(M::BestLast3.value(laps), Some(100.0));
        assert_eq!(M::LastLap.value(laps), Some(102.0));
        assert_eq!(M::AverageLast3.value([None, Some(f32::NAN), Some(-2.0)]), None);
        assert_eq!(M::AverageLast3.value([Some(99.0), None, None]), Some(99.0));
    }

    #[test]
    fn relative_column_order_preserves_choices_and_repairs_duplicates() {
        use super::{RelativeColumn as C, RelativeConfig};
        let config = RelativeConfig { column_order: vec![C::Gap, C::Driver, C::Gap], ..RelativeConfig::default() };
        let columns = config.ordered_columns();
        assert_eq!(&columns[..2], &[C::Gap, C::Driver]);
        assert_eq!(columns.len(), C::ALL.len());
    }

    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = OverlayConfig::default();
        cfg.relative.pos = [123.0, 45.0];
        cfg.relative.ahead_count = 5;

        let text = toml::to_string(&cfg).expect("config must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("config must round-trip");

        assert!((parsed.relative.pos[0] - cfg.relative.pos[0]).abs() < f32::EPSILON);
        assert!((parsed.relative.pos[1] - cfg.relative.pos[1]).abs() < f32::EPSILON);
        assert_eq!(parsed.relative.ahead_count, 5);
    }

    #[test]
    fn empty_config_uses_defaults() {
        let cfg: OverlayConfig = toml::from_str("").expect("empty config must parse");
        assert_eq!(cfg.relative.ahead_count, 4);
        assert_eq!(cfg.relative.behind_count, 4);
        assert!(cfg.standings.visible);
        assert!((cfg.standings.pit_loss_secs - 30.0).abs() < f32::EPSILON);
        assert!(cfg.radar.visible);
        assert!((cfg.radar.range_ms - 500.0).abs() < f32::EPSILON);
        assert!((cfg.radar.car_length_m - 4.7).abs() < f32::EPSILON);
        assert!((cfg.radar.range_cars - 3.0).abs() < f32::EPSILON);
        assert!(!cfg.radar.show_numbers);
        assert!(cfg.only_show_when_iracing_focused);
        assert_eq!(cfg.iracing_process_name, "iracingsim64dx11.exe");
        assert!(cfg.hide_in_garage);
        assert!(!cfg.stream_mode);
        assert!(cfg.faster_class.visible);
        assert!((cfg.faster_class.warn_secs - 4.0).abs() < f32::EPSILON);
        assert!((cfg.faster_class.alert_secs - 1.5).abs() < f32::EPSILON);
        assert!(cfg.faster_class.flash);
    }

    #[test]
    fn every_panel_defaults_to_unscaled() {
        let cfg: OverlayConfig = toml::from_str("").expect("empty config must parse");
        for scale in [cfg.relative.scale, cfg.standings.scale, cfg.radar.scale, cfg.faster_class.scale] {
            assert!((scale - 1.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn faster_class_settings_round_trip_through_toml() {
        let mut cfg = OverlayConfig::default();
        cfg.faster_class.pos = [5.0, 6.0];
        cfg.faster_class.warn_secs = 6.5;
        cfg.faster_class.alert_secs = 2.0;
        cfg.faster_class.flash = false;

        let text = toml::to_string(&cfg).expect("config must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("config must round-trip");

        assert!((parsed.faster_class.pos[0] - 5.0).abs() < f32::EPSILON);
        assert!((parsed.faster_class.warn_secs - 6.5).abs() < f32::EPSILON);
        assert!((parsed.faster_class.alert_secs - 2.0).abs() < f32::EPSILON);
        assert!(!parsed.faster_class.flash);
    }

    /// A `race-overlay.toml` written before either of the radar's two unit
    /// changes still loads: the stale `range_m` and `danger_ms` keys are
    /// ignored and the new ones take their defaults, rather than the whole
    /// file failing to parse and silently resetting every panel's saved
    /// position.
    #[test]
    fn configs_predating_the_radars_unit_changes_still_load() {
        let cfg: OverlayConfig = toml::from_str("[radar]\npos = [10.0, 20.0]\nrange_m = 25.0\ndanger_ms = 60.0\n")
            .expect("old config must parse");
        assert!((cfg.radar.pos[0] - 10.0).abs() < f32::EPSILON);
        assert!((cfg.radar.range_ms - 500.0).abs() < f32::EPSILON);
        assert!((cfg.radar.car_length_m - 4.7).abs() < f32::EPSILON);
    }

    /// The overlay writes panel positions and `--bind` writes binds, each
    /// from its own copy; a save must not revert the other's. Exercised
    /// through `save_to`/`load_from` rather than `update`, which resolves its
    /// own path and so can't be pointed at a scratch file.
    #[test]
    fn a_save_keeps_what_the_other_writer_put_in_the_file() {
        let dir = std::env::temp_dir().join("race-overlay-merge-test");
        std::fs::create_dir_all(&dir).expect("scratch folder must be creatable");
        let path = dir.join("race-overlay.toml");

        // `--bind` writes a bind into a file the overlay has already loaded.
        let mut bound = OverlayConfig::default();
        bound.binds.set(Action::NextPage, Bind::Key { vk: 0x72 });
        bound.save_to(&path).expect("the bind must save");

        // The overlay then drags a panel: it re-reads, applies only what it
        // owns, and writes back.
        let mut merged = OverlayConfig::load_from(&path).expect("the file must reload");
        merged.relative.pos = [10.0, 20.0];
        merged.save_to(&path).expect("the position must save");

        let parsed = OverlayConfig::load_from(&path).expect("the file must reload");
        assert_eq!(parsed.binds.next_page, Some(Bind::Key { vk: 0x72 }), "the bind survives the panel drag");
        assert!((parsed.relative.pos[0] - 10.0).abs() < f32::EPSILON);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A half-written settings file is every setting in it gone, so the write
    /// must never be observable half-done — and must leave no debris.
    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = std::env::temp_dir().join("race-overlay-atomic-test");
        std::fs::create_dir_all(&dir).expect("scratch folder must be creatable");
        let path = dir.join("race-overlay.toml");

        OverlayConfig::default().save_to(&path).expect("must save");
        assert!(path.is_file());
        assert!(!path.with_extension("toml.tmp").exists(), "the temporary file is renamed, not left");

        // And a second save over an existing file must succeed, which on
        // Windows means the rename has to replace rather than fail.
        OverlayConfig::default().save_to(&path).expect("must overwrite");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Settings live in `%APPDATA%`, not in `target\release` beside the exe,
    /// where a `cargo clean` would take them with it.
    #[test]
    fn settings_do_not_live_in_the_build_folder() {
        let path = config_path();
        assert!(path.ends_with("race-overlay.toml"));
        if std::env::var_os("APPDATA").is_some() {
            assert!(path.parent().is_some_and(|dir| dir.ends_with(CONFIG_DIR_NAME)));
        }
    }

    #[test]
    fn watch_positions_round_trip_and_stay_out_of_untouched_files() {
        let text = toml::to_string(&OverlayConfig::default()).expect("config must serialize");
        assert!(!text.contains("watch_pos"), "an unused watching layout must not appear in the file");

        let mut cfg = OverlayConfig::default();
        cfg.relative.watch_pos = Some([300.0, 40.0]);
        let text = toml::to_string(&cfg).expect("config must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("config must round-trip");
        assert_eq!(parsed.relative.watch_pos, Some([300.0, 40.0]));
        assert_eq!(parsed.standings.watch_pos, None, "panels never dragged while watching stay on one layout");
    }

    /// A `race-overlay.toml` written before the watching layout existed still
    /// loads, with every panel on its single (driving) layout.
    #[test]
    fn configs_predating_the_watching_layout_still_load() {
        let cfg: OverlayConfig = toml::from_str("[relative]\npos = [1.0, 2.0]\n").expect("old config must parse");
        assert_eq!(cfg.relative.watch_pos, None);
        assert!(!cfg.has_watch_layout());
    }

    /// Reset means back to a fresh install, which has one layout.
    #[test]
    fn resetting_positions_clears_the_watching_layout() {
        let mut cfg = OverlayConfig::default();
        cfg.relative.watch_pos = Some([5.0, 6.0]);
        assert!(cfg.has_watch_layout());

        cfg.reset_positions();
        assert!(!cfg.has_watch_layout());
    }

    #[test]
    fn sync_is_off_by_default_and_round_trips() {
        let cfg: OverlayConfig = toml::from_str("").expect("empty config must parse");
        assert!(!cfg.sync.enabled, "sync must be inert until switched on");
        assert!(cfg.sync.relay_url.is_empty());

        assert!(!cfg.sync.allow_team_pit_control, "remote pit control is opt-in, off by default");

        let mut cfg = OverlayConfig::default();
        cfg.sync.enabled = true;
        cfg.sync.relay_url = "wss://box.tail1234.ts.net".to_owned();
        cfg.sync.invite = "ABCD-2345".to_owned();
        cfg.sync.allow_team_pit_control = true;
        let text = toml::to_string(&cfg).expect("must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("must round-trip");
        assert!(parsed.sync.enabled);
        assert_eq!(parsed.sync.relay_url, "wss://box.tail1234.ts.net");
        assert_eq!(parsed.sync.invite, "ABCD-2345");
        assert!(parsed.sync.allow_team_pit_control);
    }

    /// A config written before team sync existed still loads, with sync off.
    #[test]
    fn configs_predating_sync_still_load() {
        let cfg: OverlayConfig = toml::from_str("[relative]\npos = [1.0, 2.0]\n").expect("old config must parse");
        assert!(!cfg.sync.enabled);
    }

    #[test]
    fn danger_marks_round_trip_and_stay_out_of_an_empty_file() {
        let text = toml::to_string(&OverlayConfig::default()).expect("serialize");
        assert!(!text.contains("[danger]"), "no marks, no danger table in the file");

        let mut cfg = OverlayConfig::default();
        cfg.danger.insert(123_456, DangerLevel::Severe);
        cfg.danger.insert(789_012, DangerLevel::Caution);
        let text = toml::to_string(&cfg).expect("serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("round-trip");
        assert_eq!(parsed.danger.get(&123_456), Some(&DangerLevel::Severe));
        assert_eq!(parsed.danger.get(&789_012), Some(&DangerLevel::Caution));
    }

    #[test]
    fn danger_levels_serialize_lowercase_and_escalate() {
        // The wire form is stable and human-editable; the order is the menu's.
        assert_eq!(DangerLevel::ALL, [DangerLevel::Caution, DangerLevel::Warning, DangerLevel::Severe]);
        let mut cfg = OverlayConfig::default();
        cfg.danger.insert(1, DangerLevel::Warning);
        assert!(toml::to_string(&cfg).expect("serialize").contains("\"warning\""));
    }

    #[test]
    fn picking_a_danger_level_toggles_it() {
        // A fresh pick sets the level; picking the same one again clears it;
        // picking a different one changes it.
        assert_eq!(DangerLevel::toggled(None, DangerLevel::Warning), Some(DangerLevel::Warning));
        assert_eq!(DangerLevel::toggled(Some(DangerLevel::Warning), DangerLevel::Warning), None);
        assert_eq!(DangerLevel::toggled(Some(DangerLevel::Caution), DangerLevel::Severe), Some(DangerLevel::Severe));
    }

    /// A config written before danger marks existed still loads.
    #[test]
    fn configs_predating_danger_marks_still_load() {
        let cfg: OverlayConfig = toml::from_str("[relative]\npos = [1.0, 2.0]\n").expect("old config parses");
        assert!(cfg.danger.is_empty());
    }

    #[test]
    fn logo_variants_read_and_write_as_folder_names() {
        assert_eq!(LogoVariant::parse("icon"), Some(LogoVariant::mono(LogoShape::Icon)));
        assert_eq!(LogoVariant::parse("badge-colour"), Some(LogoVariant { shape: LogoShape::Badge, colour: true }));
        assert_eq!(LogoVariant::parse("colour"), None);
        assert_eq!(LogoVariant::parse("icon-color"), None);
        assert_eq!(LogoVariant { shape: LogoShape::Wordmark, colour: true }.folder(), "wordmark-colour");
    }

    /// A bad override line loses that line, not the file.
    #[test]
    fn a_bad_logo_override_is_dropped_rather_than_failing_the_file() {
        let cfg: OverlayConfig = toml::from_str(
            "[logos]\nstyle = \"badge\"\ncolour = true\n[logos.overrides]\nbmw = \"icon-colour\"\nferrari = \"nope\"\n",
        )
        .expect("config must parse");
        assert_eq!(cfg.logos.style, LogoStyle::Badge);
        assert!(cfg.logos.colour);
        assert_eq!(cfg.logos.overrides.get("bmw"), Some(&LogoVariant { shape: LogoShape::Icon, colour: true }));
        assert!(!cfg.logos.overrides.contains_key("ferrari"));

        let text = toml::to_string(&cfg).expect("must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("must round-trip");
        assert_eq!(parsed.logos, cfg.logos);
    }

    #[test]
    fn width_and_column_settings_round_trip_through_toml() {
        let mut cfg = OverlayConfig::default();
        cfg.relative.width = 520.0;
        cfg.relative.show_brand = false;
        cfg.relative.show_irating = false;
        cfg.standings.name_width = 360.0;

        let text = toml::to_string(&cfg).expect("config must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("config must round-trip");

        assert!((parsed.relative.width - 520.0).abs() < f32::EPSILON);
        assert!(!parsed.relative.show_brand);
        assert!(!parsed.relative.show_irating);
        assert!(parsed.relative.show_car_number, "untouched columns stay on");
        assert!((parsed.standings.name_width - 360.0).abs() < f32::EPSILON);
    }

    /// A `race-overlay.toml` written before the widths and column switches
    /// existed still loads, at the design's own measurements with every
    /// column on.
    #[test]
    fn configs_predating_the_column_settings_still_load() {
        let cfg: OverlayConfig = toml::from_str("[relative]\npos = [1.0, 2.0]\n").expect("old config must parse");
        assert!((cfg.relative.width - 690.0).abs() < f32::EPSILON);
        assert!(cfg.relative.show_car_number && cfg.relative.show_brand);
        assert!(cfg.relative.show_recent_lap && cfg.relative.show_irating);
        assert!((cfg.standings.name_width - 440.0).abs() < f32::EPSILON);
    }

    #[test]
    fn standings_and_radar_round_trip_through_toml() {
        let mut cfg = OverlayConfig::default();
        cfg.standings.pos = [1.0, 2.0];
        cfg.standings.pit_loss_secs = 42.0;
        cfg.radar.pos = [3.0, 4.0];
        cfg.radar.range_ms = 750.0;

        let text = toml::to_string(&cfg).expect("config must serialize");
        let parsed: OverlayConfig = toml::from_str(&text).expect("config must round-trip");

        assert!((parsed.standings.pit_loss_secs - 42.0).abs() < f32::EPSILON);
        assert!((parsed.radar.range_ms - 750.0).abs() < f32::EPSILON);
        assert!((parsed.radar.pos[0] - 3.0).abs() < f32::EPSILON);
    }
}
