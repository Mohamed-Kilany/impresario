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
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, TimeZone as _};
use impresario_core::command::WireErrorCode;
use impresario_core::home::Home;
use impresario_core::resident::{
    parse_once, parse_span, BusyPolicy, DumpKind, FailureClass, MissedPolicy, Notifications,
    NotifyEvent, NotifySpec, Pacing, ResidentSections, RunSelection, Schedule, ScheduleWhen,
    Watcher, WatcherLease, WatcherPriority,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::{self, RunArgs, EXIT_INFRA, EXIT_OK, EXIT_USAGE};
use crate::jobs;
use crate::report::{ActReport, FailureReport, RunReport, RunStatus, Status};

use super::notify::{self, Message};

/// Current state schema. Bump only for incompatible changes.
const STATE_VERSION: u32 = 1;
const TICK: Duration = Duration::from_millis(500);
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// How often a watcher enforces retention. Pruning walks every retained
/// directory, so a back-to-back watcher must not pay that cost per iteration.
const RETENTION_INTERVAL: Duration = Duration::from_secs(60);

/// Schema of the per-watcher normal-report index.
const REPORT_INDEX_SCHEMA: u32 = 1;

/// Version of this binary, written to `state.json` so clients can detect a
/// resident runner left behind by an older installation.
pub const RUNNER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A runner that has not refreshed its heartbeat for this long is reported as
/// unresponsive. The runner ticks every 500 ms; the timeout leaves room for a
/// suspended host or a heavily loaded machine.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Last runner loop tick. Clients treat a stale value as an unresponsive
    /// process; `updated_at` remains the last config observation used to
    /// anchor missed fires.
    pub heartbeat_at: String,
    /// Version of the binary that wrote this state; empty on files written
    /// before the version handshake existed.
    pub runner_version: String,
    pub workspace: String,
    pub paused: bool,
    pub schedules: BTreeMap<String, ScheduleState>,
    pub watchers: BTreeMap<String, WatcherState>,
    /// Moving-window delivery counters, keyed by channel name.
    pub notification_rates: BTreeMap<String, notify::RateWindow>,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            pid: 0,
            started_at: String::new(),
            updated_at: String::new(),
            heartbeat_at: String::new(),
            runner_version: String::new(),
            workspace: String::new(),
            paused: false,
            schedules: BTreeMap::new(),
            watchers: BTreeMap::new(),
            notification_rates: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleState {
    pub last_fire: Option<String>,
    /// When the runner first observed this schedule; anchors the first fire.
    pub observed_at: Option<String>,
    /// Serialized `when` block the counters belong to. Editing the schedule
    /// resets its anchor so a new `once` is not treated as already fired.
    pub when: Option<String>,
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

/// Whether the persisted heartbeat is older than [`HEARTBEAT_TIMEOUT`].
///
/// A state file with neither timestamp belongs to a pre-handshake runner; a
/// live pid then counts as stale because liveness cannot be proven.
#[must_use]
pub fn heartbeat_stale(state: &ServiceState, now: DateTime<Local>) -> bool {
    let Some(at) = parse_local(&state.heartbeat_at).or_else(|| parse_local(&state.updated_at))
    else {
        return state.pid != 0;
    };
    now.signed_duration_since(at).num_seconds() > HEARTBEAT_TIMEOUT.as_secs() as i64
}

/// Whether a live state file was written by a different binary version.
#[must_use]
pub fn version_mismatch(state: &ServiceState) -> bool {
    state.pid != 0 && state.runner_version != RUNNER_VERSION
}

/// Resume the persisted counters when the runner restarts in the same
/// workspace. The previous `updated_at` is the last time the runner observed
/// the config, so it anchors missed-fire detection for schedules that were
/// configured while the service was off. A different workspace starts clean.
fn resume_state(
    previous: ServiceState,
    base: &Path,
    started_at: DateTime<Local>,
) -> (ServiceState, Option<DateTime<Local>>) {
    let workspace = base.display().to_string();
    let same_workspace = previous.workspace.is_empty() || previous.workspace == workspace;
    let prior_seen = if same_workspace {
        parse_local(&previous.updated_at)
    } else {
        None
    };
    let mut state = if same_workspace {
        previous
    } else {
        ServiceState::default()
    };
    state.version = STATE_VERSION;
    state.pid = std::process::id();
    state.started_at = started_at.to_rfc3339();
    state.updated_at = started_at.to_rfc3339();
    state.heartbeat_at = started_at.to_rfc3339();
    state.runner_version = RUNNER_VERSION.to_string();
    state.workspace = workspace;
    state.paused = false;
    (state, prior_seen)
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
    let started_at = Local::now();
    let previous = match read_state(home) {
        Ok(state) => state,
        Err(error) => {
            log_line(home, &format!("state read failed: {error}"));
            ServiceState::default()
        }
    };
    let (loaded, prior_seen) = resume_state(previous, base, started_at);
    let state = Arc::new(Mutex::new(loaded));
    let paused = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let running = Arc::new(Mutex::new(BTreeSet::new()));
    let watchers: WatcherHandles = Arc::new(Mutex::new(BTreeMap::new()));
    let mut first_reconcile = true;

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
            let now = Local::now();
            state.updated_at = now.to_rfc3339();
            state.heartbeat_at = now.to_rfc3339();
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
            if first_reconcile { prior_seen } else { None },
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
        first_reconcile = false;
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
    startup_seen: Option<DateTime<Local>>,
    state: &Arc<Mutex<ServiceState>>,
    running: &Arc<Mutex<BTreeSet<String>>>,
) {
    let now = Local::now();
    let started_at = state
        .lock()
        .ok()
        .and_then(|state| parse_local(&state.started_at));
    for schedule in sections.schedules.iter().filter(|value| value.enabled) {
        let when_key = serde_json::to_string(&schedule.when).unwrap_or_default();
        let runtime = match state.lock() {
            Ok(mut state) => {
                let entry = state.schedules.entry(schedule.name.clone()).or_default();
                // An old state file has no fingerprint: adopt the current
                // `when` without discarding its history. A real edit resets
                // the anchor so a new `once` is not treated as already fired.
                if entry
                    .when
                    .as_deref()
                    .is_some_and(|previous| previous != when_key)
                {
                    entry.last_fire = None;
                    entry.observed_at = None;
                    entry.disabled_once = false;
                }
                entry.when = Some(when_key);
                entry.clone()
            }
            Err(_) => continue,
        };
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
            let observed = match runtime.observed_at.as_deref().and_then(parse_local) {
                Some(observed) => observed,
                None => {
                    // A schedule configured while the runner was off uses the
                    // last persisted observation as its baseline, so a fire
                    // missed in between still follows `on_missed`. A brand-new
                    // schedule starts counting from now.
                    let observed = startup_seen.unwrap_or(now);
                    set_observed_at(state, &schedule.name, &observed);
                    observed
                }
            };
            let anchor = last.unwrap_or(observed);
            match recurring_due(schedule, anchor, now) {
                Ok(DueState::Exhausted) => {
                    mark_once_disabled(state, &schedule.name);
                    continue;
                }
                Ok(DueState::Waiting) => continue,
                Ok(DueState::Due { missed }) => {
                    if missed && matches!(schedule.on_missed, MissedPolicy::Skip) {
                        update_last_fire(state, &schedule.name, &now);
                        append_event(
                            home,
                            json!({"kind":"schedule_missed","schedule":schedule.name,"at":now.to_rfc3339()}),
                        );
                        continue;
                    }
                    now
                }
                Err(error) => {
                    log_line(
                        home,
                        &format!("schedule '{}' ignored: {error}", schedule.name),
                    );
                    continue;
                }
            }
        };
        if due > now {
            continue;
        }
        if paused {
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
            &sections.notifications,
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

/// Whether a recurring schedule is due at `now`, relative to its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DueState {
    Waiting,
    Due { missed: bool },
    Exhausted,
}

fn recurring_due(
    schedule: &Schedule,
    anchor: DateTime<Local>,
    now: DateTime<Local>,
) -> Result<DueState, String> {
    let Some(due) = schedule.next_after(&anchor)? else {
        return Ok(DueState::Exhausted);
    };
    if due > now {
        return Ok(DueState::Waiting);
    }
    Ok(DueState::Due {
        missed: (now - due) > chrono::Duration::minutes(1),
    })
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
        let notifications = sections.notifications.clone();
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
                &notifications,
                &state,
                &service_paused,
                &service_stop,
                &running,
                paused_now,
                &stop,
                &SystemWatcherRuntime,
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

/// The current enabled definition of a watcher, if it still exists.
fn live_watcher<'a>(sections: &'a ResidentSections, name: &str) -> Option<&'a Watcher> {
    sections
        .watchers
        .iter()
        .find(|value| value.name == name && value.enabled)
}

/// Clock and execution seam for the watcher loop. Production uses the wall
/// clock and the normal workspace runner; the fake-time soak test drives the
/// same loop without sleeping or touching a device.
trait WatcherRuntime {
    fn now(&self) -> DateTime<Local>;
    fn monotonic(&self) -> Instant;
    fn sleep(&self, duration: Duration, service_stop: &AtomicBool, stop: &AtomicBool);
    fn run(
        &self,
        base: &Path,
        config_path: &Path,
        devices_path: &Path,
        args: &RunArgs,
        lease: WatcherLease,
        lifetime: &mut Option<jobs::LifetimeLease>,
    ) -> cli::RunOutcome;
}

struct SystemWatcherRuntime;

impl WatcherRuntime for SystemWatcherRuntime {
    fn now(&self) -> DateTime<Local> {
        Local::now()
    }

    fn monotonic(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration, service_stop: &AtomicBool, stop: &AtomicBool) {
        sleep_unless_stopped(duration, service_stop, stop);
    }

    fn run(
        &self,
        base: &Path,
        config_path: &Path,
        devices_path: &Path,
        args: &RunArgs,
        lease: WatcherLease,
        lifetime: &mut Option<jobs::LifetimeLease>,
    ) -> cli::RunOutcome {
        match lease {
            WatcherLease::NoGap => cli::run_workspace_with_lease(
                base,
                config_path,
                devices_path,
                args,
                false,
                true,
                Some(lifetime),
            ),
            WatcherLease::Gap => {
                cli::run_workspace(base, config_path, devices_path, args, false, true)
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
    initial: &Watcher,
    initial_notifications: &Notifications,
    state: &Arc<Mutex<ServiceState>>,
    service_paused: &Arc<AtomicBool>,
    service_stop: &Arc<AtomicBool>,
    running: &Arc<Mutex<BTreeSet<String>>>,
    initially_paused: bool,
    stop: &Arc<AtomicBool>,
    runtime: &dyn WatcherRuntime,
) {
    let mut paused = initially_paused;
    let mut failure_streak = 0_u32;
    // Device backoff follows infrastructure failures only; `failure_streak`
    // stays the general failure count shown in the resident status.
    let mut infrastructure_streak = 0_u32;
    let mut watcher = initial.clone();
    let mut notifications = initial_notifications.clone();
    let mut last_devices: Option<Vec<String>> = None;
    // D-39 no-gap mode keeps the device lease across iterations. It is
    // released on stop, pause, disable or when the mode returns to `gap`.
    let mut lifetime_lease: Option<jobs::LifetimeLease> = None;
    let mut last_retention: Option<Instant> = None;
    loop {
        if stop.load(Ordering::SeqCst) || service_stop.load(Ordering::SeqCst) {
            break;
        }
        if watch_stopped(home, &watcher.name) {
            lifetime_lease = None;
            runtime.sleep(Duration::from_millis(250), service_stop, stop);
            continue;
        }
        if watch_paused(home, &watcher.name) {
            lifetime_lease = None;
            runtime.sleep(Duration::from_millis(250), service_stop, stop);
            continue;
        }
        paused |= service_paused.load(Ordering::SeqCst);
        if paused {
            lifetime_lease = None;
            runtime.sleep(Duration::from_millis(250), service_stop, stop);
            if !service_paused.load(Ordering::SeqCst) {
                paused = false;
            }
            continue;
        }
        // Re-read the layered config each iteration so edits to pacing, the run
        // selection, backoff or notifications take effect without a restart.
        match super::config::load_effective(home, config_path) {
            Ok(sections) => {
                match live_watcher(&sections, &watcher.name) {
                    Some(current) => {
                        watcher = current.clone();
                        notifications = sections.notifications.clone();
                    }
                    None => {
                        // Disabled or removed: the reconcile pass sets `stop`
                        // on its next tick, so wait for it instead of exiting
                        // into a stale handle entry.
                        lifetime_lease = None;
                        runtime.sleep(Duration::from_millis(250), service_stop, stop);
                        continue;
                    }
                }
            }
            Err(error) => {
                log_line(
                    home,
                    &format!("watcher '{}' config reload failed: {error}", watcher.name),
                );
            }
        }
        let watcher = &watcher;
        if watcher.lease == WatcherLease::Gap {
            lifetime_lease = None;
        }
        // A low-priority watcher gives queued manual/scheduled jobs a chance
        // to take the device between iterations.
        let yielding = watcher.priority == WatcherPriority::Low
            && last_devices
                .as_deref()
                .is_some_and(|devices| jobs::waiting_for(home, devices));
        if yielding {
            runtime.sleep(Duration::from_millis(250), service_stop, stop);
            continue;
        }
        if let Ok(mut set) = running.lock() {
            set.insert(watcher_key(&watcher.name));
        }
        let started = runtime.now();
        {
            if let Ok(mut state) = state.lock() {
                let entry = state.watchers.entry(watcher.name.clone()).or_default();
                entry.iterations += 1;
                entry.last_started = Some(started.to_rfc3339());
            }
        }
        let mut args = match resident_args(home, &watcher.run, BusyPolicy::Queue) {
            Ok(args) => args,
            Err(error) => {
                // No job was planned, so a held no-gap lease would only
                // starve other runs until the watcher stops.
                lifetime_lease = None;
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
                        "at": runtime.now().to_rfc3339()
                    }),
                );
                let gap = watcher_backoff(watcher, failure_streak);
                runtime.sleep(gap, service_stop, stop);
                if let Ok(mut set) = running.lock() {
                    set.remove(&watcher_key(&watcher.name));
                }
                continue;
            }
        };
        args.job_label = Some(format!("watcher {}", watcher.name));
        let outcome = runtime.run(
            base,
            config_path,
            devices_path,
            &args,
            watcher.lease,
            &mut lifetime_lease,
        );
        if outcome.run_id.is_none() {
            // Planning or submission failed, so this iteration holds no job.
            // Release a lifetime lease instead of starving other runs until
            // the watcher is stopped.
            lifetime_lease = None;
        }
        if let Some(id) = &outcome.run_id {
            if let Ok(Some(record)) = jobs::JobStore::new(home).read(id) {
                if !record.devices.is_empty() {
                    last_devices = Some(record.devices);
                }
            }
        }
        remember_watcher_report(home, base, watcher, &outcome);
        let status = outcome_status(&outcome);
        let unverifiable = outcome_unverifiable(&outcome);
        let failed = outcome.exit_code != EXIT_OK && outcome.exit_code != EXIT_USAGE;
        if failed || unverifiable {
            failure_streak = failure_streak.saturating_add(1);
        } else {
            failure_streak = 0;
        }
        infrastructure_streak = next_infrastructure_streak(
            infrastructure_streak,
            &outcome,
            watcher.device_health.treat_unreachable_as,
        );
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
                        "at": runtime.now().to_rfc3339()
                    }),
                );
            }
            let event = classify_outcome(&outcome, watcher.device_health.treat_unreachable_as)
                .unwrap_or(NotifyEvent::Failure);
            let signature = failure_signature(&outcome, &status);
            notify_watcher_failure(
                home,
                watcher,
                &outcome,
                &status,
                event,
                &signature,
                dump.as_deref(),
                state,
                &notifications,
            );
        } else {
            append_event(
                home,
                json!({
                    "kind": "watch_iteration",
                    "watcher": watcher.name,
                    "status": status,
                    "at": runtime.now().to_rfc3339()
                }),
            );
            if outcome.exit_code == EXIT_OK {
                notify_watcher_recovery(home, watcher, &outcome, &status, state, &notifications);
            }
        }
        let now_mono = runtime.monotonic();
        if last_retention.is_none_or(|at| now_mono.duration_since(at) >= RETENTION_INTERVAL) {
            if let Err(error) = prune_watcher_reports(home, base, watcher) {
                log_line(
                    home,
                    &format!("watcher '{}' retention failed: {error}", watcher.name),
                );
            }
            last_retention = Some(now_mono);
        }
        let gap = watcher_gap(watcher, infrastructure_streak);
        runtime.sleep(gap, service_stop, stop);
        if let Ok(mut set) = running.lock() {
            set.remove(&watcher_key(&watcher.name));
        }
    }
}

fn watcher_gap(watcher: &Watcher, infrastructure_streak: u32) -> Duration {
    let mut gap = match &watcher.pacing {
        Pacing::BackToBack => Duration::ZERO,
        Pacing::MinGap { gap } => impresario_core::resident::parse_span(gap)
            .ok()
            .and_then(|duration| duration.to_std().ok())
            .unwrap_or(Duration::ZERO),
    };
    if infrastructure_streak > 0 {
        gap = gap.max(watcher_backoff(watcher, infrastructure_streak));
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

#[allow(clippy::too_many_arguments)]
fn spawn_schedule(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    schedule: &Schedule,
    notifications: &Notifications,
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
    let notifications = notifications.clone();
    let state = state.clone();
    let running = running.clone();
    std::thread::spawn(move || {
        let mut args = match resident_args(&home, &schedule.run, schedule.on_busy) {
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
        args.job_label = Some(format!("schedule {}", schedule.name));
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
        notify_schedule_outcome(&home, &schedule, &outcome, &status, &state, &notifications);
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

fn set_observed_at(state: &Arc<Mutex<ServiceState>>, name: &str, at: &DateTime<Local>) {
    if let Ok(mut state) = state.lock() {
        state
            .schedules
            .entry(name.to_string())
            .or_default()
            .observed_at = Some(at.to_rfc3339());
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
        job_label: None,
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
    let kinds = &watcher.on_failure.dump;
    if kinds.is_empty() {
        return None;
    }
    let root = dump_root(base, watcher);
    if std::fs::create_dir_all(&root).is_err() {
        return None;
    }
    if let Some(run_dir) = &outcome.run_dir {
        if kinds.contains(&DumpKind::Report) {
            for name in ["report.json", "junit.xml", "report.html"] {
                let source = run_dir.join(name);
                if source.is_file() {
                    let _ = std::fs::copy(&source, root.join(name));
                }
            }
        }
        if kinds.contains(&DumpKind::Screenshot) {
            let source = run_dir.join("screenshots");
            if source.is_dir() {
                let _ = copy_tree(&source, &root.join("screenshots"), 0);
            }
        }
    }
    if let Some(report) = &outcome.report {
        let failures = failure_entries(report);
        if kinds.contains(&DumpKind::Logs) {
            let logs: Vec<serde_json::Value> = failures
                .iter()
                .map(|entry| {
                    json!({
                        "device": entry["device"],
                        "play": entry["play"],
                        "act": entry["act"],
                        "code": entry["code"],
                        "lines": entry["log_tail"],
                    })
                })
                .collect();
            write_dump_json(
                &root.join("logs.json"),
                &json!({"watcher": watcher.name, "failures": logs}),
            );
        }
        if kinds.contains(&DumpKind::DeviceState) {
            let devices: Vec<serde_json::Value> = report
                .devices
                .iter()
                .map(|device| {
                    json!({
                        "name": device.name,
                        "platform": device.platform,
                        "device_info": device.device_info,
                        "error": device.error,
                    })
                })
                .collect();
            write_dump_json(
                &root.join("device_state.json"),
                &json!({"watcher": watcher.name, "devices": devices}),
            );
        }
        if kinds.contains(&DumpKind::Evidence) {
            write_dump_json(
                &root.join("evidence.json"),
                &json!({"watcher": watcher.name, "failures": failures}),
            );
        }
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
    Some(root)
}

/// Timestamped dump directory, disambiguated when two dumps land in the same
/// millisecond.
fn dump_root(base: &Path, watcher: &Watcher) -> PathBuf {
    let parent = base.join("reports").join("watch").join(&watcher.name);
    let stamp = Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let mut candidate = parent.join(&stamp);
    let mut suffix = 1_u32;
    while candidate.exists() {
        candidate = parent.join(format!("{stamp}-{suffix}"));
        suffix = suffix.saturating_add(1);
    }
    candidate
}

/// One entry per act whose final attempt carries a failure.
fn failure_entries(report: &RunReport) -> Vec<serde_json::Value> {
    let mut entries = Vec::new();
    for device in &report.devices {
        for play in &device.plays {
            for act in &play.acts {
                let Some(failure) = act
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.failure.as_ref())
                else {
                    continue;
                };
                entries.push(json!({
                    "device": device.name,
                    "play": play.name,
                    "act": act.name,
                    "step_index": failure.step_index,
                    "code": failure.code,
                    "message": failure.message,
                    "details": failure.details,
                    "screenshot": failure.screenshot,
                    "log_tail": failure.log_tail,
                }));
            }
        }
    }
    entries
}

fn write_dump_json(path: &Path, value: &serde_json::Value) {
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    );
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

/// One normal run report produced by a watcher iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReportEntry {
    run_id: String,
    path: String,
}

/// Bounded per-watcher index of normal reports that retention may remove.
/// Seeded from job records once so reports written before the index existed
/// are still covered; it is per workspace because each workspace can have a
/// different `report_output`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ReportIndex {
    schema_version: u32,
    reports: Vec<ReportEntry>,
}

impl Default for ReportIndex {
    fn default() -> Self {
        Self {
            schema_version: REPORT_INDEX_SCHEMA,
            reports: Vec::new(),
        }
    }
}

fn workspace_key(base: &Path) -> String {
    format!(
        "{:016x}",
        crate::time_fmt::fnv1a(base.display().to_string().as_bytes())
    )
}

fn report_index_path(home: &Home, base: &Path, watcher: &Watcher) -> PathBuf {
    home.service_dir().join("watchers").join(format!(
        "{}-{}.reports.json",
        workspace_key(base),
        watcher.name
    ))
}

fn load_report_index(path: &Path) -> Option<ReportIndex> {
    let source = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&source).ok()
}

fn save_report_index(path: &Path, index: &ReportIndex) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(index).unwrap_or_default();
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, format!("{text}\n"))?;
    std::fs::rename(&temporary, path)
}

fn seed_report_index(home: &Home, base: &Path, watcher: &Watcher) -> ReportIndex {
    let exact = format!("watcher {}", watcher.name);
    let prefix = format!("watcher {} · ", watcher.name);
    let workspace = base.display().to_string();
    let mut reports: Vec<ReportEntry> = Vec::new();
    for record in jobs::JobStore::new(home).list() {
        if record.workspace != workspace || record.id.is_empty() || record.run_dir.is_empty() {
            continue;
        }
        if record.label != exact && !record.label.starts_with(&prefix) {
            continue;
        }
        if !Path::new(&record.run_dir).is_dir() {
            continue;
        }
        reports.push(ReportEntry {
            run_id: record.id,
            path: record.run_dir,
        });
    }
    reports.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    reports.dedup_by(|left, right| left.run_id == right.run_id);
    ReportIndex {
        schema_version: REPORT_INDEX_SCHEMA,
        reports,
    }
}

/// Load the watcher's report index, seeding it from the job store when the
/// file is missing or unreadable. The boolean reports whether seeding happened
/// so the caller can persist the result even when nothing was pruned.
fn load_or_seed_report_index(home: &Home, base: &Path, watcher: &Watcher) -> (ReportIndex, bool) {
    match load_report_index(&report_index_path(home, base, watcher)) {
        Some(index) => (index, false),
        None => (seed_report_index(home, base, watcher), true),
    }
}

/// Record one iteration's normal report. Reports that were not written (no
/// run directory) are not tracked and therefore never pruned.
fn remember_watcher_report(home: &Home, base: &Path, watcher: &Watcher, outcome: &cli::RunOutcome) {
    let (Some(run_id), Some(run_dir)) = (&outcome.run_id, &outcome.run_dir) else {
        return;
    };
    if run_id.is_empty() || !run_dir.is_dir() {
        return;
    }
    let path = report_index_path(home, base, watcher);
    let (mut index, seeded) = load_or_seed_report_index(home, base, watcher);
    let changed = if index.reports.iter().any(|report| report.run_id == *run_id) {
        false
    } else {
        index.reports.push(ReportEntry {
            run_id: run_id.clone(),
            path: run_dir.display().to_string(),
        });
        true
    };
    if !changed && !seeded {
        return;
    }
    index.schema_version = REPORT_INDEX_SCHEMA;
    if let Err(error) = save_report_index(&path, &index) {
        log_line(
            home,
            &format!(
                "watcher '{}' report index write failed: {error}",
                watcher.name
            ),
        );
    }
}

/// Run ids are `YYYYMMDD-HHMMSS-<4 hex>` (20 chars). The index only ever names
/// run directories, so refusing anything else keeps a corrupted or hand-edited
/// index from deleting an unrelated directory.
fn is_run_dir_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let bytes = name.as_bytes();
    bytes.len() == 20
        && bytes
            .get(..8)
            .is_some_and(|part| part.iter().all(|byte| byte.is_ascii_digit()))
        && bytes.get(8) == Some(&b'-')
        && bytes
            .get(9..15)
            .is_some_and(|part| part.iter().all(|byte| byte.is_ascii_digit()))
        && bytes.get(15) == Some(&b'-')
        && bytes
            .get(16..)
            .is_some_and(|part| part.iter().all(|byte| byte.is_ascii_hexdigit()))
}

/// Enforce one watcher's retention over its failure dumps and the normal run
/// reports its iterations produced. Both kinds share the same limits; only
/// reports named by the watcher's index are candidates, so manual runs in the
/// same `report_output` directory are never removed.
fn prune_watcher_reports(home: &Home, base: &Path, watcher: &Watcher) -> std::io::Result<()> {
    let Ok((max_reports, max_age, max_disk)) = watcher.retention.limits() else {
        return Ok(());
    };
    let dump_root = base.join("reports").join("watch").join(&watcher.name);
    let index_path = report_index_path(home, base, watcher);
    let (mut index, seeded) = load_or_seed_report_index(home, base, watcher);
    let loaded_len = index.reports.len();
    // `None` marks a failure dump; `Some` marks an indexed normal report.
    let mut entries: Vec<(std::time::SystemTime, PathBuf, Option<String>)> = Vec::new();
    if let Ok(read) = std::fs::read_dir(&dump_root) {
        for entry in read.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            entries.push((modified, path, None));
        }
    }
    for report in &index.reports {
        let path = PathBuf::from(&report.path);
        if !path.is_dir() || !is_run_dir_name(&path) {
            continue;
        }
        let modified = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((modified, path, Some(report.run_id.clone())));
    }
    entries.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    let now = std::time::SystemTime::now();
    let mut kept_total = 0_u64;
    let mut kept_reports = BTreeSet::new();
    for (position, (modified, path, run_id)) in entries.iter().enumerate() {
        let age_ok = match now.duration_since(*modified) {
            Ok(age) => chrono::Duration::from_std(age).is_ok_and(|age| age <= max_age),
            Err(_) => true,
        };
        let size = directory_size(path);
        let too_many = position >= max_reports;
        let too_old = !age_ok;
        // Keep the newest report even if it alone exceeds the disk limit, and
        // never count the size of reports that are being removed.
        let too_large = position > 0 && kept_total.saturating_add(size) > max_disk;
        let prune = too_many || too_old || too_large;
        if prune {
            let _ = std::fs::remove_dir_all(path);
        }
        // A failed removal stays indexed so the next pass retries it.
        if !prune || path.exists() {
            kept_total = kept_total.saturating_add(size);
            if let Some(run_id) = run_id {
                kept_reports.insert(run_id.clone());
            }
        }
    }
    index
        .reports
        .retain(|report| kept_reports.contains(&report.run_id));
    index.schema_version = REPORT_INDEX_SCHEMA;
    if seeded || index.reports.len() != loaded_len {
        save_report_index(&index_path, &index)?;
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

/// The report file a notification action and `notify open` should target.
fn outcome_report(outcome: &cli::RunOutcome) -> Option<PathBuf> {
    let directory = outcome.run_dir.as_ref()?;
    ["report.html", "report.json"]
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

fn notification_id(outcome: &cli::RunOutcome, kind: &str, source: &str) -> String {
    outcome.run_id.clone().unwrap_or_else(|| {
        format!(
            "{kind}:{source}:{}",
            Local::now().format("%Y%m%d-%H%M%S-%3f")
        )
    })
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

/// Only infrastructure-class failures advance the watcher's device backoff;
/// test, unverifiable and successful iterations reset the streak.
fn infrastructure_failure(outcome: &cli::RunOutcome, unreachable_as: FailureClass) -> bool {
    classify_outcome(outcome, unreachable_as) == Some(NotifyEvent::DeviceUnreachable)
}

fn next_infrastructure_streak(
    current: u32,
    outcome: &cli::RunOutcome,
    unreachable_as: FailureClass,
) -> u32 {
    if infrastructure_failure(outcome, unreachable_as) {
        current.saturating_add(1)
    } else {
        0
    }
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
    let duplicate = window.as_ref().is_some_and(|window| {
        window.signature.as_str() == signature
            && window.event.as_str() == event
            && parse_local(&window.started_at).is_some_and(|started| now - started <= throttle)
    });
    if duplicate {
        let count = window.as_mut().map_or(1, |window| {
            window.last_at = now.to_rfc3339();
            window.count = window.count.saturating_add(1);
            window.count
        });
        return FailureDecision { send: false, count };
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
    outcome: &cli::RunOutcome,
    status: &str,
    event: NotifyEvent,
    signature: &str,
    dump: Option<&Path>,
    state: &Arc<Mutex<ServiceState>>,
    notifications: &Notifications,
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
    let dump_text = dump.map(|path| path.display().to_string());
    let report = outcome_report(outcome);
    let id = notification_id(outcome, "watch", &watcher.name);
    let message = Message {
        kind: "watch",
        source: watcher.name.clone(),
        id: id.clone(),
        event: event_name(event),
        status: status.to_string(),
        signature: Some(signature.to_string()),
        count: decision.count,
        job: outcome.run_id.clone(),
        report: report.clone(),
        dump: dump.map(Path::to_path_buf),
        exit_code: Some(outcome.exit_code),
        at: now.to_rfc3339(),
    };
    if decision.send {
        append_event(
            home,
            json!({
                "kind": "watch_notify",
                "id": id,
                "watcher": watcher.name,
                "event": event_name(event),
                "signature": signature,
                "status": status,
                "unverifiable": event == NotifyEvent::Unverifiable,
                "count": decision.count,
                "channels": &channels,
                "job": outcome.run_id,
                "report": report,
                "dump": dump_text,
                "at": now.to_rfc3339()
            }),
        );
        notify::dispatch(home, notifications, state, &channels, &message);
    } else {
        append_event(
            home,
            json!({
                "kind": "watch_notify_suppressed",
                "id": id,
                "watcher": watcher.name,
                "event": event_name(event),
                "signature": signature,
                "status": status,
                "unverifiable": event == NotifyEvent::Unverifiable,
                "count": decision.count,
                "channels": &channels,
                "job": outcome.run_id,
                "report": report,
                "dump": dump_text,
                "at": now.to_rfc3339()
            }),
        );
    }
}

fn notify_watcher_recovery(
    home: &Home,
    watcher: &Watcher,
    outcome: &cli::RunOutcome,
    status: &str,
    state: &Arc<Mutex<ServiceState>>,
    notifications: &Notifications,
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
        let now = Local::now();
        let report = outcome_report(outcome);
        let id = notification_id(outcome, "watch", &watcher.name);
        let message = Message {
            kind: "watch",
            source: watcher.name.clone(),
            id: id.clone(),
            event: "recovery",
            status: status.to_string(),
            signature: None,
            count: 1,
            job: outcome.run_id.clone(),
            report: report.clone(),
            dump: None,
            exit_code: Some(outcome.exit_code),
            at: now.to_rfc3339(),
        };
        append_event(
            home,
            json!({
                "kind": "watch_notify",
                "id": id,
                "watcher": watcher.name,
                "event": "recovery",
                "status": status,
                "unverifiable": false,
                "channels": &channels,
                "job": outcome.run_id,
                "report": report,
                "at": now.to_rfc3339()
            }),
        );
        notify::dispatch(home, notifications, state, &channels, &message);
    }
}

fn notify_schedule_outcome(
    home: &Home,
    schedule: &Schedule,
    outcome: &cli::RunOutcome,
    status: &str,
    state: &Arc<Mutex<ServiceState>>,
    notifications: &Notifications,
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
    let now = Local::now();
    let report = outcome_report(outcome);
    let id = notification_id(outcome, "schedule", &schedule.name);
    let message = Message {
        kind: "schedule",
        source: schedule.name.clone(),
        id: id.clone(),
        event: event_name(notify_event),
        status: status.to_string(),
        signature: None,
        count: 1,
        job: outcome.run_id.clone(),
        report: report.clone(),
        dump: None,
        exit_code: Some(outcome.exit_code),
        at: now.to_rfc3339(),
    };
    append_event(
        home,
        json!({
            "kind": "schedule_notify",
            "id": id,
            "schedule": schedule.name,
            "event": event_name(notify_event),
            "status": status,
            "exit_code": outcome.exit_code,
            "job": outcome.run_id,
            "report": report,
            "channels": &channels,
            "at": now.to_rfc3339()
        }),
    );
    notify::dispatch(home, notifications, state, &channels, &message);
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
        if state.pid != 0 && !version_mismatch(&state) {
            return Ok(state.pid);
        }
        if version_mismatch(&state) {
            // Version handshake: a client from a newer installation restarts
            // the resident runner instead of leaving it on stale code.
            let previous = if state.runner_version.is_empty() {
                "unknown"
            } else {
                state.runner_version.as_str()
            };
            log_line(
                home,
                &format!(
                    "resident runner version {previous} differs from client {RUNNER_VERSION}; restarting"
                ),
            );
            request_stop(home)?;
            for _ in 0..100 {
                if !is_running(home) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if is_running(home) {
                return Err(format!(
                    "resident runner version {previous} differs from client {RUNNER_VERSION} and did not stop; run `impresario service restart`"
                ));
            }
        }
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
            lease: WatcherLease::Gap,
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
    fn live_watcher_uses_the_current_enabled_definition() {
        let watcher = test_watcher();
        let mut sections = ResidentSections {
            watchers: vec![watcher.clone()],
            ..ResidentSections::default()
        };
        assert_eq!(live_watcher(&sections, "soak"), Some(&watcher));

        sections.watchers[0].pacing = Pacing::MinGap {
            gap: "30s".to_string(),
        };
        assert_eq!(
            live_watcher(&sections, "soak").map(|current| &current.pacing),
            Some(&Pacing::MinGap {
                gap: "30s".to_string()
            })
        );

        sections.watchers[0].enabled = false;
        assert!(live_watcher(&sections, "soak").is_none());
        assert!(live_watcher(&sections, "missing").is_none());
    }

    #[test]
    fn prune_keeps_the_newest_reports_and_honours_max_reports() {
        let base = tempfile::tempdir().expect("base");
        let home = Home::at(base.path().join("home"));
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
        prune_watcher_reports(&home, base.path(), &watcher).expect("prune");
        let names: Vec<String> = std::fs::read_dir(&root)
            .expect("read reports")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().any(|name| name == "three"), "{names:?}");
    }

    fn write_run_report(root: &Path, run_id: &str) {
        let dir = root.join(run_id);
        std::fs::create_dir_all(&dir).expect("run dir");
        std::fs::write(dir.join("report.json"), b"{}").expect("report");
        std::thread::sleep(Duration::from_millis(20));
    }

    fn index_with(entries: &[(&str, &Path)]) -> ReportIndex {
        ReportIndex {
            schema_version: REPORT_INDEX_SCHEMA,
            reports: entries
                .iter()
                .map(|(run_id, path)| ReportEntry {
                    run_id: (*run_id).to_string(),
                    path: path.display().to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn prune_removes_tracked_normal_reports_and_rewrites_the_index() {
        let base = tempfile::tempdir().expect("base");
        let home = Home::at(base.path().join("home"));
        let report_root = base.path().join("reports");
        let ids = [
            "20260101-000000-aaaa",
            "20260101-000001-bbbb",
            "20260101-000002-cccc",
        ];
        for id in ids {
            write_run_report(&report_root, id);
        }
        let watcher = test_watcher();
        let paths: Vec<PathBuf> = ids.iter().map(|id| report_root.join(id)).collect();
        let index = index_with(&[
            (ids[0], paths[0].as_path()),
            (ids[1], paths[1].as_path()),
            (ids[2], paths[2].as_path()),
        ]);
        save_report_index(&report_index_path(&home, base.path(), &watcher), &index).expect("index");

        let mut watcher = test_watcher();
        watcher.retention.max_reports = 2;
        prune_watcher_reports(&home, base.path(), &watcher).expect("prune");

        assert!(!paths[0].exists());
        assert!(paths[1].exists());
        assert!(paths[2].exists());
        let stored = load_report_index(&report_index_path(&home, base.path(), &watcher))
            .expect("stored index");
        assert_eq!(stored.reports.len(), 2, "{:?}", stored.reports);
        assert!(stored.reports.iter().all(|entry| entry.run_id != ids[0]));
    }

    #[test]
    fn prune_combines_dumps_and_reports_and_leaves_untracked_runs_alone() {
        let base = tempfile::tempdir().expect("base");
        let home = Home::at(base.path().join("home"));
        let report_root = base.path().join("reports");
        let ids = ["20260101-000000-aaaa", "20260101-000001-bbbb"];
        for id in ids {
            write_run_report(&report_root, id);
        }
        let dump = base
            .path()
            .join("reports")
            .join("watch")
            .join("soak")
            .join("20260101-000002-dddd");
        std::fs::create_dir_all(&dump).expect("dump");
        std::fs::write(dump.join("dump.json"), b"{}").expect("dump metadata");
        std::thread::sleep(Duration::from_millis(20));
        // A manual run in the same report directory is not part of the pool.
        let manual = report_root.join("20260101-000003-eeee");
        std::fs::create_dir_all(&manual).expect("manual");
        std::fs::write(manual.join("report.json"), b"{}").expect("manual report");

        let watcher = test_watcher();
        let first = report_root.join(ids[0]);
        let second = report_root.join(ids[1]);
        let index = index_with(&[(ids[0], first.as_path()), (ids[1], second.as_path())]);
        save_report_index(&report_index_path(&home, base.path(), &watcher), &index).expect("index");

        let mut watcher = test_watcher();
        watcher.retention.max_reports = 2;
        prune_watcher_reports(&home, base.path(), &watcher).expect("prune");

        assert!(!report_root.join(ids[0]).exists());
        assert!(report_root.join(ids[1]).exists());
        assert!(dump.exists());
        assert!(manual.exists());
    }

    fn job_record(id: &str, label: &str, workspace: &Path, run_dir: &Path) -> jobs::JobRecord {
        jobs::JobRecord {
            schema_version: 1,
            id: id.to_string(),
            status: jobs::JobStatus::Passed,
            detached: true,
            created_at: "2026-11-08T00:00:00Z".to_string(),
            started_at: None,
            finished_at: None,
            duration_ms: None,
            exit_code: Some(0),
            workspace: workspace.display().to_string(),
            devices: Vec::new(),
            plays: Vec::new(),
            label: label.to_string(),
            pid: None,
            queue_position: None,
            run_dir: run_dir.display().to_string(),
            message: None,
        }
    }

    #[test]
    fn report_index_seeds_from_watcher_job_records_for_the_workspace() {
        let base = tempfile::tempdir().expect("base");
        let home = Home::at(base.path().join("home"));
        let report_root = base.path().join("reports");
        let watcher = test_watcher();
        let run = report_root.join("20260101-000000-aaaa");
        let manual = report_root.join("20260101-000001-bbbb");
        let other = report_root.join("20260101-000002-cccc");
        let prefix_collision = report_root.join("20260101-000003-dddd");
        for dir in [&run, &manual, &other, &prefix_collision] {
            std::fs::create_dir_all(dir).expect("run dir");
        }
        let store = jobs::JobStore::new(&home);
        for record in [
            job_record(
                "20260101-000000-aaaa",
                "watcher soak · 1 play(s) x 1 device(s)",
                base.path(),
                &run,
            ),
            job_record(
                "20260101-000001-bbbb",
                "1 play(s) x 1 device(s)",
                base.path(),
                &manual,
            ),
            job_record(
                "20260101-000002-cccc",
                "watcher soak · 1 play(s) x 1 device(s)",
                Path::new("elsewhere"),
                &other,
            ),
            job_record(
                "20260101-000003-dddd",
                "watcher soak2 · 1 play(s) x 1 device(s)",
                base.path(),
                &prefix_collision,
            ),
        ] {
            store.write_record(&record).expect("job record");
        }

        let index = seed_report_index(&home, base.path(), &watcher);
        assert_eq!(index.reports.len(), 1, "{:?}", index.reports);
        assert_eq!(index.reports[0].run_id, "20260101-000000-aaaa");
    }

    #[test]
    fn remember_watcher_report_adds_each_run_once() {
        let base = tempfile::tempdir().expect("base");
        let home = Home::at(base.path().join("home"));
        let watcher = test_watcher();
        let run_id = "20260101-000000-aaaa";
        let run_dir = base.path().join("reports").join(run_id);
        std::fs::create_dir_all(&run_dir).expect("run dir");
        let outcome = cli::RunOutcome {
            exit_code: EXIT_OK,
            run_id: Some(run_id.to_string()),
            run_dir: Some(run_dir),
            report: None,
            messages: Vec::new(),
            detail: None,
        };

        remember_watcher_report(&home, base.path(), &watcher, &outcome);
        remember_watcher_report(&home, base.path(), &watcher, &outcome);

        let index = load_report_index(&report_index_path(&home, base.path(), &watcher))
            .expect("report index");
        assert_eq!(index.reports.len(), 1);
        assert_eq!(index.reports[0].run_id, run_id);
    }

    /// Fake-time runtime for the 24 h soak: sleeping only advances the clock,
    /// and each iteration synthesizes one passing run report.
    struct SoakRuntime {
        wall_base: DateTime<Local>,
        mono_base: Instant,
        elapsed: Mutex<Duration>,
        horizon: Duration,
        runs: Mutex<u64>,
        report_root: PathBuf,
    }

    impl SoakRuntime {
        fn new(report_root: PathBuf, horizon: Duration) -> Self {
            let wall_base = DateTime::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
                .expect("fixed soak start")
                .with_timezone(&Local);
            Self {
                wall_base,
                mono_base: Instant::now(),
                elapsed: Mutex::new(Duration::ZERO),
                horizon,
                runs: Mutex::new(0),
                report_root,
            }
        }

        fn elapsed(&self) -> Duration {
            *self.elapsed.lock().expect("soak clock")
        }

        fn advance(&self, duration: Duration, stop: &AtomicBool) {
            let mut elapsed = self.elapsed.lock().expect("soak clock");
            *elapsed = elapsed.saturating_add(duration);
            if *elapsed >= self.horizon {
                stop.store(true, Ordering::SeqCst);
            }
        }
    }

    impl WatcherRuntime for SoakRuntime {
        fn now(&self) -> DateTime<Local> {
            self.wall_base.clone()
                + chrono::Duration::from_std(self.elapsed()).expect("soak offset")
        }

        fn monotonic(&self) -> Instant {
            self.mono_base + self.elapsed()
        }

        fn sleep(&self, duration: Duration, _service_stop: &AtomicBool, stop: &AtomicBool) {
            self.advance(duration, stop);
        }

        fn run(
            &self,
            _base: &Path,
            _config_path: &Path,
            _devices_path: &Path,
            _args: &RunArgs,
            _lease: WatcherLease,
            _lifetime: &mut Option<jobs::LifetimeLease>,
        ) -> cli::RunOutcome {
            let index = {
                let mut runs = self.runs.lock().expect("soak runs");
                *runs += 1;
                *runs
            };
            let run_id = format!(
                "{}-{:04x}",
                self.now().format("%Y%m%d-%H%M%S"),
                index & 0xffff
            );
            let run_dir = self.report_root.join(&run_id);
            std::fs::create_dir_all(&run_dir).expect("soak report dir");
            std::fs::write(run_dir.join("report.json"), b"{\"status\":\"passed\"}\n")
                .expect("soak report");
            cli::RunOutcome {
                exit_code: EXIT_OK,
                run_id: Some(run_id),
                run_dir: Some(run_dir),
                report: None,
                messages: Vec::new(),
                detail: None,
            }
        }
    }

    /// TASK 8.3 resource hygiene: drive the watcher loop over a full fake day
    /// (24 h at a 60 s gap = 1440 iterations) and prove that the resident
    /// state, report index and on-disk reports stay bounded and that the
    /// running set drains after every iteration.
    #[test]
    fn watcher_loop_soaks_a_fake_day_with_bounded_state() {
        const HORIZON: Duration = Duration::from_secs(24 * 60 * 60);
        const MAX_REPORTS: u64 = 60;
        let base = tempfile::tempdir().expect("base");
        let home = Home::at(base.path().join("home"));
        let workspace = base.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let config_path = workspace.join("impresario.config");
        std::fs::write(
            &config_path,
            "\
watchers:
  - name: soak
    run: { plays: [smoke.play] }
    pacing: { mode: min_gap, gap: 60s }
    retention: { max_reports: 60 }
",
        )
        .expect("config");
        let devices_path = workspace.join("impresario.devices.config");
        std::fs::write(&devices_path, "devices: []\n").expect("devices");
        let sections = crate::resident::config::read_layer(&config_path).expect("sections");
        let watcher = sections.watchers[0].clone();
        let notifications = sections.notifications.clone();
        let report_root = workspace.join("reports");
        let runtime = SoakRuntime::new(report_root.clone(), HORIZON);
        let state = Arc::new(Mutex::new(ServiceState::default()));
        let service_paused = Arc::new(AtomicBool::new(false));
        let service_stop = Arc::new(AtomicBool::new(false));
        let running = Arc::new(Mutex::new(BTreeSet::new()));
        let stop = Arc::new(AtomicBool::new(false));

        watcher_loop(
            &home,
            &workspace,
            &config_path,
            &devices_path,
            &watcher,
            &notifications,
            &state,
            &service_paused,
            &service_stop,
            &running,
            false,
            &stop,
            &runtime,
        );

        assert!(
            stop.load(Ordering::SeqCst),
            "the fake horizon ends the loop"
        );
        assert_eq!(runtime.elapsed(), HORIZON);
        assert_eq!(*runtime.runs.lock().expect("soak runs"), 1440);
        assert!(
            running.lock().expect("running").is_empty(),
            "every iteration releases its running marker"
        );

        let state = state.lock().expect("state");
        assert_eq!(
            state.watchers.len(),
            1,
            "one watcher entry, not one per run"
        );
        assert!(state.schedules.is_empty());
        assert!(state.notification_rates.is_empty());
        let entry = state.watchers.get("soak").expect("watcher state");
        assert_eq!(entry.iterations, 1440);
        assert_eq!(entry.failures, 0);
        assert_eq!(entry.consecutive_failures, 0);
        let state_bytes = serde_json::to_vec(&*state).expect("state json").len();
        assert!(
            state_bytes < 16 * 1024,
            "resident state grew to {state_bytes} bytes"
        );
        drop(state);

        let index = load_report_index(&report_index_path(&home, &workspace, &watcher))
            .expect("report index");
        assert_eq!(index.reports.len(), usize::try_from(MAX_REPORTS).unwrap());
        let unique: BTreeSet<&str> = index
            .reports
            .iter()
            .map(|entry| entry.run_id.as_str())
            .collect();
        assert_eq!(unique.len(), index.reports.len());
        assert!(
            index
                .reports
                .iter()
                .all(|entry| Path::new(&entry.path).is_dir()),
            "the index never names a pruned report"
        );
        let report_dirs = std::fs::read_dir(&report_root)
            .expect("report root")
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .count();
        assert_eq!(report_dirs, usize::try_from(MAX_REPORTS).unwrap());
        assert!(
            !report_root.join("watch").join("soak").exists(),
            "passing iterations write no failure dumps"
        );

        let events = std::fs::read_to_string(home.service_events_path()).expect("events");
        let iterations = events
            .lines()
            .filter(|line| line.contains("\"kind\":\"watch_iteration\""))
            .count();
        assert_eq!(iterations, 1440, "one event per simulated iteration");
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
    fn infrastructure_streak_only_advances_on_device_failures() {
        let passed = outcome(report_with(Status::Passed, RunStatus::Passed, None));
        let failed = outcome(report_with(
            Status::Failed,
            RunStatus::Failed,
            Some("CHECK_FAILED"),
        ));
        let unverifiable = outcome(report_with(Status::Unverifiable, RunStatus::Passed, None));
        let infrastructure = outcome(report_with(
            Status::Failed,
            RunStatus::Failed,
            Some("TRANSPORT"),
        ));

        let mut streak = 0;
        streak = next_infrastructure_streak(streak, &failed, FailureClass::Infrastructure);
        assert_eq!(streak, 0);
        streak = next_infrastructure_streak(streak, &unverifiable, FailureClass::Infrastructure);
        assert_eq!(streak, 0);
        streak = next_infrastructure_streak(streak, &infrastructure, FailureClass::Infrastructure);
        assert_eq!(streak, 1);
        streak = next_infrastructure_streak(streak, &infrastructure, FailureClass::Infrastructure);
        assert_eq!(streak, 2);
        streak = next_infrastructure_streak(streak, &failed, FailureClass::Infrastructure);
        assert_eq!(streak, 0);
        streak = next_infrastructure_streak(streak, &infrastructure, FailureClass::Infrastructure);
        assert_eq!(streak, 1);
        streak = next_infrastructure_streak(streak, &passed, FailureClass::Infrastructure);
        assert_eq!(streak, 0);

        let missing = cli::RunOutcome {
            exit_code: EXIT_INFRA,
            run_id: None,
            run_dir: None,
            report: None,
            messages: Vec::new(),
            detail: None,
        };
        assert_eq!(
            next_infrastructure_streak(0, &missing, FailureClass::Infrastructure),
            1
        );
        assert_eq!(
            next_infrastructure_streak(0, &missing, FailureClass::Test),
            0
        );
    }

    #[test]
    fn treat_unreachable_as_test_disables_infrastructure_backoff() {
        let infrastructure = outcome(report_with(
            Status::Failed,
            RunStatus::Failed,
            Some("TRANSPORT"),
        ));
        assert!(infrastructure_failure(
            &infrastructure,
            FailureClass::Infrastructure
        ));
        assert!(!infrastructure_failure(&infrastructure, FailureClass::Test));
        let mut streak = 0;
        for _ in 0..3 {
            streak = next_infrastructure_streak(streak, &infrastructure, FailureClass::Test);
            assert_eq!(streak, 0);
        }
    }

    #[test]
    fn watcher_gap_uses_the_infrastructure_streak() {
        let mut watcher = test_watcher();
        watcher.pacing = Pacing::MinGap {
            gap: "5s".to_string(),
        };
        assert_eq!(watcher_gap(&watcher, 0), Duration::from_secs(5));
        assert_eq!(watcher_gap(&watcher, 1), Duration::from_secs(10));
        assert_eq!(watcher_gap(&watcher, 2), Duration::from_secs(30));
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

    fn daily_schedule(at: &str) -> Schedule {
        Schedule {
            name: "nightly".to_string(),
            when: ScheduleWhen::Daily { at: at.to_string() },
            run: RunSelection::default(),
            on_missed: MissedPolicy::Skip,
            on_busy: BusyPolicy::Queue,
            notify: NotifySpec::default(),
            enabled: true,
        }
    }

    fn local_at(hour: u32, minute: u32, second: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 6, hour, minute, second)
            .single()
            .expect("unambiguous local time")
    }

    #[test]
    fn recurring_schedules_anchor_the_first_fire_and_detect_missed_runs() {
        let schedule = daily_schedule("09:00");
        let anchor = local_at(8, 0, 0);
        assert_eq!(
            recurring_due(&schedule, anchor, local_at(8, 59, 0)).unwrap(),
            DueState::Waiting
        );
        assert_eq!(
            recurring_due(&schedule, anchor, local_at(9, 0, 30)).unwrap(),
            DueState::Due { missed: false }
        );
        assert_eq!(
            recurring_due(&schedule, anchor, local_at(10, 0, 0)).unwrap(),
            DueState::Due { missed: true }
        );
    }

    #[test]
    fn interval_schedules_measure_the_next_fire_from_the_anchor() {
        let mut schedule = daily_schedule("09:00");
        schedule.when = ScheduleWhen::Interval {
            every: "1h".to_string(),
        };
        let anchor = local_at(8, 0, 0);
        assert_eq!(
            recurring_due(&schedule, anchor, local_at(8, 30, 0)).unwrap(),
            DueState::Waiting
        );
        assert_eq!(
            recurring_due(&schedule, anchor, local_at(9, 0, 30)).unwrap(),
            DueState::Due { missed: false }
        );
        assert_eq!(
            recurring_due(&schedule, anchor, local_at(10, 30, 0)).unwrap(),
            DueState::Due { missed: true }
        );
    }

    #[test]
    fn resume_state_keeps_history_for_the_same_workspace_only() {
        let base = Path::new("/workspace/demo");
        let started = Local::now();
        let mut previous = ServiceState {
            version: STATE_VERSION,
            pid: 42,
            started_at: "2026-11-05T09:00:00+00:00".to_string(),
            updated_at: "2026-11-05T10:00:00+00:00".to_string(),
            workspace: base.display().to_string(),
            paused: true,
            ..ServiceState::default()
        };
        previous.schedules.insert(
            "nightly".to_string(),
            ScheduleState {
                runs: 3,
                last_fire: Some("2026-11-05T09:00:00+00:00".to_string()),
                ..ScheduleState::default()
            },
        );
        let (resumed, prior) = resume_state(previous, base, started);
        assert_eq!(resumed.pid, std::process::id());
        assert_ne!(resumed.pid, 42);
        assert_eq!(resumed.started_at, started.to_rfc3339());
        assert!(!resumed.paused);
        assert_eq!(resumed.schedules["nightly"].runs, 3);
        assert!(prior.is_some());

        let other = Path::new("/workspace/other");
        let mut previous = ServiceState {
            workspace: base.display().to_string(),
            updated_at: "2026-11-05T10:00:00+00:00".to_string(),
            ..ServiceState::default()
        };
        previous.schedules.insert(
            "nightly".to_string(),
            ScheduleState {
                runs: 3,
                ..ScheduleState::default()
            },
        );
        let (resumed, prior) = resume_state(previous, other, started);
        assert!(resumed.schedules.is_empty());
        assert!(prior.is_none());
        assert_eq!(resumed.workspace, other.display().to_string());
    }

    #[test]
    fn resume_state_stamps_the_runner_version_and_heartbeat() {
        let base = Path::new("/workspace/demo");
        let started = Local::now();
        let (resumed, _) = resume_state(ServiceState::default(), base, started);
        assert_eq!(resumed.runner_version, RUNNER_VERSION);
        assert_eq!(resumed.heartbeat_at, started.to_rfc3339());
        assert_eq!(resumed.updated_at, started.to_rfc3339());
    }

    #[test]
    fn heartbeat_staleness_and_version_mismatch_are_detected() {
        let now = local_at(12, 0, 0);
        let fresh = ServiceState {
            pid: 42,
            heartbeat_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            runner_version: RUNNER_VERSION.to_string(),
            ..ServiceState::default()
        };
        assert!(!heartbeat_stale(&fresh, now));
        assert!(!version_mismatch(&fresh));

        let old = ServiceState {
            heartbeat_at: (now - chrono::Duration::seconds(HEARTBEAT_TIMEOUT.as_secs() as i64 + 1))
                .to_rfc3339(),
            ..fresh.clone()
        };
        assert!(heartbeat_stale(&old, now));

        // Pre-handshake files have no heartbeat field; updated_at still anchors
        // the watchdog so an upgraded client can read them.
        let legacy = ServiceState {
            heartbeat_at: String::new(),
            updated_at: now.to_rfc3339(),
            ..fresh.clone()
        };
        assert!(!heartbeat_stale(&legacy, now));
        let unknown = ServiceState {
            heartbeat_at: String::new(),
            updated_at: String::new(),
            ..fresh.clone()
        };
        assert!(heartbeat_stale(&unknown, now));

        let outdated = ServiceState {
            runner_version: "0.0.1".to_string(),
            ..fresh
        };
        assert!(version_mismatch(&outdated));
        let stopped = ServiceState { pid: 0, ..outdated };
        assert!(!version_mismatch(&stopped));
    }

    fn outcome_in(run_dir: &Path, report: RunReport) -> cli::RunOutcome {
        cli::RunOutcome {
            exit_code: report.exit_code,
            run_id: Some(report.run_id.clone()),
            run_dir: Some(run_dir.to_path_buf()),
            report: Some(report),
            messages: Vec::new(),
            detail: None,
        }
    }

    #[test]
    fn dump_bundles_copy_only_the_selected_kinds() {
        let base = tempfile::tempdir().expect("base");
        let run_dir = base.path().join("reports").join("run-1");
        std::fs::create_dir_all(run_dir.join("screenshots").join("tv")).expect("screenshots");
        std::fs::write(run_dir.join("report.json"), b"{}").expect("report");
        std::fs::write(run_dir.join("junit.xml"), b"<testsuite/>").expect("junit");
        std::fs::write(run_dir.join("report.html"), b"<html/>").expect("html");
        std::fs::write(
            run_dir.join("screenshots").join("tv").join("failure-1.png"),
            b"png",
        )
        .expect("screenshot");
        let mut report = report_with(Status::Failed, RunStatus::Failed, Some("CHECK_FAILED"));
        let failure = report.devices[0].plays[0].acts[0].attempts[0]
            .failure
            .as_mut()
            .expect("failure");
        failure.log_tail = vec!["line one".to_string()];
        failure
            .details
            .insert("probe".to_string(), "value".to_string());
        failure.screenshot = Some("screenshots/tv/failure-1.png".to_string());
        let outcome = outcome_in(&run_dir, report);
        let mut watcher = test_watcher();

        watcher.on_failure.dump = vec![DumpKind::Report];
        let root = write_dump(base.path(), &watcher, &outcome, "failed").expect("report dump");
        assert!(root.join("report.json").is_file());
        assert!(root.join("junit.xml").is_file());
        assert!(root.join("report.html").is_file());
        assert!(!root.join("screenshots").exists());
        assert!(!root.join("logs.json").exists());

        watcher.on_failure.dump = vec![DumpKind::Screenshot];
        let root = write_dump(base.path(), &watcher, &outcome, "failed").expect("screenshot dump");
        assert!(root.join("screenshots/tv/failure-1.png").is_file());
        assert!(!root.join("report.json").exists());

        watcher.on_failure.dump = vec![DumpKind::Logs];
        let root = write_dump(base.path(), &watcher, &outcome, "failed").expect("logs dump");
        let logs: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("logs.json")).expect("logs"))
                .expect("logs json");
        assert_eq!(logs["failures"][0]["lines"][0], "line one");
        assert!(!root.join("report.json").exists());

        watcher.on_failure.dump = vec![DumpKind::DeviceState];
        let root = write_dump(base.path(), &watcher, &outcome, "failed").expect("device dump");
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("device_state.json")).expect("device state"),
        )
        .expect("device state json");
        assert_eq!(state["devices"][0]["name"], "tv");

        watcher.on_failure.dump = vec![DumpKind::Evidence];
        let root = write_dump(base.path(), &watcher, &outcome, "failed").expect("evidence dump");
        let evidence: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("evidence.json")).expect("evidence"),
        )
        .expect("evidence json");
        assert_eq!(evidence["failures"][0]["details"]["probe"], "value");
    }
}
