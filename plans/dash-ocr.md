# Reading tyre data off the in-car dash

iRacing publishes no live tyre temperature or pressure (see the correction at
the top of `plans/tire-info-data.md`). The sim's *own* in-car dash does show
them, because it renders from physics state the SDK never exposes. So the only
route to live rubber data is to read the pixels.

This is the design for that. **Phase 1 is built and passed** — see the
result section below; phases 2 onward are still design.

## The one thing to decide first

iRacing withholds this data deliberately. Reconstructing it by reading the
screen is a workaround for a restriction the sim chose to impose, and it is
worth being clear-eyed that it could be viewed as circumventing that — the
same data by another route. There is no technical protection being broken and
nothing is injected into the sim, but it is a judgement call about the spirit
of the restriction, and it was Flynn's to make. **He made it: go ahead.**

## Why it is tractable here

Three things make this far easier than general-purpose screen OCR:

- **The font is fixed.** A given car's dash renders digits in one typeface at
  one size. Once the region is locked, this is not OCR — it is template
  matching over ten glyphs.
- **The region is static.** The dash does not move relative to the car, and the
  camera is fixed in the cockpit. It moves only when the seat position, FOV or
  camera changes, which is rare and detectable.
- **The values are constrained.** Tyre temps are 2–3 digits in a known range;
  pressures 2 digits and a decimal. Implausible readings can be rejected
  outright, and a value that jumps 40 °C in 50 ms is a misread, not a tyre.

## Capture: the iRacing window, not the screen

**This is the load-bearing decision.** Windows Graphics Capture (WGC) can
capture either a monitor or a *specific window*. It must be the window:

- Our own overlay is topmost and — per Flynn's own point — users park the Dash
  widget directly over the car's real dash in a GT3. A **monitor** capture
  composites the overlay on top, so we would be reading our own widget instead
  of the sim's. A **window** capture reads iRacing's swapchain and never sees
  our overlay at all.
- It also survives the overlay being moved, and works when other windows
  overlap.

`windows` 0.62 — already a dependency — carries the features needed, so this
adds **no new crates**: `Graphics_Capture` (WGC), `Graphics_Imaging`
(`SoftwareBitmap`), `Media_Ocr` (the built-in OCR engine), plus
`Win32_Graphics_Direct3D11` for the texture copy.

The frame arrives as a GPU texture. Crop on the GPU with
`CopySubresourceRegion` into a small staging texture, then map that — so only
the tyre region ever crosses to the CPU, not the whole 3440x1440 frame.

## Pipeline

```
WGC frame (GPU) ─► crop to region (GPU) ─► map staging ─► binarise
   ─► segment digit cells ─► classify glyphs ─► sanity-check ─► publish
```

Steady state runs at the sample rate; everything expensive happens once.

### Locating the region

Three tiers, cheapest first — Flynn's own suggestion is tier one and it is the
right instinct:

1. **Behind the Dash widget.** Users place the widget over the real dash, so
   the widget's own rectangle is an excellent prior. Search it first, expanded
   by ~50%.
2. **Lower-centre sweep.** If that misses, scan the bottom half of the frame.
   Run the Windows OCR engine over it *once*, find numeric clusters, and keep
   those whose geometry looks like a tyre grid — four values in a 2x2, or two
   rows of two, roughly symmetric about a vertical axis.
3. **Manual.** A settings mode that lets the driver drag a box over the dash.
   Always available as the fallback, because auto-detection will fail on some
   car or seat position and a widget that cannot be fixed by hand is a widget
   that gets switched off.

Once found, store the region **relative to the iRacing window's client rect**
so it survives a resize, keyed by car id (`DriverInfo.Drivers[].CarID`) — a
Ferrari 296's dash is not a Porsche 992's.

Re-locate when: the car changes, the window resizes, or confidence drops for
more than ~2 s (which also covers the driver changing seat/FOV).

### Reading the glyphs

Two stages, and the second is the one that runs hot:

- **Bootstrap (once per car, offline-ish):** run `Windows.Media.Ocr` over the
  located region across a few hundred frames. It is free, built in, needs no
  model shipped, and is accurate enough on clean synthetic text. Use its output
  to cut and **label glyph templates** — exactly the "use a bigger model to
  label for a smaller one" idea, and the right shape for this problem.
- **Steady state:** per-digit nearest-neighbour against those templates.
  Normalise each cell to a fixed box, compare by sum-of-absolute-differences.
  Ten templates, a dozen cells — microseconds, no model, no allocation.

A tiny CNN is the obvious alternative but is not needed: with a fixed font at a
fixed size, template matching is exact rather than approximate, and it fails
*legibly* (a low-confidence cell is a number we can refuse to publish rather
than a plausible-looking wrong digit).

Reject a frame when: any cell's best match is above a distance threshold, the
value is outside a plausible range, or it moved impossibly fast. A refused
frame keeps the previous value and marks it stale — the widget must be able to
say "I have not read this for 2 s", because a stale tyre temp presented as
live is worse than none.

## Performance

Target was 20 Hz; the budget suggests far more is available. Estimates, to be
measured rather than trusted:

| Stage | Est. per sample |
| --- | --- |
| WGC frame acquire (already-produced frame) | ~0.1–0.3 ms |
| GPU crop + staging map of ~320x140 | ~0.3–0.8 ms |
| Binarise + segment | ~0.1 ms |
| Classify ~12 cells | ~0.05 ms |
| **Total** | **~1 ms** |

At 20 Hz that is ~2 % of one core; 60 Hz stays under 6 %. The one-time locate
with Windows OCR over half a frame is ~50–200 ms and happens once per car.

The real risk to frame time is not CPU but **GPU contention** — WGC and the
staging copy touch the same device the sim is rendering on. Mitigations: crop
on the GPU so the copy is tiny, never capture at more than the sample rate, and
run the whole thing on its own thread that drops frames rather than queueing.
This needs measuring against the sim's frame time before it ships.

## What will actually break it

Honest list, roughly by likelihood:

- **VR.** There is no desktop image of the dash to read. The feature simply
  does not apply, and must disable itself rather than misreport.
- **The MFD page.** Most GT3 dashes show tyres on *one* page the driver cycles
  to. If it is not up, there is nothing to read. `dcDashPage` is published and
  can gate the reader — read it, and only sample when the tyre page is up.
- **Camera.** Chase/TV cameras have no dash. Gate on the cockpit camera.
- **Per-car work.** Every car needs its own region and glyph templates. The
  bootstrap is automatic but it is still a per-car onboarding step, and rain
  or night lighting may need a second template set.
- **Motion blur and low resolution.** A dash at the far end of a 4K triple
  setup may be too few pixels per digit to segment reliably. Detect this at
  locate time and refuse rather than guess.

## Phase 1 result (2026-08-31): passed, and the target is easier than assumed

Built `capture.rs` (Windows Graphics Capture, per window) and a
`--capture=<process.exe>[,x,y,w,h]` diagnostic. Run against a live session on
a 3440x1440 triple setup, in a Mustang GT3:

**Both gating questions answered yes.**

- **Window capture excludes this overlay.** The Dash widget was parked at
  `[1400, 980]`, directly over the car's dash, and the captured frame shows the
  sim's dash completely unobstructed — no overlay pixels at all. The frame was
  demonstrably live, not cached (the dash's `STINT` field ticked between
  captures). The architectural bet in this plan holds: capture the window, and
  our own panels can never contaminate the read.
- **The digits are legible**, with room to spare.

**What the dash actually shows** — better than hoped. It is not a page that has
to be cycled to: pressures and temperatures are on screen together, in eight
labelled tiles.

```
P FL 1.59   P FR 1.59        T FL 34   T FR 34
P RL 1.59   P RR 1.59        T RL 34   T RR 34
```

Measured off the capture:

| Property | Value |
| --- | --- |
| Digit height | 23 px (median), 12 px for the decimal point |
| Digit width | 16 px (median) |
| Glyph luma | ~110 on a ~50 tile — mid-grey on dark blue, **not** white |
| Binarise threshold | ~85 works cleanly |
| Glyphs segmented | 16, by 8-connected components, no tuning |

Three consequences for the phases below:

- **Auto-location gets much easier.** Every value tile carries a text label
  (`P FL`, `T RR`) directly above it. Those labels are fixed, high-contrast
  anchors — find `P FL` once and the whole grid is located by geometry, which
  is far more robust than hunting for numeric clusters. Windows OCR only has
  to read four short labels, not the values.
- **Segmentation needs no OCR at all.** Threshold at ~85, take connected
  components, and the digits fall out. The only wrinkle is adjacent digits
  occasionally merging into one component (a 28 px-wide blob where 16 px is
  one digit), which a fixed-pitch split handles.
- **Do not key on "bright".** The glyphs are mid-grey; a naive white-text
  threshold finds nothing. This cost an hour — the first threshold tried was
  115 and returned zero components.

Still unmeasured: the per-sample cost of the cropped read at rate, and whether
GPU contention shows up in the sim's frame time. Phase 2 should measure both
before raising the sample rate.

## Phase 2 result: tiles work, Windows OCR does not

Built `ocr.rs` — `TyreGrid` (per-block tile geometry), `Reader::read_tiles`
(one capture, eight in-memory crops, laid out as a mosaic and recognised in a
single call) and plausibility filtering. Measured on a live Mustang GT3 at
3440x1440, 20 Hz, ~240 samples each.

| Approach | Per sample | Complete reads |
| --- | --- | --- |
| Whole grid, one region 690x180 | 12.1 ms | 0% (40% partial) |
| Pressures only, 215x180 | 5.8 ms | 0% (83% partial) |
| **8 tiles, mosaic, gap 16** | **3.6 ms** | **16%** (27% partial) |
| 8 tiles, mosaic, gap 26 | 4.4 ms | 1% |
| 8 tiles, 2x upscaled | 9.7 ms | 3% |

**The tiling was worth it and the recognition is not.** Cutting the grid into
eight small tiles and reading them in one call took the cost from 12 ms to
3.6 ms — 7% of a core at 20 Hz — and the geometry is now right: the values
that *do* come back are correct and land in the right corners.

But Windows OCR never gets close to reading all eight at once. It truncates
(`1.83` read as `1.0`), drops decimal points (`1.81` as `181`) and clips
digits (`81` as `8`). Plausibility bounds catch the nonsense, which is why a
wrong number never reaches the widget — but a reader that returns a full set
one time in six cannot drive a tyre display.

Findings worth keeping, all measured rather than assumed:

- **Do not pre-process.** Hard binarising to black-on-white took the read rate
  from 40% to 2%; a greyscale contrast stretch took it to zero. General OCR is
  trained on natural anti-aliased text and does best handed exactly that.
- **Do not upscale.** 2x cost three times as much for no gain.
- **Layout matters more than size.** Eight values in a 2x4 mosaic read far
  better than eight stacked in a column, because recognition is line-based and
  values side by side look like a line of numbers. Widening the gaps between
  them undoes that.
- **Per-call overhead is ~1 ms** regardless of region size, so eight separate
  calls are no cheaper than one big one. One call over a mosaic is the shape
  that wins.

**Conclusion: the template classifier is required, not optional.** This is
what the design anticipated, now with evidence. The groundwork is all in
place: digits are 23 px tall, threshold ~85 separates them cleanly, connected
components segment them with no tuning, and `TyreGrid` already says exactly
where each value sits. What remains is to cut the glyphs, label them once per
car — Windows OCR is good enough for *that*, since bootstrapping can take all
the frames it likes and keep only the confident ones — and match by
sum-of-absolute-differences at runtime.

## Phase 3: a learned font, not a model

`glyphs.rs` reads the dash by shape instead of by recognition. It segments the
marks in a tile and matches each against templates it learned earlier — and the
templates come from the OCR, which is reliable enough to *label* a glyph even
though it is not reliable enough to read a dash.

Against the saved fixture, taught once from the front-left tile:

```
learned 3 templates from "159"
read [Some("1.59"), Some("1.59"), Some("1.59"), Some("1.59")]
```

Four for four, from one label, in a debug build. That is the whole thesis:
**there is nothing to recognise here.** One font, one size, one colour, ten
shapes. A general model is solving a problem this does not have, and paying
milliseconds for it.

### Two things that were quietly wrong

- **The tiles were clipping the digits.** Hand-measured boxes cut the glyphs at
  19x11 where they are really 23x17, so every reading was of a mutilated digit.
  This is likely a large part of why the OCR numbers were so poor.
- **The dash moves.** Every bump shifts the whole cockpit by several pixels, so
  a tile sized to its value loses it under braking. Tiles are now cut with room
  around the value and the reader finds it inside the window: it picks the
  tallest band of rows (the value is far larger than its label) and the group of
  marks nearest the middle (so a neighbouring value straying into the padding is
  discarded). Verified to +20 px of shift against the fixture.

### Working without the sim

`--capture` writes a PNG; `assets/fixtures/dash-gt3.png` is one, and
`--ocr-image=<png>,x,y,w,h,column_pitch,row_pitch[,text]` teaches and reads it.
The unit tests run against that same fixture, so segmentation and matching are
regression-tested on real pixels with no cockpit required — which matters
because a live dash is never twice the same: the tyres are always at a
different temperature.

## Phases

1. ~~**Spike:** WGC window capture + dump a cropped PNG.~~ **Done** — see
   above. `capture::Capture` is the reusable piece; `--capture` is the
   diagnostic.
2. ~~**Manual region + Windows OCR.**~~ **Done, and it rules Windows OCR out
   of the hot path** — see the phase 2 result above. `--ocr-tiles` is the
   harness; nothing is published into `DashSnapshot` yet, deliberately.
3. ~~**Template classifier.**~~ **Built** — `glyphs::Font`, see above. Still
   to do: bootstrap it from the live OCR rather than a hand-typed label, and
   add staleness so a stopped reader is visibly stale rather than frozen.
4. **Auto-locate**, tiers 1 and 2, keyed per car.
5. Wire into the Dash's tyre grid, which then shows live rubber with a
   staleness indicator, falling back to the last-stop telemetry values.

Stop after stage 1 if the digits are not legible on Flynn's setup — everything
after it is wasted otherwise.
