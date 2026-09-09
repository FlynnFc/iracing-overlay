# Race Overlay

An iRacing overlay for endurance and multi-class racing: five draggable panels, a wheel-driven black box,
and a team-sync layer that lets a crew chief watch — and adjust — the car they are not sitting in.

Every screenshot here is rendered by the overlay itself from a fixed demo snapshot, so what is shown is
exactly what the code draws.

**Install, configure and build:** see [the manual](docs/manual.md).

**Contents** — [Relative](#relative) · [Black box](#black-box) · [Standings](#standings) ·
[Radar bars](#radar-bars) · [Faster class](#faster-class) ·
[Pit stall](#pit-stall) · [Status border](#status-border) · [Strategy](#strategy--fuel) ·
[Team sync](#team-sync) · [Danger drivers](#danger-drivers) · [Seat layouts](#seat-layouts) ·
[Stream mode](#stream-mode) · [CPU](#cpu-behaviour) · [Themes](#themes) · [Settings](#settings-window) ·
[Binds](#wheel-binds) · [Command line](#command-line)

---

## What this is

A single always-on-top window covering the screen, drawing panels over iRacing and passing every click
through to the sim except where a panel actually is.

- **Five panels**, each independently positioned, scaled and switched off: Relative, Standings, Radar Bars,
  Faster Class and Pit Stall.
- **A black box** sharing the Relative's frame — six pages walked with wheel buttons, with real pit service
  control (fuel, tyres, pressures, tearoff, fast repair).
- **Estimates are labelled and honest.** Anything projected says so, and anything the sim has not published is
  left blank rather than guessed — a made-up number on a pit board loses races.
- **No slanted geometry.** Straight-edged blocks throughout.

Start `race-overlay.exe` and it sits in the tray: a tick per panel, **Layout Mode** (draws every widget on
stand-in data so it can be dragged into place), **Settings…** and **Quit**. Panels are dragged with the mouse
and remember where they were put; settings apply as they are changed and are written to
`%APPDATA%\race\race-overlay.toml` once they stop changing.

---

## Relative

The cars around you in time order, with the status gutter outside the card on the left. This is page one of
the black box, so the tab rail across the top belongs to it.

<img src="docs/features/img/relative-default.png" alt="The Relative panel" width="820">

Position on its plate, car number, driver flag, name, manufacturer mark, recent pace, the iRating badge
(bordered in the licence-class colour, carrying the projected change), and the gap. A car on a different lap
is coloured — red for a car about to lap you, blue for one you have lapped — and a car in the pits is greyed
out whole. The class reads from the wash behind the name.

The **status gutter** sits outside the card, one slot per row, so an annotation about a car never competes
with the driver's name for space. In priority order: a black-flag marker, the session's fastest-lap
stopwatch, a `PIT` block while that car is in its stall or approaching, or its off-track tally. Rows with
nothing to report draw nothing.

<details>
<summary><b>Column and layout options</b></summary>

| | |
|---|---|
| <img src="docs/features/img/relative-minimal.png" alt="Optional columns off" width="400"> | **Every column is optional.** Car number, manufacturer mark, recent lap and iRating badge each switch off; the name column absorbs the space rather than leaving a hole. |
| <img src="docs/features/img/relative-narrow.png" alt="A narrowed Relative" width="400"> | **Width and depth are configurable.** Narrowing trims the name run — long names gain an ellipsis — while the fixed columns keep their measured widths. |
| <img src="docs/features/img/relative-racechange.png" alt="Position change since the start" width="400"> | **Position change** can measure this lap or the whole race, or be off. A car that has not moved shows nothing — on a panel about right now, silence is the usual state. |
| <img src="docs/features/img/relative-grid.png" alt="The grid countdown" width="400"> | **On the grid**, the clock's place is taken by how many cars are gridded and how long is left. |

</details>

<details>
<summary><b>Who the panel is about</b></summary>

| | |
|---|---|
| <img src="docs/features/img/relative-spectating.png" alt="Spectating notice" width="400"> | **Spectating.** The panel centres on the camera car and says whose race it is showing. A Relative quietly about somebody else is the one way spectator focus could mislead. |
| <img src="docs/features/img/relative-teammate.png" alt="Team-mate driving" width="400"> | **A team-mate in your car.** Same slot, same colour: for a team-mate's stint this is the normal state of the panel for hours, not a warning. |

</details>

---

## Black box

Six pages in the Relative's frame, walked with two wheel buttons. Every control is a real iRacing pit command,
and every value shown is the sim's own echo of what is armed.

| | |
|---|---|
| <img src="docs/features/img/blackbox-fuel.png" alt="Fuel page" width="400"> | **Fuel.** The tank arc with what is in it and what is armed to be added, laps of fuel at the measured burn, and the arm/clear controls. Fuel steps in whole litres because that is what the sim's command carries. |
| <img src="docs/features/img/blackbox-fuel-auto.png" alt="Auto Fuel" width="400"> | **Auto Fuel.** Keeps the load set to whatever finishes the race plus a margin in laps. The Add row goes read-only while it is on, because it is no longer yours to set. |
| <img src="docs/features/img/blackbox-tires.png" alt="Tires page" width="400"> | **Tires.** Four corners, each with its armed tick, cold pressure and three bars across the tread. A press arms the corner; a turn steps pressure by a whole psi — the sim's own increment. |
| <img src="docs/features/img/blackbox-tires-wear.png" alt="Tyre wear bars" width="400"> | **Bars show temperature or wear**, from the last stop. Before any stop the page says so rather than drawing zeros as though they were measurements. |
| <img src="docs/features/img/blackbox-strategy.png" alt="Pit Window page" width="400"> | **Pit Window.** Which lap to come in on, scored against the traffic you would rejoin into — see [Strategy](#strategy--fuel). |
| <img src="docs/features/img/blackbox-incar.png" alt="In-Car page" width="400"> | **In-Car.** Brake bias, ABS, traction control, throttle shape and dash page as the car publishes them. A reading, not a control: the sim accepts no remote writes for these. |
| <img src="docs/features/img/blackbox-weather.png" alt="Black box weather page" width="400"> | **Weather.** Track and air temperature, live rain or the declared chance, fog, and the wind as an arrow in the car's own frame. Current conditions, honestly labelled as such: iRacing publishes no forecast. |

> **Pages appear only when they can say something true.** The Pit Window page appears once the race is seen to
> need more than one stop. While spectating, Fuel and Tires appear only when team sync is actually carrying the
> driver's data — an empty page that looks live is worse than no page.

---

## Standings

The official classification, read class-relative: in a multi-class field every class is running its own race,
and an overall number reads as a driver being fourteenth in a race they are leading.

<img src="docs/features/img/standings-default.png" alt="The Standings panel" width="820">

Your own class in full on the left, other classes' leaders on the right, with the session badge and clock
across the top. Rows tumble to their new slot when positions change, so a change is seen to happen rather
than the table silently being different.

| | |
|---|---|
| <img src="docs/features/img/standings-endurance.png" alt="Endurance columns" width="400"> | **Endurance columns** — stint length, stops still owed, and the position each car is projected to finish in once everyone has taken theirs. Being a stop up on a rival is worth more than any plausible pace difference. |
| <img src="docs/features/img/standings-tyres.png" alt="Compound column" width="400"> | **Compound column and stint laps.** The compound letter appears only when the session publishes compounds; wets are ringed blue. |

---

## Radar bars

Two capsules framing your car, one per side. Colour encodes threat, not direction — once your own car is
drawn on the bar, position already says which side a car is on.

| | |
|---|---|
| <img src="docs/features/img/radar-default.png" alt="Radar bars" width="260"> | <img src="docs/features/img/radar-numbers.png" alt="Radar bars with metres" width="260"> |
| **The bars.** A car with clear air is slate; closer cars escalate. The gap between the capsules is set to frame your car's width in your own seating position. | **Gaps in metres**, optionally, for setting the range up the first time. |

---

## Faster class

<img src="docs/features/img/fasterclass-default.png" alt="The Faster Class card" width="360">

A quicker car is coming. The card appears at a configurable gap and turns red at a closer one, optionally
flashing. In a multi-class race the Relative does show this — as one row among eight, read foveally — and
being lifted out of it is the point.

---

## Pit stall

| | |
|---|---|
| <img src="docs/features/img/pitstall-approach.png" alt="Approaching the stall" width="220"> | <img src="docs/features/img/pitstall-inbox.png" alt="Stopped in the box" width="220"> |
| **On the way in.** The bar empties as the marks come up; a shorter configured range magnifies the last metre. | **In the box.** An overshoot is a reverse, a crew that will not come out, or a penalty. |

---

## Status border

The black box wears the session's state on its edge, so the one thing that matters right now is visible
without reading a single row.

<img src="docs/features/img/relative-box.png" alt="The BOX BOX status border" width="820">

The border sits flush on the widget's own edge and its left limb widens into a filled band across the whole
status gutter — the gutter's own markers draw on top of it, so the border passes under them rather than
detouring around them. The plate straddles the top edge.

| | | |
|---|---|---|
| <img src="docs/features/img/relative-caution.png" alt="Caution" width="250"> | <img src="docs/features/img/relative-lastlap.png" alt="Last lap" width="250"> | <img src="docs/features/img/relative-finish.png" alt="Finish" width="250"> |
| **CAUTION** — a full-course yellow. | **LAST LAP** — the white flag. | **FINISH** — the chequered, which outranks everything. |

**When BOX BOX fires**

- **By hand**, from a toggle on the Pit Window page — a crew chief's call, or your own reminder.
- **By fuel**, once you could not safely complete another lap after this one, and only in the lap's back half,
  so the call is a settled "turn in at the end of this lap" rather than a first-corner guess. With no measured
  burn it never fires: a made-up "box now" is the one false instruction that costs a race.

It comes on steady and only pulses inside 400 m of the pit entry, so the flash means "turn in now". A
hand-called BOX BOX clears itself once the car is on pit road. Priority: chequered > box call > white >
yellow — a box call outranks the flags behind it, because boxing under a yellow is exactly what you do.

---

## Strategy & fuel

<img src="docs/features/img/blackbox-strategy.png" alt="The pit window" width="750">

**The pit window.** Each candidate lap is scored against the traffic you would rejoin into — the tall column
is the recommendation, the red line where the tank runs out. Underneath: whether the rate the recommendation
asks for is actually being driven this lap, how much of the field could be seen (*confidence*), and the lap
the tank reaches.

**The stop-skip suggestion.** The note reading *"save to 2.58 L/lap to skip a stop"* is the Spa call
surfaced: it searches for the smallest per-lap saving that removes a whole stop from the remaining race, and
says so only when the saving is reachable. No measured burn, no known tank capacity, or a caution voiding the
projection all leave it unsaid rather than guessed.

**Known stop costs.** Stops are costed from what has actually been observed — each car's own measured time
stationary — rather than a single shared guess, so a car taking tyres every stop is projected with its real
loss. Rivals' stop lengths are what the tyre inference is drawn from.

---

## Team sync

In a team session every member's sim publishes the same world, but only the seated driver's sim publishes the
car. Team sync shares the measurements, so a spectating crew chief sees the fuel, tyres and strategy of the
car they are not sitting in — and a member who disconnects gets everything they missed.

**How it works**

- **An event ledger, not state streaming.** A closed lap is ~32 bytes every couple of minutes; a live scalars
  tick runs at 1 Hz and *only while somebody is listening*. A five-member team costs 1–2 KB/s in total.
- **iRacing's own session clock stamps every event**, so members merge them deterministically whatever the
  network delays. Accuracy comes from the timestamp, not from sending fast.
- **Recovery is the ledger read back.** A member who joins late or reconnects asks for what they are missing
  and replays it through the same handlers as live events — so a rebuilt overlay is identical to one that
  never dropped.
- **Rooms are keyed by iRacing's SubSessionID**, so members find each other with no configuration, and an
  invite code keeps strangers in the same subsession out.

**Hosting.** One member ticks *Host the relay from this PC* in Settings → Team Sync. The relay runs inside the
overlay — no terminal, no second program — and binds to that machine only. Teammates reach it through the
host's Tailscale Funnel, so only the host installs anything:

```
tailscale funnel 41230
```

The address that prints goes in every teammate's **Relay URL** as `wss://…`, with the same invite code. A host
who leaves the invite field empty gets one generated and saved the moment hosting starts; while hosting, that
overlay connects to its own relay automatically and the URL field is greyed out.

**What a crew chief sees**

| | |
|---|---|
| <img src="docs/features/img/blackbox-fuel-crew.png" alt="Fuel page while spectating" width="400"> | **The driver's real tank** — level, burn and armed load, tagged with whose car it is. Fuel can be adjusted from here: it travels the wire as intent, and the driver's own overlay arms it. |
| <img src="docs/features/img/blackbox-tires-crew.png" alt="Tires page while spectating" width="400"> | **The driver's tyre life** — the last stop's wear, temperatures and the pressures they came off at, plus what is armed for the next stop. This is what the double-stint decision is made on. |

> **Nobody's pit box is touched without consent.**
> - The driver arms *Let my team adjust my pit box* once per session. Off, remote requests are ignored.
> - A change is **announced, never prompted** — a passive "set by \<name\>" note. A driver mid-corner cannot
>   answer a dialog.
> - Every change lands in the sim's own black box, where the driver can see and override it.
> - A write applies **once, live**. A reconnecting driver replaying an hour-old ledger never has a stale
>   command sprung on them — that property is covered by an end-to-end test.

**Standing calls**

<img src="docs/features/img/blackbox-strategy-crew.png" alt="Strategy page with crew controls" width="750">

Below the window: the manual BOX BOX, the shared **fuel target**, and the standing **tyre policy**.

*Shared fuel target* — a crew chief sets a litres-per-lap target and it appears in the driver's Relative
footer, with their current burn beside it and the lap the tank reaches at that rate. Green at or under target,
amber over. It is session state: it survives driver swaps and reaches a new driver who was a spectator when it
was set, without anyone re-sending it.

<img src="docs/features/img/relative-fueltarget.png" alt="Fuel target in the footer" width="820">

*Standing tyre policy* — the answer to "do we take tyres at the next stop?", made durable. A press cycles it:

| Setting | What the driver's overlay does at each stop |
|---|---|
| `driver's call` | Nothing — the directive is off. |
| `every stop` | Arms all four. |
| `never` | Clears all four — the double-stint call. |
| `wear under N%` | Takes tyres only when the worst corner's remaining tread is at or under the threshold (default 80%, stepped by 5). With no wear measured yet it takes tyres: fresh rubber is the mistake that cannot lose a race. |

It is decided at the **pit-road entry edge**, once per entry, against the latest measured wear — so a policy
set mid-stint still lands on the very next stop — and goes through the same consent gate and "set by" note as
any other crew write.

---

## Danger drivers

You see somebody drive badly and want the overlay to remember it — now and in every future session, so the
moment they show up around you again the warning is already there.

<img src="docs/features/img/relative-danger.png" alt="Danger marks on Relative rows" width="820">

Three levels, worked into the row itself rather than added as another icon on the end: **caution** is a short
amber bar beside the position plate, **warning** takes the row's full height, and **severe** is full-height in
the alert red with the row's wash turning red behind it. The level reads from how much of the row the mark
claims.

**Right-click any row** to mark that driver or clear the mark — the driver is marked where they are seen, with
no list and no separate screen. Marks are keyed by iRacing customer id, the one identifier stable across
sessions, and are written straight away. A car whose entry publishes no customer id says so rather than
offering a mark that would not survive the session.

---

## Seat layouts

Every panel keeps two positions: a **driving** layout and a **watching** one. The watching layout is used
whenever you are not in the car — spectating, or sitting out while a team-mate drives — and it seeds itself
from the driving layout the first time it is used, so the two only diverge once you actually drag something
while watching. The General settings page names which one is in force, because a panel that "won't stay where
I put it" after a seat change would otherwise read as a bug.

---

## Stream mode

The overlay normally hides itself from the taskbar and alt-tab, which also hides it from OBS's window picker.
**Stream mode** (General settings) puts it back in both, so OBS lists *Race Overlay* as a Window Capture
source — use the *Windows 10 (1903 and up)* capture method and layer it over your game capture. The taskbar
button is the visible confirmation it took effect. It applies immediately, with no restart.

---

## CPU behaviour

iRacing is sensitive to CPU starvation, and an overlay that glitches the sim is worse than no overlay.

- **Background threads are marked for efficiency cores.** Telemetry reading, the input reader and every
  sync socket thread run under EcoQoS, which asks Windows to schedule them off the cores the
  sim wants.
- **The frame loop is paced, not spun.** The overlay draws at its own interval and sleeps in between, rather
  than repainting every time the desktop mouse moves across it.
- **Asset lookups are resolved once** and remembered — manufacturer marks and icons used to cost tens of
  thousands of filesystem calls a second across a full field.
- **Sync sends nothing nobody is listening to.** The 1 Hz scalars tick is gated on somebody being in the room,
  and quantized so an unchanged value sends nothing at all.

---

## Themes

| | |
|---|---|
| <img src="docs/features/img/theme-panel.png" alt="Panel theme" width="400"> | <img src="docs/features/img/theme-instrument.png" alt="Instrument theme" width="400"> |
| **Panel** — the default: translucent near-black cards with alternating row stripes. | **Instrument** — a tighter bezel, more opaque, no alternating fill: rows are told apart by the dividers between them. |

---

## Settings window

Every knob in the settings file, without the file. Changes apply to the live panels as they are made — the
panel behind the window is the preview — and are saved once they settle.

| Page | What is on it |
|---|---|
| General | Theme, hide-while-unfocused, hide-in-garage, off-track tally, driver flags, stream mode, which seat layout is in force, and **Reset all positions**. |
| Relative | Visibility, scale, card width, cars ahead/behind, and every optional column. |
| Standings | Visibility, scale, name-column width, other classes, stint laps, compound column, position change, endurance mode, pit-loss and tyre-change thresholds. |
| Logos | Manufacturer mark style and colour, with every known brand drawn as it will appear and a per-brand override. |
| Radar Bars | Car length, range in car lengths and in time, bar size, the gap between the capsules, gaps in metres. |
| Faster Class / Pit Stall | Each panel's own switches — including the Faster Class warn/alert gaps and flash. |
| Black Box | Auto Fuel and its margin, and whether the tyre bars show temperatures or wear. |
| Team Sync | On/off, **host the relay from this PC** and its port, relay URL, invite code, and the pit-control consent. |
| Binds | One row per wheel action, with press-to-capture. A control already bound elsewhere is taken anyway and the clash shown on both rows. |
| Launcher | The programs to start with the overlay. |

---

## Wheel binds

The black box is meant to be driven from the wheel at speed. Seven actions, bound to buttons or keys:

| Action | On a page | On the Relative |
|---|---|---|
| Next page / Previous page | Walk the tab rail | — |
| Next / Previous | Move the cursor down or up the page's rows | Scroll toward the cars behind or ahead |
| Increment / Decrement | Raise or lower the selected value | — |
| Toggle | Tick or untick the selected option | Recentre on the player |

Whatever is already held when a capture starts does not count, so a shifter resting in gear cannot be the
answer. Binds can also be set from a terminal with `race-overlay.exe --bind <action>`.

---

## Command line

| Flag | What it does |
|---|---|
| `--demo` | Render the fixed demo snapshot; iRacing is not needed. |
| `--demo-page=<page>` | Open the black box on `relative`, `strategy`, `fuel`, `tires`, `in-car` or `weather`. |
| `--demo-state=<a,b>` | Put the demo snapshot into a named state — a caution, a box call, a spectator's seat — for a screenshot. |
| `--screenshot=<path>` | Write one rendered frame to a PNG and quit. The window stays hidden for the run. |
| `--sync-host=<port>` | Run the team-sync relay from a terminal, printing the invite code and the funnel command. The settings page does the same without a terminal. |
| `--sync-join=<url,subsession,code,name,custid>` | Join a relay and talk to it — a diagnostic for checking a link end to end. |
| `--bind <action>` | Capture a wheel button for one action and save it. |
| `--list-devices` | Every input device the overlay can see. |
| `--dump-vars` / `--dump-all-vars` / `--dump-session-info` | What the sim is actually publishing right now. The first thing to reach for when a value reads wrong. |
