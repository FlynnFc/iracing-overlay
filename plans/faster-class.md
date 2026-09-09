# Faster Class — a warning when quicker traffic is closing in

A seventh widget. In a multi-class race it appears when a car from a quicker
class is coming up behind you, says how far back it is, and gets louder as it
arrives. Two settings tune it: how far back the warning starts, and how close
the car has to be before the warning turns into an alert. Nothing else to
configure, and it never appears in a single-class session.

## Table of contents

- [The problem](#the-problem)
- [Principles](#principles)
- [What the sim will and will not tell us](#what-the-sim-will-and-will-not-tell-us)
- [Which class is faster](#which-class-is-faster)
- [Who counts as approaching](#who-counts-as-approaching)
- [The widget](#the-widget)
- [Levels and thresholds](#levels-and-thresholds)
- [Closing rate and arrival](#closing-rate-and-arrival)
- [Defaults, and what happens when each input is missing](#defaults-and-what-happens-when-each-input-is-missing)
- [Changes, file by file](#changes-file-by-file)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## The problem

In a GT3 car in a multi-class race the thing that ruins a lap is not the
GT3 behind you, it is the prototype you did not know was there. The Relative
does show it — as one row among eight, coloured for lap status, read foveally,
by a driver who has to notice that the class colour on that row is not their
own. Crew Chief says "faster car approaching" once, into a headset that is
also saying six other things.

What the driver needs at that moment is one fact, in peripheral vision, that
escalates: *a faster car is behind you, this far back, and it is closing*.
Then, when it is actually on the bumper, something that cannot be missed.

---

## Principles

The same bar the other widgets are held to.

1. **Invisible unless it's the moment.** The Radar Bars rule. A single-class
   race never shows this panel at all; a multi-class race shows it for the
   ten or twenty seconds a stint that it is true.
2. **Geometry over notation.** The distance is drawn as a block sliding along
   a bar toward a marker that is you, so closing reads as movement. The number
   is there too, in the readout face, for the driver who wants it.
3. **Escalate, don't decorate.** The widget has three states and a colour per
   state. Nothing on it changes colour for any other reason.
4. **Settings take effect as they are changed.** Both thresholds are applied on
   the UI thread against a list the telemetry thread already sends, so the
   sliders in the settings window move the widget as you drag them. The
   radar's `range_ms` needs a restart; this deliberately does not.

---

## What the sim will and will not tell us

Everything below is already read by this crate except the two class-speed
fields, which are two new lines in the YAML parser.

| Signal | Where it comes from | Use here |
| --- | --- | --- |
| Relative-time gap, signed seconds | `relative::gap_seconds`, the same number every Relative row shows | How far back the car is |
| `CarClassID` | `DriverInfo.Drivers[]` | Same class is never "faster" |
| `CarClassRelSpeed` | `DriverInfo.Drivers[]` — *verify with `--dump-session-info`* | iRacing's own ranking of class speed; higher is faster |
| `CarClassEstLapTime` | `DriverInfo.Drivers[]` — *verify* | Fallback ranking; lower is faster |
| `CarIdxBestLapTime` | already read for the fastest-lap badge | Last-resort ranking, measured |
| `CarIdxTrackSurface` | already read | A car in the pit lane is not coming |
| `SessionTime` | already read | Closing rate |

The gap is in **seconds of relative time**, not metres, and the settings are
in seconds too. That is the unit the Relative prints beside every car, the
unit a driver already thinks in for approaching traffic, and the unit that
survives a session with no `TrackLength`. Metres would add a conversion and
a way to fail without adding anything to the decision.

**What the sim will not tell us:** whether the faster car intends to pass on
this straight, which side it will come by, or whether it is on an in-lap.
None of that is guessed.

---

## Which class is faster

Three sources, in order, the first that gives an answer wins:

1. **`CarClassRelSpeed`** — iRacing's own integer ranking, published per driver
   and equal across a class. Higher is faster. Used when both classes publish
   one and they differ.
2. **`CarClassEstLapTime`** — iRacing's estimated lap for the class, in
   seconds. Lower is faster. Used when both are positive and differ by more
   than half a second; two classes closer than that are not a class
   difference a driver would feel.
3. **Measured best laps** — the quickest `CarIdxBestLapTime` seen in each
   class so far this session. The other class must be at least 2% quicker,
   so a quick driver in a similar class is not read as faster traffic.

A car in the **same class** is never faster traffic, whatever its lap times.
With none of the three available for both classes, nothing is faster and the
widget stays hidden: a warning built on a guess is worse than none.

---

## Who counts as approaching

A car is a candidate when all of these hold:

- it is a competitor (not the pace car, not a spectator);
- it is in the world and not on pit road — a car in the pit lane is not
  arriving, however quick its class;
- its class is faster than the focus car's, by the rules above;
- its gap is finite, and it is between 15 s behind and 1 s ahead.

Lap difference is deliberately **not** a criterion. A prototype a lap down
after a repair is still a prototype and still coming through.

The telemetry thread sends every candidate, nearest first, with a closing
rate each. The UI applies the thresholds. In practice and warm-up the rule is
the same as in a race: faster traffic is faster traffic, and you are expected
to let it by. Lone qualifying has no other cars on track, so it has no
candidates.

---

## The widget

A card, 320 px wide at scale 1, three rows, drawn in the overlay's visual
system: 16 px card, 4 px blocks inside, Inter for text, Barlow Condensed for
the one number that stands alone. Nothing leans.

```
┌──────────────────────────────────────────────────┐
│ ▌ GTE  #5  Driver 2                          +1  │  class tag, car, driver, others
│ ▌                                                │
│ ▌ 1.8 s     FASTER CLASS                         │  the gap, and the state
│ ▌           CLOSE · on you in ~9 s               │
│ ▌                                                │
│ ▌ █▐████                                         │  approach bar
└──────────────────────────────────────────────────┘
   ^ left stripe in the state's colour
```

- **Left stripe.** A 6 px block down the card's inner edge, in the level's
  colour. It is the part of the widget meant to be caught without looking.
- **Class tag.** The class's short name on a solid block of its colour, the
  same tag the Standings uses for a class header, so it reads as the same
  thing. Then the car number and the driver, so the driver can match it to
  what the mirror shows.
- **Others.** When more than one faster car is inside the warning window, a
  `+N` chip at the right of the header. The nearest car drives everything
  else; the chip says there is more behind it.
- **The gap.** Seconds behind, one decimal, in the readout face, with the
  unit beside it. To its right, the eyebrow `FASTER CLASS` and the state word
  in the level's colour: `BEHIND`, `CLOSE` or `PASSING`. Under it, when there
  is an honest figure, `on you in ~N s`.
- **Approach bar.** A track the width of the card. At its left end a white
  marker: that is you. A block in the level's colour slides along it toward
  the marker: that is the car, its front at its gap, the right end of the bar
  being the warning threshold. The part of the track inside the alert
  threshold is washed faintly red, so the driver can see the block enter it.
  When the car is passing, the block sits over the marker.

Colour is urgency, on the radar's scale: amber for a car that is a factor,
red for one that is on you. There is no green state. When the car has gone
by, the widget goes, which is the all-clear.

---

## Levels and thresholds

| Level | When | Colour | State word |
| --- | --- | --- | --- |
| Warn | gap ≤ `warn_secs` | amber (`RADAR_CLOSE`) | `BEHIND` |
| Alert | gap ≤ `alert_secs` | red (`theme::alert`), flashing | `CLOSE` |
| Passing | within 0.4 s behind, through to 0.5 s ahead | red, steady | `PASSING` |

Hidden once the car is more than 0.5 s ahead, or drops back past the warning
threshold. Each boundary has **0.3 s of hysteresis** on the way out, so a gap
breathing across a threshold through a corner cannot make the widget blink.

Only the **nearest** candidate sets the level. Once a car has been dismissed
as passed, it cannot become the subject again until it is genuinely behind
(more than 0.4 s back) — a passer blocked alongside for a moment must not
make the widget reappear as it wanders across the 0.5 s line.

**Flashing** at the alert level alternates the stripe, the readout and the
block between full red and a dimmed red at 2 Hz. Never *off*: a screenshot,
or a glance during the dim half, still sees the warning. `flash = false`
holds it steady.

The two thresholds are the customisation asked for. `alert_secs` is clamped
to `warn_secs` at use, so the alert can never be set further out than the
warning it escalates.

---

## Closing rate and arrival

Per candidate, the telemetry thread keeps a four-second window of `(session
time, gap)` and reports the slope: seconds of gap lost per second, positive
while the car closes. Four seconds rather than the radar's 0.6, because a
gap read off the lap curve at three seconds' separation breathes through
corners in a way a gap at half a car length does not, and a rate measured
over a fraction of that would be jitter.

The window is keyed by `CarIdx`, so a different car arriving in the list can
never be measured as one car moving. A car that leaves the list loses its
window.

**Arrival** is `gap / rate`, printed only when the rate is at least 0.02 s/s
and the answer is 60 s or under. A GTE closing on a GT3 at three seconds a
lap is 0.03 s/s, which is a figure worth having; a car at a hundredth is
holding station, and printing `~400 s` for it would be nonsense with a
tilde on it. Whole seconds, with the tilde, because the number is a
projection off a breathing gap.

---

## Defaults, and what happens when each input is missing

| Input | Missing behaviour |
| --- | --- |
| `CarClassRelSpeed` | Fall through to `CarClassEstLapTime`, then to measured best laps. |
| Every class-speed source | Nothing is faster; the widget stays hidden. |
| Focus car's driver entry | No class to compare against; hidden. |
| `SessionTime` stalls | Rate window discards the tick; no arrival printed. |
| Gap non-finite | The car is skipped, as the radar skips it. |
| `warn_secs` ≤ 0 or NaN | The default stands in. |
| `alert_secs` > `warn_secs` | Clamped to `warn_secs`. |
| Single-class session | No candidate ever; nothing drawn, nothing computed beyond one comparison per car per tick. |

New `[faster_class]` config keys, all with working defaults:

| Key | Default | Meaning |
| --- | --- | --- |
| `visible` | `true` | The widget hides itself, so there is nothing to opt out of in the usual case. |
| `scale` | `1.0` | As every panel. |
| `warn_secs` | `4.0` | The gap at which the card appears. Between a prototype's twenty seconds of closing and a GTE's minute. |
| `alert_secs` | `1.5` | The gap at which it turns red. Roughly the point the faster car is committing to a move. |
| `flash` | `true` | Whether the alert level flashes. |
| `pos` | `[900, 60]` | Top of the screen, right of the Weather seed: in the eyeline, clear of the other seeds. |

The settings window gets a **Faster Class** page: Show, Scale, *Warn at*
(1–15 s), *Alert at* (0.5 s up to the warning), *Flash when alerting*, and
the usual reset. The tray menu gets its tick.

---

## Changes, file by file

| File | Change |
| --- | --- |
| `telemetry/session_info.rs` | `Driver` gains `CarClassRelSpeed` and `CarClassEstLapTime`. |
| `telemetry/faster_class.rs` | New. `ClassPace` + `is_faster_class`; `ClosingRates`; `arrival_secs`; `Thresholds`; `Level`; `Alarm` with the hysteresis and dismissal rules. Pure functions, unit-tested. |
| `telemetry/snapshot.rs` | `Approaching` and `FasterClassSnapshot`; a field on `TelemetrySnapshot`. |
| `telemetry/session.rs` | `DriverMeta` carries the two class-speed fields; the car loop collects sightings; `build_faster_class` sorts them and attaches rates; the tracker joins `SessionTrackers`. |
| `config.rs` | `FasterClassConfig`, the `[faster_class]` table, panel plumbing. |
| `tray.rs` | `Panel::FasterClass`. |
| `ui/faster_class.rs` | New. The card. |
| `ui/settings.rs` | The page. |
| `app.rs` | The draggable panel, the alarm state, the save. |
| `demo.rs` | The GTE Porsche already 1.8 s behind in the Relative fixture becomes the demo's approaching car. |
| `README.md` | The widget bullet and the config keys. |

---

## Testing strategy

Pure-function unit tests in `telemetry/faster_class.rs`:

- `is_faster_class`: same class is never faster; rel-speed wins; est-lap
  decides when rel-speed ties or is absent; measured laps need the margin;
  nothing known is not faster.
- `Alarm`: appears at the warning, escalates at the alert, holds through
  0.3 s of hysteresis, goes to passing, hides once the car is 0.5 s ahead,
  does not come back for a car wandering across that line, and switches
  subject when a nearer car arrives.
- `ClosingRates`: positive while closing, keyed per car, forgets a car that
  leaves, reports nothing from one sample.
- `arrival_secs`: honest at a real rate, silent at a stalemate and beyond a
  minute.
- `Thresholds::new`: nonsense falls back, alert clamps to warn.

Config round-trip and empty-config defaults in `config.rs`. Visual check
through `--demo` and layout mode, where the fixture car is 1.8 s back and
draws the warning state.

---

## Build phases

1. **Class speed.** YAML fields, `ClassPace`, `is_faster_class`, tests.
2. **Sightings.** The loop, the snapshot, the closing rates. Nothing visible
   yet; `--demo` carries a fixture.
3. **The card.** Config, tray, settings page, the widget in its warning and
   alert states.
4. **Arrival.** The projected figure, last because the widget is complete
   without it.

## Explicitly out of scope

- **Which side the car will pass on.** The SDK gives no lateral position for
  a car three seconds back; the radar takes over once it is alongside.
- **Warning the faster car about slower traffic ahead.** A different widget
  with a different question; the Relative already shows it.
- **Audio.** Crew Chief does this and does it well; the overlay is visual.
- **Per-class colour choices.** The class colour comes from the session, as
  everywhere else in the overlay.
