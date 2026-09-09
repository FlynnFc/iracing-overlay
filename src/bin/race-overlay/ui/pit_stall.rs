// Rust guideline compliant 2026-02-16

//! Renders the Pit Stall widget: how far the car is from its marks.
//!
//! A vertical bar that fills as the car closes on its box. Full is the box:
//! there is no midpoint to read against and no marker to find, just a level
//! rising to the top, which is a shape a driver can take in without looking
//! away from the crew.
//!
//! **The top is square and the foot is rounded.** The top is where the box
//! is, and a hard edge is what a limit looks like; it also means the rising
//! level can reach the top without anything having to cap it. A block used
//! to sit up there marking the box's own tolerance, and it did cap the fill
//! — but every approach then began with the bar apparently part full, which
//! is the one thing this widget must never say. Outlined instead of filled
//! it stopped lying and started looking like a stray box. The honest answer
//! was that the mark was never worth its ink: the bar goes green on the
//! marks and the metres are printed beside it.
//!
//! **The fill is linear in distance**, so it rises at a constant rate for a
//! constant road speed and its motion is a direct read of how fast the car is
//! closing. An earlier version magnified the last few metres, which made the
//! level appear to accelerate exactly where a driver is trying to judge a
//! stop — the opposite of useful.
//!
//! The fill is paper, not a colour ramp: solid for the road covered, hatch
//! for the road still to come — the same solid-versus-planned language as
//! the black box's fuel tank, stood on end. Being forty metres from the box
//! is the widget's normal state, not an alarm, so colour only arrives at the
//! end: the whole capsule goes green on the marks, and red through them.
//!
//! Overshooting fills the capsule completely rather than draining it back
//! down, so a full bar is always "you are at the box or through it" and a
//! partial one is always "still coming" — a level alone cannot tell those two
//! apart, and a bar that emptied as the car drove past its marks would read as
//! a bar that was still waiting for it. Which way you are out is said in
//! words beside the bar for the same reason.
//!
//! The distance is the hero: metres to go, in the readout face, beside the
//! bar rather than in a caption under it, because in the last metres the
//! number is what a driver wants and the eye is at the bar, not below it.
//!
//! Like Radar Bars, there is no card and no header: this is an ambient
//! indicator that exists for a few seconds a stop, not a panel.

use egui::{Color32, Rect, RichText, Ui};

use super::theme;
use super::{CARD_ROUNDING, Metrics, PANEL_BG, paint_hatch, paint_text, readout, text_tertiary};
use crate::config::PitStallConfig;
use crate::telemetry::snapshot::{PitStallSnapshot, TelemetrySnapshot};

/// Capsule geometry. Half the width it was: the fill is read as a level, and
/// a level needs no width. Short enough to sit in the corner of the eye
/// rather than run down the screen.
const BAR_WIDTH: f32 = 64.0;
const BAR_HEIGHT: f32 = 280.0;
/// The readout beside the capsule: its lane, its figure, and the label
/// under the figure.
const READOUT_GAP: f32 = 18.0;
const READOUT_WIDTH: f32 = 130.0;
const READOUT_SIZE: f32 = 44.0;
const LABEL_SIZE: f32 = 13.0;
const LABEL_GAP: f32 = 6.0;
/// How far up from the capsule's foot the readout sits.
const READOUT_LIFT: f32 = 60.0;

/// The hatch's ink: paper, washed back so the solid fill under it is
/// unmistakably the live part.
const HATCH_ALPHA: u8 = 115;

pub fn draw(ui: &mut Ui, snapshot: Option<&TelemetrySnapshot>, config: &PitStallConfig) {
    let Some(stall) = snapshot.map(|s| s.pit_stall) else {
        return;
    };
    if !stall.visible {
        return; // not in the lane, or already served: draw nothing at all
    }

    let metrics = Metrics::new(config.scale);
    let range_m = axis_range(config);
    let (rect, _response) =
        ui.allocate_exact_size(metrics.vec2(BAR_WIDTH + READOUT_GAP + READOUT_WIDTH, BAR_HEIGHT), egui::Sense::hover());
    let capsule = Rect::from_min_size(rect.min, metrics.vec2(BAR_WIDTH, BAR_HEIGHT));
    // The same corner radius the cards use, not a half-width stadium: a
    // rounded end curves the fill's own edge, which is the line a driver is
    // reading the level off. The top is left square for that reason and one
    // more — see the module docs.
    let radius = metrics.px(CARD_ROUNDING);
    let shape = egui::Rounding { sw: radius, se: radius, ..egui::Rounding::ZERO };

    let state = State::of(stall);
    match state {
        // Colour only at the end, and then the whole capsule: green is the
        // state the widget exists to confirm, red the one it exists to
        // prevent.
        State::InBox => {
            ui.painter().rect_filled(capsule, shape, theme::signal());
        }
        State::Overshot => {
            ui.painter().rect_filled(capsule, shape, theme::alert());
        }
        State::Approaching => {
            ui.painter().rect_filled(capsule, shape, PANEL_BG);
            // The road still to cover, hatched, from the top down; then the
            // road covered, solid, rising through it. Nothing sits above
            // either: the square top is the box, and the level reaches it.
            let fill = fill_level(stall, range_m);
            let to_come = Rect::from_min_max(capsule.min, egui::pos2(capsule.right(), band(capsule, 0.0, fill).top()));
            if to_come.height() > 0.0 {
                paint_hatch(ui, metrics, to_come, Color32::from_white_alpha(HATCH_ALPHA));
            }
            if fill > 0.0 {
                ui.painter().rect_filled(band(capsule, 0.0, fill), shape, super::text_primary());
            }
        }
    }

    let Readout { label, distance } = readout_for(stall, state);
    let colour = match state {
        State::InBox => theme::signal(),
        State::Overshot => theme::alert(),
        State::Approaching => super::text_primary(),
    };
    let left = capsule.right() + metrics.px(READOUT_GAP);
    let anchor = capsule.bottom() - metrics.px(READOUT_LIFT);
    match distance {
        Some(distance) => {
            let figure = readout(distance, metrics.px(READOUT_SIZE)).color(colour);
            paint_text(
                ui,
                egui::pos2(left, anchor - metrics.px(READOUT_SIZE / 2.0)),
                egui::Align2::LEFT_CENTER,
                figure,
            );
            paint_text(
                ui,
                egui::pos2(left, anchor + metrics.px(LABEL_GAP + LABEL_SIZE / 2.0)),
                egui::Align2::LEFT_CENTER,
                RichText::new(label).size(metrics.px(LABEL_SIZE)).strong().color(if state == State::Approaching {
                    text_tertiary()
                } else {
                    colour
                }),
            );
        }
        // No figure: the label alone, at the figure's height. On the marks
        // the green is the whole answer, and a bar with no measured scale is
        // honest about not having a number.
        None => paint_text(
            ui,
            egui::pos2(left, anchor),
            egui::Align2::LEFT_CENTER,
            RichText::new(label).size(metrics.px(LABEL_SIZE)).strong().color(colour),
        ),
    }
}

/// What a full capsule means, in metres from the perfect stopping point.
///
/// A hand-edited config could name zero or a negative span, which would put
/// every reading at one end of the bar; the default stands in for anything
/// that isn't a positive number.
fn axis_range(config: &PitStallConfig) -> f32 {
    if config.range_m.is_finite() && config.range_m > 0.0 { config.range_m } else { crate::config::DEFAULT_RANGE_M }
}

/// Which of the three things the bar can be saying.
///
/// Split out because the fill level alone is ambiguous — a capsule a third
/// full is a car still coming, and a full one is either a car on its marks or
/// a car that has driven through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// On the marks: the crew can work.
    InBox,
    /// Driven past the box.
    Overshot,
    /// Still coming.
    Approaching,
}

impl State {
    fn of(stall: PitStallSnapshot) -> Self {
        // The sim's own word that the crew can work outranks the arithmetic: a
        // car being serviced must never be looking at a bar that says
        // otherwise.
        if stall.in_box || stall.error_m.abs() <= stall.green_half_width_m {
            Self::InBox
        } else if stall.error_m > 0.0 {
            Self::Overshot
        } else {
            Self::Approaching
        }
    }
}

/// How full the capsule is, `0.0` empty to `1.0` full.
///
/// Full means the box, from either direction: on the marks it is full and
/// green, and past them it is full and red. A car still approaching fills
/// linearly with the distance it has left, so the level rises at the rate the
/// car is actually closing.
fn fill_level(stall: PitStallSnapshot, range_m: f32) -> f32 {
    match State::of(stall) {
        State::InBox | State::Overshot => 1.0,
        State::Approaching => level(stall.error_m.abs(), range_m),
    }
}

/// The fill level an error of `distance_m` corresponds to.
fn level(distance_m: f32, range_m: f32) -> f32 {
    if !distance_m.is_finite() {
        return 0.0;
    }
    (1.0 - distance_m / range_m).clamp(0.0, 1.0)
}

/// What is written beside the bar: a word, and the metres where there is an
/// honest figure to print — see [`PitStallSnapshot::readout_m`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Readout {
    label: &'static str,
    distance: Option<String>,
}

fn readout_for(stall: PitStallSnapshot, state: State) -> Readout {
    // One decimal place, which is about the honest resolution of a figure
    // derived from a track percentage; a second would be invented precision.
    let distance = stall.readout_m.map(|m| format!("{:.1}", m.abs()));
    match state {
        State::InBox => Readout { label: "ON THE MARKS", distance: None },
        State::Overshot => Readout { label: "M LONG", distance },
        State::Approaching => Readout { label: "M TO GO", distance },
    }
}

/// The sub-rectangle of `capsule` between two fill levels, measured from its
/// bottom.
fn band(capsule: Rect, from: f32, to: f32) -> Rect {
    let y = |level: f32| capsule.bottom() - level.clamp(0.0, 1.0) * capsule.height();
    Rect::from_min_max(egui::pos2(capsule.left(), y(to)), egui::pos2(capsule.right(), y(from)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default full-scale distance, which every test below reads against.
    const RANGE: f32 = crate::config::DEFAULT_RANGE_M;

    fn reading(error_m: f32) -> PitStallSnapshot {
        PitStallSnapshot { visible: true, error_m, readout_m: Some(error_m), in_box: false, green_half_width_m: 1.0 }
    }

    #[test]
    fn the_box_is_a_full_bar() {
        assert!((fill_level(reading(0.0), RANGE) - 1.0).abs() < f32::EPSILON);
        assert!((fill_level(reading(-1.0), RANGE) - 1.0).abs() < f32::EPSILON, "the near edge of the box is full");
        assert_eq!(State::of(reading(0.0)), State::InBox);
    }

    #[test]
    fn the_bar_is_empty_at_the_far_end_of_its_range() {
        assert!(fill_level(reading(-RANGE), RANGE).abs() < f32::EPSILON);
        assert!(fill_level(reading(-500.0), RANGE).abs() < f32::EPSILON, "further out still reads empty, not negative");
    }

    #[test]
    fn the_bar_fills_as_the_car_closes() {
        let near = fill_level(reading(-2.0), RANGE);
        let far = fill_level(reading(-8.0), RANGE);
        assert!(near > far);
        assert!(near < 1.0);
    }

    /// The point of the change from a magnified axis: equal distances are
    /// equal amounts of fill wherever they fall, so the level never appears to
    /// accelerate as the car closes on its marks.
    #[test]
    fn a_metre_is_the_same_amount_of_fill_everywhere() {
        let near = fill_level(reading(-2.0), RANGE) - fill_level(reading(-3.0), RANGE);
        let far = fill_level(reading(-7.0), RANGE) - fill_level(reading(-8.0), RANGE);
        assert!((near - far).abs() < 1e-6, "near {near} and far {far} must be equal");
    }

    /// A fill level alone cannot tell a car that has gone too far from one
    /// that is still coming, so overshooting fills the whole capsule instead
    /// of draining it back down through the approaching states.
    #[test]
    fn overshooting_fills_the_bar_rather_than_draining_it() {
        let overshot = reading(4.0);
        assert_eq!(State::of(overshot), State::Overshot);
        assert!((fill_level(overshot, RANGE) - 1.0).abs() < f32::EPSILON);
        // And is not the same picture as a car still a long way off.
        assert!(fill_level(reading(-4.0), RANGE) < 1.0);
    }

    /// The sim's own flag outranks the arithmetic — see [`State::of`].
    #[test]
    fn the_in_box_flag_is_full_however_far_off_the_arithmetic_says() {
        let mut stall = reading(50.0);
        stall.in_box = true;
        assert_eq!(State::of(stall), State::InBox);
        assert!((fill_level(stall, RANGE) - 1.0).abs() < f32::EPSILON);
        assert_eq!(readout_for(stall, State::of(stall)), Readout { label: "ON THE MARKS", distance: None });
    }

    #[test]
    fn the_readout_names_which_way_the_car_is_out() {
        let short = reading(-0.8);
        // Inside the default green half-width, so this one is already home.
        assert_eq!(readout_for(short, State::of(short)).label, "ON THE MARKS");

        let short = reading(-2.5);
        assert_eq!(
            readout_for(short, State::of(short)),
            Readout { label: "M TO GO", distance: Some("2.5".to_owned()) }
        );

        let long = reading(1.4);
        assert_eq!(readout_for(long, State::of(long)), Readout { label: "M LONG", distance: Some("1.4".to_owned()) });
    }

    /// A bar with no figure beside it is honest about what it knows; a figure
    /// derived from a guessed lap length is not.
    #[test]
    fn no_measured_scale_means_no_printed_distance() {
        let mut stall = reading(-4.0);
        stall.readout_m = None;
        assert_eq!(readout_for(stall, State::of(stall)), Readout { label: "M TO GO", distance: None });
    }

    /// A hand-edited config is the only way to get here, and a zero span would
    /// otherwise divide every reading into infinity.
    #[test]
    fn an_unusable_configured_range_falls_back_to_the_default() {
        let range = |range_m| axis_range(&PitStallConfig { range_m, ..PitStallConfig::default() });
        assert!((range(0.0) - RANGE).abs() < f32::EPSILON);
        assert!((range(-5.0) - RANGE).abs() < f32::EPSILON);
        assert!((range(f32::NAN) - RANGE).abs() < f32::EPSILON);
        assert!((range(10.0) - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_target_band_sits_at_the_top_of_the_capsule() {
        let target = level(1.0, RANGE);
        assert!(target > 0.0 && target < 1.0);
        assert!(target > 0.5, "the box must be a band at the top, not most of the bar");
    }
}
