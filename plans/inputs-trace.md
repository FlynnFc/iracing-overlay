# Inputs

A new panel: the last few seconds of throttle and brake as two traces
scrolling left, with the current pedal positions as bars and the steering
as a centre-zero bar with a degree readout. Brake segments where ABS was
active are marked. Every overlay in the survey has one; this is the shape
of ours.

## Table of contents

- [What it answers](#what-it-answers)
- [Data](#data)
- [The widget](#the-widget)
- [Sampling](#sampling)
- [Options](#options)
- [Defaults, and what happens when each input is missing](#defaults-and-what-happens-when-each-input-is-missing)
- [Changes, file by file](#changes-file-by-file)
- [Testing](#testing)
- [Build phases](#build-phases)
- [Out of scope](#out-of-scope)

## What it answers

Two questions, both asked *after* the corner rather than in it: *did I
trail the brake or stamp on it* and *was I already on the throttle when I
turned in*. That decides the design: the trace is the point, the live bars
are a courtesy, and nothing here needs to be read in the braking zone.

## Data

| Var | Type | Used for | Required |
| --- | --- | --- | --- |
| `Throttle` | f32 0..1 | trace, bar | yes |
| `Brake` | f32 0..1 | trace, bar | yes |
| `Clutch` | f32 0..1 | bar (off by default) | no |
| `BrakeABSactive` | bool | marks on the brake trace | no |
| `SteeringWheelAngle` | f32 rad | steering bar, degrees | no |
| `SteeringWheelAngleMax` | f32 rad | steering bar's full scale | no |
| `IsOnTrack` | bool | auto-hide | no |
| `SessionTime` | f64 s | the trace's x axis | already read |

`Throttle`, `Brake` and `Clutch` are the post-processing values the sim
drives the car with; the `*Raw` variants are the pedal before any
assists and are not used. None of the first six is in `vars_dump.txt`
yet: add them to the dump list and verify live, twice, before building.
The one thing to confirm by eye is the sign of `SteeringWheelAngle`
(the SDK says positive is anticlockwise, i.e. a left turn).

## The widget

One ink card at the panel's usual radius, default about 380 × 120 at
scale 1, in three regions left to right:

1. **Trace**, taking most of the width. Time runs left to right with
   *now* at the right edge; a window of `window_secs` (default 8). Two
   polylines: throttle in `SIGNAL` green, brake in `ALERT` red, each
   with a faint fill under it (alpha ~40) so a held pedal reads as area.
   Where `BrakeABSactive` was true the brake line is drawn in `CAUTION`
   amber for that run of samples, and a small amber `ABS` word sits at
   the top-right of the trace while it is currently active. Three
   hairlines at 0, 50 and 100 %, no labels. The trace's ground is the
   card's own tile fill, the same as the fuel tank's.
2. **Pedal bars**: one narrow vertical bar per enabled pedal — brake,
   throttle, and clutch when enabled — in the trace's colours, empty
   track in `text_tertiary` at low alpha. Straight-edged, 4 px radius.
   The bar's height is the trace's height, so 100 % lines up with the
   trace's top hairline.
3. **Steering**: a horizontal bar, centre-zero, filling left for a left
   turn and right for a right turn, in `text_secondary`; full scale is
   `SteeringWheelAngleMax` (half of lock-to-lock). Under it the angle in
   the mono face: `-124°` / `+38°`, with a tick at zero so straight
   ahead is unmistakable. No wheel picture: the bar is the system's
   vocabulary and reads at any size.

Nothing is slanted. No gear or speed readout — the Dash and In-Car pages
have those, and a duplicate here would just be more to look at.

## Sampling

The telemetry thread already ticks at iRacing's 60 Hz. The snapshot
carries only the **current** values (`InputsSnapshot { session_time,
throttle, brake, clutch, abs_active, steering_rad, steering_max_rad }`),
and the **UI keeps the ring buffer**, appending one sample per snapshot
it hasn't seen (keyed on `session_time`).

Why not in the telemetry thread: a ring published in every snapshot is
an allocation per tick, which the rest of the snapshot goes to some
length to avoid (see the `Arc<str>` note in `snapshot.rs`), and the
buffer is only wanted while the panel is drawn. Why keying on session
time is enough: the panel draws at the same 60 Hz pacing when shown, and
when a frame is late the x axis is time, not sample count, so the trace
stays time-correct with a coarser sample. The buffer is a `VecDeque` of
`window_secs × 60` samples, trimmed by time.

Session time going backwards (a new session, a replay scrub) clears the
buffer.

## Options

All on a new **Inputs** page of the settings window, applied live:

| Key | Default | Range |
| --- | --- | --- |
| `visible` | `true` | |
| `pos`, `scale` | as other panels | |
| `show_throttle` | `true` | |
| `show_brake` | `true` | |
| `show_clutch` | `false` | |
| `show_steering` | `true` | |
| `mark_abs` | `true` | |
| `window_secs` | `8` | 3–15, slider |
| `only_on_track` | `true` | |

Turning a pedal off removes both its trace and its bar. With both
throttle and brake off the trace region collapses and only the bars and
steering remain; with steering off the card narrows. The card's width is
whatever the enabled regions add up to, so no option leaves a hole.

## Defaults, and what happens when each input is missing

- `Throttle` or `Brake` absent → the panel draws the "waiting" placeholder
  in layout mode and nothing otherwise; a note on the console once per
  session, since this shouldn't happen in a driving session.
- `BrakeABSactive` absent → no ABS marks; the option is shown disabled
  with a hover note.
- `SteeringWheelAngle` absent → steering region hidden.
- `SteeringWheelAngleMax` absent or zero → full scale is the largest
  angle seen so far this session, never less than 90°, so the bar grows
  to fit rather than pinning.
- `IsOnTrack` absent → the panel never auto-hides.
- Demo: a synthetic 8 s sequence — brake ramp, trail-off with an ABS
  burst, throttle pick-up, steering sweep — so the screenshot shows every
  element.

## Changes, file by file

- `telemetry/session.rs`: the vars, the dump list, and an
  `InputsSnapshot` built each tick.
- `telemetry/snapshot.rs`: `TelemetrySnapshot.inputs: Option<InputsSnapshot>`.
- `config.rs`: `InputsConfig` with the keys above, default position
  bottom-centre, above the black box.
- `ui/inputs.rs` (new): the ring buffer (`InputTrace`), the three
  regions, the layout arithmetic that sizes the card from the enabled
  regions.
- `ui/settings.rs`: the Inputs page; `ui/mod.rs`: `pub mod inputs`.
- `app.rs`: register the panel (`draggable_panel("inputs", …)`), the
  tray tick, the on-track gate.
- `demo.rs`: the synthetic sequence.
- `README.md`: a paragraph under the widget list and the config keys.

## Testing

- Unit: `InputTrace` appends only unseen session times, trims by window,
  clears on time going backwards, and reports the sample runs where ABS
  was active as contiguous spans.
- Unit: steering full scale falls back to the largest seen, floored at
  90°.
- Unit: card width for each combination of enabled regions.
- Screenshot: `--demo` with defaults; with clutch on; with steering off.

## Build phases

1. Vars, dump list, live verification (steering sign, ABS var present).
2. `InputTrace` with tests; `InputsSnapshot` in the snapshot.
3. The panel with defaults only; demo sequence; screenshot.
4. Options page and the width arithmetic; the two other screenshots.
5. README.

## Out of scope

- A ghost/reference lap overlaid on the trace (iFL03). Practice analysis
  belongs in a telemetry tool.
- Steering-wheel images per wheel model (irdashies, iFL03). Decoration.
- Force-feedback clipping. Different data, different panel.
