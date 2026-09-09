// Rust guideline compliant 2026-02-16

//! Widgets drawn onto the overlay, plus the shared visual system (color
//! tokens, scaling, card/badge/gradient helpers) that makes every panel read
//! as one instrument cluster instead of four unrelated boxes.
//!
//! Every dimension in this module and its children is the pixel value
//! measured off the corresponding image in `design mocks/`, multiplied
//! through [`Metrics`]. Run `race-overlay.exe --demo` to render the widgets
//! against those mockups' own data without iRacing.
//!
//! Palette: a near-black "ink" card base with white text at three opacity
//! tiers for hierarchy, plus a tight semantic accent set — `signal` (good /
//! close / ahead), `alert` (bad / far / pit), `caution` (armed: a thing the
//! next stop will do), `player_row` (this is you, and the cursor) — rather
//! than ad-hoc colors picked per widget. A planned quantity is hatched, not
//! coloured; see [`paint_hatch`].
//!
//! Type: Inter for text, IBM Plex Mono for lap times and gaps, and Barlow
//! Condensed — the "readout" face, see [`readout`] — for a number that stands
//! alone. Geometry: cards round at [`CARD_ROUNDING`], every block inside them
//! at [`BLOCK_ROUNDING`], and nothing leans.

use std::sync::Mutex;

use egui::{Color32, Mesh, Pos2, Rect, Rounding, Shape, Stroke, Ui};

pub mod blackbox;
pub mod faster_class;
pub mod flags;
pub mod icons;
pub mod launcher_page;

/// Remembers what a short name resolved to, so it is worked out once per run.
///
/// Both asset lookups in this module tree — manufacturer marks and interface
/// icons — used to resolve their file from scratch on every row of every frame:
/// a `to_lowercase`, a `join`, two `format!`s and, worst of all, a
/// [`std::path::Path::is_file`] syscall. At display rate across a full Standings list that
/// is tens of thousands of stat calls a second, each one going through the
/// whole Windows filter-driver stack, to answer a question whose answer cannot
/// change: the files under `assets/` neither appear nor vanish while the
/// overlay runs.
///
/// Resolution is therefore done once per distinct key and the result kept. The
/// key space is bounded and tiny — one entry per manufacturer in the session,
/// one per icon in the design — so nothing here needs eviction.
///
/// The `Mutex` is uncontended in practice: every caller is on the UI thread.
/// It exists so the cache can be a `static` without the callers having to
/// thread a `&mut` through the whole drawing path.
///
/// A `Vec` scanned linearly rather than a `HashMap`, for two reasons: `Vec::new`
/// is `const` and `HashMap::new` is not, so this can be a `static` without a
/// `OnceLock` wrapped around it; and at a few dozen short keys, comparing
/// strings is cheaper than hashing one with `SipHash`, which is built for
/// collision resistance nothing here needs.
#[derive(Debug)]
pub struct Memo<V> {
    entries: Mutex<Vec<(Box<str>, V)>>,
}

impl<V: Clone> Memo<V> {
    /// Creates an empty memo, `const` so it can live in a `static`.
    #[must_use]
    pub const fn new() -> Self {
        Self { entries: Mutex::new(Vec::new()) }
    }

    /// Returns `key`'s value, calling `resolve` only the first time it is asked.
    ///
    /// # Panics
    /// If another thread panicked while holding the lock, which would mean the
    /// cache's contents can no longer be trusted.
    pub fn get_or_insert_with(&self, key: &str, resolve: impl FnOnce(&str) -> V) -> V {
        let mut entries = self.entries.lock().expect("the asset memo lock is poisoned");
        if let Some((_, value)) = entries.iter().find(|(known, _)| &**known == key) {
            return value.clone();
        }
        let value = resolve(key);
        entries.push((Box::from(key), value.clone()));
        value
    }
}

impl<V: Clone> Default for Memo<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// How far up from the executable to look for the assets folder.
///
/// A release build lives at `target/release/race-overlay.exe`, so the repo's
/// own `assets/` sits two levels above it. Three covers that with a level to
/// spare without wandering far enough up to pick up an unrelated folder.
const ASSET_SEARCH_DEPTH: usize = 3;

/// Finds `assets/<name>`, next to the executable, then up its parent
/// directories, then in the working directory.
///
/// The walk upward matters because `race-launcher` starts each program with
/// its working directory set to the executable's own folder — so a cwd-only
/// lookup finds nothing when the overlay is launched that way, and every
/// asset silently falls back to whatever stands in for it.
///
/// `None` if no such directory exists anywhere on that path.
#[must_use]
pub fn find_asset_dir(name: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from));
    let from_exe = exe_dir
        .into_iter()
        .flat_map(|dir| dir.ancestors().take(ASSET_SEARCH_DEPTH + 1).map(PathBuf::from).collect::<Vec<_>>());
    from_exe
        .chain(std::iter::once(PathBuf::from(".")))
        .map(|base| base.join("assets").join(name))
        .find(|dir| dir.is_dir())
}
pub mod logos;
pub mod pit_stall;
pub mod radar_bars;
pub mod relative;
pub mod settings;
pub mod standings;
pub mod theme;
pub mod weather;

/// The switches a driver row reads, shared by the Standings and the Relative.
///
/// Both come from the top-level config rather than either panel's own, since
/// a driver's flag or a car's off-track tally means the same thing in both.
#[derive(Debug, Clone, Copy)]
pub struct RowOptions<'a> {
    /// Tally each car's trips off the track in the gutter.
    pub show_off_tracks: bool,
    /// Draw each driver's national flag before their name — see [`flags`].
    pub show_flags: bool,
    /// Drivers marked dangerous, by customer id — for the Relative's danger
    /// glyph. See `crate::config::DangerLevel` and `plans/danger-drivers.md`.
    pub danger: &'a std::collections::BTreeMap<u32, crate::config::DangerLevel>,
    /// The shared fuel target to chase, if a spec has set one — see
    /// [`FuelTargetReadout`] and `plans/strategy-spec-mode.md`.
    pub fuel_target: Option<FuelTargetReadout>,
}

/// The shared fuel-per-lap target as the Relative footer shows it.
///
/// The crew chief's number, the car's current burn against it, and the lap
/// the tank reaches at that burn — one glance at "are we saving enough, and
/// does it make the stop". Computed by the app from the sync target and the
/// snapshot; see `OverlayApp::fuel_target_readout`.
#[derive(Debug, Clone, Copy)]
pub struct FuelTargetReadout {
    /// The litres-per-lap target the crew chief set.
    pub target_lpl: f32,
    /// The car's current measured burn, `None` before a lap has closed.
    pub current_lpl: Option<f32>,
    /// The lap the tank runs dry at the current burn, `None` if not computable.
    pub hit_lap: Option<u16>,
}

// ---- Color tokens ----------------------------------------------------

/// Teal-green: good, close, ahead, on-pace. `#34d399`, matching the design
/// mockups exactly.
pub const SIGNAL: Color32 = Color32::from_rgb(0x34, 0xD3, 0x99);
/// Coral-red: bad, far, in the pits, behind.
pub const ALERT: Color32 = Color32::from_rgb(255, 92, 87);
/// Warm amber: neutral attention (driver-aid settings, caution flags).
pub const CAUTION: Color32 = Color32::from_rgb(255, 184, 77);
/// Soft blue (Tailwind `blue-400`, `#60a5fa`): marks a car the player has
/// lapped — the relative-widget counterpart to `ALERT` marking a car about
/// to lap the player, matching the red/blue lap-status convention from
/// <https://github.com/tariknz/irdashies>.
pub const LAPPED: Color32 = Color32::from_rgb(0x60, 0xA5, 0xFA);
/// The one accent used for card-header underlines and left-edge stripes.
pub const ACCENT: Color32 = SIGNAL;

/// Violet, for the fastest-lap chip and its matching gutter marker. Picked
/// off the mockups, where fastest-lap is the one status that gets a color
/// outside the semantic set above — it's an achievement, not a warning.
pub const FASTEST: Color32 = Color32::from_rgb(0x7C, 0x3A, 0xED);
/// The fill behind a fastest-lap time in the Standings timing column.
pub const FASTEST_CELL: Color32 = Color32::from_rgb(0x3B, 0x1F, 0x6B);
/// The lap-time text inside a [`FASTEST_CELL`].
pub const FASTEST_TEXT: Color32 = Color32::from_rgb(0xC4, 0xA5, 0xFF);
/// The solid fill behind the player's own row. A muted olive rather than a
/// saturated gold: it spans a whole row, so at full intensity it would
/// out-shout the text sitting on top of it.
///
/// Still the "you" mark on the black box's own pages, where it is a stroke
/// or a narrow bar and the gold reads as an accent. The Relative's row uses
/// [`PLAYER_ROW_FILL`] instead — a whole row of this was too much of it.
pub const PLAYER_ROW: Color32 = Color32::from_rgb(0x8A, 0x7B, 0x3A);
/// The fill behind the player's own row in the Relative.
///
/// A lifted neutral, not a hue. Every colour in this overlay already means
/// something — red is danger, amber caution, violet fastest, teal good — and
/// "this row is you" is not one of those meanings; spending a colour on it
/// only makes the colours that carry meaning harder to find. It reads as
/// selected because it is plainly lighter than both the panel and the
/// alternating stripe, and because the position plate on it goes white; see
/// [`PLAYER_PLATE`].
pub const PLAYER_ROW_FILL: Color32 = Color32::from_rgb(0x3A, 0x3B, 0x40);
/// The position plate on the player's own row: near-white, with the number
/// in near-black on top of it.
///
/// The whole "you" signal in one element. Inverting the plate is the
/// loudest thing that can be done to a row without spending a colour on it,
/// and it lands where the eye already goes — the position is the first
/// thing read on any row.
pub const PLAYER_PLATE: Color32 = Color32::from_rgb(0xF2, 0xF3, 0xF6);
/// Stands in for a class's own color when the session runs only one class.
///
/// A single-make grid reports the same class color for every car, and
/// iRacing's is very often white — so the slash and the wash that exist to
/// tell classes apart say nothing, and a row of white marks competes with the
/// player's own highlight for attention. With nothing to distinguish there is
/// no reason to spend white on it.
pub const SINGLE_CLASS: Color32 = Color32::from_rgb(0x38, 0xBD, 0xF8);
/// The plate under a row's position number — see [`paint_position_plate`].
///
/// Opaque, not a translucent black: it has to read as one flat slate at every
/// point along a row, and a wash, an alternating stripe and the player's own
/// olive highlight all pass underneath it. Letting any of those tint it made
/// the plate a different color on every other row.
pub const POSITION_PLATE: Color32 = Color32::from_rgb(0x1E, 0x1D, 0x21);
/// The plate a value you can change sits on — see [`paint_control_plate`].
///
/// Opaque for the same reason [`POSITION_PLATE`] is: an alternating stripe and
/// the cursor's own olive highlight both pass underneath, and a translucent
/// plate would be a different color on the row you were actually pointing at.
/// Lifted well clear of [`PANEL_BG`], because "you can change this" is the one
/// thing the surface itself has to say.
pub const CONTROL_PLATE: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x30);
/// The shadow half of the groove between two rows — see [`paint_row_groove`].
pub const ROW_GROOVE_SHADOW: Color32 = Color32::from_black_alpha(165);
/// Its highlight half, catching the top edge of the row below.
pub const ROW_GROOVE_LIGHT: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);
// The Radar Bars' three car colors encode threat, not direction: once the
// player's own car is drawn on the bar, a marker's position already says
// whether it is ahead or behind, and spending the strongest channel on screen
// restating that would waste it. Urgency is what actually varies.
//
/// Slate: a car on the radar with more than a car length of clear air.
/// Present, not yet a factor — deliberately low-contrast so it stays out of
/// the way until it matters.
pub const RADAR_CLEAR: Color32 = Color32::from_rgb(0x7C, 0x88, 0x9C);
/// Amber: less than a car length of clear air. About to be alongside.
pub const RADAR_CLOSE: Color32 = Color32::from_rgb(0xF5, 0xA5, 0x24);
/// The outline of the player's own car, fixed at the middle of each bar.
///
/// Every other mark on the bar is read against this one, so it is drawn in
/// plain white and always on top — including on top of a car overlapping it,
/// which is the moment its edges matter most.
pub const RADAR_PLAYER: Color32 = Color32::from_rgb(0xF2, 0xF4, 0xF8);
/// The cyan of the wind arrow and the wet-weather marks.
pub const WIND: Color32 = Color32::from_rgb(0x22, 0xD3, 0xEE);

/// Background fill shared by every card: a near-black "ink" base, dark
/// enough to read as a solid card over any game content, while still
/// letting a little of the track show through at the edges.
pub const PANEL_BG: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 235);
/// A lifted card fill, for a surface that sits beside or on top of a
/// [`PANEL_BG`] one and needs to read as distinct from it — the Standings
/// timing card, and the Weather stat tiles.
pub const TILE_BG: Color32 = Color32::from_rgba_premultiplied(28, 28, 31, 240);

/// Primary text: near-white, for anything that's the point of the row.
#[must_use]
pub fn text_primary() -> Color32 {
    Color32::from_white_alpha(235)
}

/// Secondary text: labels, units, anything supporting the primary value.
#[must_use]
pub fn text_secondary() -> Color32 {
    Color32::from_white_alpha(145)
}

/// Tertiary text: the quietest tier — eyebrows, placeholders, hints.
#[must_use]
pub fn text_tertiary() -> Color32 {
    Color32::from_white_alpha(85)
}

/// The near-white an Instrument card is bordered in — bright enough to read
/// as a bezel rather than as a hairline.
#[must_use]
pub fn instrument_border() -> Color32 {
    Color32::from_rgb(0xE5, 0xE5, 0xE5)
}

/// The quieter line Instrument separates rows and tiles with, where Panel
/// uses a groove or a fill.
#[must_use]
pub fn instrument_divider() -> Color32 {
    Color32::from_white_alpha(52)
}

/// Hairline dividers and card borders.
#[must_use]
pub fn hairline() -> Color32 {
    Color32::from_white_alpha(18)
}

// ---- Shape tokens ------------------------------------------------------

pub const CARD_ROUNDING: f32 = 16.0;
/// The Standings and Relative cards' corner radius, a step tighter than
/// [`CARD_ROUNDING`]: at a table's width the full card radius read as
/// bulbous. Kept equal to [`theme::INSTRUMENT_CARD_ROUNDING`], so the two
/// tables sit the same under either theme.
pub const TABLE_CARD_ROUNDING: f32 = 12.0;
/// The radius of every block drawn *inside* a card: tags, plates, tabs, tiles,
/// the ends of a bar.
///
/// Two radii and no angles is the overlay's whole geometry. Everything used
/// to lean on one slant — the class slash, the class tags, the position plate,
/// the control plates — and the lean has gone; a plate is now a lifted block,
/// an armed one an amber block, and emphasis comes from fill and type.
pub const BLOCK_ROUNDING: f32 = 4.0;

// ---- Type ----------------------------------------------------------------

/// The family name the readout face is installed under — see
/// `app::install_fonts`.
///
/// Barlow Condensed `SemiBold`, for a number that stands alone: a position, a
/// hero figure, a tile value. Condensed buys height without spending width,
/// which is the trade an overlay wants. It never appears inside a lap time,
/// where the mono face keeps the digits in columns.
pub const READOUT_FAMILY: &str = "readout";

/// `text` in the readout face at `size`.
///
/// One helper rather than each call site naming the family, so the face can
/// be swapped in one place.
#[must_use]
pub fn readout(text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text).family(egui::FontFamily::Name(READOUT_FAMILY.into())).size(size)
}

// ---- Scaling -----------------------------------------------------------

/// Multiplies every dimension in a panel by that panel's configured scale.
///
/// Widgets are written at the exact pixel sizes of their design mockups and
/// pass each one through [`Metrics::px`], so a scale of `1.0` reproduces the
/// design and any other value resizes the whole panel coherently.
///
/// Scaling is applied here, at layout time, rather than by transforming the
/// rendered panel: egui rasterizes glyphs at the size it's asked for, so
/// text stays sharp at any scale, which a post-hoc transform of the finished
/// mesh could not do.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    scale: f32,
}

impl Metrics {
    /// Creates metrics for a panel drawn at `scale`.
    ///
    /// Non-finite or non-positive scales fall back to `1.0`; they can only
    /// come from a hand-edited config, and a panel of zero or NaN size would
    /// otherwise vanish with no clue as to why.
    #[must_use]
    pub fn new(scale: f32) -> Self {
        Self { scale: if scale.is_finite() && scale > 0.0 { scale } else { 1.0 } }
    }

    /// Scales one design-mockup pixel measurement.
    #[must_use]
    pub fn px(self, value: f32) -> f32 {
        value * self.scale
    }

    /// Scales a width/height pair.
    #[must_use]
    pub fn vec2(self, x: f32, y: f32) -> egui::Vec2 {
        egui::vec2(self.px(x), self.px(y))
    }
}

// ---- Shared widgets ------------------------------------------------------

/// The card chrome shared by every panel: rounding, padding, and a drop
/// shadow matching the design mockups' `box-shadow: 0 20px 40px
/// rgba(0,0,0,.6)`.
///
/// No border: over a bright track a hairline outline reads as a hard white
/// rectangle around the panel rather than as an edge, and the shadow already
/// separates the card from whatever is behind it.
///
/// `rounding` is per-corner so cards that sit against each other can square
/// off their facing edges and read as one surface — see [`card_rounding`].
pub fn card_frame(metrics: Metrics, fill: Color32, padding: egui::Margin, rounding: Rounding) -> egui::Frame {
    // Under Instrument the card is separated from the track by an edge rather
    // than by a shadow, and its fill backs toward opaque so that edge reads as
    // a bezel instead of a wire frame — see `theme::card_fill`.
    let frame = egui::Frame::none().fill(theme::card_fill(fill)).rounding(rounding).inner_margin(padding);
    match theme::card_stroke(metrics) {
        Some(stroke) => frame.stroke(stroke),
        None => frame.shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, metrics.px(10.0)),
            blur: metrics.px(24.0),
            spread: 0.0,
            color: Color32::from_black_alpha(120),
        }),
    }
}

/// A card rounded on all four corners — the default for a standalone panel.
#[must_use]
pub fn card_rounding(metrics: Metrics) -> Rounding {
    Rounding::same(metrics.px(card_radius_px()))
}

/// The corner radius a card takes under the theme in force.
///
/// Instrument's bezel is a tighter curve than Panel's card — see
/// [`theme::INSTRUMENT_CARD_ROUNDING`].
#[must_use]
pub fn card_radius_px() -> f32 {
    if theme::is_instrument() { theme::INSTRUMENT_CARD_ROUNDING } else { CARD_ROUNDING }
}

/// A table card rounded on all four corners — see [`TABLE_CARD_ROUNDING`].
#[must_use]
pub fn table_card_rounding(metrics: Metrics) -> Rounding {
    Rounding::same(metrics.px(TABLE_CARD_ROUNDING))
}

/// A card rounded on one side only, for cards butted up against a neighbor.
///
/// Squaring the facing edges is what lets the Standings widget's two cards
/// read as a single table split by a seam rather than as two separate boxes
/// that happen to be adjacent.
#[must_use]
pub fn card_rounding_side(metrics: Metrics, left: bool) -> Rounding {
    let r = metrics.px(card_radius_px());
    if left { Rounding { nw: r, sw: r, ne: 0.0, se: 0.0 } } else { Rounding { nw: 0.0, sw: 0.0, ne: r, se: r } }
}

/// The alternating row tint that gives a long table a readable rhythm.
///
/// Returned as a fill to paint under a row; `None` for rows that take no
/// stripe, so callers can skip the paint entirely.
#[must_use]
pub fn row_stripe(odd: bool) -> Option<Color32> {
    // Instrument has no alternating fill: its rows are told apart by the
    // dividers between them, and a stripe under a bordered card reads as a
    // second surface.
    if theme::is_instrument() {
        return None;
    }
    odd.then_some(Color32::from_white_alpha(10))
}

/// A symmetric margin in scaled pixels.
#[must_use]
pub fn margin(metrics: Metrics, x: f32, y: f32) -> egui::Margin {
    egui::Margin::symmetric(metrics.px(x), metrics.px(y))
}

/// The plate a row's position number sits on: a darker panel at the row's
/// leading edge, with a shadow falling across the row behind it.
///
/// The position is the one thing on a row read at a glance rather than
/// studied, and it was competing with the class-color wash that runs under it.
/// Sinking it into its own darker panel separates the two, and the shadow is
/// what makes the panel read as sitting *under* the row rather than as another
/// flat band of color beside it.
///
/// Straight-edged. It used to be cut to the class slash's lean; nothing in the
/// overlay leans now, so the plate ends where it ends and the class bar
/// stands beside it.
///
/// `width` is the scaled distance from `rect`'s left edge to the plate's
/// trailing edge, and `edge` an optional coloured border struck down that
/// edge — the car's class, in both panels that use this. The class was a
/// wash across the row, then a bar floating in the column beside the plate;
/// as the plate's own edge it is tucked against the one element every row
/// starts with, and it stops competing for the column next to it.
/// `row_rounding` is the row's own, so the plate curves with the outer corner
/// and stays square where it meets the rest of the row — one strip, rather
/// than a pill floating on top of one.
///
/// `fill` is [`POSITION_PLATE`] on every row but the player's own, which
/// takes [`PLAYER_ROW`] so that "you" reads from the plate alone, before the
/// row's wash is noticed.
pub fn paint_position_plate(
    ui: &Ui,
    rect: Rect,
    width: f32,
    row_rounding: Rounding,
    fill: Color32,
    edge: Option<(f32, Color32)>,
) {
    if width <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let trailing = rect.left() + width;
    let body = Rect::from_min_max(rect.min, egui::pos2(trailing, rect.max.y));
    let rounding = Rounding { nw: row_rounding.nw, sw: row_rounding.sw, ne: 0.0, se: 0.0 };
    ui.painter().rect_filled(body, rounding, fill);

    // Full row height, square: it is an edge on the plate, and an edge that
    // stopped short of the row's own top and bottom would read as one more
    // floating bar.
    if let Some((thickness, colour)) = edge.filter(|(thickness, _)| *thickness > 0.0) {
        let border = Rect::from_min_max(egui::pos2(trailing, rect.top()), egui::pos2(trailing + thickness, rect.max.y));
        ui.painter().rect_filled(border, Rounding::ZERO, colour);
    }
}

/// The plate a value you can change sits on: a raised, rounded block.
///
/// The black box is the one panel in the overlay that is *changed* rather than
/// read, and a stepped value used to be told apart from a read-only one by two
/// thin chevrons at opposite ends of a wide row — not a distinction that
/// survives being glanced at on the way into the pits. Everything steppable
/// sits on one of these; everything read-only stays flat on the row.
pub fn paint_control_plate(ui: &Ui, rect: Rect, rounding: f32) {
    paint_control_plate_filled(ui, rect, rounding, CONTROL_PLATE);
}

/// A control plate in a color of its own — see [`paint_control_plate`].
///
/// For the controls that light up rather than merely holding a value: a plate
/// that fills with the armed color says "this is on" the same way an armed
/// tyre does, where coloring only its text leaves the control looking like a
/// label that happens to be a different color.
pub fn paint_control_plate_filled(ui: &Ui, rect: Rect, rounding: f32, fill: Color32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    ui.painter().rect_filled(rect, rounding, fill);
}

/// The diagonal hatch that marks a *planned* quantity: fuel a stop will add,
/// road still to cover, a stint length guessed rather than measured.
///
/// Solid fill is what is real now; hatch is what is not yet. Carrying that
/// distinction in texture rather than colour is what frees amber to mean
/// "armed" and nothing else. Lines lean at the same angle everywhere, and are
/// clipped to `rect` — callers put a hatch inside a shape's straight run, not
/// over its rounded ends.
pub fn paint_hatch(ui: &Ui, metrics: Metrics, rect: Rect, color: Color32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let pitch = metrics.px(HATCH_PITCH);
    let stroke = Stroke::new(metrics.px(HATCH_STROKE), color);
    let painter = ui.painter().with_clip_rect(rect);
    // Each line runs from the bottom edge up and to the right at 60°, so it
    // rises `height / tan 60°` over the rect's height. Starting left of the
    // rect by that much means the first line already crosses the top-left
    // corner rather than leaving it bare.
    let run = rect.height() / HATCH_ANGLE_TAN;
    let mut x = rect.left() - run;
    while x < rect.right() {
        painter.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x + run, rect.top())], stroke);
        x += pitch;
    }
}

/// Distance between hatch lines, and their thickness, at design size.
const HATCH_PITCH: f32 = 7.0;
const HATCH_STROKE: f32 = 2.0;
/// `tan 60°`: the hatch's lean, matching the radar mockup's.
const HATCH_ANGLE_TAN: f32 = 1.732_050_8;

/// Paints a vertical gradient inside a capsule — a rect whose ends are
/// rounded to half its width — without spilling past the rounded ends.
///
/// A gradient is a mesh, and a mesh is a quad: painted straight into a
/// rounded shape it fills the corners the rounding took away. On the Radar's
/// bars, whose fill is translucent, those corners showed against the track as
/// square shoulders either side of each rounded tip.
///
/// The ends are therefore painted as their own rounded caps and the gradient
/// runs between them. The caps take the gradient's end colours flat, which
/// over a cap of half the bar's width is a small enough span of the ramp to
/// read as continuous.
pub fn gradient_capsule_v(ui: &Ui, rect: Rect, from: Color32, to: Color32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let radius = (rect.width() / 2.0).min(rect.height() / 2.0);
    let cap = |top: bool| {
        let (rect, rounding, color) = if top {
            (
                Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + radius)),
                Rounding { nw: radius, ne: radius, sw: 0.0, se: 0.0 },
                from,
            )
        } else {
            (
                Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - radius), rect.max),
                Rounding { nw: 0.0, ne: 0.0, sw: radius, se: radius },
                to,
            )
        };
        ui.painter().rect_filled(rect, rounding, color);
    };
    cap(true);
    cap(false);

    let body =
        Rect::from_min_max(egui::pos2(rect.min.x, rect.min.y + radius), egui::pos2(rect.max.x, rect.max.y - radius));
    if body.height() > 0.0 {
        paint_gradient_mesh(ui, body, [from, from, to, to]);
    }
}

/// Paints `rect` as a quad with one color per corner, in top-left,
/// top-right, bottom-right, bottom-left order.
fn paint_gradient_mesh(ui: &Ui, rect: Rect, corners: [Color32; 4]) {
    let mut mesh = Mesh::default();
    let positions = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    for (pos, color) in positions.into_iter().zip(corners) {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(Shape::mesh(mesh));
}

/// Separates two touching rows: a shadow under the upper one, a highlight on
/// the lower one.
///
/// The pair of lines a real bevel would cast, which is what makes each row
/// read as a raised surface with the join sunk between them. Rows used to be
/// held apart by a gap instead — that separates them just as well but leaves
/// them floating as loose tiles rather than reading as one instrument face.
///
/// Both lines are a physical pixel, deliberately unscaled: a hairline is the
/// one thing on a panel that should stay a hairline, and scaling one to 0.8 of
/// a pixel only blurs it across two.
pub fn paint_row_groove(ui: &Ui, y: f32, left: f32, right: f32) {
    // Instrument separates rows with one quiet line rather than a bevel: a
    // shadow-and-highlight pair reads as a raised surface, which is the thing
    // this theme does not have.
    if theme::is_instrument() {
        ui.painter().rect_filled(
            Rect::from_min_max(egui::pos2(left, y), egui::pos2(right, y + 1.0)),
            0.0,
            instrument_divider(),
        );
        return;
    }
    let line = |top: f32, color: Color32| {
        ui.painter().rect_filled(Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + 1.0)), 0.0, color);
    };
    line(y - 1.0, ROW_GROOVE_SHADOW);
    line(y, ROW_GROOVE_LIGHT);
}

/// A translucent tint of `color`, for row washes and badge fills.
#[must_use]
pub fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Paints a dashed horizontal rule, for the Radar Bars' gap markers.
pub fn dashed_line_h(ui: &Ui, y: f32, from_x: f32, to_x: f32, dash: f32, gap: f32, stroke: Stroke) {
    let mut x = from_x;
    while x < to_x {
        let end = (x + dash).min(to_x);
        ui.painter().line_segment([egui::pos2(x, y), egui::pos2(end, y)], stroke);
        x = end + gap;
    }
}

/// Parses a `0xRRGGBB`/`RRGGBB`-style hex color string from the session
/// YAML (e.g. `DriverInfo.Drivers[].LicColor`). Falls back to white if the
/// string is missing or malformed.
#[must_use]
pub fn parse_hex_color(hex: &str) -> Color32 {
    let hex = hex.trim_start_matches("0x").trim_start_matches("0X");
    if hex.len() != 6 {
        return Color32::WHITE;
    }
    let Ok(r) = u8::from_str_radix(&hex[0..2], 16) else {
        return Color32::WHITE;
    };
    let Ok(g) = u8::from_str_radix(&hex[2..4], 16) else {
        return Color32::WHITE;
    };
    let Ok(b) = u8::from_str_radix(&hex[4..6], 16) else {
        return Color32::WHITE;
    };
    Color32::from_rgb(r, g, b)
}

/// Reads a class color string, falling back to [`ACCENT`] when absent.
#[must_use]
pub fn class_color(hex: &str) -> Color32 {
    if hex.is_empty() { ACCENT } else { parse_hex_color(hex) }
}

/// The accent a row takes, given how many classes are actually running.
///
/// One class means nothing to tell apart, so its own color is replaced by
/// [`SINGLE_CLASS`]; see there for why.
#[must_use]
pub fn class_accent(hex: &str, class_count: usize) -> Color32 {
    if class_count <= 1 { SINGLE_CLASS } else { class_color(hex) }
}

/// Formats a lap time as `MM:SS.mmm` with a zero-padded minute field, the
/// form the design mockups use (`01:38.021`). Returns `"-"` for a
/// non-positive, not-yet-set value.
#[must_use]
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "lap times are far below u32's range")]
pub fn format_lap_time(secs: f32) -> String {
    if secs <= 0.0 {
        return "-".to_owned();
    }
    let minutes = (secs / 60.0).floor();
    let remainder = secs - minutes * 60.0;
    format!("{:02}:{remainder:06.3}", minutes as u32)
}

/// Formats a lap time with the minutes dropped: `1:45.6` reads `45.6`.
///
/// Lap times in a session almost always share a minute, so the minute digit
/// carries no information while costing two characters in a column that sits
/// beside a driver's name. Returns `"-"` for a non-positive value.
///
/// The trade-off is real: a 1:45.6 and a 2:45.6 both render `45.6`. That
/// only bites in a field spanning more than a minute of pace, where the
/// class colors already say which cars are not in the same race.
#[must_use]
pub fn format_short_lap_time(secs: f32) -> String {
    if secs <= 0.0 {
        return "-".to_owned();
    }
    format!("{:.1}", secs - (secs / 60.0).floor() * 60.0)
}

/// Formats a duration in seconds as `HH:MM:SS` — the Standings and Relative
/// race clock, which the mockups show zero-padded throughout (`00:20:08`).
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "session lengths are far below i64's exact-integer range")]
pub fn format_clock(secs: f64) -> String {
    let total = secs.max(0.0).floor() as i64;
    format!("{:02}:{:02}:{:02}", total / 3600, (total / 60) % 60, total % 60)
}

/// The lap counter: `12/40` against a scheduled lap count, `12/~25` against
/// a projection from the clock, and the bare lap where there is neither.
///
/// Shared by the Relative's footer and the Standings top bar, so the two can
/// never disagree about which lap it is.
#[must_use]
pub fn lap_text(meta: &crate::telemetry::snapshot::RelativeMeta) -> String {
    match (meta.session_laps, meta.predicted_total_laps) {
        (Some(total), _) => format!("{}/{total}", meta.current_lap),
        (None, Some(predicted)) => format!("{}/~{predicted}", meta.current_lap),
        (None, None) => meta.current_lap.to_string(),
    }
}

/// Formats a session length as `HH:MM`, the form it takes after the clock:
/// `00:20:08 / 00:45`.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "session lengths are far below i64's exact-integer range")]
pub fn format_session_length(secs: f64) -> String {
    let minutes = (secs.max(0.0) / 60.0).round() as i64;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Formats a duration in seconds as `M:SS` (no sub-second precision) — for
/// stint lengths, where millisecond precision would just be visual noise.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "session/stint lengths are far below i64's exact-integer range")]
pub fn format_minutes_seconds(secs: f64) -> String {
    if secs <= 0.0 {
        return "-".to_owned();
    }
    let total_seconds = secs.floor() as i64;
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// Formats an iRating as the mockups' compact `2.5k`.
#[must_use]
#[expect(clippy::cast_precision_loss, reason = "iRating values are far below f32's exact-integer range")]
pub fn format_irating(irating: i32) -> String {
    format!("{:.1}k", irating as f32 / 1000.0)
}

/// Paints `text` anchored at `pos`, bypassing egui's layout entirely.
///
/// The Standings widget places rows by computed rectangle so its three
/// columns stay in lockstep, which means it needs to draw text at an exact
/// point rather than append it to a layout.
pub fn paint_text(ui: &Ui, pos: Pos2, anchor: egui::Align2, text: egui::RichText) {
    let galley = layout(ui, text);
    let rect = anchor.anchor_size(pos, galley.size());
    // The galley already carries each run's color, so the fallback passed
    // here is never used; `Color32::WHITE` is simply the neutral choice.
    ui.painter().galley(rect.min, galley, Color32::WHITE);
}

/// Measures the width `text` will occupy, for right-aligning a run of chips.
#[must_use]
pub fn text_width(ui: &Ui, text: egui::RichText) -> f32 {
    layout(ui, text).size().x
}

/// Shortens `text` with a trailing ellipsis until it fits `max_width`.
///
/// For the driver-name columns, whose width is the one thing a narrowed
/// panel gives up: a name too long for the room it has left would otherwise
/// run under the columns to its right. `style` must apply the same size and
/// weight the caller draws with, since that is what decides how wide each
/// candidate lays out.
pub fn elide_to_width(ui: &Ui, text: &str, max_width: f32, style: impl Fn(String) -> egui::RichText) -> egui::RichText {
    let full = style(text.to_owned());
    if text_width(ui, full.clone()) <= max_width {
        return full;
    }
    let mut kept: Vec<char> = text.chars().collect();
    while kept.pop().is_some() {
        let mut candidate: String = kept.iter().collect();
        candidate.truncate(candidate.trim_end().len());
        candidate.push('\u{2026}');
        let styled = style(candidate);
        if kept.is_empty() || text_width(ui, styled.clone()) <= max_width {
            return styled;
        }
    }
    style("\u{2026}".to_owned())
}

/// Lays `text` out on one line, never wrapping — every caller is placing a
/// short label at an exact point, where a wrap would silently break the
/// alignment it was positioned for.
fn layout(ui: &Ui, text: egui::RichText) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body)
}

/// The orange a car off the track is marked in — bright enough to catch the
/// eye in peripheral vision, which is the only way a gutter gets read.
pub const OFF_TRACK: Color32 = Color32::from_rgb(0xFF, 0x8C, 0x1A);

/// Paints the gutter marker for a car's trips off the track, filling `rect`.
///
/// Lit orange with the skid mark while the car is off now; a quiet tile with
/// the mark and the tally once it is back, so the flash becomes a fact. With
/// nothing to say — on track, never off — it paints nothing.
pub fn paint_off_track_marker(ui: &Ui, metrics: Metrics, rect: Rect, off_now: bool, count: i32) {
    if !off_now && count <= 0 {
        return;
    }
    let rounding = metrics.px(6.0);
    let (fill, ink) =
        if off_now { (OFF_TRACK, Color32::BLACK) } else { (Color32::from_black_alpha(140), Color32::WHITE) };
    ui.painter().rect_filled(rect, rounding, fill);
    if count > 0 {
        // The mark up top, the tally under it.
        let glyph = Rect::from_min_max(
            egui::pos2(rect.left() + metrics.px(6.0), rect.top() + metrics.px(3.0)),
            egui::pos2(rect.right() - metrics.px(6.0), rect.top() + rect.height() * 0.58),
        );
        icons::skid(ui, glyph, ink);
        paint_text(
            ui,
            egui::pos2(rect.center().x, rect.bottom() - metrics.px(6.0)),
            egui::Align2::CENTER_CENTER,
            egui::RichText::new(count.to_string()).monospace().size(metrics.px(10.0)).strong().color(ink),
        );
    } else {
        icons::skid(ui, rect.shrink(metrics.px(5.0)), ink);
    }
}

/// The orange disc of the meatball flag.
///
/// iRacing's own is a plain orange; this one is lifted a touch so it holds on
/// the near-black tile it sits on rather than sinking into it.
const MEATBALL: Color32 = Color32::from_rgb(0xFF, 0x7A, 0x1A);

/// Paints the marker for a black flag held against a car, filling `rect`.
///
/// The Relative and Standings gutters share this so a flag reads the same in
/// both. The tile is the severity: a black flag on white for a slowdown
/// warning, which is still the driver's to clear, and all black once a
/// penalty has actually been handed out — the tile *is* the black flag, so
/// it carries no glyph. The marks on the others are the sim's own where
/// there is one — the meatball its orange disc — and a disqualification,
/// which has no flag, says so.
pub fn paint_penalty_marker(ui: &Ui, metrics: Metrics, rect: Rect, penalty: crate::telemetry::snapshot::Penalty) {
    use crate::telemetry::snapshot::Penalty;

    let rounding = metrics.px(6.0);
    let glyph = rect.shrink(metrics.px(6.0));
    match penalty {
        Penalty::Slowdown => {
            ui.painter().rect_filled(rect, rounding, Color32::WHITE);
            paint_flag_glyph(ui, metrics, glyph, Color32::BLACK);
        }
        Penalty::BlackFlag => paint_black_tile(ui, metrics, rect, rounding),
        Penalty::Repair => {
            paint_black_tile(ui, metrics, rect, rounding);
            ui.painter().circle_filled(rect.center(), metrics.px(7.0), MEATBALL);
        }
        Penalty::Disqualified => {
            paint_black_tile(ui, metrics, rect, rounding);
            paint_text(
                ui,
                rect.center(),
                egui::Align2::CENTER_CENTER,
                egui::RichText::new("DQ").monospace().size(metrics.px(12.0)).strong().color(ALERT),
            );
        }
    }
}

/// A black flag's tile: solid black with a thin white rim, so it stays a
/// shape against the near-black card behind it.
fn paint_black_tile(ui: &Ui, metrics: Metrics, rect: Rect, rounding: f32) {
    ui.painter().rect_filled(rect, rounding, Color32::BLACK);
    ui.painter().rect_stroke(rect, rounding, egui::Stroke::new(metrics.px(1.0), Color32::from_white_alpha(190)));
}

/// A solid flag, or a painter-drawn pennant when the asset folder is missing.
///
/// Filled rather than the Lucide outline: on the slowdown marker the glyph
/// stands for the black flag itself, and an outline read as a flag-shaped
/// hole rather than a black flag on white.
fn paint_flag_glyph(ui: &Ui, metrics: Metrics, rect: Rect, color: Color32) {
    if icons::svg(ui, rect, "flag-filled", color) {
        return;
    }
    let pole = egui::Stroke::new(metrics.px(2.0), color);
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + metrics.px(2.0), rect.top()),
            egui::pos2(rect.left() + metrics.px(2.0), rect.bottom()),
        ],
        pole,
    );
    let pennant = Rect::from_min_max(
        egui::pos2(rect.left() + metrics.px(3.0), rect.top()),
        egui::pos2(rect.right(), rect.center().y),
    );
    ui.painter().rect_filled(pennant, 0.0, color);
}

/// The grid's state as one string: cars gridded of how many, and the time
/// left to grid where the sim gives one — `12/24 · 1:32`.
#[must_use]
pub fn grid_text(grid: crate::telemetry::snapshot::GridStatus) -> String {
    match grid.countdown_secs {
        Some(secs) => format!("{}/{} \u{00B7} {}", grid.cars_gridded, grid.car_count, format_countdown(secs)),
        None => format!("{}/{}", grid.cars_gridded, grid.car_count),
    }
}

/// `m:ss`, rounding up: a countdown reads `0:00` when it is over, not for the
/// whole of its last second.
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "a countdown of minutes is far inside i64")]
pub fn format_countdown(secs: f64) -> String {
    let total = secs.max(0.0).ceil() as i64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors_with_and_without_0x_prefix() {
        assert_eq!(parse_hex_color("0xFF3333"), Color32::from_rgb(0xFF, 0x33, 0x33));
        assert_eq!(parse_hex_color("00FF00"), Color32::from_rgb(0, 0xFF, 0));
    }

    #[test]
    fn falls_back_to_white_for_malformed_input() {
        assert_eq!(parse_hex_color(""), Color32::WHITE);
        assert_eq!(parse_hex_color("nonsense"), Color32::WHITE);
    }

    /// The mockups zero-pad the minute field; an unpadded `1:38.021` would
    /// shift every digit in the column by one character.
    #[test]
    fn formats_lap_times_the_way_the_mockups_do() {
        assert_eq!(format_lap_time(98.021), "01:38.021");
        assert_eq!(format_lap_time(600.5), "10:00.500");
        assert_eq!(format_lap_time(0.0), "-");
        assert_eq!(format_lap_time(-1.0), "-");
    }

    #[test]
    fn formats_the_race_clock_and_session_length() {
        assert_eq!(format_clock(20.0 * 60.0 + 8.0), "00:20:08");
        assert_eq!(format_clock(3661.0), "01:01:01");
    }

    /// A negative clock would otherwise format with a stray minus sign
    /// inside the padding; `SessionTimeRemain` can read slightly negative
    /// between the checkered flag and the session actually ending.
    #[test]
    fn clamps_negative_durations_to_zero() {
        assert_eq!(format_clock(-5.0), "00:00:00");
    }

    /// The minute is dropped, so `1:45.6` reads `45.6` — and a lap under a
    /// minute still renders its seconds unchanged.
    #[test]
    fn short_lap_times_drop_the_minute() {
        assert_eq!(format_short_lap_time(105.6), "45.6");
        assert_eq!(format_short_lap_time(59.4), "59.4");
        assert_eq!(format_short_lap_time(120.0), "0.0");
        assert_eq!(format_short_lap_time(0.0), "-");
    }

    #[test]
    fn formats_iratings_in_thousands() {
        assert_eq!(format_irating(2500), "2.5k");
        assert_eq!(format_irating(800), "0.8k");
    }

    #[test]
    fn metrics_scale_every_dimension() {
        let metrics = Metrics::new(0.5);
        assert!((metrics.px(40.0) - 20.0).abs() < f32::EPSILON);
        assert!((metrics.vec2(10.0, 20.0).y - 10.0).abs() < f32::EPSILON);
    }

    /// A hand-edited config could hold `scale = 0` or a NaN; either would
    /// otherwise collapse or blank the panel with no visible cause.
    #[test]
    fn metrics_reject_unusable_scales() {
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!((Metrics::new(bad).px(10.0) - 10.0).abs() < f32::EPSILON, "scale {bad} must fall back to 1.0");
        }
    }
}
