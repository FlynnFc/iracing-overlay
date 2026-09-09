# race-tools — manual

Installing, running, configuring and building. For what the overlay *does*, with
screenshots, see the [README](../README.md).

Windows apps for iRacing, written in Rust:

- **race-launcher.exe** — one double-click starts your session stack:
  iRacing UI, Trading Paints, Crew Chief, Coach Dave Delta, your wheel base's
  software, the overlay below, and anything else you tick on the overlay's
  **Launcher** settings page (skipping whatever is already running). It finds
  the programs on your machine itself.
- **race-overlay.exe** — a transparent, click-through overlay drawn over
  iRacing, with five draggable widgets and a wheel-driven black box. The
  visual system is the one in `plans/standings-and-blackbox-redesign.md`:
  ink cards, three type faces (Inter for text, IBM Plex Mono for lap times,
  Barlow Condensed for a number that stands alone), one colour per meaning
  (amber is *armed*; a planned quantity is hatched), and nothing slanted.
  - **Standings** — one card in three bands: class sections (car count,
    class SOF) with each class's leaders plus a window around your own
    position — the top three, then the car ahead of you and the car behind;
    their timing (gap, fastest, last); and, in endurance mode, their
    strategy — a stint bar, one pip per stop still owed, the projected net
    position — over a strategy line that says laps left, stops to go,
    projected finish, and whether a rival is on fewer stops. A status
    gutter sits outside the card. Before a race starts the header shows the
    grid: how many cars are out of how many, and the time left to grid where
    iRacing publishes one.
  - **Relative** — the closest cars ahead and behind, with car numbers,
    each driver's national flag (the one on their iRacing profile, also
    shown in the Standings), license and iRating badges, lap-status
    coloring, and a race-clock
    footer. The iRating badge's projected change rates the whole entry
    list, so it holds still on the grid while cars are still arriving. The
    gutter beside each row marks a car under a black flag: a black flag on
    white for a slowdown warning, on black for a penalty to serve, the
    meatball for a car that has to pit for repairs, and `DQ` — and a car's
    trips off the track, as a skid mark with a tally.
  - **Radar Bars** — a map of the track beside you, one per side, drawn to
    scale in car lengths. Your own car is framed at the middle by two bumper
    bars a car length apart; the cars around you are blocks of the same
    length, so the part of one that is level with you lights up and overlap
    is something you see rather than something you work out. Colour is
    urgency, not direction — slate for a car with room around it, amber
    inside a car length, red once it overlaps, at which point the inboard
    edge of that bar lights as a solid rail. A streak off each block shows
    where that car was a moment ago, so closing and dropping back read as a
    length and a direction. Invisible until something is actually alongside.
  - **Faster Class** — a card that appears only when a car from a quicker
    class is coming up behind you, in a multi-class session: its class on a
    block of the class's colour, the car number and driver, the gap in
    seconds, and the same gap as a block sliding along a bar toward a marker
    that is you. Amber inside the warning distance, red once it is close,
    flashing until it is alongside, and gone once it has gone by. When the
    closing rate is steady enough to say, a line under the state reads
    `on you in ~9 s`. Both distances are yours to set, in seconds of gap,
    and take effect as the sliders move. Never appears in a single-class
    race, and doesn't need setting up: which class is quicker comes from
    iRacing's own class ranking, falling back to the quickest lap each class
    has actually set.
  - **Pit Stall** — a capsule that fills as you close on your own pit box:
    solid for the road covered, hatched for the road still to come, the box
    a block at the top, and the metres to go in large type beside it. It
    goes green once you're on the marks and red if you drive through them,
    reading `M LONG` until you've reversed back in. Appears in the last
    100 m of the lane and goes the moment you come to a stop in the box, so
    it's never up during the stop itself. Needs no setup: it targets
    iRacing's own stall position and measures the lane's real scale from
    your speed on the way in.
  - **Black box** — six pages on one card, paged from the wheel or by
    clicking the tab rail across the top: the Relative above, Fuel (the tank
    drawn as a tank — solid for what is in it, hatched for what the stop
    adds, a `FINISH` flag at what the race needs — and the four things a
    stop either does or doesn't, lit amber when armed), Tires (each corner's
    pressure to set over what came off it at the last stop: the tread as
    three bars, wear, hot pressure), Pit Window (which lap to box on, with
    the traffic and the fuel saving each lap either side would cost), In-Car
    (brake bias as a bar, ABS, TC, map, dash page) and Weather. Every control
    the wheel reaches can be clicked too.

  Panels are click-through and draggable; drag positions persist
  automatically. The overlay keeps out of the taskbar and alt-tab — there is
  nothing to switch to — and lives in the notification area instead; its
  native tray menu has **Open Settings** and **Quit Race Overlay**. Left-clicking
  the icon opens Settings directly. General has an enable/disable switch and a
  Configure shortcut for each widget.

  **Open Settings** opens a window over the overlay with every option below in
  it — show/hide and scale per panel, the Relative's car counts, the
  Standings' class and endurance options, the radar and pit-stall ranges,
  the Faster Class warning and alert distances,
  Auto Fuel and its margin. Layout, Content and Columns/Pages tabs keep controls
  separate. Selecting a widget previews only that widget at its saved position;
  switching pages keeps your placement. General also has *Reset all positions* for a panel dragged
  off-screen. Its **Controls** page sets the wheel and keyboard binds: press
  *Bind…* on an action, then the button or key (Escape cancels; whatever was
  already held doesn't count). Changes apply as you make them and save on their own; close
  the window (or press Escape) to make the overlay click-through again.
  Two values — the Standings' pit-lane time loss and the radar's time range
  — feed the telemetry thread at startup and take effect on the next start. On Windows 11 new tray icons start in the hidden
  overflow (the `^` chevron); drag it onto the tray to pin it.

## Installing

Download `iracing-overlay-<version>-windows-x64.zip` from the
[latest release](https://github.com/FlynnFc/iracing-overlay/releases/latest),
extract it anywhere (say `C:\iracing-overlay`), and run `race-overlay.exe`. That's
the whole install — the zip carries everything the apps need, and all settings
live in `%APPDATA%\race\`, so replacing the folder with a newer release loses
nothing.

Two Windows things to expect on first run:

- **SmartScreen** may warn about an unrecognised app, because the exes aren't
  code-signed. *More info → Run anyway.*
- The tray icon starts in the hidden overflow (the `^` chevron next to the
  clock); drag it onto the taskbar to pin it. The overlay lives there — the
  native tray menu opens Settings or quits. Widget visibility is on the General page.

The overlay can be started before or after iRacing — it waits idle until the
sim is running and reconnects if it restarts. `race-launcher.exe` optionally
starts your whole session stack (iRacing UI, Crew Chief, Trading Paints, …)
in one double-click; see below.

To build from source instead, install Rust from [rustup.rs](https://rustup.rs)
and see [Building](#building).

## Usage

Double-click the **iRacing Launcher** desktop shortcut, search **"Race
Launcher"** in the Start Menu, or run `target\release\race-launcher.exe` with
this folder as the working directory.

The launcher starts the overlay too, so no separate step is needed. To run it
on its own, `target\release\race-overlay.exe` works before or after iRacing —
it waits idle until iRacing is running and reconnects automatically if
iRacing restarts.

Which programs the launcher starts is set on the overlay's **Launcher**
settings page (tray icon → Settings…): a list of the programs sim racers run
alongside iRacing, each found on this machine automatically where it can be,
with a tick to start it or not, **Browse…** to point at one it couldn't find,
and **Add a program…** for your own. Rows start in the order shown; the
overlay is last so its window comes up on top. Tick **Also start the ticked
programs when the overlay starts** to get the same from double-clicking
`race-overlay.exe` alone.

## Configuration

- `%APPDATA%\race\launcher.toml` — `race-launcher`'s program list, written
  by the overlay's **Launcher** settings page. A catalogue row is `id`,
  `enabled` and, once chosen or confirmed, `path`; a program you added is
  `name`, `path`, `enabled` and optional `args`. Rows are in start order.
  [`config.toml`](config.toml) is the older list beside the exe: it is read
  once to seed `launcher.toml` if that doesn't exist yet, then ignored.
- `%APPDATA%\race\race-overlay.toml` — `race-overlay`'s settings: each
  widget's position, `visible` flag (also set on Settings > General), and
  `scale`, plus how many cars Relative
  shows ahead/behind, the Radar Bars' `range_ms`/`car_length_m`/`range_cars`,
  the Pit Stall bar's `range_m`, and the wheel binds (set from the settings
  window's Binds page, or by `--bind <action>` for scripting). Created
  automatically on first run/drag; no need to hand-author it.

  Deliberately outside the repo: it used to live beside the exe in
  `target\release`, where a `cargo clean` — or deleting `target\` to force a
  rebuild — took every bind and panel position with it. An older file found
  there is moved into place automatically on the next run and the original
  renamed to `race-overlay.toml.moved`.

  `[pit_stall] range_m` is what an empty bar means, in metres from your
  marks, and so also how much the bar magnifies the final metres. `50.0` by
  default; raise it for a bar that starts moving further out and fills more
  slowly, lower it to magnify the last metre. The bar's own size doesn't
  change with it, so this is purely how much lane one capsule spans.

  Note that this is a *default*: a `race-overlay.toml` written by an earlier
  version already has its own `range_m` and keeps it. Delete that line (or set
  it to `50.0`) to pick up the new one.

  `[radar] car_length_m` (`4.7`) is both the scale the bars are drawn at and
  where overlap begins, since the SDK publishes no car length. Raise it for a
  prototype, lower it for something short, and the widget gets correspondingly
  more or less cautious. `range_cars` (`3.0`) is how much track each half of a
  bar covers, in those car lengths: lower magnifies the overlap zone further,
  higher warns earlier of cars that aren't a factor yet. `range_ms` (`500`) is
  separate — it is how close in relative time a car has to be to register at
  all, and cars inside it but outside `range_cars` draw as a pip at the end of
  the bar rather than being clamped to it. `show_numbers` (`false`) prints each
  car's clear gap in metres beside it.

  `[faster_class] warn_secs` (`4.0`) is how many seconds behind you a car
  from a quicker class is when the card appears, and `alert_secs` (`1.5`)
  how close it is when the card turns red; the alert can never be set
  further out than the warning. `flash` (`true`) pulses the red between full
  and dimmed. Both distances are in seconds of relative gap — the same figure
  the Relative prints beside each car — and apply as they are changed.

  `hide_in_garage` (`true`) drops the panels while the car is in the garage,
  so they don't cover the setup screen; they come back as the car goes out.
  `only_show_when_iracing_focused` (`true`) does the same while you're
  alt-tabbed into another app. Layout mode and `--demo` override both.

  `[standings] show_other_classes` (`false`) adds a section per class the
  player isn't in, each showing just its leader. Off, the panel is the
  player's own class alone.

  `show_off_tracks` (`true`) counts each car's trips off the track and
  shows the tally in the Standings and Relative gutters: the skid mark lights
  orange while a car is off, and stays as a quiet chip with the count once it
  is back. Also on the settings window's General page.

  `show_flags` (`true`) draws each driver's national flag — the one on their
  iRacing profile — before their name in the Standings and Relative. A
  driver who hasn't picked one gets an empty slot, so names stay in one
  column. Also on the General page.

  `[blackbox] tyre_bars` (`"temps"`) is what the three bars on each wheel of
  the Tires page show, from the last stop: `"temps"` for the carcass
  temperature at each tread position, judged against the tyre's own mean, or
  `"wear"` for the tread remaining at each. Also on the settings window's
  Black box page.

  `[standings] show_stint_laps` (`true`) prints each car's current stint
  length, in laps, beside its stint bar in the strategy band. Stops still
  owed show as dots up to four and as a number past that.

  `[standings] endurance_mode` is `"auto"` by default: the strategy band
  (stint, stops owed, projected net position) and the strategy line appear
  once the session is seen to need more than one stop, and stay for the rest
  of it. `"on"` and `"off"` force it either way.

  Every widget is laid out at the exact pixel size of its design mockup, so
  `scale = 1.0` reproduces the design. Set a panel's `scale` to resize it —
  sizes are multiplied at layout time, so text stays sharp rather than being
  resampled.
- `assets/icon.ico` / `assets/icon.png` — the executable and tray icon,
  embedded at build time by [`build.rs`](build.rs); `assets/icon.svg` and
  `assets/icon-small.svg` are the sources (the small one is what the 16 and
  24 px sizes in the `.ico` are drawn from).
- `assets/fonts/` — Inter, IBM Plex Mono and Barlow Condensed, each with its
  OFL licence, embedded into the overlay at build time.
- `assets/logos/` — manufacturer marks for the Standings and Relative, every
  brand in four shapes (icon, badge, horizontal, wordmark) and two inks
  (white, brand colour), one folder per variant, looked up beside the exe
  and then in the working directory. Which variant draws is chosen on the
  **Logos** settings page — a style for every brand at once, colour where it
  reads on the dark ground, and per-brand overrides — and saved under
  `[logos]` in `race-overlay.toml`. `manifest.toml` in the folder holds the
  default pick per brand. Add a brand by dropping in
  `<lowercase-hyphenated-name>.svg`; cars with no matching file fall back to
  a short text abbreviation. See [`assets/logos/README.md`](assets/logos/README.md).

## Comparing against the design mockups

```
target\release\race-overlay.exe --demo
```

Renders every widget against a fixed snapshot built from the data in the
`design mocks/` images — the same drivers, lap times, gaps and weather — so
the widgets can be put side by side with the images they were designed from
without iRacing running. Demo mode also ignores the
`only_show_when_iracing_focused` setting, so the panels stay visible while
you look at the mockups.

Both binaries read and write `%APPDATA%\race\`: `launcher.toml` for the
launcher's list and `race-overlay.toml` for the overlay. `race-launcher
--dry-run` prints what it would start without starting anything.

## Building

```
cargo build --release
cargo test
```

Binaries land in `target\release\`. CI builds and tests every push
(`.github/workflows/ci.yml`), and pushing a version tag (`git tag v0.2.0 &&
git push origin v0.2.0`) builds and publishes a release zip automatically
(`.github/workflows/release.yml`).
