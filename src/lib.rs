//! CronManager — YAML-driven cron scheduler.
//!
//! See [`docs/DESIGN.md`](https://github.com/turnerrainer/cronmanager/blob/dev/docs/DESIGN.md)
//! for the domain design.

pub mod config;
pub mod dsl;
pub mod error;
pub mod executor;
pub mod history;
pub mod router;
pub mod scheduler;
