# Standings stint tracking

The standings derives each car's current stint and recent range from per-car
lap, track-surface and lap-position telemetry. iRacing does not publish rival
fuel loads or tyre service, so a detected stop is evidence of a new stint, not
proof of what service the crew performed.

## Boundaries and missing telemetry

- A stint starts at a witnessed pit-lane exit. At session start, lap zero is
  also a trustworthy start. Leaving the initial garage after one or more
  `NotInWorld` samples establishes the first boundary without counting a stop.
- If the overlay first sees a car already racing after lap zero, that first
  segment is partial. Its next stop is counted, but the partial segment is not
  added to typical-range history.
- `NotInWorld` is absence, not an off-pit-road observation. It neither closes
  a lane visit nor starts a stint by itself.
- A lane visit entered before a `NotInWorld` gap stays one visit. Service that
  was directly observed still resets the stint once at the eventual exit, but
  the interrupted visit contributes neither a service time nor a range sample.
- Track to `NotInWorld` to pit stall is treated as a tow or garage transition.
  Its later garage departure does not count as a stop or reset the racing
  stint, and the span is invalidated for later range learning.
- A continuously observed lane visit with more than one second of missing
  lap-position data retains the conservative stop inference. Its known
  exit-to-exit lap boundaries can supply range, but the blind interval cannot
  supply a service duration.
- A transient missing `SessionNum` keeps the current session state. A different
  concrete session number clears all stint and stop history.
- A rejected tow/garage boundary deliberately preserves the lap-based stint,
  so its elapsed-seconds value includes the absent garage time if the car later
  rejoins. The stint bar and stop forecast use laps rather than that duration.
- Attaching during lap zero gives a trustworthy lap-count origin, but the
  first stint's elapsed-seconds history can be short by the part of that lap
  completed before attachment.
- The first genuine results row establishes the official completed-stop
  baseline. Live fallback rows do not. Stops detected afterwards are added to
  that baseline immediately; a delayed scorer update acknowledges the same
  stop without resetting its stint twice.
- A scorer increase can confirm an interrupted or otherwise unobservable
  visit. A witnessed exit then becomes the new stint boundary; a wholly
  offline increase starts an approximate boundary when the car returns. Neither
  case contributes range or service history.
- A continuously observed drive-through remains part of the same fuel stint
  even when the official results count uses a different convention and rises.

## Heuristics and limits

- A rival is considered stationary after its lap position moves by no more
  than `0.001` laps for two seconds. A long queue can therefore resemble
  service on noisy or sparse telemetry.
- At least three stationary seconds are required for an observed stop. A very
  short splash can be missed.
- Completed runs under five laps count as stops but are omitted from typical
  range. This filters damage and penalty visits; it does not prove fuel was
  added on longer runs.
- Position-blind lane visits favor a stop inference over leaving a rival's
  stint running forever. A blind drive-through can therefore still reset the
  inferred stint.
- The player's separate stop counter uses the direct pit-stall and speed
  signals and holds an open visit across `NotInWorld`; it does not use the
  rival movement inference.
