# Hide in garage — getting the overlay out of the way of the setup screen

When the garage screen is up, the widgets are covering a UI the driver is
actively reading and clicking. They should get out of the way on their own,
and come back the moment the car is on track.

## Table of contents

- [What happens today](#what-happens-today)
- [The signal](#the-signal)
- [The rule](#the-rule)
- [When the input is missing or stale](#when-the-input-is-missing-or-stale)
- [Changes, file by file](#changes-file-by-file)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## What happens today

Visibility is one expression, `app.rs:312`:

```rust
let should_show = self.demo
    || self.layout_mode()
    || !self.config.only_show_when_iracing_focused
    || self.focus_tracker.is_focused(&self.config.iracing_process_name);
```

Everything in it is about *which window* has focus. iRacing is focused while
you sit in the garage, so all five panels are drawn over the setup screen —
Relative and Standings squarely over the tyre and fuel columns most setups are
edited from.

The gate is already the right shape for this. It is a single test, evaluated
per frame, and `draw_panels` is skipped wholesale when it fails. Nothing about
the widgets needs to know.

## The signal

`IsInGarage` — a `bool` telemetry var, "1=Car in garage physics running". It is
already named in `CANDIDATE_VARS` under the `state` group (`session.rs:274`),
so `race-overlay --dump-vars` run from the garage confirms it on the rig
before any of this is written.

It is true while the car sits in the garage with the garage screen up, and
false the moment you take to the track. That is exactly the question being
asked, which is why it beats the alternatives:

- `IsOnTrack` ("player is in the car, physics running") is false in the garage
  *and* false on the grid before a rolling start, during a tow, and while
  watching a replay. Hiding on it would hide the overlay at moments the driver
  wants it most.
- `CamCameraState & irsdk_IsSessionScreen` covers every out-of-car sim screen,
  not just the garage. Broader, and unverified — see
  [out of scope](#explicitly-out-of-scope).

## The rule

Hide the panels when **all** of these hold:

1. `hide_in_garage` is on in the config (default on).
2. The newest snapshot says `in_garage`.
3. That snapshot is fresher than `SNAPSHOT_STALE` (2 s) — see below.

and **none** of these hold:

4. Demo mode.
5. Layout mode.

Demo and layout mode override for the same reason they already override the
focus check, and layout mode especially: `app.rs:199` says outright that
layout mode "is a thing you turn on for a minute in the garage". A garage rule
that hid the panels you had just turned on in order to drag them would make
the feature that positions the widgets unusable.

**Hiding is delayed, showing is immediate.** The condition must hold
continuously for `HIDE_DELAY` (250 ms) before the panels go; the instant it
clears they come straight back. A hardcoded constant, not a setting — a
quarter-second is below noticing when you open the garage, and asymmetry means
the delay can never cost you an overlay on the way out of the pit box. If
`IsInGarage` turns out to be perfectly clean across the transition the delay
costs nothing; if it chatters for a frame during the load, nothing flickers.

## When the input is missing or stale

**The var is absent.** Looked up through the existing optional-var path
(`Option<Var>`), like `cam_car_idx`, `session_flags` and every other
non-essential var. Absent means `in_garage: false`, which means today's
behaviour exactly. A sim build that stops publishing it degrades to an overlay
that never hides, not to one that never shows.

**No snapshot has ever arrived.** `self.latest` is `None` before iRacing
connects. `None` means not hidden.

**The snapshot has stopped arriving.** This one matters and is not
theoretical. On `DataUpdateResult::SessionExpired` the telemetry loop returns
(`session.rs:381`) and sends nothing further, but the app keeps its last
`TelemetrySnapshot` forever. Leave a session from the garage and the final
snapshot ever sent says `in_garage: true` — a rule reading it directly would
hide the overlay until the process was restarted.

So the app timestamps snapshot arrival and treats the garage flag as false
once the newest one is older than `SNAPSHOT_STALE` (2 s, four times the 500 ms
`DATA_WAIT` and far beyond the 60 Hz tick that feeds it). Staleness only
un-hides; it never hides.

In practice `only_show_when_iracing_focused` covers most of this case already
— iRacing exiting takes its window with it — but it defaults on, not
always-on, and a rule that depends on a different setting being enabled to
avoid a stuck overlay is a bug waiting for the first person who turns it off.

## Changes, file by file

**`telemetry/session.rs`**

- `TelemetryVars`: add `in_garage: Option<Var>` via `find("IsInGarage")`, in
  `find_all` (`session.rs:850`) alongside the other optional vars.
- `build_snapshot`: read it into the new snapshot field, defaulting to `false`
  when the var is absent.

**`telemetry/snapshot.rs`**

- `TelemetrySnapshot` (`snapshot.rs:507`): add `pub in_garage: bool`, doc'd as
  "the garage screen is up and the car is in it" rather than as a UI
  instruction — the snapshot describes the sim, and `app.rs` decides what that
  means for drawing.
- `demo::snapshot` gets `in_garage: false` with the rest of the mockup data.

**`config.rs`**

- `hide_in_garage: bool`, `#[serde(default = "default_true")]`, immediately
  after `only_show_when_iracing_focused` (`config.rs:56`) — same class of
  setting, same default shape.
- The `Default` impl (`config.rs:176`) and the defaults test (`config.rs:658`)
  follow.

**`app.rs`**

- `latest_at: Option<Instant>`, stamped in the same `try_recv` loop that sets
  `self.latest` (`app.rs:287`).
- A `const fn`-adjacent helper, `hidden_by_garage(&self, now: Instant) ->
  bool`, holding conditions 1–3 and the `HIDE_DELAY` edge tracking. Kept off
  the `should_show` expression itself so the delay state has somewhere to
  live and so it is unit-testable without a frame.
- `should_show` becomes the existing expression `&& !self.hidden_by_garage(now)`
  — with demo and layout mode already short-circuiting ahead of it, so
  conditions 4 and 5 need no restating.

**`README.md`**

- One line in the config section next to `only_show_when_iracing_focused`
  (README:111).

## Testing strategy

Unit tests, in the style already in these modules:

- `hidden_by_garage`: setting off → never hidden; no snapshot → not hidden;
  fresh snapshot with `in_garage` → hidden once `HIDE_DELAY` has elapsed and
  not before; snapshot older than `SNAPSHOT_STALE` → not hidden, even with
  the flag set; flag clearing → not hidden on the very next call, with no
  delay.
- Config: `hide_in_garage` defaults on, and round-trips through TOML.

Manual QA, for the parts only a live sim answers:

1. `race-overlay --dump-vars` from the garage, then from the car on track.
   `IsInGarage` reads 1 then 0. If it does not, none of the rest is worth
   building — this is phase 0 for a reason.
2. Sit in the garage. Panels gone. Open the setup tabs and confirm nothing is
   drawn over them.
3. Click Drive. Panels are back by the time the car is in the world.
4. Practice session: return to the garage mid-session, go back out. Hides and
   shows both ways.
5. Race weekend: the pit box is not the garage. Sit in the box during a race
   and confirm the overlay stays up — this is the regression that would hurt.
6. Turn layout mode on from the tray while in the garage. Panels appear and
   stay, so they can be dragged.
7. Leave the session from the garage, with `only_show_when_iracing_focused`
   turned off. The overlay comes back within ~2 s rather than staying hidden.

## Build phases

0. **Confirm the var.** `--dump-vars` in the garage and on track. No code.
1. **Everything else.** The var, the snapshot field, the config flag, the gate
   and its tests.

Not split further. The plumbing has no observable behaviour without the gate,
and the gate cannot exist without the plumbing; a phase boundary between them
would ship a field nothing reads.

## Explicitly out of scope

- **The Options screen and other sim menus.** The user-facing behaviour would
  be nice, but the signal for it — `CamCameraState & irsdk_IsSessionScreen` —
  is documented as "viewing the session screen (out of car)" and it is not
  established that it is set while the in-sim Options dialog is up. Adding it
  on an assumption risks the overlay vanishing during camera transitions or in
  replays. If it is wanted later, the honest first step is to add
  `CamCameraState` to `CANDIDATE_VARS` and watch what the bit actually does
  across the garage, Options, the results screen and a replay; the gate built
  here takes another disjunct without restructuring.
- **Per-widget garage behaviour.** All five panels hide together. The black
  box is the only arguable exception — cold pressures and fuel are garage-time
  numbers — but iRacing's own garage screen shows all of it better, in the
  window the panel would be covering.
- **Pausing the telemetry thread while hidden.** The snapshot stream is what
  tells the app it has left the garage.
- **Suppressing binds or Auto Fuel while hidden.** Binds are already polled
  regardless of visibility, deliberately (`app.rs:321`), and the same
  reasoning holds here: state the driver changed should not depend on whether
  a panel happened to be drawn.
