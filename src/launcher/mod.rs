// Rust guideline compliant 2026-02-16

//! The session launcher's program list: what to start, where it is, how.
//!
//! The list lives in `%APPDATA%\race\launcher.toml` ([`LauncherConfig`]),
//! written by the overlay's settings window and read by `race-launcher.exe`.
//! Rows are either an entry of the built-in [`CATALOGUE`] — the programs sim
//! racers run alongside iRacing, each with the places it is usually installed
//! — or a program the driver added by path.
//!
//! A catalogue row need not carry a path: [`detect_all`] finds the program on
//! this machine by looking in its known folders, then through the Start Menu's
//! shortcuts, then the registry's uninstall entries (see [`detect`]). A saved
//! path that still exists wins over all of that, so a driver's own choice
//! sticks; one that has gone falls back to detection, so a reinstall on
//! another drive fixes itself.
//!
//! Nothing here is started until [`start`] is called. The launcher calls it
//! for every enabled row; the settings page calls it for **Start** buttons.
//! Both skip a program whose exe name is already among the running processes.
//! See `plans/launcher-settings.md` for the decisions.

pub mod detect;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

use crate::config::{Config as LegacyConfig, find_beside_exe_or_cwd};

/// Folder under `%APPDATA%` these tools keep their settings in — the same
/// one `race-overlay.toml` uses.
const CONFIG_DIR_NAME: &str = "race";

/// File name of the program list.
pub const LAUNCHER_FILE_NAME: &str = "launcher.toml";

/// The catalogue id of the overlay itself, which is found beside whichever
/// of the two binaries is running rather than looked for.
pub const OVERLAY_ID: &str = "race-overlay";

/// File name of the overlay's executable.
const OVERLAY_EXE: &str = "race-overlay.exe";

/// One program the launcher knows how to find.
///
/// `exe` may hold a single `*` (`VRS*.exe`) for programs whose file name
/// carries a version. `folders` are tried first, in order, with `%VAR%`
/// expanded from the environment; `%EXE_DIR%` is the running binary's own
/// folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    /// Stable id written to `launcher.toml`; never renamed.
    pub id: &'static str,
    /// What the settings page calls it.
    pub name: &'static str,
    /// One line on what it is, for a driver who has never heard of it.
    pub note: &'static str,
    /// The executable's file name, matched case-insensitively.
    pub exe: &'static str,
    /// Folders it is usually installed in, most likely first.
    pub folders: &'static [&'static str],
    /// Arguments it needs, if any.
    pub args: &'static [&'static str],
    /// Whether a fresh install starts it when it is found.
    pub default_enabled: bool,
}

/// Every program the launcher knows, in the order a fresh list shows them.
///
/// The overlay is last so its window comes up over the others; it waits idle
/// for iRacing anyway. Wheel-base software is on by default because a base
/// with no manager running is a base with no force-feedback profile. Overlay
/// and telemetry apps that compete with this overlay are listed but off.
pub const CATALOGUE: &[Known] = &[
    Known {
        id: "iracing-ui",
        name: "iRacing UI",
        note: "The sim's launcher",
        exe: "iRacingUI.exe",
        folders: &[r"%ProgramFiles(x86)%\iRacing\ui"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "trading-paints",
        name: "Trading Paints",
        note: "Custom liveries",
        exe: "Trading Paints.exe",
        folders: &[r"%ProgramFiles(x86)%\Rhinode LLC\Trading Paints"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "crew-chief",
        name: "Crew Chief",
        note: "Spotter and engineer",
        exe: "CrewChiefV4.exe",
        folders: &[r"%ProgramFiles(x86)%\Britton IT Ltd\CrewChiefV4"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "coach-dave-delta",
        name: "Coach Dave Delta",
        note: "Setups and delta",
        exe: "Coach Dave Delta.exe",
        folders: &[r"%LOCALAPPDATA%\CoachDaveDelta"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "garage61",
        name: "Garage 61",
        note: "Telemetry sharing",
        exe: "garage61-launcher.exe",
        folders: &[r"%APPDATA%\garage61-install"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "racelab",
        name: "RacelabApps",
        note: "Overlays",
        exe: "RacelabApps.exe",
        folders: &[r"%LOCALAPPDATA%\racelabapps"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "simhub",
        name: "SimHub",
        note: "Dash and devices",
        exe: "SimHubWPF.exe",
        folders: &[r"%ProgramFiles(x86)%\SimHub"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "simpro",
        name: "SimPro Manager",
        note: "Simagic wheel base",
        exe: "simpro3.exe",
        folders: &[r"%ProgramFiles(x86)%\Simagic\Simpro3\bin"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "moza-pit-house",
        name: "MOZA Pit House",
        note: "MOZA wheel base",
        exe: "MOZA Pit House.exe",
        folders: &[r"%ProgramFiles(x86)%\MOZA Pit House"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "fanatec",
        name: "Fanatec Control Panel",
        note: "Fanatec wheel base",
        exe: "Fanatec Control Panel.exe",
        folders: &[r"%ProgramFiles%\Fanatec\Fanatec Control Panel"],
        args: &[],
        default_enabled: true,
    },
    Known {
        id: "kapps",
        name: "Kapps",
        note: "Overlays",
        exe: "Kapps.exe",
        folders: &[r"%LOCALAPPDATA%\Programs\Kapps"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "jrt",
        name: "Joel Real Timing",
        note: "Timing overlay",
        exe: "JRT.exe",
        folders: &[r"%ProgramFiles(x86)%\Joel Real Timing"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "ioverlay",
        name: "iOverlay",
        note: "Overlays",
        exe: "iOverlay.exe",
        folders: &[r"%LOCALAPPDATA%\iOverlay"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "vrs",
        name: "VRS Telemetry",
        note: "Telemetry logger",
        exe: "VRS*.exe",
        folders: &[r"%LOCALAPPDATA%\Programs\VRS"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "lovely",
        name: "Lovely Sim Racing",
        note: "Dash",
        exe: "Lovely*.exe",
        folders: &[r"%LOCALAPPDATA%\Lovely Sim Racing"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "marvins",
        name: "Marvin's Awesome iRacing App",
        note: "Timing and setups",
        exe: "Marvin*.exe",
        folders: &[r"%LOCALAPPDATA%\Programs\Marvins Awesome iRacing App - Refactored"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "racedirector",
        name: "RaceDirector",
        note: "League tools",
        exe: "RaceDirector.exe",
        folders: &[r"%APPDATA%\RaceDirector"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: "discord",
        name: "Discord",
        note: "Voice",
        exe: "Update.exe",
        folders: &[r"%LOCALAPPDATA%\Discord"],
        args: &["--processStart", "Discord.exe"],
        default_enabled: false,
    },
    Known {
        id: "obs",
        name: "OBS Studio",
        note: "Recording",
        exe: "obs64.exe",
        folders: &[r"%ProgramFiles%\obs-studio\bin\64bit"],
        args: &[],
        default_enabled: false,
    },
    Known {
        id: OVERLAY_ID,
        name: "Race Overlay",
        note: "This overlay",
        exe: OVERLAY_EXE,
        folders: &["%EXE_DIR%"],
        args: &[],
        default_enabled: true,
    },
];

/// The catalogue entry with this id, if any.
#[must_use]
pub fn known(id: &str) -> Option<&'static Known> {
    CATALOGUE.iter().find(|known| known.id == id)
}

/// The program list, as written to `launcher.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct LauncherConfig {
    /// Rows in start order.
    #[serde(default)]
    pub programs: Vec<ProgramEntry>,
    /// Also start the enabled rows when `race-overlay.exe` itself starts,
    /// for people who start the overlay directly rather than through the
    /// launcher. Off by default: two entry points that both start things
    /// is a surprise unless asked for.
    #[serde(default)]
    pub start_with_overlay: bool,
}

/// One row of the list: a catalogue program or the driver's own.
///
/// A catalogue row has an `id` and takes its name, note and arguments from
/// the catalogue; `path` is written only once the driver has chosen or
/// confirmed one. A custom row has no `id` and carries everything itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProgramEntry {
    /// Catalogue id, or `None` for a program the driver added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Display name; required for a custom row, ignored for a catalogue one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the launcher starts it.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Where it is, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Extra command-line arguments; a custom row's own, or an override of
    /// the catalogue's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl ProgramEntry {
    /// A row for one catalogue program, with its default tick and no path.
    #[must_use]
    pub fn for_known(known: &Known) -> Self {
        Self { id: Some(known.id.to_owned()), name: None, enabled: known.default_enabled, path: None, args: Vec::new() }
    }

    /// The catalogue entry behind this row, if it is a catalogue row.
    #[must_use]
    pub fn known(&self) -> Option<&'static Known> {
        self.id.as_deref().and_then(known)
    }

    /// What the row is called.
    #[must_use]
    pub fn display_name(&self) -> String {
        if let Some(known) = self.known() {
            return known.name.to_owned();
        }
        self.name.clone().or_else(|| self.path.as_deref().and_then(file_stem)).unwrap_or_else(|| "Program".to_owned())
    }

    /// The one-line note under the name.
    #[must_use]
    pub fn note(&self) -> &'static str {
        self.known().map_or("Added by you", |known| known.note)
    }

    /// The arguments it is started with: the row's own if any, else the
    /// catalogue's.
    #[must_use]
    pub fn effective_args(&self) -> Vec<String> {
        if !self.args.is_empty() {
            return self.args.clone();
        }
        self.known().map(|known| known.args.iter().map(|arg| (*arg).to_owned()).collect()).unwrap_or_default()
    }
}

/// Where `launcher.toml` is read from and written to.
///
/// `%APPDATA%\race\launcher.toml`, falling back to beside the executable on
/// a machine with no `APPDATA`.
#[must_use]
pub fn launcher_path() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(appdata).join(CONFIG_DIR_NAME).join(LAUNCHER_FILE_NAME);
    }
    let beside_exe = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(LAUNCHER_FILE_NAME)));
    beside_exe.unwrap_or_else(|| PathBuf::from(LAUNCHER_FILE_NAME))
}

impl LauncherConfig {
    /// Loads `launcher.toml`, seeding it from an older `config.toml` if needed.
    ///
    /// With neither file present the list is empty, which [`Self::rows`]
    /// fills from the catalogue — a fresh install needs no file at all.
    ///
    /// # Errors
    /// Returns an error if `launcher.toml` exists but is not valid TOML for
    /// this schema, or if the seed file exists but cannot be parsed. Neither
    /// is defaulted past: a list that failed to parse is a list to look at,
    /// not to replace.
    pub fn load_or_seed() -> anyhow::Result<Self> {
        let path = launcher_path();
        if path.is_file() {
            return Self::load_from(&path);
        }
        let Some(legacy_path) = find_beside_exe_or_cwd(crate::config::CONFIG_FILE_NAME) else {
            return Ok(Self::default());
        };
        let legacy = LegacyConfig::load_from(&legacy_path)?;
        let seeded = Self::from_legacy(&legacy);
        match seeded.save_to(&path) {
            Ok(()) => println!("note: program list moved to {} (from {})", path.display(), legacy_path.display()),
            Err(err) => println!("note: could not write {}: {err:#}", path.display()),
        }
        Ok(seeded)
    }

    /// Loads the list from an explicit TOML file path.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Writes the list to [`launcher_path`].
    ///
    /// # Errors
    /// Returns an error if the folder can't be created or the file written.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&launcher_path())
    }

    /// Writes the list to an explicit TOML file path, atomically.
    ///
    /// # Errors
    /// Returns an error if the value can't be serialized, the parent folder
    /// can't be created, or either the write or the rename fails.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self).context("serializing launcher.toml")?;
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, path).with_context(|| format!("renaming {} into place", temp.display()))
    }

    /// Converts an older `config.toml` list, matching rows to the catalogue
    /// by exe name so `SimPro Manager` with `simpro3.exe` becomes `simpro`.
    #[must_use]
    pub fn from_legacy(legacy: &LegacyConfig) -> Self {
        let programs = legacy
            .programs
            .iter()
            .map(|program| {
                let matched = file_name(&program.path).and_then(|name| known_by_exe(&name));
                match matched {
                    Some(known) => ProgramEntry {
                        id: Some(known.id.to_owned()),
                        name: None,
                        enabled: true,
                        // The overlay's path is never saved: it is found
                        // beside the running binary every time.
                        path: (known.id != OVERLAY_ID).then(|| program.path.clone()),
                        args: program.args.clone(),
                    },
                    None => ProgramEntry {
                        id: None,
                        name: Some(program.name.clone()),
                        enabled: true,
                        path: Some(program.path.clone()),
                        args: program.args.clone(),
                    },
                }
            })
            .collect();
        Self { programs, start_with_overlay: false }
    }

    /// The full list to show and start: saved rows first, in their order,
    /// then every catalogue program not among them with its default tick.
    ///
    /// The overlay's own row goes last when it has never been placed, so
    /// its window comes up on top.
    #[must_use]
    pub fn rows(&self) -> Vec<ProgramEntry> {
        let mut rows = self.programs.clone();
        let listed: HashSet<&str> = rows.iter().filter_map(|row| row.id.as_deref()).collect();
        let mut unlisted: Vec<ProgramEntry> =
            CATALOGUE.iter().filter(|known| !listed.contains(known.id)).map(ProgramEntry::for_known).collect();
        // The overlay last: it is the last catalogue entry already, but a
        // custom row appended by the driver would otherwise follow it.
        let overlay_at = unlisted.iter().position(|row| row.id.as_deref() == Some(OVERLAY_ID));
        let overlay = overlay_at.map(|at| unlisted.remove(at));
        rows.extend(unlisted);
        rows.extend(overlay);
        rows
    }
}

/// Where each detected catalogue program is, by id.
pub type Detection = HashMap<String, PathBuf>;

/// Finds every catalogue program on this machine.
///
/// Known folders first, then Start Menu shortcuts, then the registry's
/// uninstall entries; the first existing file wins. Touches a few hundred
/// files and the registry, so callers with a frame to draw run it on a
/// worker thread.
#[must_use]
pub fn detect_all() -> Detection {
    let shortcuts = detect::start_menu_targets();
    let installs = detect::registry_install_folders();
    CATALOGUE
        .iter()
        .filter_map(|known| detect_one(known, &shortcuts, &installs).map(|path| (known.id.to_owned(), path)))
        .collect()
}

/// Finds one catalogue program, given the machine-wide indexes.
fn detect_one(known: &Known, shortcuts: &[PathBuf], installs: &[(String, Vec<PathBuf>)]) -> Option<PathBuf> {
    let from_folders =
        known.folders.iter().filter_map(|folder| expand_env(folder)).find_map(|dir| find_exe_in(&dir, known.exe));
    if from_folders.is_some() {
        return from_folders;
    }
    let from_shortcuts = shortcuts
        .iter()
        .find(|target| file_name(target).is_some_and(|name| exe_matches(known.exe, &name)) && target.is_file())
        .cloned();
    if from_shortcuts.is_some() {
        return from_shortcuts;
    }
    let wanted = known.name.to_lowercase();
    installs
        .iter()
        .filter(|(display, _)| display.to_lowercase().contains(&wanted))
        .flat_map(|(_, folders)| folders.iter())
        .find_map(|dir| find_exe_in(dir, known.exe))
}

/// The first file in `dir` whose name matches `pattern`, if any.
///
/// An exact name is a single `is_file` check; a pattern with `*` reads the
/// folder.
fn find_exe_in(dir: &Path, pattern: &str) -> Option<PathBuf> {
    if !pattern.contains('*') {
        let path = dir.join(pattern);
        return path.is_file().then_some(path);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .find(|path| file_name(path).is_some_and(|name| exe_matches(pattern, &name)))
}

/// Whether an exe file name matches a catalogue pattern, case-insensitively.
///
/// A single `*` matches anything; `VRS*.exe` matches `VRS Telemetry 2.exe`.
#[must_use]
pub fn exe_matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let name = name.to_lowercase();
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, suffix)) => {
            name.len() >= prefix.len() + suffix.len() && name.starts_with(prefix) && name.ends_with(suffix)
        }
    }
}

/// The catalogue entry whose exe name matches `name`, if any.
#[must_use]
pub fn known_by_exe(name: &str) -> Option<&'static Known> {
    CATALOGUE.iter().find(|known| exe_matches(known.exe, name))
}

/// Expands `%VAR%` references from the environment; `None` if one is unset.
///
/// `%EXE_DIR%` is the running binary's folder, so the overlay can be found
/// beside whichever binary is asking.
#[must_use]
pub fn expand_env(template: &str) -> Option<PathBuf> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        let var = &after[..end];
        let value = if var == "EXE_DIR" {
            std::env::current_exe().ok()?.parent()?.to_string_lossy().into_owned()
        } else {
            std::env::var(var).ok()?
        };
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(PathBuf::from(out))
}

/// Shortens a path for display by folding a standard folder back into its
/// `%VAR%`, so a row fits on one line.
#[must_use]
pub fn shorten_path(path: &Path) -> String {
    let shown = path.to_string_lossy();
    for var in ["LOCALAPPDATA", "APPDATA", "ProgramFiles(x86)", "ProgramFiles"] {
        if let Ok(value) = std::env::var(var)
            && !value.is_empty()
            && shown.to_lowercase().starts_with(&value.to_lowercase())
            && let Some(rest) = shown.get(value.len()..)
        {
            return format!("%{var}%{rest}");
        }
    }
    shown.into_owned()
}

/// A row with its path worked out, ready to show or start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// What the row is called.
    pub name: String,
    /// The executable, if one was found.
    pub path: Option<PathBuf>,
    /// The arguments it is started with.
    pub args: Vec<String>,
    /// How the path was arrived at.
    pub source: PathSource,
}

/// Where a resolved path came from, for the row to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    /// The path saved in the row, and it exists.
    Saved,
    /// Detection found it; nothing was saved.
    Found,
    /// The saved path is gone, but detection found it elsewhere.
    Moved,
    /// Nowhere.
    Missing,
}

/// Works out where a row's program is.
///
/// A saved path that exists wins. Otherwise a catalogue row falls back to
/// what [`detect_all`] found; the overlay's row is always found beside the
/// running binary.
#[must_use]
pub fn resolve(entry: &ProgramEntry, detected: &Detection) -> Resolved {
    let name = entry.display_name();
    let args = entry.effective_args();
    let saved = entry.path.as_deref().filter(|path| path.is_file()).map(Path::to_path_buf);
    if let Some(path) = saved {
        return Resolved { name, path: Some(path), args, source: PathSource::Saved };
    }
    let found = entry.id.as_deref().and_then(|id| detected.get(id)).cloned();
    let source = match (&found, &entry.path) {
        (Some(_), Some(_)) => PathSource::Moved,
        (Some(_), None) => PathSource::Found,
        (None, _) => PathSource::Missing,
    };
    Resolved { name, path: found, args, source }
}

/// Starts every enabled row that isn't already running, reporting each.
///
/// What a double-click on the launcher does, and what the overlay does at
/// its own startup when [`LauncherConfig::start_with_overlay`] is set. The
/// machine is scanned only if some enabled row has no usable path of its
/// own; on a settled install every row does.
#[must_use]
pub fn start_enabled(config: &LauncherConfig) -> Vec<(String, Started)> {
    let rows = config.rows();
    let needs_detection = rows.iter().any(|row| row.enabled && !row.path.as_deref().is_some_and(Path::is_file));
    let detected = if needs_detection { detect_all() } else { Detection::new() };
    let running = running_exe_names();
    rows.iter()
        .filter(|row| row.enabled)
        .map(|row| {
            let program = resolve(row, &detected);
            let outcome = start(&program, &running);
            (program.name, outcome)
        })
        .collect()
}

/// Starts the enabled rows on a background thread if the list asks for it.
///
/// For the overlay's own startup: nothing waits on it, and a list that can't
/// be read is a note on the console rather than a reason not to draw. When
/// the overlay was itself started by the launcher, every row comes back as
/// already running, which is the right answer and prints nothing.
pub fn start_with_overlay_if_set() {
    std::thread::spawn(|| {
        let config = match LauncherConfig::load_or_seed() {
            Ok(config) => config,
            Err(err) => {
                println!("note: {err:#}; not starting the session programs");
                return;
            }
        };
        if !config.start_with_overlay {
            return;
        }
        for (name, outcome) in start_enabled(&config) {
            match outcome {
                Started::Started => println!("[start] {name}"),
                Started::AlreadyRunning => {}
                Started::NotFound => println!("[??]    {name}: not found"),
                Started::Failed(reason) => println!("[fail]  {name}: {reason}"),
            }
        }
    });
}

/// The lowercase file names of every running process.
#[must_use]
pub fn running_exe_names() -> HashSet<String> {
    let system = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    system.processes().values().map(|process| process.name().to_string_lossy().to_lowercase()).collect()
}

/// What starting a program came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// Spawned.
    Started,
    /// Its exe name was already among the running processes.
    AlreadyRunning,
    /// No executable to start.
    NotFound,
    /// The spawn failed; the text is the OS's reason.
    Failed(String),
}

/// Starts a resolved program unless it is running already or missing.
///
/// The working directory is the executable's own folder — some programs
/// (OBS) find their files relative to it.
#[must_use]
pub fn start<S: std::hash::BuildHasher>(program: &Resolved, running: &HashSet<String, S>) -> Started {
    let Some(path) = &program.path else {
        return Started::NotFound;
    };
    if file_name(path).is_some_and(|name| running.contains(&name.to_lowercase())) {
        return Started::AlreadyRunning;
    }
    let mut command = Command::new(path);
    command.args(&program.args);
    if let Some(dir) = path.parent() {
        command.current_dir(dir);
    }
    match command.spawn() {
        Ok(_child) => Started::Started,
        Err(err) => Started::Failed(err.to_string()),
    }
}

/// The file name of a path, if it has one.
fn file_name(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().into_owned())
}

/// The file stem of a path, if it has one.
fn file_stem(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_ids_are_unique_and_stable_looking() {
        let mut seen = HashSet::new();
        for known in CATALOGUE {
            assert!(seen.insert(known.id), "duplicate id {}", known.id);
            assert!(known.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{}", known.id);
        }
        assert_eq!(CATALOGUE.last().map(|known| known.id), Some(OVERLAY_ID));
    }

    #[test]
    fn exe_patterns_match_case_insensitively_with_one_star() {
        assert!(exe_matches("iRacingUI.exe", "iracingui.exe"));
        assert!(exe_matches("VRS*.exe", "VRS Telemetry 2.exe"));
        assert!(exe_matches("VRS*.exe", "vrs.exe"));
        assert!(!exe_matches("VRS*.exe", "vr.exe"));
        assert!(!exe_matches("Kapps.exe", "kapps-updater.exe"));
    }

    #[test]
    fn expands_environment_variables_and_refuses_unset_ones() {
        // SAFETY: tests in this module run single-threaded with respect to
        // this variable; nothing else reads it.
        unsafe { std::env::set_var("RACE_TEST_DIR", r"C:\Tools") };
        assert_eq!(expand_env(r"%RACE_TEST_DIR%\app"), Some(PathBuf::from(r"C:\Tools\app")));
        assert_eq!(expand_env(r"%RACE_TEST_UNSET_VARIABLE%\app"), None);
        assert_eq!(expand_env(r"C:\plain"), Some(PathBuf::from(r"C:\plain")));
    }

    #[test]
    fn a_legacy_list_maps_to_catalogue_rows_by_exe_name() {
        let legacy: LegacyConfig = toml::from_str(
            r#"
            [[programs]]
            name = "SimPro Manager"
            path = 'C:\Program Files (x86)\Simagic\Simpro3\bin\simpro3.exe'

            [[programs]]
            name = "My deck"
            path = 'C:\Tools\deck.exe'
            args = ["--profile", "race"]

            [[programs]]
            name = "Race Overlay"
            path = 'C:\repo\target\release\race-overlay.exe'
            "#,
        )
        .expect("legacy config must parse");
        let seeded = LauncherConfig::from_legacy(&legacy);
        assert_eq!(seeded.programs[0].id.as_deref(), Some("simpro"));
        assert!(seeded.programs[0].path.is_some());
        assert_eq!(seeded.programs[1].id, None);
        assert_eq!(seeded.programs[1].name.as_deref(), Some("My deck"));
        assert_eq!(seeded.programs[1].args, vec!["--profile", "race"]);
        assert_eq!(seeded.programs[2].id.as_deref(), Some(OVERLAY_ID));
        assert_eq!(seeded.programs[2].path, None, "the overlay is found beside the binary, never saved");
    }

    #[test]
    fn rows_append_the_rest_of_the_catalogue_with_the_overlay_last() {
        let config = LauncherConfig {
            programs: vec![ProgramEntry { id: None, name: Some("Mine".to_owned()), ..ProgramEntry::default() }],
            start_with_overlay: false,
        };
        let rows = config.rows();
        assert_eq!(rows.len(), CATALOGUE.len() + 1);
        assert_eq!(rows[0].name.as_deref(), Some("Mine"));
        assert_eq!(rows.last().and_then(|row| row.id.as_deref()), Some(OVERLAY_ID));
        let overlay_rows = rows.iter().filter(|row| row.id.as_deref() == Some(OVERLAY_ID)).count();
        assert_eq!(overlay_rows, 1);
    }

    #[test]
    fn round_trips_through_toml_without_writing_empty_fields() {
        let config = LauncherConfig {
            programs: vec![
                ProgramEntry { id: Some("garage61".to_owned()), enabled: false, ..ProgramEntry::default() },
                ProgramEntry {
                    id: None,
                    name: Some("Deck".to_owned()),
                    enabled: true,
                    path: Some(PathBuf::from(r"C:\Tools\deck.exe")),
                    args: vec!["--x".to_owned()],
                },
            ],
            start_with_overlay: true,
        };
        let text = toml::to_string(&config).expect("must serialize");
        assert!(!text.contains("args = []"));
        assert!(!text.contains("name = \"\""));
        let parsed: LauncherConfig = toml::from_str(&text).expect("must parse");
        assert_eq!(parsed, config);
    }

    #[test]
    fn a_saved_path_that_exists_beats_detection() {
        let here = std::env::current_exe().expect("tests have an exe");
        let entry = ProgramEntry { id: Some("kapps".to_owned()), path: Some(here.clone()), ..ProgramEntry::default() };
        let mut detected = Detection::new();
        detected.insert("kapps".to_owned(), PathBuf::from(r"C:\elsewhere\Kapps.exe"));
        let resolved = resolve(&entry, &detected);
        assert_eq!(resolved.path, Some(here));
        assert_eq!(resolved.source, PathSource::Saved);

        let gone = ProgramEntry { path: Some(PathBuf::from(r"C:\gone\Kapps.exe")), ..entry };
        let resolved = resolve(&gone, &detected);
        assert_eq!(resolved.source, PathSource::Moved);
        assert_eq!(resolved.path, Some(PathBuf::from(r"C:\elsewhere\Kapps.exe")));
    }
}
