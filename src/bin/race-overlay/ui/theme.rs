// Rust guideline compliant 2026-02-16

//! The visual language the panels are drawn in, and the tokens that differ
//! between them — see `plans/instrument-theme.md`.
//!
//! Two styles ship. **Panel** is the overlay's original look: filled
//! near-black cards with a drop shadow, no border, and a muted semantic
//! accent set. **Instrument** is a hardware-display look — the same card
//! but bordered in near-white rather than shadowed, its inner tiles reduced to
//! outlines, and the accents replaced by the saturated set a hardware display
//! uses.
//!
//! Only the tokens that actually differ live here. Everything a theme does not
//! change — type, spacing, geometry — stays where it was in [`super`], because
//! a token that cannot vary is not a theme's business.
//!
//! The active theme is a process-wide atomic rather than a value threaded
//! through every draw call. Every widget reads it dozens of times a frame and
//! it changes at most once per frame, from the settings window, which is
//! exactly the shape [`super::logos`] already uses for the same reason. An
//! atomic rather than that module's `Mutex`, because these are read on the
//! hot path: a relaxed load is a single instruction, where a lock is not.

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{Color32, Stroke};
use serde::{Deserialize, Serialize};

use super::Metrics;

/// Which visual language the panels are drawn in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Filled cards, drop shadows, muted semantic accents — the original.
    #[default]
    Panel,
    /// Bordered cards, outlined tiles, saturated display accents, applied to
    /// the panels that opt into it.
    Instrument,
}

impl Theme {
    /// Every theme, in the order the settings page lists them.
    pub const ALL: [Self; 2] = [Self::Panel, Self::Instrument];

    /// What the settings page calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Panel => "Panel",
            Self::Instrument => "Instrument",
        }
    }

    /// The one-line description under the selector.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            Self::Panel => "Filled cards with soft accents \u{2014} the original look.",
            Self::Instrument => "Bordered cards with display accents, like a DDU.",
        }
    }
}

/// The active theme, as a [`Theme`] discriminant.
///
/// `u8` rather than the enum because atomics are only defined over integers;
/// [`current`] maps it back, treating anything unrecognised as the default so
/// a torn or future value can never panic a frame.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// Sets the theme every subsequent draw call reads. Called once a frame.
pub fn apply(theme: Theme) {
    ACTIVE.store(theme as u8, Ordering::Relaxed);
}

/// The theme in force.
#[must_use]
pub fn current() -> Theme {
    match ACTIVE.load(Ordering::Relaxed) {
        1 => Theme::Instrument,
        _ => Theme::Panel,
    }
}

/// Whether the Instrument language is in force, for the handful of places
/// that change shape rather than colour.
#[must_use]
pub fn is_instrument() -> bool {
    current() == Theme::Instrument
}

// ---- Accents -------------------------------------------------------------
//
// Instrument's set is sampled from a real DDU cluster: a
// hardware display uses the full gamut, so these are saturated where the
// Panel set is deliberately muted. Each function keeps the Panel constant's
// meaning, so a call site reads the same either way.

/// Good, close, ahead, on-pace.
#[must_use]
pub fn signal() -> Color32 {
    match current() {
        Theme::Panel => super::SIGNAL,
        Theme::Instrument => Color32::from_rgb(0x00, 0xE6, 0x3C),
    }
}

/// Bad, far, in the pits, behind.
#[must_use]
pub fn alert() -> Color32 {
    match current() {
        Theme::Panel => super::ALERT,
        Theme::Instrument => Color32::from_rgb(0xF0, 0x1C, 0x1C),
    }
}

/// Neutral attention: driver aids, caution flags, an armed service.
#[must_use]
pub fn caution() -> Color32 {
    match current() {
        Theme::Panel => super::CAUTION,
        Theme::Instrument => Color32::from_rgb(0xE8, 0xC8, 0x20),
    }
}

/// A car the player has lapped.
#[must_use]
pub fn lapped() -> Color32 {
    match current() {
        Theme::Panel => super::LAPPED,
        Theme::Instrument => Color32::from_rgb(0x2C, 0xB6, 0xF0),
    }
}

/// The fastest lap of the session, and its chip.
#[must_use]
pub fn fastest() -> Color32 {
    match current() {
        Theme::Panel => super::FASTEST,
        Theme::Instrument => Color32::from_rgb(0xC7, 0x0C, 0xCF),
    }
}

/// The lap time printed inside a fastest-lap cell.
#[must_use]
pub fn fastest_text() -> Color32 {
    match current() {
        Theme::Panel => super::FASTEST_TEXT,
        Theme::Instrument => Color32::from_rgb(0xF0, 0x8C, 0xF5),
    }
}

/// The fill behind a fastest-lap time.
#[must_use]
pub fn fastest_cell() -> Color32 {
    match current() {
        Theme::Panel => super::FASTEST_CELL,
        Theme::Instrument => Color32::from_rgb(0x3A, 0x06, 0x3C),
    }
}

// ---- Surfaces ------------------------------------------------------------

/// A card's border, and `None` where the theme uses a shadow instead.
///
/// This is the difference a glance actually registers: Panel separates a card
/// from the track by casting a shadow, Instrument by drawing an edge.
#[must_use]
pub fn card_stroke(metrics: Metrics) -> Option<Stroke> {
    match current() {
        Theme::Panel => None,
        Theme::Instrument => Some(Stroke::new(metrics.px(1.5), super::instrument_border())),
    }
}

/// The radius Instrument draws its bezelled cards at.
pub const INSTRUMENT_CARD_ROUNDING: f32 = 12.0;

/// How opaque a themed card's fill is, as a multiplier on the fill the widget
/// asked for.
///
/// Instrument leans on an outline rather than a shadow to separate a card from
/// the track, and an outline over a translucent fill reads as a wire frame
/// with the world showing through it. Backing the fill toward opaque is what
/// keeps the border reading as a bezel — see the open question in
/// `plans/instrument-theme.md`, which this answers.
#[must_use]
pub fn card_fill(fill: Color32) -> Color32 {
    match current() {
        Theme::Panel => fill,
        Theme::Instrument => {
            // Premultiplied, so the channels are already scaled by the alpha
            // they came with; re-scaling them to the new alpha keeps the ink
            // colour and only takes the track out from behind it.
            let alpha = f32::from(fill.a()).max(1.0);
            let lift = |channel: u8| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a channel scaled by a ratio of at most 255/alpha stays inside u8 after the clamp"
                )]
                let scaled = (f32::from(channel) * INSTRUMENT_CARD_ALPHA / alpha).clamp(0.0, 255.0) as u8;
                scaled
            };
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a constant below 255")]
            let opacity = INSTRUMENT_CARD_ALPHA as u8;
            Color32::from_rgba_premultiplied(lift(fill.r()), lift(fill.g()), lift(fill.b()), opacity)
        }
    }
}

/// The alpha an Instrument card's fill is taken to. Not fully opaque: the
/// overlay is still an overlay, and a sliver of track through it is what keeps
/// it from reading as a hole punched in the screen.
const INSTRUMENT_CARD_ALPHA: f32 = 232.0;

/// A tile inside a card — the Standings timing band, the Weather stat tiles.
///
/// Instrument hollows these out: the border is doing the separating, so a
/// second filled surface inside a bordered one is a box in a box.
#[must_use]
pub fn tile_fill(fill: Color32) -> Color32 {
    match current() {
        Theme::Panel => fill,
        Theme::Instrument => Color32::TRANSPARENT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The atomic round-trips, and an unknown discriminant falls back rather
    /// than panicking a frame.
    #[test]
    fn the_active_theme_round_trips() {
        apply(Theme::Instrument);
        assert_eq!(current(), Theme::Instrument);
        assert!(is_instrument());
        apply(Theme::Panel);
        assert_eq!(current(), Theme::Panel);
        assert!(!is_instrument());

        ACTIVE.store(200, Ordering::Relaxed);
        assert_eq!(current(), Theme::Panel, "an unrecognised discriminant reads as the default");
        apply(Theme::Panel);
    }

    /// Instrument takes a translucent card toward opaque while keeping its
    /// ink, so a bordered card does not read as a wire frame.
    #[test]
    fn instrument_backs_a_card_fill_toward_opaque() {
        apply(Theme::Panel);
        let translucent = Color32::from_rgba_premultiplied(12, 12, 12, 170);
        assert_eq!(card_fill(translucent), translucent, "Panel leaves a fill alone");

        apply(Theme::Instrument);
        let lifted = card_fill(translucent);
        assert!(lifted.a() > translucent.a(), "the fill backs toward opaque");
        // The ink is unchanged: the channels scale with the alpha, so the
        // colour behind the alpha stays the same near-black.
        let before = f32::from(translucent.r()) / f32::from(translucent.a());
        let after = f32::from(lifted.r()) / f32::from(lifted.a());
        assert!((before - after).abs() < 0.02, "the ink colour survives the lift");
        apply(Theme::Panel);
    }

    /// Every theme has to be namable and describable for the settings page.
    #[test]
    fn every_theme_is_labelled() {
        for theme in Theme::ALL {
            assert!(!theme.label().is_empty());
            assert!(!theme.hint().is_empty());
        }
    }
}
