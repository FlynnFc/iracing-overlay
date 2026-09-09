# Radar Bars — overlap, not arithmetic

A redesign of the existing Radar Bars widget. Same two capsules in the same
place; a different thing drawn inside them. The widget stops being a *time
axis with numbers on it* and becomes a *picture of where the other car is*.

## Table of contents

- [The problem](#the-problem)
- [Principles](#principles)
- [What the sim will and will not tell us](#what-the-sim-will-and-will-not-tell-us)
- [From milliseconds to car lengths](#from-milliseconds-to-car-lengths)
- [The widget](#the-widget)
- [Closing rate](#closing-rate)
- [Colour](#colour)
- [The double-count bug](#the-double-count-bug)
- [Defaults, and what happens when each input is missing](#defaults-and-what-happens-when-each-input-is-missing)
- [Changes, file by file](#changes-file-by-file)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## The problem

The current bar shows `248` and `82` against a 500 ms axis. To turn that into
a driving decision the driver must, mid-corner:

1. remember the unit is milliseconds of relative time, not metres;
2. multiply by their own speed to get metres — `82 ms × 55 m/s ≈ 4.5 m`;
3. divide by a car length they have to know from memory — `≈ 1 car`;
4. compare that to how far they want to move over.

Four steps, one of which is a multiplication. That is the "cognitive power"
cost. Worse, three of the four inputs are not on screen.

Two further faults, both visible in the screenshot that prompted this:

- **There is no "me" on the bar.** The player is an unmarked midpoint. Overlap
  — the actual question — is the relationship between two cars, and only one
  of them is drawn.
- **A car past `range_ms` pins to the end of the bar** rather than leaving. A
  car 800 ms back and a car 500 ms back draw identically, at the extreme
  position, which is the position that should mean "as far as this bar goes".

---

## Principles

1. **Answer the question that is actually being asked.** In a wheel-to-wheel
   moment there is exactly one: *is there a car in the space I want to move
   into?* Everything the widget draws either answers that or is noise.
2. **Geometry over notation.** A shape next to a shape is read pre-attentively,
   in peripheral vision, with no decoding. A number is read foveally and
   decoded. The redesign spends its budget on geometry and, by default,
   prints no numbers at all.
3. **Never lie about precision.** Same rule as the Pit Stall bar. A car
   outside the drawn range leaves the drawn range; it does not pin to the end
   and imply contact.
4. **Invisible unless it's the moment.** Unchanged from today.

---

## What the sim will and will not tell us

Everything below is already read by this crate — no new SDK variables.

| Signal | Where it already comes from | Use here |
| --- | --- | --- |
| Relative-time gaps, signed seconds | `session.rs` builds `radar_gaps_secs` off the player's `LapCurve` | Longitudinal separation, per car |
| `CarLeftRight` | `session.rs:1070` | The only lateral signal that exists. Says *a* car is on a side, never which, never how far laterally |
| `Speed` (m/s) | `vars.speed`, already in `TelemetrySnapshot::speed_mps` | Converts seconds of gap into metres |

**There is no lateral distance in the SDK.** "How far over can I move" cannot
be answered directly, and this plan does not pretend otherwise. What it can
answer is the question that actually gates the move: *how much of that car is
beside me right now* — longitudinal overlap. A car with no overlap is a car
you can move across in front of or behind; a car overlapping your door is not.

---

## From milliseconds to car lengths

The obvious conversion is the player's own speed — at wheel-to-wheel range the
two cars are within a few km/h of each other, so `gap_secs × speed_mps` is
metres to well within the precision the widget draws.

**It was not used.** That conversion collapses exactly where the radar matters
most: at a standing start, or crawling out of a chicane, speed goes to nothing
and every car on the bar piles onto the player. The separation is instead
measured from the two cars' own track positions against the track's length,
which is exact and speed-independent:

```
separation_m = wrapped(other_lap_dist_pct − me_lap_dist_pct) × track_length_m
```

Both inputs are already read for other widgets, so this cost nothing. The
speed-based estimate survives only as a fallback for a session that publishes
no track length, and even then at a nominal speed rather than a live one.

A car length then turns metres into the unit a driver actually thinks in:

```
overlap_cars = separation_m / car_length_m       // car_length_m default 4.7
```

Either way the point stands: **metres are honest at low speed and
milliseconds are not**. Rolling out of a chicane at 60 km/h, a car 82 ms back
is 1.4 m away — half a car length, side by side. The old bar drew that
identically to 82 ms at 250 km/h, which is 5.7 m — fully clear. Same pixels,
opposite decisions.

---

## The widget

Same two capsules, same position, same "invisible when clear" rule. Inside,
the axis is no longer time — it is **metres of track, centred on your own
front axle-to-rear axle box**, spanning `±3` car lengths (about ±14 m).

```
        LEFT BAR                     what it means
  ┌──────────────┐
  │  ╌╌╌ ╌╌╌ ╌╌╌ │   ← faint tick, 2½ car lengths ahead
  │              │
  │  ╌╌╌ ╌╌╌ ╌╌╌ │   ← faint tick, 1½ car lengths ahead
  │  ▛▀▀▀▀▀▀▀▀▜  │   ← YOU: two bumper bars exactly one car length
  │              │     apart, fixed at the middle, always drawn
  │  ▟█████████▙ │   ← the lit band: the part of him level with you
  │  ██████████  │   ← the other car, one car length tall
  │      ▓▓      │   ← the tail: where he was 0.6 s ago
  │  ╌╌╌ ╌╌╌ ╌╌╌ │
  └──────────────┘
```

The three things that make it readable without arithmetic:

- **Your car is on the bar.** A fixed frame — two heavy bumper bars a car
  length apart — at the centre. It never moves, and it is the reference every
  other mark is read against. Deliberately *not* a filled box: a box here is
  the same shape as the cars around it, and "which of these blocks is me" is
  the one question the widget must never provoke.
- **The other car is drawn at its true length.** One car length tall, on the
  same scale as your frame. So *visual* overlap is *actual* overlap, and "can
  I move over half a car length?" is answered by looking at how much of the
  block sits between your bumper bars — not by reading a number.
- **The overlapping part is lit.** Strictly redundant, since the frame already
  marks your bodywork, and worth it anyway: it turns a comparison of two edges
  into a single lit band whose height *is* the overlap.

Ticks at each whole car length give the half-length judgement a ruler without
adding text.

A car beyond ±3 lengths is **not drawn as a block**. It gets a thin pip at the
end of the bar — "something is out there, closing" — which cannot be confused
with an overlapping car. This replaces today's clamp-to-the-end behaviour.

Numbers are **off by default** (`show_numbers = false`). When enabled, one
small readout per marker, in metres, not milliseconds.

---

## Closing rate

The block carries a **tail**: a streak leaving the block's far edge, covering
the ground that car crossed in the last 0.6 s.

- Long tail pointing down = they are dropping back, you are pulling clear.
- Long tail pointing up = they are coming, and fast.
- No tail = a stalemate, locked alongside. This is itself a decision: hold
  your line, nothing is about to resolve.

Rate as *length and direction of a streak* needs no legend and no unit. It is
the same reason a speedometer needle beats a digital readout for "am I
accelerating".

The streak leaves the block's *far* edge rather than its centre, so even a
slow drift shows something instead of hiding under the block it belongs to.

Implementation: a short ring buffer of `(session_time, separation_m)` per
side/slot in the existing `RadarSmoothing` struct, which already holds
per-side state across ticks. One guard matters more than the arithmetic: a
slot holds *a role* ("nearest car ahead on this side"), not a car, so when the
car filling it changes, the separation jumps. Measuring across that jump would
report tens of m/s and draw a tail the length of the bar, so a sample implying
more than 40 m/s discards the window instead.

---

## Colour

Today colour encodes direction (amber ahead, green behind). Once your own car
is drawn on the bar, **position already encodes direction** — a block above
your slot is ahead, below is behind, and no one has ever needed a colour to
tell them that. That frees the strongest channel on screen for the thing that
actually varies in urgency:

| State | Colour | Meaning |
| --- | --- | --- |
| Clear, > 1 car length from your slot | Dim slate | Present. Not your problem yet. |
| Within 1 car length, not overlapping | Amber | About to be your problem. |
| Overlapping your slot at all | Red, plus the bar's inboard edge lit as a solid rail | There is a car in that space. Do not move. |

The lit rail matters: it is a single high-contrast vertical line at the edge
of your vision on the side the car is on. It reads as a wall. That is the one
signal that should survive being seen at the very edge of peripheral vision
with your eyes on the apex.

---

## The double-count bug

`build_radar` (`session.rs:1690`) hands the *same* `nearest_ahead_behind` pair
to both sides when `CarLeftRight::CarLeftRight` is reported. The screenshot
shows the result: `248`/`82` on the left bar and `248`/`82` on the right. Two
cars are rendered as four.

Fix: when both sides report, take the two nearest cars overall and place one
on each bar rather than duplicating the pair. The *count* then stops being
wrong, and count is what a driver reacts to first.

Which car goes to which bar is still inferred — the SDK gives no way to know —
but not arbitrarily. A coin flip here is actively dangerous: it can tell you
the car on your left is ahead when he is behind. So the assignment is made by
**continuity**: whichever pairing leaves each bar closest to the gap it was
already showing wins. In the common case the second car arrives while the
first has been alongside for a second or more, so the first keeps its bar and
the new one takes the other. Only with no history on either side — both cars
arriving on the same tick — does it fall back to arbitrary-but-stable.

---

## Defaults, and what happens when each input is missing

| Input | Missing behaviour |
| --- | --- |
| `TrackLength` | Metres fall back to the relative-time gap at a nominal 40 m/s. An estimate, but a car drawn roughly right beats an empty bar next to a car that is there. |
| Either car's `CarIdxLapDistPct` | Same fallback. |
| `car_length_m` | Config default `4.7` (GT3-ish). Configurable per taste; not per car, because the SDK does not publish car length. Clamped to at least 1 m so a hand-edited config cannot divide the axis by nothing. |
| `CarLeftRight` clear | Unchanged: draw nothing. |
| Gap history shorter than 0.6 s | No tail drawn until there is history. A missing tail reads as "stable", which is the safe default. |

New `[radar]` config keys, all with working defaults:
`car_length_m = 4.7`, `range_cars = 3.0`, `show_numbers = false`. `range_ms`
stays, now meaning only how close a car must be to register at all. The
retired `danger_ms` — and `range_m` before it — are simply ignored, so an
existing `race-overlay.toml` keeps loading rather than failing to parse and
silently resetting every panel's saved position.

---

## Changes, file by file

| File | Change |
| --- | --- |
| `telemetry/radar.rs` | Everything works on a `Contact { gap_secs, gap_m }`. Adds `two_nearest`, `assign_sides`, `clear_gap_metres`, `threat` → `Clear`/`Close`/`Overlapping`, and `SeparationHistory` for the closing rate. Pure functions, unit-tested, no UI or telemetry types — matches the module's existing charter. |
| `telemetry/snapshot.rs` | `RadarSide` holds two `Option<RadarCar>` instead of two `Option<f32>` of milliseconds; `RadarCar` carries separation, gap and closing rate. |
| `telemetry/session.rs` | Builds each contact's metres in the loop that already computes its seconds; `build_radar` fixes the double-count; `GapSmoothing` gains the separation history the tail needs. |
| `ui/radar_bars.rs` | The redesign: player frame, true-length blocks, lit overlap, ticks, tails, threat colouring, edge rail, off-range pips. |
| `ui/mod.rs` | Retire `RADAR_AHEAD`/`RADAR_BEHIND`; add the three threat colours. |
| `config.rs` | New keys above, back-compat for the old ones. |
| `demo.rs` | Demo snapshot gains a speed and asymmetric per-side cars, so `--demo` exercises overlap, non-overlap and a tail rather than the symmetric pair it shows today. |
| `README.md` | Rewrite the Radar Bars bullet and the `[radar]` config notes. |

---

## Testing strategy

Pure-function unit tests in `telemetry/radar.rs`, in the style already there:

- `separation_metres` is honest at low speed (the 60 km/h chicane case above).
- `overlap_state` boundaries: exactly one car length apart is `Close`, not
  `Overlapping`; zero separation is `Overlapping`.
- Closing rate signs: approaching from behind is positive, dropping back
  negative, steady is zero within tolerance.
- `build_radar` with `CarLeftRight` reports two distinct cars, not one pair
  twice.

Visual verification through `--demo`, which is what it exists for.

---

## Build phases

1. **Metres.** `Contact` + `clear_gap_metres` + `threat` + snapshot/session
   plumbing + the double-count fix. No visual change yet; tests prove the
   numbers.
2. **Geometry.** Player frame, true-length blocks, car-length ticks, off-range
   pips. This alone removes the arithmetic.
3. **Threat colour + edge rail.** The peripheral-vision layer.
4. **Tails.** Closing rate. Last because it is the only piece needing history,
   and the widget is already a large improvement without it.

## Settled during the build

Recorded because each overturned something written above.

- **Metres come from track positions, not from speed.** See
  [From milliseconds to car lengths](#from-milliseconds-to-car-lengths).
- **The player is a frame, not a filled slot.** Drawn as a box in the first
  pass, and against the demo snapshot it read as a third car. Bumper bars
  cannot be confused with a marker.
- **The overlapping band is lit.** Discovered by accident in the first pass —
  a faint slot fill happened to lighten the overlapping part of a car — and
  kept, because it reads better than either edge comparison alone.
- **The tail leaves the block's far edge.** Drawn from the block's centre at
  first, which hid every tail shorter than half a car length — i.e. every
  ordinary one.
- **Range and detection are separate settings.** `range_cars` is what the bar
  draws; `range_ms` is what registers at all. Cars between the two are pips.

---

## Explicitly out of scope

- **True lateral distance / a top-down 2D radar.** The SDK does not expose
  other cars' lateral position. Any 2D radar drawn from `CarLeftRight` would
  be inventing the second axis, which breaks principle 3.
- **Per-car car lengths.** Not published by the SDK; a config value is honest,
  a guessed lookup table is not.
- **More than one car per side per direction.** Three cars abreast is a real
  situation the SDK cannot describe well enough to draw.
