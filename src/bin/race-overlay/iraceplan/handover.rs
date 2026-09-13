//! Reconciles a scheduled handover with observed progress. Fuel is a range limit,
//! not evidence that the crew has committed to a driver change at the next stop.

use super::Plan;
use crate::sync::protocol::HandoverKey;
use crate::sync::store::SyncedCar;
use crate::telemetry::snapshot::{CarSnapshot, CourseFlag, Seat, TelemetrySnapshot};
use crate::telemetry::stint_estimation::StintAge;
use chrono::{DateTime, Utc};
use std::fmt::Write;

const ALERT_LAPS: f64 = 3.0;

/// A labelled demo on the incoming driver's spectator screen; no network or writes.
pub fn seed_demo(snapshot: &mut TelemetrySnapshot) {
    snapshot.identity.subsession = Some(1);
    snapshot.identity.track_id = Some(341);
    snapshot.identity.team_id = Some(1);
    snapshot.identity.player_cust_id = Some(2);
    snapshot.seat = Seat::Spectating("Alex Morgan".into());
    snapshot.relative_meta.spectating = Some("Alex Morgan".into());
    snapshot.relative_meta.racing_under_way = true;
    snapshot.course_flag = CourseFlag::Green;
    snapshot.endurance.lap_driven_pct = Some(0.0);
    let car = &mut snapshot.relative[snapshot.focus_index];
    car.cust_id = Some(1);
    car.driver_name = "Alex Morgan".into();
    car.car_screen_name = "Ferrari 296 GT3".into();
    car.recent_laps = [Some(120.0); 3];
    car.track_location = iracing_telem::flags::TrackLocation::OnTrack;
    if let Some(entry) = snapshot.standings.iter_mut().find(|entry| entry.car_idx == car.car_idx) {
        entry.current_stint_secs = 52.0 * 60.0;
        entry.stint_age = StintAge::Observed(26);
    }
}

/// Additional screenshot fixtures use the same telemetry and event-folding paths.
pub fn demo_state(snapshot: &mut TelemetrySnapshot, state: &str) {
    seed_demo(snapshot);
    if state == "stints" {
        let car_idx = snapshot.relative[snapshot.focus_index].car_idx;
        if let Some(entry) = snapshot.standings.iter_mut().find(|entry| entry.car_idx == car_idx) {
            entry.current_stint_secs = 23.0 * 60.0;
            entry.stint_age = StintAge::Observed(11);
        }
    }
    if state == "handover-crew" {
        snapshot.identity.player_cust_id = Some(3);
    }
    if state == "handover-pits" {
        snapshot.relative[snapshot.focus_index].track_location = iracing_telem::flags::TrackLocation::InPitStall;
    }
}

pub fn demo_events(states: &[String]) -> Vec<(f64, crate::sync::protocol::Event)> {
    use crate::sync::protocol::Event;
    if !states.iter().any(|state| state == "stints" || state.starts_with("handover-")) {
        return Vec::new();
    }
    let Some(plan) = super::feed().plan else { return Vec::new() };
    let car_idx = crate::demo::snapshot().relative.iter().find(|car| car.is_focus).map(|car| car.car_idx);
    let normal_stint = states.iter().any(|state| state == "stints");
    let mut events = vec![
        (0.0, Event::StintBoundary { driver: "Alex Morgan".to_owned() }),
        (1.0, Event::LapClosed { lap: 25, fuel_litres: 18.0, used_litres: 3.6 }),
        (2.0, Event::LapClosed { lap: 26, fuel_litres: 14.4, used_litres: 3.6 }),
        (
            3.0,
            Event::DriverScalars {
                car_idx,
                fuel_litres: if normal_stint { 65.0 } else { 14.4 },
                service_fuel_litres: None,
                tyres_armed: [false; 4],
                tyre_pressures_kpa: [159.0; 4],
            },
        ),
    ];
    if states.iter().any(|state| matches!(state.as_str(), "handover-ready" | "handover-crew"))
        && let Some(key) = plan.strategy.stints.get(2).and_then(|stint| key_for(&plan, stint))
    {
        events.push((4.0, Event::HandoverReady { key, ready: true }));
    }
    events
}

#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "due time, local seat, freshness and pit location are independent observations"
)]
pub struct Handover {
    pub key: HandoverKey,
    pub driver: String,
    pub laps: Option<u32>,
    pub due: bool,
    pub incoming: bool,
    pub basis: &'static str,
    pub detail: String,
    pub stale: bool,
    pub in_pits: bool,
    pub estimated_start: Option<DateTime<Utc>>,
    pub shift_seconds: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Panel {
    pub handover: Handover,
    pub ready: bool,
    pub can_mark: bool,
}

pub fn current(
    snapshot: Option<&TelemetrySnapshot>,
    synced: Option<&SyncedCar>,
    reserve_laps: f32,
) -> Option<Handover> {
    let snapshot = snapshot?;
    let feed = super::feed();
    let plan = feed.plan.as_ref()?;
    if feed.subsession != snapshot.identity.subsession {
        return None;
    }
    let now = Utc::now();
    if !super::session::Context::from_snapshot(snapshot).is_some_and(|context| context.matches(plan, now)) {
        return None;
    }
    let mut handover = evaluate(plan, now, snapshot, synced, reserve_laps)?;
    handover.stale = feed.stale();
    if handover.stale {
        "PLAN STALE · verify handover with crew".clone_into(&mut handover.detail);
    }
    Some(handover)
}

pub fn matches_plan(plan: &Plan, snapshot: &TelemetrySnapshot) -> bool {
    plan_car(plan, snapshot).is_some()
}

pub fn plan_car<'a>(plan: &Plan, snapshot: &'a TelemetrySnapshot) -> Option<&'a CarSnapshot> {
    if !snapshot.relative_meta.session_kind.is_race()
        || !snapshot.relative_meta.racing_under_way
        || snapshot.course_flag == CourseFlag::Checkered
        || snapshot.identity.track_id != Some(plan.planning.track.iracing_id)
    {
        return None;
    }
    let matches = |car: &&CarSnapshot| {
        let team = car.team_id.or_else(|| car.is_focus.then_some(snapshot.identity.team_id).flatten());
        let registrable = match plan.planning.kind {
            super::Registration::Team => team,
            super::Registration::Individual => car.cust_id,
        };
        registrable == Some(plan.planning.registrable.iracing_id) && car.car_screen_name.as_ref() == plan.planning.car
    };
    let focus = snapshot.relative.get(snapshot.focus_index).filter(matches);
    focus.or_else(|| (snapshot.seat != Seat::Driving).then(|| snapshot.relative.iter().find(matches)).flatten())
}

/// Observed pit-exit age lets a delayed double stint stay attached to its original
/// assignment. Large/ambiguous deviations fall back to the labelled schedule.
pub fn active_index(plan: &Plan, now: DateTime<Utc>, snapshot: Option<&TelemetrySnapshot>) -> (Option<usize>, bool) {
    if let Some(snapshot) = snapshot.filter(|snapshot| matches_plan(plan, snapshot))
        && let Some(car) = plan_car(plan, snapshot)
        && car.cust_id.is_some()
        && let Some(entry) = snapshot.standings.iter().find(|entry| entry.car_idx == car.car_idx)
        && matches!(entry.stint_age, StintAge::Observed(_))
        && entry.current_stint_secs.is_finite()
        && entry.current_stint_secs >= 0.0
    {
        let candidate = plan
            .strategy
            .stints
            .iter()
            .enumerate()
            .filter(|(_, stint)| stint.driver.as_ref().map(|driver| driver.iracing_id) == car.cust_id)
            .map(|(index, stint)| (index, stint, (seconds(now - stint.start_at) - entry.current_stint_secs).abs()))
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((index, stint, distance)) = candidate {
            let tolerance = (seconds(stint.end_at - stint.start_at) / 2.0).min(1800.0);
            if distance <= tolerance {
                return (Some(index), true);
            }
        }
    }
    (plan.preview_index(now), false)
}

fn pace(plan: &Plan, index: usize, snapshot: &TelemetrySnapshot) -> Option<f64> {
    let car = plan_car(plan, snapshot)?;
    let recent: Vec<f64> = car
        .recent_laps
        .iter()
        .flatten()
        .copied()
        .map(f64::from)
        .filter(|seconds| seconds.is_finite() && *seconds > 10.0 && *seconds < 1800.0)
        .collect();
    if !recent.is_empty() {
        return Some(recent.iter().sum::<f64>() / f64::from(u8::try_from(recent.len()).ok()?));
    }
    let driver = plan.strategy.drivers.iter().find(|driver| Some(driver.iracing_id) == car.cust_id)?;
    let milliseconds = match plan.strategy.stints[index].setup.as_deref() {
        Some("dry") => driver.average_lap_time_dry,
        Some("wet") => driver.average_lap_time_wet,
        _ => None,
    }?;
    (milliseconds.is_finite() && (10_000.0..1_800_000.0).contains(&milliseconds)).then_some(milliseconds / 1000.0)
}

#[expect(clippy::cast_precision_loss, reason = "differences within chrono's DateTime range are below 2^53 seconds")]
fn seconds(delta: chrono::Duration) -> f64 {
    delta.num_seconds() as f64
}

fn live_fuel(
    plan: &Plan,
    snapshot: &TelemetrySnapshot,
    synced: Option<&SyncedCar>,
) -> Option<(f32, f32, &'static str)> {
    let car = plan_car(plan, snapshot)?;
    if let Some(synced) = synced.filter(|synced| synced.car_idx == Some(car.car_idx)) {
        synced.burn_per_lap.map(|burn| (synced.fuel_litres, burn, "Team sync + plan"))
    } else if snapshot.seat == Seat::Driving && snapshot.pit_service.fuel_reading_valid {
        snapshot
            .pit_service
            .fuel_per_lap_litres
            .map(|burn| (snapshot.pit_service.fuel_level_litres, burn, "Live fuel + plan"))
    } else {
        None
    }
}

fn forecast_start(
    stints: &[super::Stint],
    now: DateTime<Utc>,
    laps: Option<f64>,
    pace: Option<f64>,
) -> Option<DateTime<Utc>> {
    let (laps, pace) = laps.zip(pace)?;
    let service = stints
        .windows(2)
        .try_fold(0_i64, |total, pair| total.checked_add((pair[1].start_at - pair[0].end_at).num_seconds().max(0)))?;
    let driving = std::time::Duration::try_from_secs_f64((laps * pace).ceil()).ok()?.as_secs();
    let remaining = i64::try_from(driving).ok()?.checked_add(service)?;
    now.checked_add_signed(chrono::Duration::try_seconds(remaining)?)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite rounded lap count is clamped to 0..999 before conversion"
)]
fn display_laps(laps: f64) -> Option<u32> {
    laps.is_finite().then(|| laps.ceil().clamp(0.0, 999.0) as u32)
}

fn key_for(plan: &Plan, stint: &super::Stint) -> Option<HandoverKey> {
    Some(HandoverKey {
        planning_id: plan.planning.id,
        strategy_id: plan.strategy.id,
        stint_number: stint.number,
        driver_id: stint.driver.as_ref()?.iracing_id,
        starts_at: stint.start_at.timestamp(),
    })
}

fn append_drift(detail: &mut String, drift: Option<f64>) {
    if let Some(drift) = drift.filter(|seconds| seconds.abs() >= 120.0) {
        let _ = write!(
            detail,
            " · stint start {}m {}",
            (drift.abs() / 60.0).round(),
            if drift < 0.0 { "late" } else { "early" }
        );
    }
}

pub fn evaluate(
    plan: &Plan,
    now: DateTime<Utc>,
    snapshot: &TelemetrySnapshot,
    synced: Option<&SyncedCar>,
    reserve_laps: f32,
) -> Option<Handover> {
    if !matches_plan(plan, snapshot) {
        return None;
    }
    let stints = &plan.strategy.stints;
    if now < stints.first()?.start_at - chrono::Duration::minutes(30)
        || now > stints.last()?.end_at + chrono::Duration::minutes(30)
    {
        return None;
    }
    let (index, observed) = active_index(plan, now, Some(snapshot));
    let index = index?;
    let current = &stints[index];
    let car = plan_car(plan, snapshot)?;
    // Do not invent a handover when an unobserved driver change already disagrees
    // with the schedule. The Stints page explicitly surfaces that discrepancy.
    let current_driver = current.driver.as_ref()?.iracing_id;
    if car.cust_id != Some(current_driver) {
        return None;
    }
    let next_index = (index + 1..stints.len())
        .find(|i| stints[*i].driver.as_ref().map(|driver| driver.iracing_id) != Some(current_driver))?;
    let next = &stints[next_index];
    let driver = next.driver.as_ref()?;
    let last = &stints[next_index - 1];
    let lap_time = pace(plan, index, snapshot);
    let scheduled_seconds = seconds(last.end_at - now).max(0.0);
    let scheduled_laps = lap_time.map(|pace| scheduled_seconds / pace);
    let mut laps = scheduled_laps;
    let mut basis = "Plan estimate";
    let mut detail = "Based on scheduled handover; no live fuel confirmation".to_owned();
    let mut drift = None;
    let entry = snapshot.standings.iter().find(|entry| entry.car_idx == car.car_idx);
    let mut observed_remaining = None;
    if observed
        && let Some(entry) = entry
        && let StintAge::Observed(completed) = entry.stint_age
    {
        let fraction = snapshot
            .endurance
            .lap_driven_pct
            .filter(|_| car.is_focus)
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .unwrap_or(0.0);
        observed_remaining = current
            .estimated_laps
            .map(|target| (f64::from(target) - f64::from(completed) - f64::from(fraction)).max(0.0));
        let expected_age = seconds(now - current.start_at);
        drift = Some(entry.current_stint_secs - expected_age);
    }
    let fuel = live_fuel(plan, snapshot, synced);
    let future_laps: Option<f64> =
        stints[index + 1..next_index].iter().map(|stint| stint.estimated_laps.map(f64::from)).sum();
    if let Some((fuel, burn, source)) = fuel.filter(|(fuel, burn, _)| fuel.is_finite() && *fuel >= 0.0 && burn.is_finite() && *burn > 0.0)
        && let Some(future_laps) = future_laps
        // During repeated stints, fuel cannot locate us in the rotation without
        // an observed boundary. Keep the schedule instead of guessing a stint number.
        && (observed || next_index == index + 1)
    {
        let reserve = if reserve_laps.is_finite() { reserve_laps.max(0.0) } else { 1.0 };
        let range = (f64::from(fuel) / f64::from(burn) - f64::from(reserve)).max(0.0);
        let planned_remaining =
            observed_remaining.or_else(|| lap_time.map(|pace| seconds(current.end_at - now).max(0.0) / pace));
        if let Some(planned_remaining) = planned_remaining {
            laps = Some(planned_remaining.min(range) + future_laps);
            basis = source;
            detail = if (range - planned_remaining).abs() <= 2.0 {
                format!("Plan and live fuel agree within 2 laps · {reserve:.1} lap reserve")
            } else if range < planned_remaining {
                format!("Fuel limit ~{range:.0} laps; plan ~{planned_remaining:.0} · check earlier stop")
            } else {
                format!("Planned stop before fuel limit (~{range:.0} laps of usable fuel)")
            };
        }
    }
    let in_pits = matches!(
        car.track_location,
        iracing_telem::flags::TrackLocation::ApproachingPits | iracing_telem::flags::TrackLocation::InPitStall
    ) && next_index == index + 1;
    if in_pits {
        laps = Some(0.0);
        "Team car is in the pits · confirm driver change".clone_into(&mut detail);
    }
    append_drift(&mut detail, drift);
    let due = in_pits
        || laps.is_some_and(|laps| laps <= ALERT_LAPS)
        || scheduled_laps.is_some_and(|laps| laps <= ALERT_LAPS)
        || (lap_time.is_none() && scheduled_seconds <= 180.0);
    let live_forecast = basis != "Plan estimate";
    let estimated_start =
        if live_forecast { forecast_start(&stints[index..=next_index], now, laps, lap_time) } else { None };
    let shift_seconds = estimated_start.map(|start| (start - next.start_at).num_seconds());
    Some(Handover {
        key: key_for(plan, next)?,
        driver: driver.name.clone(),
        laps: laps.and_then(display_laps),
        due,
        incoming: snapshot.identity.player_cust_id == Some(driver.iracing_id) && snapshot.seat != Seat::Driving,
        basis,
        detail,
        stale: false,
        in_pits,
        estimated_start,
        shift_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario() -> (Plan, DateTime<Utc>, TelemetrySnapshot, SyncedCar) {
        let now = Utc::now();
        let plan = super::super::example_plan(now - chrono::Duration::minutes(114));
        let mut snapshot = crate::demo::snapshot();
        snapshot.identity.track_id = Some(341);
        snapshot.identity.team_id = Some(1);
        snapshot.identity.player_cust_id = Some(2);
        snapshot.seat = Seat::Spectating("Alex Morgan".into());
        snapshot.relative_meta.racing_under_way = true;
        snapshot.course_flag = CourseFlag::Green;
        snapshot.endurance.lap_driven_pct = Some(0.0);
        let car = &mut snapshot.relative[snapshot.focus_index];
        car.cust_id = Some(1);
        car.driver_name = "Alex Morgan".into();
        car.car_screen_name = "Ferrari 296 GT3".into();
        car.recent_laps = [Some(120.0); 3];
        car.track_location = iracing_telem::flags::TrackLocation::OnTrack;
        let car_idx = car.car_idx;
        let entry = snapshot.standings.iter_mut().find(|entry| entry.car_idx == car_idx).unwrap();
        entry.current_stint_secs = 54.0 * 60.0;
        entry.stint_age = StintAge::Observed(26);
        let synced = SyncedCar {
            car_idx: Some(car_idx),
            driver: Some("Alex Morgan".to_owned()),
            fuel_litres: 14.0,
            burn_per_lap: Some(3.5),
            service_fuel_litres: None,
            tyres_armed: [false; 4],
            tyre_pressures_kpa: [0.0; 4],
            tyres: None,
        };
        (plan, now, snapshot, synced)
    }

    #[test]
    fn spectator_camera_can_follow_another_car_without_changing_handover() {
        let (plan, now, mut snapshot, synced) = scenario();
        let team_index = snapshot.focus_index;
        snapshot.relative[team_index].team_id = Some(1);
        snapshot.relative[team_index].is_focus = false;
        snapshot.focus_index = (team_index + 1) % snapshot.relative.len();
        snapshot.relative[snapshot.focus_index].is_focus = true;
        snapshot.identity.team_id = Some(999);
        snapshot.endurance.lap_driven_pct = Some(0.9);
        snapshot.pit_service.fuel_per_lap_litres = Some(99.0);
        let handover = evaluate(&plan, now, &snapshot, Some(&synced), 1.0).unwrap();
        assert!(handover.incoming && handover.due);
        assert_eq!(handover.laps, Some(3));
        assert_eq!(handover.basis, "Team sync + plan");
        snapshot.seat = Seat::Driving;
        assert!(evaluate(&plan, now, &snapshot, Some(&synced), 1.0).is_none());
    }

    #[test]
    fn incoming_driver_gets_three_laps_supported_by_synced_fuel() {
        let (plan, now, snapshot, synced) = scenario();
        let handover = evaluate(&plan, now, &snapshot, Some(&synced), 1.0).unwrap();
        assert_eq!(handover.laps, Some(3));
        assert!(handover.due && handover.incoming);
        assert_eq!(handover.key.stint_number, 3);
        assert_eq!(handover.basis, "Team sync + plan");
        assert!(handover.detail.contains("agree"));
        assert!(handover.estimated_start.is_some());
    }

    #[test]
    fn earlier_fuel_limit_warns_without_claiming_a_confirmed_stop() {
        let (plan, now, snapshot, mut synced) = scenario();
        synced.fuel_litres = 7.0;
        let handover = evaluate(&plan, now, &snapshot, Some(&synced), 1.0).unwrap();
        assert_eq!(handover.laps, Some(1));
        assert!(handover.due);
        assert!(handover.basis.contains("plan"));
    }

    #[test]
    fn a_delayed_double_stint_does_not_advance_by_wall_clock_alone() {
        let (plan, now, snapshot, synced) = scenario();
        let later = now + chrono::Duration::minutes(10);
        assert_eq!(plan.preview_index(later), Some(2));
        assert_eq!(active_index(&plan, later, Some(&snapshot)), (Some(1), true));
        let handover = evaluate(&plan, later, &snapshot, Some(&synced), 1.0).unwrap();
        assert!(handover.incoming && handover.due);
        assert!(handover.detail.contains("late"));
        assert_eq!(handover.key.stint_number, 3);
    }

    #[test]
    fn another_same_driver_stint_keeps_handover_warning_quiet() {
        let (mut plan, now, snapshot, synced) = scenario();
        for stint in &mut plan.strategy.stints {
            stint.start_at += chrono::Duration::minutes(60);
            stint.end_at += chrono::Duration::minutes(60);
        }
        let handover = evaluate(&plan, now, &snapshot, Some(&synced), 1.0).unwrap();
        assert_eq!(handover.key.stint_number, 3);
        assert_eq!(handover.laps, Some(32));
        assert!(!handover.due);
    }

    #[test]
    fn missing_sync_falls_back_to_plan_and_never_uses_spectator_tank() {
        let (plan, now, mut snapshot, mut synced) = scenario();
        snapshot.pit_service.fuel_level_litres = 0.0;
        snapshot.pit_service.fuel_per_lap_litres = Some(99.0);
        let fallback = evaluate(&plan, now, &snapshot, None, 1.0).unwrap();
        assert_eq!(fallback.basis, "Plan estimate");
        assert_eq!(fallback.laps, Some(2));
        synced.car_idx = Some(-99);
        let wrong_car = evaluate(&plan, now, &snapshot, Some(&synced), 1.0).unwrap();
        assert_eq!(wrong_car.basis, "Plan estimate");
    }

    #[test]
    fn wrong_race_and_finish_never_trigger_a_personal_alert() {
        let (plan, now, mut snapshot, synced) = scenario();
        snapshot.identity.track_id = Some(99);
        assert!(evaluate(&plan, now, &snapshot, Some(&synced), 1.0).is_none());
        snapshot.identity.track_id = Some(341);
        snapshot.course_flag = CourseFlag::Checkered;
        assert!(evaluate(&plan, now, &snapshot, Some(&synced), 1.0).is_none());
    }

    #[test]
    fn the_seated_driver_sees_handover_but_not_an_incoming_prompt() {
        let (plan, now, mut snapshot, synced) = scenario();
        snapshot.identity.player_cust_id = Some(1);
        snapshot.seat = Seat::Driving;
        let handover = evaluate(&plan, now, &snapshot, Some(&synced), 1.0).unwrap();
        assert!(!handover.incoming);
        assert!(handover.due);
    }
}
