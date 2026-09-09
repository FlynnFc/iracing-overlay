// Rust guideline compliant 2026-02-16

//! Telemetry acquisition: connecting to iRacing, reading vars, and turning
//! them into the owned snapshots the UI renders.

pub mod endurance;
pub mod faster_class;
pub mod irating;
pub mod net_position;
pub mod pit;
pub mod pit_model;
pub mod pit_window;
pub mod race_plan;
pub mod radar;
pub mod relative;
pub mod session;
pub mod session_info;
pub mod snapshot;
pub mod sof;
pub mod standings;
pub mod weather;
