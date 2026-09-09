# Dash widget

A DDU-style dashboard panel, reproducing the reference screenshot (`design
mocks/dash.png`): engine-status tiles, live delta, session strip, tire grid,
gear tower, lap times, driver-aid chips and a fuel box, all on one card.

Unlike the other widgets this one does **not** use the overlay's semantic
accent set: it imitates a hardware display, so its palette (pure-green delta,
magenta/yellow lap times, near-white bezels) is sampled from the mock itself.

## Why

The overlay covers race context (Relative, Standings) and situational alerts
(Radar, Pit Stall) but nothing shows the car itself: gear, revs, delta, aids,
tires and fuel at a glance. That is what a dash is for.

## How the layout was arrived at

Every dimension is **measured off the mock**, not estimated: the mock is
491x297 with a 21 px margin, so its pixels are the widget's design pixels and
`Metrics::px` scales them. Box edges were found by scanning the image for its
near-white (`#E5E5E5`) strokes, glyph sizes from the cap heights of the
numerals inside them.

Two consequences worth knowing before changing anything:

- **Every grouping is a legend box** — a thin bright outline with its title set
  into a gap in the top edge — not a filled panel. `legend_box`/
  `stroke_box_with_top_gap` draw them, and a legend wider than the straight run
  between the corners tightens the corners rather than shrinking itself.
- **The mock's numerals are wider than any face this overlay carries.** Barlow
  Condensed SemiBold is the only bold face here and is far narrower at the same
  cap height. The lap times — the one place the shortfall is glaring — are
  therefore set at the mock's cap height and *tracked out* to the mock's span
  by `paint_tracked`. Everything else accepts the narrower figure rather than
  growing taller than the mock.

## Seeing it

Screen capture cannot read this window: it is layered, topmost and drawn by
OpenGL, so a screen `BitBlt` leaves it out, the same blit with `CAPTUREBLT`
returns it at a fraction of its alpha, and `PrintWindow` returns black. The
overlay therefore screenshots itself:

```
race-overlay.exe --demo --screenshot=<path>
```

which draws `SCREENSHOT_AFTER_FRAMES` frames (long enough for the SVG icons to
load), reads the framebuffer back through the renderer's own GL context,
composites it onto black — what the mock sits on — and quits.

## Layout (design pixels at scale 1.0)

One card, 491x297 (content 448x254 inside a 21 px margin). Two outer columns
171 wide — left at x0, right at x277 — with an 83-wide middle column between
them at x183, clear of both by 12 px. Three rows: y0 h47, y58 h138, y207 h47.

1. **Top row** — left: a plain box of four engine-status tiles; middle: the
   delta plate and its trend bar; right: a plain box with the `RACE` clock,
   `POS`, `LAP` and `TIME` columns, each sized to the wider of its value and
   its label so they cannot collide, and the whole row squeezed if it would
   overrun (an hour-plus clock).
2. **Main row** — left: the `TIRES` legend box, a 2x2 grid around a faint
   crosshair (pressure in PSI at the outer corner, the two edge temperatures
   with the tire graphic between them facing the middle, compound pill on the
   crosshair); middle: the gear box with the revs *inside* it, the shift
   strip, the white speed box, then two condition rows (air/track temperature
   with the flag chip pinned right, humidity/wind); right: the `LAP TIMES`
   legend box — predicted white, last yellow, best magenta.
3. **Bottom row** — driver-aid chips `TC`, `TC CUT` (blue), `ABS` (gold), `BB`
   (red, and the one wide chip at 66), `MAP` (green), then the `FUEL` legend
   box (litres, avg per lap, laps of fuel left).

## Data sources and absence behavior

Everything is per-tick player-car data. The rule throughout: **a channel the
car does not publish disappears rather than showing a fake zero** (same
principle as the Weather readings row and the black box's adjustment rows).

| Element | Source | When absent |
| --- | --- | --- |
| Delta | `LapDeltaToBestLap`, gated on `LapDeltaToBestLap_OK` | grey `-.---` on a flat plate |
| RACE clock | `RelativeMeta::countdown_secs()` (same as Relative footer) | elapsed time |
| POS | focus row's `class_position` from standings | `-` |
| LAP | `ui::lap_text` (same as Relative/Standings) | bare lap |
| TIME | `SessionTimeOfDay` as HH:MM | column hidden |
| Gear | `Gear` (-1 = R, 0 = N) | `N` |
| RPM / speed | `RPM`, `Speed` | `0` |
| Shift strip | YAML `DriverCarSLFirstRPM/SLShiftRPM/SLLastRPM/SLBlinkRPM` | empty strip |
| Warning chips | `EngineWarnings` bits: pit limiter, water temp, oil, fuel pressure | chips drawn unlit |
| Flag chip | `SessionFlags` → GRN/YEL/BLU/WHT/CHK/RED | grey `—` |
| Tire pressure | `{LF..}pressure` (absent live), else garage `{LF..}coldPressure` — in practice always the latter | `--.-` |
| Tire temps | `{LF..}temp{L,M,R}` (absent live), else carcass `{LF..}tempC{L,M,R}` — last-stop values, not live | `--°` |
| Tire graphic | coloured by the temperatures above | neutral grey |
| Compound chip | `PlayerTireCompound` (0 = DRY, else WET) | chip hidden |
| TC / TC CUT / ABS / BB / MAP | `dcTractionControl`, `dcTractionControl2`, `dcABS`, `dcBrakeBias`, `dcFuelMixture` | chip hidden (car has no such control) |
| Fuel | `PitService` fuel level + measured per-lap; laps = level ÷ per-lap | AVG/LAPS show `-` until a racing lap is measured |
| Air/track/humidity/wind | existing `WeatherSnapshot` + `RelativeHumidity` | `--` / hidden |

Chips whose variables exist but read zero still draw (a TC of 0 is a
setting); only a missing variable hides a chip, mirroring `CarAdjustments`.

**Tires: no live temps or pressures exist.** `--dump-all-vars` against a live
session settled this — see the correction at the top of
`plans/tire-info-data.md`. The live stream carries no `{LF..}temp{L,M,R}` and
no `{LF..}pressure` (those exist only in recorded `.ibt` files), which is why
reading them left the grid empty. What it carries is the carcass family
`{LF..}tempC{L,M,R}` and the garage cold pressure, **both of which only change
at a pit stop**. The grid therefore shows last-stop rubber, the same as the
sim's own box, and is honest about it rather than implying live data. A zero
counts as unpublished, so the fallbacks are tried on zero as well as absence.

## Decisions

- **A panel, not a black box page.** It is glanced at continuously while
  driving, like the Radar — not paged into. Draggable, scalable, tray- and
  settings-toggleable like every panel.
- **Defaults**: `visible = true`, `scale = 1.0`, seeded at `[705, 430]` on
  1920x1080 — under the mirror line, above the Radar's own seed; a one-time
  seed like every panel position.
- **Delta colors**: negative (faster than best) on a pure-green plate with
  black ink, positive on red, unavailable a flat grey plate. The bar under it
  is the lap-times pair — magenta for the best lap on the left, yellow for the
  last on the right — lit outward from the centre, ±2 s full scale.
- **Shift strip**: green fill from `SLFirst` to `SLLast`, the whole run turning
  red at `SLBlink`. The red block at the strip's end is the limiter zone,
  always drawn so the strip reads as a scale.
- **Status tiles light, not appear**: all four are always drawn, dim but still
  in their own hue — the way a dark dashboard lamp still shows its lens
  colour — so the cluster is stable and a bright tile is the news.
- **Predicted lap** = session best + current delta; needs both, else a dashed
  placeholder. It's a projection, so it takes the plain white row.
- **Spectating/team-mate**: player scalars describe the player's sim, so in
  a spectator/team-mate seat the widget still draws whatever the sim
  publishes — same behavior as the black box's live pages.
- **Demo data** is representative (mid-stint GT3: 4th gear, 6250 rpm, small
  green delta) rather than the template's zeros, because the reference
  screenshot is an unpopulated template and zeros would hide layout bugs.

## Touched files

- `telemetry/snapshot.rs` — `DashSnapshot` (+ `ShiftLights`, `FlagState`,
  `EngineAlarms`) and a `dash` field on `TelemetrySnapshot`.
- `telemetry/session.rs` — new `DashVars` group probed by name; assembly in
  `build_snapshot`; new candidates in `CANDIDATE_VARS` for `--dump-vars`.
- `telemetry/session_info.rs` — shift-light RPMs off `DriverInfo`.
- `ui/dash.rs` — the widget; registered in `ui/mod.rs`.
- `config.rs` — `DashConfig` (pos/visible/scale), reset, panel plumbing.
- `tray.rs` — `Panel::Dash`.
- `ui/settings.rs` — a Dash page (Show, Scale, Reset).
- `app.rs` — draw block, plus `--screenshot` (`take_screenshot`, `write_png`)
  and `DemoOptions`.
- `main.rs` — the `--screenshot=<path>` flag.
- `demo.rs` — fixture data.
