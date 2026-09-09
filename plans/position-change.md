# Position change since the start

A small `▲3` / `▼2` after each driver's position in the Standings and the
Relative: how many places in class they have gained or lost since the race
started. Blank at zero, blank outside a race, blank for a car we never saw
start.

## Where the start position comes from

iRacing does not publish a start position. Three sources, tried in order:

1. **The grid, watched live.** In a race session, while `SessionState` is
   `GetInCar`, `Warmup` or `ParadeLaps`, iRacing reports each car's
   `CarIdxClassPosition` as its grid slot. The tracker records the last
   value seen for every car before the state becomes `Racing`. This is
   the source whenever the overlay was up before the green, which is the
   normal case.
2. **Qualifying results in the session string.** `QualifyResultsInfo.
   Results[]` carries `CarIdx` and a zero-based `ClassPosition`. This is
   what iFL03 uses exclusively (`iracing.cpp:913`). It is the fallback
   when the overlay joined after the green, or was restarted mid-race.
   Not identical to the grid — a qualifying penalty or a driver who
   didn't take the start moves the grid — but the honest best available.
3. **Neither** (a practice-turned-race with no quali data, or the parser
   sees no `QualifyResultsInfo`) → the column is hidden for the session.

Our `session_info.rs` does not parse `QualifyResultsInfo` yet; that is a
new struct beside `SessionInfo`.

## Rules

- **Race sessions only**, and only once `SessionState` is `Racing`,
  `Checkered` or `CoolDown`. Before the green the header already shows
  the grid; in practice and qualifying there is no start to measure from.
- **Per car, not per driver.** A team swap doesn't reset anything.
- **In class.** Gained/lost against `CarIdxClassPosition`, matching every
  other position the panels show.
- **Reset** when `SessionNum` changes, or when the state drops back to a
  pre-race state (a restart after a red flag re-grids the field).
- A car that joins after the green, or is missing from both sources,
  shows nothing rather than a number from the wrong baseline.
- Zero is blank. A column of dashes says nothing; an empty slot beside a
  position that hasn't moved reads correctly.

## Where it sits

Fixed column in both panels, laid out right-to-left with the rest of the
right-hand run:

- **Standings**: left of the tyre-compound tag (or of the iRating pill
  when that column is off). Width fixed at the readout width of `▼88` so
  the pills don't move as numbers change.
- **Relative**: in `draw_row_trailing`, left of the iRating badge, same
  fixed width.

Form: the Lucide `chevron-up` / `chevron-down` already used on the iRating
badge, at the badge's chevron size, followed by the count in the mono
face at the recent-lap size. Green `SIGNAL` for gained, `ALERT` red for
lost — the same pair the iRating delta uses, so the two deltas on a row
read as one language. Dimmed with the row.

The player's own row gets it too. That number is the one most worth
having.

## Defaults, and what happens when each input is missing

- `SessionState` absent → treated as never racing; nothing shows.
- `CarIdxClassPosition` zero for a car pre-green (not yet gridded) →
  that car has no baseline until it reports one; if it never does before
  the green, source 2 is tried for that car alone.
- Config: top-level `show_position_change` (default `true`) in
  `RowOptions`, checkbox on the General page.

## Changes, file by file

- `telemetry/session_info.rs`: `QualifyResultsInfo { results: Vec<{
  car_idx, class_position }> }`, `#[serde(default)]` so its absence is
  fine.
- `telemetry/session.rs`: a `StartPositions` tracker in
  `SessionTrackers`: `HashMap<car_idx, i32>` plus the state it was
  captured in; `update(state, class_positions, quali)` each tick;
  `change_for(car_idx, current)`; reset rules above. Cache the quali
  results in `SessionInfoCache` on each YAML refresh.
- `telemetry/snapshot.rs`: `CarSnapshot.position_change: Option<i32>`,
  `StandingsEntry.position_change`.
- `config.rs` / `ui/settings.rs` / `ui/mod.rs`: the toggle.
- `ui/standings.rs`, `ui/relative.rs`: the cell.
- `demo.rs`: a spread of values (`+2`, `-1`, `0`, `+5`) across the rows.

## Testing

- Unit: tracker captures the last pre-green class position; a car first
  seen after the green gets `None`; quali fallback fills a car the grid
  never reported; reset on `SessionNum` change and on a return to
  `Warmup`; sign convention (`grid 5 → now 3` is `+2`).
- Unit: `QualifyResultsInfo` parses from a YAML fragment, and its absence
  parses to empty.
- Screenshot: both demo panels.

## Build phases

1. YAML struct and tracker with tests.
2. Snapshot fields, the cell in both panels, demo values, screenshots.

## Out of scope

- Overall (multi-class) position change. Nobody reads that number.
- Position at the last pit stop, or over the last lap. Different feature.
