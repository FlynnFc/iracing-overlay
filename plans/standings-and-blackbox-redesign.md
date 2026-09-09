# Standings and black box — one instrument, read at speed

A visual overhaul of the Standings widget and every black box page. Not a
restyle: the same data, the same controls, the same wheel binds, arranged so
that each panel answers its one question in the length of a straight.

`plans/blackbox-design.md` made each page an instrument instead of a form.
This plan is the next step: the pages now look like six different
instruments, and Standings looks like a fourth product again. What follows
is the system that makes them read as one — and the two or three real
faults the screenshots turned up along the way.

**Status: implemented 28 Aug 2026**, every phase, with two later decisions
folded in — Tires and Tire Info are one page, and every control is
clickable (see *Clicking*). The artifact at
<https://claude.ai/code/artifact/346f4d40-cdf5-43aa-8637-28aabe321e26> is the
design as built.

## Table of contents

- [Where it stands](#where-it-stands)
- [The brief, pinned](#the-brief-pinned)
- [Tokens](#tokens)
- [The signature: the pit board](#the-signature-the-pit-board)
- [Standings](#standings)
- [The black box chassis](#the-black-box-chassis)
- [Page by page](#page-by-page)
- [Pit Stall](#pit-stall)
- [Weather widget](#weather-widget)
- [The icon](#the-icon)
- [Motion](#motion)
- [Absent inputs](#absent-inputs)
- [What this costs in code](#what-this-costs-in-code)
- [Build order](#build-order)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## Where it stands

Screenshots from `--demo` and `--demo-page=…` on 28 Aug 2026, against the
release binary of 12 Aug.

**Standings** is three cards — drivers, timing, endurance strip — plus a
gutter, each with its own rounded corners and its own fill. A row is one car,
but the eye has to cross two seams to read it, and the seams are exactly where
the interesting numbers start. The endurance strip's three columns are
`Stint · Stops · Pit`, and in the demo every row reads `6L · 1 · 26.9 T`: a
column of identical values is a column that has told the driver nothing per
row. The summary band under the table — laps left, stops owed, projected
finish, and `RIVAL ON 1 FEWER` — is the most decision-shaped information on
the panel and is set in the panel's smallest type, in grey, with the one alarm
as a small red caption at the far right. Everything above it is good: the
class tags, the class slash, the position plate, the olive player row, the
purple fastest cell, the gutter chips. That is the identity, and this plan
keeps all of it — minus the lean those shapes currently have, which goes
everywhere (see *Layout*).

**Black box.** Every page is a 600-pixel rounded card with a 15-pixel title
and no indication of which of seven pages it is, or which one the next press
lands on. Page by page:

- *Fuel* — the hero is the `55 L` stepper plate, not the tank. The tank is a
  34-pixel bar in green and amber: green for "in the tank", amber for "adding",
  and then amber again on the `FUEL` and `TEAROFF` tiles for "armed". One
  colour, two meanings, on one page. The finish notch is a one-pixel white
  line that disappears against the amber.
- *Strategy* — `NO STOP NEEDED` in 34-pixel green, two lines of sentence-case
  prose, a 150-pixel void, then a footer. The empty state is bigger than the
  full one.
- *Tire Info* — four equal boxes with no car between them; three tread bars
  a few pixels tall whose *numbers* carry the temperature colour while the bars
  stay grey; and the pressure caption overprints the wear percentages
  (`97% 177 kPa` collide at LF and RF). That last one is a bug regardless of
  this plan.
- *Tires* — the corner grid with 34-pixel pressures; the closest of the pages
  to right.
- *In-Car* and *Weather* — value-over-label tiles. Quiet and fine, and
  identical to any other dark-theme dashboard.

None of the six shares a visual centre with Standings: nothing slants, no
plate, no class-tag geometry. They are drawn by the same code and do not look
related.

## The brief, pinned

One subject, one audience, one job.

- **Subject:** an in-sim instrument cluster, over a moving video background,
  read from the driving position.
- **Audience:** Flynn, driving. Multiclass, often endurance. Reading happens
  in the one-second windows a straight allows, with the wheel doing the
  pressing.
- **The job of every panel:** be read, not looked at. The first thing the eye
  lands on is the answer; everything else is checkable afterwards.

The vernacular this borrows from is the pit wall, not the broadcast: the pit
board with its slotted, condensed numerals; the dash with one big readout and
alarm colours; tyre chalk. The current Standings already leans on the TV
timing tower, which is fine for names and gaps, but the black box is a *dash*,
and a dash speaks numbers first.

## Tokens

### Colour — unchanged, and now with rules

The overhaul adds no hue. Every colour below already exists in
`ui/mod.rs`; what changes is that each now means exactly one thing, on every
page.

| Token | Hex | Means | Never means |
| --- | --- | --- | --- |
| Ink | `#121212` @ 92 % | the card | — |
| Paper | white at 100 / 70 / 45 % | text, actual quantities (fills) | — |
| `SIGNAL` | `#34D399` | good, ahead, on pace, on the marks | "in the tank" |
| `ALERT` | `#FF5C57` | bad, overlapping, hot edge, rival ahead on strategy | — |
| `CAUTION` | `#FFB84D` | **armed** — a thing the next stop will do | a fuel quantity, a temperature |
| `PLAYER_ROW` | `#8A7B3A` | you — and, on the black box, **the cursor** | — |
| `FASTEST` | `#7C3AED` | fastest lap of the class | — |
| `LAPPED` | `#60A5FA` | a lap down; cold edge of a tyre | — |

Two structural devices carry meaning that used to be carried by colour:

- **Solid vs hatched.** Solid fill is what is real now; the diagonal hatch the
  radar mockup uses (`design mocks/Screenshot_14.jpg`) is what is *planned*:
  fuel the stop will add, a lap not yet run. Hatch is white at 45 % on ink, so
  it needs no colour at all. This is what frees amber to mean armed and nothing
  else.
- **Lit vs unlit.** An armed control is a filled amber plate with ink text; an
  unarmed one is an ink plate with paper text. State is a pattern of lit tiles,
  as the previous plan asked, and lit now has one colour.

### Type — three faces, three jobs

| Role | Face | Where |
| --- | --- | --- |
| Readout | **Barlow Condensed** SemiBold | positions; every black box hero number; pit-window lap numbers; tile values |
| Text | Inter (existing) | names, labels, footers, tab rail |
| Timing | IBM Plex Mono (existing) | every lap time and gap, wherever ten characters have to line up |

Barlow Condensed is the one addition. Condensed is not a style choice here
but a trade: an overlay's scarce resource is width, and a condensed numeral
buys height without spending it — a 24-pixel position fits the plate column
that a 17-pixel Inter figure fills today. Its vocabulary is highway signage
and licence plates, which is the pit board's vocabulary, and it has tabular
figures so a column of positions holds its alignment. SIL OFL, so it ships in
`assets/fonts/` beside the other two with its licence file.

The rule that keeps three faces from becoming a mess: **inside a table row,
one numeric face** — Plex Mono — because gap, fastest and last must align as a
string. Condensed is for a number that stands alone: a position, a hero, a
tile. It never appears inside a lap time.

Scale, at design size:

| Step | Size | Face | Used for |
| --- | --- | --- | --- |
| Hero | 44 | Readout | pit-window lap, on its plate |
| Readout | 34 | Readout | tyre pressures, tile values, fuel figures, strategy line |
| Position | 24 | Readout | Standings position plate |
| Name | 20 | Text Medium | driver names |
| Time | 17 | Timing | gaps, laps |
| Label | 13 | Text SemiBold, +0.06 em, caps | column heads, tile labels, tab rail |
| Note | 12 | Text | footers, subtitles |

### Layout — nothing leans

Today the class slash, the class tags, the position plate and the control
plates all lean on one angle (`ui::slash_slant`). Flynn's call: the slants go.
The geometry becomes two radii and no angles — the card at 16 px, every block
inside it (tag, plate, tab, tile, bar end) at 4 px, the class slash a straight
4 px bar in the class colour. What used to be carried by the lean is carried
by fill and type instead: a plate is a lifted block, an armed plate is an
amber one, a position is a heavy condensed numeral.

## The signature: the pit board

The one thing to remember these panels by: **numbers that stand alone are set
like a pit board** — condensed, heavy, in a plate — and the black box carries
a **tab rail** of seven tags, the current page lit, so the wheel's page ring
is visible on the card it drives.

That rail is the single risk taken. It is 26 pixels of permanent chrome on a
panel that argues for economy. It earns them because the black box today
answers "where am I in the ring?" with nothing, and a wheel button pressed at
speed needs the answer to be on the screen before the press, not after.

## Standings

**Question:** where am I, who is around me, and is my strategy winning.

### One card, three bands

The three cards become one card with three bands. Bands are vertical tints,
not separate rounded fills: the driver band at the card's ink, the timing band
lifted one step (`STANDINGS_TILE_BG`, as now), the strategy band lifted the
same step again. Rows run straight across all three. The class underline
already spans the cards; now it has nothing to span and simply runs.

Width at scale 1: `440 + 286 + 170 = 896`, down from `976`. The strategy band
is narrower because two of its three columns become glyphs.

```
┌ R  00:20:08 / 00:45   L12/25                          🚗 36 ──────────────┐
│ ▞DP▞▞1▞                    SOF 1852 │  Gap    Fastest    Last   │ STINT  STOPS  NET │
│▐ 1 ▌/ Matthew Naylor  1.8k  ⌂       │  -      01:38.021  01:38.785│ ▬▬▬▬   ●     ▲1  │  ⏱
│ ▞GTE▞▞20▞                  SOF 4690 │                             │                  │
│▐ 1 ▌/ Sindre Setsaas  8.0k  ⌂       │  -      01:38.635  01:40.063│ ▬▬▬▬▬  ●     —   │  ⏱
│ ▞GT3▞▞15▞                  SOF 2817 │                             │                  │
│▐ 1 ▌/ Renan Azeredo   6.3k  ⌂       │  -      01:42.198  01:42.353│ ▬▬▬    ●     —   │
│▐ 2 ▌/ Marco Acunto    6.6k  ⌂       │  0.9    01:42.089  01:42.619│ ▬▬▬    ●     ▲1  │  ⏱
│▐ 3 ▌/ Fabian Seischegg 4.4k ⌂       │  11.5   01:42.702  01:43.231│ ▬▬▬▬▬▬ ●     ▼1  │
│ ······                              │                             │                  │
│▐ 9 ▌/ Adaildo Vieira  5.2k  ⌂       │  32.0   01:42.278  01:43.334│ ▬▬     ●     —   │  ●
│▐10 ▌/ Neil Cooper     2.5k  ⌂       │  35.2   01:44.002  01:44.247│ ▬▬▬▬   ●     —   │
│▐11 ▌/ Rick Zwieten    1.9k  ⌂       │  44.1   01:44.059  01:44.292│ ▬      ○     ▼2  │  PIT
│▐12 ▌/ Istvan Fodor    2.2k  ⌂  ◀ you│  47.4   01:45.035  01:45.302│ ▬▬▬▬   ●     ▲2  │
│▐13 ▌/ Alexandr Fescov 1.7k  ⌂       │  52.1   01:44.822  01:46.677│ ▬▬▬▬   ●     —   │
│▐14 ▌/ Marius Rieck    1.9k  ⌂       │  53.5   01:44.754  01:46.184│ ▬▬▬▬▬  ●     ▼1  │
│▐15 ▌/ Preston Perlmutter 2.4k ⌂     │  11L    01:43.111  01:44.601│ ▬▬▬    ●●    ▼1  │
├──────────────────────────────────────────────────────────────────────────────────────┤
│  13 LAPS LEFT     1 STOP TO GO      NET  P10  ▲2      ▞RIVAL ON 1 FEWER▞             │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### What changes, column by column

- **Position plate.** A straight-edged plate now, with the numeral in Readout
  24 instead of Inter 17. On the player's row the plate is olive-filled with ink
  text, so "you" is readable from the plate alone before the row wash is
  noticed.
- **Top bar.** Adds the session length after the clock (`/ 00:45`, as the
  mockup has it) and the lap counter `L12/25` — both exist on the Relative's
  footer today and are absent from the panel that most needs them. Both fall
  away when the session has no limit of that kind.
- **Class bar, single-class sessions.** When every car is in one class the
  bar says nothing, so it is not drawn — on Standings or on the Relative —
  and the name moves up to where the bar was. The class header keeps its
  count and SOF, drawn in paper rather than the class colour, since there is
  no second class to tell it apart from. `SINGLE_CLASS` blue goes with it.
- **Timing band.** Unchanged. It is right.
- **Strategy band** — three columns, each now varying row to row:
  - `STINT` — a bar, not `6L`: laps since this car's last stop as a fraction
    of its own average stint (`current_stint_laps / avg_stint_laps`). A bar
    near full is a car about to pit; the player's bar is olive, others paper
    at 70 %. With no completed stint yet, the bar is the fraction of the
    player's average instead, drawn hatched — planned, not measured — and
    absent if there is no average anywhere yet.
  - `STOPS` — pips, one per stop still owed: `●●` two, `●` one, `○` none.
    Empty (not `○`) while `stops_remaining` is `None`.
  - `NET` — the projected class finish as a delta from the row's current
    position: `▲2` in `SIGNAL`, `▼1` in `ALERT`, `—` for level. The current
    per-row `Pit 26.9 T` goes: a rival's stationary time matters once a race,
    and the strategy line below carries the player's own.
- **Strategy line** (the old summary band). Set in Readout 34 for the three
  numbers — `13`, `1`, `P10 ▲2` — with Label-size captions under them, and the
  rival call as an `ALERT` tag at the right, or a `SIGNAL` tag reading
  `STRATEGY LEVEL`. The band is 44 pixels rather than 34 so the numerals have
  room, and it is the largest type on the panel because it is the panel's
  verdict. Endurance mode still governs whether the band and the strategy band
  appear at all.
- **Gutter.** Unchanged: stopwatch, `PIT`, off-track dot, penalty flag.

### What this deliberately keeps

Class tags with count and SOF (square-cornered now), the class bar, iRating
pill, brand mark,
class-colour wash fading past the first letter, the purple fastest cell,
dimmed rows for cars in the pits or laps down. These are the identity; the
plan touches numerals and the strategy zone and nothing else in the driver
band.

## The black box chassis

Every page is drawn into the same frame. This is what the previous plan's
"shared language" lacked: a shared *shape*.

```
┌ [RELATIVE] [FUEL] [TIRES] [PIT WINDOW] [IN-CAR] [WEATHER] ─────────────┐  26 px rail
│                                                                       │
│   page body — as tall as its content, never padded to a fixed height  │
│                                                                       │
│  note: what these controls apply to, or how much to trust the numbers │  22 px footer
└───────────────────────────────────────────────────────────────────────┘
```

- **Tab rail.** Six tags in the class-tag geometry, Label type, ~118 pixels
  each across the card. The current page is a paper-filled tag
  with ink text (the same treatment as the gutter's `PIT` chip); the others are
  ink with paper at 45 %. The rail replaces the title row: the lit tab *is* the
  title, and the icon beside the old title goes with it. The page order on the
  rail is `Page::ALL` — the order the wheel walks — so the driver can count
  presses.
- **Footer.** The old subtitle (`Applies next pit-stop`, `Current conditions`,
  `Estimates · good`) moves here, right-aligned, Note type. A page with nothing
  to say has no footer.
- **Cursor.** Olive — `PLAYER_ROW` — as now: a 2-pixel ring on whatever control
  the cursor is on, plus the control's own plate lifted one step. The white
  outline the Fuel stepper has today is gone; the cursor has one colour across
  the overlay and it is the one that already means "you".
- **Control plate.** A 6 px-rounded block, chevrons at the ends when it
  steps. Armed → amber fill, ink text. Read-only pages have no plates.
- **Width.** Page one — the Relative — is 690 wide today and every other page
  is 600. The chassis takes one width, and it is 690: the page that is on
  screen all race must not change size when the rail comes and goes. The
  other pages gain 90 pixels; the tank gets longer and the tyres sit a little
  further apart, and nothing else needs re-measuring.

## Page by page

### Fuel — "how much, and does it get me there?"

The tank becomes the hero, drawn at a size that can hold its own numbers.

```
┌ rail ─────────────────────────────────────────────────────────────┐
│                                                                   │
│   ADD                                        FINISH ▾             │
│  ‹┌────────────────────────────┬─────────────────┬──────┐›        │  56 px
│   │ 32 L                       │ +55 L  (hatched)│      │         │
│   └────────────────────────────┴─────────────────┴──────┘         │
│    solid = in the tank          hatch = this stop adds            │
│                                                                   │
│     37.4          13           +24                                │
│     LAPS OF FUEL  LAPS LEFT    SPARE                              │
│                                                                   │
│  ▞FUEL▞   ▞TEAROFF▞   ▞FAST REPAIR · 0▞   ▞AUTO · +0.5 LAP▞        │
│                                                                   │
│                                          Applies at the next stop │
└───────────────────────────────────────────────────────────────────┘
```

- The tank is a 56-pixel capsule the width of the card, in the pit-stall bar's
  rounded-end language. Solid paper for what is in it, hatch for what the stop
  adds, ink beyond. Both quantities are written *inside* their segments in
  Readout 34 — `32 L` in the solid, `+55 L` in the hatch — so the number and
  the length are the same object. A segment too short for its number puts it
  just outside, on the ink.
- `FINISH` is a flag on the tank's top edge at the litres the race needs. Where the hatch ends past the flag, the run-out is spare; short of
  it, the gap is drawn in `ALERT` hatch with `-6 L` inside — the shortfall as a
  length. This is the page's whole argument: enough or not is where two edges
  sit.
- The stepper chevrons sit at the tank's ends. The `ADD` control is the tank;
  the cursor rings the capsule. Under Auto Fuel the tank is read-only and the
  chevrons are gone, as the current page already does with the row.
- Three readouts under the tank, Readout 34 over Label: laps of fuel, laps
  left, and their difference as `SPARE` in `SIGNAL` or `SHORT` in `ALERT`.
  This replaces the green sentence.
- Arm strip: four plates. Fast Repair carries its remaining count;
  Auto carries its margin when on, so the margin stepper stops being a
  separate row and becomes what the cursor steps while on the `AUTO` plate
  (a press toggles, a turn changes the margin). Lit = amber.

### Tires — "which corners, at what pressure — and what came off them?"

One page, not two. Tires and Tire Info were folded together (Flynn, 28 Aug):
the data a driver uses to decide a pressure change — which edge of which
tyre ran hot at the last stop — belongs on the same tile as the control that
changes it, and the fold takes the ring down to six pages with the car drawn
once. The same corner grid, on a car: a faint outline of the body between the
wheels, with the cabin on it, so the four tiles read as attached to
something.

```
┌ rail ─────────────────────────────────────────────────────────────┐
│   ┌────────┐        ╭──────────╮        ┌────────┐               │
│   │  159   │        │          │        │  159   │   pressure to │
│   │  kPa   │        │  ┌────┐  │        │  kPa   │   set (armed  │
│   │ ▂ ▅ █  │        │  │    │  │        │ █ ▅ ▂  │   = amber)    │
│   │ 68 78 83        │  └────┘  │        │78 72 62│   last stop:  │
│   │ ▬▬▬▬▬▬ │        │ ▐ALL FOUR▌│       │ ▬▬▬▬▬▬ │   tread bars, │
│   │177 kPa hot       │          │        │174 kPa hot  wear, hot  │
│   └────────┘        ╰──────────╯        └────────┘   pressure    │
│   (LR and RR below, the same)                                     │
│                   DRY · applies at the next stop · tread from the last stop │
└───────────────────────────────────────────────────────────────────┘
```

- Tyre plate, 156 × 150: the pressure to set in Readout 34 with `kPa` under
  it, then the three tread bars, their temperatures, the wear as one fill,
  and the hot pressure the tyre came off at. Armed → amber fill, ink
  figures. "Fronts only" is two amber shapes at the front of a car.
- The bars carry the colour and the numbers stay quiet: `ALERT` above the
  tyre's own mean by more than eight degrees, `LAPPED` blue below it, paper
  within. A hot outer edge on the right front is a red bar at the top right
  of the car.
- Wear is one fill along the plate — its lowest point across the three,
  since they were never different enough to read — and goes `ALERT` below
  75 %.
- Before the first stop a corner has no readout: its bars are empty
  troughs, its temperatures dashes, and the footer says `no stop yet this
  session`. The sim publishes zeros there, and a zero is not a temperature.
- `ALL FOUR` is a plate on the body between the axles, lit when every
  corner is. Cursor order is unchanged: `[all four, LF, RF, LR, RR]`.
- Compound and the two notes share the footer.

### In-Car — "what am I set to?"

```
┌ rail ─────────────────────────────────────────────────────────────┐
│  ┌────────────────────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌───────┐        │
│  │  54.0 %            │ │  3  │ │  4  │ │  1  │ │ RACE1 │        │
│  │  ▬▬▬▬▬▬▬▬▬│▬▬▬▬▬▬  │ │ ABS │ │ TC  │ │ MAP │ │ DASH  │        │
│  │  BIAS  front       │ └─────┘ └─────┘ └─────┘ └───────┘        │
│  └────────────────────┘                                Read-only │
└───────────────────────────────────────────────────────────────────┘
```

- Brake bias is a split, so it is drawn as one: a bar with a notch at 50 % and
  the fill to the front. The number stays. It is the one value on this page a
  driver changes mid-lap, and a length can be seen moving where a number
  cannot.
- Tiles: Readout 34 over Label. Dash shows the sim's page name when it
  publishes one, `PAGE 1` otherwise, never a bare `0`.
- No plates: nothing here is settable.

### Weather — "what's it doing?"

```
┌ rail ─────────────────────────────────────────────────────────────┐
│  ┌──────┐ ┌──────┐ ┌────────────┐ ┌──────┐ ┌──────┐               │
│  │ 54°  │ │ 23°  │ │   ↗  3     │ │ 10 % │ │ 23 % │               │
│  │TRACK │ │ AIR  │ │  km/h      │ │ FOG  │ │ RAIN │               │
│  └──────┘ └──────┘ └────────────┘ └──────┘ └──────┘               │
│                                                Current conditions │
└───────────────────────────────────────────────────────────────────┘
```

- The wind tile is double width and the arrow is 28 pixels, car-relative,
  with the speed beside it rather than under it. Today's 10-pixel arrow is the
  page's only spatial cue and is the smallest thing on it.
- Everything else as now, at the new type.

## Pit Stall

**Question:** how far to my marks — and, once stopped, which way am I off.

Today: a 120 × 320 capsule that fills red, then amber, then green as the car
closes, with `2.5 m SHORT` in 30-pixel Inter beneath it. Two things are wrong
with it under the new rules. Red means *bad* everywhere else on the overlay,
and being forty metres from the box is not bad — it is the normal state of
the widget for most of its life, so the alarm colour is on screen exactly
when nothing is wrong. And the number a driver actually wants in the last
metres is small and under the bar, where the eye is not.

```
                 ┌──┐  ← the box: a paper block at the top, 24 px
                 │▒▒│
                 │▒▒│    hatch = road still to cover
                 │▒▒│
                 ├──┤
                 │██│    solid paper = covered
                 │██│         12.4  ← Readout 44, beside the bar, not under it
                 │██│         M
                 └──┘
```

- **Capsule** 64 wide, 280 tall — half the width, a little shorter. The fill
  is read as a level, and a level needs no width.
- **Fill is paper, not a colour ramp.** Solid for the road covered, hatch for
  the road still to come: the same solid-vs-planned language as the fuel
  tank, stood on end. The box itself is a 24-pixel paper block at the top.
  The fill stays linear in distance, as now.
- **Colour arrives only at the end.** On the marks the whole capsule goes
  `SIGNAL` green — the state the widget exists to confirm. Through the marks
  it goes `ALERT` red with `LONG` beside it, and stays until the car is back.
  Nothing is amber; there is nothing to arm.
- **The readout is the hero.** Metres to go in Readout 44 beside the bar,
  unit in Label beneath the number. Once stationary and off the marks it
  reads `SHORT` or `LONG` in Label over the distance, in `ALERT`. On the
  marks there is no number: the green is the answer, and the widget is gone
  a moment later anyway.
- No card, no header, as now. Footprint ~200 × 280 against ~120 × 370.

## Weather widget

**Question:** what is the track doing, and where is the wind hitting me.

Today: four tiles stacked in a 300-wide card, ~600 tall, the bottom one a
190-pixel compass whose ring counter-rotates to true north while the arrow
stays in the car's frame. It is a lot of screen for four numbers, and the
ring answers a question a driver does not have — the track already says
which way the car is pointing.

```
┌──────────────────────────────────────┐
│  54°       23°        23%      10%   │  Readout 34 over Label
│  TRACK     AIR        RAIN     FOG   │
│ ┌──────────────────────────────────┐ │
│ │        ╭─╮                       │ │
│ │   ↗    │ │      3       km/h     │ │  car nose-up, arrow the way the wind pushes
│ │        ╰─╯      crosswind, from the left-rear
│ └──────────────────────────────────┘ │
└──────────────────────────────────────┘
```

- **One card, 300 wide, ~180 tall.** Two rows: the four readings, then the
  wind.
- **Readings** in Readout 34 over Label, as the black box's tiles. Rain and
  fog appear only when non-zero; the row closes up without them.
- **Wind** is the same car-relative arrow the black box's Weather page has,
  at a size that earns the tile: the car silhouette from the Tires page,
  nose up, 44 pixels tall; the arrow in the car's frame, pointing the way the
  wind pushes, its length scaled to speed. Speed in Readout 34, and a plain
  phrase under it — `headwind`, `tailwind`, `crosswind from the left`, from
  eight sectors — because a word is read faster than an angle is judged.
- **The compass ring goes.** True north survives as a small `N` tick on a
  faint ring round the car, counter-rotated as the ring was, so the
  information is still there for the driver who wants it and costs nothing
  for the one who does not.
- Footprint 300 × 180 against 300 × 600.

## The icon

The current mark — three leaning class slashes on ink — is the slant. The new
one keeps its meaning and drops the lean: three standings rows, each a class
bar in gold, blue and pink beside a paper plate of a different length, on a
16-radius ink square. It reads as a timing tower and as a pit board's slots,
and it is drawn from the same three colours and geometry as the panels.

At 16 and 24 pixels — the tray, where it sits among white monochrome glyphs —
the plates cannot survive, so those sizes are a second drawing: three whole
rows in the class colours, thicker. The `.ico` carries both; `assets/icon.svg`
and `assets/icon-small.svg` are the sources.

Already done: `assets/icon.ico`, `icon.png`, and the two SVGs are the new
mark; the previous files are kept beside this session's scratchpad.

## Motion

One deliberate moment, no ambient effects.

- **Tab rail.** Every page shows it, the Relative included — always, with
  no slide. A version that slid in over the Relative after a page press and
  on hover was built and taken out again (Flynn, 28 Aug): 26 pixels of
  permanent chrome is a fair price for a rail that is simply there, and a
  rail that comes and goes is one more moving thing on a panel that is
  looked at all race.
- **Cursor.** Moves with no animation. A cursor that eases is a cursor whose
  position is ambiguous for 120 ms.
- **Armed plates.** Snap. The sim's own state changed instantly; the panel
  agrees.

Nothing on the black box animates, so there is no reduced-motion switch to
offer.

## Clicking

Every control the wheel can reach, the mouse can too (Flynn, 28 Aug): the
rail's tabs change page; the tank's chevrons and a tyre's chevrons step; an
arm plate, a tyre, `ALL FOUR` and `AUTO` toggle; `AUTO`'s margin has its own
small chevrons. A click lands the cursor on the control and then does what
the bind would have done, through the same `apply`, so the two can never
disagree. The overlay already stops being click-through while the pointer
is over a panel — that is what makes the panels draggable — so no new
plumbing is needed for the click to arrive. A hovered control lifts a
step, so the mouse has an affordance the wheel never needed.

## Absent inputs

Every value has a form for "not known yet", and none of them is a number.

| Input missing | Where | Shown |
| --- | --- | --- |
| Session length / lap count | Standings top bar | clock alone; no `L12/25` |
| Only one class in the session | Standings and Relative rows | no class bar; header count and SOF in paper |
| `avg_stint_laps` for a row | STINT bar | hatched bar against the player's average; nothing if no average exists |
| `stops_remaining` for a row | STOPS pips | nothing |
| `projected_class_position` for a row | NET | `—` |
| No lap completed | Fuel readouts | `—` in all three; tank still drawn, no FINISH flag |
| Race needs less than the tank | Fuel FINISH flag | flag sits inside the solid; hatch, if any, is all spare |
| Fast repairs unavailable for the car | Fuel arm strip | plate absent, strip closes up to three |
| No pit window yet | Pit Window | one row: `—` plate, `needs one racing lap` |
| No stop needed | Pit Window | one row: `NO STOP` plate, `tank reaches lap N` |
| Under caution | Pit Window traffic row | row absent, footer `traffic void under caution` |
| No stop yet | Tires | empty tread troughs and dashes, footer `no stop yet this session` |
| Car lacks ABS / TC / MAP | In-Car | tile absent; the strip closes up |
| Dash page name not published | In-Car | `PAGE n` |
| Wind speed zero | Weather (page and widget) | arrow hidden, `calm` |
| Wind direction not yet known | Weather widget | speed alone, no arrow, no phrase |
| Rain / fog zero or unpublished | Weather widget | tile absent, row closes up |
| Pit Stall: stall position not yet published | Pit Stall | widget hidden, as now |

Nothing is ever padded to hold a fixed height. A page is as tall as what it
has to say.

## What this costs in code

- **Fonts.** `assets/fonts/BarlowCondensed-SemiBold.ttf` + OFL, installed in
  `app.rs::install_fonts` as `FontFamily::Name("readout")`. A `readout()`
  helper beside `text_primary()` in `ui/mod.rs` so no call site spells the
  family name.
- **Hatch.** One painter in `ui/mod.rs`: a clipped run of slanted 45 %-paper
  lines at the overlay's angle, used by Fuel and by the STINT bar. Nothing
  draws a hatch today.
- **Standings.** `draw_columns` becomes one card with three tinted bands —
  `Side` and the per-column rounding logic go away, which is a simplification.
  `endurance_columns` changes from three text columns to bar / pips / delta.
  `draw_summary` grows to the strategy line. The `PitProjection` estimate is
  still not shown; its `dead_code` note stands.
- **Black box.** The title row becomes `draw_rail`; `Page::title()` and the
  title icon go, `Page::subtitle()` feeds the footer. The `Shape` enum is
  untouched: `Rows`, `Corners`, the fuel shape, `TyreInfo`, `Tiles`,
  `PitWindow` all survive; each `draw_*` is reworked inside its own function.
  The control order and `apply` do not move — this is the same split the last
  plan made, and it holds.
- **Car silhouette.** One new SVG in `assets/icons/`, plan view, following the
  `currentColor → #FFFFFF` rule in that folder's README.
- **Slants retired.** `slash_slant`, `CLASS_SLASH_SKEW`, the parallelogram
  painting behind the class tags and the plate-end cuts all go; the class
  slash becomes a `rect_filled`. Net fewer lines.
- **Tests.** The rail's page order equals `Page::ALL`; the Fuel tank's
  segments sum to the tank capacity and the FINISH flag never draws past the
  capsule; a Tire Info readout never lays pressure text over the wear bar.
  The `--demo-page` snapshot tests keep working unchanged.

## Build order

Each phase leaves the overlay usable and better than before it.

1. **Type and hatch.** Install Barlow Condensed and the hatch painter; switch
   the Standings position plate and the black box tile values to Readout.
   Visible, low risk, and it is what everything after leans on.
2. **Standings.** One card, three bands; the strategy band's new columns; the
   strategy line. Done before the black box because it is on screen all race.
3. **Chassis.** The tab rail and footer across all seven pages, with the
   Relative's slide-in. Every page gains its "where am I" at once.
4. **Fuel.** The tank. The page with the most on it and the one used on every
   in-lap.
5. **Tires.** The folded page: pressure controls over last-stop readouts, on
   the car.
6. **Pit Window, In-Car, Weather.** The hero plate; the bias bar; the wind
   tile. Small, in one pass.
7. **Pit Stall and the Weather widget.** The hatch capsule with its readout;
   the two-row weather card. The wind tile from phase 6 is reused.

## Explicitly out of scope

- What any page controls, and the wheel binds. Presses and turns do exactly
  what they do today.
- The Relative's own rows — position, class bar, name, brand mark, recent
  pace, iRating badge with licence border and change, gap; header with SOF,
  conditions and incidents; footer with grid, race clock and lap — and the
  Radar Bars. They are already
  instruments. The Relative loses its slants like everything else and gains
  the rail; nothing else on it moves. (Pit Stall and the Weather widget were
  out of scope in the first draft of this plan and are now in it — see their
  sections.)
- Any new colour. If a page seems to need one, the page is wrong.
