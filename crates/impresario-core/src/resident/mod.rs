//! Resident runner configuration and schedule arithmetic (TASK.md section 8).

pub mod model;
pub mod schedule;

pub use model::{
    BusyPolicy, DeviceHealth, DumpKind, FailureClass, FailurePolicy, MissedPolicy, NotifyEvent,
    NotifySpec, Pacing, ResidentSections, Retention, RunSelection, Schedule, ScheduleWhen,
    Throttle, Watcher, WatcherPriority,
};
pub use schedule::{
    next_after, next_fires, parse_cron, parse_hhmm, parse_once, parse_span, parse_weekday,
    validate_cron, validate_when, CronExpr,
};
