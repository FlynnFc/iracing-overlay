# Tyre compound tag

A small tag on each driver row of the Standings and the Relative saying
which compound that car is on — shown **only while the field disagrees**.
In a dry race where everyone is on the same rubber it never appears; the
moment one car gambles on wets, every row says what it is on.

## Where the data comes from

- `CarIdxTireCompound` — an `i32` per car: the index into that car's
  compound list, `-1` where the sim doesn't know (a car that hasn't left
  the garage, or a session type without the var).
- `PlayerTireCompound` — the same for the player; a cross-check only.
- `TrackWetness` and `Precipitation`, which the Weather widget already
  reads, to name the compound.

Neither var is in `vars_dump.txt` (the dump lists only what the app asks
for), so the first build step is to add both names to the `--dump-vars`
list and run it twice against a live session, per the usual rule.

### What the index means

**Correction (2026-09-05):** iRacing *does* publish compound names for
the player's car — `DriverInfo.DriverTires` in the session YAML maps
`TireIndex` to `TireCompoundType` ("Hard", "Wet"; see
`target/release/session_info_dump.yaml:1213`). The player's own
compound should be resolved by name (see
[weather-rain.md](weather-rain.md), "Own car on wets"). Whether those
names are safe for *other* cars in mixed-class fields is unverified —
`DriverTires` is a `DriverInfo` (player) block — so the per-car tags
below keep the index heuristic, with names as a cross-check for the
player's row.

For the rest of the field the
convention, as iFL03 handles it (`OverlayRelative.h:56`): in the
two-compound series that make up nearly all of iRacing's road racing,
`0` is the dry (primary) and `1` is the wet. Some series expose a
primary/alternate pair where `1` is a second dry compound. `2` and `3`
appear only in series with a longer list.

Rule used here:

| Index | Track wet, or rain falling | Track dry |
| --- | --- | --- |
| 0 | `DRY` | `DRY` |
| 1 | `WET` | `ALT` |
| 2 | `ALT` | `ALT` |
| 3 | `WET` | `WET` |
| other / -1 | no tag | no tag |

"Track wet" is `TrackWetness` above dry or `Precipitation` above 1 %,
the same test iFL03 uses. The name is decided once per snapshot for the
whole field, not per car, so two cars on index 1 never get different words.

## When the tag shows

The tag column exists for the whole panel or not at all; rows never
disagree about whether it is there.

- **Shown** when at least two competitors (pace car and spectators
  excluded, `-1` excluded) report different indices.
- **Hidden** otherwise, including when the var is absent.
- **Hysteresis**: the difference must hold for 2 s of session time before
  the column appears, and the column stays until no difference has been
  seen for 10 s. A car cycling through the garage flickers its index; this
  keeps the panel from twitching. Both constants live in the telemetry
  tracker with a comment.
- Not gated on session type. In a practice session where one car tries
  wets, seeing it is the point.

## Where it sits

Both panels already lay their right-hand runs out right-to-left, so the tag
takes a fixed column with nothing shifting:

- **Standings**: between the driver name and the iRating pill, at the
  pill's height. The name column loses the tag's width only while the
  column is shown.
- **Relative**: in `draw_row_trailing`, left of the iRating badge. Same
  reasoning.

The tag is a straight-edged block, 4 px radius, three letters in the
condensed readout face at the class-tag size. Colour is the meaning:

- `WET`: filled with the `WIND` cyan (the panel's water colour), dark text.
- `DRY`: outlined in `text_tertiary`, text `text_secondary`. The common
  case is quiet.
- `ALT`: outlined in `CAUTION` amber, amber text. Something is different
  and we can't say what.

A dimmed row (in the pits, lapped) dims the tag with the rest of the row.

## Defaults, and what happens when each input is missing

- `CarIdxTireCompound` absent → the tracker reports "no disagreement" and
  nothing ever shows. No note on the console; this is normal in some
  session types.
- A car at `-1` → no tag on that row; it doesn't count toward
  disagreement.
- `TrackWetness`/`Precipitation` absent → treated as dry, so index 1 reads
  `ALT`. Honest: we don't know.
- Config: one top-level `show_tyre_compound` (default `true`) in
  `RowOptions` beside `show_flags`, with a checkbox on the General page.
  Off means the column never appears even when the field disagrees.

## Changes, file by file

- `telemetry/session.rs`: add the two vars to `TelemetryVars` and the
  dump list; read the per-car array each tick; a `CompoundTracker` in
  `SessionTrackers` holding the disagreement timer; name resolution.
- `telemetry/snapshot.rs`: `CarSnapshot.compound: Option<Compound>` and
  `StandingsEntry.compound`, plus `TelemetrySnapshot.compounds_differ:
  bool`. `Compound` is a three-variant enum (`Dry`, `Wet`, `Alt`) with
  its label and colour role, so the UI never sees the index.
- `config.rs` / `ui/settings.rs` / `ui/mod.rs`: the toggle and
  `RowOptions.show_tyre_compound`.
- `ui/standings.rs`, `ui/relative.rs`: the tag, drawn only when
  `compounds_differ && show_tyre_compound`.
- `demo.rs`: two GT3 cars on `Wet`, the rest `Dry`, `compounds_differ`
  true, so the mockup shows the column.

## Testing

- Unit: name resolution table above (index × wetness → label).
- Unit: tracker hysteresis — a one-tick disagreement doesn't show; a
  sustained one shows after 2 s; the column holds 10 s after it ends;
  `-1` cars are ignored.
- Screenshot: `--demo` for both panels with the column on; a second demo
  run with `show_tyre_compound = false` in the scratch config to confirm
  the rows close up.

## Build phases

1. Vars, dump list, live verification of the index values in a wet
   session (or a practice where the player swaps to wets).
2. Tracker, snapshot fields, tests.
3. The tag in both panels, demo values, screenshots.

## Out of scope

- Per-car compound history (when they changed). The stint tooltip could
  carry it later.
- Tyre age. `CarIdxTireCompound` says nothing about it.
