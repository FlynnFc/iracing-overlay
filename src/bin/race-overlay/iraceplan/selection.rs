//! Explicit race/strategy selection by name. Catalog requests share the existing
//! background worker, so opening a dropdown never blocks the overlay.

use super::{Config, Identity};
use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Deserialize)]
struct Session {
    name: String,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
struct Race {
    id: u64,
    session: Session,
    registrable: Identity,
    car: String,
    #[serde(rename = "type")]
    kind: String,
}

impl Race {
    fn label(&self) -> String {
        format!(
            "{} · {} · {} · {}",
            self.session.start_at.with_timezone(&Local).format("%d %b %H:%M"),
            self.session.name,
            self.registrable.name,
            self.car
        )
    }
}

#[derive(Debug, Clone, Default)]
struct Catalog {
    races: Vec<Race>,
    strategies: Vec<(u64, String)>,
    planning_id: u64,
    pending: Option<u64>,
    loading: bool,
    status: String,
}

static CATALOG: OnceLock<Mutex<Catalog>> = OnceLock::new();

fn request(planning_id: u64) {
    if let Ok(mut catalog) = CATALOG.get_or_init(Mutex::default).lock() {
        catalog.pending = Some(planning_id);
        catalog.loading = true;
        "Loading…".clone_into(&mut catalog.status);
    }
    super::start();
}

pub(super) fn poll(client: &reqwest::blocking::Client) {
    let pending = CATALOG.get().and_then(|catalog| catalog.lock().ok()?.pending.take());
    let Some(planning_id) = pending else { return };
    let result = if planning_id == 0 { races(client) } else { strategies(client, planning_id) };
    if let Ok(mut catalog) = CATALOG.get_or_init(Mutex::default).lock() {
        catalog.loading = false;
        match result {
            Ok(result) => {
                if planning_id == 0 {
                    catalog.races = result.races;
                } else {
                    catalog.strategies = result.strategies;
                    catalog.planning_id = planning_id;
                }
                "Choose a race and strategy, then save.".clone_into(&mut catalog.status);
            }
            Err(message) => message.clone_into(&mut catalog.status),
        }
    }
}

fn races(client: &reqwest::blocking::Client) -> Result<Catalog, &'static str> {
    #[derive(Deserialize)]
    struct Response {
        plannings: Vec<Race>,
    }
    let response: Response =
        serde_json::from_slice(&super::read_api(client, "plannings")?).map_err(|_error| "Cannot read race list")?;
    let mut races: Vec<_> = response.plannings.into_iter().filter(|race| race.kind == "team").collect();
    let now = Utc::now();
    races.sort_by_key(|race| {
        (
            race.session.end_at < now,
            if race.session.end_at < now {
                -race.session.start_at.timestamp()
            } else {
                race.session.start_at.timestamp()
            },
        )
    });
    Ok(Catalog { races, ..Catalog::default() })
}

fn strategies(client: &reqwest::blocking::Client, planning_id: u64) -> Result<Catalog, &'static str> {
    let response: super::Response =
        serde_json::from_slice(&super::read_api(client, &format!("plannings/{planning_id}"))?)
            .map_err(|_error| "Cannot read strategies")?;
    Ok(Catalog {
        strategies: response.planning.strategies.into_iter().map(|strategy| (strategy.id, strategy.name)).collect(),
        ..Catalog::default()
    })
}

pub(super) fn settings(ui: &mut egui::Ui, config: &mut Config) {
    let catalog = CATALOG.get_or_init(Mutex::default).lock().map(|catalog| catalog.clone()).unwrap_or_default();
    if ui.add_enabled(!catalog.loading, egui::Button::new("Find my team races")).clicked() {
        request(0);
    }
    if !catalog.status.is_empty() {
        ui.small(&catalog.status);
    }
    let current = super::feed();
    let selected = catalog
        .races
        .iter()
        .find(|race| race.id == config.planning_id)
        .map(Race::label)
        .or_else(|| {
            current
                .plan
                .as_ref()
                .filter(|plan| plan.planning.id == config.planning_id)
                .map(|plan| format!("{} · {}", plan.planning.registrable.name, plan.planning.track.name))
        })
        .unwrap_or_else(|| "Choose race".to_owned());
    let old = config.planning_id;
    egui::ComboBox::from_id_salt("iraceplan-race").selected_text(selected).width(360.0).show_ui(ui, |ui| {
        for race in &catalog.races {
            ui.selectable_value(&mut config.planning_id, race.id, race.label());
        }
    });
    if old != config.planning_id {
        config.strategy_id = 0;
        request(config.planning_id);
    }
    let strategies = if catalog.planning_id == config.planning_id {
        catalog.strategies
    } else {
        current.plan.as_ref().filter(|plan| plan.planning.id == config.planning_id).map_or_else(Vec::new, |plan| {
            plan.planning.strategies.iter().map(|strategy| (strategy.id, strategy.name.clone())).collect()
        })
    };
    if strategies.is_empty()
        && config.planning_id != 0
        && ui.add_enabled(!catalog.loading, egui::Button::new("Load strategies for this race")).clicked()
    {
        request(config.planning_id);
    }
    let selected = strategies
        .iter()
        .find(|(id, _)| *id == config.strategy_id)
        .map_or("Choose strategy", |(_, name)| name.as_str());
    egui::ComboBox::from_id_salt("iraceplan-strategy").selected_text(selected).show_ui(ui, |ui| {
        for (id, name) in &strategies {
            ui.selectable_value(&mut config.strategy_id, *id, name);
        }
    });
    ui.collapsing("Manual selection", |ui| {
        ui.horizontal(|ui| {
            ui.label("Planning ID");
            ui.add(egui::DragValue::new(&mut config.planning_id).range(0..=u64::MAX));
            ui.label("Strategy ID");
            ui.add(egui::DragValue::new(&mut config.strategy_id).range(0..=u64::MAX));
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "uses the locally configured iRacePlan credential for read-only catalog requests"]
    fn configured_catalog_lists_team_races_and_strategies() {
        let config = super::super::Config::load().unwrap();
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let races = super::races(&client).expect("team race catalog");
        assert!(races.races.iter().any(|race| race.id == config.planning_id));
        let strategies = super::strategies(&client, config.planning_id).expect("strategy catalog");
        assert!(strategies.strategies.iter().any(|(id, _)| *id == config.strategy_id));
    }
}
