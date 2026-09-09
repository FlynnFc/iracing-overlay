// Rust guideline compliant 2026-02-16

//! The Launcher page of the settings window: `race-launcher.exe`'s program
//! list as rows you tick, reorder and start, instead of a TOML you edit.
//!
//! The list is `race_tools::launcher`'s: the built-in catalogue plus the
//! driver's own additions, read from `%APPDATA%\race\launcher.toml` the
//! first time the page is drawn and written back — by the app, on settle —
//! after any change. Detection of where each program lives runs on a
//! worker thread when the page opens and on **Rescan**; the file picker
//! behind **Browse…** and **Add a program…** blocks on a modal dialog, so it
//! runs on a thread of its own too. Both hand their answer back through a
//! channel the page polls each frame.
//!
//! Unlike the rest of the settings window this page does touch the disk
//! and start processes, because that is what it is for; see
//! `plans/launcher-settings.md`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use race_tools::launcher::{
    self, Detection, LauncherConfig, OVERLAY_ID, PathSource, ProgramEntry, Started, detect, launcher_path,
};

use super::CAUTION;
use super::settings::Outcome;

/// How often the running-process list is refreshed while the page is open.
/// A process scan a second is cheap; a **Start** button that stays pressable
/// for a second after the program came up is not worth more.
const RUNNING_REFRESH: Duration = Duration::from_secs(1);

/// How long a failed start's reason stays under its row.
const ERROR_SHOWN_FOR: Duration = Duration::from_secs(10);

/// The height the row list scrolls inside, leaving room for the buttons
/// above and below it within the window's fixed page height.
const LIST_HEIGHT: f32 = 248.0;

/// Width of the name box on a custom row, which is editable.
const NAME_EDIT_WIDTH: f32 = 150.0;

/// Which row a running file picker is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Browse {
    /// Replace this row's path.
    Row(usize),
    /// Add a new custom row.
    New,
}

/// The page's own state, kept between frames.
#[derive(Debug, Default)]
pub struct LauncherPage {
    /// Whether the list has been read from disk yet.
    loaded: bool,
    /// Why the list couldn't be read, shown at the top of the page.
    load_error: Option<String>,
    /// The rows, in start order — what `launcher.toml` will hold.
    rows: Vec<ProgramEntry>,
    /// Whether the overlay starts the ticked rows itself at startup.
    start_with_overlay: bool,
    /// Where detection last found each catalogue program.
    detected: Detection,
    /// A detection pass in progress.
    detecting: Option<Receiver<Detection>>,
    /// Lowercase exe names of running processes, as of `running_at`.
    running: HashSet<String>,
    running_at: Option<Instant>,
    /// A file picker in progress, and what it is for.
    browse: Option<(Browse, Receiver<Option<PathBuf>>)>,
    /// Rows changed since the last write.
    dirty: bool,
    /// Failed starts to show: row index, reason, when.
    errors: Vec<(usize, String, Instant)>,
}

impl LauncherPage {
    /// Reads the list on the first frame the page is shown, and starts
    /// looking for the programs in it.
    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        match LauncherConfig::load_or_seed() {
            Ok(config) => {
                self.rows = config.rows();
                self.start_with_overlay = config.start_with_overlay;
            }
            Err(err) => {
                self.load_error = Some(format!("{err:#}"));
                self.rows = LauncherConfig::default().rows();
            }
        }
        self.rescan();
    }

    /// Starts a detection pass on a worker thread.
    fn rescan(&mut self) {
        if self.detecting.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(launcher::detect_all());
        });
        self.detecting = Some(rx);
    }

    /// Collects finished background work and refreshes the running list.
    fn poll(&mut self, now: Instant) {
        if let Some(rx) = &self.detecting
            && let Ok(detected) = rx.try_recv()
        {
            self.detected = detected;
            self.detecting = None;
        }
        if let Some((target, rx)) = &self.browse
            && let Ok(picked) = rx.try_recv()
        {
            let target = *target;
            self.browse = None;
            if let Some(path) = picked {
                self.apply_pick(target, path);
            }
        }
        if self.running_at.is_none_or(|at| now.duration_since(at) >= RUNNING_REFRESH) {
            self.running = launcher::running_exe_names();
            self.running_at = Some(now);
        }
        self.errors.retain(|(_, _, at)| now.duration_since(*at) < ERROR_SHOWN_FOR);
    }

    /// Puts a picked file where the picker was opened for.
    fn apply_pick(&mut self, target: Browse, path: PathBuf) {
        match target {
            Browse::Row(index) => {
                if let Some(row) = self.rows.get_mut(index) {
                    row.path = Some(path);
                    row.enabled = true;
                }
            }
            Browse::New => {
                let name = detect::file_description(&path)
                    .or_else(|| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()));
                let row = ProgramEntry { id: None, name, enabled: true, path: Some(path), args: Vec::new() };
                // Before the overlay's row, so the overlay stays last.
                let at =
                    self.rows.iter().position(|row| row.id.as_deref() == Some(OVERLAY_ID)).unwrap_or(self.rows.len());
                self.rows.insert(at, row);
            }
        }
        self.dirty = true;
    }

    /// Opens the file picker on a worker thread.
    fn browse(&mut self, target: Browse, initial: Option<PathBuf>) {
        if self.browse.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(detect::pick_executable(initial.as_deref()));
        });
        self.browse = Some((target, rx));
    }

    /// Starts one row now, noting a failure under it.
    fn start_row(&mut self, index: usize, now: Instant) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let program = launcher::resolve(row, &self.detected);
        match launcher::start(&program, &self.running) {
            Started::Started => self.running_at = None,
            Started::AlreadyRunning | Started::NotFound => {}
            Started::Failed(reason) => self.errors.push((index, reason, now)),
        }
    }

    /// What a double-click on the launcher does, minus its console window.
    fn start_all(&mut self, now: Instant) {
        for index in 0..self.rows.len() {
            if self.rows[index].enabled {
                self.start_row(index, now);
            }
        }
    }

    /// Writes the list if anything changed since the last write.
    ///
    /// Called by the app once the settings have settled, alongside its own
    /// file. A failed write is a note on the console and stays dirty, so it
    /// is tried again on the next settle.
    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        let config = LauncherConfig { programs: self.rows.clone(), start_with_overlay: self.start_with_overlay };
        match config.save() {
            Ok(()) => self.dirty = false,
            Err(err) => println!("note: could not save {}: {err:#}", launcher_path().display()),
        }
    }
}

/// What the rows asked for this frame, applied once they are all drawn —
/// a row can't be reordered or removed from inside its own loop.
#[derive(Debug, Default)]
struct RowActions {
    swap: Option<(usize, usize)>,
    remove: Option<usize>,
    start: Option<usize>,
    browse: Option<(usize, Option<PathBuf>)>,
}

/// Draws the page, applying every change straight to the page's rows.
pub fn draw(ui: &mut Ui, page: &mut LauncherPage, now: Instant) -> Outcome {
    page.ensure_loaded();
    page.poll(now);
    let mut changed = draw_header(ui, page, now);

    let mut actions = RowActions::default();
    egui::ScrollArea::vertical().max_height(LIST_HEIGHT).show(ui, |ui| {
        let count = page.rows.len();
        for index in 0..count {
            changed |= draw_row(ui, page, index, count, &mut actions);
        }
    });

    if let Some((from, to)) = actions.swap {
        page.rows.swap(from, to);
        page.dirty = true;
        changed = true;
    }
    if let Some(index) = actions.remove
        && index < page.rows.len()
    {
        page.rows.remove(index);
        page.dirty = true;
        changed = true;
    }
    if let Some(index) = actions.start {
        page.start_row(index, now);
    }
    if let Some((index, initial)) = actions.browse {
        page.browse(Browse::Row(index), initial);
    }

    changed |= draw_footer(ui, page);
    Outcome { changed, ..Outcome::default() }
}

/// The notice, title line and its buttons, and the start-with-overlay tick.
fn draw_header(ui: &mut Ui, page: &mut LauncherPage, now: Instant) -> bool {
    let mut changed = false;
    if let Some(err) = &page.load_error {
        ui.colored_label(CAUTION, format!("Could not read {}: {err}", launcher_path().display()));
        ui.label(RichText::new("Showing the built-in list. Nothing is written until you change something.").small());
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        ui.label("Programs race-launcher.exe starts, in this order.");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Start all now")
                .on_hover_text("Starts every ticked program that isn't already running.")
                .clicked()
            {
                page.start_all(now);
            }
            if ui.add_enabled(page.detecting.is_none(), egui::Button::new("Rescan")).clicked() {
                page.rescan();
            }
            if page.detecting.is_some() {
                ui.spinner();
            }
        });
    });
    if ui
        .checkbox(&mut page.start_with_overlay, "Also start the ticked programs when the overlay starts")
        .on_hover_text(
            "For starting race-overlay.exe on its own rather than through race-launcher.exe. \
             Programs already running are skipped, so it is harmless either way.",
        )
        .changed()
    {
        page.dirty = true;
        changed = true;
    }
    ui.add_space(4.0);
    changed
}

/// One row: tick, name, note and buttons on the first line; the path (and
/// a custom row's arguments) on the second; any failed start under those.
fn draw_row(ui: &mut Ui, page: &mut LauncherPage, index: usize, count: usize, actions: &mut RowActions) -> bool {
    let mut changed = false;
    let program = launcher::resolve(&page.rows[index], &page.detected);
    let is_overlay = page.rows[index].id.as_deref() == Some(OVERLAY_ID);
    let is_custom = page.rows[index].id.is_none();
    let found = program.path.is_some();
    let running = program
        .path
        .as_deref()
        .and_then(std::path::Path::file_name)
        .is_some_and(|name| page.running.contains(&name.to_string_lossy().to_lowercase()));

    ui.horizontal(|ui| {
        let row = &mut page.rows[index];
        if ui.checkbox(&mut row.enabled, "").changed() {
            // Ticking a detected row pins the path the launcher will use, so
            // it needn't scan the machine every start.
            if row.enabled && row.path.is_none() && program.source == PathSource::Found {
                row.path.clone_from(&program.path);
            }
            page.dirty = true;
            changed = true;
        }
        if is_custom {
            let mut name = row.name.clone().unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(NAME_EDIT_WIDTH)).changed() {
                row.name = Some(name);
                page.dirty = true;
                changed = true;
            }
        } else {
            ui.strong(&program.name);
        }
        let note = if is_overlay && running { "This overlay (running)" } else { row.note() };
        ui.label(RichText::new(note).weak());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(index + 1 < count, egui::Button::new("\u{25BC}")).clicked() {
                actions.swap = Some((index, index + 1));
            }
            if ui.add_enabled(index > 0, egui::Button::new("\u{25B2}")).clicked() {
                actions.swap = Some((index, index - 1));
            }
            if is_custom && ui.button("Remove").clicked() {
                actions.remove = Some(index);
            }
            if !is_overlay && ui.add_enabled(page.browse.is_none(), egui::Button::new("Browse\u{2026}")).clicked() {
                actions.browse = Some((index, program.path.clone()));
            }
            if !is_overlay {
                let label = if running { "Running" } else { "Start" };
                if ui.add_enabled(found && !running, egui::Button::new(label)).clicked() {
                    actions.start = Some(index);
                }
            }
        });
    });

    // The path line, with the full path on hover where it was shortened; a
    // custom row also gets its arguments to edit.
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        match (&program.path, program.source) {
            (Some(path), PathSource::Moved) => {
                ui.colored_label(CAUTION, "moved \u{2014} found at")
                    .on_hover_text("The saved path no longer exists; this one does.");
                ui.label(RichText::new(launcher::shorten_path(path)).small()).on_hover_text(path.display().to_string());
            }
            (Some(path), _) => {
                ui.label(RichText::new(launcher::shorten_path(path)).small()).on_hover_text(path.display().to_string());
            }
            (None, _) => {
                ui.label(RichText::new("Not found").small().weak()).on_hover_text(
                    "Not in its usual folder, the Start Menu or the registry. Browse\u{2026} to point at it.",
                );
            }
        }
        if is_custom {
            let row = &mut page.rows[index];
            let mut args = row.args.join(" ");
            ui.label(RichText::new("args").small().weak());
            if ui.add(egui::TextEdit::singleline(&mut args).desired_width(120.0)).changed() {
                row.args = args.split_whitespace().map(str::to_owned).collect();
                page.dirty = true;
                changed = true;
            }
        }
    });
    for (_, reason, _) in page.errors.iter().filter(|(at, _, _)| *at == index) {
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            ui.colored_label(CAUTION, format!("could not start: {reason}"));
        });
    }
    ui.add_space(2.0);
    changed
}

/// **Add a program…**, **Reset to defaults**, and where the list lives.
fn draw_footer(ui: &mut Ui, page: &mut LauncherPage) -> bool {
    let mut changed = false;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(page.browse.is_none(), egui::Button::new("Add a program\u{2026}")).clicked() {
            page.browse(Browse::New, None);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Reset to defaults")
                .on_hover_text(
                    "Forgets every path and tick and looks for the programs again, as a fresh install would.",
                )
                .clicked()
            {
                page.rows = LauncherConfig::default().rows();
                page.start_with_overlay = false;
                page.dirty = true;
                page.rescan();
                changed = true;
            }
        });
    });
    ui.add_space(4.0);
    ui.small(format!("List: {}", launcher_path().display()));
    changed
}
