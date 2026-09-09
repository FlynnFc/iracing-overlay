# Black box status border — flags and BOX BOX around the whole widget

> **Status**: built and tested. `SessionFlags` → `CourseFlag`, the
> `box_this_lap` fuel trigger (`endurance::box_this_lap`), the `resolve_status`
> priority and the `paint_status_frame` border+plate, plus the manual
> `Control::BoxBox` toggle on the Strategy page. Verified by screenshot. The
> only open item is that the border currently always pulses on the pit
> approach with no off-switch — add one if it distracts.
> **Frame redesign (2026-09-07)**: the border now sits flush on the widget's
> own edge (no clear gap), the plate straddles the top edge, and on the
> Relative the border's left limb widens into a filled band across the whole
> status gutter — set into a paint slot reserved before the rows, so the
> off-track/penalty markers draw *on top of* the border rather than the
> border wrapping around them. Also fixed: the fuel-based BOX call is gated
> on `Seat::Driving`, so a spectator's dead fuel var can no longer raise
> BOX BOX every lap.

The black box should wear the session's state on its edge: a coloured border
around the whole widget with a label plate on the top edge — CAUTION, LAST
LAP, FINISH, or **BOX BOX** — so the one thing that matters right now is
visible without reading a single row. The reference is the yellow-bordered
panel in the screenshot, but with the state named on top rather than left
implicit.

Two sources drive it: iRacing's course flags (yellow, white, checkered), and
a box call — set by the fuel calc when this is the lap to pit, or called by
hand from the Strategy tab.

## The border

A shared **status frame** wrapping every page of the black box — the Relative
(page one) and the `card_frame` pages alike, so the border is the widget's,
not a page's. Drawn as:

- A straight-edged stroke around the black box's outer rect, in the state's
  colour, a few pixels proud of the existing card so it reads as a frame and
  not a recolour. No border at all in the ordinary green-flag, no-call state —
  the widget looks exactly as it does today until something is worth saying.
- A **label plate on the top edge**, centred, straight-edged (house style — no
  slanted tabs), the state's colour filled with a contrasting ink: the text
  *on* the border, as asked. `BOX BOX`, `CAUTION`, `LAST LAP`, `FINISH`.
- **BOX BOX comes on steady** when the call is live — it is a clear state, not
  a nag. It **starts pulsing only within ~400 m of pit entry** (the same
  full-to-dimmed flash the Faster Class card uses), so the flash means "here,
  now, turn in" rather than "sometime this lap". The flags are always steady.

  Distance to pit entry is measured to the player's own pit stall
  (`DriverPitTrkPct`, already cached as `pit_stall_pct`) along the lap, from
  the current `lap_dist_pct` and `track_length_m` — the SDK publishes the
  stall, not the lane entry, so the stall is the reference and 400 m is
  comfortably before it. Where either the stall or the track length is
  unknown, the border still lights steady; it just never reaches the pulse,
  which is the safe degradation.

One border, one plate: the frame shows a single state, chosen by priority
below. Implemented as one helper the `draw` function wraps its whole body in,
taking the resolved state — so neither the Relative path nor the card path
repeats it.

## The states, and which wins

The global `SessionFlags` telemetry variable carries the course flags in the
same `iracing_telem::flags::Flags` bitfield the per-car penalty read already
uses (`telemetry/relative.rs:653`); nothing reads the *session-wide* one yet,
so that is the one new telemetry read. The box call is derived (below). One
resolved `BlackBoxStatus` enum, highest priority first:

1. **Finish** (`CHECKERED`) — the race is over; nothing else matters. White
   border, `FINISH`.
2. **Box** — a box call is live. Caution-red border, pulsing `BOX BOX`. Above
   the flags because it is the actionable one: boxing under a yellow is
   exactly what you do, so a live call outranks the caution behind it.
3. **Last lap** (`WHITE`) — white border, `LAST LAP`.
4. **Caution** (`YELLOW` / `CAUTION`) — amber border, `CAUTION`.
5. **None** (green or unflagged) — no border, no plate.

The priority is a single ordered match, easy to re-tune; this order is the
default, argued above.

## The box call — when to say BOX BOX

Two independent triggers, OR'd into the Box state:

**Auto, from the fuel calc.** The pit window already knows the last lap the
tank reaches (`telemetry/pit_window.rs`, `endurance::laps_remaining`, and the
per-lap burn). The call is: *this is the lap you must pit on, and you are
committed to it.* So it lights when

- the current lap is the last the fuel margin allows before the tank runs
  under its reserve, **and**
- you are **past half way round it** (`fuel_use.lap_fraction >= 0.5`).

The half-lap gate is the point you raised: early in the lap there is still a
decision to make (save harder, reassess), but once you are through the lap's
back half the call is settled — come in at the end of *this* lap — so that is
when the border lights and the driver commits to the pit entry. It clears the
moment the car is on pit road or the stop is taken (`pit_service.on_pit_road`,
or the lap counter advancing past a completed stop).

Honest-or-silent, like every projection here: with no measured burn yet, or
under caution where the burn figure is meaningless, there is no auto call —
the border simply doesn't light. A made-up "box now" is the one false
instruction that actually costs a race.

**Manual, from the Strategy tab.** A `Call BOX BOX` toggle on the pit-window
page (a new `Control::BoxBox`), so a spectator or the driver can raise the
border by hand — "just to be there", as asked, whether or not the fuel maths
agrees. It is a **reminder, not a command**: it arms nothing on the sim, it
only lights the border. It stays lit until toggled off or the stop is made.
Stored as black-box state beside the page/cursor, not in the config — a box
call is a moment, not a setting.

When both triggers agree the border is simply Box; there is no double state.

## Changes, file by file

**`telemetry/session.rs` + `snapshot.rs`**
- Read the session-wide `SessionFlags` var and resolve a `CourseFlag`
  (`Green`/`Yellow`/`White`/`Checkered`) onto the snapshot.
- Add `box_this_lap: bool`, computed from the fuel/lap-fraction rule above,
  next to the existing endurance projection so it shares its inputs and its
  confidence gate.

**`telemetry/pit_window.rs`**
- The "last lap the tank allows" is already latent in the window maths;
  surface it as the single lap number the auto-call compares against, unit
  tested (exact-fit, one-lap-short, plenty-in-hand, and the half-lap edge).

**`ui/blackbox/mod.rs`**
- A `status_frame` helper drawing the border and top plate for a resolved
  `BlackBoxStatus`, wrapping the whole of `draw`.
- `resolve_status(snapshot, box_called)` folding the course flag and the two
  box triggers into the priority above — pure, unit tested per case.
- `Control::BoxBox` on the Strategy page, and the manual flag on `BlackBox`
  state with its toggle and its auto-clear.

**`ui/blackbox/pages.rs`**
- The `Call BOX BOX` toggle in `pit_window`'s controls.

## Testing strategy

- `resolve_status`: each flag alone, box alone, and the contests — box under
  yellow is Box, checkered over box is Finish, white over yellow is Last Lap.
- The auto box-call rule: lights only in the lap's back half of the committed
  lap; silent with no burn and under caution; clears on pit road.
- Glyph/plate: label and colour per state, the way the row marks are tested.

## Explicitly out of scope

- Blue-flag / meatball / other per-car flags — those stay in the Relative's
  own row gutter, which already carries them; this border is the *session's*
  state and the *player's* box call, not per-car penalties.
- Arming the stop from the manual call. BOX BOX is a reminder; the pit box is
  set through the existing Fuel/Tyres controls (and, later, team-sync writes).
- Audio. A border is a glance; a beep is a separate feature and a separate
  can of worms.
