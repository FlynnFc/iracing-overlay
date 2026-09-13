# Live scoring verification before unseen-stint estimation

13 September 2026. This report records the live verification performed before
implementing unseen-stint estimation, along with the public source comparisons
and the limits of the captured evidence.

## Result so far

`ResultsPositions` is used as a live race-classification source by other overlays.
The second local live capture below confirms that its `LapsComplete` can advance
while the corresponding car is continuously `NotInWorld` on this client. It does
not establish a universal or per-lap freshness guarantee.

The September 8/9 local session-info dumps are individual snapshots, so they
cannot answer a question about updates over time.

## Local live captures, 13 September 2026

The preliminary `live-20260913-01.csv` capture ran for 75.1 seconds before its
SDK session expired. It had 4,505 new SDK ticks, 933 numeric classified-car rows
and 10 YAML revisions. It found two absent-car `LapsComplete` increments, but
neither had been absent for more than its reported `LastTime + 30` seconds. That
result alone was inconclusive.

The independent `live-20260913-02.csv` capture completed its 300-second bound.
It contains 17,995 new SDK ticks, 9,579 numeric classified-car rows and 61 YAML
revisions. All required SDK variables were present; there were no YAML parse
failures, phase or clock regressions, missing surfaces, or local gaps over two
seconds. It recorded 39 YAML `ResultsPositions.LapsComplete` advances while the
matching car had remained continuously `NotInWorld`; 11 advances, across 11
different car slots, passed the stricter `absence > YAML LastTime + 30 seconds`
test. At every strict event, the car's `TrackSurface` was `-1`, direct pit-road
flag was `0`, and raw `Lap`, `LapCompleted`, and `LastLapTime` were all `-1`.

Examples from the second capture:

| CarIdx | Elapsed | YAML revision | `LapsComplete` | YAML `LastTime` | Continuous absence | Excess over `LastTime + 30` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 5 | 295.441 s | 73 | 4 -> 5 | 138.692 s | 295.400 s | 126.708 s |
| 29 | 283.439 s | 71 | 3 -> 4 | 141.252 s | 283.400 s | 112.148 s |
| 2 | 232.417 s | 64 | 6 -> 7 | 137.535 s | 232.383 s | 64.849 s |

This confirms, within the sampled live session, that the YAML classification
can publish a completed-lap advance while the corresponding car remains locally
unavailable for longer than a lap plus the publication margin. It does not give
a universal freshness guarantee: raw per-car lap data remained unavailable, and
a future session may publish differently. The estimator may use these updates as
fresh scoring evidence only with explicit uncertainty; it must not manufacture
the unseen lap's timing or treat stale F2 values as corroboration.

## irDashies source and recordings

Reviewed commit `5f620ae256c0653a890097d69669131ca089e185` of
[tariknz/irdashies](https://github.com/tariknz/irdashies/tree/5f620ae256c0653a890097d69669131ca089e185).

- Its [pit tracker](https://github.com/tariknz/irdashies/blob/5f620ae256c0653a890097d69669131ca089e185/src/frontend/context/PitLapStore/PitLapStore.tsx#L95-L147)
  skips unavailable cars before detecting pit-road entry or exit. Its explanation
  describes `CarIdxOnPitRoad` dropping to false during a driver swap, even while
  the car is still in its stall. It preserves the visit through that interval.
- Its [stint display](https://github.com/tariknz/irdashies/blob/5f620ae256c0653a890097d69669131ca089e185/src/frontend/components/Standings/components/DriverInfoRow/cells/lapCountUtils.ts#L29-L72)
  distinguishes a witnessed stop, observation from session start, and joining
  mid-session without a witnessed stop. The latter is unknown; it does not
  reconstruct an unseen stop from a long lap in this path.
- Its [driver-swap replay test](https://github.com/tariknz/irdashies/blob/5f620ae256c0653a890097d69669131ca089e185/src/frontend/context/PitLapStore/pitTiming.replay.spec.ts#L21-L72)
  documents and exercises a captured swap, supporting the need to hold the pit
  state across unavailable samples.

The [Road America fixture](https://github.com/tariknz/irdashies/blob/5f620ae256c0653a890097d69669131ca089e185/test-data/fixtures/multiclass-road-america.json)
contains 60 anonymised frames at roughly 1 Hz, spanning session times 21860.033
through 21919.2. The source describes it as a real capture. Direct inspection
found 21 array slots with `CarIdxTrackSurface == -1` throughout those frames.
For example, car index 1 has these unchanged values:

| Field | All 60 samples |
| --- | --- |
| Track surface | -1 |
| Pit-road flag | false |
| Live lap / completed lap | -1 / -1 |
| Live last-lap time | -1 |
| F2 gap | 192.532 seconds |
| Position | 47 |

This proves that nonzero raw gaps and positions can remain stale alongside
unavailable lap data in that fixture. It does not prove whether each absent slot
represents a car still racing beyond the client's coverage or a retired car.
Nor does it establish how the YAML changed: the fixture includes a static
`sessions[].ResultsPositions` snapshot, not timestamped session-YAML revisions.
It therefore cannot confirm or disprove the proposed independent scoring stream.

irDashies also has a [recorder that saves raw frames and every YAML revision](https://github.com/tariknz/irdashies/blob/5f620ae256c0653a890097d69669131ca089e185/docs/TELEMETRY_RECORD_REPLAY.md#L40-L66).
That is the right kind of recording for this question. An AI recording does not
by itself establish behavior for network-unavailable opponents in multiplayer.

## Another overlay's approach

Reviewed iracing-pitwall commit `69f7badd1168ceb9f3cc0d5318043e37b1400ede`.
Its [release notes](https://github.com/Swizzjack/iracing-pitwall/releases/tag/v0.2.23)
describe using `ResultsPositions` for live standings, updated at lap crossings.
Its [gap tracker](https://github.com/Swizzjack/iracing-pitwall/blob/69f7badd1168ceb9f3cc0d5318043e37b1400ede/bridge/src/telemetry/gap_tracker.rs#L1-L12)
holds intervals until the individual car's completed-lap count advances.
Its [pit tracker](https://github.com/Swizzjack/iracing-pitwall/blob/69f7badd1168ceb9f3cc0d5318043e37b1400ede/bridge/src/telemetry/pit_tracker.rs#L33-L81)
uses live pit-road transitions rather than an unseen-lap estimator.

These are implementation evidence and maintainer statements, not a substitute
for observing our particular missing-car condition in a live capture.

## Capture protocol for future verification

Use the read-only diagnostic in a populated multiplayer session:

```powershell
.\race-overlay.exe --capture-scoring=scoring-check.csv --capture-seconds=300
```

It creates a new CSV and summary beside the specified path. It reads live SDK
frames and the session-info revision, with numeric per-car fields only. It sends
no pit commands and changes no simulator settings. Existing output files are
not overwritten; use a new filename for another capture.

For a useful sample, at least one car must be continuously unavailable locally
while it continues racing and completing laps. A populated spectator session
with restricted transmitted-car coverage can provide that condition. Reducing
Draw Cars alone is not a reliable way to create it: transmitted cars and drawn
cars are separate settings.
[iRacing's explanation](https://support.iracing.com/support/solutions/articles/31000149355-connection-type-max-cars).

Check whether the same car's YAML completed-lap count advances and its last-lap
time remains valid during that interval. A scoring increment immediately after
disappearance may be a delayed publication of an earlier lap. Sustained absence
longer than the reported lap time plus a publication margin is stronger evidence.
Keep the raw fields and durations available for review rather than calling every
off-world increment proof. Data gaps, session changes and time regressions break
the continuity check. An empty evidence count is inconclusive if the capture did
not contain a suitable active, unavailable car.

## Consequence for implementation

Build the per-car lap ledger from fresh absent-car classification advances and
feed the one/two-lap anomaly model described in [the estimator
design](unseen-stop-estimation.md). Preserve the confidence and uncertainty that
come from the capture rule: the update indicates a scored lap, not its hidden
lap time or stop cause. The estimator must fall back to before/after observations,
driver-change evidence, crew history and explicitly labelled fuel-strategy priors
when that rule is not met. It may not count stale F2 values as independent
evidence or manufacture skipped lap times from a single latest `LastTime`.
