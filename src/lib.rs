//! CronManager-on-Rust — YAML-driven cron scheduler.
//!
//! See [`docs/DESIGN.md`](https://github.com/Buerostack/CronManager-on-Rust/blob/dev/docs/DESIGN.md)
//! for the domain design.

pub mod config;
pub mod dsl;
pub mod error;
pub mod executor;
pub mod history;
pub mod router;
pub mod scheduler;
