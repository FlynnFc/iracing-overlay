// Rust guideline compliant 2026-02-16

//! Shared pieces of the race toolkit.
//!
//! [`launcher`] is the session launcher's program list — what to start,
//! where each program is on this machine, and how to start it — used by
//! `race-launcher.exe` to do the starting and by `race-overlay.exe`'s
//! settings window to edit the list. [`config`] holds the older `config.toml`
//! shape, kept so an existing list is carried over once, plus the
//! beside-the-exe file lookup both binaries use.

pub mod config;
pub mod launcher;
