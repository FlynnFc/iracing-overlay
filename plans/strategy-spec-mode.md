# Strategy page — spec mode

> **Status**: the **shared fuel target** (build step 4) is built and tested —
> `Event::FuelTarget` (protocol v4, session state that replays to late
> joiners), `TeamState::fuel_target`, `TeamSync::set_fuel_target`, the pure
> `endurance::projected_dry_lap` hit-lap math, the Relative *footer* readout
> (`TGT x.xx · current · Lnn`, green at-or-under / amber over, with slack
> against jitter), and the spec-side "Fuel target" stepper on the Strategy
> page (spectator seat only; a turn steps 0.05 L/lap, a press clears).
> Note: the readout landed in the footer centre rather than the header — the
> header row is full and the footer had the room.
> The **plan model and the stop-skip suggestion** are built and tested:
> `telemetry/race_plan.rs` (the greedy `plan_race` schedule, the
> `burn_to_skip_a_stop` search, and the `burn_for_stop_count` back-solve, all
> pure with edge-case tests — NaN/zero/negative inputs, empty tanks,
> brim-fill chains, unreachable saves) and the "save to X.XX L/lap to skip a
> stop" note on the pit-window page, gated honest-or-silent (no burn, no
> tank, caution, or no stop to skip all leave it unsaid). The back-solve is
> staged (`#[expect]`) until its "aim for N stops" control lands.
> The **standing `TyrePolicy` directive** is built and tested — protocol v5's
> `TyrePolicy` (`EveryStop` / `Never` / `BelowWear { threshold_pct }`, default
> 80) and `Event::TyrePolicySet` (session state, `None` clears, replays to
> late joiners and driver swaps), the pure `TyrePolicy::tyres_this_stop`
> decision against `TyreInfo::lowest_remaining_pct` (no wear data → take
> tyres), the store fold (`TeamState::tyre_policy`, with its setter's name),
> and the driver-side application: `TeamSync::apply_tyre_policy` fires a
> `SetAllTyres` write once per **pit-road entry edge**, attributed to whoever
> set the policy, through the same `take_pit_writes` → consent-gate path as
> any crew write. Spec-side control on the Strategy page: a press cycles
> off → every stop → never → wear-threshold → off, a turn steps the threshold
> by 5 (5–95%) while on the wear policy. Because the relay never echoes a
> frame to its sender, `TeamSync::publish` now also folds a member's own
> session-state events into their own store (`fold_own_writes`) — this fixed
> the fuel target too, which the setter previously never saw on their own
> screen.
> The five spec-face display bands and the "aim for N stops" control (its
> `burn_for_stop_count` back-solve is staged) are **not started**.

The Strategy page already computes the whole fuel picture: consumption,
laps-to-finish, stint length, stops still owed, and a traffic-scored pit
window (`ui/blackbox/pages.rs::pit_window`, `telemetry/pit_window.rs`,
`telemetry/endurance.rs`). But it is built for a driver at speed — one number
per row, withdrawn entirely while spectating (`Page::available_from`,
`Seat::Spectating`) — and a crew chief has the opposite constraints: time to
read, and the whole race to plan, not just the next stop.

Spec mode is the same page, unlocked and widened, when a watching seat has it
open. It answers the crew chief's actual questions: *how's the fuel usage
trending, are we taking tyres this stop, when are we scheduled to stop, and
should we move it?* No new widget — the driver's Strategy page grows a second
face, so the two never drift apart.

Depends on team sync (`plans/team-sync.md`) phase 2 for its inputs: while
spectating, fuel, tyre and stop history come off the ledger, so the crew
chief sees the driver's real numbers rather than the empty local scalars.

## What changes, seat by seat

The page is withdrawn in a watching seat today because its inputs are
player-only scalars that read empty off-seat. Team-sync phase 2 fills them
from the ledger, so the first change is simply: **`Strategy` is available in
`Seat::Spectating` and `Seat::TeamMate`** once the ledger is feeding it, and
`pages_for` stops withdrawing it there. The driving-seat page is unchanged.

In a watching seat the page renders its **spec face**: the same computed
values, but laid out for reading rather than glancing, with the history the
driver's face has no room for.

## The spec face

Five bands, each a thing a crew chief decides on. Every figure already exists
or is a short pure function over data that does; nothing here needs data
iRacing doesn't publish.

**1. Fuel, trending — not just now.** The driver's face shows current burn
and laps-to-finish. The spec face adds the *trend*: burn per lap over the
last ~10 laps as a sparkline, the stint average, and the delta between them
("running 0.12 under stint average"). Whether consumption is drifting is the
question behind every fuel-save call, and it is invisible in a single number.
Source: the ledger's lap-closed events, which carry per-lap `used_litres`
already.

**2. Tyre life, this stint and last.** The driver knows their tyre feel; the
crew chief needs the record. Show wear/temp at the last few stops (latched
values, the same the Tires page reads) beside the current stint's laps and
the pace fall-off across it — recent-lap pace now versus early-stint pace,
which is the measurable shadow of tyre drop. This is the "do we take tyres
this stop" band: a stint whose pace has fallen and whose last no-tyre stop
was long ago answers itself.

**3. The schedule, and the stop worth skipping.** When the current tank runs
dry (lap N, from fuel and burn), how many stops remain
(`endurance::stops_remaining`), and the planned stop laps for the rest of the
race — the greedy fill the strategy sandbox plan described, but here the
*page's own* projection, shown as a short list of "stop ~lap 34, ~lap 68".
This is "when are we scheduled to stop".

The smart part: the page also computes the **nearest burn that removes a
stop** and how far it is from now. Because a per-lap save compounds — save X
litres a lap over a 34-lap stint and the next stint starts X×34 litres richer
— a small, sustainable save can turn N stops into N−1 across a long race. So
band 3 re-runs its own schedule at progressively lower burns and finds the
first that drops the stop count, then reports the gap: *"save 0.14 L/lap to
skip a stop — you're already under that this stint."* This is the Spa-24h
call surfaced automatically rather than discovered on lap 300.

It appears **only when the save is real and reachable**, held to the same
principles as everything else here:

- *Reachable*: the required save is within a small band of the measured burn
  (a tenth or two, not "lift off and coast"). A stop only skippable by an
  impossible save is not a suggestion, it is noise.
- *Basis*: computed from measured consumption over a settled stint, not from
  one green lap; carries the same confidence as band 1 and is suppressed
  under caution, where the burn figure is meaningless.
- *Sticky*: once suggested it does not flicker in and out lap to lap — it is
  shown while the achievable save stays within reach, and withdrawn only when
  the margin is clearly lost, so a driver isn't chasing a target that blinks.

When the save is being achieved, the readout flips to confirmation: *"on for
N stops — 1.2 laps of margin in hand"*, which is the same figure the shared
fuel target (below) lets a spec lock in for the driver to hold.

**4. The next stop, and moving it.** The traffic-scored pit window already
picks which of the next laps to box on and what each emerges into
(`pit_window`); the spec face shows more of it — the top few candidate laps
with their conflict car and confidence, not just the single recommendation.
This is "when *should* we stop", the traffic answer laid beside the fuel one.

**5. The cars around us — their stops, read and predicted.** Everything the
sim publishes per car makes rival strategy partly legible, and a crew chief
reads it constantly. For each nearby car (the Relative's own neighbours):

- *Stops taken so far* (`StandingsEntry::pit_stops`) and *current stint
  length* (`current_stint_laps`) against their *average*
  (`avg_stint_laps`) — how deep into a stint they are, and so how soon they
  are due.
- *Predicted next stop*: laps until their stint reaches its average length,
  turned into an approximate lap. Shown as "~lap 71" with the same
  degrade-don't-blank rule — a car with no stint history yet shows nothing
  rather than a guess.
- *Tyres or not, last stop*: inferred from stationary time
  (`last_pit_secs` against `avg_pit_secs` and the model's `tyre_change_secs`),
  the same inference the Standings already uses. A stop long enough to have
  fit tyres is marked as such; a short one reads as fuel-only.

  **The final-stop caveat you raised is a first-class rule here.** Near the
  end of a race a rival's last stop is a fuel splash, not a full fill, so it
  is *short* — and reading short as "no tyres" would be exactly wrong. So the
  tyre inference is **suppressed, or marked uncertain, when a stop could be a
  finishing splash**: when the car's remaining laps are less than a stint's
  fuel, a short stop is ambiguous (a small fuel load, tyres or not) and the
  band says so rather than asserting no tyres. The inference is only made
  where the stop length is being driven by a normal fill.

This band is read-only and needs no new sim data — it is the whole-field
telemetry the app already reads, gathered into the one place a strategist
looks. It draws from any seat's data but earns its room in the spec face,
where there is space for a row per neighbour.

### Worked example — the double-stint call

The crew chief watches band 2 through a full stint and sees the tyres barely
gave up pace — lowest wear still high at the stop. They decide to double-stint
the tyres for the rest of the race. Rather than remember to untick tyres at
every stop, they set a **standing tyre directive** (below), and from then on
the driver's overlay arms the stops correctly on its own. Mid-race they can
change it, or make it conditional: *double-stint only while the lowest wear
value is above the threshold*, so the car takes fresh tyres automatically once
wear finally crosses it. See standing directives below.

## Adjusting, not just reading

The crew chief's read becomes an action three ways, each already speced or
below:

- **Tyre/fuel for the next stop** travels the phase-4 pit-box write path
  (`plans/team-sync.md`): toggle tyres, set the fuel load, and it arms on the
  driver's sim behind the consent gate, echoing back so every screen agrees.
  The spec face is where those controls live while watching — the same
  `PitRequest` surface, laid out with room to think.
- **What-if without acting** is the sketch idea from the strategy-sandbox
  plan, folded in here rather than a separate panel: a burn-per-lap nudge and
  a finish-margin slider that re-run band 3's schedule *locally*, showing the
  moved stop laps against the measured ones. Nothing published, nothing
  armed — a lens, not a command. When a what-if becomes a decision it is
  carried out through the write path above.

### Standing directives — a decision that outlives one stop

A one-off pit-box write sets the *next* stop. A standing directive sets a rule
the driver's overlay applies at *every* remaining stop, the way Auto Fuel
already tops up each stop on its own — so a "double-stint the tyres" call is
made once, not re-entered every 34 laps. This is the escalation your
double-stint example needs, and it lives under the same "allow team pit
control" consent tick.

- **Tyre directive**, published as a `TyrePolicy` ledger event set from the
  spec face:
  - *Every stop* (always take tyres), *never* (double-stint the rest of the
    race), or *conditional* — take tyres only when the lowest measured wear is
    at or below a threshold, **default 80%**. Above the threshold the car
    double-stints; once wear finally crosses it, the next stop takes fresh
    tyres automatically.
  - The threshold is evaluated **on the driver's overlay**, against that car's
    real wear scalars at pit entry — the same one-dataset, decide-where-the-
    -data-is rule as every pit command. A spec's seconds-old wear view never
    makes the call; it only sets the policy.
  - Last write wins, it survives reconnects as ledger state, and unsetting it
    returns stops to manual. Applied changes show the same passive "tyres:
    double-stint (set by <name>)" note as any remote write.
- Fuel has no separate directive: Auto Fuel already is the standing fuel rule,
  and the shared fuel target below is how a spec bends it.

Every directive is still a relay of intent — the sim only accepts commands
from the seated client, so the driver's overlay is what arms, always behind
the consent gate.

Folding the sandbox in here supersedes `plans/strategy-sandbox.md`: one page
with a driver face and a spec face beats a second widget that would show the
same numbers a slider's throw from the ones the pit window already draws.

## The shared fuel target — the crew chief calls a number the driver drives to

A what-if is a private lens. But once the crew chief decides "we need to save
to 2.55 L/lap to skip the last stop", that number wants to reach the driver —
not as an armed command, but as a **target to chase**. This is the third kind
of cross-seat data, distinct from both the local sketch and the pit-box
write:

- A spec **sets a fuel-per-lap target** on the spec face, two ways into the
  same number:
  - *Directly* — a slider seeded at the measured burn, so nudging it down is a
    save call.
  - *By stop count* — "aim for 5 stops over the race", which band 3
    back-solves into the stint length it implies (~34 laps) and the litres per
    lap that reaches it. The crew chief thinks in stops; the target comes out
    in litres. Either input lands on the same `FuelTarget { litres_per_lap }`.

  Setting it publishes that event to the ledger — a display intent, so unlike
  a pit-box write it never touches the sim and needs no consent gate. Any spec
  can revise it; last write wins, the sim's own rule everywhere in sync.
- The **driver's Relative header** — the strip that already names the seat
  and the spectating driver (`RelativeMeta`, `ui/relative.rs:354`) — grows a
  fuel-target readout while a target is set: **current p/lap** beside
  **target p/lap**, coloured by whether the driver is under or over it, so
  the driver has a live "am I saving enough" without opening a page. It shows
  in the driving seat (the driver chasing it) and echoes on every spec face
  (the crew watching them chase it).
- The header also shows the **projected hit lap**: at the current running
  average since the target was set, which lap the tank now reaches — so the
  target is not an abstract litre figure but "keep this up and we make lap
  96". As the laps go on and the average trues up, the projected lap firms
  toward the crew chief's intended stop lap; the driver watches it converge.

This is the spec's decision made drivable: the crew chief thinks in stops and
stints, presses it down to one number, and the driver gets one number to
beat. The target is session-scoped ledger state like everything else — a
reconnecting driver gets the current target in their backlog, and clearing it
(spec sets it back to measured, or an explicit clear) drops the header
readout.

**Delivery across a driver swap.** Because the target is current ledger state,
whoever is in the car has it — a new driver taking over mid-race gets the
standing target in their join backlog automatically, with no one re-sending
it. What must *not* repeat is the **alert**: a member who already saw the
target arrive while spectating should not get a fresh "new target: 2.55" toast
the moment they climb into the car. So the target readout is state (always
shown while set), but the arrival alert fires once per member per distinct
target value — tracked by the target's ledger sequence, which every member
already holds. Seeing it as a spec counts as having seen it; the driver's seat
inherits a target it was already watching without a redundant ping. A genuinely
new value (the crew chief revises the call) alerts again, because that is news.

## Stop-cost estimates feed every prediction

Bands 3 and 4, and the stop-count target back-solve, all need "what does a
stop cost" — tyres versus fuel separately, since a double-stint skips the tyre
time. The model for this already exists: `pit_model::PitModel::stop_cost_secs`
splits a stop into fuel (`overhead + litres × fill_rate_secs_per_litre`) and
tyre (`tyre_change_secs`) components, each a parameter with a default that
measured stops refine. The class figures you'd expect — roughly 20s for
tyres, ~40s for a full fuel load — are exactly these parameters, so the spec
face reads them from the model rather than hardcoding, and every prediction
sharpens as the race's own stops are measured (`avg_pit_secs`). A double-stint
projection simply drops the tyre component; a tyres-and-fuel stop includes
both. No new constants — the estimates are the model's, surfaced.

## Build shape

1. **Unlock.** `pages_for` / `available_from` offer Strategy in watching
   seats; requires team-sync phase 2 feeding the inputs, so it lands with it.
2. **The spec face.** A watching-seat layout branch in `pit_window` (or a
   sibling `strategy_spec` builder sharing its computation), drawing the five
   bands. The projections are pure functions in `telemetry/` per the existing
   strategy principles — the schedule, the fuel-trend delta, the
   nearest-burn-that-skips-a-stop search, and the rival next-stop / tyre
   inference (with its finishing-splash guard) are new small ones;
   tyre-life fall-off and the candidate list already exist.
3. **What-if sliders.** Local sketch state on the page, re-running band 3.
4. **The shared fuel target.** A `FuelTarget { litres_per_lap }` variant on
   the sync `Event` enum (a protocol-version bump — one of two additions here
   that touch phase 1's wire types), the target slider and stop-count
   back-solve on the spec face, and the Relative-header readout with its
   projected hit lap and once-per-value arrival alert. Independent of the
   pit-box write path: a target displays, it never arms.
5. **Write controls and standing directives.** The one-off `PitRequest`
   surface plus the `TyrePolicy` standing directive (the second new sync
   `Event` variant), both on the spec face and both behind the "allow team
   pit control" consent tick, once team-sync phase 4 exists. The directive's
   threshold check runs on the driver's overlay at pit entry.

Steps 2's pure projections, the hit-lap and stop-count back-solve math in
step 4, and the wear-threshold evaluation in step 5 can be written and tested
before the sync wiring — same pattern as everything in `telemetry/`.

## Explicitly out of scope

- Driving-seat changes. The driver's face stays as it is; spec mode is
  additive and watching-only.
- Rival fuel/tyre/strategy. Still unpublished by the sim; the pit window's
  existing traffic reasoning is as far as another car's state can be known.
- Persisted what-ifs — stale by the next race, as the sandbox plan noted.
