//! Shared helpers for the master integration suites (spec section 15).
//!
//! `MockAgent` servers must not share a tokio runtime with the CLI (whose
//! `run` command builds its own), so [`TestAgent`] serves each mock from a
//! dedicated thread; exec-level tests use the explicit `#[tokio::test]`
//! runtime helpers instead.

#![allow(dead_code)]

pub mod lsp;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use impresario_clients::agent_client::SessionOptions;
use impresario_clients::testing::{MockAgentBuilder, MockState};
use impresario_core::config::{AppReset, DeviceConfig, DeviceOptions, ImpresarioConfig};
use impresario_core::parser::{parse_play, parse_scene};
use impresario_core::platforms::Platform;
use impresario_core::workspace::Workspace;
use impresario_master::exec::ExecOptions;
use impresario_master::plan::{self, ExecutionPlan, PlanInput, Selection};
use tokio_util::sync::CancellationToken;

/// A scene every test play can reference.
pub const SCENE: &str = r##"Scene: TestScreen
Platform: cross-platform

Pieces:
    A: web="#a", android="id:a", ios="accessibilityId:a"
    B: web="#b", android="id:b", ios="accessibilityId:b"
    Input: web="#input", android="id:input", ios="accessibilityId:input"
"##;

/// One act, three wire steps: validate_check, run_action, run_action.
pub const PLAY_BASIC: &str = r#"Play: Basic

Act: Pass

Stage: S
    Scene: TestScreen
        Check: A
        Action: click => B
        Action: type 'x' => Input
"#;

/// Two single-stage acts for reconnect/fail-fast scenarios.
pub const PLAY_TWO_ACTS: &str = r#"Play: Two Acts

Act: First

Stage: S
    Scene: TestScreen
        Check: A
        Action: click => B

Act: Second

Stage: S
    Scene: TestScreen
        Check: A
"#;

/// A single hanging step with a short effective timeout.
pub const PLAY_HANG: &str = r#"Play: Hang

Act: Hang

Stage: S
    Scene: TestScreen
        Action: click => A timeout 400ms
"#;

/// A screenshot step plus a check (both cross the wire).
pub const PLAY_SHOT: &str = r#"Play: Shot

Act: Capture

Stage: S
    Scene: TestScreen
        Action: screenshot 'shot1'
        Check: A
"#;

/// Two acts with distinct tags for `--tag` / `--act` filtering.
pub const PLAY_TAGGED: &str = r#"Play: Tagged

Act: Smoky
Tags: smoke

Stage: S
    Scene: TestScreen
        Check: A

Act: Plain

Stage: S
    Scene: TestScreen
        Check: A
        Action: click => B
"#;

/// Two acts where the first ends with a long-enough host-side wait that a
/// scripted stream drop (after the first result) has closed the session
/// before the second act needs it — deterministic reconnect.
pub const PLAY_RECONNECT: &str = r#"Play: Reconnect

Act: First

Stage: S
    Scene: TestScreen
        Check: A
        Action: wait 300ms

Act: Second

Stage: S
    Scene: TestScreen
        Check: A
"#;

/// A check whose expected text is a secret (`${env.*}`).
pub const PLAY_SECRET: &str = r#"Play: Secret

Act: Leak

Stage: S
    Scene: TestScreen
        Check: A text '${env.IMPRESARIO_TEST_SECRET}'
"#;

/// A `MockAgent` served from its own OS thread and runtime.
pub struct TestAgent {
    pub address: String,
    pub state: Arc<MockState>,
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestAgent {
    pub fn start(builder: MockAgentBuilder) -> Self {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<(String, Arc<MockState>)>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("mock runtime starts");
            runtime.block_on(async move {
                let agent = builder.start().await.expect("mock starts");
                ready_tx
                    .send((agent.address(), agent.state()))
                    .expect("ready signal");
                let _ = tokio::task::spawn_blocking(move || stop_rx.recv()).await;
                drop(agent);
            });
        });
        let (address, state) = ready_rx.recv().expect("mock address");
        Self {
            address,
            state,
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }
}

impl Drop for TestAgent {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A `web` device pointed directly at a mock address (loopback + open port,
/// so `ConnectorTransport` connects without spawning anything).
pub fn device(name: &str, agent_address: &str) -> DeviceConfig {
    DeviceConfig {
        name: name.to_string(),
        platform: Platform::Web,
        target_endpoint: "launch".to_string(),
        app_id: None,
        app_url: Some("http://localhost/index.html".to_string()),
        agent_address: Some(agent_address.to_string()),
        connector: None,
        options: DeviceOptions::default(),
    }
}

/// Parse an `ImpresarioConfig` from YAML with test-friendly timings.
///
/// `extra` appends more YAML lines; it must not redefine the keys above
/// (`serde` rejects duplicate fields). `parallel_devices` is deliberately
/// absent, so tests can set it (`serde` default is 4).
pub fn test_config(extra: &str) -> ImpresarioConfig {
    let yaml = format!(
        "project: tests\n\
         timeout_default_ms: 2000\n\
         screenshot_on_failure: true\n\
         report_output: ./reports\n\
         command_grace_ms: 200\n\
         agent_start_timeout_ms: 2000\n\
         app_reset: never\n\
         {extra}"
    );
    serde_yaml_ng::from_str(&yaml).expect("test config parses")
}

/// Session tunables for exec-level tests (short settle windows).
pub fn test_session_options() -> SessionOptions {
    SessionOptions {
        heartbeat_timeout: Duration::from_secs(5),
        cancel_wait: Duration::from_millis(200),
        watchdog_poll: Duration::from_millis(20),
    }
}

/// Exec options writing under `<temp>/reports/<run-id>`.
pub fn exec_options(
    temp: &tempfile::TempDir,
    config: ImpresarioConfig,
    run_id: &str,
) -> ExecOptions {
    ExecOptions {
        config,
        run_id: run_id.to_string(),
        report_dir: temp.path().join("reports"),
        retries: 0,
        fail_fast: false,
        token: None,
        shutdown: CancellationToken::new(),
        master_version: "0.1.0-test".to_string(),
        session_options: test_session_options(),
    }
}

/// Compile `play_source` against [`SCENE`] for the given devices.
pub fn build_test_plan(
    config: &ImpresarioConfig,
    devices: &[DeviceConfig],
    play_source: &str,
) -> ExecutionPlan {
    build_test_plan_with(config, devices, play_source, &Selection::default())
}

/// Like [`build_test_plan`] but with an explicit Act selection.
pub fn build_test_plan_with(
    config: &ImpresarioConfig,
    devices: &[DeviceConfig],
    play_source: &str,
    selection: &Selection,
) -> ExecutionPlan {
    let scene = parse_scene(SCENE, Path::new("scenes/TestScreen.scene"))
        .ast
        .expect("test scene parses");
    let play = parse_play(play_source, Path::new("plays/test.play"))
        .ast
        .expect("test play parses");
    let workspace = Workspace {
        scenes: vec![scene],
        plays: vec![play.clone()],
    };
    plan::build_plan(&PlanInput {
        config,
        workspace: &workspace,
        devices,
        plays: std::slice::from_ref(&play),
        selection,
        cli_vars: &BTreeMap::new(),
        cli_timeout: None,
    })
    .expect("plan builds")
}

/// A process-wide empty global home for tests.
///
/// `run_with` and the spawned binary read `IMPRESARIO_HOME`; without this a
/// developer's real `~/.impresario/config.yaml` would leak into the suite.
/// The directory is created once and never removed (it is small and the
/// process id makes it unique).
pub fn test_home() -> &'static Path {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join(format!(
            "impresario-test-home-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create test home");
        std::env::set_var("IMPRESARIO_HOME", &home);
        home
    })
}

/// Write a complete CLI workspace: both configs, the shared scene and one
/// `.play` file per `(file name, source)` pair.
pub fn write_workspace(
    dir: &Path,
    config_extra: &str,
    devices: &[(String, String)],
    plays: &[(&str, &str)],
) {
    test_home();
    std::fs::create_dir_all(dir.join("scenes")).expect("mkdir scenes");
    std::fs::create_dir_all(dir.join("plays")).expect("mkdir plays");
    let config = format!(
        "project: tests\n\
         timeout_default_ms: 2000\n\
         screenshot_on_failure: true\n\
         report_output: ./reports\n\
         command_grace_ms: 200\n\
         agent_start_timeout_ms: 2000\n\
         app_reset: never\n\
         {config_extra}"
    );
    std::fs::write(dir.join("impresario.config"), config).expect("write config");
    let mut yaml = String::from("devices:\n");
    for (name, address) in devices {
        yaml.push_str(&format!(
            "  - name: {name}\n    platform: web\n    target_endpoint: launch\n    agent_address: {address}\n"
        ));
    }
    std::fs::write(dir.join("impresario.devices.config"), yaml).expect("write devices");
    std::fs::write(dir.join("scenes/TestScreen.scene"), SCENE).expect("write scene");
    for (file, source) in plays {
        std::fs::write(dir.join("plays").join(file), source).expect("write play");
    }
}

/// `argv` for `run_with` with a workspace.
pub fn argv(workspace: &Path, rest: &[&str]) -> Vec<String> {
    test_home();
    let mut args = vec![
        "impresario".to_string(),
        "--workspace".to_string(),
        workspace.display().to_string(),
    ];
    args.extend(rest.iter().map(|s| (*s).to_string()));
    args
}

/// The single run directory under `<workspace>/reports`.
pub fn run_dir(workspace: &Path) -> std::path::PathBuf {
    let reports = workspace.join("reports");
    let mut entries: Vec<_> = std::fs::read_dir(&reports)
        .expect("reports dir exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one run dir: {entries:?}"
    );
    entries.remove(0)
}

/// Load the `report.json` of the single run under `<workspace>/reports`.
pub fn read_report_json(workspace: &Path) -> serde_json::Value {
    let text =
        std::fs::read_to_string(run_dir(workspace).join("report.json")).expect("report.json");
    serde_json::from_str(&text).expect("report.json parses")
}

/// A config with `app_reset` overridden, for retry/reset tests.
pub fn with_app_reset(mut config: ImpresarioConfig, reset: AppReset) -> ImpresarioConfig {
    config.app_reset = reset;
    config
}
