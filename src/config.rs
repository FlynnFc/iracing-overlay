// Rust guideline compliant 2026-02-16

//! The older `config.toml` beside the launcher, plus the beside-the-exe file
//! lookup reused by `race-overlay`.
//!
//! `config.toml` was the launcher's program list until the list moved to
//! `%APPDATA%\race\launcher.toml` (see [`crate::launcher`]). It is still read
//! once, when no `launcher.toml` exists yet, so a hand-written list carries
//! over; after that it is ignored.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

/// File name the launcher used to look for next to the executable.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The older launcher configuration, read only to seed `launcher.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Programs the launcher starts, in order.
    #[serde(default)]
    pub programs: Vec<Program>,
}

/// One program entry in the older `config.toml`.
#[derive(Debug, Deserialize)]
pub struct Program {
    /// Display name used in launcher output.
    pub name: String,
    /// Full path to the executable.
    pub path: PathBuf,
    /// Extra command-line arguments, if any.
    #[serde(default)]
    pub args: Vec<String>,
}

impl Config {
    /// Loads `config.toml` from the executable's folder, falling back to the working directory.
    ///
    /// # Errors
    /// Returns an error if no `config.toml` is found in either location, or
    /// if the file exists but is not valid TOML for this schema.
    pub fn load() -> anyhow::Result<Self> {
        let path = find_beside_exe_or_cwd(CONFIG_FILE_NAME)
            .context("no config.toml found next to the executable or in the working directory")?;
        Self::load_from(&path)
    }

    /// Loads configuration from an explicit TOML file path.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}

/// Returns the first existing file named `name`: next to the exe, then in the working directory.
#[must_use]
pub fn find_beside_exe_or_cwd(name: &str) -> Option<PathBuf> {
    let beside_exe = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(name)));
    let in_cwd = std::env::current_dir().ok().map(|cwd| cwd.join(name));
    [beside_exe, in_cwd].into_iter().flatten().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg: Config = toml::from_str(
            r#"
            [[programs]]
            name = "iRacing UI"
            path = 'C:\iRacing\ui\iRacingUI.exe'
            "#,
        )
        .expect("valid config must parse");

        assert_eq!(cfg.programs.len(), 1);
        assert_eq!(cfg.programs[0].name, "iRacing UI");
    }

    #[test]
    fn empty_config_uses_defaults() {
        let cfg: Config = toml::from_str("").expect("empty config must parse");
        assert!(cfg.programs.is_empty());
    }
}
