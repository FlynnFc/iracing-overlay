# Seat layouts — one panel arrangement for driving, another for watching

When you are in the car, the panels sit where they don't cover the mirrors and
the braking references. When you are spectating, none of that matters and the
screen is a broadcast: the same panels want to be somewhere else entirely.
Today there is one saved position per panel, so switching between the two means
dragging everything around twice per session.

## The rule

Two layouts, resolved from the `Seat` the snapshot already carries
(`telemetry/snapshot.rs:939`):

- `Seat::Driving` → the **driving** layout. The ordinary case, and demo mode's.
- `Seat::Spectating(_)` and `Seat::TeamMate(_)` → the **watching** layout.
  A team-mate's stint is watching by any practical measure: you are not in the
  car, and the screen is being read the way a spectator reads it.
- `Seat::OutOfCar` → **whichever layout was last active**, defaulting to
  driving. The garage, a tow and the wait between stints are transitions, not
  places; a layout that snapped every panel across the screen mid-tow would be
  motion nobody asked for. Stickiness also means the rule cannot flap: only a
  definite seat moves the panels.

No stale-snapshot special case is needed: seat is a property of the last
snapshot, and when snapshots stop coming the last known seat is the right
answer anyway (same reasoning as the garage rule's staleness guard, inverted —
nothing here hides anything, it only picks coordinates).

## What is stored

Each panel config gains one optional field beside `pos`:

```toml
[relative]
pos = [2460.0, 1009.0]
watch_pos = [60.0, 300.0]
```

- `watch_pos: Option<[f32; 2]>`, `#[serde(default, skip_serializing_if =
  "Option::is_none")]`. `None` means "no watching layout saved for this
  panel — use `pos`". Existing configs load unchanged and, until the first
  drag while watching, save unchanged: the feature is invisible until used.
- `pos` keeps its name and meaning (the driving layout). Renaming it would
  orphan every saved position in every existing file.

Scale, visibility, widths and every other setting stay shared between the two
layouts. Positions are the thing that differs between driving and directing a
broadcast; forking the rest doubles the settings surface for no asked-for
reason.

## How it behaves

- **Reading:** `draw_panels` (`app.rs:455` and friends) hands
  `draggable_panel` the active slot: `pos` while driving,
  `watch_pos.get_or_insert(pos)` while watching. `get_or_insert` is the whole
  seeding story — the first time a watching layout is looked at, it starts as
  a copy of the driving one, and only diverges when a panel is dragged.
- **Writing:** unchanged. A drag mutates whichever slot was handed out, and
  the existing settle-then-save path persists it. Dragging while spectating
  edits the watching layout; dragging while driving edits the driving one.
  That is also the editing story: there is no mode to pick, you arrange the
  panels in the situation you are arranging them for.
- **Layout mode and demo mode** edit the driving layout (seat defaults to
  `Driving` in both). Arranging the watching layout is done while actually
  watching, which is easy to be doing safely.
- **Reset all positions** (`config.rs:121`) clears every `watch_pos` back to
  `None` as well as reseeding every `pos`. A watching layout dragged
  off-screen is the same failure the button exists for.
- **The dash save merge** (`app.rs:956`) copies `dash.watch_pos` alongside
  `dash.pos`, or the next panel drag would silently drop a saved watching
  position for the dash.

The switch itself is a per-frame choice of which coordinates to hand the
panels — no animation, no window recreation. The panels jump once when the
seat resolves, which is the honest behaviour: the seat changed.

## Changes, file by file

**`config.rs`**

- `watch_pos: Option<[f32; 2]>` on all seven panel configs, plus the
  `Default` impls and `reset_positions`.

**`app.rs`**

- `OverlayApp` gains `active_layout: Layout` (a two-variant enum, `Driving` /
  `Watching`), updated each frame from the latest snapshot's seat per the
  stickiness rule above.
- A small helper on each panel config — or one free function
  `active_pos(pos: &mut [f32; 2], watch_pos: &mut Option<[f32; 2]>, layout:
  Layout) -> &mut [f32; 2]` — used at the seven `draggable_panel` call sites.
- The dash merge in `save_config`.

**`ui/settings.rs`**

- One line on the General page under the existing ticks, stating which layout
  is being edited when one has diverged — otherwise the second layout is
  invisible state, and a panel that "won't stay where I put it" is a bug
  report waiting to happen. Nothing else: no toggle, no per-layout pages.

## Testing strategy

Unit tests in the existing style:

- Layout resolution: `Driving` → driving; `Spectating`/`TeamMate` → watching;
  `OutOfCar` keeps the previous answer; the very first `OutOfCar` reads
  driving.
- Config: `watch_pos` round-trips; a file without it loads with `None`; a
  default config serialises without the key.
- `reset_positions` clears `watch_pos`.

Manual QA: drive a session (panels unchanged from today); spectate one and
drag a panel; swap between sessions and confirm each layout comes back; press
Reset all positions and confirm both layouts reseed.

## Explicitly out of scope

- A manual layout switch or hotkey. The seat is already known; a toggle is a
  piece of state to forget in front of a broadcast.
- Per-layout visibility, scale or any setting other than position.
- A third layout for replays or the garage. `OutOfCar` stickiness covers the
  garage; replays keep the seat they were entered from.
