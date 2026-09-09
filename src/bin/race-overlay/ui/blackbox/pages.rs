// Rust guideline compliant 2026-02-16

//! The black box pages' contents: what rows each shows, and what a press on
//! one asks the sim to do.
//!
//! Pure functions over a snapshot, so every page's shape is testable without
//! a window or a session.

use std::collections::HashMap;

use iracing_telem::flags::TrackLocation;

use super::{Control, FuelGauge, PageLayout, Row, RowKind, Shape, SyncControls, Tile, TyreDirective, TyreReadout};
use crate::input::Action;
use crate::telemetry::pit::{
    Corner, PitRequest, PitService, corner_index, fuel_to_finish_litres, round_kpa, round_litres,
};
use crate::telemetry::snapshot::TelemetrySnapshot;

/// Litres added or removed per click of the rotary.
///
/// `PitCommand::Fuel` carries an `i16` of litres, so tenths cannot be sent
/// however they are displayed — a one-litre step is the finest this can
/// honestly offer, and pretending otherwise would show a decimal that never
/// reaches the sim.
const FUEL_STEP: i16 = 1;

/// Pressure is stepped a whole psi at a time, not a whole kPa.
///
/// The sim keeps tyre pressure in psi and publishes kPa as a conversion of it:
/// a live car reads `158.579` kPa on all four corners, which is 23.000 psi to
/// four figures, and `LFcoldPressure` from the garage reads the same. A one-kPa
/// command therefore lands inside the psi the corner is already on, comes back
/// from `PitSvLFP` as the value it started at, and — because the next step is
/// computed from that readback — sends the identical command again. The
/// broadcast climbed, the readback never moved, and the panel sat frozen on one
/// number however far the commands went.
///
/// Stepping by the sim's own increment means every click changes something the
/// sim can actually represent, and the panel follows it.
const PRESSURE_STEP_PSI: f32 = 1.0;

/// The conversion the sim's own kPa readings are produced by.
const KPA_PER_PSI: f32 = 6.894_757;

/// Fuel loads outside this are a mis-click, not an intent.
const MAX_FUEL_LITRES: i16 = 200;

/// Cold pressures outside this are a mis-click, not an intent.
const PRESSURE_RANGE_KPA: std::ops::RangeInclusive<i16> = 50..=250;

/// Turns a press on a row into the pit command it asks for.
///
/// Returns `None` for anything the sim won't accept — which includes every
/// press made while the driver isn't in the car, since iRacing silently
/// ignores those and acting as though it hadn't would put the panel out of
/// step with the sim.
#[must_use]
pub fn request_for(action: Action, control: Control, kind: &RowKind, service: &PitService) -> Option<PitRequest> {
    if !service.in_car {
        return None;
    }
    let step = match action {
        Action::Increment => 1_i16,
        Action::Decrement => -1_i16,
        Action::Toggle => 0,
        _ => return None,
    };

    match control {
        Control::Fuel => {
            if action == Action::Toggle {
                // Toggling fuel off is `ClearFuel`; toggling it on re-sends
                // the amount the sim already has, which is what arms the box.
                return Some(if service.fuel_armed {
                    PitRequest::ClearFuel
                } else {
                    PitRequest::SetFuel(round_litres(service.fuel_amount_litres))
                });
            }
            let current = round_litres(service.fuel_amount_litres);
            let next = current.saturating_add(step * FUEL_STEP).clamp(0, MAX_FUEL_LITRES);
            (next != current).then_some(PitRequest::SetFuel(next))
        }
        Control::Tyre(corner) => {
            // One control for the wheel, not two: a press arms or clears that
            // corner, a turn sets its pressure. Both are already distinct
            // actions, and splitting them across two cursor stops cost eight
            // clicks to walk the car on the way into the lane.
            if action != Action::Toggle {
                return step_pressure(corner, step, service);
            }
            let armed = matches!(kind, RowKind::Corner { armed, .. } if *armed);
            Some(PitRequest::SetTyre { corner, armed: !armed })
        }
        Control::AllTyres => {
            // Follows the row rather than the sim so the press does what the
            // tick the driver is looking at says it will: ticked clears all
            // four, anything less arms all four.
            let all_armed = matches!(kind, RowKind::Toggle { checked, .. } if *checked);
            (action == Action::Toggle).then_some(PitRequest::SetAllTyres(!all_armed))
        }
        Control::Tearoff => (action == Action::Toggle).then_some(PitRequest::SetTearoff(!service.tearoff_armed)),
        // Handled by the app: these are this app's own settings, reminders and
        // team-sync targets, not pit service, so they never become a broadcast
        // message.
        Control::AutoFuel | Control::BoxBox | Control::FuelTarget | Control::TyrePolicy => None,
        Control::FastRepair => {
            // Nothing to arm when there are none left; the row says so.
            if service.fast_repairs_available == 0 && !service.fast_repair_armed {
                return None;
            }
            (action == Action::Toggle).then_some(PitRequest::SetFastRepair(!service.fast_repair_armed))
        }
    }
}

/// One click of a corner's cold pressure, or `None` where it is already at the
/// end of what a mis-click could plausibly have meant.
///
/// Stepped a whole psi at a time — see [`PRESSURE_STEP_PSI`] for why a kPa
/// step left the panel frozen. The current pressure is snapped to whole psi
/// before the step, so a setup that starts on a fraction of one lands on the
/// sim's own increments from the first click rather than carrying the fraction
/// forward for the rest of the race.
///
/// Arming follows from setting a pressure, because iRacing's own per-corner
/// command carries both — see [`PitRequest::SetTyrePressure`].
fn step_pressure(corner: Corner, step: i16, service: &PitService) -> Option<PitRequest> {
    let current_kpa = round_kpa(service.tyre_pressure_kpa(corner));
    let psi = f32::from(current_kpa) / KPA_PER_PSI;
    let next_psi = psi.round() + f32::from(step) * PRESSURE_STEP_PSI;
    let next_kpa = round_kpa(next_psi * KPA_PER_PSI).clamp(*PRESSURE_RANGE_KPA.start(), *PRESSURE_RANGE_KPA.end());
    (next_kpa != current_kpa).then_some(PitRequest::SetTyrePressure { corner, kpa: next_kpa })
}

fn stepper(label: &str, value: String, control: Control) -> Row {
    Row { label: label.to_owned(), kind: RowKind::Stepper { value, control } }
}

fn toggle(label: &str, checked: bool, control: Control) -> Row {
    Row { label: label.to_owned(), kind: RowKind::Toggle { checked, control } }
}

fn reading(label: &str, value: String) -> Row {
    Row { label: label.to_owned(), kind: RowKind::Static { value } }
}

/// Wind below this is calm: no arrow, just the figure. Half a metre a second
/// rounds to a couple of km/h, which is nothing a car feels.
const CALM_MPS: f32 = 0.5;

/// Laps per click of the margin stepper.
pub const MARGIN_STEP_LAPS: f32 = 0.5;

/// Beyond this the margin stops being a safety net and starts being ballast.
pub const MAX_MARGIN_LAPS: f32 = 10.0;

/// `design mocks/Screenshot_16.jpg`.
///
/// Auto Fuel and Margin (Laps) are reimplemented rather than read: iRacing's
/// own versions are black box UI features with no SDK equivalent, so these
/// compute the load from measured consumption and arm it through the ordinary
/// fuel command. Everything else on the page is the sim's own state.
///
/// While Auto Fuel is on, the Add row is shown but not selectable — it is a
/// readout of what Auto Fuel has armed, and letting the cursor land on a
/// number that would be overwritten a frame later invites exactly one bug
/// report.
#[must_use]
pub fn fuel(
    snapshot: &TelemetrySnapshot,
    settings: &crate::config::BlackBoxConfig,
    auto_fuel_litres: Option<i16>,
    synced: Option<&crate::sync::store::SyncedCar>,
) -> PageLayout {
    // While spectating, the app writes the team car's synced fuel over this
    // sim's empty pit service before the page reads it (see
    // `app::synced_blackbox_snapshot`), so every value below comes from
    // `service` whichever seat this is. `synced` here only decides the tag and
    // which controls to show.
    let service = &snapshot.pit_service;

    // What Auto Fuel has decided on, in preference to what the sim has armed:
    // on track the command is deliberately held back until the pit entry, so
    // the sim's figure is a stop or a stint behind, and showing it would read
    // as Auto Fuel having quietly stopped working. Falls back to the sim's own
    // number whenever Auto Fuel has nothing of its own.
    let armed = round_litres(service.fuel_amount_litres);
    let adding = if settings.auto_fuel && synced.is_none() { auto_fuel_litres.unwrap_or(armed) } else { armed };
    let add = format!("{adding} L");

    // The load first: it is the number this page exists to set, and everything
    // under it is a consequence of it. While Auto Fuel is on it is a readout
    // instead — letting the cursor land on a figure that gets overwritten a
    // frame later invites exactly one bug report.
    //
    // A spectator gets live fuel controls too — the app routes their presses
    // over the wire to the driver's overlay (behind that driver's consent) —
    // but only the two the synced echo reflects: the load and its arm. Auto
    // Fuel is the driver's own, and tearoff / fast repair aren't in the echo.
    let controls = if let Some(car) = synced {
        let mut rows = Vec::new();
        if let Some(driver) = &car.driver {
            rows.push(reading("Via", format!("\u{25C9} {driver}")));
        }
        rows.push(stepper("Add", add, Control::Fuel));
        rows.push(toggle("Fuel", service.fuel_armed, Control::Fuel));
        rows
    } else {
        vec![
            if settings.auto_fuel { reading("Add", add) } else { stepper("Add", add, Control::Fuel) },
            toggle("Fuel", service.fuel_armed, Control::Fuel),
            toggle("Tearoff", service.tearoff_armed, Control::Tearoff),
            toggle("Fast Repair", service.fast_repair_armed, Control::FastRepair),
            toggle("Auto", settings.auto_fuel, Control::AutoFuel),
        ]
    };

    // Laps left in the tank, from what the car has actually been drinking.
    // Absent until a lap has been completed — a made-up figure is worse than a
    // missing one.
    let per_lap = service.fuel_per_lap_litres.filter(|litres| *litres > 0.0);
    let laps_remaining = snapshot.endurance.laps_remaining;
    // The same sum the armed load comes from — margin aside — so the FINISH
    // mark and the fill can never disagree about where the race ends.
    let to_finish_litres = fuel_to_finish_litres(laps_remaining, snapshot.endurance.lap_driven_pct, per_lap, 0.0);
    // While the rig is pumping, the sim holds the armed figure at the full
    // load as the tank rises to meet it; drawn as level-plus-load the hatch
    // slides right instead of being consumed. The latched end level pins the
    // far edge, and what is left to go in is the distance to it.
    let adding_litres =
        service.refuel_target_litres.map_or(f32::from(adding), |target| (target - service.fuel_level_litres).max(0.0));
    let after_the_stop = service.fuel_level_litres + adding_litres;

    PageLayout {
        controls,
        shape: Shape::Fuel {
            fast_repairs: fast_repair_note(service),
            // The margin rides on the AUTO plate — see `Control::AutoFuel`.
            // Not while spectating: Auto Fuel is the driver's, inert here.
            margin_laps: (synced.is_none() && settings.auto_fuel).then_some(settings.fuel_margin_laps),
            gauge: FuelGauge {
                in_tank_litres: service.fuel_level_litres,
                adding_litres,
                // A spectator's own sim publishes no tank for a car it isn't
                // driving, and the ledger carries no capacity, so the arc is
                // left off rather than drawn against a wrong full mark.
                capacity_litres: if synced.is_some() { None } else { service.tank_capacity_litres },
                to_finish_litres,
                laps_covered: per_lap.map(|litres| service.fuel_level_litres / litres),
                laps_after_stop: per_lap.filter(|_| adding_litres > 0.0).map(|litres| after_the_stop / litres),
                laps_remaining,
            },
        },
    }
}

/// The stop-skip search's bounds: half a lap of reserve (the box call's own
/// figure), savings up to 0.2 L/lap — a tenth or two, not lift-and-coast —
/// searched in hundredth steps.
const SKIP_RESERVE_LAPS: f32 = 0.5;
const SKIP_MAX_SAVE_LPL: f32 = 0.2;
const SKIP_SEARCH_STEP: f32 = 0.01;

fn fast_repair_note(service: &PitService) -> String {
    match service.fast_repairs_available {
        // iRacing's own sentinel for an unlimited allowance.
        255 => "unlimited".to_owned(),
        n => format!("{n} remaining"),
    }
}

/// `design mocks/Screenshot_17.jpg`, plus an all-four row of our own.
///
/// "All Tires" is first because it is the press a driver actually wants under
/// pressure: arm the set, or clear the set, without walking the cursor
/// through four corners on the way into the pit lane. It is a readout as much
/// as a control — ticked only when all four are, so a glance says whether the
/// stop is a full set or something less.
#[must_use]
pub fn tires(snapshot: &TelemetrySnapshot, bars: crate::config::TyreBars) -> PageLayout {
    let service = &snapshot.pit_service;
    // The set-wide control first, then across the front axle and across the
    // rear — the order a driver reads a car, and the order the grid draws.
    let mut controls = vec![toggle("All Four", service.all_tyres_armed(), Control::AllTyres)];
    controls.extend(Corner::ALL.map(|corner| Row {
        label: corner.short_name().to_owned(),
        kind: RowKind::Corner {
            armed: service.tyre_armed(corner),
            pressure_kpa: round_kpa(service.tyre_pressure_kpa(corner)),
            control: Control::Tyre(corner),
        },
    }));
    // What came off each wheel at the last stop, under the pressure that goes
    // on at the next. The carcass temperatures are latched when the car last
    // came to rest in its box — see `telemetry::session::TyreLatch` — and a
    // corner that reads all zeros has no stop behind it yet: the sim
    // publishes nothing there until the first one.
    let readouts = Corner::ALL.map(|corner| {
        let tyre = snapshot.tyres.corners[corner_index(corner)];
        let collected = tyre.temps_c.iter().any(|t| *t > 0.0);
        collected.then_some(TyreReadout { temps_c: tyre.temps_c, wear: tyre.wear, pressure_kpa: tyre.pressure_kpa })
    });
    PageLayout {
        controls,
        shape: Shape::Corners { compound: service.pending_tyre_compound, readouts: Box::new(readouts), bars, wear_threshold_pct: None },
    }
}

/// `design mocks/Screenshot_20.jpg`.
///
/// Every row is a reading: iRacing accepts no broadcast message for brake
/// bias, ABS, traction control, engine map or dash page, so these can be shown
/// but never set. Rows appear only for the controls this car actually
/// publishes, so a car without ABS simply has no ABS line.
#[must_use]
pub fn in_car(snapshot: &TelemetrySnapshot) -> PageLayout {
    let adjustments = &snapshot.adjustments;
    // Short names, because the value is the point and a tile has room for one
    // word. Only the controls this car actually publishes get a tile, so a car
    // without ABS simply has no ABS column.
    let tiles = [
        adjustments.brake_bias.map(|bias| Tile::split(format!("{bias:.1}%"), "BIAS", bias / 100.0)),
        adjustments.abs.map(|abs| Tile::new(format!("{abs:.0}"), "ABS")),
        adjustments.traction_control.map(|tc| Tile::new(format!("{tc:.0}"), "TC")),
        adjustments.throttle_shape.map(|map| Tile::new(format!("{map:.0}"), "MAP")),
        // The sim publishes the dash page as a number and never a name, so it
        // is called a page rather than shown as a bare figure.
        adjustments.dash_page.map(|page| Tile::new(format!("PAGE {page:.0}"), "DASH")),
    ];
    PageLayout { controls: Vec::new(), shape: Shape::Tiles(tiles.into_iter().flatten().collect()) }
}

/// `design mocks/Screenshot_21.jpg`, reduced to what exists.
///
/// The mockup's ten-slot forecast has no source: no telemetry variable and no
/// session-YAML section publishes one. These are the current conditions, and
/// the page says so rather than implying a forecast it cannot give.
#[must_use]
pub fn weather(snapshot: &TelemetrySnapshot) -> PageLayout {
    let weather = snapshot.weather;
    // Everything the sim actually publishes about the weather, and nothing it
    // does not: there is no forecast in either the telemetry or the session
    // YAML, so this page is the conditions now and says as much in its
    // subtitle. The rain slot is the live rain while it falls and the
    // session's declared chance while it is dry — see
    // `ui::weather::rain_reading`, which every weather surface shares.
    let rain = crate::ui::weather::rain_reading(&weather);
    let tiles = [
        Some(Tile::new(format!("{:.0}\u{b0}", weather.track_temp_c), "TRACK")),
        Some(Tile::new(format!("{:.0}\u{b0}", weather.air_temp_c), "AIR")),
        // Calm air has no direction worth drawing, so the arrow goes with it.
        Some(Tile::pointing(
            format!("{:.0}", weather.wind_speed_mps * 3.6),
            "KM/H",
            weather.wind_dir_relative_to_car_rad.filter(|_| weather.wind_speed_mps >= CALM_MPS),
        )),
        weather.fog.map(|fog| Tile::new(format!("{:.0}%", fog * 100.0), "FOG")),
        rain.map(|rain| Tile::new(rain.value, rain.label)),
    ];
    PageLayout { controls: Vec::new(), shape: Shape::Tiles(tiles.into_iter().flatten().collect()) }
}

/// The Strategy page: what the chosen fuel target asks of the tank.
///
/// Every row is absent rather than approximate when what it rests on is
/// unknown, which in practice means the whole page is a placeholder until the
/// player has completed one racing lap — there is no honest consumption figure
/// before that, and a fuel plan built on a guess is one a driver will act on
/// anyway.
///
/// The target row is first because it is the only control here; everything
/// below it is a consequence of it.
#[must_use]
pub fn pit_window(snapshot: &TelemetrySnapshot, box_called: bool, sync: SyncControls) -> PageLayout {
    use crate::telemetry::pit_window::{ClassOrder, Config, Player, Rival, lap_progress_litres, pit_window};

    let cfg = Config::default();
    let service = &snapshot.pit_service;
    // The player's own row carries their pace, the same rolling figure every
    // other car on the Relative is measured by — so the projection compares
    // like with like rather than mixing a best lap with recent ones.
    let me = snapshot.relative.iter().find(|car| car.is_focus);
    let player = Player {
        current_lap: snapshot.relative_meta.current_lap,
        lap_secs: me.and_then(|car| car.best_recent_lap_secs).unwrap_or(0.0),
        fuel_level_litres: service.fuel_level_litres,
        fuel_per_lap_litres: service.fuel_per_lap_litres.unwrap_or(0.0),
        laps_remaining: snapshot.endurance.laps_remaining,
        tank_capacity_litres: service.tank_capacity_litres,
    };

    // Which classes are quicker than the player's, from the field's own best
    // laps rather than from any assumption about what a class is.
    let mut class_of: HashMap<i32, i32> = HashMap::new();
    let mut class_pace: HashMap<i32, f32> = HashMap::new();
    for entry in &snapshot.standings {
        class_of.insert(entry.car_idx, entry.car_class_id);
        if entry.best_lap_secs > 0.0 {
            let best = class_pace.entry(entry.car_class_id).or_insert(entry.best_lap_secs);
            *best = best.min(entry.best_lap_secs);
        }
    }
    let my_class = snapshot.focus_car_class_id;
    let my_class_pace = my_class.and_then(|id| class_pace.get(&id).copied());

    let mut cars_skipped = 0_usize;
    let field: Vec<Rival> = snapshot
        .relative
        .iter()
        .filter(|car| !car.is_focus)
        // A car in the pits at the moment of the projection is not traffic;
        // where it rejoins is a guess this refuses to make.
        .filter(|car| !matches!(car.track_location, TrackLocation::InPitStall | TrackLocation::ApproachingPits))
        .filter_map(|car| {
            let Some(lap_secs) = car.best_recent_lap_secs.filter(|secs| *secs > 0.0) else {
                // Counted rather than quietly dropped: the page says how much
                // of the field it could not see.
                cars_skipped += 1;
                return None;
            };
            let their_class = class_of.get(&car.car_idx).copied();
            let class = match (my_class, their_class) {
                (Some(mine), Some(theirs)) if mine == theirs => ClassOrder::Same,
                (Some(_), Some(theirs)) => match (my_class_pace, class_pace.get(&theirs).copied()) {
                    (Some(mine), Some(theirs)) if theirs < mine => ClassOrder::Faster,
                    (Some(_), Some(_)) => ClassOrder::Slower,
                    // A class nobody has set a lap in yet is treated as the
                    // player's own, which is the middle of the three weights.
                    _ => ClassOrder::Same,
                },
                _ => ClassOrder::Same,
            };
            Some(Rival { car_idx: car.car_idx, gap_secs: car.gap_to_player_secs, lap_secs, class })
        })
        .collect();

    // What the stop costs against staying out, at the load this window is
    // built around: enough to reach the end from here. A bigger fill is a
    // longer stop, which is part of what the candidates are being weighed on.
    #[expect(clippy::cast_precision_loss, reason = "a lap count is exact in f32 far beyond any race length")]
    let to_finish =
        player.laps_remaining.map_or(service.fuel_amount_litres, |laps| laps as f32 * player.fuel_per_lap_litres);
    let litres = to_finish - service.fuel_level_litres;
    let stop_secs = snapshot.pit_model.stop_cost_secs(litres.max(0.0), true);

    let confidence = confidence_for(snapshot, field.len(), cars_skipped);
    let window = pit_window(&player, &field, cars_skipped, stop_secs, confidence, &cfg);

    // Whether the rate the recommendation asks for is actually being driven,
    // on this lap. A window that says "save 0.31" and never says whether you
    // are managing it has given half an instruction.
    let this_lap_litres = window
        .recommended_lap
        .and_then(|lap| window.candidates.iter().find(|c| c.lap == lap))
        .filter(|best| !best.save.is_free())
        .and_then(|best| {
            let laps = f32::from(u8::try_from(best.laps_from_now.saturating_add(1)).unwrap_or(u8::MAX));
            let required = service.fuel_level_litres / laps.max(1.0);
            snapshot
                .fuel_use
                .used_this_lap_litres
                .zip(snapshot.fuel_use.lap_fraction)
                .map(|(used, fraction)| lap_progress_litres(required, fraction, used))
        });
    // A surname is what a driver recognises on a panel; the full name is too
    // long for a line that already carries two of them.
    let labels = snapshot
        .relative
        .iter()
        .map(|car| {
            let short = car.driver_name.rsplit(' ').next().unwrap_or(&car.driver_name);
            (car.car_idx, short.to_owned())
        })
        .collect();
    // The Spa call, surfaced: whether a reachable per-lap saving removes a
    // whole stop from the remaining race. Honest-or-silent like the window
    // itself — no measured burn, no known tank, or a caution voiding the
    // projection all leave it unsaid rather than guessed.
    let skip_hint = (window.confidence != crate::telemetry::pit_window::Confidence::Void)
        .then(|| {
            let plan_inputs = crate::telemetry::race_plan::PlanInputs {
                current_lap: u16::try_from(snapshot.relative_meta.current_lap.max(0)).unwrap_or(0),
                laps_remaining: player.laps_remaining.and_then(|laps| u16::try_from(laps).ok())?,
                fuel_litres: service.fuel_level_litres,
                burn_per_lap: service.fuel_per_lap_litres?,
                tank_litres: service.tank_capacity_litres?,
                reserve_laps: SKIP_RESERVE_LAPS,
            };
            let saved =
                crate::telemetry::race_plan::burn_to_skip_a_stop(&plan_inputs, SKIP_MAX_SAVE_LPL, SKIP_SEARCH_STEP)?;
            Some(format!("save to {saved:.2} L/lap to skip a stop"))
        })
        .flatten();

    PageLayout {
        controls: strategy_controls(box_called, sync),
        shape: Shape::PitWindow { window: window.into(), this_lap_litres, labels, skip_hint },
    }
}

/// The Strategy page's control rows: the manual BOX BOX, and — for a spec —
/// the two standing calls sync carries.
///
/// BOX BOX raises the status border by hand whether or not the fuel maths
/// has called it yet. The fuel target is the number the driver's Relative
/// footer chases (a turn steps it, a press clears it); the tyre directive is
/// the standing answer to "do we take tyres" (a press cycles it, a turn
/// moves the wear threshold).
fn strategy_controls(box_called: bool, sync: SyncControls) -> Vec<Row> {
    use crate::sync::protocol::TyrePolicy;

    let mut controls = vec![toggle("Call BOX BOX", box_called, Control::BoxBox)];
    if let Some(target) = sync.fuel_target {
        let value = if target > 0.0 { format!("{target:.2} L") } else { "off".to_owned() };
        controls.push(stepper("Fuel target", value, Control::FuelTarget));
    }
    if let Some(directive) = sync.tyre_policy {
        let value = match directive {
            TyreDirective::DriversCall => "driver's call".to_owned(),
            TyreDirective::Set(TyrePolicy::EveryStop) => "every stop".to_owned(),
            TyreDirective::Set(TyrePolicy::Never) => "never".to_owned(),
            TyreDirective::Set(TyrePolicy::BelowWear { threshold_pct }) => format!("wear under {threshold_pct}%"),
        };
        controls.push(stepper("Tyre policy", value, Control::TyrePolicy));
    }
    controls
}

/// How much of the projection to believe, from what went into it.
///
/// Deliberately pessimistic at every boundary: a window that says `good` and is
/// not is worse than one that says `fair` and is better.
fn confidence_for(
    snapshot: &TelemetrySnapshot,
    seen: usize,
    skipped: usize,
) -> crate::telemetry::pit_window::Confidence {
    use crate::telemetry::pit_window::Confidence;

    // One caution rewrites every gap in the race, so nothing projected through
    // it is worth showing. Outside a race there is no stop to plan.
    if snapshot.relative_meta.under_caution || !snapshot.relative_meta.session_kind.is_race() {
        return Confidence::Void;
    }
    let total = seen + skipped;
    if total == 0 {
        return Confidence::Poor;
    }
    #[expect(clippy::cast_precision_loss, reason = "a field is a few dozen cars, far inside f32")]
    let covered = seen as f32 / total as f32;
    if covered < 0.5 {
        Confidence::Poor
    } else if covered < 0.8 || snapshot.pit_model.is_provisional() {
        // A stop cost that is still a constant moves every exit time together,
        // which is exactly what the ordering of the candidates turns on.
        Confidence::Fair
    } else {
        Confidence::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_car_service() -> PitService {
        PitService {
            in_car: true,
            fuel_amount_litres: 40.0,
            tyre_pressures_kpa: [159.0; 4],
            fast_repairs_available: 2,
            ..PitService::default()
        }
    }

    #[test]
    fn nothing_is_sent_while_the_driver_is_out_of_the_car() {
        // iRacing ignores pit commands then, so emitting one would leave the
        // panel showing a change the sim never made.
        let service = PitService { in_car: false, ..in_car_service() };
        let kind = RowKind::Stepper { value: String::new(), control: Control::Fuel };
        assert_eq!(request_for(Action::Increment, Control::Fuel, &kind, &service), None);
    }

    #[test]
    fn fuel_steps_by_a_whole_litre() {
        let service = in_car_service();
        let kind = RowKind::Stepper { value: String::new(), control: Control::Fuel };
        assert_eq!(request_for(Action::Increment, Control::Fuel, &kind, &service), Some(PitRequest::SetFuel(41)));
        assert_eq!(request_for(Action::Decrement, Control::Fuel, &kind, &service), Some(PitRequest::SetFuel(39)));
    }

    #[test]
    fn fuel_cannot_be_stepped_below_empty() {
        let service = PitService { fuel_amount_litres: 0.0, ..in_car_service() };
        let kind = RowKind::Stepper { value: String::new(), control: Control::Fuel };
        assert_eq!(request_for(Action::Decrement, Control::Fuel, &kind, &service), None);
    }

    #[test]
    fn toggling_fuel_arms_and_clears_it() {
        let armed = PitService { fuel_armed: true, ..in_car_service() };
        let kind = RowKind::Toggle { checked: true, control: Control::Fuel };
        assert_eq!(request_for(Action::Toggle, Control::Fuel, &kind, &armed), Some(PitRequest::ClearFuel));

        let idle = in_car_service();
        assert_eq!(request_for(Action::Toggle, Control::Fuel, &kind, &idle), Some(PitRequest::SetFuel(40)));
    }

    /// Turning a wheel sets its pressure, by a whole psi — see
    /// [`PRESSURE_STEP_PSI`] for why a kPa step left the panel frozen.
    #[test]
    fn pressures_step_by_a_whole_psi_and_stay_in_range() {
        // 158.579 kPa is 23.000 psi, which is what a live car actually reads.
        let service = PitService { tyre_pressures_kpa: [158.579; 4], ..in_car_service() };
        let kind = corner_kind(false, 159);
        let step = |action| request_for(action, Control::Tyre(Corner::RightRear), &kind, &service);

        // 24 psi and 22 psi, to the whole kPa the command API carries.
        assert_eq!(step(Action::Increment), Some(PitRequest::SetTyrePressure { corner: Corner::RightRear, kpa: 165 }));
        assert_eq!(step(Action::Decrement), Some(PitRequest::SetTyrePressure { corner: Corner::RightRear, kpa: 152 }));

        let maxed = PitService { tyre_pressures_kpa: [250.0; 4], ..in_car_service() };
        assert_eq!(request_for(Action::Increment, Control::Tyre(Corner::RightRear), &kind, &maxed), None);
    }

    /// Every click has to reach a value the sim can hold, or the panel sits on
    /// one number while the commands climb past it — the bug this replaced.
    #[test]
    fn every_click_moves_the_pressure_the_sim_reports_back() {
        let mut kpa = 158.579_f32;
        for _ in 0..5 {
            let service = PitService { tyre_pressures_kpa: [kpa; 4], ..in_car_service() };
            let kind = corner_kind(false, round_kpa(kpa));
            let Some(PitRequest::SetTyrePressure { kpa: next, .. }) =
                request_for(Action::Increment, Control::Tyre(Corner::LeftFront), &kind, &service)
            else {
                panic!("a click below the ceiling must always ask for something");
            };
            assert_ne!(next, round_kpa(kpa), "the ask must differ from what is already set");
            // What the sim gives back: the nearest whole psi to what was asked.
            kpa = (f32::from(next) / KPA_PER_PSI).round() * KPA_PER_PSI;
        }
        assert_eq!(round_kpa(kpa), 193, "five psi up from 23 is 28 psi");
    }

    /// And pressing it arms it, on the same cursor stop.
    #[test]
    fn a_press_on_a_wheel_arms_that_wheel_and_the_next_clears_it() {
        let service = in_car_service();
        assert_eq!(
            request_for(Action::Toggle, Control::Tyre(Corner::RightRear), &corner_kind(false, 159), &service),
            Some(PitRequest::SetTyre { corner: Corner::RightRear, armed: true })
        );
        assert_eq!(
            request_for(Action::Toggle, Control::Tyre(Corner::RightRear), &corner_kind(true, 159), &service),
            Some(PitRequest::SetTyre { corner: Corner::RightRear, armed: false })
        );
    }

    fn corner_kind(armed: bool, pressure_kpa: i16) -> RowKind {
        RowKind::Corner { armed, pressure_kpa, control: Control::Tyre(Corner::RightRear) }
    }

    #[test]
    fn a_fast_repair_cannot_be_armed_when_none_are_left() {
        let service = PitService { fast_repairs_available: 0, ..in_car_service() };
        let kind = RowKind::Toggle { checked: false, control: Control::FastRepair };
        assert_eq!(request_for(Action::Toggle, Control::FastRepair, &kind, &service), None);
    }

    #[test]
    fn an_unlimited_allowance_reads_as_such() {
        let service = PitService { fast_repairs_available: 255, ..in_car_service() };
        assert_eq!(fast_repair_note(&service), "unlimited");
        assert_eq!(fast_repair_note(&in_car_service()), "2 remaining");
    }

    /// One cursor stop per wheel, not two — see [`RowKind::Corner`].
    #[test]
    fn the_tyres_page_is_the_set_then_one_stop_per_wheel() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.pit_service = in_car_service();
        let layout = tires(&snapshot, crate::config::TyreBars::Temps);
        assert_eq!(layout.controls.len(), 5, "all four, then one wheel each");
        assert_eq!(layout.controls[0].label, "All Four");
        // `Corner::ALL` order, which is what the grid lays out across the axles.
        let wheels: Vec<&str> = layout.controls[1..].iter().map(|row| row.label.as_str()).collect();
        assert_eq!(wheels, ["LF", "RF", "LR", "RR"]);
        assert!(matches!(layout.shape, Shape::Corners { .. }));
    }

    #[test]
    fn the_all_tyres_row_is_ticked_only_when_every_corner_is() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.pit_service = PitService { tyres_armed: [true, true, true, false], ..in_car_service() };
        assert!(matches!(
            tires(&snapshot, crate::config::TyreBars::Temps).controls.first().map(|r| &r.kind),
            Some(RowKind::Toggle { checked: false, .. })
        ));

        snapshot.pit_service.tyres_armed = [true; 4];
        assert!(matches!(
            tires(&snapshot, crate::config::TyreBars::Temps).controls.first().map(|r| &r.kind),
            Some(RowKind::Toggle { checked: true, .. })
        ));
    }

    #[test]
    fn one_press_arms_all_four_and_the_next_clears_them() {
        let service = in_car_service();
        let empty = RowKind::Toggle { checked: false, control: Control::AllTyres };
        assert_eq!(
            request_for(Action::Toggle, Control::AllTyres, &empty, &service),
            Some(PitRequest::SetAllTyres(true))
        );

        let full = RowKind::Toggle { checked: true, control: Control::AllTyres };
        assert_eq!(
            request_for(Action::Toggle, Control::AllTyres, &full, &service),
            Some(PitRequest::SetAllTyres(false))
        );
        // The rotary must not arm a set of tyres by accident on the way past.
        assert_eq!(request_for(Action::Increment, Control::AllTyres, &full, &service), None);
    }

    #[test]
    fn in_car_tiles_appear_only_for_controls_the_car_has() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.adjustments.abs = None;
        let Shape::Tiles(tiles) = in_car(&snapshot).shape else { panic!("the In-Car page is a tile strip") };
        assert!(tiles.iter().all(|tile| tile.label != "ABS"), "a car without ABS gets no ABS tile");
        assert!(tiles.iter().any(|tile| tile.label == "BIAS"), "and keeps the ones it does publish");
    }

    /// Only the wind points anywhere, and the optional readings appear
    /// only when the session publishes them. The demo's shower takes the
    /// rain slot as live rain; a dry session falls back to the declared
    /// chance, and a session publishing neither has no rain tile at all.
    #[test]
    fn the_weather_page_is_tiles_and_only_the_wind_has_a_heading() {
        let mut snapshot = crate::demo::snapshot();
        let Shape::Tiles(full) = weather(&snapshot).shape else { panic!("the Weather page is a tile strip") };
        let labels: Vec<&str> = full.iter().map(|tile| tile.label.as_str()).collect();
        assert_eq!(labels, ["TRACK", "AIR", "KM/H", "FOG", "RAIN"]);
        assert_eq!(full[4].value, "42%", "the demo's live shower outranks its 23% declared chance");
        let pointing: Vec<&str> =
            full.iter().filter(|tile| tile.heading_rad.is_some()).map(|tile| tile.label.as_str()).collect();
        assert_eq!(pointing, ["KM/H"], "a temperature does not point anywhere");

        snapshot.weather.precip_now = Some(0.75);
        let Shape::Tiles(pouring) = weather(&snapshot).shape else { panic!("the Weather page is a tile strip") };
        assert_eq!(pouring[4].label, "HEAVY RAIN", "hard rain is named, not just numbered");

        // A session that publishes none of the optionals simply has none of
        // their tiles, rather than three reading zero.
        snapshot.weather.fog = None;
        snapshot.weather.precip_chance = None;
        snapshot.weather.precip_now = None;
        let Shape::Tiles(bare) = weather(&snapshot).shape else { panic!("the Weather page is a tile strip") };
        assert_eq!(bare.len(), 3, "track, air and wind are always published");
    }

    /// Every corner keeps its own last-stop readings, in `Corner::ALL` order —
    /// which is the order the grid lays them out across the axles.
    #[test]
    fn the_tires_page_carries_one_readout_per_corner() {
        let snapshot = crate::demo::snapshot();
        let Shape::Corners { readouts, .. } = tires(&snapshot, crate::config::TyreBars::Temps).shape else {
            panic!("the Tires page is a grid")
        };
        for (index, readout) in readouts.iter().enumerate() {
            let readout = readout.expect("the demo has a stop behind it");
            let expected = snapshot.tyres.corners[index];
            for (got, want) in readout.temps_c.iter().zip(expected.temps_c) {
                assert!((got - want).abs() < f32::EPSILON, "corner {index} kept its own temperatures");
            }
            assert!((readout.pressure_kpa - expected.pressure_kpa).abs() < f32::EPSILON);
        }
    }

    /// Before the first stop the sim publishes zeros, and a zero is not a
    /// temperature: the corner says it has nothing rather than showing one.
    #[test]
    fn a_corner_with_no_stop_behind_it_has_no_readout() {
        let mut snapshot = crate::demo::snapshot();
        snapshot.tyres.corners[0].temps_c = [0.0; 3];
        let Shape::Corners { readouts, .. } = tires(&snapshot, crate::config::TyreBars::Temps).shape else {
            panic!("the Tires page is a grid")
        };
        assert!(readouts[0].is_none());
        assert!(readouts[1].is_some());
    }
}
