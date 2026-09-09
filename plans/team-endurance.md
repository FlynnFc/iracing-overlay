# Team endurance — the overlay through a driver swap

In a team event you drive a stint, climb out, and a team-mate takes the car
while you watch from the spotter seat; a couple of hours later you climb back
in. The overlay should be right the whole way through, with nothing to set and
nothing to restart: Relative and Standings keep describing *your team's race*,
the pages that are about the seat step aside while somebody else is in it, and
the fuel and pace models carry across the swap instead of learning a lie from
it.

## Table of contents

- [What already works, and why](#what-already-works-and-why)
- [What goes wrong today](#what-goes-wrong-today)
- [The one thing nobody knows yet](#the-one-thing-nobody-knows-yet)
- [Decisions](#decisions)
- [The seat](#the-seat)
- [What each widget does in each seat](#what-each-widget-does-in-each-seat)
- [Data continuity across the swap](#data-continuity-across-the-swap)
- [Incidents](#incidents)
- [Edge cases](#edge-cases)
- [Changes, file by file](#changes-file-by-file)
- [Testing strategy](#testing-strategy)
- [Build phases](#build-phases)
- [Explicitly out of scope](#explicitly-out-of-scope)

---

## What already works, and why

Every widget is centred on `focus_car_idx`, resolved each tick by
`resolve_focus_car` (`telemetry/session.rs:1274`). Rule one is "if the
player's own entry is a real competitor, focus is `player_car_idx`", and
`player_car_idx` is `DriverInfo:DriverCarIdx` from the session YAML
(`session.rs:1219`). In a team session that index names the **team's car**,
not whoever is in the seat. So while a team-mate drives:

- **Relative, Standings, Radar** read only the per-car `CarIdx*` arrays,
  indexed by focus. All of them keep working. The team car's row in
  `Drivers[]` is rewritten by iRacing at the swap and re-parsed on the next
  `SessionInfoUpdate` bump (`session.rs:1207`), so the row shows the
  team-mate's name without anyone doing anything.
- **Weather** is the session's; nothing about it is per-player.
- **Pit controls are already safe.** Every press and the Auto Fuel path check
  `pit_service.in_car` (`IsOnTrack`) first (`ui/blackbox/pages.rs:58`,
  `ui/blackbox/mod.rs:963`), so nothing is sent to the sim from the spotter
  seat.

`plans/spectator-focus.md` called this out ("Team endurance needs no change")
and asked for it to be confirmed in a team session before writing code for it.
This plan is what remains once that confirmation is treated as a phase of the
work rather than a precondition for it.

## What goes wrong today

Everything that goes wrong comes from one fact: the **player scalars** —
`FuelLevel`, tyre temps and wear, `dcBrakeBias`, `LapLastLapTime`, `Speed`,
`PlayerCarInPitStall` and the rest — describe *the car the player is sitting
in*. Out of the car they are either frozen or zero. The code has a guard for
that condition, but the guard is `spectating = focus != player`
(`session.rs:1733`), and in a team session focus **is** the player, so it
never fires.

1. **Fuel, Tires, In-Car and Pit Window stay on screen reading zeros.**
   `pages_for` (`ui/blackbox/mod.rs:402`) withdraws the four car pages only
   while spectating. Its own doc comment says why they must be withdrawn:
   "zeros that look like measurements are worse than a page that isn't
   there." A team-mate's stint is exactly that case, unguarded.
2. **The fuel model can only survive the swap by luck.** `FuelTracker::update`
   (`session.rs:958`) records `last_level - level` at each lap change. If
   `FuelLevel` collapses to zero when you climb out, the drop is discarded
   only because the swap lap touched pit road; the refill on your return is
   discarded as "the tank got fuller". It works, but it is not designed to,
   and nothing the team-mate burns is learnt.
3. **Pace reads a frozen scalar.** `LapPace` is fed `LapLastLapTime` whenever
   `focus_is_player` (`session.rs:1415`). Out of the car that scalar stops
   moving (or reads zero — see the next section). At each lap change the
   tracker pushes the same stale figure into its window again.
4. **The pit stall bar draws for the team-mate's stops.** `is_driving` is
   `focus_is_player` (`session.rs:1857`), so the bar appears as the team-mate
   rolls into the box — measured with `Speed`, which is *your* speed, which is
   zero. A scale calibration on zero speed is a bar in the wrong place.
5. **The incident count is the wrong number.** The header shows
   `PlayerCarMyIncidentCount` against the session's limit
   (`session.rs:1122`, `ui/relative.rs:295`). In a team event the limit is
   applied to the **team's** count.
6. **Nothing says who is driving.** The Relative and Standings headers have a
   "◉ name" notice for spectating (`ui/relative.rs:272`,
   `ui/standings.rs:607`), but a team-mate's stint shows nothing.

## The one thing nobody knows yet

Which player scalars iRacing keeps publishing to a **crew member** — someone
in a team session, not in the car, watching their own car. The sim's own UI
gives a hint: a crew member can open the fuel black box but not the tyre or
in-car ones, which suggests fuel level and the pit-service request state
(`PitSv*`, `dp*`) are shared with the crew while physics-side readings are
not. What the public record actually says (2026-08-29):

- The community SDK docs define `IsOnTrackCar` as true when "the players
  car is on the track and the players sim is running physics for the car",
  and add that it **"will remain false if the car is under the control of
  another driver in a team event"**. So it is *not* the crew's "my car is
  out" signal; `CarIdxTrackSurface[player]` is.
  (https://sajax.github.io/irsdkdocs/telemetry/isontrackcar.html)
- The TeamTactics project, built for exactly this case, states: "Some of
  the telemetry data in iRacing is delayed for all currently not driving
  team members because of anti-cheat reasons" — and lists `FuelLevel`,
  `LapLastLapTime`, `PitSvFlags`, `OnPitRoad` among what its client reads
  from every team member. Delayed, then, not absent.
  (https://github.com/simracingtools/teamtactics)
- The three incident counters are documented in iRacing's own 2016 S3
  release notes: `PlayerCarTeamIncidentCount` is "incident count this
  session for your whole team"; in a non-team event all three are equal.
- iRacing's black-box article says only that the F8 In-Car box "cannot be
  adjusted by the Spotter or Crew Chief" — which is the sim's own
  statement that the fuel box *can* be.

Nothing public says how long "delayed" is, or what `IsInGarage` reads for
the crew. The unknowns, each with a **default** the plan builds to and what
changes if the dump says otherwise:

| Variable, from the spotter seat            | Default assumed | If the dump says otherwise |
|--------------------------------------------|-----------------|----------------------------|
| `FuelLevel`, `FuelLevelPct`                | published, delayed by some seconds | if absent/zero: treated as unpublished; Fuel page shows "—" for the tank, model pauses |
| `PitSvFuel`, `PitSvFlags`, `dpFuelFill`…   | published       | Fuel page controls withdrawn while out of the car |
| `LapLastLapTime`, `LapBestLapTime`         | published, delayed | no change — per-car arrays are used out of the car regardless |
| tyre temps / wear / pressures, `dc*`       | not published   | Tires / In-Car stay available out of the car |
| `IsOnTrackCar`                             | **false** while a team-mate drives (documented) | not used for the seat; in the dump only to confirm |
| `IsInGarage`                               | false for crew  | **must know**: if true, `hide_in_garage` blanks the whole overlay for the team-mate's stint |
| `PlayerCarTeamIncidentCount`               | present         | keep showing `MyIncidentCount` |
| `PitCommand` broadcast from crew           | ignored         | Phase 3: let the spotter arm the fuel load |

Every default is chosen so that the code is **correct under either answer**
wherever that is possible (fuel level is trusted out of the car only when it
reads above zero; pace never reads the scalar out of the car), and where it
is not possible the variable is listed here so Phase 0 settles it.

## Decisions

1. **The seat, not the focus, gates the player scalars.** A new per-tick
   value, `driving = IsOnTrack`, is what decides whether a player scalar
   means anything. It is true exactly when the player is in the car with the
   physics running. The existing `focus_is_player` keeps its job — which car
   the widgets are about — and stops being used as a proxy for "the scalars
   are live".
2. **A team-mate's stint is a third state, not spectating.** Spectating
   (`RelativeMeta::spectating`) means *somebody else's car*: it moves focus to
   the camera, withdraws the car pages, and names the driver in the accent
   colour. A team-mate's stint is *our car, somebody else in it*. Focus does
   not move, Pit Window stays, and the notice says who is driving. Modelling
   it as spectating would be a lie the rest of the code would then have to
   work around.
3. **Team-ness comes from the YAML, not from the scalars.**
   `WeekendInfo.TeamRacing` says whether this is a team session;
   `DriverInfo.DriverUserID` is the player; `Drivers[].UserID` is who is in
   each car right now. "A team-mate is driving" is
   `team_racing && drivers[player_car_idx].user_id != driver_user_id`. This
   cannot flap tick to tick and does not depend on any unverified scalar.
4. **Fuel level becomes optional.** `PitService::fuel_level_litres` is a
   measurement, and out of the car it may not exist. It becomes
   `Option<f32>`: `Some` when driving; out of the car, `Some` only if
   `FuelLevel` reads above zero (the crew-gets-fuel case), else `None`. Every
   consumer already has an honest "don't know" rendering (the Fuel gauge's
   "—", Pit Window's `Verdict::Waiting`); they get `None` instead of `0.0`.
5. **Lap times come from the per-car arrays whenever not driving.** The
   existing split at `session.rs:1415` switches from `focus_is_player` to
   `driving`. Driving keeps the scalar path that has been run on track; out
   of the car — spectating or crew — reads `CarIdxLastLapTime` /
   `CarIdxBestLapTime` for the focus car. The pace window carries through
   the swap on the team car's real laps.
6. **The pit stall bar is the driver's.** `is_driving` becomes `driving`.
   It does not draw for a team-mate's stops. (Their overlay draws theirs.)
7. **Auto Fuel is the driver's too.** It stays gated on `in_car`. Two
   overlays on one team must not both arm loads. The latched target is still
   shown as a readout on the spotter's Fuel page, so the spotter can see what
   the driver's overlay will do.
8. **Pages available in each seat are a function of the seat.** See the
   table below. `pages_for` reads the seat, not just `spectating`.
9. **Incidents show the number the limit is applied to.** Team count in a
   team session, own count otherwise.
10. **No new config.** None of this needs a setting. A team session is
    detected; the seat is detected; the pages follow.

## The seat

```rust
/// Where the player is relative to the car the widgets are about.
pub enum Seat {
    /// In the car, physics running (`IsOnTrack`). Every player scalar is live.
    Driving,
    /// Our car, a team-mate in it. Focus stays on the car; the seat pages
    /// step aside; the name is who is driving.
    TeamMate(Arc<str>),
    /// Our car, nobody in it: garage, tow, or between stints.
    OutOfCar,
    /// Somebody else's car, from the camera. Today's `spectating`.
    Spectating(Arc<str>),
}
```

Resolved every tick in `build_snapshot`, after focus:

1. `focus != player` → `Spectating(name of focus driver)`. Unchanged.
2. `IsOnTrack` → `Driving`.
3. `team_racing && drivers[player].user_id != driver_user_id && car_on_track`
   → `TeamMate(drivers[player].user_name)`, where `car_on_track` is
   `CarIdxTrackSurface[player] != NotInWorld`. Not `IsOnTrackCar`: that
   is documented to stay false while a team-mate drives.
4. Otherwise `OutOfCar`.

Order matters: `IsOnTrack` is checked before the YAML test so the moment you
are back in the car you are `Driving`, even if the YAML has not yet been
re-published with your name on the entry.

`Seat` lives in `TelemetrySnapshot` and replaces nothing yet:
`relative_meta.spectating` stays as the accessor the widgets already use, set
from `Seat::Spectating`. A follow-up can fold it in.

## What each widget does in each seat

| Widget / page    | Driving | TeamMate                                   | OutOfCar          | Spectating |
|------------------|---------|--------------------------------------------|-------------------|------------|
| Relative         | as now  | as now; header notice "⇄ NAME"             | as now            | as now     |
| Standings        | as now  | as now; header notice "⇄ NAME"             | as now            | as now     |
| Radar            | as now  | as now (follows the car)                   | as now            | as now     |
| Weather          | as now  | as now                                     | as now            | as now     |
| Pit Window       | as now  | **kept**; fuel inputs `None` → `Waiting` where fuel is unknown, stint projection still runs | as now | withdrawn (as now) |
| Fuel             | as now  | kept as a readout: tank "—" when unknown; every press is refused by the existing `in_car` gate (see D7, Phase 3) | as now | withdrawn |
| Tires            | as now  | withdrawn                                  | as now            | withdrawn  |
| In-Car           | as now  | withdrawn                                  | as now            | withdrawn  |
| Pit stall bar    | as now  | hidden                                     | hidden (as now: `IsOnTrack` is false) | hidden |
| Auto Fuel        | as now  | off (readout only)                         | off (as now)      | off        |

`OutOfCar` withdraws nothing. In a solo session — the garage, a tow — the
scalars are still the player's own car's, so every page is as honest as it
was; and this is what keeps the seat off a solo driver's racing entirely.
The one team case it leaves, a team car in the garage with nobody in it, is
already covered by `hide_in_garage`.

The notice: same slot as the spectating notice (`ui/relative.rs:272`,
`ui/standings.rs:607`), same accent colour, glyph `⇄` (U+21C4) instead of
`◉`, text is the driver's name as Relative already abbreviates names. It is
the normal state of the panel for hours at a time, so it is not amber.

When `settle_page` finds the showing page withdrawn it already moves to the
first available page (`ui/blackbox/mod.rs:772`). Climbing out on the Tires
page lands on Relative; climbing back in does not move the page — nothing
was taken away.

## Data continuity across the swap

**Fuel per lap.** `FuelTracker::update` takes `Option<f32>`. At a lap change
with `None` the lap is not recorded and `last_level` becomes `None`; the
window is kept. The first lap after fuel returns has no `last_level` and is
not recorded either. Under the default (crew sees fuel) the tracker learns
the team-mate's laps like anyone else's — the swap lap itself touched pit
road and is discarded, as today.

**Fuel used this lap** (`LapFuelUse`, `session.rs:927`): the smoothed level
is reset when the input goes `None`, so the first readings after fuel
returns are seeded, not smoothed up from zero.

**Pace.** Per D5. On the swap back, `LapPace` has been fed the per-car array
throughout, and the first lap in the car is the out-lap, tainted by pit
road. No discontinuity.

**Stint history and pit-window projections** are per car, from
`CarIdxTrackSurface` and `CarIdxLap`, and already survive.

**Tank capacity** is latched from the YAML (`session.rs:1795`); the
`FuelLevel / FuelLevelPct` estimate is only attempted while driving.

**Late launch.** If the overlay starts mid-race with a team-mate already
driving, everything above degrades to "not yet known" rather than wrong:
no fuel history, no pace history, Pit Window `Waiting`, Fuel page tank per
the crew-fuel answer. It fills in as laps complete, exactly as at a race
start.

## Incidents

`PlayerCarTeamIncidentCount` is looked up alongside
`PlayerCarMyIncidentCount`. `RelativeMeta::incidents` becomes the team count
when `team_racing` and the team var is published, else own count. The
header text is unchanged (`"12/17"`). Own count is not shown separately —
it is not the number that ends the race.

## Edge cases

- **A delayed fuel level.** TeamTactics reports crew telemetry is delayed
  for anti-cheat reasons. A constant delay cancels between consecutive lap
  boundaries, so the per-lap figure is unbiased; only the "in tank now"
  readout lags, which the crew-seat Fuel page can live with.
- **YAML re-publish lag at the swap.** Sequence out: `IsOnTrack` drops first
  (seat → `OutOfCar` for a few seconds), then the YAML names the team-mate
  (→ `TeamMate`). Pages withdrawn at the first step stay withdrawn; Pit
  Window is available in both; the notice appears when the name does.
  Sequence in: `IsOnTrack` rises → `Driving` immediately (D3 ordering).
- **`Drivers[].UserID` missing from the YAML** (older sim, or a parser
  miss): `TeamMate` cannot be detected and the seat is `OutOfCar` — which
  withdraws the same pages and only loses the name in the notice. Safe.
- **Team session, driving alone all race.** `TeamRacing: 1` with one driver.
  Seat is `Driving` throughout; incident count switches to the team count,
  which equals own count. Nothing else changes.
- **Team-mate in the car in the garage.** `CarIdxTrackSurface[player]` is
  `NotInWorld` → `OutOfCar`. If Phase 0 finds `IsInGarage` reads true for the crew, the
  garage-hide gate (`app.rs:537`) must additionally require `driving`, or
  the overlay disappears for the whole stint. This is the one place a wrong
  guess blanks everything, hence the bold row in the table above.
- **Spectator entry in a team session.** `is_competitor` false → focus from
  camera → `Spectating`. Unchanged.
- **Demo mode** is a driver's view; `Seat::Driving`. `--demo-page` gains
  nothing.
- **Two team-mates both running the overlay.** Each sees their own seat.
  Auto Fuel arms from the driving one only (D7).

## Changes, file by file

- `telemetry/session_info.rs` — parse `WeekendInfo.TeamRacing: i32`,
  `DriverInfo.DriverUserID: Option<i32>`, `Driver.UserID: Option<i32>`
  (`default` on all three; nothing reads `TeamName`, so it is not parsed).
- `telemetry/session.rs`
  - `CANDIDATE_VARS`: new `"team"` group — `IsOnTrackCar`, `PlayerCarIdx`,
    `CamCarIdx`, `LapLastLapTime`, `LapBestLapTime`, `Speed`,
    `PlayerCarMyIncidentCount`, `PlayerCarTeamIncidentCount`,
    `PlayerCarDriverIncidentCount`, `PlayerCarTowTime`, `IsInGarage`,
    `CarIdxLastLapTime`, `CarIdxBestLapTime`. (Phase 0.)
  - `TelemetryVars`: `team_incidents: find("PlayerCarTeamIncidentCount")`.
  - `SessionInfoCache`: `team_racing`, `driver_user_id`; `DriverMeta` gains
    `user_id`.
  - `build_snapshot`: compute `driving`, `car_on_track`, `seat`; switch the
    lap-time source, `is_driving`, the tank estimate and the fuel level on
    `driving`; incidents per team rule.
  - `FuelTracker::update`, `LapFuelUse::update`: `Option<f32>` level.
- `telemetry/snapshot.rs` — `Seat` enum; `TelemetrySnapshot::seat`;
  `PitService::fuel_level_litres: Option<f32>`.
- `telemetry/pit_window.rs` — `Player::fuel_level_litres: Option<f32>`;
  `None` reaches the same `Waiting` verdict a zero per-lap does today.
- `telemetry/pit.rs` — `fuel_to_finish_litres` unchanged (takes per-lap,
  not level).
- `ui/blackbox/mod.rs` — `pages_for` reads `snapshot.seat`;
  `Page::available_from(&Seat)` replaces `is_about_the_player_car`;
  `auto_fuel_request` unchanged (already gated on `in_car`). The private
  row-position enum that was also called `Seat` is renamed `RowPlace`.
- `ui/blackbox/pages.rs` — `fuel`: gauge takes `Option` tank (Phase 2). The
  controls stay listed out of the car: an empty control list is what
  `PageLayout::is_bare` reads as "waiting for iRacing", and `request_for`
  already refuses every press while `!in_car`.
- `ui/blackbox/*` gauge drawing — "—" for an unknown tank, no bar.
- `ui/relative.rs`, `ui/standings.rs` — team-mate notice beside the
  spectating one; `RelativeMeta` / `TopBarMeta` gain `team_mate:
  Option<Arc<str>>`.
- `app.rs` — garage hide additionally requires `driving` **only if Phase 0
  shows `IsInGarage` true for crew**.
- `demo.rs` — `seat: Seat::Driving`, `fuel_level_litres: Some(..)`.

## Testing strategy

Unit, in the modules that own the logic (the codebase's existing style —
`#[test]` beside the code, named as sentences):

- `resolve_seat`: each row of the resolution order; the YAML-lag sequence
  both directions; the player's car `NotInWorld`.
- `FuelTracker`: a `None` gap in the middle of a stint records nothing and
  keeps the window; refill after `None` is skipped.
- `LapFuelUse`: re-seeds after `None`.
- `pages_for(seat)`: every cell of the page table.
- `Page` settling when Tires is withdrawn mid-stint.
- incidents: team count chosen only when `team_racing` and published.
- `session_info`: YAML with and without `TeamRacing`/`UserID` fields.

Live, once per phase, in a team practice session (hosted, one team-mate):
climb out, watch a lap, climb back in. Check the page set, the notice, the
fuel gauge, the pace figure on Pit Window, and that the pit stall bar stays
away for the team-mate's stop.

## Build phases

Status (2026-08-29): **Phase 0 built. Phase 1 built** — `Seat`, its
resolution and tests, the page set, the header notice in both widgets, pit
stall gating, team incident count; 361 tests, clippy clean. Phases 2 and 3
wait on the spotter-seat dump.

**Phase 0 — instrument and find out.** Add the `"team"` group to
`CANDIDATE_VARS` and the four YAML fields. Flynn runs
`race-overlay --dump-vars` and `--dump-session-info` from the spotter seat
while a team-mate is on track, and once more from the car. The two dumps
answer every row of the unknowns table. Ships nothing user-visible; ~30
lines.

**Phase 1 — the seat.** `Seat`, its resolution, the notice, `pages_for`,
pit-stall gating, incidents. After this the pages stop lying and the header
says who is driving. Usable on its own.

**Phase 2 — continuity.** Optional fuel level through the trackers, the
gauge and Pit Window; lap-time source on `driving`; garage-hide fix if
needed. After this the models carry across the swap.

**Phase 3 — crew fuel (conditional).** Only if Phase 0 shows the `PitCommand`
broadcast is honoured from the spotter seat: Fuel page controls come back for
`TeamMate`, gated on a new `PitService::can_command` rather than `in_car`;
Auto Fuel stays driver-only. If the sim ignores crew commands, this phase is
dropped and the Fuel page stays a readout out of the car.

## Explicitly out of scope

- Sharing fuel/pace history between team-mates' overlays. There is no
  channel for it and the per-car arrays already give each overlay the same
  laps.
- A driver-change countdown, stint timer or drive-time-limit tracker.
  Real enduro need, separate spec.
- Showing the *previous* driver's name, or a stint-by-driver log.
- Any change to focus resolution — `spectator-focus.md` still describes it.
