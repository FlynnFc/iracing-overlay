// Rust guideline compliant 2026-02-16

//! Small glyphs drawn with egui's painter rather than loaded from an icon
//! font: a road, a sun, fog, wind, a compass ring, a stopwatch, and the
//! circled letters in the Standings and Relative chrome.
//!
//! egui ships no icon set, and the fonts this overlay embeds (Inter, IBM
//! Plex Mono) have no pictographs, so every icon in the design mockups has
//! to be constructed from primitives. Each function fills the `rect` it is
//! given, so callers size icons by the rectangle they allocate rather than
//! by a parameter.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use egui::{Color32, Rect, Stroke, Ui};

use super::{Memo, paint_text};

/// The directory holding the interface icons, resolved once per run.
fn icon_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| super::find_asset_dir("icons")).as_ref()
}

/// Each icon's `file://` URI, or `None` where the file isn't there; see [`Memo`].
static URIS: Memo<Option<Arc<str>>> = Memo::new();

/// The URI for the named icon, hitting the disk at most once for each name.
fn icon_uri(name: &str) -> Option<Arc<str>> {
    URIS.get_or_insert_with(name, |name| {
        let path = icon_dir()?.join(format!("{name}.svg"));
        path.is_file().then(|| Arc::from(format!("file://{}", path.display())))
    })
}

/// Draws the named icon from `assets/icons/`, centred in `rect`, in `color`.
///
/// The icon keeps its own aspect ratio and is scaled to the largest size that
/// fits, so `rect` is the space it may use rather than the shape it takes.
///
/// Returns `false` when the file isn't there, so a caller can fall back to a
/// painter-drawn glyph rather than leaving a hole in the layout.
///
/// These are Lucide marks whose `currentColor` strokes have been rewritten to
/// white — see `assets/icons/README.md`. White is the identity under egui's
/// multiplicative tint, so one file renders in whatever colour is asked for.
pub fn svg(ui: &Ui, rect: Rect, name: &str, color: Color32) -> bool {
    let Some(uri) = icon_uri(name) else { return false };
    let image = egui::Image::new(&*uri).fit_to_exact_size(rect.size()).tint(color);
    // `paint_at` fills the rectangle it is handed regardless of the icon's own
    // proportions, so the fitted size is worked out here and the icon centred.
    // Lucide marks are square, which is also the fallback for the one frame
    // where the texture is still loading.
    let fitted = image.load_and_calc_size(ui, rect.size()).unwrap_or_else(|| egui::Vec2::splat(rect.size().min_elem()));
    image.paint_at(ui, Rect::from_center_size(rect.center(), fitted));
    true
}

/// Draws a road receding to a vanishing point, for the track-temp tile.
pub fn road(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(rect.width() * 0.09, color);
    let (top, bottom) = (rect.top(), rect.bottom());
    // The verges converge toward the top, giving the perspective read that
    // distinguishes this from a plain rectangle at icon sizes.
    let inset = rect.width() * 0.28;
    painter.line_segment([egui::pos2(rect.left() + inset, top), egui::pos2(rect.left(), bottom)], stroke);
    painter.line_segment([egui::pos2(rect.right() - inset, top), egui::pos2(rect.right(), bottom)], stroke);

    // Center dashes, shortening and narrowing with distance.
    let dash_stroke = Stroke::new(rect.width() * 0.08, color);
    for (start, end) in [(0.06_f32, 0.26_f32), (0.40, 0.64), (0.78, 1.0)] {
        painter.line_segment(
            [
                egui::pos2(rect.center().x, top + rect.height() * start),
                egui::pos2(rect.center().x, top + rect.height() * end),
            ],
            dash_stroke,
        );
    }
}

/// Draws a cloud with drifting lines beneath it, for the fog tile.
pub fn fog(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    // A cloud as three overlapping discs on a common baseline, which reads
    // correctly at icon size without needing a bezier outline.
    let baseline = rect.top() + rect.height() * 0.46;
    for (dx, dy, r) in [(-0.22_f32, 0.02_f32, 0.17_f32), (0.0, -0.06, 0.23), (0.24, 0.02, 0.19)] {
        let center = egui::pos2(rect.center().x + rect.width() * dx, baseline + rect.height() * dy);
        painter.circle_filled(center, rect.width() * r, color);
    }
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(rect.center().x - rect.width() * 0.4, baseline - rect.height() * 0.02),
            egui::pos2(rect.center().x + rect.width() * 0.42, baseline + rect.height() * 0.2),
        ),
        0.0,
        color,
    );

    let stroke = Stroke::new(rect.height() * 0.075, color);
    for (i, width) in [0.9_f32, 0.72, 0.84].into_iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "a loop counter below four")]
        let y = baseline + rect.height() * (0.32 + 0.16 * i as f32);
        let half = rect.width() * width * 0.5;
        painter.line_segment([egui::pos2(rect.center().x - half, y), egui::pos2(rect.center().x + half, y)], stroke);
    }
}

/// Draws the skid mark a dash's traction-control light uses: a car from
/// behind over two wavy lines — the off-track marker.
pub fn skid(ui: &Ui, rect: Rect, color: Color32) {
    let width = rect.width();
    let height = rect.height();
    // The car: a wide body with a narrower cabin on top, seen from behind.
    let body = Rect::from_min_max(
        egui::pos2(rect.left() + width * 0.12, rect.top() + height * 0.28),
        egui::pos2(rect.right() - width * 0.12, rect.top() + height * 0.56),
    );
    let cabin = Rect::from_min_max(
        egui::pos2(rect.left() + width * 0.28, rect.top() + height * 0.08),
        egui::pos2(rect.right() - width * 0.28, rect.top() + height * 0.32),
    );
    ui.painter().rect_filled(cabin, width * 0.08, color);
    ui.painter().rect_filled(body, width * 0.08, color);
    // Two wavy skid lines under it.
    let stroke = Stroke::new((width * 0.09).max(1.0), color);
    for line_x in [rect.left() + width * 0.22, rect.right() - width * 0.22] {
        let points: Vec<egui::Pos2> = (0..=8_u8)
            .map(|step| {
                let along = f32::from(step) / 8.0;
                egui::pos2(
                    line_x + (along * std::f32::consts::TAU).sin() * width * 0.07,
                    rect.top() + height * (0.66 + 0.32 * along),
                )
            })
            .collect();
        ui.painter().add(egui::Shape::line(points, stroke));
    }
}

/// Draws a stopwatch: a dial with a crown and a hand, marking a fastest lap.
pub fn stopwatch(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let center = rect.center() + egui::vec2(0.0, rect.height() * 0.06);
    let radius = rect.width().min(rect.height()) * 0.36;
    let stroke = Stroke::new(rect.width() * 0.09, color);
    painter.circle_stroke(center, radius, stroke);
    // Crown, and the hand pointing to roughly one o'clock.
    painter.line_segment(
        [egui::pos2(center.x, center.y - radius), egui::pos2(center.x, center.y - radius - rect.height() * 0.14)],
        stroke,
    );
    painter.line_segment([center, center + egui::vec2(radius * 0.5, -radius * 0.5)], stroke);
}

/// Draws a letter inside a circle — the Standings header's session-type mark
/// and the Relative footer's ABS chip.
pub fn circled_text(ui: &Ui, rect: Rect, text: &str, color: Color32, text_size: f32) {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) / 2.0;
    ui.painter().circle_stroke(center, radius, Stroke::new(rect.width() * 0.06, color));
    paint_text(
        ui,
        center,
        egui::Align2::CENTER_CENTER,
        egui::RichText::new(text).size(text_size).strong().color(color),
    );
}

/// Draws the car silhouette used beside a car count.
pub fn car(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    // Body: a rounded slab with a narrower cabin sitting on top of it.
    let body = Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + rect.height() * 0.42),
        egui::pos2(rect.right(), rect.bottom() - rect.height() * 0.16),
    );
    painter.rect_filled(body, rect.height() * 0.18, color);
    let cabin = Rect::from_min_max(
        egui::pos2(rect.left() + rect.width() * 0.24, rect.top() + rect.height() * 0.14),
        egui::pos2(rect.right() - rect.width() * 0.2, body.top() + rect.height() * 0.08),
    );
    painter.rect_filled(cabin, rect.height() * 0.14, color);

    let wheel_r = rect.height() * 0.13;
    for x in [rect.left() + rect.width() * 0.24, rect.right() - rect.width() * 0.24] {
        painter.circle_filled(egui::pos2(x, rect.bottom() - wheel_r * 0.7), wheel_r, color);
    }
}

/// Draws a small multiplication sign, the Relative header's incident mark.
pub fn cross(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(rect.width() * 0.14, color);
    painter.line_segment([rect.left_top(), rect.right_bottom()], stroke);
    painter.line_segment([rect.right_top(), rect.left_bottom()], stroke);
}
