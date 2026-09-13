//! Finds the website's scheduled stint plan once when an iRacing room is joined.
//! The schedule endpoint avoids the 100-entry cap on the planning history list.

use super::{EventSession, Plan, StrategyChoice, session::Context};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    schedule: Schedule,
}

#[derive(Deserialize)]
struct Schedule {
    races: Vec<Race>,
}

#[derive(Deserialize)]
struct Race {
    id: u64,
    #[serde(flatten)]
    session: EventSession,
    car: String,
    stint_plan: Option<StintPlan>,
}

#[derive(Deserialize)]
struct StintPlan {
    name: String,
}

pub(super) fn discover(
    context: &Context,
    now: DateTime<Utc>,
    mut read: impl FnMut(&str) -> Result<Vec<u8>, &'static str>,
) -> Result<Option<Plan>, &'static str> {
    // Include ongoing endurance races that started on an earlier day.
    let from = (now - chrono::Duration::days(7)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let to = (now + chrono::Duration::minutes(30)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let body = read(&format!("schedule?from={from}&to={to}"))?;
    let response: Response = serde_json::from_slice(&body).map_err(|_error| "Cannot read race schedule")?;
    let mut matched = None;
    let mut invalid_plan = None;
    for race in response.schedule.races {
        let Some(stints) = race.stint_plan else { continue };
        if !race.session.contains(now) || !context.includes_car(&race.car) {
            continue;
        }
        let body = read(&format!("plannings/{}", race.id))?;
        let plan = match Plan::parse(&body, race.id, StrategyChoice::Scheduled(&stints.name)) {
            Ok(plan) => plan,
            Err(message) => {
                invalid_plan = Some(message);
                continue;
            }
        };
        // Names are presentation data. The detailed record confirms the track
        // configuration and team/driver IDs before a schedule can appear.
        if context.matches(&plan, now) {
            if matched.is_some() {
                return Err("Multiple plans match this session; resolve duplicates in iRacePlan");
            }
            matched = Some(plan);
        }
    }
    if matched.is_none()
        && let Some(message) = invalid_plan
    {
        return Err(message);
    }
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> (Context, DateTime<Utc>, Value, Value) {
        let now = DateTime::parse_from_rfc3339("2026-09-19T12:20:00Z").unwrap().with_timezone(&Utc);
        let mut snapshot = crate::demo::snapshot();
        crate::iraceplan::handover::seed_demo(&mut snapshot);
        snapshot.identity.team_id = Some(516213);
        let context = Context::from_snapshot(&snapshot).unwrap();
        let session = json!({"name":"Britcar 24 Presented by Cosworth", "start_at":"2026-09-19T12:00:00Z", "end_at":"2026-09-20T12:48:00Z"});
        let schedule = json!({"schedule":{"races":[{
            "id":27505306,"name":session["name"],"start_at":session["start_at"],"end_at":session["end_at"],
            "car":"Ferrari 296 GT3","stint_plan":{"name":"Prelim"}
        }]}});
        let planning = json!({"planning":{
            "id":27505306,"type":"team","session":session,"car":"Ferrari 296 GT3",
            "registrable":{"iracing_id":516213,"name":"Winton Wookies"},"track":{"iracing_id":341,"name":"Silverstone"},
            "strategies":[
                {"id":999,"name":"Alternative","status":"up-to-date","stints":[]},
                {"id":29015,"name":"Prelim","status":"up-to-date","stints":[
                    {"number":1,"start_at":"2026-09-19T12:45:00Z","end_at":"2026-09-19T13:43:00Z","driver":null}
                ]}
            ]
        }});
        (context, now, schedule, planning)
    }

    #[test]
    fn joining_finds_britcar_and_the_websites_strategy_without_configured_ids() {
        let (context, now, schedule, planning) = fixture();
        let mut paths = Vec::new();
        let result = discover(&context, now, |path| {
            paths.push(path.to_owned());
            Ok(serde_json::to_vec(if path.starts_with("schedule?") { &schedule } else { &planning }).unwrap())
        })
        .unwrap()
        .unwrap();
        assert_eq!(result.planning.id, 27505306);
        assert_eq!(result.strategy.id, 29015);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].contains("from=2026-09-12T12:20:00Z"));
        assert_eq!(paths[1], "plannings/27505306");
    }

    #[test]
    fn no_schedule_wrong_date_wrong_car_and_no_stints_need_only_the_join_lookup() {
        let (context, now, schedule, _) = fixture();
        for scenario in 0..4 {
            let mut schedule = schedule.clone();
            match scenario {
                0 => schedule["schedule"]["races"] = json!([]),
                1 => schedule["schedule"]["races"][0]["end_at"] = json!("2026-09-18T12:00:00Z"),
                2 => schedule["schedule"]["races"][0]["car"] = json!("Wrong car"),
                _ => schedule["schedule"]["races"][0]["stint_plan"] = Value::Null,
            }
            let mut calls = 0;
            assert!(
                discover(&context, now, |path| {
                    calls += 1;
                    assert!(path.starts_with("schedule?"));
                    Ok(serde_json::to_vec(&schedule).unwrap())
                })
                .unwrap()
                .is_none()
            );
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn the_event_name_does_not_override_wrong_team_or_track_and_duplicates_are_rejected() {
        let (context, now, schedule, planning) = fixture();
        for wrong in ["registrable", "track"] {
            let mut planning = planning.clone();
            planning["planning"][wrong]["iracing_id"] = json!(999);
            assert!(
                discover(&context, now, |path| {
                    Ok(serde_json::to_vec(if path.starts_with("schedule?") { &schedule } else { &planning }).unwrap())
                })
                .unwrap()
                .is_none()
            );
        }
        let mut schedule = schedule;
        let duplicate = schedule["schedule"]["races"][0].clone();
        schedule["schedule"]["races"].as_array_mut().unwrap().push(duplicate);
        assert!(
            discover(&context, now, |path| {
                Ok(serde_json::to_vec(if path.starts_with("schedule?") { &schedule } else { &planning }).unwrap())
            })
            .is_err()
        );
    }

    #[test]
    #[ignore = "uses the saved iRacePlan key for one schedule read and matching planning reads"]
    fn configured_account_automatically_finds_britcar() {
        let (context, now, _, _) = fixture();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let plan = discover(&context, now, |path| super::super::read_api(&client, path)).unwrap().unwrap();
        assert_eq!(plan.planning.id, 27505306);
        assert_eq!(plan.strategy.name, "Prelim");
        assert!(plan.planning.session.unwrap().name.contains("Britcar"));
    }
}
