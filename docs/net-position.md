# NET position calculation

NET estimates class order after the remaining pit stops. It ranks each car on:

`live time deficit + remaining stops × total stop loss`

The preferred deficit comes from current lap numbers and track fractions,
using the focus class's measured lap curve when available. Other classes, or a
class without a measured curve, use distance fraction and class best pace as an
approximation. If a classified car has no local live progress because it is not
rendered, the whole class instead uses finite, non-negative YAML
`ResultsPositions.Time` gaps,
rebased from the class leader's same scorer reading. This keeps every contender
on one reference point; a visible car behind an absent leader cannot become a
false zero-gap leader. A gap is accepted only after that car's completed-lap
count advances, and expires if it no longer advances. Repeated YAML revisions
do not refresh an unchanged car row. Raw `CarIdxF2Time` can freeze while an
unavailable car keeps racing, so it cannot supply this fallback. That official
gap is already a
total scored deficit, so NET never adds a lap-time estimate to it. A nonleader
zero or an inconsistent scorer relation is unknown, not a zero gap. The
separate displayed F2 input is never used to price a newly completed stop.

Total stop loss includes lane transit and stationary service. Once measured,
the lane model supplies transit. Service uses the car's median, or its class
median when it has no measurement. Until both transit and service are available,
the configured loss remains the provisional total estimate; service is never
added to that total again. A newly measured total can legitimately revise NET.

Stint range uses consistent exit-to-exit lap counts, so a timing-line crossing
inside the lane does not subtract one lap from every learned tank range.
An inferred stop without an observed stationary duration resets its stint but
does not inject a zero-second service sample.

NET keeps every classified car in its class projection. A complete set of live
progress uses live gaps. Otherwise every contender uses the same scorer source,
and a `~NET` label and row tooltip mark the entire class projection as provisional.
Scorer updates can lag a newly completed stop, so this estimate may temporarily
differ from a client with full live progress. Off-world cars never supply stale
lap positions as live measurements.
NET is still withheld for a class during an ongoing pit visit or when neither
source can produce a consistent gap. Borrowed stint history may supply a
remaining-stop forecast for NET and the **STOPS TO GO** footer. The per-car
**Stops** column always shows completed stops observed in the session.

An inferred stint age also marks the class projection as `~NET`. Remaining stops
are evaluated at both ends of the age interval. If those forecasts differ for
any contender, the class has no single NET position until the ambiguity is
resolved; the footer can show a remaining-stop range. An unknown age never
becomes an assumed zero-stop strategy. Estimated visits neither increment the
observed Stops column nor enter measured stint-range or service-time history.

## Limits and validation

This remains a strategy estimate: stint history does not reveal a rival's actual
fuel load, a splash, a tyre-only stop, damage service, or future safety cars.
Changing pace and fuel saving can change the real stop count. No smoothing can
turn those unknowns into observations.

Regression tests cover paid-stop invariance, stale F2 data at exit, lane plus
service cost, timing-line crossings, whole-lap deficits, missing telemetry,
class separation, unobserved service and invalid numerical inputs. A useful live
check is to record NET before entry and after exit with the same stop plan:
after accounting for actual time lost and newly learned service, paying a stop
should not itself create or erase an advantage. Replay or live-session validation
is still needed to assess real telemetry timing and prediction error.
