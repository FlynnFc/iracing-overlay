//! One pit instruction, shared by the driver's and crew's Fuel pages.

use super::{ARMED_SURFACE, CONTROL_SURFACE, PAGE_SURFACE, hit, paint_armed_edge, paint_cursor_ring, paint_hover};
use super::{Action, Click, Metrics, Rect, RichText, RowKind, Stroke, Ui};
use super::{paint_text, text_primary, text_secondary, text_tertiary, theme};

pub(super) const HEIGHT: f32 = 82.0;
pub(super) const GAP: f32 = 12.0;

/// Independent targets within the same control: arming never also steps fuel.
fn regions(rect: Rect, metrics: Metrics) -> [Rect; 4] {
    let amount_width = metrics.px(204.0).min(rect.width() * 0.5);
    let amount = Rect::from_min_max(egui::pos2(rect.right() - amount_width, rect.top()), rect.max);
    let edge = metrics.px(36.0).min(amount.width() / 3.0);
    [
        Rect::from_min_max(rect.min, egui::pos2(amount.left() - metrics.px(12.0), rect.bottom())),
        Rect::from_min_max(amount.min, egui::pos2(amount.left() + edge, amount.bottom())),
        amount.shrink2(egui::vec2(edge, 0.0)),
        Rect::from_min_max(egui::pos2(amount.right() - edge, amount.top()), amount.max),
    ]
}

#[expect(clippy::too_many_arguments, reason = "a shared control retains its row index and provenance for both seats")]
pub(super) fn draw(
    ui: &Ui,
    metrics: Metrics,
    rect: Rect,
    kind: &RowKind,
    index: usize,
    selected: bool,
    driver: Option<&str>,
    clicks: &mut Vec<Click>,
) {
    let RowKind::Fuel { litres, armed, automatic } = *kind else {
        return;
    };
    let rounding = metrics.px(8.0);
    ui.painter().rect_filled(rect, rounding, if armed { ARMED_SURFACE } else { CONTROL_SURFACE });
    paint_armed_edge(ui, metrics, rect, rounding, armed);
    if selected {
        paint_cursor_ring(ui, metrics, rect, rounding);
    }
    let inner = rect.shrink2(metrics.vec2(16.0, 0.0));
    paint_text(
        ui,
        egui::pos2(inner.left(), rect.top() + metrics.px(17.0)),
        egui::Align2::LEFT_CENTER,
        RichText::new("NEXT STOP").size(metrics.px(11.0)).strong().color(text_secondary()),
    );
    let provenance = driver.map(|name| format!("Driver: {name}"));
    let note = provenance.as_deref().unwrap_or(if automatic { "AUTO FUEL" } else { "" });
    let note = crate::ui::elide_to_width(ui, note, inner.width() - metrics.px(120.0), |text| {
        RichText::new(text).size(metrics.px(11.0)).color(text_tertiary())
    });
    paint_text(ui, egui::pos2(inner.right(), rect.top() + metrics.px(17.0)), egui::Align2::RIGHT_CENTER, note);
    let body = Rect::from_min_max(
        egui::pos2(inner.left(), rect.top() + metrics.px(32.0)),
        egui::pos2(inner.right(), rect.bottom() - metrics.px(8.0)),
    );
    let [toggle, minus, amount, plus] = regions(body, metrics);
    if hit(ui, toggle, ("refuel-toggle", index), clicks, Click::Control { index, action: Action::Toggle }) {
        paint_hover(ui, toggle, metrics.px(4.0));
    }
    let check = Rect::from_center_size(
        egui::pos2(toggle.left() + metrics.px(10.0), toggle.center().y),
        metrics.vec2(20.0, 20.0),
    );
    ui.painter().rect_filled(check, metrics.px(4.0), if armed { theme::caution() } else { PAGE_SURFACE });
    if armed {
        let stroke = Stroke::new(metrics.px(2.0), egui::Color32::from_rgb(30, 24, 16));
        let points = [
            egui::pos2(check.left() + metrics.px(4.0), check.center().y),
            egui::pos2(check.center().x - metrics.px(1.0), check.bottom() - metrics.px(5.0)),
            egui::pos2(check.right() - metrics.px(4.0), check.top() + metrics.px(5.0)),
        ];
        ui.painter().line_segment([points[0], points[1]], stroke);
        ui.painter().line_segment([points[1], points[2]], stroke);
    } else {
        ui.painter().rect_stroke(check, metrics.px(4.0), Stroke::new(metrics.px(1.0), text_tertiary()));
    }
    let label = if armed {
        "Refuel"
    } else if automatic {
        "Not armed"
    } else {
        "Skip fuel"
    };
    paint_text(
        ui,
        egui::pos2(check.right() + metrics.px(11.0), toggle.center().y),
        egui::Align2::LEFT_CENTER,
        RichText::new(label).size(metrics.px(16.0)).strong().color(if armed {
            theme::caution()
        } else {
            text_secondary()
        }),
    );
    let value = crate::ui::elide_to_width(ui, &format!("{litres} L"), amount.width() - metrics.px(6.0), |text| {
        crate::ui::readout(text, metrics.px(34.0)).color(if armed || automatic {
            text_primary()
        } else {
            text_secondary()
        })
    });
    paint_text(ui, amount.center(), egui::Align2::CENTER_CENTER, value);
    if !automatic {
        for (target, glyph, action) in [(minus, "\u{2212}", Action::Decrement), (plus, "+", Action::Increment)] {
            if hit(ui, target, ("refuel-step", index, glyph), clicks, Click::Control { index, action }) {
                paint_hover(ui, target, metrics.px(4.0));
            }
            paint_text(
                ui,
                target.center(),
                egui::Align2::CENTER_CENTER,
                RichText::new(glyph).size(metrics.px(24.0)).color(text_secondary()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_targets_emit_one_action_and_auto_removes_only_amount_actions() {
        for automatic in [false, true] {
            let ctx = egui::Context::default();
            crate::app::install_fonts(&ctx);
            let metrics = Metrics::new(1.0);
            let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), metrics.vec2(440.0, HEIGHT));
            let kind = RowKind::Fuel { litres: 64, armed: true, automatic };
            let body = Rect::from_min_max(egui::pos2(36.0, 52.0), egui::pos2(444.0, 94.0));
            let [toggle, minus, amount, plus] = regions(body, metrics);
            let frame = |events| {
                let mut clicks = Vec::new();
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    events,
                    ..Default::default()
                };
                drop(ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw(ui, metrics, rect, &kind, 2, true, Some("Teammate"), &mut clicks);
                    });
                }));
                clicks
            };
            frame(vec![]);
            for (target, action) in [
                (toggle, Some(Action::Toggle)),
                (minus, (!automatic).then_some(Action::Decrement)),
                (amount, None),
                (plus, (!automatic).then_some(Action::Increment)),
            ] {
                let pos = target.center();
                frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                let got = frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                let expected: Vec<_> = action.map(|action| Click::Control { index: 2, action }).into_iter().collect();
                assert_eq!(got, expected);
            }
        }
    }

    #[test]
    fn arming_and_amount_targets_stay_separate_at_supported_widths_and_scales() {
        for scale in [0.65, 1.0, 1.5, 2.0] {
            let metrics = Metrics::new(scale);
            for width in [440.0, 650.0, 850.0] {
                let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), metrics.vec2(width, 42.0));
                let [toggle, minus, amount, plus] = regions(rect, metrics);
                for target in [toggle, minus, amount, plus] {
                    assert!(rect.contains_rect(target));
                }
                assert!(toggle.right() < minus.left());
                assert!((minus.right() - amount.left()).abs() < 0.001);
                assert!((amount.right() - plus.left()).abs() < 0.001);
                assert!(toggle.width() + 0.001 >= metrics.px(180.0));
                assert!(amount.width() + 0.001 >= metrics.px(132.0));
            }
        }
    }
}
