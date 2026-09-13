# Endurance reliability audit — 13 September 2026

This audit covers the Suzuka reports, the upcoming 24-hour event, settings/bind persistence,
standings positions and intervals, NET, SOF, Relative rendering and team sync. Changes are implemented
in the working tree. Validation uses deterministic telemetry scenarios, real loopback WebSocket clients,
the user's existing settings file and rendered demo frames. It does not represent a completed live
24-hour iRacing run.

## Findings and implemented fixes

| Area | Finding | Result |
| --- | --- | --- |
| Own pit count | Per-car surface and stall flags can lag the direct player signals at pit entry/exit. A stall flag that cleared on release could lose the visit. | Direct `OnPitRoad` and a latched stall observation drive the player's count and shared stint tracker. An interrupted open visit remains one visit. |
| Rival stint stuck at 32 | Fully unseen stops cannot reset the observed stint without new evidence. The initial audit incorrectly assumed normal SDK results supplied a stop counter; both recorded sessions omit it. | Read optional per-car pit-road flags for observable visits. Missing `PitStops` stays unknown. A 45-second return reconciliation remains available only for feeds that explicitly supply a counter; it does not solve wholly unseen stops in the recorded feed. |
| Spectator standings | The compact selection hid the rest of the classified field. | Persisted FULL/COMPACT control for spectators and teammates, with a bounded scroll area, fixed headings and aligned columns. |
| Renderer gaps and disconnects | Absence cannot establish either pit exit or a new stint. An unseen stop and a simple renderer blink must remain distinguishable. | Preserve history across `NotInWorld` and 30-second gaps; require pit or scorer evidence for resets. Interrupted visits do not contaminate measured service history. |
| Penalty visit after a stop | A second genuine service must count independently, but a short damaged/penalty stint must not become the typical tank range. | Count qualifying visits; omit completed runs under five laps from typical-range learning. Drive-throughs retain the fuel stint when directly observed. |
| Positions differed between teammates | Distance ordering depended on the cars available to each client's renderer. | Race classification uses shared scored positions, including off-world classified entries. Relative takes its class positions from that same classification. |
| Leader vanished or falsely showed TOW | A zero/missing live position sent an officially scored leader to the back; any mid-session `NotInWorld` transition started a fake tow clock. | Fall back to the official row's position when live scoring is unavailable. Remove rival absence-based tow detection. Only a positive player `PlayerCarTowTime` supplies a confirmed TOW countdown. |
| NET after early stops | Stale scoring gaps could disagree with a newly paid stop; incomplete fields and mixed gap reference points could promote an unrendered car or a visible follower incorrectly. | Forecast all expected remaining stops through the finish. Use complete live class progress when available; otherwise use a consistent scorer source for the entire class and mark it `~NET`. Include transit plus service once. |
| SOF changed during the race | It was recalculated from changing visible/classified subsets. | Latch each class's estimate from the session roster once every listed competitor has a usable iRating. Renderer changes and subsequent driver swaps cannot move it. |
| GAP over whole laps | Lap-only gaps hid the total time deficit and the interval between adjacent classified cars. | GAP shows total time in seconds, minutes or hours with a muted lap label. Fresh scorer totals keep the class-leader reference; live fallbacks are marked `~` and require that leader. Auto alternates GAP/INT every 1 to 120 seconds (five by default). |
| Relative numbers vibrated | Interpolated row coordinates could leave numeric glyphs between physical pixels, especially at fractional scale. | Snap numeric baselines to physical pixels and finish/snap tiny row movements. Names/icons keep their usual rendering. The code path is fixed; the original race-time symptom has not been reproduced live. |
| Relative presentation | Driver name consumed a separate line; status frame included the external badges and crossed its label. | Inline driver name beside SOF, frame around the card only, label above it. Confirmed pit exits show `OUT` until the next lap crossing. |
| Settings and binds reset | Deserializing numeric customer-ID keys under `[danger]` through `toml::Value` failed and discarded the entire configuration. | Explicitly parse TOML string keys as customer IDs. The existing file now loads the saved width 580, scale 0.9, three rows ahead/behind, all seven binds and one danger mark. |
| Concurrent settings writers | A stale overlay copy could overwrite a CLI bind or another process's unrelated change. An external reload could also erase a pending bind capture/clear. | Merge only locally changed fields against a fresh file, lock writes across processes, and atomically replace using a unique synced temporary file. Preserve pending local bind edits. Retry failed writes with a delay and flush on normal exit. |
| Clipboard | The click-through input path lacked the normal clipboard shortcuts. | Explicit Copy/Paste buttons and Ctrl+A/C/V/X. Paste trims outer whitespace; changed URL/invite reconnects without restarting the app. |
| Team-driver data | Cached driving snapshots could keep publishing after telemetry stopped; same-team spectators did not use the normal synced pages, and camera focus could show another car's tank. | Fresh local snapshot gate, explicit validity for private fuel, same-team spectator support, and car-index matching before displaying replicated private values. |
| Team reconnect | Restarts could collide with prior sequence numbers; practice/race shared a room; host restart could lose state despite surviving clients. | Resume sequence numbering after catch-up, phase-specific rooms, contiguous recovery tips and replica-to-relay recovery. |
| Old pit commands | Restored history and delayed commands could operate the pit box after reconnect. | Separate recovered history from live events. Historical writes never execute; buffered disconnected writes are discarded. Live writes expire after two seconds; local queued writes also require matching session identity and a two-second lifetime. Driver consent still applies. |
| Long team backlogs | A long event history could exceed one message or resume publishing before catch-up finished. | Chunk backlogs, explicitly signal completion, limit messages to 8 MiB and normal relay history to 150,000 events per producer per phase. |
| Missing/partial fuel laps | Attaching mid-lap, missing fuel, a sample gap or skipped lap numbers could manufacture a full lap's consumption. | Re-establish a crossing baseline; only complete observed laps enter the fuel average. Invalid private scalars cannot publish a fake empty tank. |

## Team sync with no driver overlay

Two spectators continue collecting their own public telemetry. They do not originate private fuel or tyre
measurements. Explicit crew decisions and requested recovery are still outbound traffic, and the network
maintains its normal connection/roster messages. A driver publishes current scalars at up to 1 Hz only
while someone is listening; an unchanged reading gets a two-second heartbeat. Private pages become
unavailable after five seconds without fresh driver scalars, using both session and receipt clocks.

Recovered data describes the past: it cannot arm the pit box. A surviving client can restore a restarted
relay, including events originally produced by another member. All participants must use protocol 8;
older builds are refused with a version message.

Public rival pit/stint/off-track histories remain local. The protocol has legacy observation variants,
but the current producer/consumer path does not exchange those local trackers. A restarting spectator
recovers shared driver/crew state, not its entire local field model. If all clients and the relay exit,
their in-memory event history is gone. Disk persistence and public field-history recovery are separate
future work, not capabilities claimed by this patch.

## Remaining measurement limits

- A pit visit wholly outside every observation, without a corresponding scorer increment, cannot be
  reconstructed as a confirmed stop. Ordinary recorded SDK results omit `PitStops`; there is no
  normal-counter recovery to rely on. Merely blinking out never invents a stop. The separate estimator
  can supply a visibly uncertain age from fresh scoring history, driver changes and measured stint priors:
  [unseen-stop estimation](unseen-stop-estimation.md).
- Rivals do not expose a confirmed tow timer through this telemetry path, so their absence never shows
  TOW, even after a debounce delay. The player's positive direct timer supplies time remaining. Standings
  retains scored absent cars; Relative cannot show a trustworthy nearby gap without live data.
- Rival stationary detection is heuristic: roughly 0.001 laps of movement or less for two seconds,
  with at least three stationary seconds for service. A queue can resemble service; a very short splash
  can be missed. Tyre changes and rival fuel loads are inferred, not observed.
- A late scorer correction can establish an approximate stint boundary. It cannot reveal the exact
  unseen exit time or justify an `OUT` badge after the fact. Corrections beyond the return window may
  lack enough evidence for a precise boundary. See [stint tracking](stint-tracking.md).
- NET assumes the measured/borrowed stint range and stop cost continue. Safety cars, damage, fuel saving,
  splashes and changed tyre strategy can alter the outcome. `~NET` uses scoring gaps that can lag a pit
  exit; two clients with different live coverage can temporarily have different estimates. Unknown gaps
  or a car still in a pit visit can withhold the class projection. A stint-age interval that implies
  different remaining stop counts also withholds a single NET. The crossed-line fallback now uses
  per-car freshness of YAML `Time`; the live capture proved raw F2 can freeze while YAML laps advance.
  See [NET calculation](net-position.md).
- SOF is the existing local estimate, now fixed for the session. It is not downloaded from official
  results. Late attachment, incomplete roster publication or team-rating differences can make it differ
  from the official value. iRacing describes SOF in terms of the entered competitors' iRatings in its
  [official explanation](https://support.iracing.com/support/solutions/articles/31000133458-why-do-i-receive-different-amounts-of-points-for-the-same-finishing-position-in-different-races-).
- The 24-hour scenario advances synthetic session time; it is not a 24-hour wall-clock memory, network
  or graphics soak. Real SDK timing, Tailscale transport, swaps and transient device loss need rehearsal.

## Validation

The unseen-scoring check completed five minutes with 17,995 SDK ticks, 61 YAML revisions and 9,579
numeric car rows. Eleven car slots each published a new lap after being continuously unavailable for
longer than that lap plus 30 seconds. There were no continuity or parse failures. This verifies the
scoring input in that session; it does not validate inferred-stop accuracy against labelled real stops.
See [the capture report](unseen-scoring-verification.md).

The estimator tests cover delayed publication after visual return, one- and two-lap losses, staggered
field slowdowns, receiver gaps, driver swaps and conditional strategy priors. Team-driver tests cover
ratings, measured pace, incomplete scoring, swaps, same-driver refuelling and tied comparisons;
[team-driver strength](team-driver-strength.md) describes the known-roster limitation.

Automated coverage includes direct-player and rival stops, delayed scoring, renderer gaps, disconnects,
penalty visits, tow/garage ambiguity, partial histories, formation departures, session changes, OUT,
whole-lap intervals, render-independent positions, fixed SOF and full-race NET. The 24-hour scenario
checks exact counts and bounded rolling histories while exercising repeated stops and absences.

Loopback tests exercise late join, reconnect, host recovery, chunked catch-up, version/phase separation,
freshness and command non-replay. Config regressions cover numeric danger keys, persisted layout/binds,
concurrent edits and removals. The real saved file was inspected through `--check-config` without exposing
sync credentials. Rendered QA uses a temporary APPDATA, so preview runs cannot alter the user's settings.

- `cargo test -q --all-targets`: 659 overlay tests and nine shared-library tests passed.
- `cargo test -q --all-targets --no-default-features`: 657 overlay tests and nine shared-library tests passed.
- `cargo clippy --all-targets --no-default-features -- -D warnings`: passed without warnings.
- `cargo fmt --all --check`: passed.
- Optimized unlicensed candidate builds in `target/endurance-build/release/`, with the review package
  staged separately in `target/endurance-candidate/`.

Rendered examples: [OUT badge](features/img/relative-outlap.png),
[inline driver](features/img/relative-teammate.png), [card-only BOX frame](features/img/relative-box.png),
[next-car intervals](features/img/standings-next.png), [confirmed player tow](features/img/standings-tow.png), and
[full spectator standings](features/img/standings-full.png),
[estimated stints](features/img/standings-estimated.png),
[team-driver strength](features/img/standings-team-strength.png), and
[automatic GAP/INT](features/img/standings-auto.png), and
[total leader gaps](features/img/standings-long-gaps.png).

## Live rehearsal

1. Put the same candidate build on driver and crew machines. Confirm the saved layout and binds survive
   a normal close/relaunch. Copy/paste the relay URL/invite and confirm every member joins the race phase.
2. Use two clients with substantially different rendered-car limits. Compare position numbers with
   iRacing scoring, including cars absent locally. A blinking leader should retain its standings slot
   without TOW. Confirm SOF stays fixed through a driver swap.
3. Observe one normal stop and a second penalty/service visit. Record completed stops, current stint and
   OUT before entry, after exit and after the next timing-line crossing. Confirm a drive-through is not
   learned as a normal tank range.
4. Have a rival or test teammate go unobserved for about 30 seconds around entry, service and exit.
   Record the SDK scoring updates through the absence, and compare with a teammate's witnessed visit.
   Confirm whether last-lap time and completed laps continue updating. An uncertain stint is preferable
   to a fabricated service time; do not expect a `PitStops` field in the usual SDK results.
5. Record NET before and after an early stop, with full and reduced rendered fields. Check that it means
   all expected stops to the finish, and that scoring fallback is labelled. Review any change alongside
   actual time lost and a revised service estimate.
6. Turn off the driver's overlay while two spectators remain. Fuel/tyre pages should expire within five
   seconds. Restart a spectator and then the relay separately: shared history should return, with no old
   pit command applied. Swap driver and confirm private data belongs to the newly seated driver/team car.
7. Test remote pit control deliberately in practice with the driver option enabled. A live request should
   appear in iRacing's black box once; a request delayed over two seconds or sent while disconnected must
   not arrive later. Keep iRacing's own black box visible during this check.
8. Run a multi-hour rehearsal at the race's normal UI scale, monitoring numeric stability and process
   memory. Save session-info/variable diagnostics if any mismatch occurs; synthetic tests cannot identify
   every difference in a real car's SDK publication.
