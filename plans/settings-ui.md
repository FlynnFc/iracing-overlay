# Settings UI — every knob in one window, no file editing

Everything the overlay can do is already configurable, and none of it is
discoverable: the knobs live in `%APPDATA%\race\race-overlay.toml`, which a
driver has to know exists, open in an editor, and get the TOML right in. For
a release that has to change. This is the spec for a settings window, with
the decisions made and the defaults chosen, so the build is a build and not a
series of judgement calls.

What already exists and stays: drag-to-position (with layout mode), the tray
menu's per-panel show/hide ticks, and `--bind` on the command line. The
window subsumes the last of those; the first two remain the fast path.

## Table of contents

- [Decisions](#decisions)
- [What is in it](#what-is-in-it)
- [Behaviour](#behaviour)
- [Edge cases](#edge-cases)
- [Build order](#build-order)
- [Out of scope](#out-of-scope)

---

## Decisions

1. **It is an egui window inside the overlay, not a second program.** The
   overlay already has an egui context, a font set, and the panel chrome;
   a settings window drawn with them looks like part of the product. A
   separate exe would need an IPC or file-watch path for every setting
   (today only the binds hot-reload), and a second window to alt-tab to,
   which the overlay deliberately doesn't have. The overlay already flips
   itself to pointer-capturing whenever egui wants the pointer, so a window
   with widgets in it is clickable with no new plumbing.
2. **Opened from the tray ("Settings…"), and from a bind.** The tray is the
   overlay's only handle; a bind is for changing something from the wheel
   mid-session without reaching for a mouse — Relative scale, most likely.
3. **Every change applies immediately and is saved on settle.** No Apply
   button, no Save button: the overlay is visible behind the window, so a
   scale slider that moves the panel as it is dragged *is* the preview.
   Written to disk the same way the black box's own settings are — once
   the value has stopped changing for a second — so a slider drag is one
   write, not sixty.
4. **The file stays the source of truth and stays hand-editable.** The
   window reads and writes `OverlayConfig`; nothing gets a second home.
   Anyone who prefers the file keeps it, and the README keeps documenting it.
5. **Defaults are the mockup values.** "Reset" on any page restores what a
   fresh install has, which is already tuned to the design mocks.

## What is in it

One window, ~640×520 at scale 1, centred on first open and thereafter where
it was left (its position is saved like a panel's). Left rail of pages, page
on the right.

| Page | Controls |
| --- | --- |
| **General** | Only show while iRacing is focused · Hide in garage · iRacing process name (advanced, collapsed) · **Reset all positions** · **Open settings folder** |
| **Relative** | Show · Scale (0.5–2.0) · Cars ahead (1–10) · Cars behind (1–10) · Scroll limit (advanced) |
| **Standings** | Show · Scale · Show other classes' leaders (off) · Endurance columns (Auto / On / Off) · Pit loss seconds · Tyre-change threshold seconds |
| **Radar Bars** | Show · Scale · Car length m · Range (car lengths) · Range ms · Show gap numbers |
| **Weather** | Show · Scale |
| **Pit Stall** | Show · Scale · Range m |
| **Black Box** | Auto Fuel · Fuel margin laps (0–10, tenths) |
| **Binds** | One row per action: current bind, **Bind…** (press-to-capture), **Clear** |

Every numeric control is a slider with the number beside it and a typed
entry on click, bounded to the ranges above — the same bounds the config
loader clamps to, so the window can never write a value the file would
refuse. Each page has a **Reset page** button at the foot.

"Show" on a widget page is the same flag the tray ticks; the two never
disagree because they are one field.

## Behaviour

- **Opening** forces the panels on: the window overrides
  `only_show_when_iracing_focused` and `hide_in_garage` while it is open,
  because changing a setting means looking at the panel it changes. Layout
  mode is *not* forced — the demo snapshot replaces live data, and a driver
  changing a setting mid-session should keep seeing the session.
- **Closing** is the window's own × or Escape. Nothing to confirm; it is
  already saved.
- **The window is a panel** for input purposes: click-through is off while
  it is open, so clicks on it land. Clicks on the sim beneath it, while it
  is open, also land on the overlay rather than the sim — the price of a
  window over a click-through sheet. Closing it restores passthrough. The
  window says so in its title bar ("close to click through to iRacing").
- **Press-to-capture for binds** reuses `input::device`: after **Bind…**
  the row reads "press a button…" and takes the next button press on any
  polled device or the next key, with a 5 s timeout back to the old bind.
  Escape cancels. A button already bound to another action is reported on
  the row ("also Relative Scroll Down") rather than silently stolen; the
  driver can clear the other.
- **Scale changes re-anchor at the panel's top-left**, which is what the
  drag position already is, so a panel grows to the right and down and
  stays where it was put.

## Edge cases

- **Config file missing or unparseable** at open: the window opens on the
  defaults and a one-line notice at its foot says the file was unreadable
  and will be rewritten on the first change. Today the overlay already
  runs on defaults in this case; this only makes it visible.
- **A panel dragged off-screen** (monitor unplugged, resolution changed) is
  what **Reset all positions** is for; it also appears in the tray so a
  driver with no panels on screen can still reach it.
- **Scale that would put a panel off-screen** is allowed — the bound is on
  the value, not the geometry — because clamping to the screen makes the
  slider stop for reasons the driver can't see.
- **Demo mode** (`--demo`) opens the window but never writes, matching
  what drag does in demo mode today.
- **Two overlays running** (launcher plus a manual start): last write wins,
  as it does today for drag. Not worth a lock for a case the launcher
  already avoids by skipping programs that are running.
- **Binds page with no wheel present**: keyboard capture still works; the
  device list shows "no controllers found" rather than an empty table.
- **The window's own bind is unbound** by default. Nothing about the box
  can be reached from the wheel without a bind, and this is one more.

## Status (2026-08-28)

Phases 1 to 3 are built: `ui/settings.rs`, opened from the tray, every row
in the table above except **Open settings folder** (the file's path is
printed on the General page instead). Escape closes it. The Binds page
captures on the input thread (`input::Actions::start_capture`) with an
8 s timeout, Escape to cancel, and a clash shown on both rows rather than
refused; the overlay now writes binds itself, so `--bind` is a scripting
path rather than the documented one. Two rows are labelled as taking effect on the next start — pit-lane
time loss and the radar's time range — because both are handed to the
telemetry thread once, at startup; making them live means sending a
`SnapshotTuning` update down the pit-request channel, which is a small
follow-up. Not yet done: a bind to open the window, the Binds page
(phase 3), and the window remembering its own position (phase 4).

## Build order

Each phase ships on its own.

1. **The window and General + per-widget Show/Scale.** Tray "Settings…",
   the rail, immediate apply, settle-and-save, Reset all positions. This is
   the release-blocking part: show/hide and size are what "customise" means
   to most people.
2. **Per-widget options** — every remaining row in the table above except
   Binds. Mostly plumbing from `ui::settings` to fields that already exist.
3. **Binds page** with press-to-capture, replacing the `--bind` CLI as the
   documented way (the CLI stays for scripting).
4. **Polish**: window position saved, Escape to close, Open settings
   folder, the unreadable-file notice.

## Out of scope

- Colours and themes. The palette is the design; a theme picker is a
  different product.
- Profiles per car or track. One config is enough until someone asks.
- Anything the sim publishes no data for — this window sets how the
  overlay shows things, not what it can know.
