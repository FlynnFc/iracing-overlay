# iRacePlan Stints and handovers

The Black Box automatically finds the current session's iRacePlan event and shows
**Stints** when it has a matching stint plan. It shows
current/upcoming driver, planned start/end and countdown, driver fuel target versus
measured burn, next driver change, and the next three stints. Driver colours connect the current
card, incoming-driver countdown and a time-based calendar. Planned stint blocks
are sized by duration, grouped into driver lanes and linked at driver changes.
The NOW marker follows the clock; a green border marks an acknowledged next
assignment. Hover a block for its exact times, laps and planned fuel load. Repeated
stints remain separate blocks. Times use the computer's local timezone.

## Connection

Detection is enabled by default and uses the key supplied with the build or your
saved credential. Join iRacing and the overlay looks for that event in iRacePlan.
There is no race or strategy selector: it uses the stint strategy named in the
website's schedule. **Settings > Black Box > Content** lets you disable detection
or save a different API key.

Credentials are resolved in this order: runtime `IRACEPLAN` environment variable,
Windows Credential Manager entry `race-overlay/iraceplan`, then the bundled key.
A blank password field preserves the saved credential. The GitHub release workflow
builds with `--features bundled-iraceplan` and the repository secret `IRACEPLAN`.
The key is deliberately embedded for shared team setup and is recoverable from the
binary. Ordinary builds without that feature do not embed it. No key is stored in
source, the settings file, or team sync.

The enable switch lives in `%APPDATA%\race\iraceplan.toml`:

```toml
enabled = true
```

Old `planning_id` and `strategy_id` settings are ignored.

Matching uses the track/configuration ID, team ID (or driver ID for an individual
entry), car name and event dates. The
event window includes 30 minutes before and after the API's session times, or the
stint schedule if session times are absent. This allows the event's practice and
qualifying sessions. Spectators can watch another car as long as the planned team
car is present. The matched plan is tied to the current iRacing room, so a cached
plan cannot appear in another room before its discovery check. An event's name
alone is insufficient to distinguish different teams, circuits or race dates.

There are no background API requests while disconnected. Each new join makes one
schedule request covering the previous seven days and the next 30 minutes, so a
24-hour race remains discoverable when joining on its second day. Unlike the
planning-history list, this endpoint is not restricted to the 100 most recently
created plans. The overlay fetches details only for entries with a stint strategy,
an applicable event time and a matching car, then verifies the track and team IDs.
No match means no further requests until another join. If multiple plans match,
the page stays hidden and settings report the duplicate plans to resolve on the
website. Only a confirmed match starts the 30-second refresh, with a 10-second
request timeout. Leaving, losing telemetry or joining an unrelated session hides
Stints, moves an open Stints page back to an available page, and stops refreshes.
A request already in flight may finish. Opening tabs and repainting use cached
data and never trigger requests.

Failed updates during a matching session retain a visibly **STALE** plan. A failed
initial discovery is not retried until another join or an explicit connection
save. Refreshes read only the detected planning and retain its scheduled strategy
ID for that room. Saving the connection reruns discovery when a session is present.
Disabling the connection removes the page and stops requests.
Settings reload while the overlay runs.

## Handover and Ready

An amber banner appears above any Black Box page near the next different driver's
handover. The incoming spectator sees **DRIVING IN ~3 LAPS** (or fewer) and a
**Ready** button. Other team members see the next driver and their acknowledgement.
Ready can be revoked and is tied to the exact assignment, driver, and planned start,
so a revised assignment does not inherit an old acknowledgement. It is an explicit
acknowledgement, not proof that someone is online. A live team-sync connection is
required to submit it. Everyone using sync, including the relay, must update to
protocol version **9** together.

The estimate combines planned laps, observed pit-exit stint age, recent pace, and
fresh matching team-sync fuel where available, including the configured fuel
reserve. Consecutive stints by the same driver defer the driver-change warning.
Spectators can watch another car while the alert follows the planned team car.
A spectator's local fuel tank is never used. Without matching live fuel, the banner
explicitly says **Plan estimate**. Fuel shortage signals a possible earlier stop;
it does not confirm the crew's decision to change drivers. In the pits the banner
asks the incoming driver to prepare for the next change.

Live comparison requires a matching race, track/configuration, team and car.
If the observed driver differs, the page shows the discrepancy and uses that driver's
own target if configured. Missing or zero wet targets remain unavailable. Observed
stint age can keep a delayed double stint attached to the correct assignment. The
page also shows an estimated handover time and shift when live fuel supports it.
Large or ambiguous deviations fall back to the labelled schedule. Stale telemetry
is not presented as live; stale plans disable new Ready acknowledgements.

The documented API has no strategy-edit endpoint or live stint event stream.
Revised times are projections in the overlay; edit the plan in iRacePlan and the
next successful fetch will pick it up. The integration sends no pit-service commands
and performs no strategy writeback. Availability surveys are outside this feature.

## Validation

```powershell
cargo test
cargo test --bin race-overlay --no-default-features iraceplan:: -- --ignored
race-overlay.exe --demo --demo-page=stints --demo-state=handover --screenshot=handover.png
```

The demo uses synthetic drivers and a labelled demo plan, without API requests.
Automated tests cover automatic event and strategy discovery, session matching and refresh transitions, tab withdrawal,
estimate fallbacks, repeated stints, camera changes, Ready clicks and relay replay.
A live race remains the final operational validation.
