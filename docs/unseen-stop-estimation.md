# Estimating unseen stops and stint age

Implementation and design notes, 13 September 2026. The overlay estimates stint
age separately from its measured pit-stop history. [Live scoring verification](unseen-scoring-verification.md)
confirmed 11 completed-lap updates across 11 continuously unavailable cars in a
five-minute local session. Their live lap arrays remained unavailable throughout
those updates. This verifies a useful input, not universal scoring freshness or
the accuracy of an inferred stop.

## Implemented behavior

The bounded per-car estimator consumes successfully parsed scoring revisions,
including rows for `NotInWorld` cars. It recognizes fresh laps by completed-lap
number, retains unknown gaps when several laps arrive at once, and handles
identical consecutive lap times without mistaking them for a stale value.

It combines three sources: isolated one- or two-lap losses followed by a return
to normal pace, changes of the active driver of the same team entry, and a
conditional strategy prior learned from directly observed complete stints.
Common field slowdowns and caution periods suppress timing-based candidates.
A new witnessed boundary supersedes older estimates. A long blind interval can
widen the age without manufacturing a visit or a precise pit-exit lap.

The display uses `12` for an observed age, `~9–15` for an inferred interval, and
`?` when there is no useful basis. Tooltips identify the source. A strategy prior
assumes normal fuel cycles; early splashes and unusual starting fuel can violate
that assumption. There are no uncalibrated confidence percentages.

Estimated boundaries never increase the factual stop count, trigger `OUT`, or
enter the training history as measured fuel stints. NET computes remaining stops
at both ends of each age range. If every endpoint agrees, the class can show a
provisional `~NET`; if a car's endpoints imply different stop counts, the class
projection is withheld and the remaining-stop interval is retained.

The broader design below includes further evidence and validation work. Public
crew-history exchange, tyre-change evidence, a weighted full-race strategy
simulation and retrospective reconstruction of older boundaries are not part of
this implementation. Live captures with labelled pit visits are still needed
to measure false positives and interval accuracy.

## The most useful distinction

A car missing from live position arrays is not necessarily missing from the
scoring feed. Keep a scoring history for every classified car, including cars
whose surface is `NotInWorld`. If lap counts and last-lap times continue updating,
we can identify the lap containing a likely stop despite never seeing its pit lane
position. The live capture verified that behavior in the sampled session; a
cached value must still not be mistaken for a fresh lap.

Our recorded SDK results contain `LapsComplete`, `LastTime`, `FastestTime`,
position, `Time`, driver identity and car metadata. Neither recorded session
contains `PitStops`. The race sample's `Time` is a classification interval, not
total time spent driving. Do not subtract normal race pace from that value as if
it were cumulative elapsed time. The implementation retains the lap/time pair and validates the race-gap `Time`
field against per-car scoring progress. Other evidence below remains proposed.

## Evidence, from strongest to weakest

| Evidence | What it establishes | What remains uncertain |
| --- | --- | --- |
| Another crew client witnessed a lane visit and service | An observation we missed locally; share the event with its lap/time and source | Duplicate events, partial observations and conflicts must be reconciled |
| Observable per-car pit-road state, dwell and exit | A locally observed stop boundary | A stationary queue can resemble service; fuel amount is private |
| A validated change of active driver for the same team car | The car was stationary in its pit stall during the interval | Could follow towing; does not prove refuelling or reveal exact exit time |
| Fresh tyre-compound change | Supports tyre service during the interval | No information about same-compound changes; availability transitions are not changes |
| An isolated slow lap, or adjacent pair of slow laps, with pit-like loss | A candidate visit and a narrow lap window | Spin, damage, penalty and traffic can have the same time cost |
| Time/progress loss across an observation gap | An aggregate budget for one or more possible visits | Cannot locate a stop within a multi-lap blind interval by itself |
| Fuel-range and strategy history | Which ages and stop counts are plausible | Initial fuel, splashes, fuel saving and safety cars change the prior |
| Post-return pace change | Weak supporting evidence | Fuel weight, traffic, driver pace and tyres are confounded |

iRacing explicitly requires a driver change to happen while stationary in the
team's pit stall. This is particularly useful for endurance races: preserve the
car's history across the identity change, but record the change as new evidence
of a pit-stall visit. Validate that the SDK field represents the active driver,
rather than a roster refresh or a different entry, before using it.
[Source: Team Driving](https://support.iracing.com/support/solutions/articles/31000133594-team-driving).

Driver stint, time since a service visit and fuel stint are separate quantities.
A driver change does not prove a full tank; tyre service does not prove refuelling.
Keep those distinctions in the estimator even if the UI initially shows only
one strategy-oriented age.

## Collect the history before guessing

Key records by session/subsession, phase and car entry, retaining the entry across
driver swaps. Store completed-lap number, lap time, scoring gap, publication and
receipt times, optional live progress, pit state, driver/tyre identity and flags.

Append a lap when its completed-lap number advances, not when its lap-time value
changes. Two legitimate laps can have the same time. When the count jumps by more
than one, `LastTime` only describes the latest lap; leave the skipped laps unknown.
Session changes and replay rewinds cannot be normal racing progress. A periodically
republished unchanged row is not evidence of a newly completed lap.

Use live per-car lap fields where validated, with YAML as a coarser fallback.
Freshness of scoring and freshness of position are separate. A valid scoring row
must remain useful when live distance disappears.

## Locate a likely unseen stop

Learn a robust normal pace for each car using recent clean laps, adjusted by the
contemporaneous pace trend of comparable cars. Borrow from the same car model and
class while its own history is sparse. Personal best is a poor normal baseline.

For a candidate one- or two-lap window:

`unexplained loss = observed lap time sum - expected clean lap time sum`

Compare the loss with actual observed pit-loss distributions for this track,
car and ruleset: transit, splash, normal fuel service, tyre service and longer
visits. The timing line may split a visit across two laps, so retain adjacent
windows and de-duplicate overlapping candidates. Learn the likely split from
witnessed visits. Do not assume the slow lap number is exactly the exit lap.

Correct for common slowdowns using flags and the field's contemporaneous laps.
Do not infer a stop if most comparable cars also lost the same time. Driver swaps,
fresh compound changes and a plausible fuel age strengthen a candidate; a large
incident, long absence or continuing damaged pace should preserve repair/tow
alternatives. Missing incident data is not proof of a clean lap.

Scoring-gap change can corroborate the loss after correcting for the reference
car's pace and pit activity. Prefer several clean reference cars over a leader
who may be stopping. Lap-time excess and gap change often describe the same lost
seconds: they are correlated, and must not count as two independent votes.

Example: clean pace is 120 seconds. Consecutive laps take 145 and 153 seconds:
the combined excess is 58 seconds. Comparable witnessed stops cost 54–64 seconds,
the field remained at normal pace, and a known 27-lap stint was due. A stop in
that two-lap window is a strong candidate. Two laps later the display could read
`~2–3 laps`, with the evidence in its tooltip. The exact interval depends on the
track's timing-line/pit geometry. These numbers illustrate the method, not a
calibrated probability or a universal threshold.

## When even the lap stream was missing

Start from the last known stint age and enumerate plausible stop counts and exit
laps inside the observation gap. Compare each hypothesis with:

- Distance completed versus elapsed time, using a learned lap-time profile for
  fractional laps where possible. Uniform seconds per fraction is a rough fallback.
- The available last-lap time and relative scoring loss on return.
- The distribution of previous stint lengths, service costs, driver changes and
  tyre evidence.
- Explicit alternatives: no stop plus traffic, penalty, repair, tow or disconnect.

Maintain multiple hypotheses rather than resetting to age zero on reappearance.
For example, if the car vanishes at age 25 and returns five laps later, a normal
27–29 lap fuel cycle suggests an exit during the gap and a current age around
1–3 laps. That remains conditional on normal fuelling; saving fuel or an earlier
splash can keep a no-stop hypothesis alive. A 400-second gap is not automatically
several stops just because a normal visit costs 60 seconds.

## A car first seen halfway through the race

First consult the local scoring ledger and other crew members' observation history.
This frequently changes the problem from first-ever observation to ordinary
reconstruction. Public field-history sharing is proposed work; the current team
sync does not exchange these trackers.

Without any history, simulate plausible sequences of fuel stints up to the car's
current lap. Include distributions for the starting tank, normal refills, early
stops, fuel saving and splashes. Keep only sequences compatible with fresh timing
and identity evidence. Their remaining ages form the initial age distribution.

Example: first sighting at lap 65; normal complete fuel stints are 27–29 laps.
Under a full-start, two-normal-refill hypothesis, the most recent stop was around
lap 54–58, making current age roughly 7–11 laps. That is a useful strategy prior,
not a measured age. If early splashes are common, several other ages remain
plausible and the displayed interval must widen. `65 % 28 = 9` hides all of these
assumptions and should never be presented as an exact result.

There is another useful trick: revise the uncertain history backwards after the
next witnessed stop. If that stop is at lap 76 and a normal complete run is
27–29 laps, the preceding exit was plausibly around lap 47–49. A previously
uncertain age at lap 60 becomes roughly 11–13 under that hypothesis. This
improves future strategy and can identify which earlier slow lap was likely the
stop. Do not use the estimate as a new measured training sample.

## Display and strategy use

- `12` for age from an observed boundary; tooltip describes the observation.
- `~12` when a single estimated age window is narrow enough to be useful.
- `~9–15` for a broad or multi-hypothesis estimate; `?` if information is inadequate.
- Explain whether the basis is an individual lap anomaly, driver swap, crew
  observation or only a class fuel-strategy prior.

Do not increase the factual stop count or show confirmed `OUT` solely from a
probabilistic hypothesis. Keep estimated count and age separately. If strategy
uses uncertain ages, evaluate NET across those hypotheses and mark its uncertainty
too; an estimated stint must not silently become a precise projected position.
Avoid confidence percentages until held-out labelled data supports calibration.

Implement as a bounded per-car hypothesis filter, with later observations able to
revise recent uncertain history. Cap retained candidates and keep confirmed
observations immutable. Track confidence in the stop's existence separately from
confidence in its exit lap: a driver swap can establish a pit-stall visit while
leaving its timing broad.

## Validation and better coverage

Capture a practice session from two clients, one with good telemetry coverage and
one deliberately restricted. Use witnessed visits as labels, then hide random
intervals and entire cars from the estimator. Also test cold attachment late in
the race. Measure false stop rate, missed visits, age error and interval coverage
on held-out sessions. Include drive-throughs, splashes, double stints, rain,
driver swaps, repairs, towing, packet loss and coincident leader stops.

Max Cars controls how many cars the server is asked to transmit; Draw Cars is a
separate rendering setting. Raising the former and using an appropriate network
setting can improve observation coverage without drawing every car, although it
does not guarantee complete data and carries processing costs.
[Source: Connection Type & Max Cars](https://support.iracing.com/support/solutions/articles/31000149355-connection-type-max-cars).

The `irsdkLogAllCars` option introduced in June 2026 allows CarIdx arrays to grow
to the entries-table size. That is capacity support, not a documented promise to
recover network-unseen cars.
[Source: 2026 Season 3 Patch 1](https://support.iracing.com/support/solutions/articles/31000179039-2026-season-3-patch-1-release-notes-2026-06-12-02-).

Pit service costs must be learned for the actual ruleset. iRacing's current
regulations allow sequential or simultaneous fuel/tyre work and different
refuelling rates, so a universal one-minute threshold is particularly fragile.
[Source: Series-Specific Sporting Regulations](https://support.iracing.com/support/solutions/articles/31000179080-series-specific-sporting-regulations-branding).
