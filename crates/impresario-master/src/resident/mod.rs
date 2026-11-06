//! Resident runner: scheduler, watchers and the service CLI (TASK.md section 8).

pub mod cmd;
pub mod config;
pub mod notify;
pub mod service;

pub use cmd::{
    run_notify, run_schedule, run_service, run_watch, NotifyArgs, ScheduleArgs, ServiceArgs,
    WatchArgs,
};
