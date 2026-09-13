# iRacePlan Stints and handovers

The Black Box gains a **Stints** page after the selected strategy loads. It shows
current/upcoming driver, planned start/end and countdown, driver fuel target versus
measured burn, next driver change, and the next three stints. Driver colours connect the current
card, incoming-driver countdown and a time-based calendar. Planned stint blocks
are sized by duration, grouped into driver lanes and linked at driver changes.
The NOW marker follows the clock; a green border marks an acknowledged next
assignment. Hover a block for its exact times, laps and planned fuel load. Repeated
stints remain separate blocks. Times use the computer's local timezone.

## Connection

Open **Settings > Black Box > Content**, enable iRacePlan, choose **Find my team
races**, select a race and strategy by name, then save the connection. Manual IDs
remain available. Selection is explicit so an alternative strategy cannot silently
replace the chosen one.

Credentials are resolved in this order: runtime `IRACEPLAN` environment variable,
Windows Credential Manager entry `race-overlay/iraceplan`, then the bundled key.
A blank password field preserves the saved credential. The GitHub release workflow
builds with `--features bundled-iraceplan` and the repository secret `IRACEPLAN`.
The key is deliberately embedded for shared team setup and is recoverable from the
binary. Ordinary builds without that feature do not embed it. No key is stored in
source, the selection file, or team sync.

Selection lives in `%APPDATA%\race\iraceplan.toml`:

```toml
enabled = true
planning_id = 123
strategy_id = 456
```

The background connection reads the selected planning every 30 seconds with a
10-second timeout. Failed updates retain a visibly **STALE** plan; changing the
selection clears it on the next poll. Disabling the connection removes the page
and stops requests. Settings reload while the overlay runs.

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
Automated tests cover estimate fallbacks, repeated stints, camera changes, Ready
clicks and relay replay. A live race remains the final operational validation.
