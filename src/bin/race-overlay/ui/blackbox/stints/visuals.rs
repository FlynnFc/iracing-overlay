//! Paints the stint board from structured observations, without parsing display strings.

use egui::{Align2, Color32, Rect, RichText, Stroke, Ui};

use super::{Current, Fuel, Preview, View, time};
use crate::ui::{CAUTION, Metrics, SIGNAL, icons, paint_text, readout, text_primary, text_secondary, text_tertiary};

const TILE: Color32 = Color32::from_rgb(25, 29, 35);
const TRACK: Color32 = Color32::from_rgb(48, 54, 63);
const BLUE: Color32 = Color32::from_rgb(107, 181, 255);
const VIOLET: Color32 = Color32::from_rgb(184, 163, 255);

struct Canvas<'a> {
    ui: &'a Ui,
    metrics: Metrics,
    rect: Rect,
}

impl Canvas<'_> {
    fn width(&self) -> f32 {
        self.rect.width() / self.metrics.px(1.0)
    }

    fn area(&self, [x, y, width, height]: [f32; 4]) -> Rect {
        Rect::from_min_size(self.rect.min + self.metrics.vec2(x, y), self.metrics.vec2(width.max(0.0), height.max(0.0)))
    }

    fn text(&self, area: [f32; 4], text: &str, size: f32, color: Color32) {
        self.rich(area, RichText::new(text).size(self.metrics.px(size)).color(color));
    }

    fn number(&self, area: [f32; 4], text: &str, size: f32, color: Color32) {
        self.rich(area, readout(text, self.metrics.px(size)).color(color));
    }

    fn rich(&self, area: [f32; 4], style: RichText) {
        let rect = self.area(area);
        let galley = egui::WidgetText::from(style).into_galley(
            self.ui,
            Some(egui::TextWrapMode::Truncate),
            rect.width(),
            egui::TextStyle::Body,
        );
        self.ui.painter().galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            text_primary(),
        );
    }

    fn icon(&self, area: [f32; 4], name: &str, color: Color32) {
        icons::svg(self.ui, self.area(area), name, color);
    }

    fn tile(&self) {
        self.ui.painter().rect_filled(self.rect, self.metrics.px(5.0), TILE);
    }

    fn pill(&self, area: [f32; 4], text: &str, color: Color32) {
        self.ui.painter().rect_filled(self.area(area), self.metrics.px(3.0), color.gamma_multiply(0.13));
        self.text([area[0] + 7.0, area[1], area[2] - 14.0, area[3]], text, 10.0, color);
    }

    fn hover(&self, id: &'static str, text: &str) {
        self.ui.interact(self.rect, self.ui.id().with(id), egui::Sense::hover()).on_hover_text(text);
    }
}

fn accent(name: &str) -> Color32 {
    if name.bytes().fold(0_u32, |sum, byte| sum.wrapping_add(u32::from(byte))).is_multiple_of(3) {
        BLUE
    } else {
        VIOLET
    }
}

fn avatar(c: &Canvas<'_>, area: [f32; 4], name: &str) {
    let rect = c.area(area);
    let color = accent(name);
    c.ui.painter().circle_filled(rect.center(), rect.width() / 2.0, color.gamma_multiply(0.14));
    c.ui.painter().circle_stroke(rect.center(), rect.width() / 2.0, Stroke::new(c.metrics.px(1.5), color));
    let initials: String = name.split_whitespace().filter_map(|part| part.chars().next()).take(2).collect();
    paint_text(
        c.ui,
        rect.center(),
        Align2::CENTER_CENTER,
        RichText::new(initials).strong().size(rect.height() * 0.36).color(color),
    );
}

fn header(c: &Canvas<'_>, view: &View) {
    let w = c.width();
    c.icon([0.0, 9.0, 16.0, 16.0], "flag", text_secondary());
    c.text([25.0, 5.0, w - 147.0, 24.0], &view.heading, 12.0, text_secondary());
    let color = if view.warning { CAUTION } else { SIGNAL };
    c.ui.painter().circle_filled(c.area([w - 117.0, 15.0, 5.0, 5.0]).center(), c.metrics.px(2.5), color);
    c.text([w - 105.0, 5.0, 105.0, 24.0], &view.status, 11.0, color);
    c.hover("stints-status", &view.status_detail);
}

fn hero(c: &Canvas<'_>, current: &Current) {
    c.tile();
    let w = c.width();
    let color = accent(&current.name);
    c.ui.painter().rect_filled(c.area([0.0, 8.0, 3.0, 109.0]), c.metrics.px(1.0), color);
    avatar(c, [15.0, 26.0, 48.0, 48.0], &current.name);
    c.text([80.0, 10.0, w - 239.0, 18.0], current.phase, 10.0, color);
    c.text([80.0, 28.0, w - 239.0, 33.0], &current.name, 25.0, text_primary());
    let planned = format!("Plan stint {} / {}", current.number, current.total);
    c.text(
        [80.0, 63.0, w - 239.0, 18.0],
        current.driver_note.as_deref().unwrap_or(&planned),
        11.0,
        if current.driver_note.is_some() { CAUTION } else { text_secondary() },
    );
    c.icon([w - 148.0, 13.0, 13.0, 13.0], "clock", text_secondary());
    c.text(
        [w - 128.0, 10.0, 114.0, 19.0],
        &format!("{} {}", current.clock_label, current.clock),
        10.0,
        text_secondary(),
    );
    c.number([w - 148.0, 29.0, 134.0, 51.0], &current.countdown, 42.0, text_primary());
    c.text([w - 148.0, 78.0, 134.0, 15.0], "PLANNED COUNTDOWN", 9.0, text_tertiary());
    c.icon([15.0, 93.0, 13.0, 13.0], "route", color);
    c.text([35.0, 91.0, w - 50.0, 16.0], &current.progress_label, 11.0, text_secondary());
    // A segmented track makes progress readable without promoting calendar time to observed laps.
    let width = w - 30.0;
    for i in 0_u8..20 {
        let x = 15.0 + f32::from(i) * width / 20.0;
        let segment = c.area([x, 112.0, width / 20.0 - 2.0, 5.0]);
        c.ui.painter().rect_filled(segment, c.metrics.px(1.0), TRACK);
        let fill = (current.progress * 20.0 - f32::from(i)).clamp(0.0, 1.0);
        if fill > 0.0 {
            let filled = Rect::from_min_size(segment.min, egui::vec2(segment.width() * fill, segment.height()));
            if current.observed_laps {
                c.ui.painter().rect_filled(filled, c.metrics.px(1.0), color);
            } else {
                crate::ui::paint_hatch(c.ui, c.metrics, filled, text_secondary());
            }
        }
    }
    c.hover(
        "stints-current",
        &format!("{}\n{} {}\n{}", current.name, current.clock_label, current.clock, current.progress_label),
    );
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "fuel values are bounded API quantities; ratios are clamped to the tile width"
)]
fn fuel(c: &Canvas<'_>, fuel: &Fuel) {
    c.tile();
    let w = c.width();
    let delta = fuel.target.zip(fuel.measured).map(|(target, actual)| f64::from(actual) - target);
    let ink = delta.map_or(text_secondary(), |delta| if delta > 0.025 { CAUTION } else { SIGNAL });
    c.icon([13.0, 11.0, 17.0, 17.0], "fuel-pump", text_secondary());
    c.text([38.0, 9.0, w - 142.0, 20.0], "FUEL / LAP", 11.0, text_secondary());
    if let Some(delta) = delta {
        c.pill([w - 89.0, 10.0, 76.0, 19.0], &format!("{delta:+.2} L"), ink);
    }
    let target = fuel.target.map_or_else(|| "--".to_owned(), |v| format!("{v:.2}"));
    let actual = fuel.measured.map_or_else(|| "--".to_owned(), |v| format!("{v:.2}"));
    let half = w / 2.0;
    c.number([14.0, 33.0, half - 25.0, 37.0], &target, 32.0, text_primary());
    c.number([half + 7.0, 33.0, half - 21.0, 37.0], &actual, 32.0, ink);
    c.text([14.0, 68.0, half - 25.0, 17.0], &format!("{} TARGET", fuel.setup.to_uppercase()), 9.0, text_tertiary());
    c.text([half + 7.0, 68.0, half - 21.0, 17.0], "MEASURED", 9.0, text_tertiary());
    let max = fuel.target.unwrap_or(0.0).max(f64::from(fuel.measured.unwrap_or(0.0))).max(0.1) * 1.1;
    for (x, value, color) in [(14.0, fuel.target, text_secondary()), (half + 7.0, fuel.measured.map(f64::from), ink)] {
        let width = half - 28.0;
        c.ui.painter().rect_filled(c.area([x, 90.0, width, 4.0]), c.metrics.px(1.0), TRACK);
        if let Some(value) = value {
            c.ui.painter().rect_filled(
                c.area([x, 90.0, width * (value / max).clamp(0.0, 1.0) as f32, 4.0]),
                c.metrics.px(1.0),
                color,
            );
        }
    }
    c.text([14.0, 102.0, w - 28.0, 16.0], fuel.source, 10.0, text_tertiary());
    c.hover(
        "stints-fuel",
        &format!(
            "{} setup / litres per lap\nTarget: {target}\nMeasured: {actual}\n{}\nScheduled stint total: {} L",
            fuel.setup,
            fuel.source,
            fuel.planned_total.map_or_else(|| "--".to_owned(), |v| format!("{v:.1}"))
        ),
    );
}

fn next(c: &Canvas<'_>, view: &View) {
    c.tile();
    let w = c.width();
    let Some(next) = &view.next else {
        c.icon([15.0, 16.0, 23.0, 23.0], "flag-waving", text_secondary());
        c.text([48.0, 13.0, w - 61.0, 29.0], "Final driver", 19.0, text_primary());
        c.text([15.0, 62.0, w - 30.0, 20.0], "No later change planned", 12.0, text_secondary());
        return;
    };
    let panel = view.handover.as_ref();
    let ready = panel.is_some_and(|panel| panel.ready);
    let fresh = panel.filter(|panel| !panel.handover.stale);
    let color = accent(&next.name);
    c.icon([13.0, 11.0, 17.0, 17.0], "car-front", color);
    c.text([38.0, 9.0, w - 143.0, 20.0], "NEXT DRIVER", 11.0, text_secondary());
    let badge = c.area([w - 97.0, 10.0, 84.0, 19.0]);
    if ready {
        c.ui.painter().rect_filled(badge, c.metrics.px(3.0), SIGNAL.gamma_multiply(0.13));
        let stroke = Stroke::new(c.metrics.px(1.5), SIGNAL);
        let point = |x: f32, y: f32| c.area([w - 97.0 + x, 10.0 + y, 0.0, 0.0]).min;
        c.ui.painter().line_segment([point(7.0, 10.0), point(10.0, 13.0)], stroke);
        c.ui.painter().line_segment([point(10.0, 13.0), point(16.0, 6.0)], stroke);
        c.text([w - 75.0, 10.0, 58.0, 19.0], "READY", 10.0, SIGNAL);
    } else {
        c.pill([w - 97.0, 10.0, 84.0, 19.0], "AWAITING", text_tertiary());
    }
    c.text([14.0, 33.0, w - 99.0, 29.0], &next.name, 25.0, text_primary());
    avatar(c, [14.0, 70.0, 32.0, 32.0], &next.name);
    let forecast = fresh.and_then(|panel| panel.handover.estimated_start);
    let at = time(forecast.unwrap_or(next.start));
    c.number([58.0, 60.0, w - 154.0, 39.0], &at, 34.0, text_primary());
    c.text(
        [58.0, 96.0, w - 154.0, 15.0],
        if forecast.is_some() { "EST. HANDOVER" } else { "PLANNED CHANGE" },
        9.0,
        text_tertiary(),
    );
    if let Some(panel) = fresh {
        if panel.handover.in_pits {
            c.icon([w - 60.0, 65.0, 26.0, 26.0], "car-front", CAUTION);
            c.text([w - 70.0, 98.0, 58.0, 13.0], "IN PITS", 10.0, CAUTION);
        } else if let Some(laps) = panel.handover.laps {
            let ink = if panel.handover.due { CAUTION } else { text_primary() };
            let ring = c.area([w - 81.0, 53.0, 64.0, 64.0]);
            c.ui.painter().circle_filled(ring.center(), ring.width() / 2.0, ink.gamma_multiply(0.05));
            c.ui.painter().circle_stroke(
                ring.center(),
                ring.width() / 2.0,
                Stroke::new(c.metrics.px(1.5), ink.gamma_multiply(0.6)),
            );
            paint_text(
                c.ui,
                ring.center() - c.metrics.vec2(0.0, 7.0),
                Align2::CENTER_CENTER,
                readout(format!("~{laps}"), c.metrics.px(34.0)).color(ink),
            );
            paint_text(
                c.ui,
                ring.center() + c.metrics.vec2(0.0, 17.0),
                Align2::CENTER_CENTER,
                RichText::new("LAPS").size(c.metrics.px(9.0)).color(text_tertiary()),
            );
        }
    }
    let note = if panel.is_some_and(|panel| panel.handover.stale) {
        "Plan stale / verify with crew".to_owned()
    } else if let Some(shift) =
        fresh.and_then(|panel| panel.handover.shift_seconds).filter(|shift| shift.unsigned_abs() >= 60)
    {
        format!(
            "{}m {} / plan {}",
            shift.unsigned_abs() / 60,
            if shift > 0 { "late" } else { "early" },
            time(next.start)
        )
    } else if forecast.is_some() {
        "Fuel + plan estimate".to_owned()
    } else {
        "Plan estimate / no live fuel".to_owned()
    };
    c.text([14.0, 115.0, w - 109.0, 14.0], &note, 10.0, if view.warning { CAUTION } else { text_secondary() });
    if let Some(panel) = panel {
        c.hover(
            "stints-next",
            &format!(
                "{}\n{}\nReady is the incoming driver's acknowledgement.",
                panel.handover.basis, panel.handover.detail
            ),
        );
    }
}

fn drivers(view: &View) -> Vec<&str> {
    let mut drivers = Vec::new();
    for block in &view.calendar {
        if !drivers.contains(&block.name.as_str()) {
            drivers.push(block.name.as_str());
        }
    }
    drivers
}

struct TimeAxis {
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
    left: f32,
    width: f32,
}

impl TimeAxis {
    fn x(&self, at: chrono::DateTime<chrono::Utc>) -> f32 {
        let span = (self.end - self.start).to_std().map_or(1.0, |d| d.as_secs_f32()).max(1.0);
        let elapsed = (at - self.start).to_std().map_or(0.0, |d| d.as_secs_f32());
        self.left + (elapsed / span).clamp(0.0, 1.0) * self.width
    }
}

fn calendar_block(c: &Canvas<'_>, axis: &TimeAxis, block: &Preview, row: u8, view: &View) {
    let x = axis.x(block.start);
    let width = (axis.x(block.end) - x - 2.0).max(3.0);
    let y = 34.0 + f32::from(row) * 56.0;
    let rect = c.area([x, y, width, 41.0]);
    let color = accent(&block.name);
    let in_car = view.current.as_ref().is_some_and(|current| current.number == block.number && current.live_driver);
    let next = view.handover.as_ref().filter(|panel| panel.handover.key.stint_number == block.number);
    let edge = if in_car {
        color
    } else if next.is_some_and(|panel| panel.ready) {
        SIGNAL
    } else {
        color.gamma_multiply(0.4)
    };
    c.ui.painter().rect_filled(rect, c.metrics.px(4.0), color.gamma_multiply(if in_car { 0.32 } else { 0.14 }));
    c.ui.painter().rect_stroke(
        rect,
        c.metrics.px(4.0),
        Stroke::new(c.metrics.px(if in_car { 2.0 } else { 1.0 }), edge),
    );
    let title = if width > 99.0 {
        block.laps.map_or_else(|| format!("#{}", block.number), |laps| format!("#{} / {laps} laps", block.number))
    } else {
        format!("#{}", block.number)
    };
    c.text([x + 8.0, y + 2.0, width - 14.0, 19.0], &title, 12.0, if in_car { text_primary() } else { color });
    if width >= 50.0 {
        c.icon(
            [x + 8.0, y + 25.0, 10.0, 10.0],
            if in_car { "car-front" } else { "fuel-pump" },
            if in_car { color } else { text_secondary() },
        );
        let detail = if in_car {
            "IN CAR".to_owned()
        } else {
            block.load.map_or_else(|| "-- L".to_owned(), |load| format!("{load:.0} L"))
        };
        c.text([x + 23.0, y + 21.0, width - 28.0, 19.0], &detail, 10.0, text_secondary());
    }
    c.ui.interact(rect, c.ui.id().with(("stint-block", block.number)), egui::Sense::hover()).on_hover_text(format!(
        "#{} / {}\n{} - {} / {} minutes\n{} laps / {} L planned load{}",
        block.number,
        block.name,
        time(block.start),
        time(block.end),
        (block.end - block.start).num_minutes(),
        block.laps.map_or_else(|| "--".to_owned(), |v| v.to_string()),
        block.load.map_or_else(|| "--".to_owned(), |v| format!("{v:.0}")),
        if block.repeat { "\nSame driver stays in" } else { "" }
    ));
}

fn calendar(c: &Canvas<'_>, view: &View) {
    let (Some(first), Some(last)) = (view.calendar.first(), view.calendar.last()) else { return };
    let names = drivers(view);
    let start =
        chrono::DateTime::from_timestamp(first.start.timestamp().div_euclid(1800) * 1800, 0).unwrap_or(first.start);
    let end =
        chrono::DateTime::from_timestamp((last.end.timestamp().div_euclid(1800) + 1) * 1800, 0).unwrap_or(last.end);
    let axis = TimeAxis { start, end, left: 113.0, width: c.width() - 116.0 };
    let row_count = f32::from(u8::try_from(names.len()).unwrap_or(4));
    let bottom = 34.0 + row_count * 56.0;
    let step = if (end - start).num_hours() >= 5 { 7200 } else { 3600 };
    for tick in 0..=12 {
        let Some(at) = start.checked_add_signed(chrono::Duration::seconds(tick * step)) else { break };
        if at > end {
            break;
        }
        let x = axis.x(at);
        c.ui.painter().line_segment(
            [c.area([x, 25.0, 0.0, 0.0]).min, c.area([x, bottom - 8.0, 0.0, 0.0]).min],
            Stroke::new(c.metrics.px(1.0), TRACK.gamma_multiply(0.7)),
        );
        c.text([(x - 22.0).clamp(88.0, c.width() - 50.0), 4.0, 50.0, 16.0], &time(at), 10.0, text_tertiary());
    }
    for pair in view.calendar.windows(2).filter(|pair| pair[0].name != pair[1].name) {
        let row =
            |name: &str| names.iter().position(|known| *known == name).and_then(|i| u8::try_from(i).ok()).unwrap_or(0);
        let from = egui::pos2(axis.x(pair[0].end), 54.5 + f32::from(row(&pair[0].name)) * 56.0);
        let to = egui::pos2(axis.x(pair[1].start), 54.5 + f32::from(row(&pair[1].name)) * 56.0);
        let mid = f32::midpoint(from.x, to.x);
        let ready = view
            .handover
            .as_ref()
            .is_some_and(|panel| panel.ready && panel.handover.key.stint_number == pair[1].number);
        let color = if ready { SIGNAL } else { text_secondary() };
        let at = |point: egui::Pos2| c.area([point.x, point.y, 0.0, 0.0]).min;
        c.ui.painter().add(egui::Shape::line(
            vec![at(from), at(egui::pos2(mid, from.y)), at(egui::pos2(mid, to.y)), at(to)],
            Stroke::new(c.metrics.px(1.5), color),
        ));
        c.ui.painter().line_segment([at(to - egui::vec2(5.0, 4.0)), at(to)], Stroke::new(c.metrics.px(1.5), color));
        c.ui.painter().line_segment([at(to - egui::vec2(5.0, -4.0)), at(to)], Stroke::new(c.metrics.px(1.5), color));
    }
    for (i, name) in names.iter().enumerate() {
        let y = 34.0 + f32::from(u8::try_from(i).unwrap_or(0)) * 56.0;
        avatar(c, [3.0, y + 9.0, 23.0, 23.0], name);
        c.text([34.0, y + 6.0, 71.0, 29.0], name, 11.0, text_primary());
        for block in view.calendar.iter().filter(|block| block.name == *name) {
            calendar_block(c, &axis, block, u8::try_from(i).unwrap_or(0), view);
        }
    }
    if let Some(now) = view.now.filter(|now| *now >= start && *now <= end) {
        let x = axis.x(now);
        c.ui.painter().line_segment(
            [c.area([x, 25.0, 0.0, 0.0]).min, c.area([x, bottom - 2.0, 0.0, 0.0]).min],
            Stroke::new(c.metrics.px(1.5), text_primary()),
        );
        c.ui.painter().circle_filled(c.area([x, 25.0, 0.0, 0.0]).min, c.metrics.px(3.0), text_primary());
        let marker_x = (x - 24.0).clamp(axis.left, c.width() - 89.0);
        c.pill([marker_x, bottom, 86.0, 18.0], &format!("NOW {}", time(now)), text_primary());
    }
}

pub fn alert_marker(ui: &Ui, metrics: Metrics, rect: Rect, panel: &crate::iraceplan::handover::Panel) {
    let c = Canvas { ui, metrics, rect };
    let mark = c.area([13.0, 13.0, 59.0, 59.0]);
    let handover = &panel.handover;
    ui.painter().circle_filled(mark.center(), mark.width() / 2.0, CAUTION.gamma_multiply(0.10));
    ui.painter().circle_stroke(mark.center(), mark.width() / 2.0, Stroke::new(metrics.px(1.5), CAUTION));
    if handover.stale {
        icons::svg(ui, mark.shrink(metrics.px(13.0)), "circle-alert", CAUTION);
    } else if handover.in_pits {
        icons::svg(ui, mark.shrink(metrics.px(12.0)), "car-front", CAUTION);
    } else if handover.incoming && handover.laps.is_some() {
        paint_text(
            ui,
            mark.center() - metrics.vec2(0.0, 7.0),
            Align2::CENTER_CENTER,
            readout(format!("~{}", handover.laps.unwrap_or(0)), metrics.px(34.0)).color(CAUTION),
        );
        paint_text(
            ui,
            mark.center() + metrics.vec2(0.0, 17.0),
            Align2::CENTER_CENTER,
            RichText::new("LAPS").size(metrics.px(9.0)).color(CAUTION),
        );
    } else {
        icons::svg(ui, mark.shrink(metrics.px(12.0)), "car-front", if panel.ready { SIGNAL } else { CAUTION });
    }
}

pub fn draw(ui: &mut Ui, metrics: Metrics, view: &View) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let width = ui.available_width();
        let pad = metrics.px(14.0);
        let content = |rect: Rect| rect.shrink2(egui::vec2(pad, 0.0));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, metrics.px(36.0)), egui::Sense::hover());
        header(&Canvas { ui, metrics, rect: content(rect) }, view);
        let Some(current) = &view.current else {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, metrics.px(100.0)), egui::Sense::hover());
            let c = Canvas { ui, metrics, rect: content(rect) };
            c.icon([15.0, 20.0, 35.0, 35.0], "flag-waving", SIGNAL);
            c.text([65.0, 20.0, c.width() - 80.0, 35.0], "Schedule complete", 23.0, text_primary());
            return;
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, metrics.px(127.0)), egui::Sense::hover());
        hero(&Canvas { ui, metrics, rect: content(rect) }, current);
        ui.add_space(metrics.px(8.0));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, metrics.px(132.0)), egui::Sense::hover());
        let rect = content(rect);
        let split = if rect.width() >= metrics.px(600.0) { 0.40 } else { 0.5 };
        let mid = rect.left() + rect.width() * split;
        fuel(
            &Canvas {
                ui,
                metrics,
                rect: Rect::from_min_max(rect.min, egui::pos2(mid - metrics.px(4.0), rect.bottom())),
            },
            &view.fuel,
        );
        next(
            &Canvas { ui, metrics, rect: Rect::from_min_max(egui::pos2(mid + metrics.px(4.0), rect.top()), rect.max) },
            view,
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, metrics.px(37.0)), egui::Sense::hover());
        let c = Canvas { ui, metrics, rect: content(rect) };
        c.icon([3.0, 13.0, 18.0, 18.0], "route", text_secondary());
        c.text([31.0, 10.0, c.width() - 31.0, 25.0], "STINT PLAN / LOCAL TIME", 11.0, text_secondary());
        let height = 56.0 * f32::from(u8::try_from(drivers(view).len()).unwrap_or(4)) + 58.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, metrics.px(height)), egui::Sense::hover());
        calendar(&Canvas { ui, metrics, rect: content(rect) }, view);
        ui.add_space(metrics.px(7.0));
    });
}
