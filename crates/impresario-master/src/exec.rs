//! The executor (spec 8.3): drive a compiled [`ExecutionPlan`] across the
//! selected devices and produce the [`RunReport`].
//!
//! One task per device, bounded by `parallel_devices`; within a device Plays
//! and Acts run strictly sequentially. Acts are retried up to `retries`
//! times (each retry restarts the app), a pass after a failure is `flaky`,
//! `--fail-fast` cancels a global token and Ctrl-C cancels the session's
//! in-flight command. Exit codes follow spec 8.3 (0/1/3, 130 interrupted).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use impresario_clients::agent_client::{
    AgentClient, AgentSession, Capability, ClientError, DeviceInfo, HandshakeOutcome,
    HandshakeParams, SessionOptions,
};
use impresario_clients::log::{LogBuffer, LogLevel, LogRecord};
use impresario_clients::transport::{transport_for, DeviceTransport, ResetOutcome};
use impresario_core::apps::AppConfig;
use impresario_core::ast::AppOp;
use impresario_core::command::{Command, CommandOutcome, WireError, WireErrorCode};
use impresario_core::config::{AppReset, DeviceConfig, ImpresarioConfig};
use impresario_core::probe::{ProbeKind, UnverifiablePolicy, Verdict};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::{Id, JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::plan::{DevicePlan, ExecutionPlan, PlannedAct, PlannedStep, PlannedWork};
use crate::redact::Redactor;
use crate::report::{
    ActReport, AttemptReport, DeviceInfoJson, DeviceReport, ErrorJson, FailureReport, PlayReport,
    RunReport, RunStatus, SpanJson, StageReport, Status, StepReport,
};
use crate::time_fmt::{now_millis, rfc3339_utc};

/// Failure-capture screenshot budget (master deadline cap, decision I-24).
const FAILURE_CAPTURE_TIMEOUT_MS: u32 = 10_000;

/// Time given to a cancelled in-flight command during shutdown.
const SHUTDOWN_SETTLE: Duration = Duration::from_secs(3);

/// Time given to the final `Shutdown` RPC.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Log entries attached to a failure (spec 8.3: "the last 200 log entries").
const LOG_TAIL: usize = 200;

/// How long the master waits for `StreamLogs` to open before it continues
/// without agent logs (decision I-74: grpc-java sends response headers only
/// with the first message, so awaiting the open blocks forever on an idle
/// executor whose log bus never replays).
const LOG_STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Agent log ring kept per device (host-side logs live in the transport).
const DEVICE_LOG_CAPACITY: usize = 2000;

/// Everything the executor needs that is not the plan itself.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub config: ImpresarioConfig,
    /// Run id (also the report directory name under `report_dir`).
    pub run_id: String,
    /// Resolved `report_output` root; the run directory is `report_dir/<run_id>`.
    pub report_dir: PathBuf,
    /// Effective retries (CLI beats config).
    pub retries: u32,
    /// `--fail-fast`.
    pub fail_fast: bool,
    /// `IMPRESARIO_AGENT_TOKEN` (spec 7.2.1).
    pub token: Option<String>,
    /// Cancelled on the first Ctrl-C (spec 8.3 Shutdown).
    pub shutdown: CancellationToken,
    /// The master version reported in the handshake.
    pub master_version: String,
    /// Session tunables (tests shrink the deadlines).
    pub session_options: SessionOptions,
    /// Optional live progress stream for a job view (TASK-2 W-CLI).
    pub events: Option<std::sync::mpsc::Sender<ExecEvent>>,
}

/// One live executor event. Every text field is already redacted and safe to
/// persist; `source` intentionally carries the unresolved DSL source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ExecEvent {
    RunStarted {
        run_id: String,
        devices: Vec<String>,
        acts: usize,
    },
    DeviceStarted {
        device: String,
        platform: String,
    },
    ActStarted {
        device: String,
        play: String,
        act: String,
        attempt: u32,
        example: BTreeMap<String, String>,
    },
    ActFinished {
        device: String,
        play: String,
        act: String,
        status: String,
        duration_ms: u64,
    },
    StepStarted {
        device: String,
        play: String,
        act: String,
        stage: String,
        index: usize,
        label: String,
        source: String,
    },
    StepFinished {
        device: String,
        play: String,
        act: String,
        stage: String,
        index: usize,
        status: String,
        duration_ms: u64,
        error: Option<String>,
    },
    LogLine {
        device: String,
        level: String,
        source: String,
        message: String,
        unix_ms: u64,
    },
    DeviceFinished {
        device: String,
        status: String,
        error: Option<String>,
    },
    RunFinished {
        status: String,
        exit_code: i32,
        duration_ms: u64,
    },
}

fn emit(events: Option<&std::sync::mpsc::Sender<ExecEvent>>, event: ExecEvent) {
    if let Some(sender) = events {
        // A disconnected viewer must never affect a run.
        let _ = sender.send(event);
    }
}

impl ExecOptions {
    /// `report_dir/<run-id>`: where reporters and screenshots go (spec 8.4).
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.report_dir.join(&self.run_id)
    }
}

/// What `execute` produced.
#[derive(Debug)]
pub struct ExecOutcome {
    pub report: RunReport,
    pub exit_code: i32,
}

/// State shared by every device task.
struct Shared {
    options: ExecOptions,
    run_dir: PathBuf,
    redactor: Redactor,
    /// Cancelled on the first failure when `--fail-fast` is on.
    fail_fast: CancellationToken,
}

/// Execute one plan. Never fails: everything is reported in the `RunReport`.
pub async fn execute(plan: &ExecutionPlan, options: &ExecOptions) -> ExecOutcome {
    let started_ms = now_millis();
    let run_dir = options.run_dir();
    let shared = Arc::new(Shared {
        options: options.clone(),
        run_dir,
        redactor: Redactor::new(plan.secrets.clone()),
        fail_fast: CancellationToken::new(),
    });
    let permits = usize::try_from(options.config.parallel_devices.max(1)).unwrap_or(1);
    let semaphore = Arc::new(Semaphore::new(permits));

    let devices: Vec<String> = plan
        .devices
        .iter()
        .map(|device| device.device.name.clone())
        .collect();
    let act_count = plan
        .devices
        .first()
        .map(|device| {
            device
                .plays
                .iter()
                .map(|play| play.acts.len())
                .sum::<usize>()
        })
        .unwrap_or(0);
    emit(
        options.events.as_ref(),
        ExecEvent::RunStarted {
            run_id: options.run_id.clone(),
            devices,
            acts: act_count,
        },
    );

    let mut set: JoinSet<(usize, DeviceOutcome)> = JoinSet::new();
    let mut index_of: HashMap<Id, usize> = HashMap::new();
    for (index, device_plan) in plan.devices.iter().enumerate() {
        let shared = Arc::clone(&shared);
        let semaphore = Arc::clone(&semaphore);
        let device_plan = device_plan.clone();
        let handle = set.spawn(async move {
            let permit = semaphore.acquire_owned().await.ok();
            let outcome = run_device(&device_plan, &shared).await;
            drop(permit);
            (index, outcome)
        });
        index_of.insert(handle.id(), index);
    }

    let mut outcomes: Vec<Option<DeviceOutcome>> = plan.devices.iter().map(|_| None).collect();
    while let Some(joined) = set.join_next_with_id().await {
        match joined {
            Ok((_id, (index, outcome))) => {
                if let Some(slot) = outcomes.get_mut(index) {
                    *slot = Some(outcome);
                }
            }
            Err(error) => {
                let index = index_of.get(&error.id()).copied();
                match index.and_then(|index| plan.devices.get(index).map(|plan| (index, plan))) {
                    Some((index, device_plan)) => {
                        if let Some(slot) = outcomes.get_mut(index) {
                            *slot = Some(DeviceOutcome {
                                report: panic_device_report(device_plan, &error.to_string()),
                                infra_zero_failure: false,
                            });
                        }
                    }
                    None => outcomes.push(Some(DeviceOutcome {
                        report: DeviceReport {
                            name: "unknown".to_string(),
                            platform: String::new(),
                            device_info: None,
                            error: Some(ErrorJson {
                                code: "INTERNAL".to_string(),
                                message: format!("a device task panicked: {error}"),
                                details: BTreeMap::new(),
                            }),
                            plays: Vec::new(),
                        },
                        infra_zero_failure: false,
                    })),
                }
            }
        }
    }

    let interrupted = options.shutdown.is_cancelled();
    let mut device_error = false;
    let mut infra_zero = false;
    let mut act_failed = false;
    let mut devices = Vec::new();
    for outcome in outcomes.into_iter().flatten() {
        device_error |= outcome.report.error.is_some();
        infra_zero |= outcome.infra_zero_failure;
        act_failed |= outcome
            .report
            .plays
            .iter()
            .any(|play| play.acts.iter().any(|act| act.status == Status::Failed));
        devices.push(outcome.report);
    }

    let exit_code = if interrupted {
        130
    } else if device_error || infra_zero {
        3
    } else if act_failed {
        1
    } else {
        0
    };
    let status = if interrupted {
        RunStatus::Interrupted
    } else if exit_code == 3 {
        RunStatus::Error
    } else if act_failed {
        RunStatus::Failed
    } else {
        RunStatus::Passed
    };
    let report = RunReport {
        schema_version: 2,
        run_id: options.run_id.clone(),
        started_at: rfc3339_utc(started_ms),
        duration_ms: now_millis().saturating_sub(started_ms),
        status,
        exit_code,
        devices,
    };
    emit(
        options.events.as_ref(),
        ExecEvent::RunFinished {
            status: status.as_str().to_string(),
            exit_code,
            duration_ms: report.duration_ms,
        },
    );
    ExecOutcome { report, exit_code }
}

// ---------------------------------------------------------------------------
// per device
// ---------------------------------------------------------------------------

struct DeviceOutcome {
    report: DeviceReport,
    /// The act's final attempt failed with an infrastructure code while
    /// producing zero wire step results (exit code 3, decision I-21).
    infra_zero_failure: bool,
}

struct Connection {
    session: AgentSession,
    handshake: HandshakeOutcome,
    log_task: Option<JoinHandle<()>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(task) = self.log_task.take() {
            task.abort();
        }
    }
}

struct DeviceRun<'a> {
    transport: &'a mut (dyn DeviceTransport + 'a),
    connection: &'a mut Option<Connection>,
    device: &'a DeviceConfig,
    app: Option<&'a AppConfig>,
    shared: &'a Shared,
    agent_logs: &'a LogBuffer,
    /// Whether a LAUNCH was issued for this app (first reset uses LAUNCH).
    launched: bool,
}

async fn run_device(plan: &DevicePlan, shared: &Shared) -> DeviceOutcome {
    let device = &plan.device;
    let mut transport = transport_for(device.platform);
    let agent_logs = LogBuffer::new(DEVICE_LOG_CAPACITY);
    let mut connection: Option<Connection>;

    let mut device_error: Option<WireError> = None;
    let mut device_info: Option<DeviceInfoJson> = None;
    let mut infra_zero = false;
    let mut done: Vec<Vec<Option<ActReport>>> = plan
        .plays
        .iter()
        .map(|play| play.acts.iter().map(|_| None).collect())
        .collect();

    emit(
        shared.options.events.as_ref(),
        ExecEvent::DeviceStarted {
            device: device.name.clone(),
            platform: device.platform.as_str().to_string(),
        },
    );

    let initial = establish(
        &mut *transport,
        device,
        plan.app.as_ref(),
        shared,
        &agent_logs,
    )
    .await;
    connection = match initial {
        Ok(conn) => Some(conn),
        Err(error) => {
            device_error = Some(error);
            None
        }
    };
    if let Some(conn) = connection.as_ref() {
        device_info = conn.handshake.device.as_ref().map(device_info_json);
    }

    if device_error.is_none() {
        let lacks_app_control = !connection
            .as_ref()
            .is_some_and(|conn| conn.handshake.has_capability(Capability::AppControl));
        if needs_app_control(plan, &shared.options.config) && lacks_app_control {
            device_error = Some(phase(
                WireError::new(
                    WireErrorCode::UnsupportedAction,
                    "the executor does not advertise APP_CONTROL, which app_reset requires",
                ),
                "capabilities",
            ));
        } else {
            let mut run = DeviceRun {
                transport: &mut *transport,
                connection: &mut connection,
                device,
                app: plan.app.as_ref(),
                shared,
                agent_logs: &agent_logs,
                launched: false,
            };
            'plays: for (play_index, play) in plan.plays.iter().enumerate() {
                for (act_index, act) in play.acts.iter().enumerate() {
                    if shared.options.shutdown.is_cancelled() || shared.fail_fast.is_cancelled() {
                        break 'plays;
                    }
                    let outcome = run.run_act(play.name.as_str(), act, act_index == 0).await;
                    if outcome.report.status == Status::Failed && shared.options.fail_fast {
                        shared.fail_fast.cancel();
                    }
                    emit(
                        shared.options.events.as_ref(),
                        ExecEvent::ActFinished {
                            device: device.name.clone(),
                            play: play.name.clone(),
                            act: outcome.report.name.clone(),
                            status: outcome.report.status.as_str().to_string(),
                            duration_ms: outcome
                                .report
                                .attempts
                                .iter()
                                .map(|attempt| attempt.duration_ms)
                                .sum(),
                        },
                    );
                    if let Some(slot) = done
                        .get_mut(play_index)
                        .and_then(|acts| acts.get_mut(act_index))
                    {
                        *slot = Some(outcome.report);
                    }
                    infra_zero |= outcome.infra_zero_failure;
                }
            }
            run.finish().await;
        }
    }

    if let Err(error) = transport.teardown().await {
        tracing::debug!(%error, "transport teardown failed");
    }

    let default_status = if device_error.is_some() || shared.options.shutdown.is_cancelled() {
        Status::Aborted
    } else {
        Status::Skipped
    };
    let plays = assemble_plays(plan, &done, default_status);
    let error = device_error.map(|error| error_json(shared, &error));
    let report = DeviceReport {
        name: device.name.clone(),
        platform: device.platform.as_str().to_string(),
        device_info,
        error,
        plays,
    };
    let status = if report.error.is_some() {
        "error"
    } else if report
        .plays
        .iter()
        .any(|play| play.acts.iter().any(|act| act.status == Status::Failed))
    {
        "failed"
    } else {
        "passed"
    };
    emit(
        shared.options.events.as_ref(),
        ExecEvent::DeviceFinished {
            device: device.name.clone(),
            status: status.to_string(),
            error: report.error.as_ref().map(|error| error.message.clone()),
        },
    );
    DeviceOutcome {
        report,
        infra_zero_failure: infra_zero,
    }
}

/// Connect to the executor: prepare → connect (with retry) → handshake →
/// open session → best-effort log stream (spec 7.3 steps 1-3).
async fn establish(
    transport: &mut dyn DeviceTransport,
    device: &DeviceConfig,
    app: Option<&AppConfig>,
    shared: &Shared,
    agent_logs: &LogBuffer,
) -> Result<Connection, WireError> {
    let endpoint = transport
        .prepare(device, app)
        .await
        .map_err(|error| phase(error.to_wire(), "prepare"))?;
    let client = AgentClient::connect_retry(
        endpoint.address(),
        Duration::from_millis(shared.options.config.agent_start_timeout_ms),
    )
    .await
    .map_err(|error| phase(error.to_wire(), "connect"))?;
    let client = client.with_token(shared.options.token.clone());
    let mut params = HandshakeParams::new(
        device.name.clone(),
        device.platform,
        transport.target_endpoint(device),
        shared.options.master_version.clone(),
    );
    let app_entry = app.and_then(|app| app.platform(device.platform));
    params.app_id = app_entry.and_then(|entry| entry.id.clone());
    params.app_url = app_entry.and_then(|entry| entry.url.clone());
    params.auth_token = shared.options.token.clone();
    params.options = device_wire_options(device, app, &shared.options.config);
    let handshake = client
        .handshake(&params)
        .await
        .map_err(|error| phase(error.to_wire(), "handshake"))?;
    let session = client
        .open_session(
            &handshake.session_id,
            shared.options.session_options.clone(),
        )
        .await
        .map_err(|error| phase(error.to_wire(), "session"))?;
    let log_task = if handshake.has_capability(Capability::Logs) {
        match tokio::time::timeout(
            LOG_STREAM_OPEN_TIMEOUT,
            session.stream_logs(LogLevel::Debug),
        )
        .await
        {
            Ok(Ok(mut stream)) => {
                let buffer = agent_logs.clone();
                let events = shared.options.events.clone();
                let redactor = shared.redactor.clone();
                let device_name = device.name.clone();
                Some(tokio::spawn(async move {
                    while let Some(entry) = stream.next().await {
                        match entry {
                            Ok(record) => {
                                let message = redactor.redact(&record.message);
                                emit(
                                    events.as_ref(),
                                    ExecEvent::LogLine {
                                        device: device_name.clone(),
                                        level: record.level.as_str().to_string(),
                                        source: record.source.clone(),
                                        message,
                                        unix_ms: u64::try_from(record.unix_ms).unwrap_or(0),
                                    },
                                );
                                buffer.push(record);
                            }
                            Err(error) => {
                                tracing::debug!(%error, "agent log stream ended");
                                break;
                            }
                        }
                    }
                }))
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "StreamLogs is unavailable; continuing without agent logs");
                None
            }
            Err(_) => {
                tracing::debug!(
                    "StreamLogs did not open within {} ms; continuing without agent logs",
                    LOG_STREAM_OPEN_TIMEOUT.as_millis()
                );
                None
            }
        }
    } else {
        None
    };
    Ok(Connection {
        session,
        handshake,
        log_task,
    })
}

/// Options sent to the executor in the handshake (spec 7.1 `options` map,
/// decision I-34): the device's own documented options plus the polling and
/// loop bounds executors enforce (5.6) which `Command` does not carry.
fn device_wire_options(
    device: &DeviceConfig,
    app: Option<&AppConfig>,
    config: &ImpresarioConfig,
) -> BTreeMap<String, String> {
    let mut options = BTreeMap::new();
    let device_options = &device.options;
    options.insert(
        "cdp_backend".to_string(),
        device_options.cdp_backend.as_str().to_string(),
    );
    options.insert(
        "key_dispatch".to_string(),
        device_options.key_dispatch.as_str().to_string(),
    );
    options.insert(
        "clear_app_data".to_string(),
        app.is_some_and(|app| app.options.clear_data).to_string(),
    );
    options.insert(
        "animations_disabled".to_string(),
        device_options.animations_disabled.to_string(),
    );
    if let Some(page_match) = &device_options.page_match {
        options.insert("page_match".to_string(), page_match.clone());
    }
    if let Some(port) = device_options.agent_port {
        options.insert("agent_port".to_string(), port.to_string());
    }
    options.insert(
        "poll_interval_ms".to_string(),
        config.poll_interval_ms.to_string(),
    );
    options.insert(
        "scroll_max_attempts".to_string(),
        config.scroll_max_attempts.to_string(),
    );
    options.insert(
        "focus_max_steps".to_string(),
        config.focus_max_steps.to_string(),
    );
    options
}

fn needs_app_control(plan: &DevicePlan, config: &ImpresarioConfig) -> bool {
    config.app_reset != AppReset::Never
        && plan
            .plays
            .iter()
            .any(|play| play.acts.iter().any(|act| !act.stages.is_empty()))
}

fn panic_device_report(plan: &DevicePlan, message: &str) -> DeviceReport {
    DeviceReport {
        name: plan.device.name.clone(),
        platform: plan.device.platform.as_str().to_string(),
        device_info: None,
        error: Some(ErrorJson {
            code: "INTERNAL".to_string(),
            message: format!("the device task panicked: {message}"),
            details: BTreeMap::new(),
        }),
        plays: assemble_plays(plan, &[], Status::Aborted),
    }
}

fn redacted_example(act: &PlannedAct, redactor: Option<&Redactor>) -> BTreeMap<String, String> {
    act.example
        .iter()
        .map(|(key, value)| {
            let value = if is_secret_example_key(key) {
                "***".to_string()
            } else {
                match redactor {
                    Some(redactor) => redactor.redact(value),
                    None => value.clone(),
                }
            };
            (key.clone(), value)
        })
        .collect()
}

fn display_act_name(act: &PlannedAct, redactor: Option<&Redactor>) -> String {
    if act.example.is_empty() {
        return act.name.clone();
    }
    let values = act
        .example
        .iter()
        .map(|(key, value)| {
            let value = if is_secret_example_key(key) {
                "***".to_string()
            } else {
                match redactor {
                    Some(redactor) => redactor.redact(value),
                    None => value.clone(),
                }
            };
            format!("{key}={value}")
        })
        .collect::<Vec<_>>();
    format!("{} [{}]", act.name, values.join(", "))
}

fn is_secret_example_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "pin"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn assemble_plays(
    plan: &DevicePlan,
    done: &[Vec<Option<ActReport>>],
    default_status: Status,
) -> Vec<PlayReport> {
    plan.plays
        .iter()
        .enumerate()
        .map(|(play_index, play)| PlayReport {
            name: play.name.clone(),
            file: play.file.clone(),
            acts: play
                .acts
                .iter()
                .enumerate()
                .map(|(act_index, act)| {
                    done.get(play_index)
                        .and_then(|acts| acts.get(act_index))
                        .and_then(|slot| slot.clone())
                        .unwrap_or_else(|| ActReport {
                            name: display_act_name(act, None),
                            tags: act.tags.clone(),
                            example: redacted_example(act, None),
                            status: default_status,
                            flaky: false,
                            attempts: Vec::new(),
                        })
                })
                .collect(),
        })
        .collect()
}

fn device_info_json(info: &DeviceInfo) -> DeviceInfoJson {
    DeviceInfoJson {
        model: info.model.clone(),
        os_name: info.os_name.clone(),
        os_version: info.os_version.clone(),
        runtime: info.runtime.clone(),
        screen_width_px: info.screen_width_px,
        screen_height_px: info.screen_height_px,
        pixel_ratio: info.pixel_ratio,
    }
}

// ---------------------------------------------------------------------------
// one device, one act
// ---------------------------------------------------------------------------

struct ActOutcome {
    report: ActReport,
    infra_zero_failure: bool,
}

// The outcome carries the full command result (including probe evidence); the
// enum is local to one send and boxing would only add indirection.
#[allow(clippy::large_enum_variant)]
enum SendOutcome {
    /// The command produced a result (or a client-side error) normally.
    Done(Result<CommandOutcome, ClientError>),
    /// The run was interrupted while the command was in flight (spec 8.3).
    Aborted,
}

enum StepOutcome {
    Passed {
        duration_ms: u64,
        wire: bool,
    },
    Failed {
        error: WireError,
        duration_ms: u64,
        wire: bool,
    },
    Unverifiable {
        error: WireError,
        duration_ms: u64,
        wire: bool,
    },
    Aborted {
        duration_ms: u64,
        wire: bool,
    },
}

impl DeviceRun<'_> {
    /// Make sure a live session exists; reconnects with a fresh
    /// prepare/handshake/session when the old one died (spec 8.3 retries).
    async fn ensure_connection(&mut self) -> Result<(), WireError> {
        let alive = self
            .connection
            .as_ref()
            .is_some_and(|connection| !connection.session.is_closed());
        if alive {
            return Ok(());
        }
        if self.connection.is_some() {
            *self.connection = None;
            if let Err(error) = self.transport.teardown().await {
                tracing::debug!(%error, "transport teardown before reconnect failed");
            }
        }
        let connection = establish(
            self.transport,
            self.device,
            self.app,
            self.shared,
            self.agent_logs,
        )
        .await?;
        *self.connection = Some(connection);
        Ok(())
    }

    /// `AppControl{LAUNCH|RESTART, clear_data}` before an Act (spec 7.3
    /// step 4; the first reset of the run issues LAUNCH).
    async fn reset_app(&mut self, clear_data: bool) -> Result<(), WireError> {
        // Host-side clean state before the reset command (I-84): the Apple
        // family cannot wipe another app's sandbox from inside the UI-test
        // runner, so the host reinstalls the app under test. Android wipes the
        // package in the agent and reports `NotRequested` here.
        match self.transport.reset_app_data(self.device, self.app).await {
            Ok(ResetOutcome::Reset) => {
                tracing::debug!(device = %self.device.name, "app reinstalled for a clean state");
            }
            Ok(ResetOutcome::NotRequested) => {}
            Ok(ResetOutcome::Unavailable(reason)) => {
                tracing::warn!(
                    device = %self.device.name,
                    %reason,
                    "clear_app_data degrades to a cold start (I-84)"
                );
            }
            Err(error) => return Err(phase(error.to_wire(), "app_reset")),
        }

        let timeout_ms =
            u32::try_from(self.shared.options.config.timeout_default_ms).unwrap_or(u32::MAX);
        let op = if self.launched {
            AppOp::Restart
        } else {
            AppOp::Launch
        };
        let command = Command::AppControl { op, clear_data };
        let result = {
            let Some(connection) = self.connection.as_ref() else {
                return Err(WireError::new(
                    WireErrorCode::Internal,
                    "no session for app control",
                ));
            };
            if !connection.handshake.has_capability(Capability::AppControl) {
                return Err(phase(
                    WireError::new(
                        WireErrorCode::UnsupportedAction,
                        "the executor does not advertise APP_CONTROL",
                    ),
                    "app_reset",
                ));
            }
            send_wire(self.shared, &connection.session, &command, timeout_ms).await
        };
        match result {
            SendOutcome::Aborted => Err(phase(
                WireError::new(WireErrorCode::Cancelled, "interrupted during app control"),
                "app_reset",
            )),
            SendOutcome::Done(Ok(outcome)) if outcome.ok => {
                self.launched = true;
                Ok(())
            }
            SendOutcome::Done(Ok(outcome)) => Err(phase(
                outcome.error.unwrap_or_else(|| {
                    WireError::new(
                        WireErrorCode::Internal,
                        "app control failed without ErrorInfo",
                    )
                }),
                "app_reset",
            )),
            SendOutcome::Done(Err(error)) => Err(phase(error.to_wire(), "app_reset")),
        }
    }

    async fn run_act(
        &mut self,
        play_name: &str,
        act: &PlannedAct,
        first_act_of_play: bool,
    ) -> ActOutcome {
        let total = self.shared.options.retries.saturating_add(1);
        let mut attempts: Vec<(AttemptReport, usize, Option<WireErrorCode>)> = Vec::new();
        let mut final_status = Status::Failed;

        let mut attempt_n = 0u32;
        while (attempts.len() as u32) < total {
            attempt_n += 1;
            if attempt_n > 1
                && (self.shared.options.shutdown.is_cancelled()
                    || self.shared.fail_fast.is_cancelled())
            {
                break;
            }
            emit(
                self.shared.options.events.as_ref(),
                ExecEvent::ActStarted {
                    device: self.device.name.clone(),
                    play: play_name.to_string(),
                    act: act.name.clone(),
                    attempt: attempt_n,
                    example: redacted_example(act, Some(&self.shared.redactor)),
                },
            );
            let policy = self.shared.options.config.app_reset;
            let reset_due = policy != AppReset::Never
                && (attempt_n > 1 || first_act_of_play || matches!(policy, AppReset::PerAct));

            let setup = match self.ensure_connection().await {
                Err(error) => Err(error),
                Ok(()) if reset_due => {
                    self.reset_app(self.app.is_some_and(|app| app.options.clear_data))
                        .await
                }
                Ok(()) => Ok(()),
            };

            match setup {
                Err(error) => {
                    let report = self.failure_report(0, &error, attempt_n).await;
                    attempts.push((
                        AttemptReport {
                            n: attempt_n,
                            status: Status::Failed,
                            duration_ms: 0,
                            stages: Vec::new(),
                            failure: Some(report),
                        },
                        0,
                        Some(error.code),
                    ));
                }
                Ok(()) => {
                    let outcome = self
                        .run_attempt(play_name, act.name.as_str(), act, attempt_n)
                        .await;
                    let status = outcome.report.status;
                    let code = outcome
                        .report
                        .failure
                        .as_ref()
                        .and_then(|failure| wire_code_of(&failure.code));
                    attempts.push((outcome.report, outcome.wire_results, code));
                    if status != Status::Failed {
                        final_status = status;
                        break;
                    }
                }
            }
        }

        let flaky = final_status == Status::Passed
            && attempts.iter().any(|(a, _, _)| a.status == Status::Failed);
        let infra_zero_failure = final_status == Status::Failed
            && attempts.last().is_some_and(|(_, wire_results, code)| {
                *wire_results == 0 && code.is_some_and(WireErrorCode::is_infrastructure)
            });
        ActOutcome {
            report: ActReport {
                name: display_act_name(act, Some(&self.shared.redactor)),
                tags: act.tags.clone(),
                example: redacted_example(act, Some(&self.shared.redactor)),
                status: final_status,
                flaky,
                attempts: attempts.into_iter().map(|(report, _, _)| report).collect(),
            },
            infra_zero_failure,
        }
    }

    async fn run_attempt(
        &mut self,
        play_name: &str,
        act_name: &str,
        act: &PlannedAct,
        attempt_n: u32,
    ) -> AttemptOutcome {
        let started = Instant::now();
        let mut stages: Vec<StageReport> = Vec::new();
        let mut wire_results = 0usize;
        let mut step_index = 0usize;
        let mut failure: Option<(usize, WireError)> = None;
        let mut aborted = false;

        for stage in &act.stages {
            let mut stage_report = StageReport {
                name: stage.name.clone(),
                span: Some(SpanJson::from(&stage.span)),
                status: Status::Passed,
                steps: Vec::new(),
            };
            for step in &stage.steps {
                let stopping = failure.is_some() || aborted;
                if stopping
                    || self.shared.options.shutdown.is_cancelled()
                    || self.shared.fail_fast.is_cancelled()
                {
                    if !stopping {
                        aborted = true;
                    }
                    stage_report.steps.push(skipped_step(step));
                    step_index += 1;
                    continue;
                }
                emit(
                    self.shared.options.events.as_ref(),
                    ExecEvent::StepStarted {
                        device: self.device.name.clone(),
                        play: play_name.to_string(),
                        act: act_name.to_string(),
                        stage: stage.name.clone(),
                        index: step_index,
                        label: step.label.clone(),
                        source: step.source_text.clone(),
                    },
                );
                let outcome = self.run_step(step).await;
                match outcome {
                    StepOutcome::Passed { duration_ms, wire } => {
                        emit(
                            self.shared.options.events.as_ref(),
                            ExecEvent::StepFinished {
                                device: self.device.name.clone(),
                                play: play_name.to_string(),
                                act: act_name.to_string(),
                                stage: stage.name.clone(),
                                index: step_index,
                                status: "passed".to_string(),
                                duration_ms,
                                error: None,
                            },
                        );
                        stage_report.steps.push(step_report(
                            self.shared,
                            step,
                            Status::Passed,
                            duration_ms,
                            None,
                        ));
                        wire_results += usize::from(wire);
                    }
                    StepOutcome::Failed {
                        error,
                        duration_ms,
                        wire,
                    } => {
                        emit(
                            self.shared.options.events.as_ref(),
                            ExecEvent::StepFinished {
                                device: self.device.name.clone(),
                                play: play_name.to_string(),
                                act: act_name.to_string(),
                                stage: stage.name.clone(),
                                index: step_index,
                                status: "failed".to_string(),
                                duration_ms,
                                error: Some(self.shared.redactor.redact(&error.message)),
                            },
                        );
                        stage_report.steps.push(step_report(
                            self.shared,
                            step,
                            Status::Failed,
                            duration_ms,
                            Some(&error),
                        ));
                        wire_results += usize::from(wire);
                        failure = Some((step_index, error));
                    }
                    StepOutcome::Unverifiable {
                        error,
                        duration_ms,
                        wire,
                    } => {
                        emit(
                            self.shared.options.events.as_ref(),
                            ExecEvent::StepFinished {
                                device: self.device.name.clone(),
                                play: play_name.to_string(),
                                act: act_name.to_string(),
                                stage: stage.name.clone(),
                                index: step_index,
                                status: "unverifiable".to_string(),
                                duration_ms,
                                error: Some(self.shared.redactor.redact(&error.message)),
                            },
                        );
                        stage_report.steps.push(step_report(
                            self.shared,
                            step,
                            Status::Unverifiable,
                            duration_ms,
                            Some(&error),
                        ));
                        wire_results += usize::from(wire);
                    }
                    StepOutcome::Aborted { duration_ms, wire } => {
                        emit(
                            self.shared.options.events.as_ref(),
                            ExecEvent::StepFinished {
                                device: self.device.name.clone(),
                                play: play_name.to_string(),
                                act: act_name.to_string(),
                                stage: stage.name.clone(),
                                index: step_index,
                                status: "aborted".to_string(),
                                duration_ms,
                                error: None,
                            },
                        );
                        stage_report.steps.push(step_report(
                            self.shared,
                            step,
                            Status::Aborted,
                            duration_ms,
                            None,
                        ));
                        wire_results += usize::from(wire);
                        aborted = true;
                    }
                }
                step_index += 1;
            }
            stage_report.status = stage_status(&stage_report);
            stages.push(stage_report);
        }

        let failure = match failure {
            Some((index, error)) => Some(self.failure_report(index, &error, attempt_n).await),
            None => None,
        };
        let status = if aborted {
            Status::Aborted
        } else if failure.is_some() {
            Status::Failed
        } else if stages
            .iter()
            .any(|stage| stage.status == Status::Unverifiable)
        {
            Status::Unverifiable
        } else {
            Status::Passed
        };
        AttemptOutcome {
            report: AttemptReport {
                n: attempt_n,
                status,
                duration_ms: elapsed_ms(started),
                stages,
                failure,
            },
            wire_results,
        }
    }

    async fn run_step(&mut self, step: &PlannedStep) -> StepOutcome {
        let started = Instant::now();
        match &step.work {
            PlannedWork::Wait { duration_ms } => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(*duration_ms)) => StepOutcome::Passed {
                        duration_ms: elapsed_ms(started),
                        wire: false,
                    },
                    _ = self.shared.options.shutdown.cancelled() => StepOutcome::Aborted {
                        duration_ms: elapsed_ms(started),
                        wire: false,
                    },
                }
            }
            PlannedWork::Screenshot { file_name } => {
                self.run_screenshot_step(step, file_name, started).await
            }
            PlannedWork::Wire => self.run_wire_step(step, started).await,
        }
    }

    async fn run_wire_step(&mut self, step: &PlannedStep, started: Instant) -> StepOutcome {
        let Some(command) = step.command.as_ref() else {
            return failed_step(
                WireError::new(
                    WireErrorCode::Internal,
                    "planned wire step without a command",
                ),
                started,
                false,
            );
        };
        let result = {
            let Some(connection) = self.connection.as_ref() else {
                return failed_step(
                    WireError::new(WireErrorCode::Transport, "no open session"),
                    started,
                    false,
                );
            };
            if matches!(command, Command::AppControl { .. })
                && !connection.handshake.has_capability(Capability::AppControl)
            {
                return failed_step(
                    WireError::new(
                        WireErrorCode::UnsupportedAction,
                        "the executor does not advertise APP_CONTROL",
                    ),
                    started,
                    false,
                );
            }
            if let Command::Probe(probe) = command {
                let capability = match probe.kind {
                    ProbeKind::Showing => Capability::ProbeShowing,
                    ProbeKind::Player => Capability::ProbePlayer,
                    ProbeKind::Drm => Capability::ProbeDrm,
                };
                if !connection.handshake.has_capability(capability) {
                    return failed_step(
                        WireError::new(
                            WireErrorCode::UnsupportedAction,
                            format!("the executor does not advertise {}", capability.as_str()),
                        ),
                        started,
                        false,
                    );
                }
            }
            send_wire(self.shared, &connection.session, command, step.timeout_ms).await
        };
        match result {
            SendOutcome::Aborted => StepOutcome::Aborted {
                duration_ms: elapsed_ms(started),
                wire: false,
            },
            SendOutcome::Done(Ok(outcome)) if outcome.ok => {
                if let Some(result) = &outcome.probe {
                    // Agents produce evidence only. Recompute every verdict
                    // here with the core rules so a Kotlin/Swift/JS executor
                    // cannot drift from the reference implementation.
                    let canonical = step
                        .command
                        .as_ref()
                        .and_then(|command| match command {
                            Command::Probe(probe) => Some(canonicalize_probe(probe, result)),
                            _ => None,
                        })
                        .unwrap_or_else(|| result.clone());
                    let result = &canonical;
                    match result.overall() {
                        Verdict::Pass => {}
                        Verdict::Fail => {
                            let summary = result.summary();
                            let mut error = WireError::new(
                                WireErrorCode::CheckFailed,
                                format!("probe failed: {summary}"),
                            );
                            error.details.insert("probe".to_string(), summary);
                            return failed_step(error, started, true);
                        }
                        Verdict::Unverifiable => {
                            let reason = impresario_core::probe::unverifiable_reason(result)
                                .unwrap_or("the required evidence was not available");
                            let summary = result.summary();
                            let mut error = WireError::new(
                                WireErrorCode::Unverifiable,
                                format!("probe unverifiable: {reason}"),
                            );
                            error.details.insert("probe".to_string(), summary);
                            error
                                .details
                                .insert("reason".to_string(), reason.to_string());
                            for (key, value) in &result.observed {
                                error
                                    .details
                                    .insert(format!("observed.{key}"), value.clone());
                            }
                            return match self.shared.options.config.unverifiable_policy {
                                UnverifiablePolicy::Fail => StepOutcome::Failed {
                                    error,
                                    duration_ms: elapsed_ms(started),
                                    wire: true,
                                },
                                UnverifiablePolicy::Warn => StepOutcome::Unverifiable {
                                    error,
                                    duration_ms: elapsed_ms(started),
                                    wire: true,
                                },
                            };
                        }
                    }
                }
                StepOutcome::Passed {
                    duration_ms: elapsed_ms(started),
                    wire: true,
                }
            }
            SendOutcome::Done(Ok(outcome)) => failed_step(
                outcome.error.unwrap_or_else(|| {
                    WireError::new(WireErrorCode::Internal, "command failed without ErrorInfo")
                }),
                started,
                true,
            ),
            SendOutcome::Done(Err(error)) => failed_step(error.to_wire(), started, false),
        }
    }

    async fn run_screenshot_step(
        &mut self,
        step: &PlannedStep,
        file_name: &str,
        started: Instant,
    ) -> StepOutcome {
        let Some(command) = step.command.as_ref() else {
            return failed_step(
                WireError::new(
                    WireErrorCode::Internal,
                    "planned screenshot step without a command",
                ),
                started,
                false,
            );
        };
        let result = {
            let Some(connection) = self.connection.as_ref() else {
                return failed_step(
                    WireError::new(WireErrorCode::Transport, "no open session"),
                    started,
                    false,
                );
            };
            if !connection.handshake.has_capability(Capability::Screenshot) {
                return failed_step(
                    WireError::new(
                        WireErrorCode::UnsupportedAction,
                        "the executor does not advertise SCREENSHOT",
                    ),
                    started,
                    false,
                );
            }
            send_wire(self.shared, &connection.session, command, step.timeout_ms).await
        };
        match result {
            SendOutcome::Aborted => StepOutcome::Aborted {
                duration_ms: elapsed_ms(started),
                wire: false,
            },
            SendOutcome::Done(Ok(outcome)) if outcome.ok => {
                if outcome.screenshot_png.is_empty() {
                    // `ok` without bytes: nothing to write, still a pass.
                    StepOutcome::Passed {
                        duration_ms: elapsed_ms(started),
                        wire: true,
                    }
                } else if write_screenshot(
                    &self.shared.run_dir,
                    &device_dir_name(self.device),
                    file_name,
                    &outcome.screenshot_png,
                )
                .is_some()
                {
                    StepOutcome::Passed {
                        duration_ms: elapsed_ms(started),
                        wire: true,
                    }
                } else {
                    failed_step(
                        WireError::new(
                            WireErrorCode::Internal,
                            "could not write the screenshot file",
                        ),
                        started,
                        true,
                    )
                }
            }
            SendOutcome::Done(Ok(outcome)) => failed_step(
                outcome.error.unwrap_or_else(|| {
                    WireError::new(
                        WireErrorCode::Internal,
                        "screenshot failed without ErrorInfo",
                    )
                }),
                started,
                true,
            ),
            SendOutcome::Done(Err(error)) => failed_step(error.to_wire(), started, false),
        }
    }

    /// The last 200 log entries (host-side + agent), redacted (spec 8.3/5.6).
    fn log_tail(&self) -> Vec<String> {
        let mut records: Vec<LogRecord> = self.transport.logs().snapshot();
        records.extend(self.agent_logs.snapshot());
        records.sort_by_key(|record| record.unix_ms);
        let skip = records.len().saturating_sub(LOG_TAIL);
        records
            .into_iter()
            .skip(skip)
            .map(|record| {
                self.shared.redactor.redact(&format!(
                    "[{}] {}: {}",
                    record.level.as_str(),
                    record.source,
                    record.message
                ))
            })
            .collect()
    }

    /// Failure capture (spec 8.3): error details, a screenshot if enabled,
    /// the last 200 log entries. Never fails the run itself.
    async fn failure_report(
        &mut self,
        step_index: usize,
        error: &WireError,
        attempt_n: u32,
    ) -> FailureReport {
        let screenshot = self.capture_failure_screenshot(attempt_n).await;
        FailureReport {
            step_index,
            code: error.code.as_str().to_string(),
            message: self.shared.redactor.redact(&error.message),
            details: error
                .details
                .iter()
                .map(|(key, value)| (key.clone(), self.shared.redactor.redact(value)))
                .collect(),
            screenshot,
            log_tail: self.log_tail(),
        }
    }

    /// `Screenshot` command next in order; `CaptureScreenshot` when the
    /// session is hung (spec 7.2.9). Best effort.
    async fn capture_failure_screenshot(&mut self, attempt_n: u32) -> Option<String> {
        if !self.shared.options.config.screenshot_on_failure {
            return None;
        }
        let connection = self.connection.as_ref()?;
        if !connection.handshake.has_capability(Capability::Screenshot) {
            return None;
        }
        let png = if connection.session.is_closed() {
            out_of_band_capture(connection).await?
        } else {
            let command = Command::Screenshot {
                label: format!("failure-{attempt_n}"),
            };
            match send_wire(
                self.shared,
                &connection.session,
                &command,
                FAILURE_CAPTURE_TIMEOUT_MS,
            )
            .await
            {
                SendOutcome::Done(Ok(outcome))
                    if outcome.ok && !outcome.screenshot_png.is_empty() =>
                {
                    outcome.screenshot_png
                }
                _ => out_of_band_capture(connection).await?,
            }
        };
        write_screenshot(
            &self.shared.run_dir,
            &device_dir_name(self.device),
            &format!("failure-{attempt_n}"),
            &png,
        )
    }

    /// End of device life (spec 7.3 step 6): `Shutdown`, drop the session
    /// (aborts the log task); the transport teardown follows in the caller.
    async fn finish(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ =
                tokio::time::timeout(SHUTDOWN_TIMEOUT, connection.session.shutdown(false)).await;
            drop(connection);
        }
    }
}

/// Send one command while watching `--shutdown`: on Ctrl-C the in-flight
/// command is cancelled and given `SHUTDOWN_SETTLE` to settle, then the step
/// is reported aborted (spec 8.3 Shutdown).
///
/// A free function (rather than a `DeviceRun` method) so callers keep their
/// borrow of [`DeviceRun::connection`] alive while sending.
async fn send_wire(
    shared: &Shared,
    session: &AgentSession,
    command: &Command,
    timeout_ms: u32,
) -> SendOutcome {
    let grace_ms = shared.options.config.command_grace_ms;
    let send = session.send(command, timeout_ms, grace_ms);
    tokio::pin!(send);
    tokio::select! {
        result = &mut send => SendOutcome::Done(result),
        () = shared.options.shutdown.cancelled() => {
            if let Some(id) = session.in_flight_command_id() {
                let settle = async {
                    let _ = session.cancel(&id).await;
                    let _ = (&mut send).await;
                };
                drop(tokio::time::timeout(SHUTDOWN_SETTLE, settle).await);
            }
            SendOutcome::Aborted
        }
    }
}

struct AttemptOutcome {
    report: AttemptReport,
    wire_results: usize,
}

async fn out_of_band_capture(connection: &Connection) -> Option<Vec<u8>> {
    let capture = tokio::time::timeout(
        Duration::from_millis(u64::from(FAILURE_CAPTURE_TIMEOUT_MS)),
        connection.session.capture_screenshot(),
    )
    .await;
    match capture {
        Ok(Ok(outcome)) if outcome.error.is_none() && !outcome.png.is_empty() => Some(outcome.png),
        _ => None,
    }
}

/// Write a screenshot under `run_dir/screenshots/<device>/` and return its
/// report-relative path (`screenshots/<device>/<name>.png`, spec 8.4).
fn write_screenshot(run_dir: &Path, device: &str, file_name: &str, png: &[u8]) -> Option<String> {
    if png.is_empty() {
        return None;
    }
    let dir = run_dir.join("screenshots").join(device);
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let path = dir.join(format!("{file_name}.png"));
    if std::fs::write(&path, png).is_err() {
        return None;
    }
    Some(format!("screenshots/{device}/{file_name}.png"))
}

fn device_dir_name(device: &DeviceConfig) -> String {
    crate::plan::sanitize_file_name(&device.name)
}

fn stage_status(stage: &StageReport) -> Status {
    if stage.steps.iter().any(|step| step.status == Status::Failed) {
        Status::Failed
    } else if stage
        .steps
        .iter()
        .any(|step| step.status == Status::Aborted)
    {
        Status::Aborted
    } else if stage
        .steps
        .iter()
        .any(|step| step.status == Status::Skipped)
    {
        Status::Skipped
    } else if stage
        .steps
        .iter()
        .any(|step| step.status == Status::Unverifiable)
    {
        Status::Unverifiable
    } else {
        Status::Passed
    }
}

fn skipped_step(step: &PlannedStep) -> StepReport {
    StepReport {
        source: step.source_text.clone(),
        span: SpanJson::from(&step.span),
        status: Status::Skipped,
        duration_ms: 0,
        error: None,
    }
}

fn step_report(
    shared: &Shared,
    step: &PlannedStep,
    status: Status,
    duration_ms: u64,
    error: Option<&WireError>,
) -> StepReport {
    StepReport {
        source: step.source_text.clone(),
        span: SpanJson::from(&step.span),
        status,
        duration_ms,
        error: error.map(|error| error_json(shared, error)),
    }
}

/// Re-evaluate agent evidence with the core claim/assertion rules (D-27).
fn canonicalize_probe(
    command: &impresario_core::command::ProbeCommand,
    result: &impresario_core::probe::ProbeResult,
) -> impresario_core::probe::ProbeResult {
    use impresario_core::probe::{
        evaluate_assertions, evaluate_claim, unverifiable_reason, Confidence, Verdict,
    };

    let mut verdicts: Vec<_> = command
        .claims
        .iter()
        .map(|claim| evaluate_claim(claim, &result.evidence, command.min_confidence))
        .collect();
    verdicts.extend(evaluate_assertions(
        &command.assertions,
        &result.evidence,
        command.min_confidence,
    ));
    let confidence = verdicts
        .iter()
        .map(|verdict| verdict.confidence)
        .max()
        .unwrap_or(Confidence::None);
    let mut canonical = result.clone();
    canonical.verdicts = verdicts;
    canonical.confidence = confidence;
    canonical.unverifiable_reason = canonical
        .verdicts
        .iter()
        .find(|verdict| verdict.verdict == Verdict::Unverifiable)
        .and_then(|verdict| verdict.reason.clone())
        .or_else(|| unverifiable_reason(result).map(str::to_string));
    canonical
}

fn failed_step(error: WireError, started: Instant, wire: bool) -> StepOutcome {
    StepOutcome::Failed {
        error,
        duration_ms: elapsed_ms(started),
        wire,
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn error_json(shared: &Shared, error: &WireError) -> ErrorJson {
    ErrorJson {
        code: error.code.as_str().to_string(),
        message: shared.redactor.redact(&error.message),
        details: error
            .details
            .iter()
            .map(|(key, value)| (key.clone(), shared.redactor.redact(value)))
            .collect(),
    }
}

fn phase(mut error: WireError, phase: &str) -> WireError {
    error.details.insert("phase".to_string(), phase.to_string());
    error
}

/// Parse a report error code back into the wire enum (exit code rule I-21).
fn wire_code_of(code: &str) -> Option<WireErrorCode> {
    WireErrorCode::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_parsing_round_trips_codes() {
        assert_eq!(wire_code_of("TRANSPORT"), Some(WireErrorCode::Transport));
        assert_eq!(
            wire_code_of("CHECK_FAILED"),
            Some(WireErrorCode::CheckFailed)
        );
        assert_eq!(wire_code_of("NOPE"), None);
    }

    #[test]
    fn stage_status_priority() {
        let step = |status| StepReport {
            source: "x".to_string(),
            span: SpanJson {
                file: "p.play".to_string(),
                line: 1,
                col: 1,
                len: 1,
            },
            status,
            duration_ms: 0,
            error: None,
        };
        let stage = |steps: Vec<StepReport>| StageReport {
            name: "S".to_string(),
            span: None,
            status: Status::Passed,
            steps,
        };
        assert_eq!(stage_status(&stage(vec![])), Status::Passed);
        assert_eq!(
            stage_status(&stage(vec![step(Status::Skipped)])),
            Status::Skipped
        );
        assert_eq!(
            stage_status(&stage(vec![step(Status::Skipped), step(Status::Aborted)])),
            Status::Aborted
        );
        assert_eq!(
            stage_status(&stage(vec![step(Status::Passed), step(Status::Failed)])),
            Status::Failed
        );
    }

    #[test]
    fn screenshot_paths_are_relative_and_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_screenshot(dir.path(), "dev_a", "failure-1", b"png-bytes");
        assert_eq!(path.as_deref(), Some("screenshots/dev_a/failure-1.png"));
        let on_disk = dir.path().join("screenshots/dev_a/failure-1.png");
        assert_eq!(std::fs::read(on_disk).expect("read"), b"png-bytes");
        assert!(write_screenshot(dir.path(), "dev_a", "empty", b"").is_none());
    }

    #[test]
    fn phase_details_are_added() {
        let error = phase(WireError::new(WireErrorCode::Transport, "x"), "prepare");
        assert_eq!(
            error.details.get("phase").map(String::as_str),
            Some("prepare")
        );
    }
}
