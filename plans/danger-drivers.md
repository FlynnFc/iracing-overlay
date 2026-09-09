# Danger drivers — marking who to watch, across sessions

> **Status**: built and tested. `CarSnapshot.cust_id`, the `[danger]` config
> map keyed by customer id, and the right-click context menu (`Click::Danger`
> → `app::apply_danger_mark`, persisted). Covered by config round-trip and
> toggle-logic tests. Marks survive across sessions and other config writes.
> **Mark redesign (2026-09-07)**: the gutter corner badge is gone — the mark
> now lives in the row itself, in the bar slot the class bar used to hold
> (`relative::paint_danger_mark`): amber bar for caution, amber full-row-height
> for warning, alert-red full height plus a red row wash for severe. The level
> reads from how much of the row the mark claims, not from a glyph.

A spectator (or driver in the garage) sees someone drive dangerously and wants
the overlay to remember it: a mark beside that driver in the Relative, now and
in every future session, so the moment they show up on the radar of a later
race the warning is already there. Three levels, because "slightly sketchy" and
"actively dangerous" want different weights.

## The interaction

**Right-click a Relative row** opens a small context menu on that driver:
*Danger: off / caution / warning / severe*. Picking a level marks them; the
row grows a danger glyph at the chosen weight. Right-click again to change or
clear it. That is the whole UI — no list to manage, no separate screen; the
mark is set where the driver is seen.

Right-click is otherwise unused in the Relative (the black box has no
right-click), so it costs nothing already there. The menu draws in any seat —
a spectator marking the field is the main case, but a driver parked in the
garage can mark too.

## The three levels

Straight-edged glyphs in the row's status gutter, escalating — no slanted
tags (per the house style):

- **Caution** — a hollow marker in a muted amber. "Give them room."
- **Warning** — a filled `!` in amber. "Watch this one."
- **Severe** — a filled `!` in the caution red the app already uses for real
  alerts. "Expect the worst."

One glyph, one column, read at a glance in peripheral vision — the same bar
the off-track tally and other row marks live in. The colour *is* the level; a
driver should not have to read a number.

## Persistence — the point of the feature

A mark is worthless if it forgets the driver between races, so it is keyed on
the thing that survives a session: the driver's **iRacing customer id**, not
their `car_idx` (which is per-session) or their name (which repeats and can be
changed). Stored in `race-overlay.toml`:

```toml
[danger]
"123456" = "severe"
"789012" = "warning"
```

- A new map on `OverlayConfig`, `danger: BTreeMap<u32, DangerLevel>` (cust-id
  → level), `skip_serializing_if` empty so an unused feature stays out of the
  file. Written through the same `OverlayConfig::update` path every other
  setting uses, so marking a driver mid-session survives a `--bind` write and
  vice versa.
- Read every frame while building the Relative rows: a row whose driver's
  cust-id is in the map carries its level.

This needs the one piece of plumbing the Relative lacks: **`CarSnapshot` must
carry the driver's `cust_id`**. It has `car_idx` and `driver_name` but not the
stable id; `DriverMeta::user_id` already holds it in the session cache, so
`build_snapshot` just copies it onto the row (as it does the flair id and the
rest). Without it there is no session-stable key and the whole feature can't
persist — so this is the first build step.

## Build shape

1. **The stable key.** Add `cust_id: Option<u32>` to `CarSnapshot`, populated
   in `build_snapshot` from `DriverMeta::user_id`. Pure plumbing, no behaviour
   yet — lands and is asserted in the session tests.
2. **Storage.** `DangerLevel` enum (`Caution`/`Warning`/`Severe`) and the
   `danger` map on `OverlayConfig`, with round-trip and default-empty tests
   like every other config addition.
3. **The mark.** The Relative row reads its level from the map and draws the
   glyph in the status gutter; unit-test the glyph choice per level, the way
   the other row marks are tested.
4. **The menu.** Right-click a row opens the context menu; a pick writes the
   map through `OverlayConfig::update`. This is the only step touching input
   plumbing — the Relative already collects clicks (`blackbox::Click`), so the
   right-click rides the same path.

Steps 1–3 stand without step 4: a `danger` map hand-written into the TOML
already draws marks, which is how the drawing is tested before the menu
exists.

## Optional later — sharing marks across the team

The danger map is local by default, which is right: one member's read on a
driver is their own. But a `DangerMark { cust_id, level }` sync event would let
a crew chief's marks reach the team's drivers, the same way the fuel target
does — set once, seen by everyone, survives a reconnect in the ledger. Left
out of the first build deliberately: local marking proves the feature, and
shared marks raise "whose call wins" questions (last-write, or per-member?)
that aren't worth answering until the local version is in use. When it lands it
is one more event variant, not new machinery.

## Explicitly out of scope

- Auto-detection of dangerous driving. The mark is a human judgement; inferring
  it from incident counts would be a different, noisier feature.
- A management screen. The Relative row is the one place marks are set and
  seen; a driver worth un-marking is one you can right-click.
- Notes or reasons per driver. A level is the whole vocabulary — anything
  longer isn't readable at racing speed anyway.
