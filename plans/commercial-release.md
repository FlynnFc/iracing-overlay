# Commercial Windows release and Polar activation

Status: implementation plan, 2026-09-09. No paid license gate is implemented yet.

### First-run UI implemented (temporary acceptance)

The first-run license page now follows the graphite Settings design. Per the user's
follow-up, any non-empty key is accepted while Polar is being configured. Completion
is stored in `%APPDATA%\race\license-setup.toml`; the supplied key is not stored.
This is onboarding state only, never evidence of payment. Real Polar integration
must ignore this marker and require activation again.

Normal startup waits for the page before starting companion apps, telemetry or the
overlay tray. Close/Escape cancels startup. Existing developer demo/diagnostic paths
remain available. `--license-preview` opens an isolated preview regardless of saved
completion; it never saves or starts the overlay. Combine it with
`--screenshot=docs/design/license-first-start.png` to capture the design.

Implementation: `licence.rs` owns temporary acceptance and startup;
`ui/licence.rs` owns the form. To repeat first-run setup, remove only the
`license-setup.toml` marker. The broader production plan below remains outstanding.

## Requirement and first-release decisions

The source repository is private. Customers buy through Polar, download the Windows
app, and enter their license key before the overlay is usable. A download or a local
configuration flag does not establish entitlement.

Proposed first release: one self-contained `race-overlay.exe`, delivered in a ZIP
with install instructions and third-party notices through Polar. An installer can
follow when shortcuts, uninstall, and upgrades justify it; an installer is not
required to sell a Windows executable.

Start locked, validate online on every application start, and unlock only after
Polar confirms the license for this overlay's specific benefit. No unpaid demo or
trial in the commercial build. These defaults follow the requested paid-only gate.
The existing developer screenshot/demo tools belong in a separate development
feature that commercial CI explicitly excludes.

## What the repository actually does today

| Area | Current behavior | Required change |
|---|---|---|
| `Cargo.toml` | Builds native Rust Windows binaries; release stripping and LTO already enabled | Explicit commercial build configuration, public Polar IDs, version consistency |
| `build.rs` | Embeds icon/version resources; permits resource compilation failure | Embed runtime assets; fail commercial packaging if resources are missing |
| `ui/logos.rs`, `ui/flags.rs`, `ui/icons.rs` | Load SVG assets from disk | Embed SVGs and logo manifest; retain development overrides if useful |
| `main.rs` | Starts configured companion apps and telemetry before the UI | Open activation UI first; defer all paid runtime startup |
| `app.rs` | Owns controls, sync, painting, and pit dispatch | Central entitlement state and explicit paid-runtime lifecycle |
| `ui/settings.rs` / `tray.rs` | Settings and panel controls are available immediately | Activation/retry/portal/quit while locked; License page once active |
| `race-launcher.rs` | Separate executable launches the session stack | Fold into `--launch`; avoid recursive self-launch and duplicate startup |
| `.github/workflows/release.yml` | Tests, builds two EXEs, zips loose assets, publishes GitHub release | Produce verified commercial artifact and deliver via Polar |

The current release workflow has **no code-signing step**. Signing is outstanding,
despite the earlier draft describing it as handled. Private GitHub release assets
are useful for internal retention but are not a customer download solution.

## Customer flow

1. Buy the overlay product in Polar. Attach a License Keys benefit and a File
   Downloads benefit to that product; do not grant the same license benefit to an
   unpaid product or trial if access must require payment.
2. Download and extract the release ZIP, then open `race-overlay.exe`.
3. First run opens a normal, focusable activation window: **Enter your license
   key**, masked input with reveal/paste, **Activate**, **Buy a license**, and
   **Find my license**. Closing it must not start the overlay in the background.
4. Validate the key, activate this installation if device limits are enabled,
   validate that activation, then save credentials and start the runtime once.
5. Later launches show **Checking license...** briefly. A saved credential avoids
   retyping; it never authorizes access without the startup validation.
6. Settings > License shows masked key, status, and **Deactivate this device**.
   Device-limit errors link to the customer portal to free an activation.

Network errors leave startup locked with a retry action. No modal, sound, or
disappearing panel should interrupt a race that already passed validation.

## Polar contract

Use HTTPS calls from the desktop to Polar's public customer-portal license
endpoints. No organization access token, checkout secret, or webhook secret is
embedded in the EXE. Public organization ID, overlay benefit ID, checkout URL,
and portal URL are build configuration. Production and sandbox builds are separate;
the commercial artifact must not accept a runtime API-host override.

| Operation | Endpoint under `https://api.polar.sh/v1` | Request / handling |
|---|---|---|
| Preflight and startup validation | `/customer-portal/license-keys/validate` | Send `key`, `organization_id`, `benefit_id`; include saved `activation_id` when configured |
| Reserve device | `/customer-portal/license-keys/activate` | Send `key`, `organization_id`, friendly `label`; persist returned activation ID |
| Release device | `/customer-portal/license-keys/deactivate` | Send `key`, `organization_id`, `activation_id`; confirm server outcome before reporting success |

Validation returns a license object, not a `valid: true` boolean. Require a parsed
success response with `status == granted`, matching organization and benefit, an
unexpired `expires_at` when present, and the expected activation when required.
Unknown states, malformed responses, or mismatched IDs never unlock. Omit
`increment_usage`: opening the app should not consume a usage allowance.

Preflight checks entitlement before consuming a device slot. Prove the behavior
of preflight without an activation in sandbox with activation limits enabled;
adapt to the documented activation-required outcome without treating it as a
grant. Final validation with benefit and activation is always required to unlock.
If a new activation fails final entitlement checks, attempt to release that slot.

Use one background request at a time, bounded connection/response timeouts,
bounded response size, normal certificate verification, and safe typed error
messages. Do not display raw response bodies: validation errors can echo the key.
Redact keys and customer details from logs. Handle 429 with Retry-After/backoff;
do not treat a rate limit as revocation. Treat 404/422 according to the verified
error contract, with no accidental unlock on any error.

Persist a returned activation ID immediately as a pending credential before final
validation. A timeout after activation may have consumed a slot: do not blindly
repeat activation. Offer recovery through the portal if the response was lost.
Prevent simultaneous activation in two app instances with a per-user single-instance
guard. Deactivation failure keeps credentials for retry; a separate local forget
action must explain that it does not free the remote slot.

## License lifecycle and offline policy

| State | Available behavior | Transition |
|---|---|---|
| Unactivated | Key entry, purchase/portal links, notices, quit | Activate starts background check |
| Checking | Activation window, progress, quit; no paid workers | Valid response unlocks; error returns to locked UI |
| Active | Full overlay and controls | Recheck between sim sessions and periodically while idle |
| Recheck pending during session | Existing session continues | Revalidate before entering the next session |
| Locked after rejection/outage | Retry or replace license; no paid workers | Successful validation unlocks |

Proposed policy: online validation before each process start and each new iRacing
session; recheck after 15 minutes while idle. Pin an already validated running
session until it ends, so a network outage or license change cannot remove tools
mid-stint. A recheck failure locks at the session boundary. Use the existing
telemetry session identity/lifecycle, not just process existence or whether the
car is moving; a pit stop is not a session end. Disconnect/reconnect to the same
session within the running app must not cause a mid-race lockout. Distinguish that
case from an actual new session in implementation tests.

This explicitly permits a previously authorized session to finish after remote
revocation. Immediate revocation would require accepting mid-race interruption.
No perpetual offline fallback and no editable `last_validated` timestamp grants
access. If offline startup is wanted later, define a bounded grace policy and use
server-signed entitlements; that adds a backend and is a separate scope decision.

## Local storage and enforcement

Store a versioned credential blob under `%LOCALAPPDATA%\race\license.dat`, protected
with Windows DPAPI in current-user scope. Include the key, activation ID, environment,
and a random installation ID; do not persist unnecessary customer email/address data.
DPAPI reduces casual extraction/copying; it is not proof of purchase or DRM against
the machine owner. Online Polar validation remains the authority.

Write atomically; preserve the old credential until replacement succeeds. A missing,
corrupt, or undecryptable file means locked with a recovery explanation. Do not
fall back to writing secrets beside the EXE, and do not claim a lost key is recoverable
from a corrupt file. An update must preserve credentials and settings. Reinstallation
or Windows-user changes may require key entry and portal activation recovery.

Proposed modules beneath `src/bin/race-overlay/licence/`:

- `client.rs`: HTTPS transport, DTOs, response checks, sanitized errors.
- `store.rs`: DPAPI, atomic persistence, schema/environment handling.
- `mod.rs`: state machine, worker results, session authorization and retry policy.
- `ui/licence.rs`: activation window and License settings page.

Keep transport replaceable for tests. Compare a small synchronous HTTP client on
a background worker with WinHTTP before implementation; either needs JSON parsing.
WinHTTP avoids an additional TLS stack but requires more unsafe handle management.

Gate **startup and execution**, not just painting. `main.rs` must not start telemetry,
launcher actions, sync hosting/joining, or input-driven pit commands while locked.
`OverlayApp::gui_run` must return through the activation UI before normal paid work,
and `dispatch_pit_request` must require active authorization. Lock transitions stop
workers, drop queued commands and clear stale telemetry/sync state. Unlock starts
workers once. Check all tray and settings actions for alternative startup paths.

Audit `--demo`, `--screenshot`, `--sync-host`, `--sync-join`, dump commands, bind
commands, and the old launcher target. Remove developer-only entry points from the
commercial feature set; gate any paid entry points retained in the commercial app.
Development bypasses must be compile-time only and absent from the shipped build.

## Packaging and delivery

1. Embed assets as described in the earlier packaging investigation. Include all
   third-party notices, including fonts and transitive dependency notices, both in
   the app and release ZIP. Check asset loading from outside the repository.
2. Fold launcher behavior into the licensed app. `--launch` records requested
   startup intent, opens activation if needed, and resumes once authorized. Skip
   the overlay's own launcher row so it cannot spawn itself recursively.
3. Build Windows x64 with `cargo build --locked --release --bin race-overlay`
   plus the explicit commercial feature set. Fail release builds with missing IDs,
   sandbox configuration, or enabled development bypasses. Align tag/package/EXE
   versions; do not distribute credentials or local machine configuration.
4. Inspect DLL dependencies on a clean Windows VM. Evaluate static CRT for Rust,
   mimalloc and GLFW together; if runtime DLLs remain, provide their supported
   installation path and verify it. Do not assume a single EXE is self-contained.
5. Add Authenticode signing and timestamping with release-only credentials. Verify
   the signature, then generate SHA-256 checksums and ZIP the final signed binary.
6. Stage the ZIP as a private CI artifact. Initially upload the verified artifact
   to the Polar file benefit manually; automate delivery after that flow works.
   Preserve previous releases for rollback. Download and verify the customer copy.
7. Ship customer install/activate/update instructions. Keep private developer build
   documentation. Review current MIT package/release metadata before commercial
   distribution; do not rewrite historical licensing as part of packaging.

No automatic updater in v1. A customer downloads the newer build, closes the app,
replaces the EXE, and keeps settings/license data. Later an installer can place
the app under the user's local Programs directory and create Start Menu shortcuts.

## Implementation slices and acceptance

1. **Gate first:** license state machine, fake transport, activation UI, deferred
   workers. Prove a fresh/corrupt install and every alternate entry point stay
   locked. Valid activation starts once; invalid activation starts nothing.
2. **Real Polar integration:** sandbox client, benefit/activation checks, DPAPI,
   retry/deactivate/recovery. Test wrong organization/benefit, expired/revoked/
   disabled keys, max devices, malformed 200, 404/422, 429, timeout, failed disk
   writes, stale async responses, and a second instance. No real keys in fixtures.
3. **Session lifecycle:** idle rechecks, same-session reconnect, next-session
   validation, worker shutdown, no queued pit command leaking across a lock.
4. **One EXE:** embedded assets, launcher consolidation, commercial feature audit,
   notices, clean-machine dependency check. Remove loose assets only when verified.
5. **Sale rehearsal:** signed build, Polar download, activate as a customer, restart,
   deactivate, update, and refund/cancel test. Verify when Polar actually removes
   the benefit for the chosen product settings; do not assume all payment events
   revoke a key immediately. Add server-side webhook handling only if that behavior
   needs custom business rules.

Run the existing `cargo test` and `cargo clippy --all-targets -- -D warnings`, plus
commercial-feature checks. Use mocked transport/clock for policy tests and sandbox
for API integration. Clean-VM UI and install tests are required before selling.

## Inputs still needed before connecting the commercial build

- Polar organization ID, overlay License Keys benefit ID, checkout and portal URLs.
- One-time purchase versus subscription, and whether trials/free grants exist.
- Device limit (proposal: two activations; confirm the commercial policy).
- Acceptance of online startup and allowing an authorized race session to finish.
- Publisher identity/signing setup and final customer-facing product name.

These do not prevent building the gated flow against a fake client and sandbox.
Never request or embed a Polar organization access token for desktop activation.

## Sources checked

- [Polar license benefits](https://polar.sh/docs/features/benefits/license-keys)
- [Public activation endpoint](https://polar.sh/docs/api-reference/customer-portal/license-keys/activate)
- [Validation fields and response states](https://polar.sh/docs/api-reference/customer-portal/license-keys/validate)
- [Public deactivation endpoint](https://polar.sh/docs/api-reference/customer-portal/license-keys/deactivate)
- [File download benefits](https://polar.sh/docs/features/benefits/file-downloads)

Documentation checked 2026-09-09. Current API reference pages redirect to a versioned
reference; confirm the version/header contract during integration and record it in
the client tests. The above is a design, not evidence of a tested Polar account.
