// Rust guideline compliant 2026-02-16

//! Renders the Faster Class widget: a quicker class is coming up behind you.
//!
//! One card, three rows, that exists only while it is true. The top row says
//! *who*: the class on its own colour block, then the car number and the
//! driver, so the mirror can be matched to the warning. The middle row says
//! *how far*: seconds behind, in the readout face, with the state beside it —
//! `BEHIND`, `CLOSE`, `PASSING` — and, when the closing rate is honest enough
//! to say, how long until the car is on the bumper. The bottom row is the
//! same fact as a shape: a bar with you at its left end and every quicker
//! car inside the warning span sliding toward you as its own class plate —
//! the same colour-and-name tag the header and the Standings use — so
//! closing reads as movement without reading a number, and a train of
//! traffic reads as a train.
//!
//! Colour is urgency on the radar's scale — amber for a car that is a factor,
//! red for one that is on you — carried by a stripe down the card's left
//! edge, which is the part meant to be caught without looking. At the alert
//! level the red flashes, between full and dimmed rather than on and off, so
//! no frame is ever a blank card. There is no green: when the car has gone by
//! the card goes, and that is the all-clear.
//!
//! The thresholds are applied here, not in the telemetry thread, so the two
//! sliders on the settings page move the card as they are dragged. See
//! `plans/faster-class.md`.

use egui::{Color32, Rect, RichText, Ui};

use super::theme;
use super::{
    BLOCK_ROUNDING, Metrics, PANEL_BG, RADAR_CLOSE, card_frame, card_rounding, class_color, margin, paint_text,
    readout, text_primary, text_secondary, text_tertiary, text_width, tint,
};
use crate::config::FasterClassConfig;
use crate::telemetry::faster_class::{self, Alarm, Level, Subject, Thresholds};
use crate::telemetry::snapshot::{Approaching, TelemetrySnapshot};

/// Card geometry, at design size.
const CARD_WIDTH: f32 = 320.0;
const CARD_PAD: (f32, f32) = (16.0, 12.0);
const CONTENT_HEIGHT: f32 = 104.0;
/// The stripe down the card's inner left edge, and the gap to the content.
const STRIPE_WIDTH: f32 = 4.0;
const STRIPE_GAP: f32 = 12.0;

/// The header row: class tag, car number, driver, and the "+N" chip.
const HEADER_HEIGHT: f32 = 22.0;
const TAG_WIDTH: f32 = 46.0;
const TAG_GAP: f32 = 8.0;
const TAG_SIZE: f32 = 13.0;
const NAME_SIZE: f32 = 14.0;
const NAME_GAP: f32 = 6.0;
const OTHERS_WIDTH: f32 = 34.0;

/// The readout row: the gap, its unit, and the state column beside it.
const READOUT_TOP: f32 = 30.0;
const READOUT_HEIGHT: f32 = 48.0;
const READOUT_SIZE: f32 = 44.0;
const UNIT_SIZE: f32 = 14.0;
const UNIT_GAP: f32 = 6.0;
/// Where the state column starts, from the content's left edge — past the
/// widest gap the readout prints (`15.0`) with room to spare.
const STATE_LEFT: f32 = 118.0;
const EYEBROW_SIZE: f32 = 11.0;
const STATE_SIZE: f32 = 16.0;
const ARRIVAL_SIZE: f32 = 12.0;

/// The approach bar: the track, the marker that is you, and the car's block.
const BAR_TOP: f32 = 88.0;
const BAR_HEIGHT: f32 = 10.0;
const MARKER_WIDTH: f32 = 4.0;
const MARKER_OVERHANG: f32 = 3.0;
/// The class plate that rides the bar in place of the block: the same
/// straight-edged, class-coloured tag the header and the Standings use, so
/// the mark on the bar is the mark everywhere else. It stands proud of the
/// bar by the player marker's own overhang, its width follows its label
/// between these bounds, and the label is the class short name at tag scale.
const PLATE_PAD: f32 = 7.0;
const PLATE_MIN_WIDTH: f32 = 26.0;
const PLATE_MAX_WIDTH: f32 = 64.0;
const PLATE_LABEL_SIZE: f32 = 10.0;
/// The wash over the part of the track inside the alert threshold.
const ALERT_ZONE_ALPHA: u8 = 55;
const TRACK_ALPHA: u8 = 6;

/// How fast the alert level flashes, and how far the dim half drops.
///
/// Two a second is brisk enough to read as urgent without strobing; the dim
/// half keeps almost half the colour so the card never reads as empty.
const FLASH_HZ: f64 = 2.0;
const FLASH_DIM: f32 = 0.45;

/// The overflow mark a driver's name is cut to when it will not fit.
const ELLIPSIS: &str = "\u{2026}";

/// Draws the card if a quicker class is close enough to warn about.
///
/// `alarm` is this widget's level state, held across frames by the app.
/// `preview` is layout mode: with a car in the list but none inside the
/// thresholds, the nearest is drawn at the warning level anyway, so the panel
/// can be positioned whatever the sliders say.
pub fn draw(
    ui: &mut Ui,
    snapshot: Option<&TelemetrySnapshot>,
    config: &FasterClassConfig,
    alarm: &mut Alarm,
    preview: bool,
) {
    let thresholds = Thresholds::new(config.warn_secs, config.alert_secs);
    let approaching: &[Approaching] = match snapshot {
        Some(snapshot) => &snapshot.faster_class.approaching,
        None => &[],
    };
    let cars: Vec<(i32, f32)> = approaching.iter().map(|car| (car.car_idx, car.behind_secs)).collect();
    let subject = alarm
        .update(&cars, thresholds)
        .or_else(|| (preview && !cars.is_empty()).then_some(Subject { index: 0, level: Level::Warn }));
    let Some(Subject { index, level }) = subject else {
        return; // nothing coming: draw nothing at all, not even an empty card
    };
    let Some(car) = approaching.get(index) else {
        return;
    };
    let others = faster_class::others_within(&cars, index, thresholds.warn_secs);

    let metrics = Metrics::new(config.scale);
    let now = ui.input(|input| input.time);
    let colour = level_colour(level, config.flash && !flash_on(now));
    let surface = if theme::is_instrument() { PANEL_BG } else { Color32::from_rgba_premultiplied(17, 19, 23, 218) };
    card_frame(metrics, surface, margin(metrics, CARD_PAD.0, CARD_PAD.1), card_rounding(metrics)).show(ui, |ui| {
        let (rect, _response) =
            ui.allocate_exact_size(metrics.vec2(CARD_WIDTH - CARD_PAD.0 * 2.0, CONTENT_HEIGHT), egui::Sense::hover());
        stripe(ui, metrics, rect, colour);
        let content =
            Rect::from_min_max(egui::pos2(rect.left() + metrics.px(STRIPE_WIDTH + STRIPE_GAP), rect.top()), rect.max);
        header(ui, metrics, content, car, others, colour);
        gap_readout(ui, metrics, content, car, level, colour);
        approach_bar(ui, metrics, content, approaching, index, thresholds, preview);
    });
}

/// The block down the left edge, in the level's colour: the one mark meant to
/// be read from the corner of the eye.
fn stripe(ui: &Ui, metrics: Metrics, rect: Rect, colour: Color32) {
    let stripe = Rect::from_min_max(rect.min, egui::pos2(rect.left() + metrics.px(STRIPE_WIDTH), rect.bottom()));
    ui.painter().rect_filled(stripe, metrics.px(STRIPE_WIDTH / 2.0), colour);
}

/// Class tag, car number, driver, and how many more are behind this one.
fn header(ui: &Ui, metrics: Metrics, content: Rect, car: &Approaching, others: usize, colour: Color32) {
    let row = Rect::from_min_size(content.min, egui::vec2(content.width(), metrics.px(HEADER_HEIGHT)));
    let rounding = metrics.px(BLOCK_ROUNDING);

    // The class, on a solid block of its own colour — the same tag the
    // Standings puts on a class header, so it reads as the same thing.
    let tag = Rect::from_min_size(row.min, egui::vec2(metrics.px(TAG_WIDTH), row.height()));
    let name = if car.car_class_short_name.is_empty() { "?" } else { &*car.car_class_short_name };
    let class = class_color(&car.car_class_color);
    let (tag_fill, tag_ink) =
        if theme::is_instrument() { (class, Color32::from_black_alpha(230)) } else { (tint(class, 24), class) };
    ui.painter().rect_filled(tag, rounding, tag_fill);
    paint_text(
        ui,
        tag.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new(name).size(metrics.px(TAG_SIZE)).strong().color(tag_ink),
    );

    // The chip is laid out first so the driver's name knows where to stop.
    let mut right = row.right();
    if others > 0 {
        let chip = Rect::from_min_max(egui::pos2(row.right() - metrics.px(OTHERS_WIDTH), row.top()), row.max);
        ui.painter().rect_filled(chip, rounding, tint(colour, if theme::is_instrument() { 60 } else { 16 }));
        paint_text(
            ui,
            chip.center(),
            egui::Align2::CENTER_CENTER,
            RichText::new(format!("+{others}")).size(metrics.px(TAG_SIZE)).strong().color(colour),
        );
        right = chip.left() - metrics.px(TAG_GAP);
    }

    let mut x = tag.right() + metrics.px(TAG_GAP);
    let middle = row.center().y;
    if !car.car_number.is_empty() {
        let number = RichText::new(format!("#{}", car.car_number)).size(metrics.px(NAME_SIZE)).color(text_secondary());
        paint_text(ui, egui::pos2(x, middle), egui::Align2::LEFT_CENTER, number.clone());
        x += text_width(ui, number) + metrics.px(NAME_GAP);
    }
    let driver = |text: &str| RichText::new(text).size(metrics.px(NAME_SIZE)).color(text_primary());
    let fitted = fit(&car.driver_name, right - x, |text| text_width(ui, driver(text)));
    paint_text(ui, egui::pos2(x, middle), egui::Align2::LEFT_CENTER, driver(&fitted));
}

/// The gap in the readout face, its unit, and the state column beside it.
fn gap_readout(ui: &Ui, metrics: Metrics, content: Rect, car: &Approaching, level: Level, colour: Color32) {
    let row = Rect::from_min_size(
        egui::pos2(content.left(), content.top() + metrics.px(READOUT_TOP)),
        egui::vec2(content.width(), metrics.px(READOUT_HEIGHT)),
    );
    let middle = row.center().y;

    // A car alongside or just past has no gap worth a decimal; zero says so.
    let ink = if level == Level::Warn && !theme::is_instrument() { text_primary() } else { colour };
    let figure = readout(format!("{:.1}", car.behind_secs.max(0.0)), metrics.px(READOUT_SIZE)).color(ink);
    let figure_width = text_width(ui, figure.clone());
    paint_text(ui, egui::pos2(row.left(), middle), egui::Align2::LEFT_CENTER, figure);
    paint_text(
        ui,
        egui::pos2(row.left() + figure_width + metrics.px(UNIT_GAP), middle + metrics.px(8.0)),
        egui::Align2::LEFT_CENTER,
        RichText::new("S").size(metrics.px(UNIT_SIZE)).strong().color(text_tertiary()),
    );

    let x = row.left() + metrics.px(STATE_LEFT);
    paint_text(
        ui,
        egui::pos2(x, middle - metrics.px(16.0)),
        egui::Align2::LEFT_CENTER,
        RichText::new("FASTER CLASS").size(metrics.px(EYEBROW_SIZE)).strong().color(text_tertiary()),
    );
    paint_text(
        ui,
        egui::pos2(x, middle + metrics.px(1.0)),
        egui::Align2::LEFT_CENTER,
        RichText::new(state_word(level)).size(metrics.px(STATE_SIZE)).strong().color(colour),
    );
    if let Some(arrival) = faster_class::arrival_secs(car.behind_secs, car.closing_rate) {
        paint_text(
            ui,
            egui::pos2(x, middle + metrics.px(17.0)),
            egui::Align2::LEFT_CENTER,
            RichText::new(arrival_text(arrival)).size(metrics.px(ARRIVAL_SIZE)).color(text_secondary()),
        );
    }
}

/// The bar: you at the left end, and every quicker car inside the warning
/// span sliding up it — each a class plate at its own true position, so an
/// LMP2 and two GTPs arriving together read as three marks in order rather
/// than one block and a chip.
///
/// Each plate wears its class colour and name, the same tag the header and
/// the Standings use, so the car in the mirror can be matched by colour
/// alone. Urgency stays with the card — the stripe, the readout, the
/// alert-zone wash the plates slide into — rather than being repeated on
/// every mark. Farthest is drawn first, so of two cars nose to tail the
/// nearer one is the one left whole.
fn approach_bar(
    ui: &Ui,
    metrics: Metrics,
    content: Rect,
    approaching: &[Approaching],
    subject: usize,
    thresholds: Thresholds,
    preview: bool,
) {
    let track = Rect::from_min_size(
        egui::pos2(content.left(), content.top() + metrics.px(BAR_TOP)),
        egui::vec2(content.width(), metrics.px(BAR_HEIGHT)),
    );
    let rounding = metrics.px(BLOCK_ROUNDING);
    ui.painter().rect_filled(track, rounding, Color32::from_white_alpha(TRACK_ALPHA));

    // Your rear bumper is the marker's trailing edge; the warning threshold
    // is where a narrowest plate sits fully inside the far end of the track.
    // Wider plates are nudged left of the very end instead of stretching the
    // axis, so one long class name doesn't move every other car's position.
    let near = track.left() + metrics.px(MARKER_WIDTH);
    let far = track.right() - metrics.px(PLATE_MIN_WIDTH);
    let alert_fraction = axis_fraction(thresholds.alert_secs, thresholds.warn_secs);
    if alert_fraction > 0.0 {
        let zone = Rect::from_min_max(track.min, egui::pos2(near + (far - near) * alert_fraction, track.bottom()));
        ui.painter().rect_filled(zone, rounding, tint(theme::alert(), ALERT_ZONE_ALPHA));
    }

    // Every car inside the span, plus the subject wherever it is. In
    // preview, every car: layout mode exists to show the whole scene, so a
    // tight warning slider must not empty the bar it is being judged
    // against — anything beyond the span pins to the far end instead.
    let mut order: Vec<usize> = (0..approaching.len())
        .filter(|&index| preview || index == subject || approaching[index].behind_secs <= thresholds.warn_secs)
        .collect();
    order.sort_by(|&a, &b| approaching[b].behind_secs.total_cmp(&approaching[a].behind_secs));
    for index in order {
        draw_plate(ui, metrics, track, (near, far), &approaching[index], thresholds);
    }

    // Drawn last: a car overlapping the marker must not bury the mark that
    // says where you are.
    let marker = Rect::from_min_max(
        egui::pos2(track.left(), track.top() - metrics.px(MARKER_OVERHANG)),
        egui::pos2(near, track.bottom() + metrics.px(MARKER_OVERHANG)),
    );
    ui.painter().rect_filled(marker, metrics.px(MARKER_WIDTH / 2.0), text_primary());
}

/// One car on the bar: its class plate, standing proud of the track by the
/// player marker's own overhang so the two read as marks of the same rank.
///
/// The plate's leading edge is the car's nose, and it stops on your bumper
/// rather than sliding past the marker as the block used to: a car with a
/// negative gap is the PASSING state's job to announce, and a mark drifting
/// over the stripe would be clutter where the eye reads urgency.
fn draw_plate(
    ui: &Ui,
    metrics: Metrics,
    track: Rect,
    (near, far): (f32, f32),
    car: &Approaching,
    thresholds: Thresholds,
) {
    let label = |text: &str| {
        RichText::new(text).size(metrics.px(PLATE_LABEL_SIZE)).strong().color(Color32::from_black_alpha(230))
    };
    let fitted = fit(&car.car_class_short_name, metrics.px(PLATE_MAX_WIDTH - PLATE_PAD * 2.0), |text| {
        text_width(ui, label(text))
    });
    let width = (text_width(ui, label(&fitted)) + metrics.px(PLATE_PAD * 2.0)).max(metrics.px(PLATE_MIN_WIDTH));

    let front = near + (far - near) * axis_fraction(car.behind_secs, thresholds.warn_secs).max(0.0);
    let front = front.min(track.right() - width);
    let plate = Rect::from_min_max(
        egui::pos2(front, track.top() - metrics.px(MARKER_OVERHANG)),
        egui::pos2(front + width, track.bottom() + metrics.px(MARKER_OVERHANG)),
    );
    ui.painter().rect_filled(plate, metrics.px(BLOCK_ROUNDING), class_color(&car.car_class_color));
    paint_text(ui, plate.center(), egui::Align2::CENTER_CENTER, label(&fitted));
}

/// Where a gap sits along the bar: `0.0` at the marker, `1.0` at the warning
/// threshold. A car just gone by sits a little left of the marker; further
/// than that is clipped by the track.
fn axis_fraction(behind_secs: f32, warn_secs: f32) -> f32 {
    if !behind_secs.is_finite() || !warn_secs.is_finite() || warn_secs <= 0.0 {
        return 0.0;
    }
    (behind_secs / warn_secs).clamp(-0.25, 1.0)
}

/// The level's colour, dimmed for the off half of a flash.
fn level_colour(level: Level, dimmed: bool) -> Color32 {
    match level {
        Level::Warn => RADAR_CLOSE,
        Level::Alert if dimmed => theme::alert().gamma_multiply(FLASH_DIM),
        Level::Alert | Level::Passing => theme::alert(),
    }
}

/// Whether the flash is in its bright half at `time_secs`.
fn flash_on(time_secs: f64) -> bool {
    (time_secs * FLASH_HZ).fract() < 0.5
}

fn state_word(level: Level) -> &'static str {
    match level {
        Level::Warn => "BEHIND",
        Level::Alert => "CLOSE",
        Level::Passing => "PASSING",
    }
}

/// `on you in ~9 s`: whole seconds, with the tilde, because the figure is a
/// projection off a gap that breathes through every corner.
fn arrival_text(arrival_secs: f32) -> String {
    format!("on you in ~{} s", arrival_secs.round().max(1.0))
}

/// Cuts `text` to fit `max_width`, measured by `width`, ending in an ellipsis
/// when anything had to go.
fn fit(text: &str, max_width: f32, width: impl Fn(&str) -> f32) -> String {
    if width(text) <= max_width {
        return text.to_owned();
    }
    let mut kept = String::with_capacity(text.len());
    for (end, _) in text.char_indices().skip(1) {
        let candidate = format!("{}{ELLIPSIS}", &text[..end]);
        if width(&candidate) > max_width {
            break;
        }
        kept = candidate;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_runs_from_the_marker_to_the_warning_threshold() {
        assert!(axis_fraction(0.0, 4.0).abs() < f32::EPSILON);
        assert!((axis_fraction(4.0, 4.0) - 1.0).abs() < f32::EPSILON);
        assert!((axis_fraction(2.0, 4.0) - 0.5).abs() < f32::EPSILON);
        assert!((axis_fraction(6.0, 4.0) - 1.0).abs() < f32::EPSILON, "beyond the threshold pins to the end");
        assert!(axis_fraction(-0.3, 4.0) < 0.0, "a car just gone by sits left of the marker");
        assert!((axis_fraction(-9.0, 4.0) + 0.25).abs() < f32::EPSILON, "but only a little");
    }

    /// A hand-edited config is the only way to get here, and a zero span
    /// would otherwise divide every gap into infinity.
    #[test]
    fn a_nonsense_axis_puts_the_car_at_the_marker() {
        assert!(axis_fraction(2.0, 0.0).abs() < f32::EPSILON);
        assert!(axis_fraction(f32::NAN, 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_flash_is_on_for_half_of_each_cycle() {
        assert!(flash_on(0.0));
        assert!(flash_on(0.2));
        assert!(!flash_on(0.3));
        assert!(flash_on(0.5));
    }

    #[test]
    fn arrival_reads_in_whole_seconds_with_a_tilde() {
        assert_eq!(arrival_text(8.6), "on you in ~9 s");
        assert_eq!(arrival_text(0.2), "on you in ~1 s", "never a zero: it is still coming");
    }

    #[test]
    #[expect(clippy::cast_precision_loss, reason = "a name is a handful of characters, far inside f32")]
    fn a_name_that_fits_is_left_alone_and_one_that_does_not_is_cut() {
        // Each character is one unit wide.
        let width = |text: &str| text.chars().count() as f32;
        assert_eq!(fit("Driver 2", 10.0, width), "Driver 2");
        assert_eq!(fit("Istvan Fodor", 7.0, width), "Istvan\u{2026}");
        assert_eq!(fit("Istvan Fodor", 1.0, width), "", "no room for even one letter and the mark");
    }

    #[test]
    fn every_level_has_a_word_and_a_colour() {
        for level in [Level::Warn, Level::Alert, Level::Passing] {
            assert!(!state_word(level).is_empty());
            assert_ne!(level_colour(level, false), Color32::TRANSPARENT);
        }
        assert_ne!(level_colour(Level::Alert, true), level_colour(Level::Alert, false), "the flash dims");
        assert_eq!(level_colour(Level::Warn, true), level_colour(Level::Warn, false), "only the alert flashes");
    }
}
