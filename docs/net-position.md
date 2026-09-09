# NET position calculation

NET estimates class order after the remaining pit stops. It ranks each car on:

`live time deficit + remaining stops × total stop loss`

The live deficit includes whole laps. It comes from current lap numbers and
track fractions, using the focus class's measured lap curve when available.
Other classes, or a class without a measured curve, use distance fraction and
class best pace as an approximation. The displayed F2 gap is a separate input
and is not used to price a newly completed stop.

Total stop loss includes lane transit and stationary service. Once measured,
the lane model supplies transit. Service uses the car's median, or its class
median when it has no measurement. Until both transit and service are available,
the configured loss remains the provisional total estimate; service is never
added to that total again. A newly measured total can legitimately revise NET.

Stint range uses consistent exit-to-exit lap counts, so a timing-line crossing
inside the lane does not subtract one lap from every learned tank range.
An inferred stop without an observed stationary duration resets its stint but
does not inject a zero-second service sample.

NET ranks cars currently in the world; an absent car's NET is unknown and it
re-enters the projection when it returns. These ranks assume absent cars do not
resume racing. NET is withheld for a class during an ongoing pit visit or when an active contender has
no usable live progress. This avoids charging an unfinished stop twice or
presenting a missing car as zero seconds behind. It resumes when the inputs are
usable. Stops borrowed from class history are also shown in the Stops column.

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
