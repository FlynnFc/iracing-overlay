# Team sync — one race's data shared across a team's overlays

> **Status**:
> - Phase 1 (protocol, relay, ledger, client, recovery) — built and tested,
>   `src/bin/race-overlay/sync/`, exercised by `--sync-host` / `--sync-join`.
> - Phase 2 **data pipeline** — built and tested: the producer (`feed.rs`,
>   driver telemetry → events), the consumer store (`store.rs`, ledger →
>   `TeamState`), the app runtime (`runtime.rs`, connect/produce/consume each
>   frame), plus the `[sync]` config and the snapshot identity/session-time
>   plumbing. Inert unless `sync.enabled`.
> - Phase 2 **spectator Fuel page** — built and tested: `TeamState::synced_car`
>   feeds a `SyncedCar` into the Fuel page, `pages_for`/`available_from` unlock
>   Fuel in `Seat::Spectating` **only when synced data is present**, and the
>   page renders read-only from the ledger with a "via <driver>" tag and no
>   tank arc (capacity unknown off-seat).
> - Phase 2 **spectator Tyres page + tyre writes** — built and tested (v3
>   protocol): a `TyreReadings` event carries latched wear/temp/pressure on
>   change, `DriverScalars` gained the armed cold pressures, the store folds
>   them into `SyncedCar`, the Tyres page unlocks while spectating **only once
>   a stop has put life on the wire**, and tyre arm/clear/pressure controls
>   route over the wire from the spec seat against the synced-adjusted
>   snapshot. Covered by protocol round-trips, store folds, a producer→consumer
>   reconstruction test, and the page-gating tests.
> - Phase 2 **spectator Strategy page** — built and tested: the pit-window
>   projections re-source from the synced-adjusted snapshot (the driver's real
>   burn and tank feed the stop plan), the page unlocks while spectating once
>   fuel is synced, and the BOX BOX border's fuel call is recomputed from the
>   driver's tank so a crew chief sees it fire. **Phase 2 is now complete.**
> - Phase 4 **crew-chief pit writes** — built and tested: `PitRequest` is
>   serializable, a v2 `Event::PitWrite` carries it, a spectator's black-box
>   fuel controls route over the wire (`dispatch_pit_request`) against a
>   synced-adjusted snapshot, and the seated driver's overlay applies them
>   behind the `allow_team_pit_control` consent gate with a passive "set by"
>   note. The safety property — a write applies once, live, and a replayed one
>   never re-arms — is covered by an end-to-end loopback test plus pure gate
>   tests (`pit_request_routes_over_wire`, `team_write_applies`). **Phase 4 is
>   now complete**: tyre writes route from the spec seat, and the shared
>   `FuelTarget` (v4) and standing `TyrePolicy` (v5) both shipped — see
>   `plans/strategy-spec-mode.md` for their design.
> - **Settings UI** — built: a Team Sync page in the settings window edits
>   `[sync]` (enabled, relay URL, invite code, and the pit-control consent),
>   with the note that URL/invite changes take hold on a sync toggle or the
>   next session.
> - **Hosting from the settings page** — built: `sync.host` + `sync.host_port`
>   (default 41230) run the relay *inside* the overlay, so the hosting member
>   never opens a terminal. `OverlayApp::ensure_relay` spawns it once per run
>   (`Relay` has no shutdown, so a port change wants a restart — the page says
>   so), generating and persisting an invite when none is saved, and
>   `effective_sync_config` points the host's own client at
>   `ws://127.0.0.1:<port>` so the URL field is purely the joiners' side.
>   Failures (port taken, invite unparseable) are shown on the page and not
>   retried until the port or invite changes. `InviteCode::parse` is the new
>   half of the round trip: the one saved string serves the host's relay and
>   their own client alike.
> - **Hardening pass** (edge cases, tested): a lag-spike lap jump divides its
>   fuel delta across the gap instead of reporting it as one lap's burn; a
>   backwards lap (session restart) resets the baseline without an event; a
>   subsession change resets the producer/store/pending-writes wholesale; and
>   both socket ends cap messages at 4 MB (`protocol::socket_config`) so a
>   hostile frame can't make either side buffer tungstenite's 64 MB default.
> - Phase 3 (DC-recovery replay into the driver's *trackers* — consumption
>   history, rival stops) — not started; the spectator store already recovers
>   from backlog, but the telemetry-thread trackers are `!Send` and rebuilding
>   them across that boundary is its own piece of work.

In a team session, each member's sim publishes the same world (positions,
laps, gaps) but only the seated driver's sim publishes the car: fuel level,
consumption, tyres, in-car adjustments. Everybody else's overlay is blind to
exactly the things a crew chief wants to watch — and an overlay that
disconnects loses everything it had *measured* (consumption history, rivals'
stop times, off-track tallies, race-start baselines), even though the sim
hands the *world* back on rejoin.

Both problems are the same problem: measurements live in one process. The fix
is a shared ledger.

## The shape: an event ledger, not state streaming

Everything worth syncing is either a small **event** or derivable from one:

- *Lap closed*: lap number, session time, fuel remaining, consumption that
  lap. ~32 bytes, once every ~2 minutes.
- *Pit stop observed* (any car): car index, stationary seconds, tyre
  inference. ~16 bytes, rare.
- *Off-track counted*: car index, new tally. ~8 bytes.
- *Stint boundary*: who got in the car, when.
- *Driver scalars tick*: fuel level, tyre latches, and the sim's armed
  pit-service state (fuel to add, which corners are ticked, pressures,
  compound) — the only recurring item, sent at 1 Hz **only while someone
  else is listening**, quantized so an unchanged value sends nothing. The
  pit-service state is in the tick, not an event, because it is also the
  *echo* that closes the crew-chief write loop below.

Every event carries the producer's member id, a per-producer sequence number,
and the iRacing **SessionTime** it happened at. SessionTime is the anchor that
makes this accurate with minimal traffic: every member's sim shares the same
session clock, so events merge deterministically whatever the network delays
or arrival order — accuracy comes from the timestamp, not from sending fast.
Between fuel events, listeners interpolate with the same burn model the Fuel
page already uses; the next lap-closed event trues it up to the measured
litre. Sharp where it matters, cheap in between.

**Cost**: a driver produces well under 200 B/s; a five-member team relayed to
everyone is ~1–2 KB/s total. Rounding error next to the sim's own traffic.

## Recovery is the ledger read back

The relay keeps the session's ledger (a few hundred KB for a 24-hour race at
the rates above). A member who joins late or reconnects says "I have producer
P up to sequence N" and receives only what it missed, then replays it through
the same handlers that consume live events — one code path, so a rebuilt
overlay is bit-identical to one that never dropped. The existing trackers
(`SessionTrackers`, pit observations, off-track tallies) already separate
measurement from display, which is what makes replay possible without
restructuring them.

Sessions are identified by iRacing's **SubSessionID** (already in the session
YAML), so members find each other with zero configuration; a team passphrase
in the config keeps strangers in the same subsession out of the room.

## Topology

A tiny relay, not peer-to-peer: NAT traversal is a project of its own, and a
relay is also what holds the ledger for rejoiners — P2P would need someone
authoritative anyway.

The relay lives **inside the overlay**, behind a "Host team sync" tick on the
settings General page — no separate binary, no VPS. The hosting machine runs
[Tailscale](https://tailscale.com) and exposes the relay with **Funnel**
(`tailscale funnel <port>`), which publishes it at a stable public
`https://<machine>.<tailnet>.ts.net` URL. Teammates install nothing: they
paste that URL and the invite code into their overlay and connect like to any
website.

The security properties this buys, and the ones we add:

- Funnel's TLS certificate lives on the hosting machine — `tailscaled`
  terminates TLS *on the host*, not at Tailscale's edge, so the tunnel is
  encrypted end-to-end and Tailscale's relays only ever see ciphertext.
- The relay binds **localhost only**. The single route to it from anywhere —
  LAN included — is through the funnel; there is no port forward and no
  listener on a real interface to probe.
- Ticking "Host team sync" generates a random **invite code**, shown beside
  the URL for the host to hand to the team. Joins present it and are checked
  in constant time; failures are rate-limited. The code rotates whenever the
  host unticks and re-ticks.
- A room is bound to one SubSessionID, so even a leaked code only ever
  reaches one race's fuel numbers, and only while it runs.

The overlay connects out via WebSocket (TCP is fine at these rates; latency
of a second is invisible when events carry their own SessionTime), wire
format `postcard` with a version byte first. Disconnects retry with backoff
forever; the ledger makes missed time a non-event. If the team ever wants an
always-on box instead, the same relay module compiles into a standalone
`race-relay` binary — nothing about the protocol assumes it lives in the
overlay.

## What it unlocks, in order

1. **Crew chief reads** — a teammate on `Seat::TeamMate`/`Spectating` sees
   the live Fuel and Tires pages for their own car, fed from the ledger
   instead of the (empty) local sim scalars. The pages already withdraw
   themselves off-seat via `pages_for`; this hands them back with synced data
   and a "via <driver>" tag so nobody mistakes relayed numbers for local ones.
2. **DC recovery** — trackers rebuilt from replay on rejoin.
3. **Crew chief writes** — the whole pit box, not just fuel: the existing
   `PitRequest` enum (fuel litres, per-corner tyres, cold pressures, tearoff,
   fast repair) travels the wire as-is to the seated driver's overlay, which
   applies it through the existing `pit_requests` channel. Three rules keep a
   remote adjustment *correct* rather than merely delivered:

   - **Intents cross the wire, never commands.** `commands_for` needs the
     sim's current service state to rebuild around iRacing's all-or-nothing
     `ClearTires` (see `telemetry/pit.rs:189`), so that translation must
     happen on the driver's machine against fresh state — a spec client
     pre-computing commands from its own seconds-old view would re-arm the
     wrong corners.
   - **The sim's echo is the only truth.** A spec UI shows its own request as
     *pending* until the armed state comes back changed in the driver scalars
     tick — what iRacing actually accepted, read off `PitSvFuel` and friends.
     Everyone (driver and every spec) converges on the sim's answer, so two
     crew members and the driver can all touch the box and still see one
     consistent state; last write wins, which is also the sim's own rule.
   - **Consent is armed once, not begged per change.** The driver ticks
     "allow team pit control" for the session; each applied remote change
     then shows a passive note ("Fuel 42 L — set by <name>") rather than a
     confirmation dialog. A driver mid-corner cannot be answering prompts,
     and a crew chief whose every pressure tweak stalls on one is not
     controlling anything. Unticking kills remote writes instantly.

   The sim only accepts pit commands from the seated client, so this remains
   a relay of intent, not remote control.

   **Auto Fuel with the team connected** is the case that makes correctness
   non-negotiable: a spectator and the driver may both have it ticked, and a
   spectator who joined mid-stint measured only part of the consumption
   history. Three rules make the number one number:

   - *One actuator.* Only the seated overlay's Auto Fuel computes and arms.
     While synced and off-seat, the tick goes inert and the Fuel page says
     so ("Auto Fuel: driver's overlay") — a second auto-fueler sending
     remote requests would fight the first at every recalculation.
   - *One dataset.* While synced, the fuel model's per-lap consumption
     comes from the ledger's lap-closed events rather than from locally
     accumulated measurements. After catch-up the ledger is identical bytes
     on every machine, so every member derives the same figure whenever it
     is computed — joining half-way through a stint changes nothing,
     because the laps you weren't there for are in the ledger.
   - *One display.* Every screen shows the sim's echoed armed fuel, not its
     own calculation. Even if two versions of the app ever disagreed, what
     is shown is what the car will actually get.

Beyond these phases, the synced data drives the spec-mode Strategy page in
`plans/strategy-spec-mode.md`: it reads the ledger for the crew chief's
fuel/tyre/schedule view, and adds one new wire event — a shared
`FuelTarget` the crew chief sets and the driver's Relative header chases —
which is the only strategy feature that touches this protocol (a version
bump); the what-if sketches and read-outs write nothing.

## Explicitly out of scope

- Voice, chat, or anything Discord already does.
- Syncing panel layouts/settings between members.
- Cross-team or public data sharing.
- A hosted service run for other people. The relay is a binary teams run
  themselves.

## Decisions taken

- **Hosting**: embedded relay + Tailscale Funnel, as above. Only the host
  installs Tailscale; teammates get a URL and an invite code. (Vercel was
  considered and dropped: serverless functions can't hold a WebSocket, and
  the polling redesign adds a KV store, seconds of lag, and free-tier
  arithmetic for 24-hour races.)
- **Crew chief writes** (phase 3): the full `PitRequest` surface, gated by a
  per-session "allow team pit control" tick on the driver's side with a
  passive note per applied change — not a per-change confirmation dialog,
  which would be unusable mid-stint. The team already trusts each other;
  per-member permissions can be added later without protocol changes if that
  ever stops being true.
