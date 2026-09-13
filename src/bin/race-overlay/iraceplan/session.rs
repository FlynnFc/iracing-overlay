//! Session demand controls HTTP polling independently of tabs and repaint frequency.

use super::Plan;
use crate::telemetry::snapshot::{Seat, TelemetrySnapshot};
use chrono::{DateTime, Utc};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    team_id: Option<u32>,
    driver_id: Option<u32>,
    car: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Context {
    pub key: u64,
    track_id: u32,
    entries: Vec<Entry>,
}

impl Context {
    pub fn from_snapshot(snapshot: &TelemetrySnapshot) -> Option<Self> {
        let key = snapshot.identity.subsession.filter(|id| *id > 0)?;
        let track_id = snapshot.identity.track_id.filter(|id| *id > 0)?;
        let entries = snapshot
            .relative
            .iter()
            .filter(|car| snapshot.seat != Seat::Driving || car.is_focus)
            .filter_map(|car| {
                let team_id = car
                    .team_id
                    .or_else(|| car.is_focus.then_some(snapshot.identity.team_id).flatten())
                    .filter(|id| *id > 0);
                let driver_id = car.cust_id.filter(|id| *id > 0);
                (!car.car_screen_name.is_empty() && (team_id.is_some() || driver_id.is_some())).then(|| Entry {
                    team_id,
                    driver_id,
                    car: Arc::clone(&car.car_screen_name),
                })
            })
            .collect::<Vec<_>>();
        (!entries.is_empty()).then_some(Self { key, track_id, entries })
    }

    pub fn includes_car(&self, car: &str) -> bool {
        self.entries.iter().any(|entry| entry.car.as_ref() == car)
    }

    pub fn matches(&self, plan: &Plan, now: DateTime<Utc>) -> bool {
        self.track_id == plan.planning.track.iracing_id
            && self.entries.iter().any(|entry| {
                let registrable = match plan.planning.kind {
                    super::Registration::Team => entry.team_id,
                    super::Registration::Individual => entry.driver_id,
                };
                registrable == Some(plan.planning.registrable.iracing_id) && entry.car.as_ref() == plan.planning.car
            })
            && plan.in_event_window(now)
    }
}

struct Observation {
    context: Option<Context>,
    seen_at: Instant,
}

static SESSION: OnceLock<RwLock<Observation>> = OnceLock::new();

pub(super) fn observe(snapshot: Option<&TelemetrySnapshot>) {
    let context = snapshot.and_then(Context::from_snapshot);
    let now = Instant::now();
    let state = SESSION.get_or_init(|| RwLock::new(Observation { context: None, seen_at: now }));
    if let Ok(mut state) = state.write() {
        *state = Observation { context, seen_at: now };
    }
}

pub(super) fn current() -> Option<Context> {
    let state = SESSION.get()?.read().ok()?;
    (state.seen_at.elapsed() < Duration::from_secs(5)).then(|| state.context.clone()).flatten()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Discover,
    Update,
}

#[derive(Default)]
pub(super) struct Refresh {
    key: Option<u64>,
    checked: bool,
    next: Option<Instant>,
}

impl Refresh {
    /// Every join gets one schedule lookup. Only a confirmed plan gets a timer.
    pub fn due(&mut self, key: Option<u64>, matched: bool, now: Instant) -> Option<Action> {
        if self.key != key {
            *self = Self { key, ..Self::default() };
        }
        key?;
        if !self.checked {
            self.checked = true;
            return Some(Action::Discover);
        }
        if !matched {
            self.next = None;
            return None;
        }
        self.next.is_none_or(|at| now >= at).then_some(Action::Update)
    }

    pub fn completed(&mut self, matched: bool, now: Instant) {
        self.next = matched.then(|| now + super::POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_unmatched_sessions_have_no_refresh_timer() {
        let now = Instant::now();
        let mut refresh = Refresh::default();
        assert_eq!(refresh.due(None, false, now), None);
        assert_eq!(refresh.due(Some(1), false, now), Some(Action::Discover));
        refresh.completed(false, now);
        for seconds in [1, 30, 3600] {
            assert_eq!(refresh.due(Some(1), false, now + Duration::from_secs(seconds)), None);
        }
        assert_eq!(refresh.due(Some(2), false, now + Duration::from_secs(3601)), Some(Action::Discover));
    }

    #[test]
    fn polling_stops_on_a_mismatch_or_disconnect_and_resumes_on_a_matching_join() {
        let now = Instant::now();
        let mut refresh = Refresh::default();
        assert_eq!(refresh.due(Some(1), false, now), Some(Action::Discover));
        refresh.completed(true, now);
        assert_eq!(refresh.due(Some(1), true, now + Duration::from_secs(29)), None);
        assert_eq!(refresh.due(Some(1), true, now + Duration::from_secs(30)), Some(Action::Update));
        refresh.completed(true, now + Duration::from_secs(30));
        assert_eq!(refresh.due(Some(1), false, now + Duration::from_secs(60)), None);
        assert_eq!(refresh.due(None, false, now + Duration::from_secs(90)), None);
        assert_eq!(refresh.due(Some(2), false, now + Duration::from_secs(91)), Some(Action::Discover));
        refresh.completed(false, now + Duration::from_secs(91));
        assert_eq!(refresh.due(Some(2), false, now + Duration::from_secs(121)), None);
        assert_eq!(refresh.due(Some(3), false, now + Duration::from_secs(122)), Some(Action::Discover));
    }

    #[test]
    fn matching_uses_event_dates_and_allows_the_matching_pre_race_and_spectator_car() {
        let now = Utc::now();
        let plan = super::super::example_plan(now + chrono::Duration::minutes(20));
        let mut snapshot = crate::demo::snapshot();
        crate::iraceplan::handover::seed_demo(&mut snapshot);
        snapshot.relative_meta.racing_under_way = false;
        snapshot.relative_meta.session_kind = crate::telemetry::snapshot::SessionKind::Practice;
        let matched = |snapshot: &TelemetrySnapshot, at| {
            Context::from_snapshot(snapshot).is_some_and(|context| context.matches(&plan, at))
        };
        assert!(matched(&snapshot, now));
        assert!(!matched(&snapshot, now + chrono::Duration::days(7)));
        assert!(!matched(&snapshot, now - chrono::Duration::days(7)));
        snapshot.identity.track_id = Some(999);
        assert!(!matched(&snapshot, now));
        snapshot.identity.track_id = Some(341);
        snapshot.identity.team_id = Some(999);
        assert!(!matched(&snapshot, now));
        snapshot.identity.team_id = Some(1);
        let team = snapshot.focus_index;
        snapshot.relative[team].car_screen_name = "Wrong car".into();
        assert!(!matched(&snapshot, now));
        snapshot.relative[team].car_screen_name = "Ferrari 296 GT3".into();
        snapshot.relative[team].team_id = Some(1);
        snapshot.relative[team].is_focus = false;
        snapshot.focus_index = (team + 1) % snapshot.relative.len();
        snapshot.relative[snapshot.focus_index].is_focus = true;
        snapshot.identity.team_id = Some(999);
        assert!(matched(&snapshot, now));
        snapshot.seat = Seat::Driving;
        assert!(!matched(&snapshot, now));
        snapshot.identity.subsession = None;
        assert!(!matched(&snapshot, now));
    }

    #[test]
    fn individual_plans_match_driver_identity_instead_of_team_identity() {
        let now = Utc::now();
        let mut plan = super::super::example_plan(now);
        plan.planning.kind = super::super::Registration::Individual;
        let mut snapshot = crate::demo::snapshot();
        crate::iraceplan::handover::seed_demo(&mut snapshot);
        snapshot.seat = Seat::Driving;
        snapshot.identity.team_id = None;
        let context = Context::from_snapshot(&snapshot).unwrap();
        assert!(context.matches(&plan, now));
        plan.planning.registrable.iracing_id = 999;
        assert!(!context.matches(&plan, now));
    }
}
