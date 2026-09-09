# Sector colours

> **Built, then scrapped (2026-09-08).** The measuring worked and the code is
> in the history; the display never earned its place on the panel. Three
> versions were tried — every sector coloured, then only improvements against
> a dark rail, then equal blocks instead of proportional ones — and each was
> still more ink than the Relative could carry for what it said. Nothing of
> it ships. This is kept for the findings below, which cost a live session to
> establish and which anyone thinking about sectors again will want first.

A thin bar along the bottom of every driver's row, split into one block per
sector: purple where that car ran the quickest sector anyone in its class
has managed, green where it beat its own best, yellow where it didn't. A
timing screen's colours, read across a row instead of down a column.

## What iRacing actually gives us

Verified against a live session (Spa, 2026-09-08, `--dump-all-vars` and
`--dump-session-info`), not recalled from the SDK docs.

Per car, live, in every session type including qualifying:

| Variable | Already read? | What it is |
| --- | --- | --- |
| `CarIdxLapDistPct` | yes | Fraction of the lap, 0..1, per car |
| `CarIdxLap` | yes | Lap count, per car |
| `CarIdxLastLapTime`, `CarIdxBestLapTime` | yes | Whole laps only |
| `CarIdxBestLapNum` | no | Which lap the best was, per car |
| `CarIdxTrackSurface` | yes | On track / pit stall / not in world |
| `SessionTime` | yes | The clock every stamp is taken from |

**There is no per-car sector variable, and no sector variable at all.** The
session publishes 336 variables; not one of them mentions a sector, in any
spelling. iRacing gives other cars whole lap times and nothing finer, and
gives the player delta bars rather than splits. So the colours cannot be
read — they have to be measured, from data the overlay already has on every
tick.

The one new read is the sector *boundaries*, which live in the session
string. Spa's, exactly as dumped:

```yaml
SplitTimeInfo:
 Sectors:
 - SectorNum: 0
   SectorStartPct: 0.000000
 - SectorNum: 1
   SectorStartPct: 0.313076
 - SectorNum: 2
   SectorStartPct: 0.522864
 - SectorNum: 3
   SectorStartPct: 0.723114
```

`session_info::SplitTimeInfo` parses that section. Note that **Spa has four
sectors, not three** — the count is per track and read from here, never
assumed. The sectors are also markedly uneven: 31.3%, 21.0%, 20.0%, 27.7%
of the lap.

## Measuring a sector

The telemetry thread already runs on every one of iRacing's ticks (60 Hz),
and every car's lap fraction arrives on each one. A sector time is the gap
between two boundary crossings:

- Hold the previous tick's `(lap_dist_pct, session_time)` per car.
- A boundary at `b` was crossed this tick when `prev_pct < b <= pct`; the
  start/finish line is the same test with the wrap from ~1.0 to ~0.0.
- Interpolate the crossing rather than stamping the tick:
  `t = t_prev + (b - pct_prev) / (pct - pct_prev) * (t - t_prev)`.
  Without it the resolution is one tick — 16.7 ms, which is wide enough to
  colour a sector wrongly. With it, a 90-second lap resolves to well under a
  millisecond, because a tick only covers ~0.02% of a lap.
- Sector time is the difference between consecutive crossings. The lap's
  last sector ends at the line.

Accuracy is good enough to *rank* sectors, which is all a colour is. It is
not good enough to publish as a time next to iRacing's own, and the widget
should not print one.

### What must be thrown away

A sector is discarded, rather than recorded, when any of these is true —
each one otherwise produces a nonsense time that would be purple forever:

- The car was anywhere but on track during the sector (`CarIdxTrackSurface`
  says pit stall, approaching pits, or not in world).
- The car left the world mid-sector and came back — the tow case the
  Standings already tracks.
- Its lap counter moved by anything other than what the boundary implies
  (a reset to pits, a session restart).
- The time is outside a plausible band at all. Built as a flat one second
  to ten minutes rather than as a share of the class reference lap
  (`telemetry::relative` measures one): the bound that matters is the lower
  one, since an impossibly quick reading would sit purple for the session,
  and a crawling car's slow sector can never be anyone's best.
- The overlay only joined mid-sector, so there is no opening stamp.

An off-track is a special case worth its own decision: the sector should
still be *shown* (losing a second to a wide moment is exactly what you want
to see) but must not be allowed to set a personal or session best. That
needs `OffTrackCounter`'s per-car count sampled at each boundary — cheap,
but it is the one rule that couples this to another tracker, so it is worth
holding back to a second pass if it complicates the first.

Everything resets on `SessionNum` changing, which is what makes qualifying's
purples stop haunting the race.

## The colours

Standard timing-screen meaning, per class — every other classification in
these two widgets is per class, and a session-wide purple in a multi-class
race tells a GT4 driver nothing:

- **Purple** — the quickest anyone in this class has run this sector this
  session.
- **Green** — this car's own best, but not the class's.
- **The row's own hairline** — slower than its own best.
- **Nothing at all** — no valid time for that sector yet.

The third of those is the one that matters, and it was learned the hard way.
Yellow-for-slower is what a broadcast timing screen does, and across seven
Relative rows it was unreadable: nearly every sector of nearly every lap is
slower than that car's own best, so the panel was loud in direct proportion
to how little was happening. Muting the colours treated the symptom. Not
painting the default state at all is the fix — the bar sits as a quiet rail
showing where the sectors divide and lights only where somebody improved,
which is what lets the lit blocks go back to nearly full strength.

## Where it sits

A bar along the bottom edge of each driver's row, running the width of the
row and divided into as many blocks as the track has sectors — four at Spa,
three at most places, whatever `SplitTimeInfo` says. Two or three points
tall, straight-edged, hairline gaps between blocks, drawn inside the row's
own rect so nothing about the existing layout moves. Relative rows are 44
points and Standings rows 34, so the bar costs no height in either.

The blocks are split **in proportion to the sectors' real lengths**, so the
bar is a picture of the lap: Spa's first block is half again the width of
its third, because that sector is. Equal blocks would read more evenly but
would say something untrue about where the time went.

- **Relative** first — it is the widget already showing recent pace, and the
  rows are tall enough to carry the bar without crowding.
- **Standings** second, identically, once the Relative's has been lived with.

A sector with no valid time yet is left dark rather than filled, so a bar
reads as "what has been set", not as a claim about the whole lap.

Which lap the blocks describe: the lap **in progress**. Sectors light as
they are set and the ones ahead stay dark, which is what makes watching a
qualifying lap worth anything — S1 going purple as the car crosses the line
is the whole point. On completion the finished lap holds for a moment
before the next lap's bar starts filling, so a sector set at the line isn't
gone before it has been seen.

## Phases

1. ~~**Boundaries.** Parse `SplitTimeInfo`.~~ Done —
   `session_info::SplitTimeInfo`, and `sectors::Layout` refuses a list it
   cannot be sure of rather than guessing.
2. ~~**Every car.**~~ Done — `sectors::Timer`, fed one pass per tick from
   `build_snapshot`, with the discard rules above and per-class and per-car
   bests. Ranks land in `CarSnapshot::sectors` as a fixed array, so nothing
   allocates on the 60 Hz path.
3. ~~**The Relative's bar**, behind a settings toggle, defaulting on.~~ Done
   — `ui::relative::paint_sector_bar` and `RelativeConfig::show_sectors`.
4. **The Standings' bar**, its own toggle on that widget's page. Not built:
   the Relative's is the one to live with first.
5. **The off-track invalidation rule.** Not built — a sector containing an
   off-track is currently allowed to set a best, which in qualifying is a
   purple for a lap iRacing would have deleted.

**Written but never compiled or driven.** Phases 1 to 3 went in while the
car was on track, so nothing here has been through `cargo` at all, let alone
past a real lap. The first run is where this feature lives or dies: if the
measured times don't agree with the sim's own sector deltas to within a few
hundredths, nothing built on them is worth keeping.

## Cost

Per tick: one comparison per car per boundary — 63 cars by three, on a
thread that already walks every car array several times. The state is three
floats and a stamp per car. Nothing here is close to being measurable.
