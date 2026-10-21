//! Jobs, the per-user background supervisor and device leases (TASK-2 3.5).
//!
//! A run is a job with an id. Attached runs execute in the CLI process; a
//! detached run is submitted to a small per-user daemon over a local socket
//! (`interprocess`, S-5). Both write the same state under
//! `~/.impresario/jobs/<id>/` so listing, attaching, tailing, stopping,
//! waiting and reporting are identical. The daemon owns no device itself.
//!
//! Device leases are atomic files (`jobs/leases/<device>.json`) rather than
//! daemon state: they therefore also protect an attached run against a
//! detached one, and a crashed process is recognised by its dead pid instead
//! of a daemon memory table. A stop marker cancels a job at the next command
//! boundary. Wait files (`jobs/leases/wait.<job>`) carry the same owner pid
//! and device list, so a crashed waiter is cleaned up as well.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use impresario_core::config::{ImpresarioConfig, Reporter};
use impresario_core::home::Home;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::exec::{self, ExecEvent, ExecOptions};
use crate::plan::ExecutionPlan;
use crate::report::{RunReport, RunStatus};
use crate::reporters;
use crate::time_fmt::rfc3339_utc;

const JOB_SCHEMA: u32 = 1;
const EVENT_FILE: &str = "events.jsonl";
const CONSOLE_LOG: &str = "console.log";
const RECORD_FILE: &str = "job.json";
const REPORT_COPY: &str = "report.json";
const STOP_FILE: &str = "stop";
const WAIT_POLL: Duration = Duration::from_millis(250);

/// Job lifecycle. `Stopped` and `Interrupted` both exit 130 but say why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Passed,
    Failed,
    Interrupted,
    Error,
    Stopped,
}

impl JobStatus {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Passed => "passed",
            JobStatus::Failed => "failed",
            JobStatus::Interrupted => "interrupted",
            JobStatus::Error => "error",
            JobStatus::Stopped => "stopped",
        }
    }

    #[must_use]
    pub fn terminal(self) -> bool {
        !matches!(self, JobStatus::Queued | JobStatus::Running)
    }
}

/// What happens when one of the requested devices is leased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum OnBusy {
    /// Wait, showing the queue position.
    #[default]
    Wait,
    /// Fail before touching a device.
    Fail,
    /// Return a queued job immediately (detached runs); attached runs wait.
    Queue,
}

/// Serializable description of one run. It may contain resolved secret values
/// in `plan.secrets`; it is sent over the local socket but never written to
/// `job.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub id: String,
    pub base: PathBuf,
    pub report_dir: PathBuf,
    pub config: ImpresarioConfig,
    pub plan: ExecutionPlan,
    pub retries: u32,
    pub fail_fast: bool,
    pub token: Option<String>,
    pub master_version: String,
    pub reporters: Vec<Reporter>,
    pub color: bool,
    pub on_busy: OnBusy,
    pub detached: bool,
    pub label: String,
}

impl JobSpec {
    /// The report directory of this job (`report_dir/<id>`).
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.report_dir.join(&self.id)
    }

    fn devices(&self) -> Vec<String> {
        self.plan
            .devices
            .iter()
            .map(|device| device.device.name.clone())
            .collect()
    }

    fn plays(&self) -> Vec<String> {
        let mut names = BTreeSet::new();
        for device in &self.plan.devices {
            for play in &device.plays {
                names.insert(play.name.clone());
            }
        }
        names.into_iter().collect()
    }
}

/// Durable job state (`jobs/<id>/job.json`). Contains no secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub schema_version: u32,
    pub id: String,
    pub status: JobStatus,
    pub detached: bool,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub workspace: String,
    pub devices: Vec<String>,
    pub plays: Vec<String>,
    pub label: String,
    pub pid: Option<u32>,
    pub queue_position: Option<usize>,
    pub run_dir: String,
    pub message: Option<String>,
}

/// Result of a finished job, also used by an attached foreground run.
#[derive(Debug)]
pub struct JobOutcome {
    pub record: JobRecord,
    pub report: Option<RunReport>,
    pub reporter_errors: Vec<String>,
}

/// Persistent job directories.
#[derive(Debug, Clone)]
pub struct JobStore {
    home: Home,
}

impl JobStore {
    #[must_use]
    pub fn new(home: &Home) -> Self {
        Self { home: home.clone() }
    }

    #[must_use]
    pub fn root(&self) -> PathBuf {
        self.home.jobs_dir()
    }

    #[must_use]
    pub fn dir(&self, id: &str) -> PathBuf {
        self.root().join(id)
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.dir(id).join(RECORD_FILE)
    }

    #[must_use]
    pub fn events_path(&self, id: &str) -> PathBuf {
        self.dir(id).join(EVENT_FILE)
    }

    #[must_use]
    pub fn console_path(&self, id: &str) -> PathBuf {
        self.dir(id).join(CONSOLE_LOG)
    }

    #[must_use]
    pub fn stop_path(&self, id: &str) -> PathBuf {
        self.dir(id).join(STOP_FILE)
    }

    #[must_use]
    pub fn report_path(&self, id: &str) -> PathBuf {
        self.dir(id).join(REPORT_COPY)
    }

    #[must_use]
    pub fn lease_dir(&self) -> PathBuf {
        self.root().join("leases")
    }

    /// Create the state directory and the initial `queued` record.
    pub fn prepare(&self, spec: &JobSpec) -> Result<JobRecord, String> {
        std::fs::create_dir_all(self.dir(&spec.id))
            .map_err(|error| format!("cannot create {}: {error}", self.dir(&spec.id).display()))?;
        let record = JobRecord {
            schema_version: JOB_SCHEMA,
            id: spec.id.clone(),
            status: JobStatus::Queued,
            detached: spec.detached,
            created_at: now_rfc3339(),
            started_at: None,
            finished_at: None,
            duration_ms: None,
            exit_code: None,
            workspace: spec.base.display().to_string(),
            devices: spec.devices(),
            plays: spec.plays(),
            label: spec.label.clone(),
            pid: Some(std::process::id()),
            queue_position: None,
            run_dir: spec.run_dir().display().to_string(),
            message: None,
        };
        self.write_record(&record)?;
        Ok(record)
    }

    /// Replace `job.json`; failures are reported, never ignored.
    pub fn write_record(&self, record: &JobRecord) -> Result<(), String> {
        let dir = self.dir(&record.id);
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let text = serde_json::to_string_pretty(record)
            .map_err(|error| format!("cannot serialize job {}: {error}", record.id))?;
        let path = self.record_path(&record.id);
        std::fs::write(&path, format!("{text}\n"))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// Read one record; a missing file is `Ok(None)`, a broken file is an error.
    pub fn read(&self, id: &str) -> Result<Option<JobRecord>, String> {
        let path = self.record_path(id);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|error| format!("cannot parse {}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    /// All readable records, newest first.
    pub fn list(&self) -> Vec<JobRecord> {
        let mut records = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.root()) else {
            return records;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Ok(Some(record)) = self.read(&id) {
                records.push(record);
            }
        }
        records.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        records
    }

    /// Read the report copy written when the job completed.
    pub fn read_report(&self, id: &str) -> Result<RunReport, String> {
        let path = self.report_path(id);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))
    }

    /// A daemon restart/reboot can never leave a job `running`: mark every
    /// non-terminal job whose owner process is gone as `interrupted`.
    pub fn recover_interrupted(&self) {
        for mut record in self.list() {
            if record.status.terminal() {
                continue;
            }
            if record.pid.is_some_and(pid_alive) {
                continue;
            }
            if record.status == JobStatus::Queued && record.pid.is_none() {
                // Never acknowledged; leave it for `rm`.
                continue;
            }
            record.status = JobStatus::Interrupted;
            record.finished_at = Some(now_rfc3339());
            record.queue_position = None;
            record.message = Some(
                "the supervisor or process that owned this job is gone; the job was interrupted"
                    .to_string(),
            );
            if record.exit_code.is_none() {
                record.exit_code = Some(130);
            }
            let _ = self.write_record(&record);
            let _ = std::fs::write(
                self.dir(&record.id).join("interrupted"),
                "interrupted by recovery\n",
            );
        }
        lease::clean_stale(self);
    }

    /// Ask a running job to stop. Returns `false` when it is not active.
    pub fn request_stop(&self, id: &str) -> Result<bool, String> {
        let Some(record) = self.read(id)? else {
            return Err(format!("unknown job '{id}'"));
        };
        if record.status.terminal() {
            return Ok(false);
        }
        let path = self.stop_path(id);
        std::fs::write(&path, "stop\n")
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(true)
    }

    /// Remove a terminal job directory; active jobs are refused.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let Some(record) = self.read(id)? else {
            return Err(format!("unknown job '{id}'"));
        };
        if !record.status.terminal() {
            return Err(format!(
                "job '{id}' is {}; stop it before removing it",
                record.status.label()
            ));
        }
        let dir = self.dir(id);
        std::fs::remove_dir_all(&dir)
            .map_err(|error| format!("cannot remove {}: {error}", dir.display()))
    }
}

fn now_rfc3339() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    rfc3339_utc(millis)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Run an attached job: prepare its record, acquire leases and execute.
pub fn run_foreground(
    home: &Home,
    spec: &JobSpec,
    print_console: bool,
) -> Result<JobOutcome, String> {
    let store = JobStore::new(home);
    store.prepare(spec)?;
    Ok(run_prepared(&store, spec, print_console))
}

/// Execute a job whose `job.json` has already been written by [`JobStore::prepare`].
pub fn run_prepared(store: &JobStore, spec: &JobSpec, print_console: bool) -> JobOutcome {
    let mut record = match store.read(&spec.id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            let fallback = JobRecord {
                schema_version: JOB_SCHEMA,
                id: spec.id.clone(),
                status: JobStatus::Queued,
                detached: spec.detached,
                created_at: now_rfc3339(),
                started_at: None,
                finished_at: None,
                duration_ms: None,
                exit_code: None,
                workspace: spec.base.display().to_string(),
                devices: spec.devices(),
                plays: spec.plays(),
                label: spec.label.clone(),
                pid: Some(std::process::id()),
                queue_position: None,
                run_dir: spec.run_dir().display().to_string(),
                message: None,
            };
            let _ = store.write_record(&fallback);
            fallback
        }
        Err(error) => {
            return JobOutcome {
                record: failed_record(spec, &error),
                report: None,
                reporter_errors: vec![error],
            }
        }
    };

    if spec.on_busy == OnBusy::Fail && !lease::available(store, &record.devices) {
        let message = format!(
            "device lease busy for {}; --on-busy fail leaves the devices untouched",
            record.devices.join(", ")
        );
        record.status = JobStatus::Error;
        record.finished_at = Some(now_rfc3339());
        record.exit_code = Some(3);
        record.message = Some(message.clone());
        let _ = store.write_record(&record);
        return JobOutcome {
            record,
            report: None,
            reporter_errors: vec![message],
        };
    }

    let shutdown = CancellationToken::new();
    let done = Arc::new(AtomicBool::new(false));
    let _watcher = StopWatcher::start(
        store.clone(),
        spec.id.clone(),
        shutdown.clone(),
        Arc::clone(&done),
    );

    let lease = match lease::acquire(
        store,
        &spec.id,
        &record.devices,
        spec.on_busy,
        |position| {
            if let Ok(Some(mut current)) = store.read(&spec.id) {
                current.queue_position = Some(position);
                let _ = store.write_record(&current);
            }
        },
        &shutdown,
    ) {
        Ok(lease) => lease,
        Err(error) => {
            done.store(true, Ordering::Relaxed);
            record.status = if shutdown.is_cancelled() {
                JobStatus::Stopped
            } else {
                JobStatus::Error
            };
            record.finished_at = Some(now_rfc3339());
            record.exit_code = Some(if shutdown.is_cancelled() { 130 } else { 3 });
            record.message = Some(error.clone());
            let _ = store.write_record(&record);
            return JobOutcome {
                record,
                report: None,
                reporter_errors: vec![error],
            };
        }
    };

    record.status = JobStatus::Running;
    record.started_at = Some(now_rfc3339());
    record.queue_position = None;
    record.pid = Some(std::process::id());
    let _ = store.write_record(&record);

    let (sender, writer) = EventWriter::start(store.clone(), &spec.id);
    let options = ExecOptions {
        config: spec.config.clone(),
        run_id: spec.id.clone(),
        report_dir: spec.report_dir.clone(),
        retries: spec.retries,
        fail_fast: spec.fail_fast,
        token: spec.token.clone(),
        shutdown: shutdown.clone(),
        master_version: spec.master_version.clone(),
        session_options: impresario_clients::agent_client::SessionOptions::default(),
        events: Some(sender),
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let outcome = match runtime {
        Ok(runtime) => {
            let outcome = runtime.block_on(exec::execute(&spec.plan, &options));
            // The executor can leave log-stream tasks alive; dropping the
            // runtime closes their event senders so the writer can finish.
            drop(runtime);
            outcome
        }
        Err(error) => {
            let message = format!("cannot start the async runtime: {error}");
            let report = RunReport {
                schema_version: 2,
                run_id: spec.id.clone(),
                started_at: now_rfc3339(),
                duration_ms: 0,
                status: RunStatus::Error,
                exit_code: 3,
                devices: Vec::new(),
            };
            let _ = std::fs::write(store.console_path(&spec.id), format!("run: {message}\n"));
            writer.join();
            done.store(true, Ordering::Relaxed);
            drop(lease);
            record.status = JobStatus::Error;
            record.finished_at = Some(now_rfc3339());
            record.exit_code = Some(3);
            record.message = Some(message.clone());
            let _ = store.write_record(&record);
            return JobOutcome {
                record,
                report: Some(report),
                reporter_errors: vec![message],
            };
        }
    };
    drop(options);
    writer.join();
    done.store(true, Ordering::Relaxed);
    drop(lease);

    let run_dir = spec.run_dir();
    let file_reporters: Vec<Reporter> = spec
        .reporters
        .iter()
        .copied()
        .filter(|reporter| *reporter != Reporter::Console)
        .collect();
    let mut reporter_errors = reporters::write(&outcome.report, &run_dir, &file_reporters, false);
    let console_text = reporters::console::render(&outcome.report, spec.color);
    if report_console(&spec.reporters) {
        if print_console {
            print!("{console_text}");
            let _ = std::io::stdout().flush();
        }
        let _ = append_text(store, &spec.id, &console_text);
    }
    let json = reporters::json::render(&outcome.report);
    if let Ok(json) = &json {
        let path = store.report_path(&spec.id);
        let _ = std::fs::write(&path, json);
    } else if let Err(error) = json {
        reporter_errors.push(error);
    }

    let mut exit_code = outcome.exit_code;
    if !reporter_errors.is_empty() && exit_code == 0 {
        exit_code = 3;
    }
    let stopped = store.stop_path(&spec.id).exists();
    let status = if stopped {
        JobStatus::Stopped
    } else if exit_code == 0 {
        JobStatus::Passed
    } else if exit_code == 1 {
        JobStatus::Failed
    } else if exit_code == 130 {
        JobStatus::Interrupted
    } else {
        JobStatus::Error
    };
    record.status = status;
    record.finished_at = Some(now_rfc3339());
    record.duration_ms = Some(outcome.report.duration_ms);
    record.exit_code = Some(exit_code);
    record.queue_position = None;
    record.message = Some(summary_line(&outcome.report, exit_code, status));
    let _ = store.write_record(&record);

    JobOutcome {
        record,
        report: Some(outcome.report),
        reporter_errors,
    }
}

fn report_console(reporters: &[Reporter]) -> bool {
    reporters.is_empty() || reporters.contains(&Reporter::Console)
}

fn append_text(store: &JobStore, id: &str, text: &str) -> std::io::Result<()> {
    let path = store.console_path(id);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}

fn failed_record(spec: &JobSpec, message: &str) -> JobRecord {
    JobRecord {
        schema_version: JOB_SCHEMA,
        id: spec.id.clone(),
        status: JobStatus::Error,
        detached: spec.detached,
        created_at: now_rfc3339(),
        started_at: None,
        finished_at: Some(now_rfc3339()),
        duration_ms: None,
        exit_code: Some(3),
        workspace: spec.base.display().to_string(),
        devices: spec.devices(),
        plays: spec.plays(),
        label: spec.label.clone(),
        pid: Some(std::process::id()),
        queue_position: None,
        run_dir: spec.run_dir().display().to_string(),
        message: Some(message.to_string()),
    }
}

fn summary_line(report: &RunReport, exit_code: i32, status: JobStatus) -> String {
    let summary = report.summary();
    format!(
        "{}: {} act(s) passed, {} failed, {} skipped; exit {}",
        status.label(),
        summary.acts_passed,
        summary.acts_failed,
        summary.acts_skipped,
        exit_code
    )
}

// ---------------------------------------------------------------------------
// events and logs
// ---------------------------------------------------------------------------

struct EventWriter {
    handle: Option<std::thread::JoinHandle<()>>,
}

impl EventWriter {
    fn start(store: JobStore, id: &str) -> (std::sync::mpsc::Sender<ExecEvent>, Self) {
        let (sender, receiver) = std::sync::mpsc::channel::<ExecEvent>();
        let path = store.events_path(id);
        let log = store.console_path(id);
        let id = id.to_string();
        let handle = std::thread::spawn(move || {
            let mut events = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok();
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .ok();
            while let Ok(event) = receiver.recv() {
                if let Some(file) = events.as_mut() {
                    if let Ok(line) = serde_json::to_string(&event) {
                        let _ = writeln!(file, "{line}");
                    }
                }
                if let Some(file) = log.as_mut() {
                    let line = event_line(&id, &event);
                    let _ = writeln!(file, "{line}");
                }
            }
        });
        (
            sender,
            Self {
                handle: Some(handle),
            },
        )
    }

    fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn event_line(id: &str, event: &ExecEvent) -> String {
    match event {
        ExecEvent::RunStarted {
            run_id,
            devices,
            acts,
        } => format!(
            "{id}: run {run_id} started on {} device(s), {acts} act(s) per device",
            devices.len()
        ),
        ExecEvent::DeviceStarted { device, platform } => {
            format!("{id}: device {device} ({platform}) started")
        }
        ExecEvent::ActStarted {
            device,
            play,
            act,
            attempt,
            example,
        } => {
            let suffix = if example.is_empty() {
                String::new()
            } else {
                format!(
                    " [{}]",
                    example
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!("{id}: {device} {play} / {act}{suffix} attempt {attempt}")
        }
        ExecEvent::ActFinished {
            device,
            play,
            act,
            status,
            duration_ms,
        } => format!(
            "{id}: {device} {play} / {act} {status} ({} ms)",
            duration_ms
        ),
        ExecEvent::StepStarted {
            device,
            act,
            index,
            label,
            source,
            ..
        } => format!("{id}: {device} {act} step {} {label}: {source}", index + 1),
        ExecEvent::StepFinished {
            device,
            act,
            index,
            status,
            duration_ms,
            error,
            ..
        } => {
            let error = error
                .as_ref()
                .map(|error| format!(" - {error}"))
                .unwrap_or_default();
            format!(
                "{id}: {device} {act} step {} {status} ({} ms){error}",
                index + 1,
                duration_ms
            )
        }
        ExecEvent::LogLine {
            device,
            level,
            source,
            message,
            ..
        } => format!("{id}: [{level}] {device} {source}: {message}"),
        ExecEvent::DeviceFinished {
            device,
            status,
            error,
        } => {
            let error = error
                .as_ref()
                .map(|error| format!(" - {error}"))
                .unwrap_or_default();
            format!("{id}: device {device} {status}{error}")
        }
        ExecEvent::RunFinished {
            status,
            exit_code,
            duration_ms,
        } => format!("{id}: run {status}, exit {exit_code}, {duration_ms} ms"),
    }
}

/// One event parsed from `events.jsonl`; malformed lines are skipped.
pub fn read_events(path: &Path) -> Vec<ExecEvent> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// leases and stop markers
// ---------------------------------------------------------------------------

mod lease {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Owner {
        job: String,
        pid: u32,
        devices: Vec<String>,
        started_ms: u64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct WaitEntry {
        job: String,
        pid: u32,
        devices: Vec<String>,
    }

    pub struct Guard {
        paths: Vec<PathBuf>,
        wait: PathBuf,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
            let _ = std::fs::remove_file(&self.wait);
        }
    }

    fn lease_path(store: &JobStore, device: &str) -> PathBuf {
        store.lease_dir().join(format!(
            "{}.json",
            device
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        ))
    }

    fn wait_path(store: &JobStore, job: &str) -> PathBuf {
        store.lease_dir().join(format!("wait.{job}"))
    }

    fn write_wait(path: &Path, job: &str, devices: &[String]) -> std::io::Result<()> {
        let entry = WaitEntry {
            job: job.to_string(),
            pid: std::process::id(),
            devices: devices.to_vec(),
        };
        let text = serde_json::to_string(&entry)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(path, format!("{text}\n"))
    }

    fn read_wait(path: &Path) -> Option<WaitEntry> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn owner_alive(path: &Path) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(owner) = serde_json::from_str::<Owner>(&text) else {
            return false;
        };
        if owner.job.is_empty() {
            return false;
        }
        pid_alive(owner.pid)
    }

    fn remove_stale_lease(path: &Path) {
        if path.exists() && !owner_alive(path) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn remove_stale_wait(path: &Path) {
        if !path.exists() {
            return;
        }
        let live =
            read_wait(path).is_some_and(|entry| !entry.job.is_empty() && pid_alive(entry.pid));
        if !live {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn available(store: &JobStore, devices: &[String]) -> bool {
        let _ = std::fs::create_dir_all(store.lease_dir());
        for device in devices {
            let path = lease_path(store, device);
            if path.exists() {
                remove_stale_lease(&path);
                if path.exists() {
                    return false;
                }
            }
        }
        true
    }

    pub fn acquire(
        store: &JobStore,
        job: &str,
        devices: &[String],
        policy: OnBusy,
        mut queued: impl FnMut(usize),
        shutdown: &CancellationToken,
    ) -> Result<Guard, String> {
        std::fs::create_dir_all(store.lease_dir()).map_err(|error| {
            format!(
                "cannot create lease directory {}: {error}",
                store.lease_dir().display()
            )
        })?;
        clean_stale(store);
        let wait = wait_path(store, job);
        if devices.is_empty() {
            return Ok(Guard {
                paths: Vec::new(),
                wait,
            });
        }
        let mut devices = devices.to_vec();
        devices.sort();
        devices.dedup();
        write_wait(&wait, job, &devices)
            .map_err(|error| format!("cannot write {}: {error}", wait.display()))?;

        loop {
            if shutdown.is_cancelled() {
                let _ = std::fs::remove_file(&wait);
                return Err("stopped while waiting for a device lease".to_string());
            }
            let mut paths = Vec::new();
            let mut busy = false;
            for device in &devices {
                let path = lease_path(store, device);
                if path.exists() {
                    remove_stale_lease(&path);
                }
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        let owner = Owner {
                            job: job.to_string(),
                            pid: std::process::id(),
                            devices: devices.clone(),
                            started_ms: now_ms(),
                        };
                        if let Ok(text) = serde_json::to_string(&owner) {
                            let _ = file.write_all(text.as_bytes());
                        }
                        paths.push(path);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        busy = true;
                        break;
                    }
                    Err(error) => {
                        for path in &paths {
                            let _ = std::fs::remove_file(path);
                        }
                        let _ = std::fs::remove_file(&wait);
                        return Err(format!("cannot create lease {}: {error}", path.display()));
                    }
                }
            }
            if !busy {
                let _ = std::fs::remove_file(&wait);
                return Ok(Guard { paths, wait });
            }
            for path in &paths {
                let _ = std::fs::remove_file(path);
            }
            if policy == OnBusy::Fail {
                let _ = std::fs::remove_file(&wait);
                return Err(format!(
                    "device lease busy for {}; --on-busy fail leaves the devices untouched",
                    devices.join(", ")
                ));
            }
            let position = queue_position(store, &devices, job);
            queued(position);
            std::thread::sleep(WAIT_POLL);
        }
    }

    fn queue_position(store: &JobStore, devices: &[String], job: &str) -> usize {
        let mut position = 1usize;
        let Ok(entries) = std::fs::read_dir(store.lease_dir()) else {
            return position;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !name.starts_with("wait.") || name == format!("wait.{job}") {
                continue;
            }
            let Some(waiting) = read_wait(&entry.path()) else {
                continue;
            };
            if waiting
                .devices
                .iter()
                .any(|device| devices.iter().any(|ours| ours == device))
            {
                position += 1;
            }
        }
        position
    }

    pub fn clean_stale(store: &JobStore) {
        let Ok(entries) = std::fs::read_dir(store.lease_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                remove_stale_lease(&path);
            } else if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("wait."))
            {
                remove_stale_wait(&path);
            }
        }
    }
}

struct StopWatcher {
    done: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl StopWatcher {
    fn start(
        store: JobStore,
        id: String,
        shutdown: CancellationToken,
        done: Arc<AtomicBool>,
    ) -> Self {
        let stop = store.stop_path(&id);
        let watch_done = Arc::clone(&done);
        let handle = std::thread::spawn(move || {
            while !watch_done.load(Ordering::Relaxed) {
                if stop.exists() {
                    shutdown.cancel();
                    return;
                }
                std::thread::sleep(WAIT_POLL);
            }
        });
        Self {
            done,
            handle: Some(handle),
        }
    }
}

impl Drop for StopWatcher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Cross-platform process liveness by pid.
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: the handle is checked and closed on every path; no pointers
        // outlive the call and the pid comes from our own job record.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code) != 0;
            let _ = CloseHandle(handle);
            ok && code == STILL_ACTIVE as u32
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 performs only the permission/existence check.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

// ---------------------------------------------------------------------------
// daemon
// ---------------------------------------------------------------------------

/// One request per local-socket connection (newline-delimited JSON).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum DaemonRequest {
    Start {
        token: String,
        spec: Box<JobSpec>,
        print_console: bool,
    },
    Ping {
        token: String,
    },
}

/// One response per request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum DaemonResponse {
    Started { id: String },
    Busy { message: String },
    Pong,
    Error { message: String },
}

fn daemon_token_path(home: &Home) -> PathBuf {
    home.jobs_dir().join("daemon.token")
}

fn read_daemon_token(home: &Home) -> Result<String, String> {
    let path = daemon_token_path(home);
    std::fs::read_to_string(&path)
        .map(|text| text.trim().to_string())
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn write_daemon_token(home: &Home) -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("cannot generate a daemon token: {error}"))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = daemon_token_path(home);
    std::fs::write(&path, format!("{token}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    restrict_private(&path);
    Ok(token)
}

#[cfg(unix)]
fn restrict_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn restrict_private(_path: &Path) {}

fn home_hash(home: &Home) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    home.root().to_string_lossy().hash(&mut hasher);
    hasher.finish()
}

#[cfg(windows)]
fn socket_name(home: &Home) -> Result<interprocess::local_socket::Name<'static>, String> {
    use interprocess::local_socket::prelude::*;
    let raw = format!("impresario-{:016x}.sock", home_hash(home));
    raw.to_ns_name::<interprocess::local_socket::GenericNamespaced>()
        .map_err(|error| format!("cannot name the impresario daemon socket: {error}"))
}

#[cfg(unix)]
fn socket_name(home: &Home) -> Result<interprocess::local_socket::Name<'static>, String> {
    use interprocess::local_socket::prelude::*;
    let raw = std::env::temp_dir().join(format!("impresario-{:016x}.sock", home_hash(home)));
    raw.to_fs_name::<interprocess::local_socket::GenericFilePath>()
        .map_err(|error| format!("cannot name the impresario daemon socket: {error}"))
}

fn connect(home: &Home) -> std::io::Result<interprocess::local_socket::Stream> {
    use interprocess::local_socket::prelude::*;
    let name = socket_name(home)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    interprocess::local_socket::Stream::connect(name)
}

fn round_trip(home: &Home, request: &DaemonRequest) -> Result<DaemonResponse, String> {
    let stream = connect(home).map_err(|error| format!("cannot reach the daemon: {error}"))?;
    let mut reader = BufReader::new(stream);
    let line = serde_json::to_string(request)
        .map_err(|error| format!("cannot serialize the daemon request: {error}"))?;
    reader
        .get_mut()
        .write_all(format!("{line}\n").as_bytes())
        .map_err(|error| format!("cannot send the daemon request: {error}"))?;
    reader
        .get_mut()
        .flush()
        .map_err(|error| format!("cannot flush the daemon request: {error}"))?;
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .map_err(|error| format!("cannot read the daemon response: {error}"))?;
    if response.trim().is_empty() {
        return Err("the daemon closed the connection without a response".to_string());
    }
    serde_json::from_str(response.trim())
        .map_err(|error| format!("cannot parse the daemon response: {error}"))
}

/// Auto-start the per-user daemon and submit a detached job.
pub fn submit_detached(home: &Home, spec: JobSpec, print_console: bool) -> Result<String, String> {
    let token = ensure_daemon(home)?;
    match round_trip(
        home,
        &DaemonRequest::Start {
            token,
            spec: Box::new(spec),
            print_console,
        },
    )? {
        DaemonResponse::Started { id } => Ok(id),
        DaemonResponse::Busy { message } | DaemonResponse::Error { message } => Err(message),
        DaemonResponse::Pong => Err("the daemon answered a start request with ping".to_string()),
    }
}

/// Connect to a live daemon, starting one when necessary.
pub fn ensure_daemon(home: &Home) -> Result<String, String> {
    home.ensure_dirs().map_err(|error| error.to_string())?;
    if let Ok(token) = read_daemon_token(home) {
        if round_trip(
            home,
            &DaemonRequest::Ping {
                token: token.clone(),
            },
        )
        .is_ok_and(|response| matches!(response, DaemonResponse::Pong))
        {
            return Ok(token);
        }
    }
    start_daemon(home)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        if let Ok(token) = read_daemon_token(home) {
            match round_trip(
                home,
                &DaemonRequest::Ping {
                    token: token.clone(),
                },
            ) {
                Ok(DaemonResponse::Pong) => return Ok(token),
                Ok(other) => last = format!("unexpected daemon answer: {other:?}"),
                Err(error) => last = error,
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "the impresario daemon did not become ready within 15 s{}{}",
        if last.is_empty() { "" } else { ": " },
        last
    ))
}

#[cfg(windows)]
fn start_daemon(home: &Home) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot locate the impresario executable: {error}"))?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("daemon")
        .arg("--serve")
        .env("IMPRESARIO_HOME", home.root())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    // The daemon must not keep the caller's stdout/stderr pipes alive: a
    // script that captures `impresario run --detach` would otherwise block
    // until the daemon exits. Stdio::null() redirects the child's own
    // handles, but Windows still inherits every *inheritable* handle in this
    // process, including a pipe inherited from `Command::output()`.
    let restore = clear_inherited_std_handles();
    let spawned = command.spawn().map(|_| ());
    restore_std_handles(restore);
    spawned.map_err(|error| format!("cannot start the impresario daemon: {error}"))
}

#[cfg(windows)]
type InheritedHandle = (
    windows_sys::Win32::Foundation::HANDLE,
    windows_sys::Win32::Foundation::HANDLE_FLAGS,
);

/// Clear `HANDLE_FLAG_INHERIT` on this process's standard handles.
#[cfg(windows)]
fn clear_inherited_std_handles() -> Vec<InheritedHandle> {
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    let mut changed = Vec::new();
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle/GetHandleInformation/SetHandleInformation read
        // or update process-local handle flags; the handle values are only
        // used for these calls and are restored below.
        unsafe {
            let handle = GetStdHandle(which);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut flags = 0;
            if GetHandleInformation(handle, &mut flags) == 0 {
                continue;
            }
            if flags & HANDLE_FLAG_INHERIT != 0
                && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0
            {
                changed.push((handle, flags));
            }
        }
    }
    changed
}

#[cfg(windows)]
fn restore_std_handles(handles: Vec<InheritedHandle>) {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    for (handle, flags) in handles {
        // SAFETY: the handle was valid when its flags were captured a moment
        // ago and belongs to this process.
        unsafe {
            let _ = SetHandleInformation(handle, HANDLE_FLAG_INHERIT, flags);
        }
    }
}

#[cfg(unix)]
fn start_daemon(home: &Home) -> Result<(), String> {
    use std::os::unix::process::CommandExt as _;
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot locate the impresario executable: {error}"))?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("daemon")
        .arg("--serve")
        .env("IMPRESARIO_HOME", home.root())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid() is async-signal-safe and only detaches the child from
    // the controlling terminal; no allocation happens after fork.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start the impresario daemon: {error}"))
}

/// Serve the local socket until idle. Called by `impresario daemon`.
pub fn serve(home: &Home, idle: Duration) -> Result<(), String> {
    use interprocess::local_socket::prelude::*;
    use interprocess::local_socket::ListenerOptions;

    home.ensure_dirs().map_err(|error| error.to_string())?;
    let store = JobStore::new(home);
    store.recover_interrupted();
    let token = write_daemon_token(home)?;
    let name = socket_name(home)?;
    let listener = match ListenerOptions::new().name(name).create_sync() {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // A live singleton answers ping; a corpse is removed and retried.
            if round_trip(
                home,
                &DaemonRequest::Ping {
                    token: token.clone(),
                },
            )
            .is_ok_and(|response| matches!(response, DaemonResponse::Pong))
            {
                return Ok(());
            }
            #[cfg(unix)]
            {
                let raw =
                    std::env::temp_dir().join(format!("impresario-{:016x}.sock", home_hash(home)));
                let _ = std::fs::remove_file(raw);
                let name = socket_name(home)?;
                ListenerOptions::new()
                    .name(name)
                    .create_sync()
                    .map_err(|error| format!("cannot listen for jobs: {error}"))?
            }
            #[cfg(not(unix))]
            {
                return Err(format!("cannot listen for jobs: {error}"));
            }
        }
        Err(error) => return Err(format!("cannot listen for jobs: {error}")),
    };

    let active = Arc::new(AtomicU64::new(0));
    let last_active = Arc::new(Mutex::new(std::time::Instant::now()));
    let idle_seconds = idle.as_secs();
    {
        let active = Arc::clone(&active);
        let last_active = Arc::clone(&last_active);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            if idle_seconds > 0
                && active.load(Ordering::Relaxed) == 0
                && last_active
                    .lock()
                    .map(|instant| instant.elapsed() >= idle)
                    .unwrap_or(false)
            {
                std::process::exit(0);
            }
        });
    }

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        *last_active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = std::time::Instant::now();
        let token = token.clone();
        let home = home.clone();
        let store = store.clone();
        let active = Arc::clone(&active);
        let last_active = Arc::clone(&last_active);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let response = match serde_json::from_str::<DaemonRequest>(line.trim()) {
                Ok(DaemonRequest::Ping { token: sent }) if sent == token => DaemonResponse::Pong,
                Ok(DaemonRequest::Ping { .. }) => DaemonResponse::Error {
                    message: "bad daemon token".to_string(),
                },
                Ok(DaemonRequest::Start {
                    token: sent,
                    spec,
                    print_console,
                }) if sent == token => {
                    if spec.on_busy == OnBusy::Fail && !lease::available(&store, &spec.devices()) {
                        DaemonResponse::Busy {
                            message: format!(
                                "device lease busy for {}; --on-busy fail leaves the devices untouched",
                                spec.devices().join(", ")
                            ),
                        }
                    } else {
                        match store.prepare(&spec) {
                            Ok(_) => {
                                active.fetch_add(1, Ordering::Relaxed);
                                *last_active
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    std::time::Instant::now();
                                let job_store = JobStore::new(&home);
                                let job_id = spec.id.clone();
                                let response_id = spec.id.clone();
                                std::thread::spawn(move || {
                                    let _ = run_prepared(&job_store, &spec, print_console);
                                    active.fetch_sub(1, Ordering::Relaxed);
                                    *last_active
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                        std::time::Instant::now();
                                    let _ = job_id;
                                });
                                DaemonResponse::Started { id: response_id }
                            }
                            Err(message) => DaemonResponse::Error { message },
                        }
                    }
                }
                Ok(DaemonRequest::Start { .. }) => DaemonResponse::Error {
                    message: "bad daemon token".to_string(),
                },
                Err(error) => DaemonResponse::Error {
                    message: format!("invalid daemon request: {error}"),
                },
            };
            let text = serde_json::to_string(&response).unwrap_or_else(|_| {
                "{\"response\":\"error\",\"message\":\"serialization failed\"}".to_string()
            });
            let _ = reader.get_mut().write_all(format!("{text}\n").as_bytes());
            let _ = reader.get_mut().flush();
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI support
// ---------------------------------------------------------------------------

/// Poll a job until it becomes terminal; returns its record.
pub fn wait_for(home: &Home, id: &str) -> Result<JobRecord, String> {
    let store = JobStore::new(home);
    loop {
        let Some(record) = store.read(id)? else {
            return Err(format!("unknown job '{id}'"));
        };
        if record.status.terminal() {
            return Ok(record);
        }
        std::thread::sleep(WAIT_POLL);
    }
}

/// Read a job's report if present.
pub fn report(home: &Home, id: &str) -> Result<RunReport, String> {
    let store = JobStore::new(home);
    store.read_report(id).or_else(|first| {
        let record = store
            .read(id)?
            .ok_or_else(|| format!("unknown job '{id}'"))?;
        let path = PathBuf::from(&record.run_dir).join("report.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{first}; cannot read {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("{first}; cannot parse {}: {error}", path.display()))
    })
}

/// Latest failed acts for `run --only-failed`, from the newest report under
/// `<workspace>/<report_output>`. `None` means no report exists yet.
#[must_use]
pub fn last_failed(base: &Path, report_output: &Path) -> Option<BTreeSet<String>> {
    let root = if report_output.is_absolute() {
        report_output.to_path_buf()
    } else {
        base.join(report_output)
    };
    let entries = std::fs::read_dir(&root).ok()?;
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs.reverse();
    for dir in dirs {
        let path = dir.join("report.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(report) = serde_json::from_str::<RunReport>(&text) else {
            continue;
        };
        let mut failed = BTreeSet::new();
        for device in &report.devices {
            for play in &device.plays {
                for act in &play.acts {
                    if act.status == crate::report::Status::Failed {
                        failed.insert(format!("{}\u{1f}{}", play.file, act.name));
                        failed.insert(act.name.clone());
                    }
                }
            }
        }
        return Some(failed);
    }
    None
}

// Keep BTreeMap imported for the public API documentation examples and for
// callers that build a profile from a map.
#[allow(dead_code)]
fn _keep_types() -> BTreeMap<String, String> {
    BTreeMap::new()
}
