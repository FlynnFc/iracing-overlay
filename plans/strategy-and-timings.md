# Strategy pages and the Timings Collector — specification

Three features, specified to be built in this order. Each is usable on its own;
each makes the next one sharper.

1. **Fuel Save Target** — what you must save, live, to change your strategy.
2. **Timings Collector** — a practice-session drill runner that measures what a
   pit stop actually costs on this track, in this car.
3. **Traffic-Scored Pit Window** — which of the next few laps to box on, scored
   by what you would emerge into.

## Table of contents

- [Principles](#principles)
- [What the sim will and will not tell us](#what-the-sim-will-and-will-not-tell-us)
- [Feature 1 — Fuel Save Target](#feature-1--fuel-save-target)
- [Feature 2 — Timings Collector](#feature-2--timings-collector)
- [Feature 3 — Traffic-Scored Pit Window](#feature-3--traffic-scored-pit-window)
- [Cross-cutting: config, storage, gating](#cross-cutting-config-storage-gating)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## Principles

These are the rules the rest of the document is held to. They exist because
"works amazingly well out of the box" is mostly a statement about defaults and
about what happens when data is missing.

1. **No row without a basis.** A figure that cannot be computed honestly is
   absent, not zero and not guessed. Every widget in this app already works
   this way (`Est. Laps` on the Fuel page appears only once a lap has been
   completed) and strategy is where a made-up number does the most damage.
2. **Degrade, never blank.** Where a *better* input is missing, fall back to a
   coarser one and say which. Only where there is no input at all does the row
   disappear.
3. **Readable at 200 km/h.** One number per row, the units in the label, no
   sentence a driver has to parse. If something needs a paragraph to explain,
   it belongs in this file, not on screen.
4. **Nothing on the wire that nobody acts on.** Every pit command is a
   machine-wide window message (see `AUTO_FUEL_MIN_INTERVAL`). Strategy reads;
   it arms only when asked, or at a pit entry.
5. **Pure functions in `telemetry/`, presentation in `ui/`.** Same split as
   `telemetry::endurance` and `telemetry::relative`: every projection is a free
   function over plain data, unit-testable with no session and no window.
6. **A projection says how confident it is.** Every forward simulation carries
   a confidence, and the UI shows it. A green-flag projection during a caution
   is worse than no projection.

---

## What the sim will and will not tell us

Established, not assumed — this is what the existing code already reads.

### Available per car (whole field)

| Data | Source | Already in |
|---|---|---|
| Track position | `CarIdxLapDistPct` | `relative::gap_seconds` |
| Relative gap, seconds | `LapCurve` | `CarSnapshot::gap_to_player_secs` |
| Lap count / laps down | `CarIdxLap` | `CarSnapshot::lap_diff`, `StandingsEntry::laps_down` |
| Recent pace | rolling window of `CarIdxLastLapTime` | `CarSnapshot::best_recent_lap_secs` |
| Best lap | `CarIdxBestLapTime` | `StandingsEntry::best_lap_secs` |
| Class, colour, iRating | session YAML | `StandingsEntry` |
| Track surface (on track / pit stall / approaching / not in world) | `CarIdxTrackSurface` | `track_location` |
| Stint length so far, average stint | measured | `StandingsEntry::current_stint_laps`, `avg_stint_laps` |
| Stationary time last stop, average | measured | `StandingsEntry::last_pit_secs`, `avg_pit_secs` |
| Stops taken, stops still owed | `PitStops` + projection | `pit_stops`, `stops_remaining` |

### Available for the player only

Fuel level, fuel used per lap, tank capacity, tyre wear and temps, armed pit
service, incident count.

### Not available at all

- **Other cars' fuel, tyre state, or planned strategy.** Everything about a
  rival's plan is inferred from their observed stint lengths.
- **Other cars' incident counts.** "Aggressive" can only ever be a measurement
  we make ourselves (lap-time variance, off-track excursions), never a rating
  read from the sim.
- **A weather forecast.** `ChanceOfRain` is a session setting, not a forecast.
- **Pit lane geometry.** No pit entry/exit position, no lane length, no speed
  limit. This is why the Timings Collector measures rather than looks up.

---

## Feature 1 — Fuel Save Target

### The problem

The Fuel page answers *what do I put in*. It does not answer the question a
driver actually asks mid-stint, which is *can I make this work, and what do I
have to do differently*. That question is arithmetic a driver does badly at
speed and a pit board does slowly.

### Rows

New page `Page::Strategy`, subtitle `"Estimates"`. Endurance-gated (see
[gating](#cross-cutting-config-storage-gating)).

```
STRATEGY                                    Estimates

  Target            Drop a stop        < >     stepper
  Laps to target    17
  Fuel needed       46.8 L
  In tank           41.2 L
  Save              0.33 L/lap  (4.1%)        red when > 0
  This lap          −0.12 L             ▼     live, signed
  In hand           −1.9 laps                 signed, red when negative
  Splash            5.6 L  ≈ 4.0 s            only when short
```

### Target selection

`Control::FuelTarget`, a stepper cycling three modes:

| Mode | Laps to target | When it is the default |
|---|---|---|
| `Next stop` | laps to the end of the planned stint | sprint, or `stops_remaining == 0` |
| `Drop a stop` | laps that one fewer stop implies | endurance with `stops_remaining >= 1` |
| `Finish` | `endurance.laps_remaining` | when `stops_remaining == 0` |

The default is chosen once per session, from `EnduranceMeta`, and then left
alone — a target that moves on its own is a target nobody trusts. Persisted in
`BlackBoxConfig` so it survives a restart.

`Drop a stop` laps-to-target: with `laps_remaining` L and `stops_remaining` S,
the stint length that would let S−1 stops cover the rest is
`ceil(L / max(1, S))`. That is what to fuel for. If S is 0, the mode is
unavailable and the stepper skips it.

### Arithmetic

```
fuel_needed      = laps_to_target × fuel_per_lap_litres
required_per_lap = fuel_level_litres / laps_to_target
save_per_lap     = fuel_per_lap_litres − required_per_lap   (> 0 means save)
save_percent     = save_per_lap / fuel_per_lap_litres
in_hand_laps     = (fuel_level_litres − fuel_needed) / fuel_per_lap_litres
splash_litres    = max(0, fuel_needed − fuel_level_litres)
splash_secs      = stop_cost(splash_litres, tyres: false)   // Feature 2
```

`fuel_per_lap_litres` is already measured over recent laps (`PitService`), not
derived from `FuelUsePerHour` — see the note in `telemetry::pit`.

### The live "This lap" row

The one genuinely new measurement, and the one that makes the page a tool
rather than a readout: **are you currently on target, right now, this lap.**

Naively you would compare fuel used so far against `required_per_lap ×
lap_dist_pct`. That is wrong in exactly the way scaling a gap by track position
is wrong: fuel burns with time under power, not with distance, so the figure
reads rich down every straight and lean through every corner, and swings by
tenths of a litre each corner while you are trying to read it.

Use the lap curve instead. `LapCurve::fraction_at(pct)` is what fraction of a
lap's *time* has passed, which is a far better proxy for what fraction of a
lap's fuel should be gone:

```
expected = required_per_lap × curve.fraction_at(me_pct)
actual   = fuel_at_line − fuel_level_litres
this_lap = expected − actual        // positive = you are up on target
```

`fuel_at_line` is latched at each start/finish crossing. Requires the curve to
have measured a lap; without it, the row is absent (principle 1) rather than
computed from track position.

**Smoothing.** Fuel level sloshes. A 0.5 s rolling mean over the raw level,
then the arithmetic — not the other way round, or the smoothing lags a lap
crossing by half a second and the row jumps every lap.

### Edge cases

| Case | Behaviour |
|---|---|
| No lap completed yet | Whole page absent — `fuel_per_lap_litres` is `None` |
| `laps_remaining` unknown (lap-limited or untimed session) | Use `SessionLapsRemain` if published; else `Next stop` mode only |
| Tank capacity unknown (near empty) | `Splash` row absent; the rest stands |
| Already comfortably clear | `Save` reads `Clear`, in the ordinary colour, no red |
| In the pit lane | Page freezes at its last on-track values, like Auto Fuel does, and says so in the subtitle. Pit-lane pace and pit-lane fuel use are not racing figures |
| Target unreachable even on an empty tank | `Save` reads `Not possible`; do not print a save rate nobody can drive to |

### Module

`telemetry/strategy.rs`, pure:

```rust
pub struct FuelPlan {
    pub laps_to_target: i32,
    pub fuel_needed_litres: f32,
    pub required_per_lap_litres: f32,
    pub save_per_lap_litres: f32,       // negative when already clear
    pub in_hand_laps: f32,
    pub splash_litres: f32,
}

pub enum FuelTarget { NextStop, DropAStop, Finish }

pub fn fuel_plan(target: FuelTarget, inputs: FuelInputs) -> Option<FuelPlan>;
pub fn lap_progress_litres(required_per_lap: f32, lap_fraction: f32, used_this_lap: f32) -> f32;
```

---

## Feature 2 — Timings Collector

### The problem

`StandingsEntry::avg_pit_secs` is **stationary time only**. Strategy needs
*total* time loss, and needs to price a stop it has never seen — "what does a
splash-and-no-tyres cost?" — which no observation of past stops can answer,
because past stops were all the same kind.

Pit lanes range from about 15 s to about 50 s of loss. Fill rates and tyre
change times vary by car. Guessing any of it makes Feature 3 wrong in the
direction that matters most.

### Where it runs

Practice and offline-test sessions only — `SessionKind::Practice`. Not
qualifying (out-laps matter), never a race. The page is absent otherwise, so it
cannot be opened at the wrong moment.

### The measurement that needs no drill

Total transit loss falls out of the lap curve, and can be measured on *every*
stop including in a race:

```
loss = (t_exit − t_entry) − (curve.fraction_at(pct_exit) − curve.fraction_at(pct_entry)) × pace_secs
```

That is: how long the car actually took between two track positions, minus how
long it would have taken at racing pace. No pit lane database, no hand-entered
lengths, correct on every track, and it self-corrects as the curve sharpens.

Detect entry/exit from `CarIdxTrackSurface` transitions
(`OnTrack → ApproachingPits`, and back). Stationary time is the
`InPitStall` span, already tracked.

`loss` includes both the lane transit *and* the stop, so the drills exist to
split it into parts that can be recombined for a stop nobody has taken yet.

### The drills

Each drill isolates exactly one unknown, and the order matters — each subtracts
the ones before it.

| # | Drill | Arms | Measures |
|---|---|---|---|
| 1 | **Drive through** | nothing | `transit_loss` — the lane with no service at all |
| 2 | **Splash** | fuel ≈ 1.5 laps' worth | `overhead` = stop − fill_rate × litres, with fill_rate still unknown, so this and #3 are solved as a pair |
| 3 | **Full fuel** | fuel to brim | `fill_rate` from the difference with #2 |
| 4 | **Tyres only** | 4 tyres, no fuel | `tyre_change_secs` |
| 5 | **Full fuel + tyres** | both | `concurrent`: whether the stop is `max(fuel, tyres)` or `fuel + tyres` |
| 6 | **Tearoff / fast repair** | each in turn | their marginal cost |

Drill 2 needs a small, known load, so it needs `tank_capacity_litres` and
`fuel_per_lap_litres` — both already measured. If either is missing the drill
is shown greyed with the reason ("needs a completed lap").

Solving 2 and 3 together: two stops with known litres `l₂ < l₃` and measured
service times `s₂, s₃` give

```
fill_rate = (s₃ − s₂) / (l₃ − l₂)
overhead  = s₂ − fill_rate × l₂
```

which is why the splash must be genuinely small — a large `l₂` makes the
subtraction ill-conditioned. 1.5 laps is the right size.

### The page

```
TIMINGS COLLECTOR                     Practice only

  Track            Spa Francorchamps
  Car              Mercedes-AMG GT3
  Progress         4 of 6

  1 Drive through      41.2 s      ✓ 3 runs
  2 Splash 12 L         6.4 s      ✓ 2 runs
  3 Full fuel          24.1 s      ✓ 2 runs
  4 Tyres only         11.8 s      ✓ 2 runs
  5 Fuel + tyres        — s        ▸ arm        ← cursor
  6 Tearoff             — s          arm

  Box approach          +1.4 s     your own time lost to the box
```

- One press on a drill row **arms exactly what that drill needs**, through the
  existing `PitRequest` / `commands_for` path. This is the single most
  important design decision on the page: the usual way a calibration run is
  wasted is arming the wrong service and not noticing until you read the
  number.
- After the stop, **auto-validate**: compare what iRacing actually serviced
  against what the drill asked for. Mismatch (tank hit capacity, tyres did not
  take, a penalty served in the lane, damage repaired) discards the sample and
  the row says `rejected — retry`. A silently wrong calibration is worse than
  no calibration.
- **Median of runs**, target 3, minimum 2. Below 2 the row shows the single
  measurement in the muted colour, and the derived model marks itself
  provisional.
- **Box approach** is `stationary_time − theoretical_service_time`: how much you
  personally lose stopping badly or crawling to the box. Nobody measures this
  and it is routinely a second or two.

### Derived model

The whole point of the page. One function, consumed by Features 1 and 3:

```rust
pub struct PitModel {
    pub transit_loss_secs: f32,
    pub overhead_secs: f32,
    pub fill_rate_secs_per_litre: f32,
    pub tyre_change_secs: f32,
    pub concurrent: bool,
    /// How many clean runs the weakest input rests on. 0 means every figure
    /// here is a default, not a measurement.
    pub runs: u8,
}

impl PitModel {
    pub fn stop_cost_secs(&self, litres: f32, tyres: bool) -> f32 {
        let fuel = if litres > 0.0 { self.overhead_secs + litres * self.fill_rate_secs_per_litre } else { 0.0 };
        let tyre = if tyres { self.tyre_change_secs } else { 0.0 };
        let service = if self.concurrent { fuel.max(tyre) } else { fuel + tyre };
        self.transit_loss_secs + service
    }
}
```

### Defaults, so it works before any drill is run

`PitModel::default()` with `runs: 0`, from figures that are about right across
iRacing's road content:

| Field | Default | Why |
|---|---|---|
| `transit_loss_secs` | measured from the first observed stop, else 30.0 | Every stop measures this for free; the constant only covers the very first |
| `overhead_secs` | 1.5 | Rig connect/disconnect |
| `fill_rate_secs_per_litre` | 0.30 | ≈ 3.3 L/s, typical iRacing GT refuel |
| `tyre_change_secs` | 12.0 | Typical four-tyre service |
| `concurrent` | `true` | iRacing services in parallel |

Any figure resting on `runs == 0` is shown in the muted colour wherever it
appears, and Feature 3 lowers its confidence accordingly. **This is what makes
the collector optional rather than a prerequisite.**

### Storage

`%APPDATA%\race\pit-timings.toml`, beside the existing config and for the same
reason (`cargo clean` must not eat it). Keyed by track *and* car, because both
change the answer:

```toml
[[entry]]
track_id = 341
track = "Spa Francorchamps"
car_id = 132
car = "Mercedes-AMG GT3 2020"
transit_loss_secs = 41.2
overhead_secs = 1.6
fill_rate_secs_per_litre = 0.283
tyre_change_secs = 11.8
concurrent = true
runs = 2
measured_at = "2026-07-29"
```

`track_id` / `car_id` need `WeekendInfo.TrackID` and the driver's `CarID` added
to `session_info.rs` — two fields, both already in the YAML. Names are stored
alongside for a human reading the file, matched on by ID.

### Edge cases

| Case | Behaviour |
|---|---|
| Damage taken during a drill | Rejected — repair time is not service time |
| Series with a fixed refuel rate | `fill_rate` measures whatever the series does; nothing special needed |
| Tank already full when a fuel drill is armed | Drill says `burn some fuel first` rather than measuring a no-op |
| Busy practice server, lane traffic | Median across runs absorbs it; the outlier is visible in the run list |
| Driver leaves the session mid-drill | Partial drill discarded; completed drills persist |
| Different car, same track | Separate entry. Transit loss is re-measured, which is correct — pit lane speed limits are per-car in some series |

---

## Feature 3 — Traffic-Scored Pit Window

The flagship, and the one a driver genuinely cannot do. Build last: it consumes
both features above.

### The question

Not "when am I out of fuel" — that is the easy part. It is: **of the next few
laps I could box on, which one puts me back on track with the least time lost
to traffic.** Answering it needs every car's position and pace, a pit cost
model, and a forward simulation. All three now exist.

### Algorithm

For each candidate lap `L` in the next `window_laps` (default 6):

1. **When I would exit.**
   `t_entry(L)` = now + time to reach the pit entry on lap `L`, from the lap
   curve. `t_exit(L) = t_entry(L) + stop_cost(litres_needed(L), tyres)`.
   `litres_needed(L)` comes from Feature 1's target, so a longer stint before
   the stop means a smaller fill and a cheaper stop — which is exactly the
   trade-off being weighed.
2. **Where everyone else is then.** Each car advances along the pct axis at its
   own `best_recent_lap_secs`, using the shared lap-curve shape. A car with no
   pace figure yet advances at the class median; a car in the pits at the time
   is skipped.
3. **Where I would be.** At the pit exit position, on cold tyres — apply an
   out-lap penalty (config, default 2.0 s) to my first lap after the stop.
4. **Score the exit.** Simulate `score_laps` laps (default 3) after the exit at
   1 Hz and integrate a penalty:

   | Situation | Penalty |
   |---|---|
   | Car of my class within ±1.0 s | 1.0 × weight, this is a fight |
   | Faster class closing within ±2.0 s | 1.5 × weight — a lift and a blue flag |
   | Slower class ahead within ±2.0 s | 0.8 × weight — a pass to make |
   | Car ahead of me, slower than me by > 0.3 s/lap | 1.2 × weight, scaled by how long until I clear it |
   | Car behind me, faster than me by > 0.3 s/lap | 1.0 × weight, scaled by how soon it arrives |

   Weights are per-second-of-exposure, so the score is in units of seconds of
   compromised running. Not a real time loss — an index — and labelled as such.
5. **Report** the candidate with the lowest score, plus the two cars I would
   emerge between and the first car that becomes a problem.

### Rows

```
PIT WINDOW                            Estimates · 2 runs

  Fuel window       lap 34 – 41
  Recommend         lap 37            green
  Traffic           ▁▃█▆▂▁            one cell per lap, 36..41
  Exit              P8, between #14 and #7
  First trouble     #23  in 2 laps    0.8 s/lap faster
  Cost of waiting   +1.4 s            vs boxing now
  Confidence        good              good | fair | poor | void
```

The strip is the whole feature in one glance. `RowKind::Strip { cells: Vec<Cell> }`
where `Cell { score: f32, is_recommended: bool, in_fuel_window: bool }` — the
only new row kind needed, drawn like the existing radar bars.

### Confidence

| Level | When |
|---|---|
| `good` | Pit model has ≥ 2 runs; ≥ 80% of the field has a recent lap time; green flag |
| `fair` | Pit model is defaults, or 50–80% of the field has pace |
| `poor` | < 50% of the field has pace, or the curve has no measured lap |
| `void` | Caution or safety car out, or the session is not a race |

`void` replaces the recommendation with `caution — window void`. A green-flag
projection during a caution is actively misleading: one caution rewrites the
entire answer, and this is the one case where showing nothing is right.

### Cost bounds and honesty

- **Say what was dropped.** If the field is bigger than the simulation covers,
  or candidates were truncated, the subtitle says so. Silent truncation reads as
  "considered everything".
- **Cheap enough to run every tick?** No, and it does not need to be. 6
  candidates × 3 laps × 1 Hz × 60 cars ≈ 3,200 car-position evaluations. That is
  nothing, but the *inputs* only change meaningfully once a lap. Recompute on
  lap crossing and on any pit-service change, cache in between. Keeps it off the
  hot path entirely.
- **Never arms a stop.** The window recommends; the driver boxes. An overlay
  that pits you is an overlay you turn off.

### Module

```rust
pub struct Candidate {
    pub lap: i32,
    pub traffic_score: f32,
    pub exit_class_position: i32,
    pub emerge_between: (Option<i32>, Option<i32>),   // CarIdx
    pub first_conflict: Option<Conflict>,
    pub in_fuel_window: bool,
}

pub struct PitWindow {
    pub fuel_window: std::ops::RangeInclusive<i32>,
    pub candidates: Vec<Candidate>,
    pub recommended_lap: Option<i32>,
    pub confidence: Confidence,
}

pub fn pit_window(field: &[Contender], me: &PlayerState, model: &PitModel, curve: &LapCurve, cfg: &WindowConfig) -> PitWindow;
```

`Contender` already exists in `telemetry::endurance` for the net-position
projection; extend it rather than adding a second field type.

---

## Cross-cutting: config, storage, gating

### Page gating

`Page::ALL` becomes a function of the snapshot rather than a constant:

```rust
pub fn pages_for(snapshot: Option<&TelemetrySnapshot>, settings: &BlackBoxConfig) -> Vec<Page>
```

| Page | Shown when |
|---|---|
| `Strategy` | `session_kind.is_race()` and endurance mode is on (`endurance_mode` = `"on"`, or `"auto"` and `multi_stop_race`) |
| `PitWindow` | as `Strategy`, plus a pit model exists (measured or default) |
| `TimingsCollector` | `session_kind == Practice` |
| all existing | unchanged |

Paging must stay stable as the list changes shape under it — the black box
already handles a page changing shape (`settle_cursor`); page *identity* must be
preserved across a rebuild rather than an index, or endurance mode latching mid
race would silently move the driver to a different page.

### New config, `[strategy]` in `race-overlay.toml`

| Key | Default | Range |
|---|---|---|
| `fuel_target` | `"auto"` | `auto` \| `next_stop` \| `drop_a_stop` \| `finish` |
| `window_laps` | `6` | 3–10 |
| `score_laps` | `3` | 1–5 |
| `out_lap_penalty_secs` | `2.0` | 0–10 |
| `same_class_threshold_secs` | `1.0` | 0.5–3 |
| `other_class_threshold_secs` | `2.0` | 0.5–5 |

All defaults chosen so the feature is correct with an empty config file; nobody
should have to tune this to get value from it.

---

## Testing strategy

Same shape as the existing modules: pure functions, named tests that state the
behaviour rather than the mechanism.

**Feature 1**
- A target that needs no saving reads clear, not a negative save rate.
- A target that is impossible on an empty tank says so rather than printing a
  rate.
- The live row is measured against lap *time* fraction, not distance — a lap
  driven with the same fuel use but a different corner/straight split gives the
  same reading.
- Every input missing in turn removes exactly the rows that depend on it.

**Feature 2**
- The splash/full pair solves to a known fill rate and overhead.
- A drill whose service does not match what was armed is rejected.
- A drill with damage repaired in the lane is rejected.
- `stop_cost` for a fill-only stop is below a fill-plus-tyres stop, and equals
  it when tyres are the binding constraint and service is concurrent.
- A model with `runs == 0` is still usable, and reports itself as defaults.
- Transit loss measured from a synthetic entry/exit pair matches the hand
  calculation.

**Feature 3**
- A field with one car, far away, recommends boxing immediately.
- A field packed at the exit point recommends waiting, and the strip shows it.
- A caution voids the recommendation.
- A car with no pace figure does not silently score as infinitely fast.
- The recommended lap is always inside the fuel window when one exists.
- Candidate scores are stable tick to tick given unchanged inputs (no jitter in
  the recommendation).

---

## Build phases

Each phase ships something usable.

| Phase | Contents | Depends on |
|---|---|---|
| 1 ✅ | `telemetry/strategy.rs` fuel plan + `Page::Strategy` rows + gating | nothing |
| 2 ✅ | Live "This lap" row, fuel-level smoothing, `fuel_at_line` latch | phase 1, lap curve |
| 3 ✅ | `PitModel` with defaults + transit-loss measurement on every stop | nothing |
| 4 | `Page::TimingsCollector`: drill state machine, arming, validation, storage | phase 3 |
| 5 | `pit_window` simulation + `RowKind::Strip` + `Page::PitWindow` | phases 3 and 4 |
| 6 | Rival profiling (lap-time variance, excursion counts) folded into scoring | phase 5 |

Phase 3 before phase 4 deliberately: transit loss is measurable in a race with
no drills at all, so the pit model starts improving itself before the collector
exists.

---

## Explicitly out of scope

- **Anything that arms a race stop automatically.** Auto Fuel already sits at
  the edge of what an overlay should do unasked.
- **Rival fuel or tyre estimation.** Not published; a guess dressed as a number
  is worse than an absent row.
- **Weather strategy.** No forecast exists to read.
- **Multi-stop optimisation over a whole race.** The next stop is the decision a
  driver actually makes; a full race plan needs assumptions (cautions, rival
  strategies) the sim cannot support, and would be wrong in ways that look
  authoritative.
- **Any figure presented as certain.** Everything here is labelled an estimate,
  because everything here is one.
