# Packaging into one exe, and the licence key

> Implementation follow-up, 2026-09-09: see
> [Commercial Windows release and Polar activation](commercial-release.md).
> That plan supersedes this draft's licensing policy, storage, demo exception,
> and release sequencing. Source privacy is now confirmed. This document remains
> useful as the original asset-embedding and launcher-consolidation investigation.

Decisions taken 2026-09-09: **the source is closing**, and **the launcher folds into
the overlay**. Both are assumed throughout.

Part A is mechanical. Part B is only coherent *because* of the first decision — a key
on a public MIT binary is a receipt, a key on a closed one is a gate. Part C is the
list of things closing the source knocks over, which is longer than it looks.

---

## Where things already stand

`cargo build --release` already produces the exe — `target/release/race-overlay.exe`,
7.5 MB, plus `race-launcher.exe` at 0.7 MB. The release workflow already tags, builds
and publishes a zip. So "make it an exe" is not the missing piece; **making it one
self-contained file** is.

Already embedded, via `include_bytes!`: the three fonts (`app.rs`, `install_fonts`),
the tray icon (`tray.rs`), the exe icon and version block (`build.rs` → `winresource`).

Still read off disk at runtime — the entire reason the zip carries an `assets/` folder:

| Where | Files | Size | Resolved by |
|---|---|---|---|
| `assets/logos/` | 461 SVG + `manifest.toml` | 5.4 MB | `ui::logos::logo_dir` |
| `assets/flags/` | 240 SVG | 2.0 MB | `ui::flags::flag_dir` |
| `assets/icons/` | 28 SVG | 39 KB | `ui::icons::icon_dir` |

`race-launcher.exe` reads no assets at all and is already self-contained.

The filesystem surface is small and uniform — three `*_dir()` functions, three
`*_uri()` functions that build a `file://` URI, and `logos::manifest()`. That is all
that has to change.

---

## Part A — one file

### A1. Embed the SVGs

`build.rs` walks `assets/{logos,flags,icons}` and writes an `embedded.rs` into
`OUT_DIR`: one sorted `&[(&str, &[u8])]` table per folder, each entry an
`include_bytes!` of the real path. `include_dir` would also do it, but a generated
table needs no new dependency, matches the `build.rs` already in the tree, and
`binary_search_by_key` on a sorted table is the same lookup shape the code already
uses for `FLAIRS`.

`build.rs` emits `cargo:rerun-if-changed` per folder, so adding a logo rebuilds.

### A2. Serve them as `bytes://`

`egui::Context::include_bytes(uri, bytes)` exists in egui 0.29.1 (`context.rs:3242`)
and is the intended route. Each `*_uri()` keeps its current `Memo` shape and its
once-per-name cost; only the miss path changes:

```
fn flag_uri(ctx: &egui::Context, code: &str) -> Option<Arc<str>>
```

- loose file on disk → `file://…` exactly as today
- otherwise embedded table hit → `ctx.include_bytes("bytes://flags/de.svg", …)` once,
  return that URI
- otherwise `None`, and the existing fallbacks (the `world` flag, the painter-drawn
  glyph) fire unchanged

All three `draw` functions already take `&Ui`, so `ui.ctx()` is in hand at every call
site. No signature change reaches past the module.

### A3. Keep the loose folder as an override

**Default: embedded.** If `assets/<name>/<file>` is found by the existing
`find_asset_dir` walk, the file wins.

Why keep it: a fat-LTO rebuild to preview one recoloured logo is a bad loop, and it
lets a user drop in their own flag. Costs one `is_file()` per asset name per run —
already what the code does.

`manifest.toml` follows the same rule: `include_str!` the embedded copy, parse a loose
file instead when there is one. A malformed *loose* manifest falls back to the embedded
one rather than to `Manifest::default()`, which is strictly better than today's silent
"every brand white".

### A4. Fold the launcher in

`race-launcher.exe` becomes `race-overlay.exe --launch`. The launcher's `main` moves
into a `launch` module; the flag is read alongside the existing `--demo` and
`--screenshot=` handling in `main.rs` and returns before any window is created. The
shared code already lives in `race_tools::launcher`, so this is a move, not a rewrite.

`race-launcher.exe` stays as a build target for one release cycle so the change does
not strand anything mid-upgrade, then goes.

**This repoints three existing shortcuts**, all with working dir = repo root:

- Desktop, "iRacing Launcher" → `target\release\race-launcher.exe`
- Start Menu, "Race Launcher" → the whole stack
- Start Menu, "Race Overlay" → the overlay alone

All three need rewriting to the single exe, the first two gaining `--launch`. Worth
doing as part of the phase rather than discovering it at the next race.

### A5. The C runtime

Nothing pins the MSVC CRT statically, so the exe wants `VCRUNTIME140.dll`. Nearly
every machine that runs iRacing has it, but "nearly" is a support class for a paid
product.

```toml
# .cargo/config.toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

**Verify, do not assume** — `mimalloc` and the native GLFW `egui_overlay` builds both
compile C and have to pick `/MT` to match. `cc` reads the flag and normally does. If
either fights it, drop this: nice-to-have, not a blocker.

### A6. Third-party notices — this one is easy to lose

Every bundled asset set is permissive, and every one of them requires its licence text
to ship alongside:

| Asset | Licence | File today |
|---|---|---|
| Car marks (cardog-ai/icons) | MIT | `assets/logos/LICENSE-cardog-icons.txt` |
| Interface icons (Lucide) | ISC | `assets/icons/LICENSE-lucide.txt` |
| Flags (flag-icons) | MIT | `assets/flags/LICENSE-flag-icons.txt` |
| Inter, IBM Plex Mono, Barlow Condensed | OFL | `assets/fonts/*-OFL.txt` |

The moment `assets/` stops shipping, those files stop shipping with it — and a paid
closed binary is exactly the case where that matters. So: `build.rs` concatenates them
(plus the Rust crate licences) into a single `NOTICES.txt` in `OUT_DIR`, `include_str!`d,
surfaced two ways — a **Licences** section on the Settings *About* page, and
`race-overlay.exe --licences` to stdout. Ship `NOTICES.txt` in the zip as well; it costs
nothing and is the form people expect to find it in.

Separately, and not a copyright question: the manufacturer marks are **trademarks**, and
the MIT licence on the SVG files grants no trademark rights. Drawing 51 car brands inside
a paid product is a different posture from drawing them in a free one. Probably fine — it
is nominative use, identifying the car you are racing — but it is worth a considered look
before the first sale rather than after a letter, and the answer might just be "colour
files only for brands that permit it" or nothing at all.

### A7. Result

One `race-overlay.exe`, roughly 15 MB (7.5 MB now + ~7.4 MB of SVG, uncompressed — not
worth compressing at this size). The release zip becomes: the exe, `config.toml`,
`README.md`, `LICENSE`, `NOTICES.txt`.

---

## Part B — the gate

### The facts on Polar

Verified against Polar's own docs:

- Keys are issued automatically on purchase, with a branded prefix (`RACE_*`).
- `POST https://api.polar.sh/v1/customer-portal/license-keys/activate` — body `key`,
  `organization_id`, `label`, optional `conditions`/`meta`. **No client secret.**
  Required only when activation limits are configured.
- `POST https://api.polar.sh/v1/customer-portal/license-keys/validate` — body `key`,
  `organization_id`, plus `activation_id` when limits are on.
- Activation limits cap machines; the customer deactivates one from their portal.
- Keys can expire, be revoked, and be rotated without losing history.

Both endpoints being secret-free is what makes this workable from the client at all —
nothing goes in the binary but the org id, which is not a secret.

### B1. What "gated" means here

The overlay refuses to draw panels without a valid licence. It does **not** refuse to
start: it starts, sits in the tray, and opens the activation window. `--launch` is
gated too — it is the same purchase.

`--demo` is the exception; see B6.

### B2. Where the licence lives

`%APPDATA%\race\licence.toml`, **its own file** — not `race-overlay.toml`, which is
rewritten whole on every settings change. Same `APPDATA`-then-beside-the-exe fallback
`config.rs` already uses.

```toml
key = "RACE_XXXX-…"
activation_id = "…"
customer_email = "…"
activated_at = 2026-09-09T00:00:00Z
last_validated = 2026-09-09T00:00:00Z
```

A missing, empty or unparseable file means unactivated. A *corrupt* one is not treated
as tampering — it is rewritten from the next successful validation, and the customer is
never asked to re-enter a key they already own because a disk write was interrupted.

### B3. Activation

Settings gains a **Licence** page: key field, Activate button, status line. Activate
fires one background POST to `/activate`, `label` = machine name. The UI thread never
waits — the button reads "Checking…" and the result lands on a later frame.

- **200** → write `licence.toml`, panels come up immediately, no restart.
- **Rejected (4xx with a real reason)** → show Polar's own message verbatim. A buyer who
  pasted a key with a typo needs to see which failure it was, not a paraphrase.
- **Activation limit reached** → say so plainly, and link the Polar portal where they can
  free a machine. This is the single most likely support email; it should answer itself.
- **No network / timeout / 5xx** → "Couldn't reach Polar — check your connection and try
  again." Nothing is written. Not a failure state, just a retry.

First activation requires a network round trip once. That is unavoidable with a hosted
key system and is fine.

### B4. Revalidation, and the rule that outranks it

**Nothing about licensing may ever change behaviour during a session.** A panel must not
vanish mid-stint. Every rule below is subordinate to that one.

Revalidate at most **once every 30 days**, at startup, in the background.

- Success → stamp `last_validated`, say nothing.
- **Ambiguous — offline, timeout, 5xx, DNS** → do nothing at all. Keep the stamp, keep
  running, try next launch. Indefinitely, with no countdown. Someone who paid does not
  lose their overlay because their router rebooted, and an offline sim rig is a normal
  thing to own.
- **Unambiguous revocation** (404 / `not_found` / a validated-false body) → revert to
  unactivated **at the next launch**, never in the running session. A refunded key going
  quiet at the start of the next sim day is fine.

So the only network dependency after activation is one that can never take the overlay
away while it is being used.

### B5. The HTTP call

- **WinHTTP via the `windows` crate** (add `Win32_Networking_WinHttp`). Zero new
  dependencies, matches how this codebase already talks to Windows, and this is one POST
  with a JSON body. **Recommended.**
- `ureq` + native-tls. Less code to write, one more dependency tree; `native-tls` is
  already in the lockfile via `tungstenite`, so the marginal cost is small.

Either sits behind `licence::activate()` / `licence::validate()` so the choice is
reversible.

### B6. The trial question — now unavoidable

Closed source plus a hard gate means nobody can see this run before paying. Screenshots
are all that is left, and this overlay's whole argument is what it does *during a race*.

The pieces are already there: `--demo` renders every panel on a fixed snapshot, which is
what generates the README images. Three shapes, roughly ascending effort:

1. **Demo mode stays ungated.** Anyone can run `--demo` and drag the real panels around
   on stand-in data. No live telemetry. Cheapest, and already built.
2. **Time-limited full trial.** Fully working for N days from first run, tracked in
   `licence.toml`. Clock-rollback resistant only via a `first_seen` stamp, which is
   defeatable — acceptable, since the goal is a trial not a vault.
3. **No trial.** Refund window carries it. `site/README.md` already has a refund-window
   placeholder to fill.

Not specced further here because it is a pricing decision, not an engineering one.

### B7. How much anti-tampering — deliberately, very little

Closed source removes the "here is the ten-line patch" problem, and that is most of the
benefit available. Beyond that: strip is already on, `panic = "abort"` is already on, and
signing is handled. Do **not** spend time on obfuscation, packers, or debugger checks.
They cost real days, break under antivirus heuristics — a genuine risk for a program that
reads another process's telemetry and draws an always-on-top window — and buy nothing
against anyone determined. Check the licence in more than one place, and stop.

---

## Part C — what closing the source knocks over

Not blockers; things that will otherwise be found late.

1. **Closing is prospective only.** Making the repo private does not retract the MIT
   grant on commits already published, and does not unmake existing clones or forks.
   Everything up to the switch stays MIT-licensed in whoever's hands already has it. That
   is fine, but it means the private repo protects *future* work, not the current tree.

2. **The sales page argues the opposite.** `site/index.html` leans on "read every line
   that runs next to your sim" as a trust asset, and the comparison table and FAQ lean on
   the same. That copy has to be rewritten, and the honest replacement is a different
   argument — signed builds, a named developer, a refund window — rather than a quieter
   version of the old one.

3. **README and manual promise a build.** `README.md` opens with "Install, configure and
   **build**", and `docs/manual.md` carries the instructions. Both need reframing to
   install-only.

4. **Distribution moves off GitHub Releases.** A private repo has no public release
   downloads, so the zip is delivered by Polar as a file benefit. The release workflow
   still builds and signs; its last step changes from "publish a GitHub release" to
   "upload to Polar" (or produce the artefact you upload). Actions minutes on a private
   repo are billed rather than free — trivial at this volume, but no longer zero.

5. **`LICENSE` in the zip is now the wrong file.** It is the MIT text. It becomes an EULA
   for the binary, with `NOTICES.txt` (A6) carrying the third-party terms beside it.

6. **The memory that says otherwise.** The recorded project note said the repo stays
   public MIT and the paid thing is the signed build. Superseded by this document.

---

## Phasing

Each phase ships something that stands on its own.

1. **A1–A3.** Embed and serve. Drop the `assets/` copy from the release workflow.
   This is the whole of "make it an exe" and depends on nothing else here.
2. **A4 + A6.** Fold in the launcher, repoint the three shortcuts, generate `NOTICES.txt`.
3. **A5.** Try `crt-static`; keep only if the C dependencies come along quietly.
4. **B2–B3, B5.** `licence.toml`, the Licence page, activation. Gate the panels.
5. **B4.** Background revalidation.
6. **Part C.** Site copy, README, EULA, delivery — alongside the repo actually going
   private, not before.

Phase 1 is the one with immediate value, and it is independent of every licensing
decision. It could be done today whatever else is settled.
