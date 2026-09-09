# Black box pages — instruments, not forms

Every page is the same shape today: a column of `label: value` rows with a
cursor running down it. That shape came from iRacing's own black box, and it is
a settings form. A settings form is read by scanning, and scanning is the one
thing a driver cannot do.

This is a re-think of what each page *is*, not of how its rows are painted.

## Table of contents

- [What glanceable means here](#what-glanceable-means-here)
- [The shared language](#the-shared-language)
- [Page by page](#page-by-page)
- [What this costs in code](#what-this-costs-in-code)
- [Build order](#build-order)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## What glanceable means here

Four rules, and every page below is an application of them.

1. **One dominant thing.** Each page answers one question. That answer is the
   biggest object on it, and the eye lands on it without being aimed.
2. **Position carries meaning where the data is spatial.** Tyres belong at the
   corners of a car, not in a list that happens to be four long. A driver
   already knows which corner is which; a list makes them read the label to
   find out.
3. **Magnitude is a length or a fill, not a number.** A bar is read before it
   is looked at. `38.2 L` has to be parsed, compared against another number
   that also has to be parsed, and the comparison done in your head at the
   pit entry.
4. **State is a lit block.** "What have I armed?" should be a pattern of lit
   and unlit tiles taken in at once, not four ticks at four different heights
   of a list.

## The shared language

Unchanged from the Relative, and already built: the card, the slant, the
control plate for anything you can change, the olive for what the cursor is on,
`CAUTION` amber for armed. What changes is that these stop being applied to a
uniform list and start being arranged per page.

Three layouts cover all five pages:

- **Hero + strip** — one big answer, a control under it, a row of state tiles
  below. (Fuel, Strategy)
- **Corner grid** — four tiles in the car's own geometry. (Tires, Tire Info)
- **Tile strip** — a run of value-over-label tiles, no gutter at all. (In-Car)

## Page by page

### Fuel — "how much, and does it get me there?"

The hero is the tank, drawn as a tank. Solid is what is in it, the lighter
segment is what this stop adds, and the notch is what finishing needs. Whether
you are putting in enough is then a matter of where two edges sit, not of
subtracting two numbers at 200 km/h.

```
┌ FUEL ───────────────────────── Applies next pit-stop ┐
│                                                      │
│    ADD               ┌────────────────────┬───────┐  │
│  ‹  55.0 L  ›        │████████████▒▒▒▒▒▒▒▒│       │  │
│                      └────────────────────┴───────┘  │
│                       in tank    adding    ▲ to finish│
│                                                      │
│    18.7 laps of fuel · 13 laps left                  │
│                                                      │
│  ┌────────┐ ┌─────────┐ ┌─────────────┐ ┌─────────┐  │
│  │  FUEL  │ │ TEAROFF │ │ FAST REPAIR │ │  AUTO   │  │
│  └────────┘ └─────────┘ └─────────────┘ └─────────┘  │
│      lit = armed for this stop                       │
└──────────────────────────────────────────────────────┘
```

The arm strip is the point of the page on an in-lap. Four tiles, lit or not,
one glance, no reading.

### Tires — "which corners, at what pressure?"

The car from above. The gap down the middle is the car; the tiles are where the
wheels are. A tile lights when that corner is being replaced, so "fronts only"
is a shape rather than a sentence.

```
┌ TIRES ──────────────────────── Applies next pit-stop ┐
│         LF                            RF             │
│  ┌───────────────┐            ┌───────────────┐      │
│  │ ✓   159 kPa   │            │ ✓   159 kPa   │      │
│  └───────────────┘            └───────────────┘      │
│                                                      │
│  ┌───────────────┐            ┌───────────────┐      │
│  │     158 kPa   │            │     158 kPa   │      │
│  └───────────────┘            └───────────────┘      │
│         LR                            RR             │
│                    ALL FOUR                          │
└──────────────────────────────────────────────────────┘
```

### Tire Info — "which corner is in trouble?"

The same four positions, so it reads as the same car as the page before it.
Each corner's three tread temperatures are bars, coloured by how far off the
band they are. A hot outer edge on the right front is then a red bar in the
top-right of the panel — seen, not read.

```
┌ TIRE INFO ──────────── Data collected at last pit stop ┐
│   LEFT FRONT                    RIGHT FRONT            │
│   ▁ ▄ █    68 78 83             █ ▄ ▁    78 72 62      │
│   98 97 97%                     98 98 99%              │
│   177 kPa                       174 kPa                │
│                                                        │
│   LEFT REAR                     RIGHT REAR             │
│   ▁ ▄ █    70 78 81             █ ▄ ▁    76 72 60      │
│   98 98 98%                     98 98 99%              │
│   174 kPa                       171 kPa                │
└────────────────────────────────────────────────────────┘
```

This is the one place in the whole overlay where a number earns a colour ramp,
and today every one of these is the same white.

### Strategy — "which lap do I box on?"

The page as built answers *am I going to make it*, against one of three
abstract targets. That is the wrong question. A driver deciding a stop is
asking two concrete ones, about specific laps:

- **If I box a lap earlier or later, what traffic do I come out into?**
- **If I want to extend by a lap, what fuel-per-lap do I have to hit?**

Both are questions about a *window of candidate laps*, so the page becomes that
window. Each column is a lap you could box on, and the two rows under it are
the two answers.

```
┌ PIT WINDOW ─────────────────────────────── Estimates · good ┐
│                                                             │
│              BOX ON LAP 38                                  │
│        out P7, between #14 and #7                           │
│        #23 catches you 2 laps later, 0.8 s/lap up           │
│                                                             │
│   LAP        34    35    36    37    38    39               │
│   TRAFFIC    ▁     ▃     █     ▆     ▂     ▁                │
│   SAVE       —     —     0.08  0.19  0.31  ✗                │
│                                ▔▔▔▔▔▔▔▔▔▔▔                  │
│                                fuel window ends here        │
└─────────────────────────────────────────────────────────────┘
```

The recommendation is the hero, because on the way to the pit entry there is
time to read one thing. The strip underneath is what makes it checkable: a
driver who disagrees can see *why* lap 38 won and what boxing a lap either side
would cost.

**The SAVE row is the second question, answered per lap.** Extending to lap `L`
means covering `L − now` laps on the fuel in the tank, so the rate needed is
`fuel_level / laps`, and what has to be given up is `measured_per_lap` minus
that. A dash means no saving is needed to reach that lap; a figure is the rate
to drive to; `✗` means the lap is past what lifting and coasting can reach — the
same [`MAX_SAVE_FRACTION`] test the current verdict uses. That single row
replaces all three of the old abstract targets, because "drop a stop" is just
one particular lap in this window.

**The TRAFFIC row is the first question.** One cell per candidate lap, height
being the traffic index — see Feature 3 in `plans/strategy-and-timings.md`,
which specifies the forward simulation, the penalty weights and the confidence
levels in full. Two things from that spec matter most here: the score is an
*index of seconds of compromised running*, never a real time loss, and it is
`void` under caution rather than shown, because one caution rewrites the whole
answer.

**The fuel window is a hard edge on the strip.** The last lap the tank can
reach without saving is where the strip stops being a free choice, and it is
drawn as such rather than left to be inferred from the SAVE row going `✗`.

What this drops: the `Next stop` / `Drop a stop` / `Finish` targets, and the
single verdict line. They were an abstraction over the thing a driver actually
picks, which is a lap.

### In-Car Adjustments — "what am I set to?"

Read-only by its own subtitle, so it has no controls and needs no gutter.
A strip of tiles, value over label, in the treatment the Weather widget already
uses.

```
┌ IN-CAR ADJUSTMENTS ────────────────────────── Read-only ┐
│  ┌────────┐┌────────┐┌────────┐┌────────┐┌───────────┐  │
│  │ 54.0%  ││   3    ││   4    ││   1    ││   RACE1   │  │
│  │  BIAS  ││  ABS   ││   TC   ││  MAP   ││   DASH    │  │
│  └────────┘└────────┘└────────┘└────────┘└───────────┘  │
└─────────────────────────────────────────────────────────┘
```

## What this costs in code

This is the part that makes it a redesign rather than a restyle, and it is
worth being plain about.

`rows_for(page, …) -> Vec<Row>` returns a linear list; `BlackBox` holds a
`cursor: usize` into it; `apply(action, &rows, …)` walks it. Every page being a
list is currently baked into the input model as well as into the drawing.

The input model does not have to change, and should not: an encoder turn has to
walk *something* in a fixed order. So the split is:

- A page keeps a **linear control order** — what the cursor walks and what a
  press acts on. `apply` is untouched.
- A page gains a **layout** that says where each of those controls is drawn,
  and what non-interactive furniture surrounds them.

Concretely, `rows_for` becomes a function returning a `PageLayout` — one of the
three shapes above — each carrying its controls in cursor order. `draw`
dispatches on the shape instead of looping over rows. The pit-command paths
(`pages::request_for`, everything in `telemetry::pit`) do not move at all.

The row grammar already built — stripes, grooves, grey labels, the olive
cursor, the control plate — is not wasted: the control plate and the cursor
treatment carry into all three layouts. The striped row list itself survives
only where a list is genuinely the right shape, which after this is nowhere
except the Relative.

## Build order

All five pages are built, in this order:

1. **Tires** — the corner grid, and the proof of the layout/control split. One
   cursor stop per wheel: a press arms it, a turn sets its pressure.
2. **Fuel** — the tank drawn as a tank, and the arm strip.
3. **Strategy** — twice. First as a verdict against three abstract targets,
   then replaced entirely by the pit window: a set of candidate laps anchored
   on the lap the tank runs to, with the stop count as the headline row. The
   three targets are gone, and `telemetry::strategy` with them; "drop a stop"
   was only ever one particular lap in this window.
4. **Tire Info** — the same corner geometry as Tires, carrying what came off
   the car. Temperatures are coloured against *each tyre's own mean* rather
   than an absolute band: a target band is a property of car, compound and
   track temperature and none of those is published, but which edge of a tyre
   is doing the work needs none of them.
5. **In-Car Adjustments** — a tile strip. No plates anywhere on it, because a
   plate means "you can change this" and nothing here can be.

6. **Weather** — a tile strip, with a car-relative wind arrow rather than a
   compass. No forecast exists to draw; `ChanceOfRain` is a session setting.

`RowKind::Columns` went with the old Tire Info, and the striped row list now
survives only on the Fuel page's controls.

## Explicitly out of scope

- Changing what any page can control. This is what the pages look like and how
  they are arranged, not what they do.
- The Relative page, which is the Relative widget and already an instrument.
