# Launcher in Settings — a program list you tick, not a TOML you edit

`race-launcher.exe` starts the session stack from `config.toml` beside the
exe: a fixed list of paths typed in by hand. It works for exactly one
machine. On this one, `config.toml` still says iRacing UI lives in
`C:\Program Files (x86)\iRacing\ui\`, and it doesn't — it is on `E:` — so
the launcher has been printing `[??] iRacing UI: not found` and starting
everything else. Nothing told anyone. Garage 61, RacelabApps, MOZA Pit
House and RaceDirector are installed and in the Start Menu, and none of
them is in the list, because adding one means finding an exe path and
editing TOML.

This is the spec for a **Launcher** page in the overlay's settings window:
a catalogue of the programs sim racers run alongside iRacing, each found on
disk automatically where it can be, a tick to start it or not, and room for
the driver's own additions. `race-launcher.exe` keeps its job — one
double-click starts the lot — it just reads the list this page writes.

## Table of contents

- [Decisions](#decisions)
- [The catalogue](#the-catalogue)
- [Finding a program](#finding-a-program)
- [Config](#config)
- [Settings page](#settings-page)
- [The launcher](#the-launcher)
- [Edge cases](#edge-cases)
- [Build order](#build-order)
- [Out of scope](#out-of-scope)

---

## Decisions

1. **The launcher and the overlay stay two programs.** Flynn starts them
   separately and the README documents them separately. The overlay
   *configures* the launcher; it does not become it. The only launching
   the overlay does is the page's **Start now** buttons, which run the
   same code the launcher does.
2. **The list moves to `%APPDATA%\race\launcher.toml`.** The overlay's
   settings already live there and are written from the window; a file
   beside the exe in a build folder is not something a settings window
   should write to. `config.toml` beside the exe is read once as a seed
   if `launcher.toml` doesn't exist yet, so the current list carries over.
3. **A catalogue of known programs, detected on disk.** Each has a stable
   id, a display name, a one-line note on what it is, the exe file name,
   and the places it is usually installed. Detection is a list of
   candidates checked with `is_file()`; the first hit wins; a miss shows
   as "Not found" with a Browse button, never as an entry that silently
   fails at launch.
4. **Enabled by default: what the launcher starts today, plus anything
   detected that is clearly part of a session.** The seed from
   `config.toml` decides for an existing install. On a fresh install,
   detected iRacing UI, Trading Paints, Crew Chief, Coach Dave Delta,
   Garage 61 and the overlay itself are ticked; detected wheel-base
   software (SimPro, Pit House) is ticked because a base with no manager
   running is a base with no FFB profile; everything else detected is
   listed but unticked. Not found = unticked, always.
5. **The driver's own programs are first-class rows.** Add via a native
   file picker (or by typing a path); same tick, same order controls, same
   Start now. They are stored by path and name; nothing about them is
   special except that detection never touches them.
6. **Order is start order, and the overlay is last by default.** It waits
   idle for iRacing anyway, and coming up last puts its window on top.
   Rows can be moved.
7. **Every edit saves on settle, like every other page.** No Apply.

## The catalogue

Built into the shared crate (`race_tools::launcher::CATALOGUE`), so both
binaries agree. `%X%` are the standard environment folders, expanded at
detection time.

| id | Name | What it is | Exe | Where to look first |
| --- | --- | --- | --- | --- |
| `iracing-ui` | iRacing UI | The sim's launcher | `iRacingUI.exe` | `%ProgramFiles(x86)%\iRacing\ui\` |
| `trading-paints` | Trading Paints | Custom liveries | `Trading Paints.exe` | `%ProgramFiles(x86)%\Rhinode LLC\Trading Paints\` |
| `crew-chief` | Crew Chief | Spotter and engineer | `CrewChiefV4.exe` | `%ProgramFiles(x86)%\Britton IT Ltd\CrewChiefV4\` |
| `coach-dave-delta` | Coach Dave Delta | Setups and delta | `Coach Dave Delta.exe` | `%LOCALAPPDATA%\CoachDaveDelta\` |
| `garage61` | Garage 61 | Telemetry sharing | `garage61-launcher.exe` | `%APPDATA%\garage61-install\` |
| `racelab` | RacelabApps | Overlays | `RacelabApps.exe` | `%LOCALAPPDATA%\racelabapps\` |
| `simhub` | SimHub | Dash and devices | `SimHubWPF.exe` | `%ProgramFiles(x86)%\SimHub\` |
| `simpro` | SimPro Manager | Simagic base | `simpro3.exe` | `%ProgramFiles(x86)%\Simagic\Simpro3\bin\` |
| `moza-pit-house` | MOZA Pit House | MOZA base | `MOZA Pit House.exe` | `%ProgramFiles(x86)%\MOZA Pit House\` |
| `fanatec` | Fanatec Control Panel | Fanatec base | `Fanatec Control Panel.exe` | `%ProgramFiles%\Fanatec\Fanatec Control Panel\` (unverified) |
| `kapps` | Kapps | Overlays | `Kapps.exe` | Start Menu / registry only |
| `jrt` | Joel Real Timing | Timing overlay | `JRT.exe` | Start Menu / registry only |
| `ioverlay` | iOverlay | Overlays | `iOverlay.exe` | Start Menu / registry only |
| `vrs` | VRS Telemetry | Telemetry | `VRS*.exe` | Start Menu / registry only |
| `lovely` | Lovely Sim Racing | Dash | `Lovely*.exe` | Start Menu / registry only |
| `marvins` | Marvin's Awesome iRacing App | Timing and setups | `*.exe` in its folder | `%LOCALAPPDATA%\Programs\Marvins Awesome iRacing App*\` |
| `racedirector` | RaceDirector | League tools | `RaceDirector.exe` | `%APPDATA%\RaceDirector\` |
| `discord` | Discord | Voice | `Update.exe --processStart Discord.exe` | `%LOCALAPPDATA%\Discord\` |
| `obs` | OBS Studio | Recording | `obs64.exe` | `%ProgramFiles%\obs-studio\bin\64bit\` |
| `race-overlay` | Race Overlay | This overlay | `race-overlay.exe` | Beside the running overlay |

The first nine rows and the overlay are verified against this machine or
its `config.toml`; rows marked *unverified* or *Start Menu / registry only*
are there so the second and third passes below have a name to match, and
their first-look paths are best effort. A row's note is what the page shows
under its name, so a driver who has never heard of Kapps learns what it is
without leaving the window.

## Finding a program

For each catalogue entry, candidates are tried in this order and the
first existing file wins:

1. **The saved path** in `launcher.toml`, if the driver set or confirmed
   one. A saved path that no longer exists falls through to the passes
   below, and the row says "moved — found at …" rather than "not found",
   so a reinstall to a new drive fixes itself.
2. **Known folders** from the table, each joined with the exe name.
3. **Start Menu shortcuts** — every `.lnk` under the user's and the
   machine's `Start Menu\Programs`, resolved with `IShellLinkW` (the
   `windows` crate, already a dependency, with `Win32_UI_Shell` and
   `Win32_System_Com` added). A shortcut matches when its target's file
   name equals the entry's exe name (or glob), whatever folder it is in.
   This is the pass that finds iRacing on `E:` here.
4. **The registry uninstall keys** (`HKLM\...\Uninstall`, its
   `WOW6432Node` twin, and `HKCU\...\Uninstall`): an entry whose
   `DisplayName` matches the catalogue name gives `InstallLocation` and
   the folder of `DisplayIcon`; each is joined with the exe name. This is
   a fallback — on this machine iRacing's `InstallLocation` points at a
   folder that doesn't hold the UI, which is why it is last.

Detection runs on a worker thread when the page is first opened (it
touches a few hundred files and the registry — fast, but not for the
render thread) and again on **Rescan**. Results are cached for the run.
The launcher itself runs the same detection at start for any enabled
catalogue row whose saved path is missing, so a moved program is still
started.

Arguments: a catalogue entry may carry fixed args (Discord's
`--processStart Discord.exe`); a custom row may have its own. The launcher
already passes `args` and sets the working directory to the exe's folder
(OBS needs that).

## Config

`%APPDATA%\race\launcher.toml`, written by the page, read by the launcher:

```toml
# Start order is list order.
[[programs]]
id = "iracing-ui"                 # catalogue id; absent for a custom row
enabled = true
path = 'E:\Iracing\iRacing\ui\iRacingUI.exe'   # confirmed/chosen; may be absent

[[programs]]
id = "garage61"
enabled = false

[[programs]]
name = "My stream deck profile"   # custom row: name + path, no id
enabled = true
path = 'C:\Tools\deck.exe'
args = ["--profile", "race"]

[[programs]]
id = "race-overlay"
enabled = true
```

- Catalogue rows that the driver has never touched are **not** written;
  the page lists the whole catalogue regardless, with defaults. So the
  file stays short and a new catalogue entry appears for everyone.
- `path` on a catalogue row is written only when the driver picked one
  with Browse, or when detection found one and the driver ticked the
  row (so the launcher doesn't have to rescan every start). A path that
  detection found and the driver never enabled is not written.
- The launcher's `Config`/`Program` in `src/config.rs` are replaced by
  this shape (`LauncherConfig`, `ProgramEntry`), with `Serialize` added.
  `find_beside_exe_or_cwd` stays for the seed.

**Seeding.** When `launcher.toml` doesn't exist, the first reader (page or
launcher) looks for `config.toml` beside the exe / in the working
directory and converts it: each `[[programs]]` becomes a row, matched to a
catalogue id by exe file name where one matches (so "SimPro Manager" with
`simpro3.exe` becomes `simpro`), otherwise kept as a custom row. All
seeded rows are enabled, in the seed's order. The seed is then written as
`launcher.toml`, and `config.toml` is left where it is but ignored from
then on; the README says so.

## Settings page

A new **Launcher** page in the rail, after Binds:

```
Programs started by race-launcher.exe, in this order.        [ Rescan ]  [ Start all now ]

 [x] iRacing UI              The sim's launcher                       ▲ ▼   [ Start ]
     E:\Iracing\iRacing\ui\iRacingUI.exe                                    [ Browse… ]
 [x] Trading Paints          Custom liveries                           ▲ ▼   [ Start ]
     C:\Program Files (x86)\Rhinode LLC\Trading Paints\Trading Paints.exe   [ Browse… ]
 [ ] Garage 61               Telemetry sharing                         ▲ ▼   [ Start ]
     %APPDATA%\garage61-install\garage61-launcher.exe                       [ Browse… ]
 [ ] SimHub                  Dash and devices                          ▲ ▼
     Not found                                                              [ Browse… ]
 [x] My stream deck profile  Custom                                    ▲ ▼   [ Start ]  [ Remove ]
     C:\Tools\deck.exe --profile race                                       [ Browse… ]
 [x] Race Overlay            This overlay (running)                    ▲ ▼
     C:\...\race-overlay.exe

 [ Add a program… ]                                            [ Reset to defaults ]
```

- One row per catalogue entry plus each custom row, in start order. The
  path line is the detected or chosen path, shortened with `%APPDATA%`
  style variables where they apply so it fits; hovering shows the full
  one. A "Not found" row is greyed, unticked, and has only Browse.
- **Tick** = enabled. Ticking a detected row writes its path.
- **▲ ▼** move a row; the overlay row can be moved too (someone may
  want it first).
- **Start** launches that row now, through the same `start_program` the
  launcher uses; the button reads **Running** and is disabled while the
  exe name is among running processes (the `sysinfo` scan the launcher
  already does, refreshed once a second while the page is open).
- **Start all now** does what a double-click on the launcher does, minus
  the console window — skipping running ones.
- **Browse…** opens `IFileOpenDialog` filtered to `*.exe`, on a helper
  thread so the overlay keeps drawing; the result sets `path`. The
  dialog's initial folder is the current path's folder if any.
- **Add a program…** opens the same dialog; the new custom row's name is
  the exe's file description from its version resource, falling back to
  the file stem. Name and args are editable in the row.
- **Remove** exists only on custom rows; a catalogue row is unticked
  instead.
- **Rescan** re-runs detection; a row with a driver-chosen path keeps it.
- **Reset to defaults** forgets every path and tick and re-detects, which
  is exactly a fresh install's page.
- The page is taller than the others; it scrolls inside the fixed page
  height rather than growing the window.

Nothing here is slanted.

## The launcher

`race-launcher.exe` changes only in what it reads:

1. Load `launcher.toml` (seeding from `config.toml` if absent).
2. For each enabled row in order: resolve the path (saved path, else
   detection for a catalogue id), skip if running, skip with `[??]` if
   nothing found, else start. Output lines as today.
3. The linger and `--dry-run` stay.

`start_program` and `running_exe_names` move into the shared crate so the
page's buttons call the same code.

## Edge cases

- **No `launcher.toml`, no `config.toml`** (fresh install) — the catalogue
  with detection defaults; the launcher starts what is detected and
  ticked by default. Nothing to edit before the first double-click works.
- **Both files exist** — `launcher.toml` wins; `config.toml` is ignored.
- **`launcher.toml` unreadable** (bad TOML) — the page shows a notice
  with the error and the file's path, lists the catalogue with defaults,
  and does not write until the driver changes something (at which point
  the bad file is renamed `.bad` beside the new one, the same way the
  overlay treats its own unreadable config). The launcher prints the
  error and starts nothing rather than guessing.
- **Two rows with the same exe name** (a custom row pointing at a
  catalogue program) — both shown; the second is skipped at launch as
  "already running", which is correct and visible.
- **The catalogue gains an entry** in a later build — it appears
  unticked-or-detected like any other; nothing in the file needs
  changing.
- **A saved path that no longer exists** — see detection pass 1; the row
  shows the fallback with "moved" and writes the new path when ticked
  again or on Rescan.
- **Detection finds two copies** (a Steam and a standalone install) — the
  first pass to hit wins, and the row's Browse is how to choose the other.
- **The overlay's own row** — path is `current_exe()`, never detected or
  browsed; the tick still matters to the launcher.
- **Start now while the overlay is click-through** — the window is up,
  so egui has the pointer; a child process is spawned detached, as the
  launcher does, and the overlay's focus rules restore foreground the
  way they do after any launch.
- **Program needs elevation** (some device managers) — `spawn` fails with
  a permission error; the row shows the error text under the path for
  ten seconds. Not retried elevated.
- **`%APPDATA%` unset** — the overlay already falls back to beside the
  exe for its own config; `launcher.toml` follows it.

## Status (2026-08-29)

All four phases are written, uncompiled at the time of writing (done
mid-race, with a no-build rule in force): `race_tools::launcher` (config,
catalogue, detection through `launcher::detect` — Start Menu shortcuts via
`IShellLinkW`, uninstall keys via the registry API, `IFileOpenDialog` for
Browse, the version resource for a new row's name), `race-launcher.exe`
reading the new list, and the **Launcher** page in `ui/launcher_page.rs`.
One departure from the text above: once the driver changes anything, the
page writes the whole merged list — catalogue rows included — rather than
only the rows touched; the merge in `LauncherConfig::rows` still appends any
catalogue entry a later build adds. Elevation failures show as "could not
start" under the row for ten seconds, as specified. Two changes after
first use: a row's tick can be changed whether or not the program was
found (the launcher skips a missing one regardless), and `launcher.toml`
gained `start_with_overlay` — the page's "Also start the ticked programs
when the overlay starts" — which the overlay honours on its own startup
(not in demo mode) via `launcher::start_with_overlay_if_set`, a background
thread that starts what isn't already running. "Not found = unticked,
always" above is therefore no longer true; the launcher's output is. To finish:
`cargo build --release`, `race-launcher --dry-run` (iRacing UI should now
be found on `E:`), then open the page and check detection against the
Start Menu.

## Build order

1. **Shared crate.** `LauncherConfig` in `%APPDATA%`, seed from
   `config.toml`, the catalogue, detection (known folders + Start Menu +
   registry), `start_program`/running moved in. `race-launcher.exe` reads
   the new file. Verified by `--dry-run` printing the same list as today,
   with iRacing UI now found on `E:`.
2. **The page**, read-only first: rows, ticks, detected paths, order,
   Start / Start all, settle-and-save.
3. **Browse, Add, Remove, Rescan, Reset** — the `IFileOpenDialog` helper
   thread and the version-resource name.
4. **Polish**: running indicator refresh, path shortening, the
   unreadable-file notice, README and `config.toml` header updated to
   point at the page.

Phase 1 alone fixes the iRacing-on-`E:` bug for the launcher.

## Out of scope

- Starting programs **in the overlay's own startup** (autostart the stack
  when the overlay opens). That is the launcher's job; two entry points
  that both start things is the confusion this avoids.
- Waiting for iRacing before starting something, or delays between
  programs. Everything in the catalogue copes with being started first.
- Closing programs at the end of a session.
- Windows startup / scheduled-task registration for the launcher.
- Detecting programs that are only installed through Steam by their app
  id; the Start Menu pass catches their shortcuts.
