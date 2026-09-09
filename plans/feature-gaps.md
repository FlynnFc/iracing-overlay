# Feature gaps against the other overlays

Surveyed 2026-09-04: the three open-source iRacing overlays with real
traction — **irdashies** (Electron/TS, the most complete), **iFL03**
(C++/Direct2D, the iRon lineage) and **RaceOverlay** (C#/WPF, a RaceLab
clone) — plus the catalogues of the two commercial ones, RaceLab and Kapps.
Per-widget notes with file paths are in the survey transcripts; this file
is the comparison against what `race-overlay` already has, and a
recommendation.

Streamer-only features (Twitch chat, heart rate, OBS browser sources, a
separate race-control window, dual-PC web server, setup hider) are listed
at the end and not recommended: this overlay is for the driver.

## Already covered, sometimes better

| Feature | Ours | Notes |
| --- | --- | --- |
| Standings with class sections, SOF, gaps, best/last | Standings | Only ours has the endurance strategy band and the grid header. |
| Relative with lap-status colour, iRating badge | Relative | Only irdashies and iFL03 also project the iRating change; ours does. |
| Car manufacturer marks | logos | irdashies has a similar sprite set; iFL03 PNGs. |
| Penalty / black-flag marker, off-track tally | gutters | None of the three tally off-tracks. Unique to ours. |
| Faster-class-approaching alert | Faster Class | iFL03 "Traffic" and irdashies "Faster Cars From Behind" are the same idea. Ours has the closing-rate line. |
| Radar / side-by-side | Radar Bars | irdashies only has `CarLeftRight` bars; iFL03 has a proper radar. Ours is to scale with overlap. |
| Fuel calc with auto-fuel | Fuel page | None of the three send pit-service commands. irdashies and iFL03 persist per-track consumption (see below). |
| Pit box countdown | Pit Stall | irdashies Pitlane Helper covers more (below). |
| Tyre wear/pressures from the last stop | Tires page | Same SDK limit everywhere; nobody has live temps. |
| Weather, wind relative to car | Weather | Equivalent. |
| Brake bias, ABS, TC, map | In-Car page | iFL03 DDU, RaceOverlay Electronics. |
| Driver flags | flags | Now done — see `driver-flags.md`. |

## Missing and worth doing

Ranked by value to a driver mid-race against cost. **S** is a day or
less, **M** a few days, **L** a week-plus or needs external data.

### 1. Race-flag banner — S

All three have it; RaceLab has two. A strip that shows the session flag
from `SessionFlags`: green, yellow (and waving), blue, white, checkered,
black/meatball/DSQ for the player, red, debris, plus the start-light
sequence gated on `SessionState`. iFL03's priority table
(`OverlayFlags.h`) is the reference. We already decode per-car black flags
for the gutters; this is the session-wide half. Could live as a thin bar
above the Relative rather than a new panel.

### 2. Delta to best lap — S

iFL03 (ring gauge), RaceOverlay (bar), irdashies (lap timer). Data is
`LapDeltaToSessionBestLap`, `LapDeltaToBestLap`, and the optimal-lap
variants, all with `_OK` validity flags. A bar plus a mono figure, hidden
in the pits and for the first 5 % of a lap (iFL03's rule). Trend colouring
over a short window. This is the one thing every driver-facing overlay has
that ours doesn't.

### 3. Shift lights — S

irdashies Tachometer, iFL03 DDU. We already parse
`DriverCarSLFirstRPM/ShiftRPM/LastRPM/BlinkRPM` and have `RPM`. A strip
of blocks lighting toward the shift point, blinking past it, in the theme's
colours. iFL03 also flashes on `BrakeABSactive`. Cheap because the data is
already in `DriverInfo`.

### 4. Inputs trace — M

All three plus RaceLab and Kapps: scrolling throttle/brake (and clutch)
traces with a steering readout, ABS activity highlighted on the brace
trace. Vars: `Throttle`, `Brake`, `Clutch`, `BrakeABSactive`,
`SteeringWheelAngle`, `Gear`, `Speed`. A 5–10 s ring buffer at 60 Hz drawn
as two polylines. The steering dial is optional; irdashies' five wheel
styles are decoration.

### 5. Radio: who is talking — S

irdashies and iFL03 highlight the transmitting driver in the Relative from
`RadioTransmitCarIdx`, holding for a few seconds after. A small mark in the
gutter, same as the penalty markers. Trivial data, real use in team and
league races.

### 6. Tyre compound column — S

irdashies and iFL03 show `CarIdxTireCompound` per car. Matters only in
sessions with wet/dry choice, so show it only when more than one compound
is out on track. One small block after the car number.

### 7. Pit-lane helper: limiter and speed — S/M

irdashies Pitlane Helper and iFL03 Pit: a limiter-off warning while on pit
road (`EngineWarnings & pitSpeedLimiter`), speed against the limit from
`WeekendInfo.TrackPitSpeedLimit`, and a pit-entry approach state from
`PlayerTrackSurface == ApproachingPits`. Would extend the Pit Stall widget
upstream of its last 100 m.

### 8. Position change since start — S

irdashies and iFL03 show gained/lost places per driver. Needs the grid
order recorded at the green (the Standings already watches for the start)
and a small ▲n/▼n after the position. Cheap; modest value.

### 9. Lap-time log and sector deltas — M

irdashies Lap Timer (history list with dirty-lap markers, predicted lap,
personal best persisted per car/track) and Sector Delta (from the YAML's
`SplitTimeInfo`). iFL03 colours track-map sectors. A black-box page: last N
laps with sector splits against session best. Practice-session value more
than race value.

### 10. Persisted fuel consumption — S

iFL03 caches average consumption per car/track (`FuelCache` in its config)
and irdashies keeps a per-track database, so the first lap of a session
already has a number. Ours starts cold each session. Save the rolling
average keyed by car and track id at session end; read it back as the
initial estimate, replaced as real laps come in.

### 11. Rejoin indicator — S

irdashies only: after an off, a clear / care / stop signal based on the gap
to the next car arriving on the track behind, shown while speed is under a
threshold. Our Radar Bars data already has the gaps; this is a state on
top of it that appears only when stationary or crawling off-line.

### 12. Track map — L

irdashies, iFL03, RaceLab, Kapps, iOverlay. Needs a per-track path: iFL03
ships `assets/tracks/track-paths.json`, irdashies uses the
`lovely-track-data` package, both community-collected from
`CarIdxLapDistPct` plus GPS-like `Lat/Lon` telemetry. Either recording our
own on first drive (the vars `Lat`, `Lon` exist in the dump) or vendoring
one of those sets. Real work; moderate race value since the Relative and
Radar cover the same "where is everyone" question for the driver.

### 13. Session-type visibility per panel — S

irdashies lets each widget declare which session types it shows in (race,
open/lone qualifying, practice, offline). We have per-panel visible only.
The Faster Class and Pit Stall already hide themselves by state; a general
rule per panel is a small config addition.

### 14. Telemetry record and replay — M

irdashies records raw frames to a tape and replays them through the whole
pipeline. We have `--demo` fixtures and `.ibt` is not live telemetry. A
tape of the shared-memory buffer would make every widget testable against
real sessions after the fact, including the flag field. Developer value,
not race value, but it would have settled several past "does this var
exist" questions.

## Missing and not recommended

- **Garage / setup cover** (all three): hides the setup screen from a
  stream. We already hide the panels in the garage; the cover is for
  viewers.
- **The Gantry** race-control window, incident feed with replay jumps
  (irdashies): spotter/broadcast tool.
- **Twitch chat, heart rate, OBS browser sources, dual-PC web server,
  raw JSON API** (irdashies, RaceOverlay): streaming.
- **Energy / ERS %** (RaceOverlay): GTP only; add if Flynn races GTP.
- **Corner-name overlay** (irdashies): needs the same track data as the
  map; novelty.
- **Delta-speed vs reference lap, ghost telemetry CSV** (irdashies, iFL03):
  practice analysis better done in Garage 61 / VRS.
- **Head-to-head "Battle" widget** (irdashies, RaceLab): the Relative and
  Faster Class already answer it.
- **Damage, live tyre temps**: nobody has them; the SDK doesn't publish
  them.

## Suggested order

Do 1–3 and 5 first (flag banner, delta bar, shift lights, radio mark): four
small additions, each a day or less, that close the visible gap with the
field. Then 4 (inputs trace) and 7 (pit-lane helper) as a pair of
medium-sized panels. 10 and 13 are quiet quality-of-life config work. Leave
the track map until there is a reason to want it.

Each will get its own spec in `plans/` before code, per the usual rule.
