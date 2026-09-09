# Strategy sandbox — a spec-only what-if for the rest of the race

> **Superseded by `plans/strategy-spec-mode.md`.** The what-if planner is
> better folded into the existing Strategy page's spec face than shipped as a
> separate widget — same numbers, one screen. Kept for the `plan_race`
> model, which that plan reuses. Do not build this as a standalone panel.

A crew chief watching a stint has one recurring question: *if we lift a
tenth of a litre a lap, does the last splash disappear?* Today answering it
means a spreadsheet on a second monitor. This widget answers it on the
overlay: the projected remaining race — stints, stop laps, fuel per stop —
with a handful of sliders to bend the assumptions and watch the plan move.

Spec-only by design: it draws in the watching seats (`Seat::Spectating`,
`Seat::TeamMate`) and never for the driver. A driver has no business
dragging sliders at speed, and the seat that has time to think is exactly
the seat this is for. Builds on team sync (`plans/team-sync.md`) phase 2:
the measured inputs come off the ledger.

## The model

One pure function, in the style of `telemetry/endurance.rs`:

```
plan_race(inputs, sketch) -> RacePlan
```

**Inputs** — all measured, none typed in:

- Burn per lap: the trailing average of the ledger's lap-closed events
  (window of ~5 laps, same figure Auto Fuel uses — one dataset, so every
  spec's baseline is identical bytes).
- Tank litres: `DriverCarFuelMaxLtr` from the session YAML.
- Current fuel: the driver scalars tick.
- Laps remaining: `endurance::laps_remaining` / `projected_total_laps`,
  which every member computes alike from the shared session clock.
- Pit loss: the existing `standings.pit_loss_secs` setting.

**The sketch** — what the sliders bend, all deltas from measured so a reset
is going back to zero:

- Burn per lap, ± a few tenths of a litre (the headline slider: fuel
  saving or pushing).
- Laps of margin at the finish (the same idea as Auto Fuel's
  `fuel_margin_laps`).
- Pit loss seconds, for tracks where the measured guess is off.

**Output** (`RacePlan`): the remaining stints — each with its start lap,
length, and litres to add at its stop — plus the projected finish margin in
laps of fuel. Computed greedily: run the tank to `margin`, fill to
`min(tank, what the remaining laps need)`, repeat. Deterministic, no
randomness, unit-tested against hand-computed races (short splash, exact-fit,
zero-stop, and the degenerate no-burn-measured case, which renders as "no
laps measured yet" rather than a plan built on nothing).

## The widget

A standalone panel, not a black box page — which buys it everything panels
already have: a `visible` tick, drag, scale, a settings page, and (from
`plans/seat-layouts.md`) a watching-layout position for free, which is the
only layout it will ever draw in.

Two columns: **measured** (sliders at zero) and **your sketch**, with the
stop laps that moved highlighted. The point of showing both is that the gap
*is* the answer — "saving 0.15 L/lap turns lap 96's splash into no stop" is
one glance. A reset control zeroes the sketch.

Sketches are **local and never act**: nothing is published to the ledger, and
nothing arms the sim — the sandbox is a lens, so the one-actuator and
one-number rules of team sync are untouched. When a sketch becomes a
decision, the crew chief carries it out through the phase-4 pit-box writes
(set the fuel for the next stop), which go through the driver-side consent
gate like any other remote change. A "share this sketch with the other
specs" event is deliberately deferred: two crew members pointing at the same
numbers over voice chat is already how teams talk, and shared mutable state
needs an owner — a problem worth having only once the local version proves
the widget earns its screen space.

## Build shape

1. `telemetry/race_plan.rs` — `plan_race` and its tests. Pure; no sync
   dependency, so it lands and tests before the wiring exists.
2. The panel (`ui/strategy_sandbox.rs`), fed from ledger-derived inputs,
   drawing only in watching seats; config struct with `visible`, `pos`,
   `watch_pos`, `scale`, and the sketch is deliberately *not* persisted — a
   what-if from last race is stale by definition.
3. Settings page entry and tray tick, like every other panel.

Depends on team-sync phase 2 for its inputs while spectating; the planner
itself (step 1) can be built and tested immediately.

## Explicitly out of scope

- Driving-seat access. The driver's strategy surface stays the Strategy
  page.
- Tyre-life or weather modelling. The plan is a fuel plan; adding wear
  models means inventing data iRacing doesn't publish.
- Shared or persisted sketches (see above).
- Rival-strategy simulation ("what if the 12 pits early") — the pit-window
  code already reasons about rivals for the driver, and duplicating it here
  doubles the widget for a question nobody asked yet.
