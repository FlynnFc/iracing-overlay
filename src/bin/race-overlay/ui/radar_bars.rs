// Rust guideline compliant 2026-02-16

//! Renders the Radar Bars widget: where the cars beside you actually are.
//!
//! Two dark capsules, one per side of the car. Each is a map of the track
//! immediately beside you, drawn to scale in metres — the middle is your own
//! car, the top is `range_cars` car lengths up the road, the bottom the same
//! distance back.
//!
//! # Why it is drawn this way
//!
//! The question a driver asks of a radar is never "how many milliseconds is
//! that car away". It is "is there a car in the space I want to move into",
//! and the honest answer is a shape, not a number. So three things are drawn
//! literally rather than encoded:
//!
//! - **Your own car is on the bar**, framed by two bumper bars exactly one car
//!   length apart at the middle. Overlap is a relationship between two cars;
//!   drawing only one of them leaves the driver to supply the other.
//! - **The other car is drawn at its true length**, on the same scale, and the
//!   part of it level with you is lit. So visual overlap *is* actual overlap,
//!   and "can I move over half a car length" is answered by looking rather
//!   than by converting.
//! - **Colour encodes threat, not direction.** Position already says ahead or
//!   behind. Slate is a car with room around it, amber one within a car
//!   length, red one overlapping — and a red car also lights the inboard edge
//!   of its bar as a solid rail, the one mark meant to be caught at the very
//!   edge of vision with your eyes on the apex.
//!
//! A car closing or dropping away trails a tail showing where it was
//! [`CLOSING_WINDOW_SECS`] ago, so the rate is a length and a direction rather
//! than another number to read. No tail means a stalemate: nothing is about to
//! resolve, hold your line.
//!
//! A car beyond the drawn range gets a thin pip at the end of its bar instead
//! of being clamped to the end, which would put it exactly where "right beside
//! you" is drawn.
//!
//! There is no card and no header: this is an ambient indicator that should be
//! invisible until a car is actually alongside, not a permanent panel.

use egui::{Color32, Rect, RichText, Stroke, Ui};

use super::theme;
use super::{Metrics, RADAR_CLEAR, RADAR_CLOSE, RADAR_PLAYER, dashed_line_h, gradient_capsule_v, paint_text};
use crate::config::RadarConfig;
use crate::telemetry::radar::{CLOSING_WINDOW_SECS, Threat, clear_gap_metres, threat};
use crate::telemetry::snapshot::{RadarCar, RadarSide, TelemetrySnapshot};

/// Guards on hand-edited configs: a capsule smaller than this stops being a
/// readable map, and a zero height would put every car at the middle. The
/// settings sliders stay well above both; the mockup's own size and the gap
/// live in the config defaults — see `config::default_radar_bar_size`.
const MIN_BAR_WIDTH: f32 = 40.0;
const MIN_BAR_HEIGHT: f32 = 150.0;

/// How far a car marker is inset from the capsule's edges, in design pixels.
const MARKER_INSET: f32 = 7.0;

/// Type scale for the optional gap readouts.
const LABEL_SIZE: f32 = 20.0;

/// Smallest closing rate that draws a tail, in m/s.
///
/// Below roughly walking pace the two cars are holding station, and a tail
/// that twitches with the last decimal of the gap would read as movement that
/// isn't there.
const MIN_TAIL_MPS: f32 = 0.35;

/// A guard on hand-edited configs: a car length at or below this would divide
/// the whole axis by nothing.
const MIN_CAR_LENGTH_M: f32 = 1.0;

/// Which side of the car a bar stands for, so its rail can light on the edge
/// facing the driver's view rather than the edge facing away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

pub fn draw(ui: &mut Ui, snapshot: Option<&TelemetrySnapshot>, config: &RadarConfig) {
    let Some(radar) = snapshot.map(|s| s.radar) else {
        return;
    };
    if is_clear(radar.left) && is_clear(radar.right) {
        return; // nothing nearby: draw nothing at all, not even an empty shell
    }

    let metrics = Metrics::new(config.scale);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        bar(ui, metrics, radar.left, Side::Left, config);
        ui.add_space(metrics.px(config.bar_gap.max(0.0)));
        bar(ui, metrics, radar.right, Side::Right, config);
    });
}

/// The configured capsule size with the hand-edit guards applied.
fn bar_size(config: &RadarConfig) -> [f32; 2] {
    [config.bar_size[0].max(MIN_BAR_WIDTH), config.bar_size[1].max(MIN_BAR_HEIGHT)]
}

fn is_clear(side: RadarSide) -> bool {
    side.ahead.is_none() && side.behind.is_none()
}

/// The scale one bar is drawn at, derived once from the configured car length.
#[derive(Debug, Clone, Copy)]
struct Scale {
    /// Screen pixels per metre of track.
    px_per_m: f32,
    /// Screen pixels one car length occupies.
    car_px: f32,
    /// Metres of track from the middle of the bar to either end.
    range_m: f32,
    car_length_m: f32,
}

impl Scale {
    fn new(bar_height: f32, config: &RadarConfig) -> Self {
        let car_length_m = config.car_length_m.max(MIN_CAR_LENGTH_M);
        // At least one car length either side, or the player's own slot would
        // not fit on the bar it is the reference for.
        let range_cars = config.range_cars.max(1.0);
        let car_px = bar_height / (2.0 * range_cars);
        Self { px_per_m: car_px / car_length_m, car_px, range_m: car_length_m * range_cars, car_length_m }
    }
}

/// One bar's geometry, worked out once and handed to everything drawn in it.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    /// The capsule itself.
    bar: Rect,
    /// The player's own car length, fixed at the middle of the capsule. The
    /// reference every other mark on the bar is read against.
    slot: Rect,
    scale: Scale,
}

impl Geometry {
    fn new(metrics: Metrics, bar: Rect, config: &RadarConfig) -> Self {
        let scale = Scale::new(bar.height(), config);
        let slot = Rect::from_center_size(bar.center(), egui::vec2(bar.width(), scale.car_px))
            .shrink2(egui::vec2(metrics.px(MARKER_INSET), 0.0));
        Self { bar, slot, scale }
    }

    /// Where a car `separation_m` up the road sits, as a rect of its own length.
    ///
    /// Clamped to the capsule so a car near the end is truncated by the bar's
    /// edge — which reads as "part of it is past the end" — rather than
    /// spilling out over the track.
    fn block(self, metrics: Metrics, separation_m: f32) -> Rect {
        let center_y = self.bar.center().y - separation_m * self.scale.px_per_m;
        Rect::from_center_size(egui::pos2(self.bar.center().x, center_y), self.slot.size())
            .intersect(self.bar.shrink(metrics.px(2.0)))
    }
}

/// Draws one side's capsule, the player's own slot, and whichever cars that
/// side currently holds.
///
/// The capsule is drawn for both sides whenever the widget is visible at all,
/// so the two sides don't jump around as cars come and go.
fn bar(ui: &mut Ui, metrics: Metrics, side: RadarSide, which: Side, config: &RadarConfig) {
    let [width, height] = bar_size(config);
    let (rect, _response) = ui.allocate_exact_size(metrics.vec2(width, height), egui::Sense::hover());
    let radius = rect.width() / 2.0;

    // The capsule fades down its length, matching the mockup — dark enough
    // to sit over bright track without becoming a solid slab in the driver's
    // peripheral vision.
    ui.painter().rect_filled(rect, radius, Color32::from_black_alpha(120));
    gradient_capsule_v(
        ui,
        rect.shrink(1.0),
        Color32::from_rgba_unmultiplied(60, 60, 64, 90),
        Color32::from_rgba_unmultiplied(16, 16, 18, 140),
    );

    let geometry = Geometry::new(metrics, rect, config);
    car_length_ticks(ui, metrics, geometry);

    // Cars first, then the player's frame on top: a car overlapping the frame
    // must not bury the marks that say where the player's own bodywork ends,
    // since those edges are the whole reading.
    let mut worst = Threat::Clear;
    for car in [side.ahead, side.behind].into_iter().flatten() {
        worst = worse_of(worst, marker(ui, metrics, geometry, car, config.show_numbers));
    }
    player_frame(ui, metrics, geometry);

    if worst == Threat::Overlapping {
        overlap_rail(ui, metrics, rect, which);
    }
}

/// Which way along the bar this car lies from the player, in screen pixels.
///
/// Positive separation is ahead, which is up the bar, which is negative on
/// screen — so everything drawn relative to a car scales its offset by this
/// rather than repeating the sign flip.
fn outward_sign(car: RadarCar) -> f32 {
    if car.separation_m >= 0.0 { -1.0 } else { 1.0 }
}

/// The more urgent of two threats.
fn worse_of(a: Threat, b: Threat) -> Threat {
    let rank = |threat| match threat {
        Threat::Clear => 0_u8,
        Threat::Close => 1,
        Threat::Overlapping => 2,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// Faint rules where each further car length begins, as a ruler for the
/// half-a-length judgement the widget exists to answer.
///
/// The first boundary either side of the player is the edge of their own slot,
/// which is already drawn, so ticks start at one and a half car lengths out.
fn car_length_ticks(ui: &Ui, metrics: Metrics, geometry: Geometry) {
    let (bar, scale) = (geometry.bar, geometry.scale);
    let mut offset = 1.5;
    while offset * scale.car_px < bar.height() / 2.0 {
        for y in [bar.center().y - offset * scale.car_px, bar.center().y + offset * scale.car_px] {
            dashed_line_h(
                ui,
                y,
                bar.left() + metrics.px(MARKER_INSET),
                bar.right() - metrics.px(MARKER_INSET),
                metrics.px(5.0),
                metrics.px(9.0),
                Stroke::new(metrics.px(2.0), Color32::from_white_alpha(38)),
            );
        }
        offset += 1.0;
    }
}

/// The player's own car: two heavy white bumper bars a car length apart, with
/// short stubs turning down their corners.
///
/// Deliberately a frame rather than a filled box. A box drawn here is the same
/// shape as the cars around it, and the driver ends up asking which of the
/// three blocks on the bar is them — the one question this widget must never
/// provoke. Bumper bars say "the track between these two lines is you", which
/// no marker can be confused with, and leave a car overlapping the frame fully
/// visible through it.
fn player_frame(ui: &Ui, metrics: Metrics, geometry: Geometry) {
    let slot = geometry.slot;
    let thickness = metrics.px(5.0);
    let stub = (slot.height() * 0.22).min(metrics.px(18.0));
    for y in [slot.top(), slot.bottom()] {
        let bumper = Rect::from_center_size(egui::pos2(slot.center().x, y), egui::vec2(slot.width(), thickness));
        ui.painter().rect_filled(bumper, thickness / 2.0, RADAR_PLAYER);
    }
    // The stubs close the frame just enough to read as one shape rather than
    // two unrelated rules, without walling the car marker in behind an outline.
    for x in [slot.left() + thickness / 2.0, slot.right() - thickness / 2.0] {
        for (from, to) in [(slot.top(), slot.top() + stub), (slot.bottom() - stub, slot.bottom())] {
            let post = Rect::from_x_y_ranges(x - thickness / 2.0..=x + thickness / 2.0, from..=to);
            ui.painter().rect_filled(post, thickness / 2.0, RADAR_PLAYER);
        }
    }
}

/// Draws one car: its tail, its block, and optionally its gap readout.
///
/// Returns how much of a threat that car is, so the bar can decide whether to
/// light its rail.
fn marker(ui: &Ui, metrics: Metrics, geometry: Geometry, car: RadarCar, show_numbers: bool) -> Threat {
    let scale = geometry.scale;
    let clear_gap_m = clear_gap_metres(car.separation_m, scale.car_length_m);
    let state = threat(clear_gap_m, scale.car_length_m);
    let color = match state {
        Threat::Clear => RADAR_CLEAR,
        Threat::Close => RADAR_CLOSE,
        Threat::Overlapping => theme::alert(),
    };

    let outward = outward_sign(car);
    if car.separation_m.abs() > scale.range_m {
        return off_range_pip(ui, metrics, geometry.bar, outward, color);
    }

    let block = geometry.block(metrics, car.separation_m);
    tail(ui, metrics, geometry, car, block.center().y, color);
    ui.painter().rect_filled(block, metrics.px(6.0), color);
    overlap_shading(ui, metrics, geometry, block);

    if show_numbers {
        gap_readout(ui, metrics, geometry.bar, block, clear_gap_m, outward);
    }
    state
}

/// Lightens exactly the part of a car that is level with the player.
///
/// The frame already says where the player's own bodywork is, so this is
/// strictly redundant — and worth it anyway. "How much of him is beside me" is
/// the question, and shading the answer turns it from a comparison of two
/// edges into a single lit band whose height *is* the overlap.
fn overlap_shading(ui: &Ui, metrics: Metrics, geometry: Geometry, block: Rect) {
    let shared = block.intersect(geometry.slot);
    if shared.height() <= 0.0 {
        return;
    }
    ui.painter().rect_filled(shared, metrics.px(6.0), Color32::from_white_alpha(90));
}

/// A mark at the end of the bar for a car past the drawn range.
///
/// Deliberately not the full block clamped to the end: that is the shape that
/// means "level with you", and a car three seconds up the road wearing it is
/// the single most misleading thing this widget could draw. Drawn at full
/// strength rather than dimmed, because a mark that can't be seen at all is
/// the same as not drawing it.
fn off_range_pip(ui: &Ui, metrics: Metrics, bar: Rect, outward: f32, color: Color32) -> Threat {
    let height = metrics.px(14.0);
    let center_y = bar.center().y + outward * (bar.height() / 2.0 - height);
    let pip = Rect::from_center_size(egui::pos2(bar.center().x, center_y), egui::vec2(bar.width(), height))
        .shrink2(egui::vec2(metrics.px(MARKER_INSET + 10.0), 0.0));
    ui.painter().rect_filled(pip, height / 2.0, color);
    Threat::Clear
}

/// A streak trailing the car's block, back along the ground it just covered.
///
/// Length is how fast the gap is changing, direction is which way. A car
/// closing on the player trails *away* from them, because that is where it
/// came from; one dropping back trails toward them. Either way the streak
/// leaves the block's far edge, so even a slow drift shows something rather
/// than hiding under the block it belongs to.
#[expect(clippy::cast_possible_truncation, reason = "a sub-second window, far inside f32")]
fn tail(ui: &Ui, metrics: Metrics, geometry: Geometry, car: RadarCar, center_y: f32, color: Color32) {
    let Some(closing_mps) = car.closing_mps.filter(|rate| rate.abs() >= MIN_TAIL_MPS) else {
        return;
    };
    let bar = geometry.bar;
    // Positive closing means the gap shrank, so a moment ago the car sat
    // further from the player; negative means it sat nearer.
    let trailing = if closing_mps > 0.0 { outward_sign(car) } else { -outward_sign(car) };
    let travelled_px = (closing_mps * CLOSING_WINDOW_SECS as f32 * geometry.scale.px_per_m).abs();
    let start_y = center_y + trailing * geometry.slot.height() / 2.0;
    let end_y = (start_y + trailing * travelled_px).clamp(bar.top(), bar.bottom());
    let streak = Rect::from_x_y_ranges(
        bar.center().x - bar.width() * 0.18..=bar.center().x + bar.width() * 0.18,
        start_y.min(end_y)..=start_y.max(end_y),
    );
    ui.painter().rect_filled(streak, metrics.px(5.0), color.gamma_multiply(0.5));
}

/// The optional readout: metres of clear air, negative once overlapping.
fn gap_readout(ui: &Ui, metrics: Metrics, bar: Rect, block: Rect, clear_gap_m: f32, outward: f32) {
    // Placed on the far side of the block from the player, where it cannot
    // land on top of the player's own slot.
    let center_y = if outward < 0.0 { block.top() - metrics.px(16.0) } else { block.bottom() + metrics.px(16.0) };
    let plate = Rect::from_center_size(
        egui::pos2(bar.center().x, center_y),
        egui::vec2(bar.width() - metrics.px(20.0), metrics.px(26.0)),
    );
    ui.painter().rect_filled(plate, metrics.px(3.0), Color32::from_black_alpha(200));
    paint_text(
        ui,
        plate.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new(format!("{clear_gap_m:.1}"))
            .monospace()
            .size(metrics.px(LABEL_SIZE))
            .strong()
            .color(Color32::WHITE),
    );
}

/// A solid rail down the edge of the bar facing the driver, lit while a car
/// overlaps them.
///
/// The one mark on this widget designed to be read without looking at it: a
/// high-contrast vertical line arriving in peripheral vision on the side the
/// car is on, which reads as a wall rather than as information.
fn overlap_rail(ui: &Ui, metrics: Metrics, bar: Rect, which: Side) {
    let width = metrics.px(9.0);
    let rail = match which {
        Side::Left => Rect::from_x_y_ranges(bar.right() - width..=bar.right(), bar.y_range()),
        Side::Right => Rect::from_x_y_ranges(bar.left()..=bar.left() + width, bar.y_range()),
    };
    ui.painter().rect_filled(rail.shrink2(egui::vec2(0.0, metrics.px(10.0))), width / 2.0, theme::alert());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(separation_m: f32) -> RadarCar {
        RadarCar { separation_m, gap_ms: separation_m / 55.0 * 1000.0, closing_mps: None }
    }

    #[test]
    fn a_side_with_no_cars_reads_clear() {
        assert!(is_clear(RadarSide::default()));
        assert!(!is_clear(RadarSide { ahead: Some(car(12.0)), behind: None }));
        assert!(!is_clear(RadarSide { ahead: None, behind: Some(car(-4.0)) }));
    }

    #[test]
    fn the_worse_threat_wins_the_bar() {
        assert_eq!(worse_of(Threat::Clear, Threat::Close), Threat::Close);
        assert_eq!(worse_of(Threat::Overlapping, Threat::Clear), Threat::Overlapping);
        assert_eq!(worse_of(Threat::Close, Threat::Close), Threat::Close);
    }

    /// The scale is the promise the whole widget rests on: a car length of
    /// track must be a car length of bar, whatever the configured length.
    #[test]
    fn one_car_length_of_track_is_one_car_length_of_bar() {
        let config = RadarConfig { car_length_m: 4.7, range_cars: 3.0, ..RadarConfig::default() };
        let scale = Scale::new(705.0, &config);
        assert!((scale.car_px - 705.0 / 6.0).abs() < 0.01);
        assert!((scale.car_length_m * scale.px_per_m - scale.car_px).abs() < 0.01);
        assert!((scale.range_m - 14.1).abs() < 0.01);
    }

    /// Half a car length is the movement the widget is meant to make
    /// judgeable, so it has to be a real number of pixels at the default size.
    #[test]
    fn half_a_car_length_is_a_visible_distance() {
        let scale = Scale::new(705.0, &RadarConfig::default());
        assert!(scale.car_px / 2.0 > 50.0, "half a car length drew only {} px", scale.car_px / 2.0);
    }

    /// A hand-edited config must not be able to shrink a capsule to nothing;
    /// the configured size passes through untouched otherwise.
    #[test]
    #[expect(clippy::float_cmp, reason = "an untouched config must pass through bit-for-bit, not approximately")]
    fn a_nonsense_bar_size_is_clamped_to_something_drawable() {
        let nonsense = RadarConfig { bar_size: [0.0, -5.0], ..RadarConfig::default() };
        let [width, height] = bar_size(&nonsense);
        assert!(width >= MIN_BAR_WIDTH && height >= MIN_BAR_HEIGHT);
        assert_eq!(bar_size(&RadarConfig::default()), RadarConfig::default().bar_size);
    }

    /// A hand-edited config must not be able to divide the axis by nothing.
    #[test]
    fn a_nonsense_car_length_still_yields_a_usable_scale() {
        let config = RadarConfig { car_length_m: 0.0, range_cars: 0.0, ..RadarConfig::default() };
        let scale = Scale::new(705.0, &config);
        assert!(scale.px_per_m.is_finite() && scale.px_per_m > 0.0);
        assert!(scale.car_px > 0.0 && scale.car_px <= 705.0);
    }
}
