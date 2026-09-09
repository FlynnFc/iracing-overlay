# Spectator focus — following the car you're watching

When you are in a session as a spectator rather than as a driver, Relative and
Standings should describe the car the camera is on, not your own empty entry.

## Table of contents

- [What's broken today](#whats-broken-today)
- [What already works](#what-already-works)
- [The rule](#the-rule)
- [What follows the focus car, and what never does](#what-follows-the-focus-car-and-what-never-does)
- [Gaps without your own driving](#gaps-without-your-own-driving)
- [Changes, file by file](#changes-file-by-file)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## What's broken today

Every widget is centred on `player_car_idx`, parsed once from the session-info
YAML (`DriverInfo:DriverCarIdx`, `session.rs:867`) and threaded through
`build_snapshot` from `session.rs:965`.

Join a session as a spectator and that index points at your own entry, which
has `IsSpectator: 1` and is never in the world. So:

- `me_lap_dist_pct`, `me_est_time`, `me_lap` and `me_gap_to_leader` are all
  absent or zero, and every relative gap is measured against a car that isn't
  on track.
- `trackers.lap_curve` never receives a sample, so the curve never becomes
  ready and the coarse fallback is all there ever is.
- Standings has no player row, so `windowed_class_positions` has no position to
  window around and every class collapses to its leaders-only view — the same
  failure the qualifying-unscored-player comment at `session.rs:1799` describes,
  for a different reason.

## What already works

**Team endurance needs no change.** In a team event `DriverCarIdx` names the
*team's car*, not whoever is currently in the seat. When a teammate is driving
and you are watching, `player_car_idx` is already your car, the telemetry
arrays are already published, and Relative and Standings already describe your
race. Confirm this in a team practice session before writing any code for it —
if it holds, the endurance case costs nothing.

The case that needs work is pure spectating: watching a session you are not
entered in.

## The rule

One index, `focus_car_idx`, resolved every tick:

1. If the player's own entry is a real competitor, focus is `player_car_idx`.
   This is the ordinary case and nothing about the app changes.
2. Otherwise — the entry is flagged `IsSpectator`, or is the pace car, or is
   missing from the drivers map entirely — focus is `CamCarIdx`, the car the
   active camera is on.
3. If `CamCarIdx` doesn't resolve to a known driver entry (it reads `-1` and
   points at nothing during camera transitions), keep the previous focus rather
   than falling back to a car nobody chose.

`is_competitor` is already computed and cached per driver (`session.rs:881`),
and it is a property of the session, not of the tick — so this rule cannot
flap. That matters: an alternative test like "the player is `NotInWorld`" would
switch focus every time you sat in the garage before joining, and Standings
would wander off while you waited.

`CamCarIdx` is a plain `i32` telemetry var, published live and in replays. It
is looked up through the existing optional-var path, so a build of the sim that
doesn't publish it degrades to today's behaviour instead of failing to start.

## What follows the focus car, and what never does

**Follows the focus car:**

- Relative — the centred row, every gap, the lap-difference column, the row
  ordering, and the scroll window's anchor.
- Standings — which row is highlighted, and the class the window is built
  around.
- SOF, which is computed for "my" class (`session.rs:1095`).
- Radar, which reuses the same gaps by construction (`session.rs:1106`).

**Never follows the focus car:**

Fuel, the pit projections, the black box pages, incident count, and everything
on the Strategy page. Every one of them reads a player-only scalar
(`PlayerCarMyIncidentCount`, the fuel level, the pit model's lane phase) that
has no per-car equivalent in the SDK. Pointing them at a camera car would not
produce another car's fuel load; it would produce your own numbers under
someone else's name, which is worse than a blank panel.

When the player is not a real entry these panels have nothing to show, so the
black box stops offering their pages at all — see `pages_for`.

## Gaps without your own driving

This is the bulk of the work, and it is the part that decides whether the
column reads to a hundredth or breathes through every corner.

`LapCurve` is built from repeated samples of one car's way around the lap. The
existing reasoning at `session.rs:484` is the guide here: the curve "describes
the track and the car, neither of which changes between the practice session
and the race after it". So on a focus change:

- **Different car class** — throw the curve and the reference lap away. A GT3
  and an LMP2 spend different fractions of a lap in the corners, and that
  fraction is exactly what the curve holds.
- **Same class** — keep both, but discard the lap currently being timed. A
  camera switch mid-lap splices two cars' track positions into one lap. Large
  jumps are already rejected by `CURVE_MAX_STEP`, but a switch to a car at a
  similar track position would not be, and it would poison a whole lap of bins.

Pace has the same shape of problem and an easier answer. `avg_lap_secs` comes
from `LapLastLapTime` and `best_lap_secs` from `LapBestLapTime`, both
player-only scalars — but the per-car arrays `CarIdxLastLapTime` and
`CarIdxBestLapTime` are already read in `build_snapshot` as `last_laps` and
`best_laps`, and `trackers.recent_laps` is already keyed by car index and
already computes a best-recent-lap for every car on track. So when focus is not
the player, source pace from those instead of from `trackers.player_pace`.
`player_pace` itself stays as-is: it feeds the fuel and stop projections, which
never leave the player.

Net effect on screen: within a lap of a camera switch the gaps are as sharp as
they are today. Before that they fall back to the est-time path already in
`relative::gap_seconds`. Degrade, never blank.

## Changes, file by file

**`telemetry/session.rs`**

- `TelemetryVars`: add `cam_car_idx: Option<Var>` via `find("CamCarIdx")`.
- New free function `resolve_focus_car(info: &SessionInfoCache, player_car_idx:
  i32, cam_car_idx: Option<i32>, previous: i32) -> i32`, implementing the rule
  above. Pure, no session, unit-testable — same shape as everything else in
  `telemetry/`.
- `SessionTrackers`: add `focus_car_idx: Option<i32>` and
  `sync_to_focus(&mut self, focus: i32, class_id: Option<i32>)`, modelled on
  the existing `sync_to_session`. It performs the curve/reference handling
  described above and clears `curve_reported_secs` / `reference_reported` so
  the notes are printed again for the new car.
- `build_snapshot`: resolve focus near the top, next to the existing
  `sync_to_session` call, then re-point the `me_*` bindings (`session.rs:965`),
  the skip test in the car loop (`session.rs:1114`), the focus row push
  (`session.rs:1171`), and the SOF class lookup (`session.rs:1095`).
- `build_standings`: rename the `player_car_idx` parameter to `focus_car_idx`.

**`telemetry/snapshot.rs`**

- Rename `CarSnapshot::is_player` and `StandingsEntry::is_player` to
  `is_focus`, with a doc comment saying what it now means. The rename is worth
  the churn: leaving it called `is_player` while it marks someone else's car is
  the kind of lie that costs an hour six months from now.
- Add `spectating: Option<String>` to `RelativeMeta` — the focus driver's name
  when focus is not the player, `None` otherwise. This is what the UI needs to
  say what it's showing.

**`telemetry/relative.rs`**

- `LapCurve::discard_lap_in_progress()`, for the same-class focus change.

**`ui/relative.rs`, `ui/standings.rs`**

- Follow the `is_focus` rename.
- When `spectating` is `Some`, show the followed driver's name in the widget
  header. Without it there is no way to tell a camera-follow view from your
  own, and a Relative that is quietly about someone else is a hazard.

  Drawn in the accent rather than in `CAUTION`, which the scroll marker beside
  it uses: a scrolled panel is a temporary state you want returned from, while
  spectating is what the widget *is* for the whole session, and an amber that
  never goes away is an amber nobody reads.

**`ui/blackbox/mod.rs`**

- `Page::is_about_the_player_car`, and `pages_for` withdrawing those pages
  while spectating. Fuel, tyres, in-car adjustments and Strategy each read a
  player-only variable with no per-car equivalent published, so watching
  somebody else does not fill them with that car's numbers — it leaves them
  reading a tank that is empty because nobody is in it. Zeros that look like
  measurements are worse than a page that isn't there.

## Testing strategy

Unit tests, in the style already in these modules:

- `resolve_focus_car`: competitor player → own index; spectator player → cam
  index; player absent from the drivers map → cam index; cam index `-1` →
  previous; cam index naming an unknown car → previous; cam index naming the
  pace car → previous.
- `sync_to_focus`: same class keeps the measured curve; different class clears
  it; both clear the reported-note flags.
- `LapCurve::discard_lap_in_progress` leaves already-measured bins intact.

Manual QA, which is the only way to test the parts that need a live sim:

1. Drive a normal race. Relative, Standings, fuel and the black box behave
   exactly as before — this is the regression that matters most.
2. Join a hosted session as a spectator. Relative centres on the camera car;
   Standings highlights and windows around it.
3. Cycle cameras between cars. The centred row follows within a tick; gaps are
   coarse briefly and sharpen within a lap.
4. Switch to a car in another class in a multi-class session. Gaps recover
   rather than reading a constant fraction long.
5. Team endurance, watching a teammate drive. Unchanged from today.

## Build phases

1. **Focus resolution and plumbing.** `CamCarIdx`, `resolve_focus_car` with its
   tests, the `is_focus` rename, and re-pointing `build_snapshot` and
   `build_standings`.
2. **Sharp gaps across a focus change.** `sync_to_focus`, `spoil_lap`, and
   per-car pace sourcing.

   *Built together with phase 1.* Phase 1 alone does not stand up: the gap
   scale comes from `LapBestLapTime`, a player-only scalar that reads zero for
   somebody who is spectating and has never driven a lap, so every gap on the
   panel would be scaled by nothing. Splitting the two would have shipped a
   phase whose only observable behaviour was a bug.
3. **Saying what you're looking at.** The `spectating` header on both Relative
   and Standings, and the player-only pages withdrawing themselves.

   The open question — hide those panels or leave them empty — resolved to
   neither: `pages_for` already exists to answer "which pages does this session
   offer", and Strategy already comes and goes through it, so the car pages
   simply stop being offered. The black box's existing `settle_page` moves a
   spectator off a withdrawn page. No new mechanism, and nothing to hide or
   un-hide.

## Explicitly out of scope

- Following the camera in replays, or after you have retired or parked. The
  focus test is "am I a real entry in this session", which is false only for a
  spectator. A driver watching a replay of their own race keeps their own
  overlays.
- A manual camera-follow toggle. The rule needs no configuration, and a toggle
  is a piece of state to get wrong before a broadcast.
- Reacting to camera *group* (`CamGroupNumber`) — TV1 versus the chase cam
  changes nothing about which car is being watched.
- Any attempt to show another car's fuel, tyres or pit state. The SDK does not
  publish them.
