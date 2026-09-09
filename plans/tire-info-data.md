# Tire Info: read the right variables, drop our latch

The Tire Info page shows 35 °C / 100 % / 159 kPa for the whole race while
iRacing's own Tire Info box shows real numbers. Three separate causes, all
confirmed against recorded `.ibt` telemetry from
`porsche992rgt3_watkinsglen 2021 fullcourse 2026-06-20 21-43-42.ibt`.

## Correction (2026-08-31): .ibt is not the live stream, and there are no live tyre temps

**Everything below was measured from a recorded `.ibt`, and the live
shared-memory stream does not carry the same variables.** Established by
enumerating every variable in a live session with `--dump-all-vars` (Porsche
992 R GT3):

| Family | In `.ibt` | Live |
| --- | --- | --- |
| `LFtempL/M/R` (surface) | live, every tick | **absent entirely** |
| `LFtempCL/CM/CR` (carcass) | frozen | present, **refreshes at pit stops** |
| `LFwearL/M/R` | frozen | present, refreshes at pit stops |
| `LFpressure` (hot) | live | **absent entirely** |
| `LFcoldPressure` | garage constant | present, constant |

**iRacing publishes no live tyre temperature or pressure at all** — which is
what the community has documented all along. The only tyre readings available
while driving are the last-stop carcass temps, last-stop wear, and the garage
cold pressure: exactly what the sim's own "data collected at last pit stop"
box shows.

> A mid-investigation reading of `58.5 / 67.1 / 70.7` on the LF looked like
> proof the carcass family was live. It was not: that sample was taken with
> `PlayerCarInPitStall = true`, `OnPitRoad = true`, `Gear = 0` — stopped in
> the box, i.e. a pit-stop refresh. Two samples taken while actually driving
> were byte-identical. **Check the pit flags in the same dump before calling a
> variable live**, and prefer sampling across laps rather than across minutes.

The conclusions below about which family is which are therefore right about
the `.ibt` and wrong about the live stream. The decision they drove — the Tire
Info page mirroring the sim's box — still stands, because it is a last-stop
page either way.

## What the sim actually publishes

Sampled across a stint and through a pit stop, **in a recorded `.ibt`**:

| Variable | Behaviour while driving |
| --- | --- |
| `LFwearL/M/R` | **Frozen.** Changes only at a pit stop. |
| `LFtempCL/CM/CR` (carcass) | **Frozen.** Changes only at a pit stop. |
| `LFtempL/M/R` (surface) | **Live**, updates every tick. |
| `LFpressure` | **Live**, tracks the tyre heating up. |
| `LFcoldPressure` | Constant — the garage setting. |

The stop itself, at 10 Hz:

```
SessionTime  OnPitRoad  InPitStall  PitstopActive  Speed   LFwearL  LFtempCL  LFpressure
  18183.800       True       False          False  17.553    1.000    34.671     180.527
  18210.200       True      False           True    0.050    0.807    71.296     178.770   <- sim refreshes here
  18210.500       True       True           True    0.002    0.807    71.296     178.769   <- stall flag, 0.3 s later
  18250.600       True       True          False    0.133    0.807    71.296     158.611
```

`LFwearL` holds 0.807 for the entire next stint. **The sim already latches this
data for us** — that is what "Data collected at last pit stop" means.

## Cause 1 — `TyreLatch` is redundant and freezes garbage

`TyreLatch` (`telemetry/session.rs`) captures on the rising edge of
`PlayerCarInPitStall` and holds that snapshot until the next rising edge.

But `PlayerCarInPitStall` goes true whenever the car is sitting in its box —
at session join, on the grid, after a tow, or rolling in before the crew starts
work. `PitstopActive` (which is what actually triggers the sim's refresh) may
come later or never. At the start of the file the car sits in its stall for 30 s
with `PlayerCarInPitStall = True` and the values still at their untouched
defaults of `1.000` / `34.671` / `158.579`.

Those are exactly the numbers on screen: 100 %, 35 °C, 159 kPa. The latch
captured the pre-service defaults, closed, and never reopened — so the sim's
real refresh at the next stop was thrown away.

Getting out of the car cleared the tracker state, `captured` went back to
`None`, the page fell through to the raw (by then refreshed) values, and the
real wear appeared. That matches the reported symptom.

**Fix:** delete `TyreLatch` and pass `LFwear*` / `LFtempC*` straight through.
The sim's own latch is the correct one and it cannot be captured early.

## Cause 2 — the two temperature families are swapped

`snapshot.rs` documents `TyreState`:

> Temperatures are the live carcass readings (`LFtempCL` and friends) [...]
> The last-stop variables (`LFtempL/M/R`) are simply not published.

Both halves are wrong. `LFtempL/M/R` *are* published, and they are the live
ones; `LFtempC*` is the frozen last-stop family. The comment needs correcting
whichever variable the page ends up using.

## Cause 3 — pressure is the garage cold value

The page reads `LFcoldPressure` (159 kPa, identical on all four). iRacing's box
shows 181 / 175 / 175 / 171 — the hot pressures of the tyres that came off.
There is no last-stop pressure variable, so matching the box means latching
`LFpressure` ourselves at the moment `LFwear*` changes.

## Decision — mirror the sim's box

The page stays a last-stop page. `LFwear*` and `LFtempC*` are read straight
through, and `LFpressure` is sampled on the wear-change edge by
`StopPressures`, which replaces `TyreLatch`. Before the first stop of the
session — and when the overlay joins one that has already had a stop, where
the pressure is simply not recoverable — the garage cold pressure stands in,
which is what the sim shows then too.

A live page built on `LFtempL/M/R` and live `LFpressure` remains possible
later; the variables are there and this change documents them.

## Verification

Both algorithms were replayed over the recorded race file. Only one refresh
occurs in it, at the lap-164 stop:

```
sim refreshes seen: 1
  t=18210.2 lap=164  LF wear=81/75/72%  LF tempC=71/73/79C  sampled kPa=179/176/177/173

end of session, LEFT FRONT
  NEW  71/73/79C  81/75/72%  179 kPa
  OLD  71/73/79C  81/75/72%  159 kPa
```

The old latch reaches the right wear and temperatures here only by luck: the
sim refreshed 0.3 s *before* the stall flag rose on this particular stop. It
still prints the cold 159 kPa rather than the 179 the tyre came off at. And
for the hour of racing before that stop it showed 100 % / 35 °C / 159 kPa,
having captured the defaults on the stall entry at `t=14603` — the reported
symptom exactly.
