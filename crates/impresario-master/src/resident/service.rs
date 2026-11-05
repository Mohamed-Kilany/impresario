//! The resident runner process, its crash-safe state and its autostart hooks.
//!
//! The TASK-2 job supervisor stays the per-job execution path. This module only
//! decides when to submit work and records the resident lifetime; every fire
//! therefore remains a normal job with the usual report, logs and leases.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone as _};
use impresario_core::command::WireErrorCode;
use impresario_core::home::Home;
use impresario_core::resident::{
    parse_once, parse_span, BusyPolicy, FailureClass, MissedPolicy, NotifyEvent, NotifySpec,
    Pacing, ResidentSections, RunSelection, Schedule, ScheduleWhen, Watcher,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::{self, RunArgs, EXIT_INFRA, EXIT_OK, EXIT_USAGE};
use crate::jobs;
use crate::report::{ActReport, FailureReport, RunReport, RunStatus, Status};

/// Current state schema. Bump only for incompatible changes.
const STATE_VERSION: u32 = 1;
const TICK: Duration = Duration::from_millis(500);
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// One stop flag per running watcher thread.
type WatcherHandles = Arc<Mutex<BTreeMap<String, Arc<AtomicBool>>>>;

/// Schedules and watchers share the `running` set but have separate namespaces.
fn schedule_key(name: &str) -> String {
    format!("schedule:{name}")
}

fn watcher_key(name: &str) -> String {
    format!("watcher:{name}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceState {
    pub version: u32,
    pub pid: u32,
    pub started_at: String,
    pub updated_at: String,
    pub workspace: String,
    pub paused: bool,
    pub schedules: BTreeMap<String, ScheduleState>,
    pub watchers: BTreeMap<String, WatcherState>,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            pid: 0,
            started_at: String::new(),
            updated_at: String::new(),
            workspace: String::new(),
            paused: false,
            schedules: BTreeMap::new(),
            watchers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleState {
    pub last_fire: Option<String>,
    pub disabled_once: bool,
    pub runs: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WatcherState {
    pub iterations: u64,
    pub failures: u64,
    pub consecutive_failures: u32,
    pub last_started: Option<String>,
    pub last_status: Option<String>,
    pub last_dump: Option<String>,
    pub next_after: Option<String>,
    pub notification: NotificationState,
}

/// Notification bookkeeping for one watcher.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationState {
    /// The currently open duplicate-failure window.
    pub failure_window: Option<FailureWindow>,
    /// A recovery notification is pending after a failing iteration.
    pub pending_recovery: bool,
}

/// Identical failures inside this window count towards one notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureWindow {
    pub signature: String,
    pub event: String,
    pub started_at: String,
    pub last_at: String,
    /// Number of occurrences in this window, including the notified first one.
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FailureDecision {
    send: bool,
    count: u64,
}

pub struct Paths {
    pub dir: PathBuf,
    pub state: PathBuf,
    pub events: PathBuf,
    pub log: PathBuf,
    lock: PathBuf,
    stop: PathBuf,
    pause: PathBuf,
}

impl Paths {
    #[must_use]
    pub fn new(home: &Home) -> Self {
        let dir = home.service_dir();
        Self {
            state: dir.join("state.json"),
            events: dir.join("events.jsonl"),
            log: dir.join("service.log"),
            lock: dir.join("service.lock"),
            stop: dir.join("control.stop"),
            pause: dir.join("control.pause"),
            dir,
        }
    }

    pub fn ensure(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("cannot create {}: {error}", self.dir.display()))
    }
}

/// Acquire the single-instance guard. A stale pid is replaced.
pub fn acquire_lock(paths: &Paths) -> Result<(), String> {
    paths.ensure()?;
    if let Ok(text) = std::fs::read_to_string(&paths.lock) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            if jobs::pid_alive(pid) {
                return Err(format!("resident runner is already running (pid {pid})"));
            }
        }
    }
    std::fs::write(&paths.lock, std::process::id().to_string())
        .map_err(|error| format!("cannot write {}: {error}", paths.lock.display()))
}

pub fn release_lock(paths: &Paths) {
    let _ = std::fs::remove_file(&paths.lock);
    let _ = std::fs::remove_file(&paths.stop);
}

pub fn read_state(home: &Home) -> Result<ServiceState, String> {
    let path = home.service_state_path();
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ServiceState::default());
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    serde_json::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn save_state(home: &Home, state: &ServiceState) -> Result<(), String> {
    let path = home.service_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| format!("cannot encode service state: {error}"))?;
    std::fs::write(&temp, text)
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&temp, &path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

pub fn append_event(home: &Home, event: serde_json::Value) {
    let path = home.service_events_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write as _;
        let _ = writeln!(file, "{event}");
    }
}

pub fn log_line(home: &Home, line: &str) {
    let path = home.service_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES) {
        let rotated = path.with_extension("log.1");
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&path, &rotated);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write as _;
        let _ = writeln!(
            file,
            "{} {line}",
            Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        );
    }
}

pub fn is_running(home: &Home) -> bool {
    match read_state(home) {
        Ok(state) => state.pid != 0 && jobs::pid_alive(state.pid),
        Err(_) => false,
    }
}

/// Run the resident loop in the foreground.
pub fn run_foreground(home: &Home, base: &Path, config_path: &Path, devices_path: &Path) -> i32 {
    let paths = Paths::new(home);
    if let Err(error) = acquire_lock(&paths) {
        eprintln!("service: {error}");
        return EXIT_INFRA;
    }
    let _ = std::fs::remove_file(&paths.stop);
    let _ = std::fs::remove_file(&paths.pause);
    log_line(home, "resident runner starting");
    let state = Arc::new(Mutex::new(ServiceState {
        pid: std::process::id(),
        started_at: Local::now().to_rfc3339(),
        updated_at: Local::now().to_rfc3339(),
        workspace: base.display().to_string(),
        ..ServiceState::default()
    }));
    let paused = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::new(Mutex::new(BTreeSet::new()));
    let watchers: WatcherHandles = Arc::new(Mutex::new(BTreeMap::new()));

    loop {
        if paths.stop.exists() {
            break;
        }
        let now_paused = paths.pause.exists();
        paused.store(now_paused, Ordering::SeqCst);
        {
            let mut state = match state.lock() {
                Ok(state) => state,
                Err(_) => break,
            };
            state.paused = now_paused;
            state.updated_at = Local::now().to_rfc3339();
            state.pid = std::process::id();
            if let Err(error) = save_state(home, &state) {
                log_line(home, &format!("state write failed: {error}"));
            }
        }

        let sections = match super::config::load_effective(home, config_path) {
            Ok(sections) => sections,
            Err(error) => {
                log_line(home, &format!("configuration error: {error}"));
                append_event(
                    home,
                    json!({"kind":"config_error","message":error,"at":Local::now().to_rfc3339()}),
                );
                std::thread::sleep(TICK);
                continue;
            }
        };
        reconcile_schedules(
            home,
            base,
            config_path,
            devices_path,
            &sections,
            now_paused,
            &state,
            &running,
        );
        reconcile_watchers(
            home,
            base,
            config_path,
            devices_path,
            &sections,
            now_paused,
            &state,
            &paused,
            &stop,
            &running,
            &watchers,
        );
        std::thread::sleep(TICK);
    }

    stop.store(true, Ordering::SeqCst);
    if let Ok(handles) = watchers.lock() {
        for handle in handles.values() {
            handle.store(true, Ordering::SeqCst);
        }
    }
    // Give watcher threads a moment to leave the current iteration.
    for _ in 0..20 {
        let idle = running.lock().map(|set| set.is_empty()).unwrap_or(false);
        if idle {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    {
        if let Ok(mut state) = state.lock() {
            state.updated_at = Local::now().to_rfc3339();
            state.pid = 0;
            let _ = save_state(home, &state);
        }
    }
    log_line(home, "resident runner stopped");
    append_event(
        home,
        json!({"kind":"service_stopped","at":Local::now().to_rfc3339()}),
    );
    release_lock(&paths);
    EXIT_OK
}

#[allow(clippy::too_many_arguments)]
fn reconcile_schedules(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    sections: &ResidentSections,
    paused: bool,
    state: &Arc<Mutex<ServiceState>>,
    running: &Arc<Mutex<BTreeSet<String>>>,
) {
    let now = Local::now();
    let started_at = state
        .lock()
        .ok()
        .and_then(|state| parse_local(&state.started_at));
    for schedule in sections.schedules.iter().filter(|value| value.enabled) {
        let runtime = state
            .lock()
            .ok()
            .and_then(|state| state.schedules.get(&schedule.name).cloned())
            .unwrap_or_default();
        if runtime.disabled_once {
            continue;
        }
        let last = runtime.last_fire.as_deref().and_then(parse_local);
        let due = if let ScheduleWhen::Once { at } = &schedule.when {
            let Some(at) = parse_once(at)
                .ok()
                .and_then(|naive| Local.from_local_datetime(&naive).earliest())
            else {
                mark_once_disabled(state, &schedule.name);
                continue;
            };
            if last.is_some() {
                mark_once_disabled(state, &schedule.name);
                continue;
            }
            if at > now {
                continue;
            }
            let observed = started_at
                .as_ref()
                .is_some_and(|start| at >= *start - chrono::Duration::minutes(1));
            if !observed && matches!(schedule.on_missed, MissedPolicy::Skip) {
                append_event(
                    home,
                    json!({"kind":"schedule_missed","schedule":schedule.name,"at":now.to_rfc3339()}),
                );
                mark_once_disabled(state, &schedule.name);
                continue;
            }
            now
        } else {
            let next = match last.as_ref() {
                Some(last) => match schedule.next_after(last) {
                    Ok(value) => value,
                    Err(error) => {
                        log_line(
                            home,
                            &format!("schedule '{}' ignored: {error}", schedule.name),
                        );
                        continue;
                    }
                },
                None => match schedule.next_after(&now) {
                    Ok(value) => value,
                    Err(error) => {
                        log_line(
                            home,
                            &format!("schedule '{}' ignored: {error}", schedule.name),
                        );
                        continue;
                    }
                },
            };
            let Some(next) = next else {
                mark_once_disabled(state, &schedule.name);
                continue;
            };
            next
        };
        if due > now {
            continue;
        }
        if paused {
            continue;
        }
        let missed = last.is_some() && (now - due) > chrono::Duration::minutes(1);
        if missed && matches!(schedule.on_missed, MissedPolicy::Skip) {
            update_last_fire(state, &schedule.name, &now);
            append_event(
                home,
                json!({"kind":"schedule_missed","schedule":schedule.name,"at":now.to_rfc3339()}),
            );
            continue;
        }
        let busy = running
            .lock()
            .map(|set| set.contains(&schedule_key(&schedule.name)))
            .unwrap_or(false);
        if busy {
            if matches!(schedule.on_busy, BusyPolicy::Queue) {
                // Leave `last_fire` untouched: the next tick retries as soon
                // as the running occurrence releases the schedule name.
                continue;
            }
            append_event(
                home,
                json!({"kind":"schedule_overlap","schedule":schedule.name,"at":now.to_rfc3339()}),
            );
            update_last_fire(state, &schedule.name, &now);
            continue;
        }
        if spawn_schedule(
            home,
            base,
            config_path,
            devices_path,
            schedule,
            state,
            running,
        ) {
            update_last_fire(state, &schedule.name, &now);
            if matches!(
                &schedule.when,
                impresario_core::resident::ScheduleWhen::Once { .. }
            ) {
                mark_once_disabled(state, &schedule.name);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reconcile_watchers(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    sections: &ResidentSections,
    paused: bool,
    state: &Arc<Mutex<ServiceState>>,
    service_paused: &Arc<AtomicBool>,
    service_stop: &Arc<AtomicBool>,
    running: &Arc<Mutex<BTreeSet<String>>>,
    handles: &WatcherHandles,
) {
    let mut desired = BTreeSet::new();
    for watcher in sections.watchers.iter().filter(|value| value.enabled) {
        desired.insert(watcher.name.clone());
        let already = handles
            .lock()
            .map(|map| map.contains_key(&watcher.name))
            .unwrap_or(false);
        if already {
            continue;
        }
        let stop = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = handles.lock() {
            map.insert(watcher.name.clone(), stop.clone());
        }
        let watcher = watcher.clone();
        let home = home.clone();
        let base = base.to_path_buf();
        let config_path = config_path.to_path_buf();
        let devices_path = devices_path.to_path_buf();
        let state = state.clone();
        let service_paused = service_paused.clone();
        let service_stop = service_stop.clone();
        let running = running.clone();
        let paused_now = paused;
        std::thread::spawn(move || {
            watcher_loop(
                &home,
                &base,
                &config_path,
                &devices_path,
                &watcher,
                &state,
                &service_paused,
                &service_stop,
                &running,
                paused_now,
                &stop,
            );
        });
    }
    if let Ok(mut map) = handles.lock() {
        let stale: Vec<String> = map
            .keys()
            .filter(|name| !desired.contains(*name))
            .cloned()
            .collect();
        for name in stale {
            if let Some(handle) = map.remove(&name) {
                handle.store(true, Ordering::SeqCst);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn watcher_loop(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    watcher: &Watcher,
    state: &Arc<Mutex<ServiceState>>,
    service_paused: &Arc<AtomicBool>,
    service_stop: &Arc<AtomicBool>,
    running: &Arc<Mutex<BTreeSet<String>>>,
    initially_paused: bool,
    stop: &Arc<AtomicBool>,
) {
    let mut paused = initially_paused;
    let mut failure_streak = 0_u32;
    loop {
        if stop.load(Ordering::SeqCst) || service_stop.load(Ordering::SeqCst) {
            break;
        }
        if watch_stopped(home, &watcher.name) {
            sleep_unless_stopped(Duration::from_millis(250), service_stop, stop);
            continue;
        }
        if watch_paused(home, &watcher.name) {
            sleep_unless_stopped(Duration::from_millis(250), service_stop, stop);
            continue;
        }
        paused |= service_paused.load(Ordering::SeqCst);
        if paused {
            sleep_unless_stopped(Duration::from_millis(250), service_stop, stop);
            if !service_paused.load(Ordering::SeqCst) {
                paused = false;
            }
            continue;
        }
        if let Ok(mut set) = running.lock() {
            set.insert(watcher_key(&watcher.name));
        }
        let started = Local::now();
        {
            if let Ok(mut state) = state.lock() {
                let entry = state.watchers.entry(watcher.name.clone()).or_default();
                entry.iterations += 1;
                entry.last_started = Some(started.to_rfc3339());
            }
        }
        let args = match resident_args(home, &watcher.run, BusyPolicy::Queue) {
            Ok(args) => args,
            Err(error) => {
                failure_streak = failure_streak.saturating_add(1);
                if let Ok(mut state) = state.lock() {
                    let entry = state.watchers.entry(watcher.name.clone()).or_default();
                    entry.last_status = Some("profile_error".to_string());
                    entry.consecutive_failures = failure_streak;
                    entry.failures += 1;
                }
                append_event(
                    home,
                    json!({
                        "kind": "watch_failure",
                        "watcher": watcher.name,
                        "status": "profile_error",
                        "error": error,
                        "at": Local::now().to_rfc3339()
                    }),
                );
                let gap = watcher_backoff(watcher, failure_streak);
                sleep_unless_stopped(gap, service_stop, stop);
                if let Ok(mut set) = running.lock() {
                    set.remove(&watcher_key(&watcher.name));
                }
                continue;
            }
        };
        let outcome = cli::run_workspace(base, config_path, devices_path, &args, false, true);
        let status = outcome_status(&outcome);
        let unverifiable = outcome_unverifiable(&outcome);
        let failed = outcome.exit_code != EXIT_OK && outcome.exit_code != EXIT_USAGE;
        if failed || unverifiable {
            failure_streak = failure_streak.saturating_add(1);
        } else {
            failure_streak = 0;
        }
        if let Ok(mut state) = state.lock() {
            let entry = state.watchers.entry(watcher.name.clone()).or_default();
            entry.last_status = Some(status.clone());
            entry.consecutive_failures = failure_streak;
            if failed || unverifiable {
                entry.failures += 1;
            }
        }
        if failed || unverifiable {
            let dump = write_dump(base, watcher, &outcome, &status);
            if let Some(path) = &dump {
                let path_text = path.display().to_string();
                if let Ok(mut state) = state.lock() {
                    state
                        .watchers
                        .entry(watcher.name.clone())
                        .or_default()
                        .last_dump = Some(path_text.clone());
                }
                append_event(
                    home,
                    json!({
                        "kind": "watch_failure",
                        "watcher": watcher.name,
                        "status": status,
                        "dump": path_text,
                        "at": Local::now().to_rfc3339()
                    }),
                );
            }
            let event = classify_outcome(&outcome, watcher.device_health.treat_unreachable_as)
                .unwrap_or(NotifyEvent::Failure);
            let signature = failure_signature(&outcome, &status);
            notify_watcher_failure(
                home,
                watcher,
                &status,
                event,
                &signature,
                dump.as_deref(),
                state,
            );
        } else {
            append_event(
                home,
                json!({
                    "kind": "watch_iteration",
                    "watcher": watcher.name,
                    "status": status,
                    "at": Local::now().to_rfc3339()
                }),
            );
            if outcome.exit_code == EXIT_OK {
                notify_watcher_recovery(home, watcher, &status, state);
            }
        }
        let gap = watcher_gap(watcher, failure_streak);
        sleep_unless_stopped(gap, service_stop, stop);
        if let Ok(mut set) = running.lock() {
            set.remove(&watcher_key(&watcher.name));
        }
    }
}

fn watcher_gap(watcher: &Watcher, failure_streak: u32) -> Duration {
    let mut gap = match &watcher.pacing {
        Pacing::BackToBack => Duration::ZERO,
        Pacing::MinGap { gap } => impresario_core::resident::parse_span(gap)
            .ok()
            .and_then(|duration| duration.to_std().ok())
            .unwrap_or(Duration::ZERO),
    };
    if failure_streak > 0 {
        gap = gap.max(watcher_backoff(watcher, failure_streak));
    }
    gap
}

fn watcher_backoff(watcher: &Watcher, failure_streak: u32) -> Duration {
    if failure_streak == 0 {
        return Duration::ZERO;
    }
    let backoff = &watcher.device_health.backoff;
    if backoff.is_empty() {
        return Duration::ZERO;
    }
    let last = backoff.len().saturating_sub(1);
    let index = usize::try_from(failure_streak.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(last);
    backoff
        .get(index)
        .and_then(|text| impresario_core::resident::parse_span(text).ok())
        .and_then(|duration| duration.to_std().ok())
        .unwrap_or(Duration::ZERO)
}

fn sleep_unless_stopped(duration: Duration, service_stop: &AtomicBool, stop: &AtomicBool) {
    let mut left = duration;
    while !left.is_zero() {
        if service_stop.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
            return;
        }
        let step = left.min(Duration::from_millis(250));
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
}

fn spawn_schedule(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    schedule: &Schedule,
    state: &Arc<Mutex<ServiceState>>,
    running: &Arc<Mutex<BTreeSet<String>>>,
) -> bool {
    if let Ok(mut set) = running.lock() {
        if !set.insert(schedule_key(&schedule.name)) {
            return false;
        }
    } else {
        return false;
    }
    let home = home.clone();
    let base = base.to_path_buf();
    let config_path = config_path.to_path_buf();
    let devices_path = devices_path.to_path_buf();
    let schedule = schedule.clone();
    let state = state.clone();
    let running = running.clone();
    std::thread::spawn(move || {
        let args = match resident_args(&home, &schedule.run, schedule.on_busy) {
            Ok(args) => args,
            Err(error) => {
                if let Ok(mut state) = state.lock() {
                    let entry = state.schedules.entry(schedule.name.clone()).or_default();
                    entry.runs += 1;
                    entry.failures += 1;
                }
                append_event(
                    &home,
                    json!({
                        "kind": "schedule_error",
                        "schedule": schedule.name,
                        "error": error,
                        "at": Local::now().to_rfc3339()
                    }),
                );
                if let Ok(mut set) = running.lock() {
                    set.remove(&schedule_key(&schedule.name));
                }
                return;
            }
        };
        let outcome = cli::run_workspace(&base, &config_path, &devices_path, &args, false, true);
        if let Ok(mut state) = state.lock() {
            let entry = state.schedules.entry(schedule.name.clone()).or_default();
            entry.runs += 1;
            if outcome.exit_code != EXIT_OK {
                entry.failures += 1;
            }
        }
        append_event(
            &home,
            json!({
                "kind": "schedule_fire",
                "schedule": schedule.name,
                "exit_code": outcome.exit_code,
                "job": outcome.run_id,
                "at": Local::now().to_rfc3339()
            }),
        );
        let status = outcome_status(&outcome);
        notify_schedule_outcome(&home, &schedule, &outcome, &status, &state);
        if let Ok(mut set) = running.lock() {
            set.remove(&schedule_key(&schedule.name));
        }
    });
    true
}

fn update_last_fire(state: &Arc<Mutex<ServiceState>>, name: &str, at: &DateTime<Local>) {
    if let Ok(mut state) = state.lock() {
        let entry = state.schedules.entry(name.to_string()).or_default();
        entry.last_fire = Some(at.to_rfc3339());
    }
}

fn mark_once_disabled(state: &Arc<Mutex<ServiceState>>, name: &str) {
    if let Ok(mut state) = state.lock() {
        state
            .schedules
            .entry(name.to_string())
            .or_default()
            .disabled_once = true;
    }
}

fn run_args(selection: &RunSelection, busy: BusyPolicy) -> RunArgs {
    RunArgs {
        play: selection.plays.clone(),
        app: selection.app.clone(),
        device: selection.actors.clone(),
        platform: Vec::new(),
        tag: selection.tags.clone(),
        exclude_tag: selection.exclude_tags.clone(),
        act: Vec::new(),
        profile: selection.profile.clone(),
        only_failed: false,
        var: Vec::new(),
        arg: Vec::new(),
        timeout: None,
        retries: selection.retries,
        unverifiable: None,
        reporter: None,
        fail_fast: false,
        dry_run: false,
        detach: false,
        on_busy: match busy {
            BusyPolicy::Queue => jobs::OnBusy::Queue,
            BusyPolicy::Skip | BusyPolicy::Fail => jobs::OnBusy::Fail,
        },
    }
}

/// Build the CLI-equivalent arguments for an immediate schedule/watch run.
#[must_use]
pub fn run_args_for_cli(selection: &RunSelection) -> RunArgs {
    run_args(selection, BusyPolicy::Queue)
}

/// Build resident arguments and merge the optional saved profile.
fn resident_args(
    home: &Home,
    selection: &RunSelection,
    busy: BusyPolicy,
) -> Result<RunArgs, String> {
    let mut args = run_args(selection, busy);
    if let Some(profile) = args.profile.clone() {
        crate::profile_cmd::apply(home, &profile, &mut args)?;
    }
    Ok(args)
}

/// Copy a failing iteration into `reports/watch/<name>/<timestamp>/`.
fn write_dump(
    base: &Path,
    watcher: &Watcher,
    outcome: &cli::RunOutcome,
    status: &str,
) -> Option<PathBuf> {
    if watcher.on_failure.dump.is_empty() {
        return None;
    }
    let stamp = Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let root = base
        .join("reports")
        .join("watch")
        .join(&watcher.name)
        .join(&stamp);
    if std::fs::create_dir_all(&root).is_err() {
        return None;
    }
    if let Some(run_dir) = &outcome.run_dir {
        let _ = copy_tree(run_dir, &root, 0);
    }
    let metadata = json!({
        "watcher": watcher.name,
        "status": status,
        "exit_code": outcome.exit_code,
        "run_id": outcome.run_id,
        "created_at": Local::now().to_rfc3339(),
        "dump": watcher.on_failure.dump,
    });
    let _ = std::fs::write(
        root.join("dump.json"),
        serde_json::to_string_pretty(&metadata).unwrap_or_default(),
    );
    let _ = prune_watcher_reports(base, watcher);
    Some(root)
}

fn copy_tree(source: &Path, destination: &Path, depth: usize) -> std::io::Result<()> {
    if depth > 8 {
        return Ok(());
    }
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree(&path, &target, depth + 1)?;
        } else if metadata.is_file() && metadata.len() <= 32 * 1024 * 1024 {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn prune_watcher_reports(base: &Path, watcher: &Watcher) -> std::io::Result<()> {
    let Ok((max_reports, max_age, max_disk)) = watcher.retention.limits() else {
        return Ok(());
    };
    let root = base.join("reports").join("watch").join(&watcher.name);
    let mut entries = Vec::new();
    let Ok(read) = std::fs::read_dir(&root) else {
        return Ok(());
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            entries.push((modified, path));
        }
    }
    entries.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let now = std::time::SystemTime::now();
    let mut kept_total = 0_u64;
    for (index, (modified, path)) in entries.iter().enumerate() {
        let age_ok = match now.duration_since(*modified) {
            Ok(age) => chrono::Duration::from_std(age).is_ok_and(|age| age <= max_age),
            Err(_) => true,
        };
        let size = directory_size(path);
        let too_many = index >= max_reports;
        let too_old = !age_ok;
        // Keep the newest report even if it alone exceeds the disk limit, and
        // never count the size of reports that are being removed.
        let too_large = index > 0 && kept_total.saturating_add(size) > max_disk;
        if too_many || too_old || too_large {
            let _ = std::fs::remove_dir_all(path);
        } else {
            kept_total = kept_total.saturating_add(size);
        }
    }
    Ok(())
}

fn directory_size(path: &Path) -> u64 {
    let mut total = 0_u64;
    let Ok(read) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in read.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path()));
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    total
}

fn outcome_status(outcome: &cli::RunOutcome) -> String {
    outcome
        .report
        .as_ref()
        .map(|report| report.status.as_str().to_string())
        .unwrap_or_else(|| format!("exit {}", outcome.exit_code))
}

fn outcome_unverifiable(outcome: &cli::RunOutcome) -> bool {
    outcome
        .report
        .as_ref()
        .is_some_and(|report| report.summary().acts_unverifiable > 0)
}

/// Classify a finished run into the notification event it represents.
fn classify_outcome(
    outcome: &cli::RunOutcome,
    unreachable_as: FailureClass,
) -> Option<NotifyEvent> {
    let Some(report) = outcome.report.as_ref() else {
        return if outcome.exit_code == EXIT_INFRA {
            Some(match unreachable_as {
                FailureClass::Infrastructure => NotifyEvent::DeviceUnreachable,
                FailureClass::Test => NotifyEvent::Failure,
            })
        } else {
            (outcome.exit_code != EXIT_OK).then_some(NotifyEvent::Failure)
        };
    };
    if report_has_infrastructure_failure(report) {
        return Some(match unreachable_as {
            FailureClass::Infrastructure => NotifyEvent::DeviceUnreachable,
            FailureClass::Test => NotifyEvent::Failure,
        });
    }
    let summary = report.summary();
    if summary.acts_failed > 0
        || summary.steps_failed > 0
        || matches!(
            report.status,
            RunStatus::Failed | RunStatus::Error | RunStatus::Interrupted
        )
    {
        return Some(NotifyEvent::Failure);
    }
    if summary.acts_unverifiable > 0 {
        return Some(NotifyEvent::Unverifiable);
    }
    if outcome.exit_code != EXIT_OK {
        return Some(NotifyEvent::Failure);
    }
    None
}

fn report_has_infrastructure_failure(report: &RunReport) -> bool {
    report.devices.iter().any(|device| {
        device
            .error
            .as_ref()
            .is_some_and(|error| is_infrastructure_code(&error.code))
            || device.plays.iter().any(|play| {
                play.acts.iter().any(|act| {
                    final_failure(act).is_some_and(|failure| is_infrastructure_code(&failure.code))
                })
            })
    })
}

/// Only the final attempt decides how a failed act is reported; earlier failed
/// attempts that a retry recovered must not keep notifying.
fn final_failure(act: &ActReport) -> Option<&FailureReport> {
    if act.status != Status::Failed {
        return None;
    }
    act.attempts
        .last()
        .and_then(|attempt| attempt.failure.as_ref())
}

fn is_infrastructure_code(code: &str) -> bool {
    WireErrorCode::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == code)
        .is_some_and(WireErrorCode::is_infrastructure)
}

/// Stable identity for duplicate-failure throttling: act/stage/step/code.
fn failure_signature(outcome: &cli::RunOutcome, fallback: &str) -> String {
    let Some(report) = outcome.report.as_ref() else {
        return fallback.to_string();
    };
    let mut unverifiable = None;
    for device in &report.devices {
        if let Some(error) = &device.error {
            return format!("{}/device/{}", device.name, error.code);
        }
        for play in &device.plays {
            for act in &play.acts {
                if let Some(failure) = final_failure(act) {
                    let stage = act
                        .attempts
                        .last()
                        .and_then(|attempt| {
                            attempt.stages.iter().find(|stage| {
                                stage.steps.iter().any(|step| step.status == Status::Failed)
                            })
                        })
                        .map_or("-", |stage| stage.name.as_str());
                    return format!(
                        "{}/{}/{}/{}/step:{}/{}",
                        device.name,
                        play.name,
                        act_signature_name(&act.name),
                        stage,
                        failure.step_index,
                        failure.code
                    );
                }
                if act.status == Status::Unverifiable && unverifiable.is_none() {
                    unverifiable = Some(format!(
                        "{}/{}/{}:unverifiable",
                        device.name,
                        play.name,
                        act_signature_name(&act.name)
                    ));
                }
            }
        }
    }
    unverifiable.unwrap_or_else(|| fallback.to_string())
}

fn act_signature_name(name: &str) -> &str {
    name.split_once(" [").map_or(name, |(base, _)| base)
}

fn event_name(event: NotifyEvent) -> &'static str {
    match event {
        NotifyEvent::Failure => "failure",
        NotifyEvent::Unverifiable => "unverifiable",
        NotifyEvent::DeviceUnreachable => "device_unreachable",
        NotifyEvent::Recovery => "recovery",
    }
}

fn notify_channels(spec: &NotifySpec, event: NotifyEvent) -> Vec<String> {
    if spec.on.contains(&event) {
        spec.channels.clone()
    } else {
        Vec::new()
    }
}

fn apply_failure_window(
    window: &mut Option<FailureWindow>,
    signature: &str,
    event: NotifyEvent,
    now: DateTime<Local>,
    throttle: chrono::Duration,
) -> FailureDecision {
    let event = event_name(event);
    if let Some(window) = window.as_mut() {
        let duplicate = window.signature.as_str() == signature
            && window.event.as_str() == event
            && parse_local(&window.started_at).is_some_and(|started| now - started <= throttle);
        if duplicate {
            window.last_at = now.to_rfc3339();
            window.count = window.count.saturating_add(1);
            return FailureDecision {
                send: false,
                count: window.count,
            };
        }
    }
    *window = Some(FailureWindow {
        signature: signature.to_string(),
        event: event.to_string(),
        started_at: now.to_rfc3339(),
        last_at: now.to_rfc3339(),
        count: 1,
    });
    FailureDecision {
        send: true,
        count: 1,
    }
}

#[allow(clippy::too_many_arguments)]
fn notify_watcher_failure(
    home: &Home,
    watcher: &Watcher,
    status: &str,
    event: NotifyEvent,
    signature: &str,
    dump: Option<&Path>,
    state: &Arc<Mutex<ServiceState>>,
) {
    let channels = watcher.on_failure.notify.clone();
    if channels.is_empty() {
        return;
    }
    let throttle = parse_span(&watcher.on_failure.throttle.same_failure_window)
        .unwrap_or_else(|_| chrono::Duration::minutes(15));
    let now = Local::now();
    let decision = {
        let Ok(mut state) = state.lock() else {
            return;
        };
        let entry = state.watchers.entry(watcher.name.clone()).or_default();
        let decision = apply_failure_window(
            &mut entry.notification.failure_window,
            signature,
            event,
            now,
            throttle,
        );
        entry.notification.pending_recovery = true;
        decision
    };
    let dump = dump.map(|path| path.display().to_string());
    if decision.send {
        append_event(
            home,
            json!({
                "kind": "watch_notify",
                "watcher": watcher.name,
                "event": event_name(event),
                "signature": signature,
                "status": status,
                "unverifiable": event == NotifyEvent::Unverifiable,
                "count": decision.count,
                "channels": channels,
                "dump": dump,
                "at": now.to_rfc3339()
            }),
        );
    } else {
        append_event(
            home,
            json!({
                "kind": "watch_notify_suppressed",
                "watcher": watcher.name,
                "event": event_name(event),
                "signature": signature,
                "status": status,
                "unverifiable": event == NotifyEvent::Unverifiable,
                "count": decision.count,
                "channels": channels,
                "dump": dump,
                "at": now.to_rfc3339()
            }),
        );
    }
}

fn notify_watcher_recovery(
    home: &Home,
    watcher: &Watcher,
    status: &str,
    state: &Arc<Mutex<ServiceState>>,
) {
    let channels = watcher.on_failure.notify.clone();
    if channels.is_empty() {
        return;
    }
    let pending = {
        let Ok(mut state) = state.lock() else {
            return;
        };
        let entry = state.watchers.entry(watcher.name.clone()).or_default();
        if entry.notification.pending_recovery {
            entry.notification.pending_recovery = false;
            entry.notification.failure_window = None;
            true
        } else {
            false
        }
    };
    if pending {
        append_event(
            home,
            json!({
                "kind": "watch_notify",
                "watcher": watcher.name,
                "event": "recovery",
                "status": status,
                "unverifiable": false,
                "channels": channels,
                "at": Local::now().to_rfc3339()
            }),
        );
    }
}

fn notify_schedule_outcome(
    home: &Home,
    schedule: &Schedule,
    outcome: &cli::RunOutcome,
    status: &str,
    state: &Arc<Mutex<ServiceState>>,
) {
    let event = classify_outcome(outcome, FailureClass::Infrastructure);
    let (channels, notify_event) = {
        let Ok(mut state) = state.lock() else {
            return;
        };
        let entry = state.schedules.entry(schedule.name.clone()).or_default();
        if let Some(event) = event {
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            (notify_channels(&schedule.notify, event), event)
        } else {
            let recovery = entry.consecutive_failures > 0;
            if recovery {
                entry.consecutive_failures = 0;
            }
            (
                if recovery {
                    notify_channels(&schedule.notify, NotifyEvent::Recovery)
                } else {
                    Vec::new()
                },
                NotifyEvent::Recovery,
            )
        }
    };
    if channels.is_empty() {
        return;
    }
    append_event(
        home,
        json!({
            "kind": "schedule_notify",
            "schedule": schedule.name,
            "event": event_name(notify_event),
            "status": status,
            "exit_code": outcome.exit_code,
            "job": outcome.run_id,
            "channels": channels,
            "at": Local::now().to_rfc3339()
        }),
    );
}

/// Parse a stored local timestamp.
#[must_use]
pub fn parse_local(text: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|value| value.with_timezone(&Local))
}

/// Parse a local time for `service status` display.
#[must_use]
pub fn display_local(text: &str) -> String {
    parse_local(text)
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| text.to_string())
}

/// The autostart file for the current OS, if one can be determined.
#[cfg(windows)]
#[must_use]
pub fn autostart_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
            .join("impresario-service.cmd"),
    )
}

/// The autostart file for the current OS, if one can be determined.
#[cfg(target_os = "macos")]
#[must_use]
pub fn autostart_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join("com.impresario.service.plist"),
    )
}

/// The autostart file for the current OS, if one can be determined.
#[cfg(all(unix, not(target_os = "macos")))]
#[must_use]
pub fn autostart_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(
        config
            .join("systemd")
            .join("user")
            .join("impresario.service"),
    )
}

/// The autostart file for the current OS, if one can be determined.
#[cfg(not(any(windows, unix)))]
#[must_use]
pub fn autostart_path() -> Option<PathBuf> {
    None
}

/// Write the per-user autostart hook. Returns the file written.
pub fn install_autostart(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
) -> Result<PathBuf, String> {
    let path = autostart_path()
        .ok_or_else(|| "cannot determine a per-user autostart directory".to_string())?;
    let _ = home;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let exe =
        std::env::current_exe().map_err(|error| format!("cannot locate impresario: {error}"))?;
    #[cfg(windows)]
    {
        let text = format!(
            "@echo off\r\n\"{}\" --workspace \"{}\" --config \"{}\" --devices-config \"{}\" service run >> \"{}\" 2>&1\r\n",
            exe.display(),
            base.display(),
            config_path.display(),
            devices_path.display(),
            home.service_log_path().display()
        );
        std::fs::write(&path, text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    #[cfg(target_os = "macos")]
    {
        let text = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.impresario.service</string>
  <key>ProgramArguments</key><array>
    <string>{}</string><string>--workspace</string><string>{}</string>
    <string>--config</string><string>{}</string>
    <string>--devices-config</string><string>{}</string>
    <string>service</string><string>run</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
</dict></plist>
"#,
            exe.display(),
            base.display(),
            config_path.display(),
            devices_path.display()
        );
        std::fs::write(&path, text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let text = format!(
            "[Unit]\nDescription=impresario resident runner\n\n[Service]\nExecStart=\"{}\" --workspace \"{}\" --config \"{}\" --devices-config \"{}\" service run\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            exe.display(),
            base.display(),
            config_path.display(),
            devices_path.display()
        );
        std::fs::write(&path, text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    Ok(path)
}

/// Remove the per-user autostart hook.
pub fn uninstall_autostart() -> Result<Option<PathBuf>, String> {
    let Some(path) = autostart_path() else {
        return Ok(None);
    };
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

/// Spawn a detached resident process for the current workspace.
pub fn start(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
) -> Result<u32, String> {
    if is_running(home) {
        let state = read_state(home)?;
        return Ok(state.pid);
    }
    let exe =
        std::env::current_exe().map_err(|error| format!("cannot locate impresario: {error}"))?;
    let log = home.service_log_path();
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|error| format!("cannot open {}: {error}", log.display()))?;
    let err = out
        .try_clone()
        .map_err(|error| format!("cannot duplicate the log handle: {error}"))?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("--workspace")
        .arg(base)
        .arg("--config")
        .arg(config_path)
        .arg("--devices-config")
        .arg(devices_path)
        .arg("service")
        .arg("run")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid() is async-signal-safe and only detaches the child
        // from the controlling terminal; no allocation happens after fork.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = command
        .spawn()
        .map_err(|error| format!("cannot start the resident runner: {error}"))?;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(state) = read_state(home) {
            if state.pid != 0 && jobs::pid_alive(state.pid) {
                return Ok(state.pid);
            }
        }
    }
    Ok(child.id())
}

/// Ask the runner to stop; the caller may poll [`is_running`].
pub fn request_stop(home: &Home) -> Result<(), String> {
    let paths = Paths::new(home);
    paths.ensure()?;
    std::fs::write(&paths.stop, Local::now().to_rfc3339())
        .map_err(|error| format!("cannot write {}: {error}", paths.stop.display()))
}

pub fn request_pause(home: &Home, paused: bool) -> Result<(), String> {
    let paths = Paths::new(home);
    paths.ensure()?;
    if paused {
        std::fs::write(&paths.pause, Local::now().to_rfc3339())
            .map_err(|error| format!("cannot write {}: {error}", paths.pause.display()))
    } else {
        match std::fs::remove_file(&paths.pause) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot remove {}: {error}", paths.pause.display())),
        }
    }
}

fn watch_control_path(home: &Home, name: &str, suffix: &str) -> PathBuf {
    home.service_dir()
        .join("watch")
        .join(format!("{name}.{suffix}"))
}

fn control_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && !name.contains("..")
}

/// Per-watcher pause marker. The service watches this file on every tick.
pub fn request_pause_watch(home: &Home, name: &str, paused: bool) -> Result<(), String> {
    if !control_name_ok(name) {
        return Err(format!("'{name}' is not a valid watcher name"));
    }
    let path = watch_control_path(home, name, "pause");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    if paused {
        std::fs::write(&path, Local::now().to_rfc3339())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
        }
    }
}

/// Ask one watcher loop to leave after the current iteration.
pub fn request_stop_watch(home: &Home, name: &str) -> Result<(), String> {
    if !control_name_ok(name) {
        return Err(format!("'{name}' is not a valid watcher name"));
    }
    let path = watch_control_path(home, name, "stop");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, Local::now().to_rfc3339())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Remove the stop marker when a watcher is explicitly started again.
pub fn clear_watch_stop(home: &Home, name: &str) -> Result<(), String> {
    if !control_name_ok(name) {
        return Err(format!("'{name}' is not a valid watcher name"));
    }
    let path = watch_control_path(home, name, "stop");
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

#[must_use]
pub fn watch_paused(home: &Home, name: &str) -> bool {
    control_name_ok(name) && watch_control_path(home, name, "pause").exists()
}

fn watch_stopped(home: &Home, name: &str) -> bool {
    control_name_ok(name) && watch_control_path(home, name, "stop").exists()
}

/// Print the service log, optionally following it.
pub fn print_logs(home: &Home, follow: bool) -> Result<(), String> {
    let path = home.service_log_path();
    let mut stdout = std::io::stdout();
    let mut file = std::fs::File::open(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    use std::io::Write as _;
    let _ = stdout.write_all(buffer.as_bytes());
    if !follow {
        return Ok(());
    }
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let mut more = String::new();
        if file.read_to_string(&mut more).is_ok() && !more.is_empty() {
            let _ = stdout.write_all(more.as_bytes());
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{
        AttemptReport, DeviceReport, PlayReport, SpanJson, StageReport, StepReport,
    };
    use impresario_core::resident::{
        DeviceHealth, FailurePolicy, Pacing, Retention, WatcherPriority,
    };

    fn test_watcher() -> Watcher {
        Watcher {
            name: "soak".to_string(),
            run: RunSelection::default(),
            pacing: Pacing::default(),
            priority: WatcherPriority::Normal,
            on_failure: FailurePolicy::default(),
            device_health: DeviceHealth {
                treat_unreachable_as: Default::default(),
                backoff: vec!["10s".to_string(), "30s".to_string(), "2m".to_string()],
            },
            retention: Retention::default(),
            enabled: true,
        }
    }

    #[test]
    fn watcher_backoff_clamps_to_the_last_entry() {
        let watcher = test_watcher();
        assert_eq!(watcher_backoff(&watcher, 0), Duration::ZERO);
        assert_eq!(watcher_backoff(&watcher, 1), Duration::from_secs(10));
        assert_eq!(watcher_backoff(&watcher, 2), Duration::from_secs(30));
        assert_eq!(watcher_backoff(&watcher, 3), Duration::from_secs(120));
        assert_eq!(watcher_backoff(&watcher, 99), Duration::from_secs(120));
    }

    #[test]
    fn prune_keeps_the_newest_reports_and_honours_max_reports() {
        let base = tempfile::tempdir().expect("base");
        let root = base.path().join("reports").join("watch").join("soak");
        std::fs::create_dir_all(&root).expect("reports root");
        for name in ["one", "two", "three"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).expect("report dir");
            std::fs::write(dir.join("report.json"), b"{}").expect("report");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut watcher = test_watcher();
        watcher.retention.max_reports = 2;
        prune_watcher_reports(base.path(), &watcher).expect("prune");
        let names: Vec<String> = std::fs::read_dir(&root)
            .expect("read reports")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().any(|name| name == "three"), "{names:?}");
    }

    fn report_with(status: Status, run_status: RunStatus, code: Option<&str>) -> RunReport {
        let step = StepReport {
            source: "Action: click => B".to_string(),
            span: SpanJson {
                file: "p.play".to_string(),
                line: 1,
                col: 1,
                len: 10,
            },
            status,
            duration_ms: 1,
            error: None,
        };
        RunReport {
            schema_version: 2,
            run_id: "run".to_string(),
            started_at: "2026-11-05T00:00:00Z".to_string(),
            duration_ms: 1,
            status: run_status,
            exit_code: if run_status == RunStatus::Passed {
                0
            } else {
                1
            },
            devices: vec![DeviceReport {
                name: "tv".to_string(),
                platform: "android".to_string(),
                device_info: None,
                error: None,
                plays: vec![PlayReport {
                    name: "P".to_string(),
                    file: "p.play".to_string(),
                    acts: vec![ActReport {
                        name: "A [user=alice]".to_string(),
                        tags: Vec::new(),
                        example: Default::default(),
                        status,
                        flaky: false,
                        attempts: vec![AttemptReport {
                            n: 1,
                            status,
                            duration_ms: 1,
                            stages: vec![StageReport {
                                name: "S".to_string(),
                                span: None,
                                status,
                                steps: vec![step],
                            }],
                            failure: code.map(|code| FailureReport {
                                step_index: 0,
                                code: code.to_string(),
                                message: "failed".to_string(),
                                details: Default::default(),
                                screenshot: None,
                                log_tail: Vec::new(),
                            }),
                        }],
                    }],
                }],
            }],
        }
    }

    fn outcome(report: RunReport) -> cli::RunOutcome {
        cli::RunOutcome {
            exit_code: report.exit_code,
            run_id: Some(report.run_id.clone()),
            run_dir: None,
            report: Some(report),
            messages: Vec::new(),
            detail: None,
        }
    }

    #[test]
    fn failure_signature_uses_the_act_stage_step_and_code() {
        let outcome = outcome(report_with(
            Status::Failed,
            RunStatus::Failed,
            Some("CHECK_FAILED"),
        ));
        assert_eq!(
            failure_signature(&outcome, "fallback"),
            "tv/P/A/S/step:0/CHECK_FAILED"
        );
        assert_eq!(act_signature_name("A [user=alice]"), "A");
    }

    #[test]
    fn outcome_classification_separates_test_unverifiable_and_infrastructure() {
        let failed = outcome(report_with(
            Status::Failed,
            RunStatus::Failed,
            Some("CHECK_FAILED"),
        ));
        assert_eq!(
            classify_outcome(&failed, FailureClass::Infrastructure),
            Some(NotifyEvent::Failure)
        );
        let unverifiable = outcome(report_with(Status::Unverifiable, RunStatus::Passed, None));
        assert_eq!(
            classify_outcome(&unverifiable, FailureClass::Infrastructure),
            Some(NotifyEvent::Unverifiable)
        );
        let infrastructure = outcome(report_with(
            Status::Failed,
            RunStatus::Failed,
            Some("TRANSPORT"),
        ));
        assert_eq!(
            classify_outcome(&infrastructure, FailureClass::Infrastructure),
            Some(NotifyEvent::DeviceUnreachable)
        );
        assert_eq!(
            classify_outcome(&infrastructure, FailureClass::Test),
            Some(NotifyEvent::Failure)
        );
        let passed = outcome(report_with(Status::Passed, RunStatus::Passed, None));
        assert_eq!(
            classify_outcome(&passed, FailureClass::Infrastructure),
            None
        );
        let missing = cli::RunOutcome {
            exit_code: EXIT_INFRA,
            run_id: None,
            run_dir: None,
            report: None,
            messages: Vec::new(),
            detail: None,
        };
        assert_eq!(
            classify_outcome(&missing, FailureClass::Infrastructure),
            Some(NotifyEvent::DeviceUnreachable)
        );
        assert_eq!(
            classify_outcome(&missing, FailureClass::Test),
            Some(NotifyEvent::Failure)
        );
    }

    #[test]
    fn identical_failures_are_throttled_inside_the_window() {
        let now = Local::now();
        let mut window = None;
        let first = apply_failure_window(
            &mut window,
            "sig",
            NotifyEvent::Failure,
            now,
            chrono::Duration::minutes(15),
        );
        assert!(first.send);
        assert_eq!(first.count, 1);
        let second = apply_failure_window(
            &mut window,
            "sig",
            NotifyEvent::Failure,
            now + chrono::Duration::minutes(5),
            chrono::Duration::minutes(15),
        );
        assert!(!second.send);
        assert_eq!(second.count, 2);
        let expired = apply_failure_window(
            &mut window,
            "sig",
            NotifyEvent::Failure,
            now + chrono::Duration::minutes(20),
            chrono::Duration::minutes(15),
        );
        assert!(expired.send);
        assert_eq!(expired.count, 1);
        let changed = apply_failure_window(
            &mut window,
            "other",
            NotifyEvent::Failure,
            now + chrono::Duration::minutes(21),
            chrono::Duration::minutes(15),
        );
        assert!(changed.send);
        assert_eq!(changed.count, 1);
    }

    #[test]
    fn schedule_notification_channels_follow_the_event_list() {
        let spec = NotifySpec {
            on: vec![NotifyEvent::Failure, NotifyEvent::Recovery],
            channels: vec!["toast".to_string(), "webhook:ops".to_string()],
        };
        assert_eq!(
            notify_channels(&spec, NotifyEvent::Failure),
            vec!["toast".to_string(), "webhook:ops".to_string()]
        );
        assert!(notify_channels(&spec, NotifyEvent::Unverifiable).is_empty());
    }
}
