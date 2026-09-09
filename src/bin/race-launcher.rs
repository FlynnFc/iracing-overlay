// Rust guideline compliant 2026-02-16

//! Starts every program needed for an iRacing session with one double-click.
//!
//! Reads the program list from `%APPDATA%\race\launcher.toml` — the one the
//! overlay's **Launcher** settings page edits — seeding it once from an
//! older `config.toml` beside this exe if there is no list yet. Programs the
//! list doesn't place a path on are found on this machine the same way the
//! settings page finds them (see `race_tools::launcher::detect_all`). Rows
//! that are switched off, already running or nowhere to be found are
//! skipped, and each is reported on its own line.

use std::time::Duration;

use race_tools::launcher::{
    Detection, LauncherConfig, PathSource, Started, detect_all, resolve, running_exe_names, start,
};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Long enough to read the status lines before the window closes.
const LINGER: Duration = Duration::from_secs(5);

fn main() {
    let dry_run = std::env::args().any(|arg| arg == "--dry-run");
    if let Err(err) = run(dry_run) {
        eprintln!("error: {err:#}");
    }
    if !dry_run {
        std::thread::sleep(LINGER);
    }
}

fn run(dry_run: bool) -> anyhow::Result<()> {
    let config = LauncherConfig::load_or_seed()?;
    let rows = config.rows();
    // Only worth scanning the machine if some enabled row has no usable
    // path of its own; on a settled install every row does.
    let needs_detection =
        rows.iter().any(|row| row.enabled && !row.path.as_deref().is_some_and(std::path::Path::is_file));
    let detected = if needs_detection { detect_all() } else { Detection::new() };
    let running = running_exe_names();

    for row in &rows {
        let program = resolve(row, &detected);
        if !row.enabled {
            println!("[off]   {}", program.name);
            continue;
        }
        let Some(path) = &program.path else {
            println!("[??]    {}: not found", program.name);
            continue;
        };
        if program.source == PathSource::Moved {
            println!("[moved] {} -> {}", program.name, path.display());
        }
        if dry_run {
            println!("[would] {} -> {}", program.name, path.display());
            continue;
        }
        match start(&program, &running) {
            Started::Started => println!("[start] {}", program.name),
            Started::AlreadyRunning => println!("[skip]  {} (already running)", program.name),
            Started::NotFound => println!("[??]    {}: not found", program.name),
            Started::Failed(reason) => println!("[fail]  {}: {reason}", program.name),
        }
    }

    Ok(())
}
