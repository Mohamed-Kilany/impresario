//! Read-only resident status shared by the TUI and the MCP server.
//!
//! The resident runner persists runtime counters in `service/state.json` and
//! the schedule/watcher definitions in the layered configuration. Both
//! clients need the same joined view; keeping it here avoids one client
//! drifting from the other and keeps every read side-effect free.

use std::path::{Path, PathBuf};

use chrono::Local;
use impresario_core::home::Home;
use impresario_core::resident::ScheduleWhen;
use serde::Serialize;

use super::config;
use super::service::{self, ServiceState};

/// The effective resident view for one workspace.
#[derive(Debug, Clone, Serialize)]
pub struct ResidentStatus {
    pub running: bool,
    pub state: ServiceState,
    pub schedules: Vec<ScheduleStatus>,
    pub watchers: Vec<WatcherStatus>,
}

/// One configured schedule joined with its persisted counters.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleStatus {
    pub name: String,
    pub enabled: bool,
    pub when: String,
    pub next_fire: Option<String>,
    pub last_fire: Option<String>,
    pub runs: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
}

/// One configured watcher joined with its persisted counters.
#[derive(Debug, Clone, Serialize)]
pub struct WatcherStatus {
    pub name: String,
    pub enabled: bool,
    pub paused: bool,
    pub iterations: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub last_started: Option<String>,
    pub last_status: Option<String>,
    pub last_dump: Option<String>,
    pub next_after: Option<String>,
}

/// A timestamped watcher dump directory.
#[derive(Debug, Clone, Serialize)]
pub struct DumpEntry {
    pub watcher: String,
    pub name: String,
    /// Workspace-relative, slash-separated path.
    pub path: String,
}

/// Join the effective config with the persisted service state.
pub fn collect(home: &Home, config_path: &Path) -> Result<ResidentStatus, String> {
    let state = service::read_state(home)?;
    let sections = config::load_effective(home, config_path)?;
    let running = state.pid != 0 && crate::jobs::pid_alive(state.pid);
    let now = Local::now();

    let schedules = sections
        .schedules
        .iter()
        .map(|schedule| {
            let runtime = state
                .schedules
                .get(&schedule.name)
                .cloned()
                .unwrap_or_default();
            let next_fire = if schedule.enabled {
                schedule
                    .next_after(&now)
                    .ok()
                    .flatten()
                    .map(|fire| service::display_local(&fire.to_rfc3339()))
            } else {
                None
            };
            ScheduleStatus {
                name: schedule.name.clone(),
                enabled: schedule.enabled,
                when: when_label(&schedule.when),
                next_fire,
                last_fire: runtime.last_fire.as_deref().map(service::display_local),
                runs: runtime.runs,
                failures: runtime.failures,
                consecutive_failures: runtime.consecutive_failures,
            }
        })
        .collect();

    let watchers = sections
        .watchers
        .iter()
        .map(|watcher| {
            let runtime = state
                .watchers
                .get(&watcher.name)
                .cloned()
                .unwrap_or_default();
            WatcherStatus {
                name: watcher.name.clone(),
                enabled: watcher.enabled,
                paused: service::watch_paused(home, &watcher.name),
                iterations: runtime.iterations,
                failures: runtime.failures,
                consecutive_failures: runtime.consecutive_failures,
                last_started: runtime.last_started.as_deref().map(service::display_local),
                last_status: runtime.last_status,
                last_dump: runtime.last_dump,
                next_after: runtime.next_after.as_deref().map(service::display_local),
            }
        })
        .collect();

    Ok(ResidentStatus {
        running,
        state,
        schedules,
        watchers,
    })
}

/// List timestamped dump directories under `reports/watch/`.
///
/// A missing watcher directory is an empty result, not an error: a newly
/// configured watcher has no dumps yet, and a deleted watcher may still have
/// old evidence on disk.
pub fn dumps(base: &Path, watcher: Option<&str>) -> Result<Vec<DumpEntry>, String> {
    let root = base.join("reports").join("watch");
    let watchers: Vec<String> = match watcher {
        Some(name) => vec![name.to_string()],
        None => match std::fs::read_dir(&root) {
            Ok(read) => {
                let mut names: Vec<String> = read
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!("cannot read {}: {error}", root.display()));
            }
        },
    };

    let mut dumps = Vec::new();
    for name in watchers {
        let dir = root.join(&name);
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot read {}: {error}", dir.display())),
        };
        let mut entries: Vec<PathBuf> = read
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        entries.sort();
        for path in entries {
            dumps.push(DumpEntry {
                watcher: name.clone(),
                name: path
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: relative_display(base, &path),
            });
        }
    }
    Ok(dumps)
}

fn relative_display(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A compact human label for a schedule's `when` block.
#[must_use]
pub fn when_label(when: &ScheduleWhen) -> String {
    match when {
        ScheduleWhen::Once { at } => format!("once {at}"),
        ScheduleWhen::Daily { at } => format!("daily {at}"),
        ScheduleWhen::Weekly { days, at } => format!("weekly {} {at}", days.join(",")),
        ScheduleWhen::Interval { every } => format!("every {every}"),
        ScheduleWhen::Cron { expr } => format!("cron {expr}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::resident::ResidentSections;

    #[test]
    fn when_labels_match_the_cli_spelling() {
        let sections = ResidentSections::from_yaml(
            r#"
schedules:
  - name: once
    when: { kind: once, at: "2026-11-06T09:00:00" }
  - name: weekly
    when: { kind: weekly, days: [mon, fri], at: "07:30" }
  - name: cron
    when: { kind: cron, expr: "*/5 * * * *" }
"#,
        )
        .expect("valid schedules");
        let labels: Vec<String> = sections
            .schedules
            .iter()
            .map(|schedule| when_label(&schedule.when))
            .collect();
        assert_eq!(labels[0], "once 2026-11-06T09:00:00");
        assert_eq!(labels[1], "weekly mon,fri 07:30");
        assert_eq!(labels[2], "cron */5 * * * *");
    }

    #[test]
    fn dump_paths_are_workspace_relative_and_ordered() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("reports/watch/soak");
        std::fs::create_dir_all(root.join("20261106-120000")).expect("first dump");
        std::fs::create_dir_all(root.join("20261106-130000")).expect("second dump");
        let listed = dumps(temp.path(), Some("soak")).expect("list dumps");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "20261106-120000");
        assert_eq!(listed[1].name, "20261106-130000");
        assert_eq!(listed[0].path, "reports/watch/soak/20261106-120000");
        assert!(dumps(temp.path(), None).is_ok());
        assert!(dumps(temp.path(), Some("missing"))
            .expect("missing is empty")
            .is_empty());
    }

    #[test]
    fn collect_joins_config_and_persisted_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = Home::at(temp.path().join("home"));
        let config_path = temp.path().join("impresario.config");
        std::fs::create_dir_all(home.service_dir()).expect("service dir");
        std::fs::write(
            &config_path,
            "schedules:\n  - name: nightly\n    when: { kind: daily, at: \"02:30\" }\n    run: { plays: [smoke.play] }\nwatchers:\n  - name: soak\n    run: { plays: [smoke.play] }\n",
        )
        .expect("workspace config");
        std::fs::write(
            home.service_state_path(),
            serde_json::json!({
                "version": 1,
                "pid": 0,
                "paused": false,
                "schedules": {
                    "nightly": { "runs": 3, "failures": 1, "consecutive_failures": 1 }
                },
                "watchers": {
                    "soak": {
                        "iterations": 4,
                        "failures": 2,
                        "consecutive_failures": 2,
                        "last_status": "failed"
                    }
                }
            })
            .to_string(),
        )
        .expect("state");

        let status = collect(&home, &config_path).expect("collect");
        assert!(!status.running);
        assert_eq!(status.schedules.len(), 1);
        assert_eq!(status.schedules[0].name, "nightly");
        assert_eq!(status.schedules[0].runs, 3);
        assert!(status.schedules[0].next_fire.is_some());
        assert_eq!(status.watchers.len(), 1);
        assert_eq!(status.watchers[0].iterations, 4);
        assert_eq!(status.watchers[0].last_status.as_deref(), Some("failed"));
    }
}
