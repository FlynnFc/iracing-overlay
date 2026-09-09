# Strategy for specs — the pit window tab, unlocked and enriched off-seat

> Supersedes the standalone widget sketched in an earlier revision
> (`strategy-sandbox.md`, deleted): same questions, answered by enhancing the
> **existing Strategy page** instead of building a second strategy surface.

The Strategy page already answers the driver's version of these questions:
which lap to box on (traffic-scored candidates, `telemetry/pit_window.rs`),
what the stop costs (`pit_model`), what save rate changes the plan. While
spectating it is withdrawn — `Page::Strategy`'s `available_from` excludes
`Seat::Spectating` — for one reason only: its inputs (`snapshot.pit_service`,
fuel level, burn) are player-only scalars that a spectator's sim never
publishes.

Team sync (`plans/team-sync.md`) phase 2 is precisely those scalars arriving
over the ledger. So the plan is: **feed the same page from the ledger while
watching, hand it back to specs, and add the sections a crew chief needs
that a driver at speed never looks at.**

## The rule

- Seated: the page is exactly what it is today. Nothing moves, nothing is
  added — a driver's strategy surface stays readable at 200 km/h.
- Watching **and synced to the driver's car**: the page comes back, fed from
  the ledger, with the spec-only sections below. The existing withdrawal
  logic in `pages_for` stays the gate; "synced" just becomes one more thing
  it knows.
- Watching and *not* synced: withdrawn as today. No honest inputs, no page —
  the "no row without a basis" principle from `strategy-and-timings.md`
  already decides this.

## What a spec sees

Top of page: today's window — candidate laps, save target, stop cost —
computed from ledger-fed inputs instead of local scalars. Below it, the
spec-only sections:

**Burn.** Fuel per lap: the current figure, and the last handful of laps as
a sparkline-style row so a trend (saving working, pace dropping) is visible
rather than inferred. Source: the ledger's lap-closed events, the same
figures every member holds.

**Stops so far.** One row per completed stop of the team's car: lap,
stationary seconds, litres added, tyres taken or not, the stint length that
set ran, and the wear it came off at. This is the "should we take tyres"
row — three stops of "came off at 70% after 28 laps" *is* the decision, laid
out to be read. Sources: a new `OurStop` ledger event (below); rival-stop
context is already per-car in `StandingsEntry` (`last_pit_secs`,
`avg_pit_secs`, stint laps) and needs nothing new.

**Schedule.** When the car is projected to stop (fuel-limited lap from the
current burn), and