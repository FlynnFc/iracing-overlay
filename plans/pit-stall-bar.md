# Pit Stall Bar — stopping on the mark, specification

A fifth widget. It appears in the last stretch of the pit lane, shows how far
your car is from the perfect stopping point in your own box, and disappears
again once you're out. Nothing to configure, nothing to calibrate, no track
database.

## Table of contents

- [Principles](#principles)
- [What the sim will and will not tell us](#what-the-sim-will-and-will-not-tell-us)
- [Where "perfect" comes from](#where-perfect-comes-from)
- [Turning track percentage into metres](#turning-track-percentage-into-metres)
- [Self-calibration](#self-calibration)
- [The widget](#the-widget)
- [When it appears and when it goes away](#when-it-appears-and-when-it-goes-away)
- [Defaults, and what happens when each input is missing](#defaults-and-what-happens-when-each-input-is-missing)
- [Changes, file by file](#changes-file-by-file)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## Principles

The same bar the rest of this repo is held to: *works amazingly well out of the
box* is mostly a claim about defaults and about behaviour when an input is
absent. Three rules follow from that, and the rest of the document is an
application of them.

1. **Never lie about precision.** A bar drawn to centimetre resolution off a
   number that is only good to a metre is worse than no bar. Every reading in
   this widget has a known provenance, and the widget's own confidence changes
   what it draws (see [Self-calibration](#self-calibration)).
2. **First stop at a new track must be useful.** Anything that only works after
   a learning lap is a feature for the second race, not the first. The sim's own
   stall position is the day-one answer; learning only sharpens it.
3. **Invisible unless it's the moment.** This is the Radar Bars rule. A driver
   glancing down mid-stint should never see this panel. It exists for perhaps
   eight seconds a stop.

---

## What the sim will and will not tell us

Confirmed against the SDK and against what this crate already reads. Anything
marked *verify* must be checked with `race-overlay.exe --dump-session-info` /
`--dump-vars` from inside a car in the pit lane before Phase 1 is written —
these two commands exist precisely for this.

**Session info YAML**

| Field | Section | Use |
| --- | --- | --- |
| `DriverPitTrkPct` | `DriverInfo` (top level, not per-driver) | *verify.* The player's assigned pit stall as a fraction of a lap. This is the whole feature's day-one target. |
| `TrackLength` | `WeekendInfo` | e.g. `"3.70 km"`. Fallback scale for pct→metres. |
| `TrackID` | `WeekendInfo` | Calibration key. |
| `CarPath` | `DriverInfo.Drivers[me]` | *verify.* Calibration key's other half — e.g. `"mercedesamggt3evo"`. |

**Telemetry variables**

| Var | Present today? | Use |
| --- | --- | --- |
| `CarIdxLapDistPct` | yes, already read | Your position along the lap. Indexed by `player_car_idx`. |
| `PlayerCarInPitStall` | yes, already read (`BlackBoxVars`) | Ground truth: the sim says you are in your box. Drives both the confirmation lamp and the whole calibration. |
| `Speed` | **new** | m/s. Used to measure the local metres-per-percent (below) and to suppress the bar while moving fast. |
| `PlayerTrackSurface` | yes, via `TrackLocation` | Distinguishes lane from box from track. |

**What the sim will not tell us:** the physical dimensions of the stall box, the
length of the pit lane path, where the box's painted markings are, or where your
crew's jack man is standing. None of that is published in any form. Everything
below is built without it.

---

## Where "perfect" comes from

`DriverPitTrkPct` is a point on the lap, in the same coordinate as
`CarIdxLapDistPct`. The signed error is the wrapped difference:

```
error_pct = wrap(my_lap_dist_pct - target_pct)      // wrap to (-0.5, 0.5]
```

Negative means short of the box, positive means past it. Wrapping matters
because a pit stall can sit either side of the start/finish line — at Spa the
lane and the line interleave, and an unwrapped subtraction would read −0.98
instead of +0.02 and put the marker hard against the wrong end of the bar.

`wrap` is the only piece of this that is subtle, and it is already the pattern
`telemetry/relative.rs` uses for track-position differences; the same helper
should be reused rather than reimplemented.

**Why not learn the position outright and skip `DriverPitTrkPct`?** Because
stall assignment changes session to session. The absolute pct is worthless
across sessions; what is stable is the *offset* between what the sim calls the
stall and where the car actually needs to be, which is a property of the car's
length and origin plus the track's lane geometry. So the sim's number is always
the base, and calibration only ever adjusts it. This is the difference between a
feature that works on your first stop at a new track and one that does not.

---

## Turning track percentage into metres

A percentage is not a distance, and the obvious conversion is wrong.

`error_pct × track_length` assumes the pit path and the racing line are the same
length. They are not — the lane cuts the corner at pit entry and rejoins after,
so a percent of lane is a different number of metres from a percent of track,
by a few percent at most circuits and by considerably more at places like
Le Mans. At the resolution this widget claims (tenths of a metre) that error is
not acceptable.

**Primary method — measure it.** While the car is moving down the lane, the
metres per percent is directly observable:

```
metres_per_pct  =  (speed_m_per_s × dt)  /  Δlap_dist_pct
```

Sample this continuously whenever the car is on pit road above about 5 m/s,
discard ticks where `Δlap_dist_pct` is zero or where the lap wraps, and hold a
rolling median over the last ~2 seconds of samples. A median rather than a mean
because a single wrapped or dropped tick produces an enormous outlier and a mean
never recovers from it inside one pit entry.

This is exact, needs no track data, self-corrects for the lane/track mismatch,
and is available *before* you reach your box — the measurement happens on the
way down the lane, which is the only place it is needed.

**Fallback — `WeekendInfo.TrackLength`.** If `Speed` is missing, if the lane was
entered too slowly to sample, or if the rolling window is empty, fall back to
`track_length_m × 1.0`. Parse `"3.70 km"` by taking the leading number and the
unit; treat a missing or unparseable value as the last fallback below.

**Last resort — no scale at all.** If neither is available the widget still
works, because the bar is fundamentally a *proportion*, not a distance: draw the
bar and its zones in percentage space using a nominal 4000 m lap, and **omit the
numeric readout entirely**. A bar with no number is honest; a number derived
from a guessed lap length is not. This is principle 1 in its concrete form.

---

## Self-calibration

The sim's stall point and the point at which the car is actually served are not
the same point, and the gap between them is car-dependent. Learning it costs
nothing, because the sim already publishes the answer.

**The observation.** `PlayerCarInPitStall` goes true as the car enters the
sim's stall box and false as it leaves. Recording `lap_dist_pct` at both
transitions gives the box's near edge and far edge in the same coordinate as
everything else. The box centre is their midpoint.

```
observed_centre_pct = (enter_pct + exit_pct) / 2
correction_m        = (observed_centre_pct - driver_pit_trk_pct) × metres_per_pct
half_width_m        = (exit_pct - enter_pct) / 2 × metres_per_pct
```

**What gets stored.** `correction_m` and `half_width_m`, keyed by
`track_id:car_path`. Not the absolute position — see the note above about stall
assignment. Stored as a rolling mean over up to the last 8 observations, with
the count kept so the widget knows how much to trust it.

**When an observation counts.** Both transitions must occur in the same visit to
the lane, in that order, with a plausible `metres_per_pct` in hand, and with a
resulting `half_width_m` between 1 m and 20 m. Anything outside that is a
telemetry glitch or a reset-to-pits teleport, and is dropped silently. A stop
where the driver reverses back into the box will produce several transitions;
take the **first** enter and the **last** exit of the visit, which brackets the
whole manoeuvre correctly.

**A full drive-through calibrates just as well as a stop.** You do not have to
stop in the box for the transitions to fire — driving straight past your own
stall during an out-lap produces both. So a practice session calibrates the
widget without the driver doing anything deliberate.

**How confidence changes the display.**

| Observations | Target | Green zone | Displayed |
| --- | --- | --- | --- |
| 0 | `DriverPitTrkPct` raw | default ±1.0 m | bar drawn normally, no distinction |
| ≥1 | corrected | learned `half_width_m` | identical, but zones now match this car |

Deliberately *not* a visible confidence indicator. A "calibrating…" badge is
noise on the one screen where noise is expensive, and the uncalibrated reading
is good enough to act on — that is the whole point of using the sim's number as
the base.

**Storage.** A new file, `%APPDATA%\race\pit-stalls.toml`, separate from
`race-overlay.toml`. The overlay config is hand-editable user settings; this is
machine-written learned data, and mixing the two means a rewrite of learned data
can lose a hand edit. Format:

```toml
[["spa:mercedesamggt3evo"]]
correction_m = -0.42
half_width_m = 4.1
observations = 6
```

Written at most once per pit visit, on the exit transition. Unreadable or
corrupt file: start empty, log a note, carry on. Never a hard failure — a
learned-data file must never be able to stop the overlay from starting.

---

## The widget

A vertical capsule that fills as the car closes on its box. **Full is the box.**
There is no midpoint to read against and no marker to find — just a level rising
toward a target band at the top, which is a shape a driver can take in without
looking away from the crew.

```
     +------+            +------+            +------+
     |::::::| <- target  |######| <- the box |######|
     |      |    band    |######|            |######|
     +------+            +------+            +------+
     |      |            |######|            |######|
     |      |            |######|            |######|
     |      |            |######|            |######|
     |######|            |######|            |######|
     |######|            |######|            |######|
     +------+            +------+            +------+
      amber               green               amber
    2.5 m SHORT          IN BOX            1.4 m LONG

    still coming        on the marks       driven through
```

**The fill is linear in distance.** It rises at a constant rate for a constant
road speed, so its motion is a direct read of how fast the car is closing. An
earlier draft magnified the last few metres; the level then appeared to
accelerate into the target exactly where a driver is trying to judge a stop,
which is the opposite of useful.

**Overshooting fills the capsule rather than draining it.** A level on its own
cannot tell a car that has gone too far from one that is still coming, and a bar
that emptied as the car drove through its marks would read as a bar still
waiting for it. So a full capsule always means "at the box or through it" and a
partial one always means "still coming"; colour and the readout say which.

**Colour** comes from the existing palette in `ui/mod.rs` — `SIGNAL` green,
`CAUTION` amber, `ALERT` red — so the widget reads as part of the same product
rather than as a stock gauge. It is judged on distance alone, so being short and
being long are treated exactly as harshly as each other:

- **Green:** inside the stall box. `half_width_m` when learned, ±1.0 m by
  default. Being anywhere in green means the crew will work.
- **Amber:** out to 3 m. Recoverable — a small correction, or the crew reaches
  you slower.
- **Red:** beyond 3 m. Keep coming, or reverse.

**The confirmation lamp.** `PlayerCarInPitStall` is ground truth and outranks
everything computed. When it is true the capsule is full and green with `IN BOX`
beneath it, regardless of what the computed error says. If the arithmetic and
the sim ever disagree, the sim wins visibly — the driver should never sit in a
served box looking at an amber bar.

**The readout.** One line under the bar: `2.5 m SHORT`, `1.4 m LONG`, or
`IN BOX`. One decimal place, which is about the honest resolution of the
measurement. Omitted entirely when there is no metres scale (above).

**Full scale is 10 m, not the 30 m appearance range.** These are deliberately
different numbers. The bar is a closeness gauge rather than a position along the
lane, and because the fill is linear, its full scale is also its zoom: the box's
own band is its half-width against that figure — the top tenth of the capsule at
10 m, but a thirtieth of it at 30 m, which would leave the only metres a driver
is actually placing the car in as a sliver. The cost is that the capsule sits
empty for the first stretch after it appears, which is the correct thing for it
to say: a car twenty metres from its box is not close to stopped.

**Geometry.** 120 × 566 px at `scale = 1.0`, being capsule (120 × 520), gap,
readout (30 px type). No card and no header, matching Radar Bars — this is an
ambient indicator, not a panel.

**Default position** `[1680.0, 240.0]` — a tall capsule needs a column rather
than a strip, and the right-hand side keeps it clear of the Relative and
Standings stack while staying inboard of the screen edge. As with every panel
this is a one-time seed; wherever it is dragged is what persists.

---

## When it appears and when it goes away

A small state machine per visit to the lane, because "am I within 30 m of my
box" alone is true twice per visit — once coming in and once leaving.

```
Away ──(on pit road)──> Approaching ──(|error| ≤ 30 m)──> Visible
                              ▲                              │
                              │                    (in stall AND stopped)
                              │                              │
                              └────────── Done <─────────────┘
                                            │
                              (leaves pit road) → Away
```

- **Visible** requires: on pit road (`TrackLocation::ApproachingPits` or
  `InPitStall`), `|error| ≤ 30 m`, and the visit not yet `Done`.
- **Done** latches on one condition only: the sim says the car is in its stall
  *and* it has stopped (under 0.5 m/s). That is the moment the stop begins and
  the bar has nothing left to say — no clutter through the fuelling or the drive
  out.
- **Stopped, not merely in the stall.** `PlayerCarInPitStall` goes true while the
  car is still rolling in, so hiding on the flag alone would take the bar away
  during the very seconds it is being used to place the car.
- **Nothing about driving past the box ends the visit.** An earlier draft also
  latched `Done` once the car was 6 m beyond its marks. That is wrong:
  overshooting and reversing back in is the moment the widget is worth the most,
  and that rule put it away exactly when the driver started to need it. So an
  overshoot keeps the bar, reads as `LONG`, and empties back toward the box as
  the car comes back.
- **A drive-through never reaches `Done`,** since it never stops. It needs no
  special case: the bar shows, sweeps through, and disappears on its own once
  the box is more than 30 m behind. The same fallback covers a car whose sim
  publishes no `Speed`, where "stopped" cannot be detected at all.
- Leaving pit road resets to **Away**, so a second stop the same lap (a penalty
  served immediately after a stop) gets a fresh bar.
- `visible = false` in config hides it always, like every other panel.
- Layout mode (`--layout`, existing) shows it with a static demo error so it can
  be dragged into place without being in a pit lane. This is not optional — a
  panel that only appears during a pit stop is otherwise impossible to position.

---

## Defaults, and what happens when each input is missing

Every one of these is a real state, not a hypothetical.

| Missing | Behaviour |
| --- | --- |
| `DriverPitTrkPct` absent or zero | **The widget never appears.** There is no target and nothing honest to draw. Logged once per session so the cause is discoverable. This is also the state in a session with no pit lane at all. |
| `CarIdxLapDistPct` absent | Never appears. Already a required var for other widgets. |
| `PlayerCarInPitStall` absent | Bar works from arithmetic alone; no `IN BOX` lamp, no calibration ever recorded, and the visit ends by running out of range rather than by stopping. Degrades, doesn't break. |
| `Speed` absent | Falls back to `TrackLength` for the metres scale, and the visit can no longer end on "stopped in the box" — it ends by running out of range instead. |
| `TrackLength` absent too | Bar drawn in percentage space, readout omitted. |
| `TrackID` or `CarPath` absent | Calibration is not stored (no key), but is still used within the session it was observed in. |
| `pit-stalls.toml` unreadable | Start empty, note on stdout, carry on. |
| Spectating | Never appears. The camera car's stall is not your stall, and `DriverPitTrkPct` describes your own entry. This is consistent with `plans/spectator-focus.md`, which keeps own-car-only readings on the player. |
| Not in the car (`IsOnTrack` false) | Never appears. |

Defaults chosen so no configuration is required:

| Setting | Default | Why |
| --- | --- | --- |
| `visible` | `true` | It hides itself 99% of the time; there is nothing to opt out of. |
| `pos` | `[1680.0, 240.0]` | A column on the right, clear of the Relative/Standings stack. |
| `scale` | `1.0` | House rule: mockup pixel size. |
| `range_m` | `10.0` | What an empty bar means, and therefore its zoom. Deliberately shorter than the appearance range — see [The widget](#the-widget). The only knob worth touching: raise it for a bar that starts moving sooner, lower it to magnify the final metre. |
| `appear_range_m` | `30.0` | Not configurable. Roughly two car lengths past the neighbouring box — far enough to be on screen and settled before the level starts to move, near enough that it isn't up for the whole lane. |
| `green_half_width_m` | `1.0` | Used only until the box is learned; deliberately tighter than a real stall box so the uncalibrated state errs toward "get it closer". |
| `amber_half_width_m` | `3.0` | About a car length. Beyond this the crew is genuinely inconvenienced. |

---

## Changes, file by file

| File | Change |
| --- | --- |
| `telemetry/session_info.rs` | Add `DriverInfo.driver_pit_trk_pct`, `WeekendInfo.track_length` + `track_id`, `Driver.car_path`. Add `parse_track_length` (`"3.70 km"` → metres) beside the existing `parse_percent` / `parse_session_seconds`, with the same "unparseable is `None`, not zero" contract. |
| `telemetry/pit_stall.rs` | **New.** The whole model as pure functions plus one small stateful tracker: `wrap_error_pct`, `MetresPerPctEstimator` (rolling median), `StallCalibration` (the enter/exit state machine and its rolling mean), `VisitState` (the appear/Done machine), and `PitStallReading` — the value the widget draws. No egui, no I/O. |
| `telemetry/pit_stall_store.rs` | **New.** Load/save `%APPDATA%\race\pit-stalls.toml`, keyed `track:car`. Reuses `config::config_path`'s directory logic; add a small `config_dir()` helper there rather than duplicating the `APPDATA` fallback. |
| `telemetry/snapshot.rs` | Add `PitStallSnapshot { error_m: Option<f32>, error_pct: f32, in_box: bool, green_half_width_m: f32, visible: bool }` and hang it off `TelemetrySnapshot`. |
| `telemetry/session.rs` | Find `Speed`. Thread the new session-info fields through `SessionInfoCache`. Drive the trackers in `build_snapshot` alongside the existing `trackers.pit_loss` / `trackers.tyre_latch` — this is exactly the shape those already have. |
| `ui/pit_stall.rs` | **New.** The widget. `draw(ui, snapshot, config)`, returning early when not visible, in the shape `ui/radar_bars.rs` already uses. |
| `ui/mod.rs` | Nothing new expected; `gradient_rect_h`, `paint_text`, `Metrics`, `SIGNAL`/`CAUTION`/`ALERT` all already exist. Add a shared zone-colour helper only if the black box wants it too. |
| `config.rs` | `PitStallConfig` + field on `OverlayConfig` + the position/threshold `default_*` fns, matching `RadarConfig` exactly. |
| `app.rs` | One more `draggable_panel` call and one more clone into `save_config`'s write-back list. |
| `demo.rs` | A `PitStallSnapshot` in the fixed snapshot, showing a mid-amber reading, so `--demo` shows the widget alongside the other four. |
| `README.md` | Fifth bullet in the widget list; the `pit-stalls.toml` file in Configuration. |

Every new and touched `.rs` file goes through the `ms-rust` skill before it is
written, and carries the `// Rust guideline compliant <date>` header the rest of
the crate uses.

---

## Testing strategy

The reason for putting the whole model in `telemetry/pit_stall.rs` with no egui
and no I/O is that all of the following are then plain unit tests, in the same
style as the tests already in `relative.rs` and `pit_model.rs`:

- `wrap_error_pct` across the start/finish line, both directions, and exactly at
  ±0.5.
- `MetresPerPctEstimator`: rejects zero-delta ticks; a single wrapped tick does
  not move the median; converges on a known scale from synthetic ticks.
- `StallCalibration`: enter/exit produces the expected correction; exit without
  enter is dropped; multiple enters take first-enter/last-exit; an implausible
  half-width is dropped; the rolling mean of 8 behaves.
- `VisitState`: appears once, latches `Done` on the way out, does not reappear,
  resets when pit road is left, and handles a drive-through.
- `parse_track_length`: `"3.70 km"`, `"3700 m"`, `""`, `"unlimited"`, garbage.
- The metres-scale fallback ladder: measured → track length → none, and that the
  "none" case really does suppress the readout rather than emitting a number.
- Zone selection at each boundary, including exactly on it.
- `pit_stall_store`: round-trips; a corrupt file yields an empty store and no
  error.

Beyond unit tests, the acceptance check is `--demo` against the other four
widgets for visual language, `--layout` for placement, and then one real
practice session: drive through the box without stopping and confirm the bar
sweeps and vanishes, then stop and confirm `IN BOX` agrees with the crew
actually working.

---

## Build phases

Each phase ships something usable on its own.

**Phase 1 — the bar, uncalibrated.** Session-info fields, `Speed`, the error
computation, the measured metres-per-pct with both fallbacks, the visibility
state machine, the widget, config, `app.rs`, `demo.rs`. No calibration, no file
on disk — the green zone is the ±1.0 m default and the `IN BOX` lamp comes
straight from `PlayerCarInPitStall`. This is already the whole feature for a
driver; everything after it is sharpening.

*Ships:* a working pit stall bar at every track, first stop, no setup.

**Phase 2 — calibration within a session.** `StallCalibration`, learning the box
edges from the in-stall transitions and correcting the target and green zone.
Held in memory only. Because a drive-through calibrates, this is usually live
before the first real stop of a race weekend.

*Ships:* the green zone now matches this car's actual box rather than a
constant.

**Phase 3 — calibration across sessions.** `pit_stall_store.rs`, the
`pit-stalls.toml` file, rolling mean over 8 observations, the corrupt-file path.

*Ships:* correct from the first lap at a track you've raced before.

Phase 1 is the one that has to be right. 2 and 3 are each small and can be
deferred without leaving anything half-built.

---

## Explicitly out of scope

- **Lateral position.** How far left or right the car is in the box. There is no
  telemetry for it, and inventing one from yaw would be a guess drawn to one
  decimal place.
- **Approach speed / braking guidance.** A "brake now" cue is a different
  feature with a different failure mode; this widget describes position only.
- **Finding a box you can't see.** The "which one is mine" problem in a 60-car
  field is real but is a different widget — an arrow or a lane map, not a bar.
  The 30 m appearance range is chosen partly so this widget doesn't half-solve
  it.
- **Other cars' stalls.** `DriverPitTrkPct` is the player's own; there is no
  published equivalent for the rest of the field.
- **Audio.** Crew Chief already owns the driver's ears in this stack.
- **A visible calibration state.** Decided against above, deliberately.
