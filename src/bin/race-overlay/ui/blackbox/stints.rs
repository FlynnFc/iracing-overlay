//! A visual stint board. Observed laps and calendar progress stay explicitly separate.

mod visuals;

use chrono::{DateTime, Local, Utc};
use egui::{Align2, RichText, Ui};

use super::{CAUTION, Metrics, paint_text, text_primary};
use crate::iraceplan::{Plan, Stint};
use crate::telemetry::snapshot::{Seat, TelemetrySnapshot};
use crate::telemetry::stint_estimation::StintAge;

#[derive(Debug, Clone, Default)]
pub struct View {
    heading: String,
    status: String,
    status_detail: String,
    warning: bool,
    current: Option<Current>,
    fuel: Fuel,
    next: Option<Next>,
    calendar: Vec<Preview>,
    now: Option<DateTime<Utc>>,
    handover: Option<crate::iraceplan::handover::Panel>,
}

#[derive(Debug, Clone)]
struct Current {
    name: String,
    phase: &'static str,
    number: u32,
    total: usize,
    countdown: String,
    clock_label: &'static str,
    clock: String,
    progress: f32,
    progress_label: String,
    observed_laps: bool,
    live_driver: bool,
    driver_note: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Fuel {
    target: Option<f64>,
    measured: Option<f32>,
    setup: String,
    planned_total: Option<f64>,
    source: &'static str,
}

#[derive(Debug, Clone)]
struct Next {
    name: String,
    start: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct Preview {
    number: u32,
    name: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    laps: Option<u32>,
    load: Option<f64>,
    repeat: bool,
}

impl View {
    pub fn set_handover(&mut self, panel: Option<&crate::iraceplan::handover::Panel>) {
        self.handover = panel.cloned();
    }
}

fn name(stint: &Stint) -> &str {
    stint.driver.as_ref().map_or("Unassigned", |driver| driver.name.as_str())
}

fn time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

fn remaining(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{}:{:02}", minutes, seconds.max(0) % 60)
    }
}

pub fn view(snapshot: Option<&TelemetrySnapshot>, synced: Option<&crate::sync::store::SyncedCar>) -> View {
    let feed = crate::iraceplan::feed();
    let Some(plan) = &feed.plan else {
        return View { status: "Waiting for iRacePlan".to_owned(), ..View::default() };
    };
    let mut view = build(plan, Utc::now(), snapshot, synced);
    view.warning = feed.stale() || plan.strategy.status != "up-to-date";
    let age = feed.fetched.map_or(0, |at| at.elapsed().as_secs());
    view.status = if feed.stale() { format!("STALE / {age}s") } else { format!("Updated {age}s") };
    view.status_detail = format!("{} / {}", feed.status, plan.strategy.status);
    view
}

fn current(plan: &Plan, index: usize, now: DateTime<Utc>) -> Current {
    let stint = &plan.strategy.stints[index];
    let underway = now >= stint.start_at;
    let duration = (stint.end_at - stint.start_at).to_std().map_or(1.0, |d| d.as_secs_f32()).max(1.0);
    let elapsed = (now - stint.start_at).to_std().map_or(0.0, |d| d.as_secs_f32());
    Current {
        name: name(stint).to_owned(),
        phase: if underway { "ON THE PLAN" } else { "UPCOMING STINT" },
        number: stint.number,
        total: plan.strategy.stints.len(),
        countdown: remaining((if underway { stint.end_at } else { stint.start_at } - now).num_seconds()),
        clock_label: if underway { "PLANNED END" } else { "PLANNED START" },
        clock: time(if underway { stint.end_at } else { stint.start_at }),
        progress: (elapsed / duration).clamp(0.0, 1.0),
        progress_label: "Schedule progress".to_owned(),
        observed_laps: false,
        live_driver: false,
        driver_note: None,
    }
}

fn measured_fuel(
    snapshot: &TelemetrySnapshot,
    synced: Option<&crate::sync::store::SyncedCar>,
    car_idx: i32,
) -> (Option<f32>, &'static str) {
    let (burn, source) = if let Some(synced) = synced.filter(|car| car.car_idx == Some(car_idx)) {
        (synced.burn_per_lap, "Team sync")
    } else if snapshot.seat == Seat::Driving && snapshot.pit_service.fuel_reading_valid {
        (snapshot.pit_service.fuel_per_lap_litres, "Live telemetry")
    } else {
        (None, "No live fuel")
    };
    (burn.filter(|value| value.is_finite() && *value > 0.0), source)
}

#[expect(clippy::cast_precision_loss, reason = "stint lap counts are small and the result is a bounded visual ratio")]
fn observed_progress(current: &mut Current, completed: i32, total: u32) {
    if total > 0 {
        let completed = completed.max(0);
        current.observed_laps = true;
        current.progress = (completed as f32 / total as f32).clamp(0.0, 1.0);
        current.progress_label = format!("{completed} / {total} laps observed");
    }
}

fn build(
    plan: &Plan,
    now: DateTime<Utc>,
    snapshot: Option<&TelemetrySnapshot>,
    synced: Option<&crate::sync::store::SyncedCar>,
) -> View {
    let mut view = View {
        heading: format!("{} / {}", plan.planning.registrable.name, plan.strategy.name),
        now: Some(now),
        ..View::default()
    };
    let (index, observed) = crate::iraceplan::handover::active_index(plan, now, snapshot);
    let Some(index) = index else { return view };
    let stint = &plan.strategy.stints[index];
    let mut current = current(plan, index, now);
    view.fuel = Fuel {
        target: stint.driver.as_ref().and_then(|driver| plan.fuel_target(stint, driver.iracing_id)),
        setup: stint.setup.as_deref().unwrap_or("Unknown setup").to_owned(),
        source: "No live fuel",
        planned_total: stint.fuel_consumption.filter(|v| v.is_finite() && *v > 0.0),
        measured: None,
    };
    if let Some(snapshot) = snapshot.filter(|s| matches_plan(plan, s) && now >= stint.start_at)
        && let Some(driver) = crate::iraceplan::handover::plan_car(plan, snapshot)
    {
        current.phase = "IN THE CAR";
        current.live_driver = true;
        current.name = driver.driver_name.to_string();
        if driver.cust_id != stint.driver.as_ref().map(|d| d.iracing_id) {
            current.driver_note = Some(format!("Driver differs / plan: {}", name(stint)));
        }
        (view.fuel.measured, view.fuel.source) = measured_fuel(snapshot, synced, driver.car_idx);
        // A different actual driver has their own target, never the scheduled driver's rate.
        view.fuel.target = driver.cust_id.and_then(|id| plan.fuel_target(stint, id));
        if observed
            && let Some(entry) = snapshot.standings.iter().find(|entry| entry.car_idx == driver.car_idx)
            && let StintAge::Observed(completed) = entry.stint_age
            && let Some(total) = stint.estimated_laps
        {
            observed_progress(&mut current, completed, total);
        }
    }
    view.current = Some(current);
    let driver_id = |stint: &Stint| stint.driver.as_ref().map(|driver| driver.iracing_id);
    view.next = plan
        .strategy
        .stints
        .iter()
        .skip(index + 1)
        .find(|next| driver_id(next) != driver_id(stint))
        .map(|next| Next { name: name(next).to_owned(), start: next.start_at });
    view.calendar = plan
        .strategy
        .stints
        .iter()
        .enumerate()
        .skip(index)
        .take(4)
        .map(|(i, next)| Preview {
            number: next.number,
            name: name(next).to_owned(),
            start: next.start_at,
            end: next.end_at,
            laps: next.estimated_laps,
            load: next.pit_fuel_capacity.filter(|v| v.is_finite() && *v > 0.0),
            repeat: i > 0 && driver_id(next) == driver_id(&plan.strategy.stints[i - 1]),
        })
        .collect();
    view
}

fn matches_plan(plan: &Plan, snapshot: &TelemetrySnapshot) -> bool {
    crate::iraceplan::handover::plan_car(plan, snapshot).is_some()
}

pub fn draw(ui: &mut Ui, metrics: Metrics, view: &View) {
    visuals::draw(ui, metrics, view);
}

pub fn draw_handover(
    ui: &mut Ui,
    metrics: Metrics,
    panel: &crate::iraceplan::handover::Panel,
    width: f32,
    clicks: &mut Vec<super::Click>,
) {
    let handover = &panel.handover;
    let height = metrics.px(88.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let rect = rect.shrink(metrics.px(1.0));
    ui.painter().rect_filled(rect, metrics.px(6.0), egui::Color32::from_rgb(42, 34, 23));
    ui.painter().rect_stroke(rect, metrics.px(6.0), egui::Stroke::new(metrics.px(1.0), CAUTION));
    visuals::alert_marker(ui, metrics, rect, panel);
    let title = if handover.stale {
        "HANDOVER · PLAN STALE".to_owned()
    } else if handover.incoming {
        if handover.in_pits {
            "DRIVING NEXT · CAR IN PITS".to_owned()
        } else {
            handover.laps.map_or_else(
                || "DRIVING SOON".to_owned(),
                |laps| {
                    if laps == 0 {
                        "DRIVING NEXT · CHECK WITH CREW".to_owned()
                    } else {
                        format!("DRIVING IN ~{laps} {}", if laps == 1 { "LAP" } else { "LAPS" })
                    }
                },
            )
        }
    } else {
        format!("NEXT DRIVER · {}", handover.driver)
    };
    let button_width = if handover.incoming { metrics.px(100.0) } else { 0.0 };
    let available = (rect.width() - button_width - metrics.px(101.0)).max(0.0);
    let title = super::super::elide_to_width(ui, &title, available, |text| {
        RichText::new(text).strong().size(metrics.px(17.0)).color(CAUTION)
    });
    paint_text(
        ui,
        egui::pos2(rect.left() + metrics.px(85.0), rect.top() + metrics.px(19.0)),
        Align2::LEFT_CENTER,
        title,
    );
    let status = if panel.ready {
        "READY acknowledged"
    } else if handover.incoming && handover.stale {
        "Refresh plan before marking Ready"
    } else if handover.incoming && !panel.can_mark {
        "Connect team sync to share Ready"
    } else {
        "Not yet acknowledged"
    };
    for (text, y) in [(format!("{} · {status}", handover.basis), 43.0), (handover.detail.clone(), 67.0)] {
        let text = super::super::elide_to_width(ui, &text, rect.width() - metrics.px(97.0), |text| {
            RichText::new(text).size(metrics.px(11.0)).color(text_primary())
        });
        paint_text(
            ui,
            egui::pos2(rect.left() + metrics.px(85.0), rect.top() + metrics.px(y)),
            Align2::LEFT_CENTER,
            text,
        );
    }
    if handover.incoming {
        let button = egui::Rect::from_min_size(
            egui::pos2(rect.right() - button_width - metrics.px(8.0), rect.top() + metrics.px(6.0)),
            egui::vec2(button_width, metrics.px(28.0)),
        );
        let response = ui
            .scope(|ui| {
                if !panel.can_mark {
                    ui.disable();
                }
                ui.put(button, egui::Button::new(if panel.ready { "Not ready" } else { "Ready" }))
            })
            .inner;
        if response.clicked() {
            clicks.push(super::Click::HandoverReady { key: handover.key, ready: !panel.ready });
        }
    }
    ui.add_space(metrics.px(5.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_button_emits_the_assignment_and_respects_disabled_sync() {
        use crate::iraceplan::handover::{Handover, Panel};
        let key = crate::sync::protocol::HandoverKey {
            planning_id: 1,
            strategy_id: 2,
            stint_number: 3,
            driver_id: 4,
            starts_at: 1000,
        };
        let mut panel = Panel {
            handover: Handover {
                key,
                driver: "Incoming".to_owned(),
                laps: Some(3),
                due: true,
                incoming: true,
                basis: "Team sync + plan",
                detail: "Plan and fuel agree".to_owned(),
                stale: false,
                in_pits: false,
                estimated_start: None,
                shift_seconds: None,
            },
            ready: false,
            can_mark: true,
        };
        let run = |ctx: &egui::Context, panel: &Panel, events| {
            let mut clicks = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_handover(ui, Metrics::new(1.0), panel, 600.0, &mut clicks);
                });
            });
            clicks
        };
        let click = |panel: &Panel| {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            let readout = fonts.families[&egui::FontFamily::Proportional].clone();
            fonts.families.insert(egui::FontFamily::Name(crate::ui::READOUT_FAMILY.into()), readout);
            ctx.set_fonts(fonts);
            let _ = run(&ctx, panel, Vec::new());
            let position = egui::pos2(550.0, 28.0);
            let _ = run(
                &ctx,
                panel,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            run(
                &ctx,
                panel,
                vec![egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            )
        };
        assert_eq!(click(&panel), vec![super::super::Click::HandoverReady { key, ready: true }]);
        panel.ready = true;
        assert_eq!(click(&panel), vec![super::super::Click::HandoverReady { key, ready: false }]);
        panel.can_mark = false;
        assert!(click(&panel).is_empty());
    }

    #[test]
    fn handover_skips_same_driver_stints_but_preview_keeps_them() {
        let now = Utc::now();
        let plan = crate::iraceplan::example_plan(now - chrono::Duration::minutes(20));
        let view = build(&plan, now, None, None);
        assert!(view.next.as_ref().is_some_and(|next| next.name == "Sam Taylor"));
        assert!(view.calendar[1].name == "Alex Morgan");
        assert_eq!(view.calendar.len(), 4);
    }

    #[test]
    fn live_comparison_requires_matching_team_track_car_and_racing() {
        let now = Utc::now();
        let plan = crate::iraceplan::example_plan(now - chrono::Duration::minutes(20));
        let mut snapshot = crate::demo::snapshot();
        snapshot.identity.track_id = Some(341);
        snapshot.identity.team_id = Some(1);
        snapshot.relative_meta.racing_under_way = true;
        let driver = &mut snapshot.relative[snapshot.focus_index];
        driver.car_screen_name = "Ferrari 296 GT3".into();
        driver.cust_id = Some(2);
        driver.driver_name = "Sam Taylor".into();
        snapshot.pit_service.fuel_per_lap_litres = Some(3.6);
        let view = build(&plan, now, Some(&snapshot), None);
        assert!(view.current.as_ref().unwrap().driver_note.as_ref().unwrap().contains("Driver differs"));
        assert_eq!(view.fuel.measured, Some(3.6));
        assert_eq!(view.fuel.target, Some(3.42));
        snapshot.identity.team_id = Some(999);
        assert!(!matches_plan(&plan, &snapshot));
        snapshot.identity.team_id = Some(1);
        snapshot.identity.track_id = Some(999);
        assert!(!matches_plan(&plan, &snapshot));
        snapshot.identity.track_id = Some(341);
        snapshot.relative[snapshot.focus_index].car_screen_name = "Another car".into();
        assert!(!matches_plan(&plan, &snapshot));
        snapshot.relative[snapshot.focus_index].car_screen_name = "Ferrari 296 GT3".into();
        snapshot.relative_meta.racing_under_way = false;
        assert!(!matches_plan(&plan, &snapshot));
    }

    #[test]
    fn spectator_without_sync_never_reads_local_fuel() {
        let now = Utc::now();
        let plan = crate::iraceplan::example_plan(now - chrono::Duration::minutes(20));
        let mut snapshot = crate::demo::snapshot();
        snapshot.identity.track_id = Some(341);
        snapshot.identity.team_id = Some(1);
        snapshot.relative_meta.racing_under_way = true;
        snapshot.relative[snapshot.focus_index].car_screen_name = "Ferrari 296 GT3".into();
        snapshot.seat = Seat::TeamMate("Alex Morgan".into());
        snapshot.pit_service.fuel_per_lap_litres = Some(99.0);
        let view = build(&plan, now, Some(&snapshot), None);
        assert_eq!(view.fuel.measured, None);
    }
}
