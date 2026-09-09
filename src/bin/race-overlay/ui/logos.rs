// Rust guideline compliant 2026-02-16

//! Resolves a car to its manufacturer mark for the Standings and Relative.
//!
//! Marks are SVG files under `assets/logos/`, one folder per variant (four
//! shapes, white or brand colour — see [`LogoVariant`]) plus optional flat
//! files a user drops in for brands upstream lacks. The folder is looked up
//! beside the executable and then in the working directory, matching how
//! this app finds its config. Files are rendered by `egui_extras`' SVG
//! loader through an `egui::Image`, so rasterization and caching are
//! handled for us.
//!
//! Which variant a brand gets is decided by the [`LogoConfig`] the app
//! hands in each frame through [`apply`], together with the manifest
//! (`assets/logos/manifest.toml`): a per-brand override first, then the
//! global style — or the manifest's curated pick — then, when colour is
//! asked for, the colour twin if the manifest lists it as legible on the
//! overlay's dark ground. See `plans/logo-styles.md`.
//!
//! On disk the wanted file is tried first, then its white twin, then the
//! curated variant, then a flat file, and failing all of those the mark is
//! a short uppercase abbreviation of the manufacturer name, so an
//! unrecognized car still gets an identifying mark in the same slot rather
//! than a hole in the row.
//!
//! Every mark is drawn to its own proportions and centred in the slot it is
//! given, never stretched to fill it — see [`draw`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Color32, Rect, Ui};
use serde::Deserialize;

use super::{Memo, paint_text, text_secondary};
use crate::config::{LogoConfig, LogoShape, LogoVariant};

/// How many characters a fallback abbreviation is cut to. Three fits the
/// slot the mockups give a mark while still separating the manufacturers
/// that share a first letter.
const ABBREVIATION_LEN: usize = 3;

/// File name of the manifest inside the logos folder.
const MANIFEST_FILE: &str = "manifest.toml";

/// Manufacturer names that don't match their file name directly.
///
/// The left side is the lowercased first word of `CarScreenName` as iRacing
/// writes it; the right side is the file in `assets/logos/`. Upstream names
/// Mercedes-Benz's file `mb`, and iRacing writes several manufacturers as a
/// hyphenated compound whose first word alone isn't the brand.
const ALIASES: &[(&str, &str)] = &[
    ("mercedes", "mb"),
    ("mercedes-amg", "mb"),
    ("mercedes-benz", "mb"),
    ("amg", "mb"),
    ("vw", "volkswagen"),
    ("chevy", "chevrolet"),
    ("corvette", "chevrolet"),
    ("land", "landrover"),
    ("land-rover", "landrover"),
    ("rolls-royce", "rolls-royce"),
    ("alfa", "alfa-romeo"),
    ("aston", "aston-martin"),
];

/// File stems whose display name isn't just the stem title-cased.
const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("mb", "Mercedes"),
    ("landrover", "Land Rover"),
    ("bmw", "BMW"),
    ("byd", "BYD"),
    ("gmc", "GMC"),
    ("ram", "RAM"),
    ("mclaren", "McLaren"),
    ("rolls-royce", "Rolls-Royce"),
    ("alfa-romeo", "Alfa Romeo"),
    ("aston-martin", "Aston Martin"),
];

/// The directory holding the mark files, resolved once per run.
fn logo_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| super::find_asset_dir("logos")).as_ref()
}

/// `assets/logos/manifest.toml`: the curated pick per brand, and which
/// colour files read on the overlay.
#[derive(Debug, Default, Deserialize)]
struct Manifest {
    /// Brand file stem → variant, for the Curated style. Unlisted brands
    /// get the white icon.
    #[serde(default)]
    curated: BTreeMap<String, LogoVariant>,
    /// Per shape, the brands whose colour file is legible on the dark ground.
    #[serde(default)]
    colour_legible: ColourLegible,
}

/// The `[colour_legible]` table, one list per shape.
#[derive(Debug, Default, Deserialize)]
struct ColourLegible {
    #[serde(default)]
    icon: BTreeSet<String>,
    #[serde(default)]
    badge: BTreeSet<String>,
    #[serde(default)]
    horizontal: BTreeSet<String>,
    #[serde(default)]
    wordmark: BTreeSet<String>,
}

impl ColourLegible {
    fn contains(&self, shape: LogoShape, brand: &str) -> bool {
        let set = match shape {
            LogoShape::Icon => &self.icon,
            LogoShape::Badge => &self.badge,
            LogoShape::Horizontal => &self.horizontal,
            LogoShape::Wordmark => &self.wordmark,
        };
        set.contains(brand)
    }
}

/// The manifest, read once per run.
///
/// A missing or unreadable manifest is a note on the console and a manifest
/// with nothing in it: every brand curated to the white icon, and colour
/// never chosen automatically. Marks still draw.
fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let Some(path) = logo_dir().map(|dir| dir.join(MANIFEST_FILE)) else {
            return Manifest::default();
        };
        match std::fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|text| toml::from_str::<Manifest>(&text).map_err(|err| err.to_string()))
        {
            Ok(manifest) => manifest,
            Err(err) => {
                println!("note: could not read {}: {err}; using the white icons", path.display());
                Manifest::default()
            }
        }
    })
}

/// The config in force, replaced by [`apply`] whenever it changes.
static ACTIVE: Mutex<Option<Arc<LogoConfig>>> = Mutex::new(None);

/// Makes `config` the one every mark is chosen by from now on.
///
/// Called by the app every frame; the shared copy is replaced only when the
/// config differs, so the per-frame cost is one comparison.
///
/// # Panics
/// If another thread panicked while holding the lock.
pub fn apply(config: &LogoConfig) {
    let mut active = ACTIVE.lock().expect("the logo config lock is poisoned");
    if active.as_deref() != Some(config) {
        *active = Some(Arc::new(config.clone()));
    }
}

/// The config in force, or the default before the app has applied one.
fn active() -> Arc<LogoConfig> {
    let active = ACTIVE.lock().expect("the logo config lock is poisoned");
    active.as_ref().map_or_else(|| Arc::new(LogoConfig::default()), Arc::clone)
}

/// Whether the current global style wants a wide slot (2:1) for every mark.
///
/// Slot width follows the global style only: a per-brand wide override under
/// a square style draws in the square slot, so rows in a column stay aligned.
#[must_use]
pub fn wide_slots() -> bool {
    active().style.shape().is_some_and(LogoShape::is_wide)
}

/// Why a brand resolved to the variant it did, for the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A per-brand override.
    Override,
    /// The manifest's curated pick (possibly its colour twin).
    Curated,
    /// The global style (possibly its colour twin).
    Style,
}

impl Reason {
    /// What the page calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Override => "Override",
            Self::Curated => "Curated",
            Self::Style => "Style",
        }
    }
}

/// The file stem a manufacturer name maps to: lowercased, then aliased.
///
/// `brand` is the first word of the car's display name, e.g. `"Mercedes-AMG"`
/// from `"Mercedes-AMG GT3"`.
#[must_use]
pub fn file_stem(brand: &str) -> String {
    let lower = brand.to_lowercase();
    ALIASES.iter().find(|(alias, _)| *alias == lower).map_or(lower, |(_, file)| (*file).to_owned())
}

/// The variant a brand file should be drawn in under `config`, and why.
#[must_use]
pub fn resolve(config: &LogoConfig, stem: &str) -> (LogoVariant, Reason) {
    if let Some(variant) = config.overrides.get(stem) {
        return (*variant, Reason::Override);
    }
    let manifest = manifest();
    let (base, reason) = match config.style.shape() {
        Some(shape) => (LogoVariant::mono(shape), Reason::Style),
        None => (manifest.curated.get(stem).copied().unwrap_or(LogoVariant::mono(LogoShape::Icon)), Reason::Curated),
    };
    if config.colour && !base.colour && manifest.colour_legible.contains(base.shape, stem) {
        return (LogoVariant { shape: base.shape, colour: true }, reason);
    }
    (base, reason)
}

/// What a manufacturer resolves to: a mark on disk, or a stand-in for one.
#[derive(Debug, Clone)]
enum Mark {
    /// A `file://` URI, ready to hand to `egui::Image` without building one.
    File(Arc<str>),
    /// The short uppercase stand-in for a manufacturer with no mark on disk.
    Abbreviation(Arc<str>),
}

/// What each (variant, brand) seen so far resolved to; see [`Memo`].
static MARKS: Memo<Mark> = Memo::new();

/// Resolves one brand file to its mark in `variant`, hitting the disk at
/// most once for each pair.
///
/// `label` is what the abbreviation is cut from when nothing is on disk —
/// the manufacturer's name as iRacing wrote it, or the stem for the grid.
fn mark_for(stem: &str, variant: LogoVariant, label: &str) -> Mark {
    let key = format!("{variant}/{stem}");
    MARKS.get_or_insert_with(&key, |_| match logo_path(stem, variant) {
        Some(path) => Mark::File(Arc::from(format!("file://{}", path.display()))),
        None => Mark::Abbreviation(Arc::from(abbreviation(label))),
    })
}

/// The first file on disk for `stem` in or near `variant`, if any.
///
/// The wanted folder, then the white twin, then the curated variant, then a
/// flat `<stem>.svg` — which is how a user adds a brand upstream lacks.
/// Touches the filesystem, so it is called only through [`mark_for`], which
/// remembers the answer.
#[must_use]
fn logo_path(stem: &str, variant: LogoVariant) -> Option<PathBuf> {
    let dir = logo_dir()?;
    let file = format!("{stem}.svg");
    let curated = manifest().curated.get(stem).copied().unwrap_or(LogoVariant::mono(LogoShape::Icon));
    let candidates = [
        dir.join(variant.folder()).join(&file),
        dir.join(variant.as_mono().folder()).join(&file),
        dir.join(curated.folder()).join(&file),
        dir.join(&file),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

/// A short uppercase stand-in for a manufacturer with no mark on disk.
#[must_use]
pub fn abbreviation(car_screen_name: &str) -> String {
    car_screen_name
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(ABBREVIATION_LEN)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Draws the manufacturer mark for `car_screen_name`, centred in `rect`.
///
/// The mark keeps its own aspect ratio and is scaled to the largest size that
/// fits, so `rect` is the space it may use rather than the shape it takes.
///
/// Falls back to [`abbreviation`] when no mark file matches. `dimmed` mutes
/// the mark to match a row that's been dimmed for being in the pits or laps
/// down, so the brand column doesn't stay bright on an otherwise grey row.
pub fn draw(ui: &mut Ui, rect: Rect, car_screen_name: &str, text_size: f32, dimmed: bool) {
    // `None` covers both an empty name and an all-whitespace one, which is what
    // a car whose driver metadata hasn't arrived yet reports.
    let Some(brand) = car_screen_name.split_whitespace().next() else {
        return;
    };
    let stem = file_stem(brand);
    let (variant, _) = resolve(&active(), &stem);
    draw_mark(ui, rect, &mark_for(&stem, variant, brand), text_size, dimmed);
}

/// Draws one brand file in the variant the config resolves it to.
///
/// For the settings page's grid, which works from file stems rather than
/// car names and wants exactly what the widgets would draw.
pub fn draw_brand(ui: &mut Ui, rect: Rect, stem: &str, text_size: f32, dimmed: bool) {
    let (variant, _) = resolve(&active(), stem);
    draw_mark(ui, rect, &mark_for(stem, variant, stem), text_size, dimmed);
}

/// Draws a resolved mark: the file, or the abbreviation.
fn draw_mark(ui: &mut Ui, rect: Rect, mark: &Mark, text_size: f32, dimmed: bool) {
    match mark {
        Mark::File(uri) => draw_file(ui, rect, uri, dimmed),
        Mark::Abbreviation(text) => {
            let color = if dimmed { text_tertiary_dim() } else { text_secondary() };
            paint_text(
                ui,
                rect.center(),
                egui::Align2::CENTER_CENTER,
                egui::RichText::new(&**text).size(text_size).strong().color(color),
            );
        }
    }
}

/// Draws a mark from disk, centred in `rect` at its own proportions.
fn draw_file(ui: &mut Ui, rect: Rect, uri: &str, dimmed: bool) {
    let tint = if dimmed { Color32::from_white_alpha(80) } else { Color32::WHITE };
    let image = egui::Image::new(uri).fit_to_exact_size(rect.size()).tint(tint);
    // `paint_at` fills whatever rectangle it is handed, whatever shape the
    // mark is, so the fitted size is worked out here and the mark centred
    // in the slot. Handing it the slot directly stretched every mark to
    // the slot's proportions — and since these are all square files, a
    // slot wider than it is tall squashed the lot of them.
    let fitted = image
        .load_and_calc_size(ui, rect.size())
        // The texture is still loading on the first frame it appears. The
        // marks on disk are square, so the largest square that fits is the
        // right guess for that one frame.
        .unwrap_or_else(|| egui::Vec2::splat(rect.size().min_elem()));
    image.paint_at(ui, Rect::from_center_size(rect.center(), fitted));
}

/// The abbreviation color on a dimmed row.
fn text_tertiary_dim() -> Color32 {
    Color32::from_white_alpha(70)
}

/// Every brand with a file somewhere under `assets/logos/`, by stem, sorted
/// by display name. Read once per run: the folder doesn't change while the
/// overlay runs.
#[must_use]
pub fn known_brands() -> &'static [String] {
    static BRANDS: OnceLock<Vec<String>> = OnceLock::new();
    BRANDS.get_or_init(|| {
        let mut stems: BTreeSet<String> = manifest().curated.keys().cloned().collect();
        if let Some(dir) = logo_dir() {
            let mut folders: Vec<PathBuf> = LogoShape::ALL
                .into_iter()
                .flat_map(|shape| {
                    [LogoVariant::mono(shape), LogoVariant { shape, colour: true }]
                        .map(|variant| dir.join(variant.folder()))
                })
                .collect();
            folders.push(dir.clone());
            for folder in folders {
                let Ok(entries) = std::fs::read_dir(folder) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
                        && let Some(stem) = path.file_stem()
                    {
                        stems.insert(stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
        let mut brands: Vec<String> = stems.into_iter().collect();
        brands.sort_by_key(|stem| display_name(stem).to_lowercase());
        brands
    })
}

/// What the settings page calls a brand file: `alfa-romeo` → "Alfa Romeo".
#[must_use]
pub fn display_name(stem: &str) -> String {
    if let Some((_, name)) = DISPLAY_NAMES.iter().find(|(known, _)| *known == stem) {
        return (*name).to_owned();
    }
    stem.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviates_the_manufacturer_not_the_model() {
        assert_eq!(abbreviation("Mercedes-AMG GT3"), "MER");
        assert_eq!(abbreviation("BMW M4 GT3"), "BMW");
        assert_eq!(abbreviation("Audi R8 LMS GT3 evo II"), "AUD");
    }

    /// `CarScreenName` is empty for a car whose driver metadata hasn't
    /// arrived yet; an abbreviation of nothing must stay empty rather than
    /// index into an empty string.
    #[test]
    fn an_empty_name_abbreviates_to_nothing() {
        assert_eq!(abbreviation(""), "");
        assert_eq!(abbreviation("   "), "");
    }

    /// Hyphens and other punctuation are dropped so the abbreviation reads
    /// as letters rather than as a fragment of the original name.
    #[test]
    fn strips_punctuation_from_abbreviations() {
        assert_eq!(abbreviation("Rolls-Royce Phantom"), "ROL");
    }

    #[test]
    fn aliases_map_iracing_names_onto_file_stems() {
        assert_eq!(file_stem("Mercedes-AMG"), "mb");
        assert_eq!(file_stem("BMW"), "bmw");
        assert_eq!(file_stem("Aston"), "aston-martin");
    }

    #[test]
    fn display_names_read_as_brands() {
        assert_eq!(display_name("alfa-romeo"), "Alfa Romeo");
        assert_eq!(display_name("mb"), "Mercedes");
        assert_eq!(display_name("porsche"), "Porsche");
    }

    /// An override wins over everything; the global style beats the
    /// curated pick; colour is only taken from the manifest's list.
    #[test]
    fn overrides_beat_style_beats_curated() {
        let mut config =
            LogoConfig { style: crate::config::LogoStyle::Badge, colour: false, overrides: BTreeMap::new() };
        assert_eq!(resolve(&config, "audi"), (LogoVariant::mono(LogoShape::Badge), Reason::Style));
        config.overrides.insert("audi".to_owned(), LogoVariant { shape: LogoShape::Wordmark, colour: true });
        assert_eq!(
            resolve(&config, "audi"),
            (LogoVariant { shape: LogoShape::Wordmark, colour: true }, Reason::Override)
        );
        // With no manifest on the test machine's path, curated is the white
        // icon and colour is never chosen automatically.
        let curated = LogoConfig { style: crate::config::LogoStyle::Curated, colour: true, overrides: BTreeMap::new() };
        let (variant, reason) = resolve(&curated, "zzz-no-such-brand");
        assert_eq!(reason, Reason::Curated);
        assert!(!variant.colour || manifest().colour_legible.contains(variant.shape, "zzz-no-such-brand"));
    }
}
