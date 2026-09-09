# Making the Dash move with the car

## What this is for

The Dash panel sits over the real dash in a GT3. The real one shakes with the
cockpit; ours is nailed to the screen. The gap between the two is what stops
the panel reading as part of the car.

The goal is presentation, not accuracy. The reader already tolerates the dash
moving — tiles are padded and glyphs are sampled into a normalised grid, so a
twenty-pixel shift changes nothing (`a_padded_tile_survives_the_dash_moving`).
Nothing here is needed to read tyre values. It is worth building only if it
looks right.

## Why the offset is measured, not predicted

The dash does not move relative to the car at all. What moves is the camera,
through iRacing's g-force camera movement, and that is scaled by the driver's
own `app.ini` settings, their field of view, their seat position and the
per-car camera. Predicting pixels from accelerations means calibrating every
one of those, and it is open loop: the error has no way to announce itself.

The offset is already visible in the frames being captured. Measuring it is
closed loop, needs no telemetry and no configuration, and corrects itself every
frame. It also cannot tell the difference between a bump, a seat adjustment and
a camera change, which is the point — it does not need to.

Windows Graphics Capture reads the sim's own window and excludes our overlay,
so the feature being tracked stays visible even underneath our panel. This is
already relied on by the tyre reader.

## What to track

A fixed feature, not a value. A value's bounding box moves when the value does
— `99` becoming `100` shifts it sideways — and the panel would twitch on every
reading. The dash labels (`P FL`, `T RR`) are fixed text at high contrast and
never move relative to the car, which makes them the natural anchor.

## How

One strip of the dash, captured each frame, reduced to two one-dimensional ink
profiles: the sum of ink per column, and per row. The offset is the shift that
best lines those profiles up against a reference pair taken at calibration —
argmin of absolute difference over a bounded search.

Cheap enough not to think about. A 200x60 strip is 200 + 60 profile samples,
and a +/-20px search is around 8k operations per axis. Two-dimensional
correlation over the raw pixels would be a thousand times that, and buys
nothing: camera shake is overwhelmingly translation.

Translation only. Roll is ignored, which is a real approximation and probably
an invisible one at these amplitudes.

## Rate, and the part that might not work

The tyre reader samples at 10 Hz, which is right for temperatures and useless
here — shake is a several-hertz signal and 100ms of quantisation would look
like stuttering. Tracking needs to run at the overlay's own frame rate on its
own cheaper path, sharing the open `Capture`.

Latency cannot be removed. The chain is: the sim presents a frame, WGC hands it
over, we measure, and our panel moves on our next render. That is one to two
frames of trailing against a cockpit that is moving with none.

There is no way to know whether that reads as attached or as floating without
looking at it. So:

- Build it behind a setting, off by default.
- Give it a strength from 0 to 100%, applied to the offset before it is used.
  Half-strength tracking may well look better than full: it takes the edge off
  the disagreement while still tying the panel to the car.
- Judge it in the car, not in a screenshot.

## Applying it

- Exponential smoothing on the offset, to keep a single bad measurement from
  jolting the panel.
- Clamped to a sane excursion, so a lost lock cannot fling the panel across
  the screen.
- Decays back to the calibrated position when the feature cannot be found,
  rather than freezing where it was.
- Applied at draw time only. The configured position is what gets saved, so
  the panel does not wander between sessions.

## Settings

- `dash.track_cockpit` — off by default.
- `dash.track_strength` — 0 to 100%, default to be chosen by eye.

## Tests

The fixture makes this testable without a sim: crop it at a known offset, and
assert the tracker recovers that offset. Cover sub-pixel-free integer shifts in
both axes, a shift larger than the search window (must report no lock rather
than a wrong answer), and a blank strip.

## What could go wrong

- **Lock drifts onto a neighbouring feature.** Bounded search plus the
  excursion clamp; re-anchor when nothing is found.
- **It looks worse than a static panel.** Entirely possible. It is a setting,
  off by default, and the strength slider exists for this.
- **Cost while driving.** Profiles are cheap, but this adds a capture grab per
  overlay frame where there was one per 100ms. Measure the sim's frame time
  with it on and off before calling it done.

## Revision: the first cut wobbled

Driven, the profile-only tracker wobbled badly. Three causes, three changes:

1. **Whole-pixel answers.** Shake a few pixels tall, quantised to integers,
   is a staircase. Now: the compositor-tracker treatment — an auto-picked
   32x32 feature patch (highest gradient energy, the corner an artist would
   drop a track point on) matched around the coarse answer, and a parabola
   through the scores either side of the best placing the answer between
   pixels.
2. **Lock lost exactly at the peak of a bump.** Motion blur pushed the match
   score past the absolute threshold, the tracker eased home mid-strike and
   jumped back on the re-lock. Now: the lock test is relative — the best
   score against the median of every score tried — because blur raises all
   scores together and leaves the valley where it was. And a miss is ridden
   out for 12 frames before any easing; brief blur holds, only a sustained
   loss decays.
3. **Two clocks beating.** The reader publishes on its own 16ms tick, the UI
   draws on another; frames saw two steps or none. Now: eased a second time
   at draw, in step with what is actually shown.

Also: reading no longer pauses in layout mode. Dragging the panel aside to
compare against the sim's dash is when live values are wanted most; only the
following is disabled while positioning.
