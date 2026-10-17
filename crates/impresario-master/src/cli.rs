//! The `impresario` CLI (spec sections 4.4 and 8.1).
//!
//! This module owns argument parsing, exit codes and the Phase-1 commands
//! (`init`, `validate`, `parse`, `devices list`). The executor-facing
//! commands (`devices check`, `run`, `lsp`) are wired here and filled in by
//! later phases; until then they fail loudly instead of pretending.

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use impresario_clients::agent_client::{AgentClient, DeviceInfo, HandshakeParams};
use impresario_clients::transport::{transport_for, DeviceTransport};
use impresario_core::config::{self, DeviceConfig, ImpresarioConfig};
use impresario_core::diag::{Code, Diagnostic, Diagnostics};
use impresario_core::parser::{parse_play, parse_scene};
use impresario_core::validation::{self, DeviceRef};
use impresario_core::workspace;
use tokio_util::sync::CancellationToken;

use crate::{exec, plan, render, reporters, time_fmt};

/// Exit codes (spec section 8.3): 0 pass · 1 test failure · 2 config/DSL ·
/// 3 infrastructure · 130 interrupted.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_INFRA: i32 = 3;
pub const EXIT_INTERRUPTED: i32 = 130;

/// Cross-platform UI test automation.
#[derive(Debug, Parser)]
#[command(name = "impresario", version, about, long_about = None)]
pub struct Cli {
    /// Path to impresario.config (default: <workspace>/impresario.config).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Path to impresario.devices.config (default: <workspace>/impresario.devices.config).
    #[arg(long, global = true, value_name = "PATH")]
    pub devices_config: Option<PathBuf>,
    /// Workspace directory relative paths resolve against (default: cwd).
    #[arg(long, global = true, value_name = "DIR")]
    pub workspace: Option<PathBuf>,
    /// More logging (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Log errors only.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Disable colored diagnostics.
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write the example workspace (impresario.config, devices, scenes, plays).
    Init {
        /// Overwrite files that already exist.
        #[arg(long)]
        force: bool,
    },
    /// Parse and validate the workspace without touching any device.
    Validate {
        /// Files or directories to validate (default: the configured dirs).
        paths: Vec<PathBuf>,
    },
    /// Parse one .scene or .play file.
    Parse {
        file: PathBuf,
        /// Dump the typed AST as JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Device utilities.
    Devices {
        #[command(subcommand)]
        command: DevicesCommand,
    },
    /// Plan and execute plays on devices (spec section 8.2/8.3).
    Run(RunArgs),
    /// Run the embedded language server on stdio (spec section 13).
    Lsp,
}

#[derive(Debug, Subcommand)]
pub enum DevicesCommand {
    /// List the configured devices.
    List,
    /// Transport prepare + handshake + DeviceInfo, then teardown.
    Check {
        /// Device names (default: all).
        names: Vec<String>,
    },
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Play file, directory or glob (repeatable; default: plays_dir).
    #[arg(long, value_name = "FILE|DIR|GLOB")]
    pub play: Vec<String>,
    /// Device name (repeatable) or `all`.
    #[arg(long, value_name = "NAME")]
    pub device: Vec<String>,
    /// Only run acts with this tag (repeatable; OR).
    #[arg(long, value_name = "TAG")]
    pub tag: Vec<String>,
    /// Exclude acts with this tag (wins over --tag).
    #[arg(long, value_name = "TAG")]
    pub exclude_tag: Vec<String>,
    /// Only run acts with this exact name (repeatable).
    #[arg(long, value_name = "NAME")]
    pub act: Vec<String>,
    /// Variable override K=V (repeatable, beats play vars).
    #[arg(long, value_name = "K=V")]
    pub var: Vec<String>,
    /// Default poll timeout in ms (beats timeout_default_ms).
    #[arg(long, value_name = "MS")]
    pub timeout: Option<u64>,
    /// Retry failed acts up to n times (beats config retries).
    #[arg(long, value_name = "N")]
    pub retries: Option<u32>,
    /// Reporters: comma list of console, junit, json, html.
    #[arg(long, value_name = "LIST")]
    pub reporter: Option<String>,
    /// Stop the whole run at the first failure.
    #[arg(long)]
    pub fail_fast: bool,
}

/// Entry point used by `main.rs`.
#[must_use]
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    run_with(&args)
}

/// Entry point that takes the argv vector (tests call this directly).
#[must_use]
pub fn run_with(argv: &[String]) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) => {
            let code = err.exit_code();
            let _ = err.print();
            return if code == 0 { EXIT_OK } else { EXIT_USAGE };
        }
    };
    render::install_hook(cli.no_color);
    init_logging(cli.verbose, cli.quiet);

    let base = workspace_base(&cli.workspace);
    let config_path = resolve_path(&base, &cli.config, "impresario.config");
    let devices_path = resolve_path(&base, &cli.devices_config, "impresario.devices.config");

    match &cli.command {
        Command::Init { force } => cmd_init(&base, *force),
        Command::Validate { paths } => cmd_validate(&base, &config_path, &devices_path, paths),
        Command::Parse { file, json } => cmd_parse(&base, file, *json),
        Command::Devices { command } => match command {
            DevicesCommand::List => cmd_devices_list(&devices_path),
            DevicesCommand::Check { names } => {
                cmd_devices_check(&config_path, &devices_path, names)
            }
        },
        Command::Run(args) => cmd_run(&base, &config_path, &devices_path, args, cli.no_color),
        Command::Lsp => cmd_lsp(),
    }
}

fn init_logging(verbose: u8, quiet: bool) {
    static LOGGING: Once = Once::new();
    LOGGING.call_once(|| {
        let default = if quiet {
            "error"
        } else if verbose > 0 {
            "debug"
        } else {
            "warn"
        };
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    });
}

fn workspace_base(workspace: &Option<PathBuf>) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match workspace {
        Some(dir) if dir.is_absolute() => dir.clone(),
        Some(dir) => cwd.join(dir),
        None => cwd,
    }
}

fn resolve_path(base: &Path, path: &Option<PathBuf>, default: &str) -> PathBuf {
    match path {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => base.join(path),
        None => base.join(default),
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

const EXAMPLE_CONFIG: &str = "\
project: impresario_tests
timeout_default_ms: 10000
screenshot_on_failure: true
report_output: ./reports
";

const EXAMPLE_DEVICES: &str = "\
devices:
  - name: living_room_tv
    platform: webos
    target_endpoint: ws://192.168.1.50:9998
    app_url: http://localhost:8080/index.html   # optional
  - name: flagship_android
    platform: android
    target_endpoint: emulator-5554
    app_id: com.example.shop
  - name: shield_tv
    platform: androidtv
    target_endpoint: 192.168.1.60:5555
    app_id: com.example.shop.tv
  - name: test_iphone
    platform: ios
    target_endpoint: 00008110-001A2345021E001E
    app_id: com.example.shop
  - name: apple_tv
    platform: tvos
    target_endpoint: 00008110-001B3456021E002F
    agent_address: 192.168.1.70:50062
    app_id: com.example.shop.tv
  # A local Chromium instance started by the connector; uncomment to try the
  # example on this machine (spec section 4.2).
  # - name: local_chrome
  #   platform: web
  #   target_endpoint: launch
  #   app_url: http://localhost:8080/index.html
";

const EXAMPLE_LOGIN_SCENE: &str = "\
Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginTitle:    web=\"h1.title\",  android=\"id:login_title\",  ios=\"accessibilityId:login_title\"
    UsernameInput: web=\"#username\", android=\"id:username\",     ios=\"accessibilityId:username\"
    PasswordInput: web=\"#password\", android=\"id:password\",     ios=\"accessibilityId:password\"
    SubmitButton:  web=\"button[type='submit']\", android=\"id:login_btn\", ios=\"accessibilityId:login\"
";

const EXAMPLE_DASHBOARD_SCENE: &str = "\
Scene: DashboardScreen
Platform: cross-platform

Pieces:
    WelcomeBanner: web=\"h2.welcome\", android=\"id:welcome_banner\", ios=\"accessibilityId:welcome_banner\"
    LogoutButton:  web=\"#logout\",    android=\"id:logout_btn\",      ios=\"accessibilityId:logout\"
";

const EXAMPLE_LOGIN_PLAY: &str = "\
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
Var: USER = 'test_user_impresario'

Act: Successful Login Flow
Tags: smoke, auth
Platforms: android, ios, webos

Stage: Navigate and Login
    Timeout: 15s
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${var.USER}' => UsernameInput
        Action: '${env.TEST_PASSWORD}' => PasswordInput
        Action: click => SubmitButton

Stage: Verify Outcome
    Scene: DashboardScreen
        Check: WelcomeBanner is visible timeout 20s
";

fn cmd_init(base: &Path, force: bool) -> i32 {
    let files: [(&str, &str); 5] = [
        ("impresario.config", EXAMPLE_CONFIG),
        ("impresario.devices.config", EXAMPLE_DEVICES),
        ("scenes/LoginScreen.scene", EXAMPLE_LOGIN_SCENE),
        ("scenes/DashboardScreen.scene", EXAMPLE_DASHBOARD_SCENE),
        ("plays/login.play", EXAMPLE_LOGIN_PLAY),
    ];
    let mut status = EXIT_OK;
    for (relative, content) in files {
        let target = relative
            .split('/')
            .fold(base.to_path_buf(), |path, part| path.join(part));
        if target.exists() && !force {
            eprintln!(
                "init: refusing to overwrite {} (use --force)",
                target.display()
            );
            status = EXIT_USAGE;
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("init: cannot create {}: {err}", parent.display());
                status = EXIT_USAGE;
                continue;
            }
        }
        match std::fs::write(&target, content) {
            Ok(()) => println!("created {}", target.display()),
            Err(err) => {
                eprintln!("init: cannot write {}: {err}", target.display());
                status = EXIT_USAGE;
            }
        }
    }
    // .gitignore gains reports/ (created when needed).
    let gitignore = base.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    let has_entry = existing.lines().any(|line| line.trim() == "reports/");
    if !has_entry {
        let mut text = existing;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("reports/\n");
        match std::fs::write(&gitignore, text) {
            Ok(()) => println!("updated {}", gitignore.display()),
            Err(err) => {
                eprintln!("init: cannot write {}: {err}", gitignore.display());
                status = EXIT_USAGE;
            }
        }
    }
    if status == EXIT_OK {
        println!("init: example workspace ready; run `impresario validate` next");
    }
    status
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

fn cmd_validate(base: &Path, config_path: &Path, devices_path: &Path, paths: &[PathBuf]) -> i32 {
    let mut all = Diagnostics::new();

    let cfg_outcome = config::load_config(config_path);
    all.extend(cfg_outcome.diagnostics.iter().cloned());
    let Some(cfg) = cfg_outcome.config else {
        render::eprint_all(&all);
        eprintln!(
            "validate: configuration errors in {}",
            config_path.display()
        );
        return EXIT_USAGE;
    };

    let devices_outcome = config::load_devices(devices_path);
    all.extend(devices_outcome.diagnostics.iter().cloned());
    let Some(devices) = devices_outcome.config else {
        render::eprint_all(&all);
        eprintln!(
            "validate: configuration errors in {}",
            devices_path.display()
        );
        return EXIT_USAGE;
    };

    if let Some(diag) = config::check_report_output(&base.join(&cfg.report_output)) {
        all.push(diag);
    }

    let loaded = if paths.is_empty() {
        workspace::load_dirs(&base.join(&cfg.scenes_dir), &base.join(&cfg.plays_dir))
    } else {
        let resolved: Vec<PathBuf> = paths
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    base.join(path)
                }
            })
            .collect();
        let mut explicit = workspace::load_paths(&resolved);
        // Validation of a play needs the scene index: when no scene was
        // given explicitly, resolve against the configured scenes dir.
        if explicit.workspace.scenes.is_empty() {
            let scenes = workspace::load_scenes(&base.join(&cfg.scenes_dir));
            explicit.workspace.scenes = scenes.workspace.scenes;
            explicit.diagnostics.extend(scenes.diagnostics);
        }
        explicit
    };
    all.extend(loaded.diagnostics.iter().cloned());

    let device_refs: Vec<DeviceRef> = devices
        .devices
        .iter()
        .map(|device| DeviceRef::new(device.name.clone(), device.platform))
        .collect();
    let semantic = validation::validate(&loaded.workspace, &device_refs);
    all.extend(semantic.iter().cloned());

    render::eprint_all(&all);
    let errors = all.errors().count();
    let warnings = all.warnings().count();
    if errors > 0 {
        eprintln!("validate: {errors} error(s), {warnings} warning(s)");
        return EXIT_USAGE;
    }
    println!(
        "validate: ok ({} scene(s), {} play(s), {} device(s), {} warning(s))",
        loaded.workspace.scenes.len(),
        loaded.workspace.plays.len(),
        devices.devices.len(),
        warnings
    );
    EXIT_OK
}

// ---------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------

fn cmd_parse(base: &Path, file: &Path, json: bool) -> i32 {
    let path = if file.is_absolute() {
        file.to_path_buf()
    } else {
        base.join(file)
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("parse: cannot read {}: {err}", path.display());
            return EXIT_USAGE;
        }
    };
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    match extension {
        "scene" => {
            let outcome = parse_scene(&source, &path);
            report_parse(
                json,
                &outcome.diagnostics,
                outcome.ast.as_ref().map(serde_json::to_value),
            )
        }
        "play" => {
            let outcome = parse_play(&source, &path);
            report_parse(
                json,
                &outcome.diagnostics,
                outcome.ast.as_ref().map(serde_json::to_value),
            )
        }
        _ => {
            eprintln!(
                "parse: {} is neither a .scene nor a .play file",
                path.display()
            );
            EXIT_USAGE
        }
    }
}

fn report_parse(
    json: bool,
    diagnostics: &Diagnostics,
    ast: Option<Result<serde_json::Value, serde_json::Error>>,
) -> i32 {
    render::eprint_all(diagnostics);
    if diagnostics.has_errors() {
        eprintln!("parse: {} error(s)", diagnostics.errors().count());
        return EXIT_USAGE;
    }
    if json {
        match ast {
            Some(Ok(value)) => match serde_json::to_string_pretty(&value) {
                Ok(text) => println!("{text}"),
                Err(err) => {
                    eprintln!("parse: cannot serialize the AST: {err}");
                    return EXIT_FAILURE;
                }
            },
            Some(Err(err)) => {
                eprintln!("parse: cannot serialize the AST: {err}");
                return EXIT_FAILURE;
            }
            None => {
                eprintln!("parse: no AST produced");
                return EXIT_USAGE;
            }
        }
    } else {
        println!("parse: ok ({} warning(s))", diagnostics.warnings().count());
    }
    EXIT_OK
}

// ---------------------------------------------------------------------------
// devices
// ---------------------------------------------------------------------------

fn cmd_devices_list(devices_path: &Path) -> i32 {
    let outcome = config::load_devices(devices_path);
    render::eprint_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        eprintln!(
            "devices list: configuration errors in {}",
            devices_path.display()
        );
        return EXIT_USAGE;
    };
    println!("{:<20} {:<10} TARGET", "NAME", "PLATFORM");
    for device in &devices.devices {
        println!(
            "{:<20} {:<10} {}",
            device.name,
            device.platform.as_str(),
            device.target_endpoint
        );
    }
    EXIT_OK
}

fn cmd_devices_check(config_path: &Path, devices_path: &Path, names: &[String]) -> i32 {
    let config_outcome = config::load_config(config_path);
    render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        eprintln!(
            "devices check: configuration errors in {}",
            config_path.display()
        );
        return EXIT_USAGE;
    };
    let outcome = config::load_devices(devices_path);
    render::eprint_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        eprintln!(
            "devices check: configuration errors in {}",
            devices_path.display()
        );
        return EXIT_USAGE;
    };
    let selected = match select_devices(&devices.devices, names) {
        Ok(selected) => selected,
        Err(message) => {
            eprintln!("devices check: {message}");
            return EXIT_USAGE;
        }
    };
    if selected.is_empty() {
        eprintln!(
            "devices check: no devices configured (see {})",
            devices_path.display()
        );
        return EXIT_USAGE;
    }

    let token = agent_token();
    let master_version = env!("CARGO_PKG_VERSION").to_string();
    let timeout_ms = config.agent_start_timeout_ms;

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("devices check: cannot start the async runtime: {err}");
            return EXIT_INFRA;
        }
    };
    let results: Vec<Result<DeviceInfo, String>> = runtime.block_on(async {
        let mut results = Vec::new();
        for device in &selected {
            results.push(check_device(device, timeout_ms, token.as_deref(), &master_version).await);
        }
        results
    });

    let mut failures = 0usize;
    for (device, result) in selected.iter().zip(results) {
        match result {
            Ok(info) => println!(
                "{} ({}): {} · {} {} · {}x{} @{}x · {}",
                device.name,
                device.platform.as_str(),
                info.model,
                info.os_name,
                info.os_version,
                info.screen_width_px,
                info.screen_height_px,
                info.pixel_ratio,
                info.runtime
            ),
            Err(message) => {
                failures += 1;
                eprintln!(
                    "{} ({}): FAILED — {message}",
                    device.name,
                    device.platform.as_str()
                );
            }
        }
    }
    if failures > 0 {
        eprintln!(
            "devices check: {failures} of {} device(s) failed",
            selected.len()
        );
        return EXIT_INFRA;
    }
    println!("devices check: {} device(s) ok", selected.len());
    EXIT_OK
}

/// Transport prepare → connect → handshake → `DeviceInfo` → session shutdown,
/// then teardown (spec 8.1); every failure is a message, never a panic.
async fn check_device(
    device: &DeviceConfig,
    timeout_ms: u64,
    token: Option<&str>,
    master_version: &str,
) -> Result<DeviceInfo, String> {
    let mut transport = transport_for(device.platform);
    let outcome = check_transport(&mut *transport, device, timeout_ms, token, master_version).await;
    let teardown = transport
        .teardown()
        .await
        .map_err(|error| error.to_string());
    match outcome {
        Ok(info) => teardown.map(|()| info),
        Err(message) => {
            let _ = teardown;
            Err(message)
        }
    }
}

async fn check_transport(
    transport: &mut dyn DeviceTransport,
    device: &DeviceConfig,
    timeout_ms: u64,
    token: Option<&str>,
    master_version: &str,
) -> Result<DeviceInfo, String> {
    let endpoint = transport
        .prepare(device)
        .await
        .map_err(|error| error.to_string())?;
    let client = AgentClient::connect_retry(endpoint.address(), Duration::from_millis(timeout_ms))
        .await
        .map_err(|error| error.to_string())?;
    let client = client.with_token(token.map(str::to_string));
    let mut params = HandshakeParams::new(
        device.name.clone(),
        device.platform,
        device.target_endpoint.clone(),
        master_version,
    );
    params.app_id = device.app_id.clone();
    params.app_url = device.app_url.clone();
    params.auth_token = token.map(str::to_string);
    let handshake = client
        .handshake(&params)
        .await
        .map_err(|error| error.to_string())?;
    if !handshake.accepted {
        return Err(handshake
            .error
            .map(|error| format!("{}: {}", error.code.as_str(), error.message))
            .unwrap_or_else(|| "the executor rejected the handshake".to_string()));
    }
    let _ = client.shutdown_session(&handshake.session_id, false).await;
    handshake
        .device
        .ok_or_else(|| "the executor did not report DeviceInfo".to_string())
}

/// The devices an invocation selected: `all` (or nothing) means everything,
/// otherwise the names in the order given (duplicates collapse).
fn select_devices(
    devices: &[DeviceConfig],
    requested: &[String],
) -> Result<Vec<DeviceConfig>, String> {
    if requested.is_empty() || requested.iter().any(|name| name == "all") {
        return Ok(devices.to_vec());
    }
    let mut selected: Vec<DeviceConfig> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    for name in requested {
        match devices.iter().find(|device| &device.name == name) {
            Some(device) => {
                if !selected.iter().any(|chosen| chosen.name == device.name) {
                    selected.push(device.clone());
                }
            }
            None => unknown.push(name.as_str()),
        }
    }
    if !unknown.is_empty() {
        let known: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
        return Err(format!(
            "unknown device(s): {} (configured: {})",
            unknown.join(", "),
            if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            }
        ));
    }
    Ok(selected)
}

/// `IMPRESARIO_AGENT_TOKEN` (spec 7.2.1); empty values count as unset.
fn agent_token() -> Option<String> {
    std::env::var("IMPRESARIO_AGENT_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
}

/// First Ctrl-C cancels the run; a second one exits immediately (spec 8.3).
async fn watch_interrupts(shutdown: CancellationToken) {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::debug!("Ctrl-C is unavailable; the watcher stops");
            return;
        }
        if shutdown.is_cancelled() {
            std::process::exit(EXIT_INTERRUPTED);
        }
        eprintln!("run: interrupted; cancelling in-flight commands (Ctrl-C again to exit now)");
        shutdown.cancel();
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

fn cmd_run(
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    args: &RunArgs,
    no_color: bool,
) -> i32 {
    use std::io::IsTerminal;

    let config_outcome = config::load_config(config_path);
    render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        eprintln!("run: configuration errors in {}", config_path.display());
        return EXIT_USAGE;
    };
    let devices_outcome = config::load_devices(devices_path);
    render::eprint_all(&devices_outcome.diagnostics);
    let Some(device_list) = devices_outcome.config else {
        eprintln!("run: configuration errors in {}", devices_path.display());
        return EXIT_USAGE;
    };

    let reporter_list = match &args.reporter {
        Some(spec) => match reporters::parse_list(spec) {
            Ok(list) => list,
            Err(message) => {
                eprintln!("run: {message}");
                return EXIT_USAGE;
            }
        },
        None => config.reporters.clone(),
    };
    if let Some(timeout) = args.timeout {
        if !plan::timeout_in_bounds(timeout) {
            eprintln!("run: --timeout must be between 100 and 600000 ms (got {timeout})");
            return EXIT_USAGE;
        }
    }
    if let Some(retries) = args.retries {
        if !plan::retries_in_bounds(retries) {
            eprintln!("run: --retries must be between 0 and 5 (got {retries})");
            return EXIT_USAGE;
        }
    }

    let selected_devices = match select_devices(&device_list.devices, &args.device) {
        Ok(selected) => selected,
        Err(message) => {
            eprintln!("run: {message}");
            return EXIT_USAGE;
        }
    };
    if selected_devices.is_empty() {
        eprintln!(
            "run: no devices configured (see {})",
            devices_path.display()
        );
        return EXIT_USAGE;
    }
    if let Some(diagnostic) = config::check_report_output(&base.join(&config.report_output)) {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(diagnostic);
        render::eprint_all(&diagnostics);
        eprintln!("run: report_output is not usable; nothing was executed");
        return EXIT_USAGE;
    }

    // Workspace: scenes come from the configured scenes_dir, plays from
    // --play patterns or plays_dir (spec 8.1).
    let scenes = workspace::load_scenes(&base.join(&config.scenes_dir));
    let plays = load_play_selection(base, &config, &args.play);
    let mut load_diagnostics = scenes.diagnostics;
    load_diagnostics.extend(plays.diagnostics);
    let ws = workspace::Workspace {
        scenes: scenes.workspace.scenes,
        plays: plays.workspace.plays,
    };
    render::eprint_all(&load_diagnostics);
    if load_diagnostics.has_errors() {
        eprintln!(
            "run: {} workspace error(s); nothing was executed",
            load_diagnostics.errors().count()
        );
        return EXIT_USAGE;
    }

    let selection = plan::Selection {
        tags: args.tag.clone(),
        exclude_tags: args.exclude_tag.clone(),
        acts: args.act.clone(),
    };
    let cli_vars = match plan::parse_cli_vars(&args.var) {
        Ok(vars) => vars,
        Err(diagnostics) => {
            render::eprint_all(&diagnostics);
            eprintln!("run: invalid --var; nothing was executed");
            return EXIT_USAGE;
        }
    };
    let input = plan::PlanInput {
        config: &config,
        workspace: &ws,
        devices: &selected_devices,
        plays: &ws.plays,
        selection: &selection,
        cli_vars: &cli_vars,
        cli_timeout: args.timeout,
    };
    let plan = match plan::build_plan(&input) {
        Ok(plan) => plan,
        Err(diagnostics) => {
            render::eprint_all(&diagnostics);
            eprintln!(
                "run: {} planning error(s); nothing was executed",
                diagnostics.errors().count()
            );
            return EXIT_USAGE;
        }
    };

    let run_id = time_fmt::run_id(std::time::SystemTime::now());
    let report_dir = base.join(&config.report_output);
    let options = exec::ExecOptions {
        config: config.clone(),
        run_id: run_id.clone(),
        report_dir: report_dir.clone(),
        retries: args.retries.unwrap_or(config.retries),
        fail_fast: args.fail_fast,
        token: agent_token(),
        shutdown: CancellationToken::new(),
        master_version: env!("CARGO_PKG_VERSION").to_string(),
        session_options: impresario_clients::agent_client::SessionOptions::default(),
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("run: cannot start the async runtime: {err}");
            return EXIT_INFRA;
        }
    };
    let shutdown = options.shutdown.clone();
    let outcome = runtime.block_on(async move {
        let watcher = tokio::spawn(watch_interrupts(shutdown));
        let outcome = exec::execute(&plan, &options).await;
        watcher.abort();
        outcome
    });

    let color = !no_color && std::io::stdout().is_terminal();
    let run_dir = report_dir.join(&run_id);
    let reporter_errors = reporters::write(&outcome.report, &run_dir, &reporter_list, color);
    for message in &reporter_errors {
        eprintln!("run: {message}");
    }
    let mut code = outcome.exit_code;
    if !reporter_errors.is_empty() && code == EXIT_OK {
        code = EXIT_INFRA;
    }
    code
}

/// Expand `--play` file/dir/glob patterns (default: the configured
/// `plays_dir`) into loaded Plays. The loader sorts and dedups paths, so the
/// run order is deterministic.
fn load_play_selection(
    base: &Path,
    config: &ImpresarioConfig,
    patterns: &[String],
) -> workspace::LoadedWorkspace {
    if patterns.is_empty() {
        return workspace::load_plays(&base.join(&config.plays_dir));
    }
    let mut targets: Vec<PathBuf> = Vec::new();
    let mut diagnostics = Diagnostics::new();
    for pattern in patterns {
        let normalized = pattern.replace('\\', "/");
        if is_glob_pattern(&normalized) {
            let expanded = base.join(&normalized);
            let text = expanded.to_string_lossy().replace('\\', "/");
            match glob::glob(&text) {
                Ok(entries) => {
                    let mut matched = 0usize;
                    for entry in entries {
                        match entry {
                            Ok(path) => {
                                matched += 1;
                                push_unique(&mut targets, path);
                            }
                            Err(err) => diagnostics.push(Diagnostic::error(
                                Code::CfgInvalidValue,
                                format!("--play '{pattern}': {err}"),
                            )),
                        }
                    }
                    if matched == 0 {
                        diagnostics.push(Diagnostic::error(
                            Code::CfgInvalidValue,
                            format!("--play '{pattern}' matched no files"),
                        ));
                    }
                }
                Err(err) => diagnostics.push(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("--play '{pattern}' is not a valid glob: {err}"),
                )),
            }
        } else {
            let path = if Path::new(pattern).is_absolute() {
                PathBuf::from(pattern)
            } else {
                base.join(pattern)
            };
            if path.exists() {
                push_unique(&mut targets, path);
            } else {
                diagnostics.push(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("--play '{pattern}' does not exist"),
                ));
            }
        }
    }
    let mut loaded = workspace::load_paths(&targets);
    loaded.diagnostics.extend(diagnostics);
    loaded
}

fn is_glob_pattern(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

fn push_unique(targets: &mut Vec<PathBuf>, path: PathBuf) {
    if !targets.contains(&path) {
        targets.push(path);
    }
}

fn cmd_lsp() -> i32 {
    eprintln!(
        "lsp: the embedded language server is not built yet \
         (spec section 13) — see docs/STATUS.md"
    );
    EXIT_USAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("impresario-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp workspace");
        dir
    }

    fn argv(workspace: &Path, rest: &[&str]) -> Vec<String> {
        let mut args = vec![
            "impresario".to_string(),
            "--workspace".to_string(),
            workspace.display().to_string(),
        ];
        args.extend(rest.iter().map(|s| (*s).to_string()));
        args
    }

    #[test]
    fn init_then_validate_passes() {
        let dir = temp_workspace("init-validate");
        let code = run_with(&argv(&dir, &["init"]));
        assert_eq!(code, EXIT_OK);
        for file in [
            "impresario.config",
            "impresario.devices.config",
            ".gitignore",
            "scenes/LoginScreen.scene",
            "scenes/DashboardScreen.scene",
            "plays/login.play",
        ] {
            assert!(dir.join(file).exists(), "{file} missing");
        }
        // The example set must validate cleanly (spec 8.1).
        let code = run_with(&argv(&dir, &["validate"]));
        assert_eq!(code, EXIT_OK);
        // No device is required for validate; nothing was touched.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let dir = temp_workspace("init-force");
        assert_eq!(run_with(&argv(&dir, &["init"])), EXIT_OK);
        std::fs::write(dir.join("plays/login.play"), "// user edit\n").expect("edit");
        assert_eq!(run_with(&argv(&dir, &["init"])), EXIT_USAGE);
        let content = std::fs::read_to_string(dir.join("plays/login.play")).expect("read");
        assert_eq!(content, "// user edit\n");
        assert_eq!(run_with(&argv(&dir, &["init", "--force"])), EXIT_OK);
        let content = std::fs::read_to_string(dir.join("plays/login.play")).expect("read");
        assert!(content.contains("Play: User Authentication Feature"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_reports_dsl_errors() {
        let dir = temp_workspace("validate-errors");
        assert_eq!(run_with(&argv(&dir, &["init"])), EXIT_OK);
        std::fs::write(
            dir.join("plays/broken.play"),
            "Play: Broken\n\nAct: A\n\nStage: S\n    Scene: NoSuchScene\n        Check: Nope\n",
        )
        .expect("write");
        assert_eq!(run_with(&argv(&dir, &["validate"])), EXIT_USAGE);
        // Validating just the good file is clean again.
        assert_eq!(
            run_with(&argv(&dir, &["validate", "plays/login.play"])),
            EXIT_OK
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_without_configs_fails() {
        let dir = temp_workspace("validate-noconfig");
        let code = run_with(&argv(&dir, &["validate"]));
        assert_eq!(code, EXIT_USAGE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_accepts_both_file_kinds() {
        let dir = temp_workspace("parse");
        assert_eq!(run_with(&argv(&dir, &["init"])), EXIT_OK);
        assert_eq!(
            run_with(&argv(&dir, &["parse", "scenes/LoginScreen.scene"])),
            EXIT_OK
        );
        assert_eq!(
            run_with(&argv(&dir, &["parse", "plays/login.play", "--json"])),
            EXIT_OK
        );
        assert_eq!(
            run_with(&argv(&dir, &["parse", "impresario.config"])),
            EXIT_USAGE
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn devices_list_prints_the_configured_devices() {
        let dir = temp_workspace("devices");
        assert_eq!(run_with(&argv(&dir, &["init"])), EXIT_OK);
        assert_eq!(run_with(&argv(&dir, &["devices", "list"])), EXIT_OK);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_and_devices_check_require_configs() {
        let dir = temp_workspace("no-config");
        // No impresario.config: these commands fail with a config error.
        assert_eq!(run_with(&argv(&dir, &["run"])), EXIT_USAGE);
        assert_eq!(run_with(&argv(&dir, &["devices", "check"])), EXIT_USAGE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lsp_is_the_only_stub() {
        let dir = temp_workspace("stubs");
        assert_eq!(run_with(&argv(&dir, &["lsp"])), EXIT_USAGE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn select_devices_all_names_and_unknown() {
        use impresario_core::platforms::Platform;
        let device = |name: &str| DeviceConfig {
            name: name.to_string(),
            platform: Platform::Android,
            target_endpoint: "emulator-5554".to_string(),
            app_id: None,
            app_url: None,
            agent_address: None,
            connector: None,
            options: Default::default(),
        };
        let devices = vec![device("a"), device("b")];
        assert_eq!(select_devices(&devices, &[]).expect("all").len(), 2);
        assert_eq!(
            select_devices(&devices, &["all".to_string()])
                .expect("all")
                .len(),
            2
        );
        let chosen = select_devices(&devices, &["b".to_string(), "a".to_string()]).expect("named");
        assert_eq!(
            chosen.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        // Duplicates collapse.
        assert_eq!(
            select_devices(&devices, &["a".to_string(), "a".to_string()])
                .expect("named")
                .len(),
            1
        );
        assert!(select_devices(&devices, &["nope".to_string()]).is_err());
    }

    #[test]
    fn play_patterns_are_glob_expanded() {
        let dir = temp_workspace("play-glob");
        assert_eq!(run_with(&argv(&dir, &["init"])), EXIT_OK);
        std::fs::write(
            dir.join("plays/extra.play"),
            "Play: Extra\n\nAct: A\n\nStage: S\n    Scene: LoginScreen\n        Check: LoginTitle\n",
        )
        .expect("write");
        let config = config::load_config(&dir.join("impresario.config"))
            .config
            .expect("config loads");
        let all = load_play_selection(&dir, &config, &[]);
        assert_eq!(all.workspace.plays.len(), 2);
        let globbed = load_play_selection(&dir, &config, &["plays/*.play".to_string()]);
        assert_eq!(globbed.workspace.plays.len(), 2);
        let single = load_play_selection(&dir, &config, &["plays/login.play".to_string()]);
        assert_eq!(single.workspace.plays.len(), 1);
        let none = load_play_selection(&dir, &config, &["plays/nope.play".to_string()]);
        assert!(none.diagnostics.has_errors());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
