//! The `impresario` CLI (spec sections 4.4 and 8.1).
//!
//! This module owns argument parsing, exit codes and every command:
//! `config`, `workspace`, `validate`, `parse`, `devices list|check`, `run`, `lsp`, plus the
//! development-only `mock-agent` (decision I-77) and `mcp` (decision I-80).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use impresario_clients::agent_client::{AgentClient, DeviceInfo, HandshakeParams};
use impresario_clients::transport::{transport_for, DeviceTransport};
use impresario_core::apps::{self, AppConfig};
use impresario_core::config::{self, DeviceConfig, ImpresarioConfig};
use impresario_core::diag::{Code, Diagnostic, Diagnostics};
use impresario_core::home::Home;
use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
use impresario_core::platforms::Platform;
use impresario_core::settings::{self, EffectiveSettings, LayerSource, SettingValue, SettingsFile};
use impresario_core::validation::{self, DeviceRef};
use impresario_core::workspace::{self, WorkspaceSource};
use tokio_util::sync::CancellationToken;

use crate::report::RunReport;
use crate::{exec, jobs, plan, render, reporters, time_fmt};

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
    /// Workspace directory relative paths resolve against. Place it before the
    /// subcommand; `--workspace` after a `config` subcommand selects the
    /// config scope instead (TASK-2 section 4.1).
    #[arg(long, value_name = "DIR")]
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
    /// Never start the interactive TUI; print the plain dashboard instead.
    #[arg(long, global = true)]
    pub no_tui: bool,
    /// Plain, scriptable output (also disables the TUI).
    #[arg(long, global = true)]
    pub plain: bool,
    /// Command to run; with no command, start the interactive TUI on a TTY.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read and edit configuration (global and workspace layers).
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Manage known workspaces.
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Parse and validate the workspace without touching any device.
    Validate {
        /// Files or directories to validate (default: the configured dirs).
        paths: Vec<PathBuf>,
    },
    /// Parse one .scene, .theatre or .play file.
    Parse {
        file: PathBuf,
        /// Dump the typed AST as JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Explore and edit test files (tree, JSON, grep, rename, new, open).
    Tests(crate::tests_cmd::TestsArgs),
    /// Format .scene, .theatre and .play files canonically.
    Fmt {
        /// Files or directories (default: the configured workspace dirs).
        paths: Vec<PathBuf>,
        /// Check only: exit 1 when a file would change.
        #[arg(long)]
        check: bool,
    },
    /// List workspace tags with counts, defining files and usages.
    Tags {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Apple ID account used for free provisioning (global one-time login).
    Account(crate::account::AccountArgs),
    /// Apple TV pairing, signing, products, DDI and agent sessions (global setup).
    Apple(crate::apple::AppleArgs),
    /// Device definitions: add, edit, list, check, import, export.
    #[command(alias = "devices")]
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Explain what every platform needs, what was found and how to fix gaps.
    Doctor {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Managed tool downloads (pinned, checksum verified, explicit consent).
    Tools {
        #[command(subcommand)]
        command: crate::tools_cmd::ToolsCommand,
    },
    /// App definitions: add, edit, list, check, import, export.
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    /// Build deterministic webOS `.ipk` or Tizen `.wgt` packages.
    Package(crate::artifact_cmd::PackageArgs),
    /// Sign and inspect Tizen packages, and manage signing profiles.
    Sign(crate::artifact_cmd::SignArgs),
    /// Plan and execute plays on devices (spec section 8.2/8.3).
    Run(RunArgs),
    /// Save and inspect reusable run profiles (`profiles:` in config.yaml).
    Profile(crate::profile_cmd::ProfileArgs),
    /// List active jobs, or every job with `--all`.
    Jobs(crate::jobs_cmd::JobsArgs),
    /// Attach to a running job (Ctrl-C detaches, `--stop` stops it).
    Attach(crate::jobs_cmd::AttachArgs),
    /// Print or follow a job's log.
    Logs(crate::jobs_cmd::LogsArgs),
    /// Ask a job to stop at the next command boundary.
    Stop(crate::jobs_cmd::StopArgs),
    /// Remove a finished job's state and logs.
    Rm(crate::jobs_cmd::RemoveArgs),
    /// Wait for a job and exit with its code.
    Wait(crate::jobs_cmd::WaitArgs),
    /// Print a finished job's report.
    Report(crate::jobs_cmd::ReportArgs),
    /// Run the per-user job supervisor (auto-started by `run --detach`).
    Daemon(crate::jobs_cmd::DaemonArgs),
    /// Print shell completion code (bash, zsh, fish, PowerShell, Elvish).
    Completions {
        /// Shell to generate for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Run the embedded language server on stdio (spec section 13).
    Lsp,
    /// Serve the Model Context Protocol on stdio for AI agents (decision I-80).
    ///
    /// Exposes the workspace, the configured devices and the run pipeline as
    /// MCP tools and resources, so an agent can author, run and debug plays
    /// without shelling out. Stdout carries protocol messages only; every
    /// log, diagnostic and progress note goes to stderr.
    Mcp,
    /// Serve the scripted mock device on loopback (no hardware needed).
    ///
    /// Automated-suite aid (decision I-77): the test harness points a `web`
    /// device at the printed loopback address through the debug-only test
    /// sidecar, and `impresario run` exercises the whole pipeline hardware-free.
    MockAgent {
        /// Loopback port to listen on (0 = any free port, printed on start).
        #[arg(long, default_value_t = 50071)]
        port: u16,
    },
}

use crate::definitions::{AppCommand, DeviceCommand};

/// `--global` / `--workspace` scope selector; the workspace is the default.
#[derive(Debug, Args)]
pub struct ScopeArgs {
    /// Use the global config (~/.impresario/config.yaml).
    #[arg(long, conflicts_with = "workspace")]
    pub global: bool,
    /// Use the workspace config (<workspace>/impresario.config).
    #[arg(long, conflicts_with = "global")]
    pub workspace: bool,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show every setting with its effective value and layer.
    List {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Print one setting.
    Get {
        /// Dotted key, for example `ui.theme`.
        key: String,
        #[command(flatten)]
        scope: ScopeArgs,
        /// Show the winning layer and the shadowed values.
        #[arg(long)]
        explain: bool,
    },
    /// Set one setting, validating it against the schema.
    Set {
        /// Dotted key, for example `timeout_default_ms`.
        key: String,
        /// New value (lists are comma-separated).
        value: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Remove one setting from a layer.
    Unset {
        /// Dotted key.
        key: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Open a config file in $VISUAL/$EDITOR and revalidate it.
    Edit {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Print the path of a config file.
    Path {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Explain where a setting's effective value comes from.
    Explain {
        /// Dotted key.
        key: String,
        #[command(flatten)]
        scope: ScopeArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Print the active workspace and why it won.
    Current,
    /// List added workspaces.
    List,
    /// Remember a workspace by path.
    Add {
        /// Workspace directory (default: the active one).
        path: Option<PathBuf>,
    },
    /// Forget a workspace path.
    Remove {
        /// Workspace directory.
        path: PathBuf,
    },
    /// Make a workspace the global default.
    Use {
        /// Workspace directory.
        path: PathBuf,
    },
    /// Create a new workspace skeleton.
    New {
        /// Directory (created when missing); default: the active workspace.
        path: Option<PathBuf>,
        /// Also write the example scenes and play.
        #[arg(long)]
        example: bool,
        /// Overwrite files that already exist.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum UnverifiableArg {
    Fail,
    Warn,
}

#[derive(Debug, Args, Clone)]
pub struct RunArgs {
    /// Play file, directory or glob (repeatable; default: plays_dir).
    #[arg(long, visible_alias = "file", value_name = "FILE|DIR|GLOB")]
    pub play: Vec<String>,
    /// App definition to bind (beats each play's `App:` header).
    #[arg(long, value_name = "NAME")]
    pub app: Option<String>,
    /// Actor/device name, `@group` (repeatable) or `all`.
    #[arg(long, visible_alias = "actor", value_name = "NAME|@GROUP")]
    pub device: Vec<String>,
    /// Only devices on this platform (repeatable or comma list).
    #[arg(long, value_name = "PLATFORM")]
    pub platform: Vec<String>,
    /// Only run acts with this tag (repeatable; OR).
    #[arg(long, value_name = "TAG")]
    pub tag: Vec<String>,
    /// Exclude acts with this tag (wins over --tag).
    #[arg(long, value_name = "TAG")]
    pub exclude_tag: Vec<String>,
    /// Only run acts with this exact name (repeatable).
    #[arg(long, value_name = "NAME")]
    pub act: Vec<String>,
    /// Use a saved run profile (see `impresario profile`).
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
    /// Run only acts that failed in the newest report under report_output.
    #[arg(long)]
    pub only_failed: bool,
    /// Variable override K=V (repeatable, beats play vars).
    #[arg(long, value_name = "K=V")]
    pub var: Vec<String>,
    /// Act parameter override: `ACT.PARAM=VALUE` (repeatable; beats Examples).
    #[arg(long = "arg", value_name = "ACT.PARAM=VALUE")]
    pub arg: Vec<String>,
    /// Default poll timeout in ms (beats timeout_default_ms).
    #[arg(long, value_name = "MS")]
    pub timeout: Option<u64>,
    /// Retry failed acts up to n times (beats config retries).
    #[arg(long, value_name = "N")]
    pub retries: Option<u32>,
    /// What to do when a probe cannot establish truth (beats config).
    #[arg(long, value_enum, value_name = "POLICY")]
    pub unverifiable: Option<UnverifiableArg>,
    /// Reporters: comma list of console, junit, json, html.
    #[arg(long, value_name = "LIST")]
    pub reporter: Option<String>,
    /// Stop the whole run at the first failure.
    #[arg(long)]
    pub fail_fast: bool,
    /// Print the resolved device/play/act matrix and exit without touching a device.
    #[arg(long)]
    pub dry_run: bool,
    /// Hand the job to the background supervisor and return immediately.
    #[arg(long)]
    pub detach: bool,
    /// What to do when another job holds a device lease.
    #[arg(long, value_enum, default_value_t = crate::jobs::OnBusy::Wait)]
    pub on_busy: crate::jobs::OnBusy,
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

    let home = match Home::resolve() {
        Ok(home) => home,
        Err(error) => {
            eprintln!("impresario: {error}");
            return EXIT_INFRA;
        }
    };
    // PATH setup belongs to the no-subcommand entry point (`impresario`).
    // Subcommands are often run by scripts, editors and tests; they must not
    // copy binaries or rewrite the user's environment as a side effect.
    if cli.command.is_none() {
        match crate::install::ensure_on_path(&home) {
            Ok(Some(report)) if report.path_updated && !cli.quiet => {
                eprintln!(
                    "impresario: installed to {}; restart your shell to use `impresario` from PATH",
                    report.bin_dir.display()
                );
            }
            Ok(_) => {}
            Err(error) => {
                if cli.verbose > 0 {
                    eprintln!("impresario: PATH setup: {error}");
                }
            }
        }
    }
    // The Apple account is machine-global and must work even when the current
    // workspace config is missing or invalid. Dispatch it before resolving a
    // workspace.
    if let Some(Command::Account(args)) = cli.command.as_ref() {
        return crate::account::run(args, &home);
    }
    if let Some(Command::Apple(args)) = cli.command.as_ref() {
        return crate::apple::run(args, &home);
    }

    let (base, base_source) = workspace_base(&cli.workspace, &home);
    let config_path = resolve_path(&base, &cli.config, "impresario.config");
    let devices_path = resolve_path(&base, &cli.devices_config, "impresario.devices.config");

    let Some(command) = cli.command.as_ref() else {
        return cmd_home(&home, &base, &config_path, &devices_path, &cli);
    };

    match command {
        Command::Config { command } => cmd_config(command, &home, &config_path),
        Command::Workspace { command } => cmd_workspace(command, &home, &base, base_source),
        Command::Validate { paths } => cmd_validate(&base, &config_path, &devices_path, paths),
        Command::Parse { file, json } => cmd_parse(&base, file, *json),
        Command::Tests(args) => crate::tests_cmd::run(&base, &config_path, args),
        Command::Fmt { paths, check } => cmd_fmt(&base, &config_path, paths, *check),
        Command::Tags { json } => cmd_tags(&base, &config_path, *json),
        // Handled before workspace resolution.
        Command::Account(_) => EXIT_INFRA,
        Command::Apple(_) => EXIT_INFRA,
        Command::Device { command } => match command {
            DeviceCommand::Check { names } => {
                cmd_devices_check(&base, &config_path, &devices_path, names)
            }
            DeviceCommand::Discover { json, timeout } => {
                crate::wizard::run_discover(&home, *json, *timeout)
            }
            DeviceCommand::Pair { target, code, json } => {
                crate::wizard::run_pair(&home, target, code, *json)
            }
            DeviceCommand::Wizard(args) => crate::wizard::run(args, &home, &devices_path),
            other => crate::definitions::run_device(other, &home, &devices_path),
        },
        Command::App { command } => {
            let apps_path = base.join("impresario.apps.config");
            crate::definitions::run_app(command, &home, &apps_path)
        }
        Command::Doctor { json } => {
            crate::doctor::run(&home, &base, &config_path, &devices_path, *json)
        }
        Command::Tools { command } => crate::tools_cmd::run(command, &home),
        Command::Package(args) => crate::artifact_cmd::run_package(args),
        Command::Sign(args) => crate::artifact_cmd::run_sign(&home, args),
        Command::Run(args) => cmd_run(
            &home,
            &base,
            &config_path,
            &devices_path,
            args,
            cli.no_color,
        ),
        Command::Profile(args) => crate::profile_cmd::run(&home, args),
        Command::Jobs(args) => crate::jobs_cmd::run_jobs(&home, args),
        Command::Attach(args) => crate::jobs_cmd::attach(&home, args),
        Command::Logs(args) => crate::jobs_cmd::logs(&home, args),
        Command::Stop(args) => crate::jobs_cmd::stop(&home, args),
        Command::Rm(args) => crate::jobs_cmd::remove(&home, args),
        Command::Wait(args) => crate::jobs_cmd::wait(&home, args),
        Command::Report(args) => crate::jobs_cmd::report(&home, args),
        Command::Daemon(args) => crate::jobs_cmd::daemon(&home, args),
        Command::Completions { shell } => {
            use clap::CommandFactory as _;
            let mut command = Cli::command();
            clap_complete::generate(*shell, &mut command, "impresario", &mut std::io::stdout());
            EXIT_OK
        }
        Command::Lsp => cmd_lsp(&base, &config_path, &devices_path),
        Command::Mcp => cmd_mcp(&base, &config_path, &devices_path),
        Command::MockAgent { port } => cmd_mock_agent(*port),
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
            // libadb logs one WARN per session for ADB banner features it
            // does not use (`server_status`, `track_mdns`), and
            // apple-codesign logs every sealed resource at WARN. Those are
            // benign and would otherwise drown the CLI output; RUST_LOG
            // overrides this filter for protocol debugging.
            "warn,libadb=error,apple_codesign=error"
        };
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    });
}

fn workspace_base(workspace: &Option<PathBuf>, home: &Home) -> (PathBuf, WorkspaceSource) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let global = SettingsFile::load(&home.config_path());
    let global_workspace = match global.values.get("workspace") {
        Some(SettingValue::Text(path)) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => None,
    };
    let environment = std::env::var_os("IMPRESARIO_WORKSPACE");
    let location = workspace::resolve_root(
        workspace.as_deref(),
        environment.as_deref(),
        &cwd,
        global_workspace.as_deref(),
    );
    (location.root, location.source)
}

/// Resolve the effective configuration from the global and workspace layers.
///
/// Thin wrapper over [`settings::load_default_layers`] kept so the CLI, the
/// MCP server and the LSP all name the same helper.
pub(crate) fn load_effective_config(
    base: &Path,
    config_path: &Path,
) -> config::ConfigOutcome<ImpresarioConfig> {
    settings::load_default_layers(config_path, base)
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
    type: webOS
    ip: 192.168.1.50
  - name: flagship_android
    type: android
    ip: 192.168.1.60
  - name: shield_tv
    type: androidtv
    ip: 192.168.1.61
  - name: test_iphone
    type: iOS
    ip: 192.168.1.70
  - name: apple_tv
    type: tvOS
    ip: 192.168.1.71
  # A local Chromium instance started by the connector; uncomment to try the
  # example on this machine.
  # - name: local_chrome
  #   type: web
  #   ip: 127.0.0.1
";

const EXAMPLE_APPS: &str = "\
# Apps are separate from devices (D-21); a play binds one with `App: shop`.
apps:
  - name: shop
    description: Example store app across phone, TV and web
    platforms:
      web:
        url: http://localhost:8080/index.html
      webos:
        id: com.example.shop
      tizen:
        id: AbCdEf1234.Shop
      android:
        id: com.example.shop
      androidtv:
        id: com.example.shop.tv
      ios:
        id: com.example.shop
      tvos:
        id: com.example.shop.tv
    options:
      clear_data: false
";

const EXAMPLE_LOGIN_SCENE: &str = "\
Scene: LoginScreen
Platforms: cross-platform

| Piece | web | android | ios |
|---|---|---|---|
| LoginTitle | h1.title | id:login_title | accessibilityId:login_title |
| UsernameInput | #username | id:username | accessibilityId:username |
| PasswordInput | #password | id:password | accessibilityId:password |
| SubmitButton | button[type='submit'] | id:login_btn | accessibilityId:login |
";

const EXAMPLE_DASHBOARD_SCENE: &str = "\
Scene: DashboardScreen
Platforms: cross-platform

| Piece | web | android | ios |
|---|---|---|---|
| WelcomeBanner | h2.welcome | id:welcome_banner | accessibilityId:welcome_banner |
| LogoutButton | #logout | id:logout_btn | accessibilityId:logout |
";

const EXAMPLE_LOGIN_PLAY: &str = "\
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
App: shop
Var: USER = 'test_user_impresario'

Act: Successful Login Flow
Tags: smoke, auth
Platforms: android, ios, web, webos
    Perform: NavigateAndLogin
    Perform: VerifyOutcome
";

const EXAMPLE_LOGIN_THEATRE: &str = "\
Theatre: Login
Stage: NavigateAndLogin
    Timeout: 15s
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${var.USER}' => UsernameInput
        Action: '${env.TEST_PASSWORD}' => PasswordInput
        Action: click => SubmitButton

Stage: VerifyOutcome
    Scene: DashboardScreen
        Check: WelcomeBanner is visible timeout 20s
";

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

impl ScopeArgs {
    /// `Some(true)` for `--global`, `Some(false)` for `--workspace`, `None`
    /// when the command should use the effective (merged) value.
    fn explicit(&self) -> Option<bool> {
        if self.global {
            Some(true)
        } else if self.workspace {
            Some(false)
        } else {
            None
        }
    }
}

fn scope_path(scope: &ScopeArgs, home: &Home, workspace_config: &Path) -> PathBuf {
    if scope.global {
        home.config_path()
    } else {
        workspace_config.to_path_buf()
    }
}

fn scope_name(scope: &ScopeArgs) -> &'static str {
    if scope.global {
        "global"
    } else {
        "workspace"
    }
}

fn source_label(source: &LayerSource) -> String {
    match source {
        LayerSource::Workspace(path) => format!("workspace {}", path.display()),
        LayerSource::Global(path) => format!("global {}", path.display()),
        LayerSource::Default => "default".to_string(),
    }
}

fn cmd_config(command: &ConfigCommand, home: &Home, workspace_config: &Path) -> i32 {
    let global = SettingsFile::load(&home.config_path());
    let workspace = SettingsFile::load(workspace_config);
    let effective = EffectiveSettings::resolve(&global, &workspace);
    match command {
        ConfigCommand::List { scope, json } => {
            let mut diagnostics = global.diagnostics.clone();
            diagnostics.extend(workspace.diagnostics.iter().cloned());
            if diagnostics.has_errors() {
                render::eprint_all(&diagnostics);
                return EXIT_USAGE;
            }
            let rows = setting_rows(scope, &global, &workspace, &effective);
            if *json {
                let mut map = serde_json::Map::new();
                for (key, value, source) in &rows {
                    let mut entry = serde_json::Map::new();
                    entry.insert(
                        "value".to_string(),
                        value.as_ref().map_or(serde_json::Value::Null, setting_json),
                    );
                    entry.insert(
                        "source".to_string(),
                        serde_json::Value::String(source.clone()),
                    );
                    map.insert((*key).to_string(), serde_json::Value::Object(entry));
                }
                match serde_json::to_string_pretty(&serde_json::Value::Object(map)) {
                    Ok(text) => println!("{text}"),
                    Err(error) => {
                        eprintln!("config list: cannot render JSON: {error}");
                        return EXIT_INFRA;
                    }
                }
                return EXIT_OK;
            }
            for (key, value, source) in &rows {
                let shown = value
                    .as_ref()
                    .map_or_else(|| "(unset)".to_string(), SettingValue::display);
                println!("{key} = {shown}  [{source}]");
            }
            EXIT_OK
        }
        ConfigCommand::Get {
            key,
            scope,
            explain,
        } => show_setting(
            key,
            &effective,
            &global,
            &workspace,
            scope.explicit(),
            *explain,
        ),
        ConfigCommand::Explain { key, .. } => {
            show_setting(key, &effective, &global, &workspace, None, true)
        }
        ConfigCommand::Set { key, value, scope } => {
            let Some(def) = settings::definition(key) else {
                return unknown_key(key);
            };
            if matches!(def.scope, settings::Scope::GlobalOnly) && !scope.global {
                eprintln!(
                    "config set: '{key}' is global-only; pass --global (workspace files are relative to the workspace)"
                );
                return EXIT_USAGE;
            }
            let parsed = match settings::parse_cli_value(def, value) {
                Ok(parsed) => parsed,
                Err(reason) => {
                    eprintln!("config set: IMP-CFG-003: {key}: {reason}");
                    return EXIT_USAGE;
                }
            };
            let path = scope_path(scope, home, workspace_config);
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => {
                    eprintln!("config set: cannot read {}: {error}", path.display());
                    return EXIT_INFRA;
                }
            };
            let edited = match settings::edit_set(&source, def, &parsed) {
                Ok(edited) => edited,
                Err(error) => {
                    eprintln!("config set: cannot edit {}: {error}", path.display());
                    return EXIT_INFRA;
                }
            };
            if let Err(message) = validate_and_write(&path, &edited) {
                eprintln!("config set: {message}");
                return EXIT_USAGE;
            }
            println!(
                "config set: {key} = {} in {} ({})",
                parsed.display(),
                path.display(),
                scope_name(scope)
            );
            EXIT_OK
        }
        ConfigCommand::Unset { key, scope } => {
            let Some(def) = settings::definition(key) else {
                return unknown_key(key);
            };
            if matches!(def.scope, settings::Scope::GlobalOnly) && !scope.global {
                eprintln!("config unset: '{key}' is global-only; pass --global");
                return EXIT_USAGE;
            }
            let path = scope_path(scope, home, workspace_config);
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    println!("config unset: {key} is not set in {}", path.display());
                    return EXIT_OK;
                }
                Err(error) => {
                    eprintln!("config unset: cannot read {}: {error}", path.display());
                    return EXIT_INFRA;
                }
            };
            let edited = match settings::edit_unset(&source, key) {
                Ok(edited) => edited,
                Err(error) => {
                    eprintln!("config unset: cannot edit {}: {error}", path.display());
                    return EXIT_INFRA;
                }
            };
            if edited == source {
                println!("config unset: {key} is not set in {}", path.display());
                return EXIT_OK;
            }
            if let Err(message) = validate_and_write(&path, &edited) {
                eprintln!("config unset: {message}");
                return EXIT_USAGE;
            }
            println!(
                "config unset: {key} removed from {} ({})",
                path.display(),
                scope_name(scope)
            );
            EXIT_OK
        }
        ConfigCommand::Edit { scope } => {
            let path = scope_path(scope, home, workspace_config);
            edit_config_file(&path)
        }
        ConfigCommand::Path { scope } => {
            let path = scope_path(scope, home, workspace_config);
            println!("{}", path.display());
            EXIT_OK
        }
    }
}

fn edit_config_file(path: &Path) -> i32 {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("config edit: cannot create {}: {error}", parent.display());
                return EXIT_INFRA;
            }
        }
    }
    let original = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            eprintln!("config edit: cannot read {}: {error}", path.display());
            return EXIT_INFRA;
        }
    };
    let (program, args) = match editor_command() {
        Ok(editor) => editor,
        Err(message) => {
            eprintln!("config edit: {message}");
            return EXIT_INFRA;
        }
    };
    if original.is_none() {
        if let Err(error) = std::fs::write(path, "# impresario configuration\n") {
            eprintln!("config edit: cannot create {}: {error}", path.display());
            return EXIT_INFRA;
        }
    }
    loop {
        let status = std::process::Command::new(&program)
            .args(&args)
            .arg(path)
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("config edit: editor exited with {status}");
                return EXIT_USAGE;
            }
            Err(error) => {
                eprintln!("config edit: cannot run {program}: {error}");
                return EXIT_INFRA;
            }
        }
        let edited = match std::fs::read_to_string(path) {
            Ok(edited) => edited,
            Err(error) => {
                eprintln!("config edit: cannot read {}: {error}", path.display());
                return EXIT_INFRA;
            }
        };
        let parsed = settings::parse_settings(&edited, path);
        if !parsed.diagnostics.has_errors() {
            println!("config edit: {} ok", path.display());
            return EXIT_OK;
        }
        render::eprint_all(&parsed.diagnostics);
        // TASK-2 section 4.1: an invalid edit keeps the old file.
        let restored = match &original {
            Some(text) => std::fs::write(path, text),
            None => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        };
        if let Err(error) = restored {
            eprintln!("config edit: could not restore {}: {error}", path.display());
            return EXIT_INFRA;
        }
        eprintln!(
            "config edit: {} is not valid; the previous contents were restored",
            path.display()
        );
        if !reopen_editor_prompt() {
            return EXIT_USAGE;
        }
        if original.is_none() {
            if let Err(error) = std::fs::write(path, "# impresario configuration\n") {
                eprintln!("config edit: cannot create {}: {error}", path.display());
                return EXIT_INFRA;
            }
        }
    }
}

pub(crate) fn editor_command() -> Result<(String, Vec<String>), String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .map_err(|_| "set VISUAL or EDITOR to your editor".to_string())?;
    if editor.trim().is_empty() {
        return Err("set VISUAL or EDITOR to your editor".to_string());
    }
    let mut parts = split_command_line(&editor).into_iter();
    let Some(program) = parts.next() else {
        return Err("set VISUAL or EDITOR to your editor".to_string());
    };
    Ok((program, parts.collect()))
}

/// Split an editor command line, honouring single and double quotes.
fn split_command_line(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for character in text.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => current.push(character),
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            None => current.push(character),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn reopen_editor_prompt() -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if std::env::var_os("CI").is_some() || !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("reopen the editor? [y/N] ");
    if std::io::stderr().flush().is_err() {
        return false;
    }
    let mut answer = String::new();
    match std::io::stdin().lock().read_line(&mut answer) {
        Ok(0) | Err(_) => false,
        Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
    }
}

fn setting_rows(
    scope: &ScopeArgs,
    global: &SettingsFile,
    workspace: &SettingsFile,
    effective: &EffectiveSettings,
) -> Vec<(&'static str, Option<SettingValue>, String)> {
    match scope.explicit() {
        Some(true) => settings::SETTINGS
            .iter()
            .map(|def| {
                (
                    def.key,
                    global.values.get(def.key).cloned(),
                    format!("global {}", global.path.display()),
                )
            })
            .collect(),
        Some(false) => settings::SETTINGS
            .iter()
            .map(|def| {
                (
                    def.key,
                    workspace.values.get(def.key).cloned(),
                    format!("workspace {}", workspace.path.display()),
                )
            })
            .collect(),
        None => effective
            .iter()
            .map(|(key, value, source)| {
                let value = if effective.is_set(key) {
                    Some(value.clone())
                } else {
                    None
                };
                (key, value, source_label(source))
            })
            .collect(),
    }
}

fn show_setting(
    key: &str,
    effective: &EffectiveSettings,
    global: &SettingsFile,
    workspace: &SettingsFile,
    scope: Option<bool>,
    explain: bool,
) -> i32 {
    let Some(def) = settings::definition(key) else {
        return unknown_key(key);
    };
    if explain {
        let Some(explained) = effective.explain(key) else {
            return unknown_key(key);
        };
        let shown = if effective.is_set(key) {
            explained.value.display()
        } else {
            "(unset)".to_string()
        };
        println!("{key} = {shown}");
        println!("  source: {}", source_label(&explained.source));
        for (source, value) in &explained.shadowed {
            println!("  shadowed: {} = {}", source_label(source), value.display());
        }
        println!("  type: {}", def.kind.type_name());
        println!("  {}", def.doc);
        return EXIT_OK;
    }
    match scope {
        Some(global_scope) => {
            let (layer, label) = if global_scope {
                (global, "global")
            } else {
                (workspace, "workspace")
            };
            match layer.values.get(key) {
                Some(value) => println!("{key} = {}", value.display()),
                None => println!("{key} = (unset)"),
            }
            println!("  source: {label} {}", layer.path.display());
            EXIT_OK
        }
        None => {
            let Some(explained) = effective.explain(key) else {
                return unknown_key(key);
            };
            let shown = if effective.is_set(key) {
                explained.value.display()
            } else {
                "(unset)".to_string()
            };
            println!("{key} = {shown}");
            println!("  source: {}", source_label(&explained.source));
            EXIT_OK
        }
    }
}

fn unknown_key(key: &str) -> i32 {
    let known: Vec<&str> = settings::SETTINGS.iter().map(|def| def.key).collect();
    let hint = impresario_core::diag::suggest(key, &known)
        .map(|hint| format!(" ({hint})"))
        .unwrap_or_default();
    eprintln!("config: IMP-CFG-001: unknown key '{key}'{hint}");
    eprintln!("config: known keys: {}", known.join(", "));
    EXIT_USAGE
}

fn setting_json(value: &SettingValue) -> serde_json::Value {
    match value {
        SettingValue::Text(text) | SettingValue::Enum(text) => {
            serde_json::Value::String(text.clone())
        }
        SettingValue::Bool(value) => serde_json::Value::Bool(*value),
        SettingValue::Int(value) => serde_json::Value::Number((*value).into()),
        SettingValue::List(values) => serde_json::Value::Array(
            values
                .iter()
                .map(|value| serde_json::Value::String(value.clone()))
                .collect(),
        ),
    }
}

fn validate_and_write(path: &Path, contents: &str) -> Result<(), String> {
    let parsed = settings::parse_settings(contents, path);
    if parsed.diagnostics.has_errors() {
        let mut rendered = String::new();
        for diagnostic in parsed.diagnostics.errors() {
            rendered.push_str(&format!("{diagnostic}\n"));
        }
        return Err(format!(
            "{} would not be valid after the edit: {}",
            path.display(),
            rendered.trim()
        ));
    }
    write_file(path, contents)
}

fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
    }
    let temp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&temp, contents)
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    #[cfg(windows)]
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&temp, path)
        .map_err(|error| format!("cannot replace {}: {error}", path.display()))
}

// ---------------------------------------------------------------------------
// workspace
// ---------------------------------------------------------------------------

fn cmd_workspace(
    command: &WorkspaceCommand,
    home: &Home,
    base: &Path,
    base_source: WorkspaceSource,
) -> i32 {
    match command {
        WorkspaceCommand::Current => {
            println!("{}", base.display());
            println!("  source: {}", base_source.as_str());
            if !base.is_dir() {
                eprintln!("  warning: the directory does not exist");
            }
            EXIT_OK
        }
        WorkspaceCommand::List => {
            let global = SettingsFile::load(&home.config_path());
            render::eprint_all(&global.diagnostics);
            let added: Vec<String> = match global.values.get("workspaces") {
                Some(SettingValue::List(values)) => values.clone(),
                _ => Vec::new(),
            };
            let mut seen = Vec::new();
            for path in added {
                if seen.contains(&path) {
                    continue;
                }
                let exists = Path::new(&path).is_dir();
                let active = same_path(Path::new(&path), base);
                println!(
                    "{} {}{}",
                    if active { "*" } else { " " },
                    path,
                    if exists { "" } else { "  (missing)" }
                );
                seen.push(path);
            }
            if !seen.iter().any(|path| same_path(Path::new(path), base)) {
                println!("* {}", base.display());
            }
            EXIT_OK
        }
        WorkspaceCommand::Add { path } => {
            let path = path.clone().unwrap_or_else(|| base.to_path_buf());
            let path = absolute_path(&path);
            if !path.is_dir() {
                eprintln!("workspace add: {} is not a directory", path.display());
                return EXIT_USAGE;
            }
            let text = path.to_string_lossy().into_owned();
            let global = SettingsFile::load(&home.config_path());
            let mut values: Vec<String> = match global.values.get("workspaces") {
                Some(SettingValue::List(values)) => values.clone(),
                _ => Vec::new(),
            };
            if values
                .iter()
                .any(|value| same_path(Path::new(value), &path))
            {
                println!("workspace add: {} is already listed", path.display());
                return EXIT_OK;
            }
            values.push(text);
            match save_global(home, "workspaces", SettingValue::List(values)) {
                Ok(()) => {
                    println!("workspace add: {}", path.display());
                    EXIT_OK
                }
                Err(message) => {
                    eprintln!("workspace add: {message}");
                    EXIT_USAGE
                }
            }
        }
        WorkspaceCommand::Remove { path } => {
            let path = absolute_path(path);
            let global = SettingsFile::load(&home.config_path());
            let values: Vec<String> = match global.values.get("workspaces") {
                Some(SettingValue::List(values)) => values.clone(),
                _ => Vec::new(),
            };
            let kept: Vec<String> = values
                .iter()
                .filter(|value| !same_path(Path::new(value), &path))
                .cloned()
                .collect();
            if kept.len() == values.len() {
                println!("workspace remove: {} is not listed", path.display());
                return EXIT_OK;
            }
            match save_global(home, "workspaces", SettingValue::List(kept)) {
                Ok(()) => {
                    println!("workspace remove: {}", path.display());
                    EXIT_OK
                }
                Err(message) => {
                    eprintln!("workspace remove: {message}");
                    EXIT_USAGE
                }
            }
        }
        WorkspaceCommand::Use { path } => {
            let path = absolute_path(path);
            if !path.is_dir() {
                eprintln!("workspace use: {} is not a directory", path.display());
                return EXIT_USAGE;
            }
            let text = path.to_string_lossy().into_owned();
            if let Err(message) = save_global(home, "workspace", SettingValue::Text(text.clone())) {
                eprintln!("workspace use: {message}");
                return EXIT_USAGE;
            }
            let global = SettingsFile::load(&home.config_path());
            let mut values: Vec<String> = match global.values.get("workspaces") {
                Some(SettingValue::List(values)) => values.clone(),
                _ => Vec::new(),
            };
            if !values
                .iter()
                .any(|value| same_path(Path::new(value), &path))
            {
                values.push(text);
                if let Err(message) = save_global(home, "workspaces", SettingValue::List(values)) {
                    eprintln!("workspace use: {message}");
                    return EXIT_USAGE;
                }
            }
            println!("workspace use: {}", path.display());
            EXIT_OK
        }
        WorkspaceCommand::New {
            path,
            example,
            force,
        } => {
            let root = match path {
                Some(path) => absolute_path(path),
                None => base.to_path_buf(),
            };
            cmd_workspace_new(&root, *example, *force)
        }
    }
}

fn cmd_workspace_new(root: &Path, example: bool, force: bool) -> i32 {
    let root = root.to_path_buf();
    let mut files: Vec<(&str, &str)> = vec![("impresario.config", EXAMPLE_CONFIG)];
    if example {
        files.extend([
            ("impresario.devices.config", EXAMPLE_DEVICES),
            ("impresario.apps.config", EXAMPLE_APPS),
            ("scenes/LoginScreen.scene", EXAMPLE_LOGIN_SCENE),
            ("scenes/DashboardScreen.scene", EXAMPLE_DASHBOARD_SCENE),
            ("theatres/login.theatre", EXAMPLE_LOGIN_THEATRE),
            ("plays/login.play", EXAMPLE_LOGIN_PLAY),
        ]);
    }
    let mut status = EXIT_OK;
    for (relative, content) in files {
        let target = relative
            .split('/')
            .fold(root.clone(), |path, part| path.join(part));
        if target.exists() && !force {
            eprintln!(
                "workspace new: refusing to overwrite {} (use --force)",
                target.display()
            );
            status = EXIT_USAGE;
            continue;
        }
        if let Some(parent) = target.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("workspace new: cannot create {}: {err}", parent.display());
                status = EXIT_USAGE;
                continue;
            }
        }
        match std::fs::write(&target, content) {
            Ok(()) => println!("created {}", target.display()),
            Err(err) => {
                eprintln!("workspace new: cannot write {}: {err}", target.display());
                status = EXIT_USAGE;
            }
        }
    }
    for directory in ["scenes", "theatres", "plays", "reports"] {
        let target = root.join(directory);
        if let Err(err) = std::fs::create_dir_all(&target) {
            eprintln!("workspace new: cannot create {}: {err}", target.display());
            status = EXIT_USAGE;
        }
    }
    if example {
        let gitignore = root.join(".gitignore");
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
                    eprintln!("workspace new: cannot write {}: {err}", gitignore.display());
                    status = EXIT_USAGE;
                }
            }
        }
    }
    if status == EXIT_OK {
        println!(
            "workspace new: {} ready; run `impresario validate --workspace {}` next",
            root.display(),
            root.display()
        );
    }
    status
}

fn save_global(home: &Home, key: &str, value: SettingValue) -> Result<(), String> {
    let Some(def) = settings::definition(key) else {
        return Err(format!("internal: unknown setting '{key}'"));
    };
    let path = home.config_path();
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let edited = settings::edit_set(&source, def, &value)
        .map_err(|error| format!("cannot edit {}: {error}", path.display()))?;
    validate_and_write(&path, &edited)
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

/// Core of `validate`, shared by the CLI and the MCP server.
#[derive(Debug)]
pub(crate) struct ValidateOutcome {
    pub diagnostics: Diagnostics,
    /// False when `impresario.config` could not be read at all.
    pub config_ok: bool,
    /// False when `impresario.devices.config` could not be read at all.
    pub devices_ok: bool,
    /// False when `impresario.apps.config` could not be read at all.
    pub apps_ok: bool,
    pub scenes: usize,
    pub theatres: usize,
    pub plays: usize,
    pub devices: usize,
    pub apps: usize,
}

pub(crate) fn validate_workspace(
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    paths: &[PathBuf],
) -> ValidateOutcome {
    let mut all = Diagnostics::new();

    let cfg_outcome = load_effective_config(base, config_path);
    all.extend(cfg_outcome.diagnostics.iter().cloned());
    let Some(cfg) = cfg_outcome.config else {
        return ValidateOutcome {
            diagnostics: all,
            config_ok: false,
            devices_ok: true,
            apps_ok: true,
            scenes: 0,
            theatres: 0,
            plays: 0,
            devices: 0,
            apps: 0,
        };
    };

    let devices_outcome = config::load_devices_merged(devices_path);
    all.extend(devices_outcome.diagnostics.iter().cloned());
    let Some(devices) = devices_outcome.config else {
        return ValidateOutcome {
            diagnostics: all,
            config_ok: true,
            devices_ok: false,
            apps_ok: true,
            scenes: 0,
            theatres: 0,
            plays: 0,
            devices: 0,
            apps: 0,
        };
    };
    let apps_path = base.join("impresario.apps.config");
    let apps_outcome = apps::load_apps_merged(&apps_path);
    all.extend(apps_outcome.diagnostics.iter().cloned());
    let Some(app_list) = apps_outcome.config else {
        return ValidateOutcome {
            diagnostics: all,
            config_ok: true,
            devices_ok: true,
            apps_ok: false,
            scenes: 0,
            theatres: 0,
            plays: 0,
            devices: devices.devices.len(),
            apps: 0,
        };
    };

    if let Some(diag) = config::check_report_output(&base.join(&cfg.report_output)) {
        all.push(diag);
    }

    let loaded = if paths.is_empty() {
        workspace::load_dirs(
            &base.join(&cfg.scenes_dir),
            &base.join(&cfg.theatres_dir),
            &base.join(&cfg.plays_dir),
        )
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
        // A theatrical play resolves `Perform:` against the configured
        // theatres, so load the stage index even when only the play was named.
        if explicit.workspace.theatres.is_empty() {
            let theatres = workspace::load_theatres(&base.join(&cfg.theatres_dir));
            explicit.workspace.theatres = theatres.workspace.theatres;
            explicit.diagnostics.extend(theatres.diagnostics);
        }
        explicit
    };
    all.extend(loaded.diagnostics.iter().cloned());

    let device_refs: Vec<DeviceRef> = devices
        .devices
        .iter()
        .map(|device| DeviceRef::new(device.name.clone(), device.platform))
        .collect();
    let semantic =
        validation::validate_with_actors(&loaded.workspace, &device_refs, &devices.groups);
    all.extend(semantic.iter().cloned());

    // Resolve each play's `App:` header against the merged app layer.
    for play in &loaded.workspace.plays {
        let Some(named) = &play.app else {
            continue;
        };
        if !app_list.apps.iter().any(|app| app.name == named.name) {
            let names: Vec<&str> = app_list.apps.iter().map(|app| app.name.as_str()).collect();
            let help = if names.is_empty() {
                "add an app definition or run `impresario app add`".to_string()
            } else {
                format!("available apps: {}", names.join(", "))
            };
            all.push(
                Diagnostic::error(Code::AppUnknown, format!("unknown app '{}'", named.name))
                    .with_span(named.span.clone())
                    .with_help(help),
            );
        }
    }

    ValidateOutcome {
        diagnostics: all,
        config_ok: true,
        devices_ok: true,
        apps_ok: true,
        scenes: loaded.workspace.scenes.len(),
        theatres: loaded.workspace.theatres.len(),
        plays: loaded.workspace.plays.len(),
        devices: devices.devices.len(),
        apps: app_list.apps.len(),
    }
}

fn cmd_fmt(base: &Path, config_path: &Path, paths: &[PathBuf], check: bool) -> i32 {
    let cfg_outcome = load_effective_config(base, config_path);
    if cfg_outcome.diagnostics.has_errors() {
        render::eprint_all(&cfg_outcome.diagnostics);
        return EXIT_USAGE;
    }
    let Some(cfg) = cfg_outcome.config else {
        eprintln!("fmt: cannot load configuration");
        return EXIT_USAGE;
    };

    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![
            base.join(&cfg.scenes_dir),
            base.join(&cfg.theatres_dir),
            base.join(&cfg.plays_dir),
        ]
    } else {
        paths
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    base.join(path)
                }
            })
            .collect()
    };

    let mut files: Vec<PathBuf> = Vec::new();
    for root in &roots {
        if root.is_dir() {
            if let Err(error) = collect_dsl_files(root, &mut files) {
                eprintln!("fmt: {error}");
                return EXIT_INFRA;
            }
        } else if is_dsl_file(root) {
            files.push(root.clone());
        } else if root.exists() || paths.is_empty() {
            // Configured directories may be absent; explicit paths may not.
            if !paths.is_empty() {
                eprintln!(
                    "fmt: {} is neither a .scene, .theatre nor .play file",
                    root.display()
                );
                return EXIT_USAGE;
            }
        } else {
            eprintln!("fmt: {} does not exist", root.display());
            return EXIT_USAGE;
        }
    }
    files.sort();
    files.dedup();

    let mut failed = false;
    let mut changed = false;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("fmt: cannot read {}: {error}", file.display());
                failed = true;
                continue;
            }
        };
        match impresario_core::fmt::format_source(&source, file) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => {
                changed = true;
                if check {
                    println!("would reformat {}", file.display());
                } else if let Err(error) = write_file(file, &formatted) {
                    eprintln!("fmt: {error}");
                    failed = true;
                }
            }
            Err(diagnostics) => {
                render::eprint_all(&diagnostics);
                failed = true;
            }
        }
    }
    if failed {
        return EXIT_USAGE;
    }
    if check && changed {
        EXIT_FAILURE
    } else {
        EXIT_OK
    }
}

fn is_dsl_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "scene" | "theatre" | "play"))
}

fn collect_dsl_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    let mut entries: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_dsl_files(&path, out)?;
        } else if is_dsl_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn cmd_tags(base: &Path, config_path: &Path, json: bool) -> i32 {
    let cfg_outcome = load_effective_config(base, config_path);
    render::eprint_all(&cfg_outcome.diagnostics);
    let Some(cfg) = cfg_outcome.config else {
        eprintln!("tags: cannot load configuration");
        return EXIT_USAGE;
    };
    let loaded = workspace::load_dirs(
        &base.join(&cfg.scenes_dir),
        &base.join(&cfg.theatres_dir),
        &base.join(&cfg.plays_dir),
    );
    if loaded.diagnostics.has_errors() {
        render::eprint_all(&loaded.diagnostics);
        return EXIT_USAGE;
    }

    #[derive(Default)]
    struct TagEntry {
        files: BTreeSet<String>,
        usages: Vec<String>,
    }
    let mut index: BTreeMap<String, TagEntry> = BTreeMap::new();
    let mut add = |tag: &str, file: &Path, usage: String| {
        let entry = index.entry(tag.to_string()).or_default();
        entry
            .files
            .insert(file.to_string_lossy().replace('\\', "/"));
        if !entry.usages.contains(&usage) {
            entry.usages.push(usage);
        }
    };
    for play in &loaded.workspace.plays {
        for tag in &play.tags {
            add(&tag.name, &play.path, format!("play {}", play.name));
        }
        for act in &play.acts {
            for tag in &act.tags {
                add(
                    &tag.name,
                    &play.path,
                    format!("act {} / {}", play.name, act.name),
                );
            }
        }
    }
    for theatre in &loaded.workspace.theatres {
        for tag in &theatre.tags {
            add(
                &tag.name,
                &theatre.path,
                format!("theatre {}", theatre.name),
            );
        }
        for stage in &theatre.stages {
            for tag in &stage.tags {
                add(
                    &tag.name,
                    &theatre.path,
                    format!("stage {}.{}", theatre.name, stage.name),
                );
            }
        }
    }

    if json {
        let tags: Vec<serde_json::Value> = index
            .iter()
            .map(|(tag, entry)| {
                serde_json::json!({
                    "tag": tag,
                    "count": entry.usages.len(),
                    "files": entry.files,
                    "usages": entry.usages,
                })
            })
            .collect();
        match serde_json::to_string_pretty(&serde_json::json!({ "tags": tags })) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("tags: cannot render JSON: {error}");
                return EXIT_INFRA;
            }
        }
        return EXIT_OK;
    }
    if index.is_empty() {
        println!("tags: none declared");
        return EXIT_OK;
    }
    for (tag, entry) in &index {
        println!(
            "{}  ({} usage{}, {} file{})",
            tag,
            entry.usages.len(),
            if entry.usages.len() == 1 { "" } else { "s" },
            entry.files.len(),
            if entry.files.len() == 1 { "" } else { "s" }
        );
        for file in &entry.files {
            println!("  file  {file}");
        }
        for usage in &entry.usages {
            println!("  use   {usage}");
        }
    }
    EXIT_OK
}

fn cmd_validate(base: &Path, config_path: &Path, devices_path: &Path, paths: &[PathBuf]) -> i32 {
    let outcome = validate_workspace(base, config_path, devices_path, paths);
    render::eprint_all(&outcome.diagnostics);
    if !outcome.config_ok {
        eprintln!(
            "validate: configuration errors in {}",
            config_path.display()
        );
        return EXIT_USAGE;
    }
    if !outcome.devices_ok {
        eprintln!(
            "validate: configuration errors in {}",
            devices_path.display()
        );
        return EXIT_USAGE;
    }
    if !outcome.apps_ok {
        eprintln!(
            "validate: configuration errors in {}",
            base.join("impresario.apps.config").display()
        );
        return EXIT_USAGE;
    }
    let errors = outcome.diagnostics.errors().count();
    let warnings = outcome.diagnostics.warnings().count();
    if errors > 0 {
        eprintln!("validate: {errors} error(s), {warnings} warning(s)");
        return EXIT_USAGE;
    }
    println!(
        "validate: ok ({} scene(s), {} theatre(s), {} play(s), {} device(s), {} app(s), {} warning(s))",
        outcome.scenes,
        outcome.theatres,
        outcome.plays,
        outcome.devices,
        outcome.apps,
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
        "theatre" => {
            let outcome = parse_theatre(&source, &path);
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
                "parse: {} is neither a .scene, .theatre nor .play file",
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

fn cmd_devices_check(
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    names: &[String],
) -> i32 {
    let config_outcome = load_effective_config(base, config_path);
    render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        eprintln!(
            "devices check: configuration errors in {}",
            config_path.display()
        );
        return EXIT_USAGE;
    };
    let outcome = config::load_devices_merged(devices_path);
    render::eprint_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        eprintln!(
            "devices check: configuration errors in {}",
            devices_path.display()
        );
        return EXIT_USAGE;
    };
    let selected = match select_devices_with_groups(&devices.devices, &devices.groups, names) {
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
    let results: Vec<Result<DeviceCheck, String>> = runtime.block_on(async {
        let mut results = Vec::new();
        for device in &selected {
            results.push(
                check_device(device, None, timeout_ms, token.as_deref(), &master_version).await,
            );
        }
        results
    });

    let mut failures = 0usize;
    for (device, result) in selected.iter().zip(results) {
        match result {
            Ok(check) => {
                let info = &check.info;
                println!(
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
                );
                for detail in check.details {
                    println!("  {detail}");
                }
                for warning in check.warnings {
                    eprintln!("  warning: {warning}");
                }
            }
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

/// Result of one `devices check` probe. Native transports report extra facts
/// (DUID, staging path, certificate state) that the executor does not know.
pub(crate) struct DeviceCheck {
    pub info: DeviceInfo,
    pub details: Vec<String>,
    pub warnings: Vec<String>,
}

/// Native probe, or transport prepare → connect → handshake → `DeviceInfo` →
/// session shutdown, then teardown (spec 8.1); every failure is a message,
/// never a panic.
pub(crate) async fn check_device(
    device: &DeviceConfig,
    app: Option<&AppConfig>,
    timeout_ms: u64,
    token: Option<&str>,
    master_version: &str,
) -> Result<DeviceCheck, String> {
    let mut transport = transport_for(device.platform);
    let outcome = check_transport(
        &mut *transport,
        device,
        app,
        timeout_ms,
        token,
        master_version,
    )
    .await;
    let teardown = transport
        .teardown()
        .await
        .map_err(|error| error.to_string());
    match outcome {
        Ok(check) => teardown.map(|()| check),
        Err(message) => {
            let _ = teardown;
            Err(message)
        }
    }
}

async fn check_transport(
    transport: &mut dyn DeviceTransport,
    device: &DeviceConfig,
    app: Option<&AppConfig>,
    timeout_ms: u64,
    token: Option<&str>,
    master_version: &str,
) -> Result<DeviceCheck, String> {
    if app.is_none() {
        if let Some(probe) = transport
            .probe(device)
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(DeviceCheck {
                info: probe.info,
                details: probe.details,
                warnings: probe.warnings,
            });
        }
    }
    let endpoint = transport
        .prepare(device, app)
        .await
        .map_err(|error| error.to_string())?;
    let client = AgentClient::connect_retry(endpoint.address(), Duration::from_millis(timeout_ms))
        .await
        .map_err(|error| error.to_string())?;
    let client = client.with_token(token.map(str::to_string));
    let mut params = HandshakeParams::new(
        device.name.clone(),
        device.platform,
        transport.target_endpoint(device),
        master_version,
    );
    let entry = app.and_then(|app| app.platform(device.platform));
    params.app_id = entry.and_then(|entry| entry.id.clone());
    params.app_url = entry.and_then(|entry| entry.url.clone());
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
    let info = handshake
        .device
        .ok_or_else(|| "the executor did not report DeviceInfo".to_string())?;
    Ok(DeviceCheck {
        info,
        details: Vec::new(),
        warnings: Vec::new(),
    })
}

/// The devices an invocation selected: `all` (or nothing) means everything,
/// otherwise the names in the order given (duplicates collapse).
pub(crate) fn select_devices(
    devices: &[DeviceConfig],
    requested: &[String],
) -> Result<Vec<DeviceConfig>, String> {
    select_devices_with_groups(devices, &std::collections::BTreeMap::new(), requested)
}

/// Select devices by name or `@group`. Groups expand from the merged devices
/// config, and `all` (or no selection) still means every configured device.
pub(crate) fn select_devices_with_groups(
    devices: &[DeviceConfig],
    groups: &std::collections::BTreeMap<String, Vec<String>>,
    requested: &[String],
) -> Result<Vec<DeviceConfig>, String> {
    if requested.is_empty() || requested.iter().any(|name| name == "all") {
        return Ok(devices.to_vec());
    }
    let mut selected: Vec<DeviceConfig> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    for requested_name in requested {
        let names: std::collections::BTreeSet<String> = if requested_name.starts_with('@') {
            match config::expand_actor_ref(devices, groups, requested_name) {
                Ok(names) => names,
                Err(_) => {
                    unknown.push(requested_name.as_str());
                    continue;
                }
            }
        } else {
            std::iter::once(requested_name.clone()).collect()
        };
        let mut missing = false;
        for resolved in names {
            match devices.iter().find(|device| device.name == resolved) {
                Some(device) => {
                    if !selected.iter().any(|chosen| chosen.name == device.name) {
                        selected.push(device.clone());
                    }
                }
                None => missing = true,
            }
        }
        if missing {
            unknown.push(requested_name.as_str());
        }
    }
    if !unknown.is_empty() {
        let known: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
        let groups: Vec<&str> = groups.keys().map(String::as_str).collect();
        return Err(format!(
            "unknown device/group(s): {} (configured devices: {}; groups: {})",
            unknown.join(", "),
            if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            },
            if groups.is_empty() {
                "none".to_string()
            } else {
                groups.join(", ")
            }
        ));
    }
    Ok(selected)
}

/// `IMPRESARIO_AGENT_TOKEN` (spec 7.2.1); empty values count as unset.
pub(crate) fn agent_token() -> Option<String> {
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

/// Core of `run`, shared by the CLI and the MCP server.
#[derive(Debug)]
pub(crate) struct RunOutcome {
    pub exit_code: i32,
    pub run_id: Option<String>,
    pub run_dir: Option<PathBuf>,
    pub report: Option<RunReport>,
    /// Pre-run refusals and reporter errors; the same text went to stderr.
    pub messages: Vec<String>,
    /// Diagnostics behind a refusal, one per line. The CLI already printed
    /// them through `render::eprint_all`; the MCP server surfaces them in
    /// the tool error instead of leaving only a count.
    pub detail: Option<String>,
}

impl RunOutcome {
    fn refusal(exit_code: i32, message: String) -> Self {
        Self {
            exit_code,
            run_id: None,
            run_dir: None,
            report: None,
            messages: vec![message],
            detail: None,
        }
    }

    fn refusal_with_detail(exit_code: i32, message: String, detail: String) -> Self {
        Self {
            detail: Some(detail),
            ..Self::refusal(exit_code, message)
        }
    }
}

/// One line per diagnostic, for refusals whose details the CLI already
/// printed but which a non-CLI caller (the MCP server) should receive too.
fn diagnostics_detail(diagnostics: &Diagnostics) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.one_line())
        .collect::<Vec<_>>()
        .join("\n")
}

/// No subcommand: enter the TUI on a real terminal, otherwise print a plain
/// dashboard. This path never prompts and never waits for input.
fn cmd_home(home: &Home, base: &Path, config_path: &Path, devices_path: &Path, cli: &Cli) -> i32 {
    use std::io::IsTerminal as _;

    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let ci = std::env::var("CI").is_ok_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    });
    let color = !cli.no_color
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").map_or(true, |term| !term.eq_ignore_ascii_case("dumb"));
    if tty && !ci && !cli.no_tui && !cli.plain {
        let context = crate::tui::Context::new(
            home.clone(),
            base.to_path_buf(),
            config_path.to_path_buf(),
            devices_path.to_path_buf(),
            color,
        );
        return crate::tui::run(context);
    }

    let reason = if cli.no_tui {
        "--no-tui"
    } else if cli.plain {
        "--plain"
    } else if ci {
        "CI is set"
    } else {
        "stdin or stdout is not a terminal"
    };
    let jobs = crate::jobs::JobStore::new(home).list();
    let devices = config::load_devices(devices_path)
        .config
        .map_or(0, |devices| devices.devices.len());
    let apps = apps::load_apps_merged(&base.join("impresario.apps.config"))
        .config
        .map_or(0, |apps| apps.apps.len());
    let active = jobs
        .iter()
        .filter(|record| !record.status.terminal())
        .count();
    println!("impresario");
    println!("  workspace   {}", base.display());
    println!("  config      {}", config_path.display());
    println!(
        "  devices     {} configured in {}",
        devices,
        devices_path.display()
    );
    println!("  apps        {apps} configured");
    println!("  jobs        {active} active, {} total", jobs.len());
    println!("  ui          plain output ({reason})");
    if !jobs.is_empty() {
        println!();
        println!("recent jobs");
        for record in jobs.iter().take(5) {
            println!(
                "  {:<22} {:<12} {}",
                record.id,
                record.status.label(),
                record.duration_ms.map_or_else(
                    || record.created_at.clone(),
                    |millis| format!("{} ms", millis)
                )
            );
        }
    }
    println!();
    println!("next");
    println!("  impresario doctor");
    println!("  impresario tests --tag smoke");
    println!("  impresario run --detach --tag smoke");
    println!("  impresario jobs --json");
    EXIT_OK
}

fn cmd_run(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    args: &RunArgs,
    no_color: bool,
) -> i32 {
    use std::io::IsTerminal;

    let mut args = args.clone();
    if let Some(profile) = args.profile.clone() {
        if let Err(message) = crate::profile_cmd::apply(home, &profile, &mut args) {
            eprintln!("run: {message}");
            return EXIT_USAGE;
        }
        eprintln!("run: using profile '{profile}'");
    }
    let color = !no_color && std::io::stdout().is_terminal();
    run_workspace(base, config_path, devices_path, &args, color, true).exit_code
}

/// Plan and execute `args`. `color` styles the console reporter (the CLI
/// prints it, the MCP server never selects it); `interrupts` installs the
/// Ctrl-C watcher, which only the CLI wants (the MCP server's Ctrl-C keeps
/// its default meaning for the whole session).
pub(crate) fn run_workspace(
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    args: &RunArgs,
    color: bool,
    interrupts: bool,
) -> RunOutcome {
    let config_outcome = load_effective_config(base, config_path);
    render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        let message = format!("run: configuration errors in {}", config_path.display());
        eprintln!("{message}");
        return RunOutcome::refusal_with_detail(
            EXIT_USAGE,
            message,
            diagnostics_detail(&config_outcome.diagnostics),
        );
    };
    let mut config = config;
    if let Some(policy) = args.unverifiable {
        config.unverifiable_policy = match policy {
            UnverifiableArg::Fail => impresario_core::probe::UnverifiablePolicy::Fail,
            UnverifiableArg::Warn => impresario_core::probe::UnverifiablePolicy::Warn,
        };
    }
    let devices_outcome = config::load_devices_merged(devices_path);
    render::eprint_all(&devices_outcome.diagnostics);
    let Some(device_list) = devices_outcome.config else {
        let message = format!("run: configuration errors in {}", devices_path.display());
        eprintln!("{message}");
        return RunOutcome::refusal_with_detail(
            EXIT_USAGE,
            message,
            diagnostics_detail(&devices_outcome.diagnostics),
        );
    };
    let apps_path = base.join("impresario.apps.config");
    let apps_outcome = apps::load_apps_merged(&apps_path);
    render::eprint_all(&apps_outcome.diagnostics);
    let Some(app_list) = apps_outcome.config else {
        let message = format!("run: configuration errors in {}", apps_path.display());
        eprintln!("{message}");
        return RunOutcome::refusal_with_detail(
            EXIT_USAGE,
            message,
            diagnostics_detail(&apps_outcome.diagnostics),
        );
    };

    let reporter_list = match &args.reporter {
        Some(spec) => match reporters::parse_list(spec) {
            Ok(list) => list,
            Err(message) => {
                eprintln!("run: {message}");
                return RunOutcome::refusal(EXIT_USAGE, format!("run: {message}"));
            }
        },
        None => config.reporters.clone(),
    };
    if let Some(timeout) = args.timeout {
        if !plan::timeout_in_bounds(timeout) {
            let message =
                format!("run: --timeout must be between 100 and 600000 ms (got {timeout})");
            eprintln!("{message}");
            return RunOutcome::refusal(EXIT_USAGE, message);
        }
    }
    if let Some(retries) = args.retries {
        if !plan::retries_in_bounds(retries) {
            let message = format!("run: --retries must be between 0 and 5 (got {retries})");
            eprintln!("{message}");
            return RunOutcome::refusal(EXIT_USAGE, message);
        }
    }

    let mut selected_devices =
        match select_devices_with_groups(&device_list.devices, &device_list.groups, &args.device) {
            Ok(selected) => selected,
            Err(message) => {
                eprintln!("run: {message}");
                return RunOutcome::refusal(EXIT_USAGE, format!("run: {message}"));
            }
        };
    if selected_devices.is_empty() {
        let message = format!(
            "run: no devices configured (see {})",
            devices_path.display()
        );
        eprintln!("{message}");
        return RunOutcome::refusal(EXIT_USAGE, message);
    }
    let mut platform_filter: Vec<String> = Vec::new();
    for raw in &args.platform {
        for name in raw.split(',') {
            let name = name.trim();
            if !name.is_empty() {
                platform_filter.push(name.to_ascii_lowercase());
            }
        }
    }
    if !platform_filter.is_empty() {
        let unknown: Vec<&str> = platform_filter
            .iter()
            .map(String::as_str)
            .filter(|name| {
                !impresario_core::platforms::Platform::ALL
                    .iter()
                    .any(|platform| platform.as_str() == *name)
            })
            .collect();
        if !unknown.is_empty() {
            let message = format!(
                "run: unknown platform (expected web, webos, tizen, android, androidtv, ios or tvos; got {})",
                unknown.join(", ")
            );
            eprintln!("{message}");
            return RunOutcome::refusal(EXIT_USAGE, message);
        }
        let filtered: Vec<DeviceConfig> = selected_devices
            .into_iter()
            .filter(|device| {
                platform_filter
                    .iter()
                    .any(|name| name == device.platform.as_str())
            })
            .collect();
        if filtered.is_empty() {
            let message = format!(
                "run: none of the selected devices run on {}",
                platform_filter.join(", ")
            );
            eprintln!("{message}");
            return RunOutcome::refusal(EXIT_USAGE, message);
        }
        selected_devices = filtered;
    }
    if let Some(diagnostic) = config::check_report_output(&base.join(&config.report_output)) {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(diagnostic);
        render::eprint_all(&diagnostics);
        let message = "run: report_output is not usable; nothing was executed".to_string();
        eprintln!("{message}");
        return RunOutcome::refusal_with_detail(
            EXIT_USAGE,
            message,
            diagnostics_detail(&diagnostics),
        );
    }

    // Workspace: scenes come from the configured scenes_dir, plays from
    // --play patterns or plays_dir (spec 8.1).
    let scenes = workspace::load_scenes(&base.join(&config.scenes_dir));
    let theatres = workspace::load_theatres(&base.join(&config.theatres_dir));
    let (play_patterns, inline_acts) = split_play_act_filters(&args.play);
    let plays = load_play_selection(base, &config, &play_patterns);
    let mut load_diagnostics = scenes.diagnostics;
    load_diagnostics.extend(theatres.diagnostics);
    load_diagnostics.extend(plays.diagnostics);
    let ws = workspace::Workspace {
        scenes: scenes.workspace.scenes,
        theatres: theatres.workspace.theatres,
        plays: plays.workspace.plays,
    };
    render::eprint_all(&load_diagnostics);
    if load_diagnostics.has_errors() {
        let message = format!(
            "run: {} workspace error(s); nothing was executed",
            load_diagnostics.errors().count()
        );
        eprintln!("{message}");
        return RunOutcome::refusal_with_detail(
            EXIT_USAGE,
            message,
            diagnostics_detail(&load_diagnostics),
        );
    }

    let mut acts = args.act.clone();
    acts.extend(inline_acts);
    let mut selection = plan::Selection {
        tags: args.tag.clone(),
        exclude_tags: args.exclude_tag.clone(),
        acts,
    };
    let mut selected_plays = ws.plays.clone();
    if args.only_failed {
        let Some(failed) = jobs::last_failed(base, &config.report_output) else {
            let message = format!(
                "run: --only-failed found no previous report under {}",
                base.join(&config.report_output).display()
            );
            eprintln!("{message}");
            return RunOutcome::refusal(EXIT_USAGE, message);
        };
        if failed.is_empty() {
            println!("run: the newest report has no failed acts; nothing to run");
            return RunOutcome {
                exit_code: EXIT_OK,
                run_id: None,
                run_dir: None,
                report: None,
                messages: Vec::new(),
                detail: None,
            };
        }
        let failed_acts: Vec<String> = failed
            .iter()
            .filter_map(|key| key.split('\u{1f}').nth(1).map(str::to_string))
            .collect();
        let failed_files: Vec<&str> = failed
            .iter()
            .filter_map(|key| key.split('\u{1f}').next())
            .filter(|file| !file.is_empty())
            .collect();
        selected_plays.retain(|play| {
            let path = play.path.to_string_lossy().replace('\\', "/");
            failed_files.iter().any(|file| {
                let file = file.replace('\\', "/");
                path == file || path.ends_with(&file) || file.ends_with(&path)
            })
        });
        if selected_plays.is_empty() {
            // The report may store only an Act name; fall back to the name
            // filter over the current selection.
            selected_plays = ws.plays.clone();
            selection.acts = failed_acts.clone();
        } else if !failed_acts.is_empty() {
            selection.acts = failed_acts;
        }
        if selection.acts.is_empty() || selected_plays.is_empty() {
            println!("run: the newest report has no retryable failed acts; nothing to run");
            return RunOutcome {
                exit_code: EXIT_OK,
                run_id: None,
                run_dir: None,
                report: None,
                messages: Vec::new(),
                detail: None,
            };
        }
    }
    let selection = selection;
    let cli_vars = match plan::parse_cli_vars(&args.var) {
        Ok(vars) => vars,
        Err(diagnostics) => {
            render::eprint_all(&diagnostics);
            let message = "run: invalid --var; nothing was executed".to_string();
            eprintln!("{message}");
            return RunOutcome::refusal_with_detail(
                EXIT_USAGE,
                message,
                diagnostics_detail(&diagnostics),
            );
        }
    };
    let cli_args = match plan::parse_cli_args(&args.arg) {
        Ok(cli_args) => cli_args,
        Err(diagnostics) => {
            render::eprint_all(&diagnostics);
            let message = "run: invalid --arg; nothing was executed".to_string();
            eprintln!("{message}");
            return RunOutcome::refusal_with_detail(
                EXIT_USAGE,
                message,
                diagnostics_detail(&diagnostics),
            );
        }
    };
    let input = plan::PlanInput {
        config: &config,
        workspace: &ws,
        base,
        devices: &selected_devices,
        all_devices: &device_list.devices,
        groups: &device_list.groups,
        apps: &app_list.apps,
        cli_app: args.app.as_deref(),
        plays: &selected_plays,
        selection: &selection,
        cli_vars: &cli_vars,
        cli_args: &cli_args,
        cli_timeout: args.timeout,
    };
    let plan = match plan::build_plan(&input) {
        Ok(plan) => plan,
        Err(diagnostics) => {
            render::eprint_all(&diagnostics);
            let message = format!(
                "run: {} planning error(s); nothing was executed",
                diagnostics.errors().count()
            );
            eprintln!("{message}");
            return RunOutcome::refusal_with_detail(
                EXIT_USAGE,
                message,
                diagnostics_detail(&diagnostics),
            );
        }
    };

    if args.dry_run {
        print_dry_run(
            &plan,
            &config,
            &reporter_list,
            &selection,
            &ws,
            &selected_devices,
            &selected_plays,
            &platform_filter,
        );
        return RunOutcome {
            exit_code: EXIT_OK,
            run_id: None,
            run_dir: None,
            report: None,
            messages: Vec::new(),
            detail: None,
        };
    }

    let run_id = time_fmt::run_id(std::time::SystemTime::now());
    let report_dir = base.join(&config.report_output);
    if interrupts {
        let home = match Home::resolve() {
            Ok(home) => home,
            Err(error) => {
                let message = format!("run: {error}");
                eprintln!("{message}");
                return RunOutcome::refusal(EXIT_INFRA, message);
            }
        };
        let spec = jobs::JobSpec {
            id: run_id.clone(),
            base: base.to_path_buf(),
            report_dir: report_dir.clone(),
            config: config.clone(),
            plan: plan.clone(),
            retries: args.retries.unwrap_or(config.retries),
            fail_fast: args.fail_fast,
            token: agent_token(),
            master_version: env!("CARGO_PKG_VERSION").to_string(),
            reporters: reporter_list.clone(),
            color,
            on_busy: args.on_busy,
            detached: args.detach,
            label: format!(
                "{} play(s) x {} device(s)",
                selected_plays.len(),
                selected_devices.len()
            ),
        };
        if args.detach {
            return match jobs::submit_detached(&home, spec, false) {
                Ok(id) => {
                    println!("job {id} submitted");
                    println!("attach: impresario attach {id}");
                    println!("logs:   impresario logs {id} -f");
                    RunOutcome {
                        exit_code: EXIT_OK,
                        run_id: Some(id.clone()),
                        run_dir: Some(report_dir.join(id)),
                        report: None,
                        messages: Vec::new(),
                        detail: None,
                    }
                }
                Err(message) => {
                    eprintln!("run: {message}");
                    RunOutcome::refusal(EXIT_INFRA, format!("run: {message}"))
                }
            };
        }
        return match jobs::run_foreground(&home, &spec, true) {
            Ok(job) => {
                for message in &job.reporter_errors {
                    eprintln!("run: {message}");
                }
                RunOutcome {
                    exit_code: job.record.exit_code.unwrap_or(EXIT_INFRA),
                    run_id: Some(run_id),
                    run_dir: Some(PathBuf::from(&job.record.run_dir)),
                    report: job.report,
                    messages: job.reporter_errors,
                    detail: None,
                }
            }
            Err(message) => {
                eprintln!("run: {message}");
                RunOutcome::refusal(EXIT_INFRA, format!("run: {message}"))
            }
        };
    }
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
        events: None,
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let message = format!("run: cannot start the async runtime: {err}");
            eprintln!("{message}");
            return RunOutcome::refusal(EXIT_INFRA, message);
        }
    };
    let shutdown = options.shutdown.clone();
    let outcome = runtime.block_on(async move {
        let watcher = if interrupts {
            Some(tokio::spawn(watch_interrupts(shutdown)))
        } else {
            None
        };
        let outcome = exec::execute(&plan, &options).await;
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        outcome
    });

    let run_dir = report_dir.join(&run_id);
    let reporter_errors = reporters::write(&outcome.report, &run_dir, &reporter_list, color);
    for message in &reporter_errors {
        eprintln!("run: {message}");
    }
    let mut code = outcome.exit_code;
    if !reporter_errors.is_empty() && code == EXIT_OK {
        code = EXIT_INFRA;
    }
    RunOutcome {
        exit_code: code,
        run_id: Some(run_id),
        run_dir: Some(run_dir),
        report: Some(outcome.report),
        messages: reporter_errors,
        detail: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn print_dry_run(
    plan: &plan::ExecutionPlan,
    config: &ImpresarioConfig,
    reporter_list: &[impresario_core::config::Reporter],
    selection: &plan::Selection,
    workspace: &workspace::Workspace,
    devices: &[DeviceConfig],
    selected_plays: &[impresario_core::ast::PlayFile],
    platform_filter: &[String],
) {
    let device_count = plan.devices.len();
    let acts: usize = plan
        .devices
        .iter()
        .map(|device| {
            device
                .plays
                .iter()
                .map(|play| play.acts.len())
                .sum::<usize>()
        })
        .sum();
    println!("run (dry): {acts} act(s) on {device_count} device(s); nothing was executed");
    println!(
        "  selection: tags=[{}] exclude=[{}] acts=[{}]",
        selection.tags.join(", "),
        selection.exclude_tags.join(", "),
        selection.acts.join(", ")
    );
    let redactor = crate::redact::Redactor::new(plan.secrets.clone());
    for device in &plan.devices {
        let app = device
            .app
            .as_ref()
            .map(|app| format!(" app={}", app.name))
            .unwrap_or_default();
        println!(
            "  device {} ({}){app}",
            device.device.name,
            device.device.platform.as_str()
        );
        for play in &device.plays {
            println!("    play {} [{}]", play.name, play.file);
            for act in &play.acts {
                let tags = if act.tags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", act.tags.join(", "))
                };
                let example = if act.example.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {{{}}}",
                        act.example
                            .iter()
                            .map(|(key, value)| format!("{key}={}", redactor.redact(value)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let steps: usize = act.stages.iter().map(|stage| stage.steps.len()).sum();
                println!(
                    "      act {}{tags}{example}: {} stage(s), {steps} step(s)",
                    act.name,
                    act.stages.len()
                );
            }
        }
    }
    println!(
        "  reporters: {}",
        reporter_list
            .iter()
            .map(|reporter| reporter.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut skipped = Vec::new();
    for device in devices {
        for play in &workspace.plays {
            let play_selected = selected_plays
                .iter()
                .any(|selected| selected.path == play.path);
            for act in &play.acts {
                if plan_contains_act(plan, &device.name, &play.name, &act.name) {
                    continue;
                }
                let reason = if !play_selected {
                    "play not in the --only-failed selection".to_string()
                } else if let Some(reason) =
                    act_skip_reason(play, act, workspace, selection, device.platform)
                {
                    reason
                } else {
                    "not in the resolved plan".to_string()
                };
                skipped.push(format!(
                    "    {} :: {} :: {}: {}",
                    device.name, play.name, act.name, reason
                ));
            }
        }
    }
    if skipped.is_empty() {
        println!("  skipped: none");
    } else {
        println!("  skipped:");
        for line in skipped {
            println!("{line}");
        }
    }
    if !platform_filter.is_empty() {
        println!(
            "  platform filter: {} (devices outside it are absent from this plan)",
            platform_filter.join(", ")
        );
    }
    let _ = config;
}

fn plan_contains_act(plan: &plan::ExecutionPlan, device: &str, play: &str, act: &str) -> bool {
    plan.devices.iter().any(|planned| {
        planned.device.name == device
            && planned.plays.iter().any(|planned_play| {
                planned_play.name == play
                    && planned_play
                        .acts
                        .iter()
                        .any(|planned_act| planned_act.name == act)
            })
    })
}

fn act_skip_reason(
    play: &impresario_core::ast::PlayFile,
    act: &impresario_core::ast::ActDecl,
    workspace: &workspace::Workspace,
    selection: &plan::Selection,
    platform: Platform,
) -> Option<String> {
    let tags = plan::effective_tags(play, act, workspace);
    if let Some(excluded) = selection
        .exclude_tags
        .iter()
        .find(|excluded| tags.contains(*excluded))
    {
        return Some(format!("excluded by --exclude-tag {excluded}"));
    }
    if !selection.tags.is_empty() && !selection.tags.iter().any(|wanted| tags.contains(wanted)) {
        return Some(format!("filtered by --tag {}", selection.tags.join(", ")));
    }
    if !selection.acts.iter().any(|name| name == &act.name) && !selection.acts.is_empty() {
        return Some(format!("filtered by --act {}", selection.acts.join(", ")));
    }
    if !act.platforms.is_empty() {
        let applies = act.platforms.iter().any(|named| {
            named
                .name
                .parse::<Platform>()
                .is_ok_and(|declared| declared == platform)
        });
        if !applies {
            return Some(format!(
                "Platforms: {} excludes {}",
                act.platforms
                    .iter()
                    .map(|named| named.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                platform.as_str()
            ));
        }
    }
    None
}

/// Split `login#"Successful login"` into the play pattern and act name.
fn split_play_act_filters(patterns: &[String]) -> (Vec<String>, Vec<String>) {
    let mut plays = Vec::new();
    let mut acts = Vec::new();
    for pattern in patterns {
        match pattern.split_once('#') {
            Some((play, act)) => {
                if !play.trim().is_empty() {
                    plays.push(play.trim().to_string());
                }
                let act = act.trim();
                let act = act
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .or_else(|| {
                        act.strip_prefix('\'')
                            .and_then(|rest| rest.strip_suffix('\''))
                    })
                    .unwrap_or(act);
                if !act.is_empty() {
                    acts.push(act.to_string());
                }
            }
            None => plays.push(pattern.clone()),
        }
    }
    (plays, acts)
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
                // Run-by-name fallback: a file stem or the play's `Play:` name.
                let all = workspace::load_plays(&base.join(&config.plays_dir));
                let matches: Vec<PathBuf> = all
                    .workspace
                    .plays
                    .iter()
                    .filter(|play| {
                        let stem = play
                            .path
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .unwrap_or_default();
                        stem == pattern
                            || play.name == *pattern
                            || pattern
                                .strip_suffix(".play")
                                .is_some_and(|without| stem == without || play.name == without)
                            || play
                                .path
                                .to_string_lossy()
                                .replace('\\', "/")
                                .ends_with(&normalized)
                    })
                    .map(|play| play.path.clone())
                    .collect();
                match matches.as_slice() {
                    [] => {
                        let names: Vec<String> = all
                            .workspace
                            .plays
                            .iter()
                            .map(|play| {
                                play.path
                                    .file_stem()
                                    .and_then(|stem| stem.to_str())
                                    .unwrap_or(&play.name)
                                    .to_string()
                            })
                            .collect();
                        let mut diagnostic = Diagnostic::error(
                            Code::CfgInvalidValue,
                            format!(
                                "--play '{pattern}' is not a file, directory, file stem or Play: name"
                            ),
                        );
                        if let Some(hint) = impresario_core::diag::suggest(
                            pattern,
                            &names.iter().map(String::as_str).collect::<Vec<_>>(),
                        ) {
                            diagnostic = diagnostic.with_help(hint);
                        } else if !names.is_empty() {
                            diagnostic = diagnostic
                                .with_help(format!("available plays: {}", names.join(", ")));
                        }
                        diagnostics.push(diagnostic);
                    }
                    [path] => push_unique(&mut targets, path.clone()),
                    many => diagnostics.push(Diagnostic::error(
                        Code::CfgInvalidValue,
                        format!(
                            "--play '{pattern}' is ambiguous; it matches {}",
                            many.iter()
                                .map(|path| path.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )),
                }
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

fn cmd_lsp(base: &Path, config_path: &Path, devices_path: &Path) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("lsp: cannot start the async runtime: {err}");
            return EXIT_INFRA;
        }
    };
    runtime.block_on(crate::lsp::run_stdio(
        base.to_path_buf(),
        config_path.to_path_buf(),
        devices_path.to_path_buf(),
    ));
    EXIT_OK
}

/// `impresario mcp` (decision I-80, not in the spec CLI list): serve the
/// Model Context Protocol on stdio so AI agents can validate, run and debug
/// plays against the configured devices. Stdout is protocol-only.
fn cmd_mcp(base: &Path, config_path: &Path, devices_path: &Path) -> i32 {
    crate::mcp::run_stdio(
        base.to_path_buf(),
        config_path.to_path_buf(),
        devices_path.to_path_buf(),
    )
}

/// `impresario mock-agent` (decision I-77, not in the spec CLI list): serve
/// the scripted `MockAgent` so a clean clone can exercise `impresario run`
/// end to end without hardware. The "listening on" line goes to stdout (the
/// README and tests parse it); logs and errors stay on stderr.
fn cmd_mock_agent(port: u16) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("mock-agent: cannot start the async runtime: {err}");
            return EXIT_INFRA;
        }
    };
    runtime.block_on(async move {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let agent = match impresario_clients::testing::MockAgent::builder()
            .bind(address)
            .start()
            .await
        {
            Ok(agent) => agent,
            Err(err) => {
                eprintln!("mock-agent: cannot listen on {address}: {err}");
                return EXIT_INFRA;
            }
        };
        println!("impresario: mock device listening on {}", agent.address());
        eprintln!(
            "mock-agent: point a device here (connector: {}) and press Ctrl-C to stop",
            agent.address()
        );
        if tokio::signal::ctrl_c().await.is_err() {
            eprintln!("mock-agent: cannot install the Ctrl-C handler");
            return EXIT_INFRA;
        }
        eprintln!("mock-agent: shutting down");
        EXIT_OK
    })
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

    /// Keep the real `~/.impresario` out of the unit tests.
    fn test_home() -> &'static Path {
        static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            let home =
                std::env::temp_dir().join(format!("impresario-cli-home-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).expect("create test home");
            std::env::set_var("IMPRESARIO_HOME", &home);
            home
        })
    }

    fn argv(workspace: &Path, rest: &[&str]) -> Vec<String> {
        test_home();
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
        let code = run_with(&argv(&dir, &["workspace", "new", "--example"]));
        assert_eq!(code, EXIT_OK);
        for file in [
            "impresario.config",
            "impresario.devices.config",
            ".gitignore",
            "scenes/LoginScreen.scene",
            "scenes/DashboardScreen.scene",
            "theatres/login.theatre",
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
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example"])),
            EXIT_OK
        );
        std::fs::write(dir.join("plays/login.play"), "// user edit\n").expect("edit");
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example"])),
            EXIT_USAGE
        );
        let content = std::fs::read_to_string(dir.join("plays/login.play")).expect("read");
        assert_eq!(content, "// user edit\n");
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example", "--force"])),
            EXIT_OK
        );
        let content = std::fs::read_to_string(dir.join("plays/login.play")).expect("read");
        assert!(content.contains("Play: User Authentication Feature"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_reports_dsl_errors() {
        let dir = temp_workspace("validate-errors");
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example"])),
            EXIT_OK
        );
        std::fs::write(
            dir.join("plays/broken.play"),
            "Play: Broken\n\nAct: A\n    Perform: Missing\n",
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
    fn validate_without_configs_uses_defaults() {
        // Missing files are empty layers (TASK-2 4.2), so an empty workspace is
        // valid with the schema defaults.
        let dir = temp_workspace("validate-noconfig");
        let code = run_with(&argv(&dir, &["validate"]));
        assert_eq!(code, EXIT_OK);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unverifiable_flag_accepts_fail_and_warn() {
        let fail =
            Cli::try_parse_from(["impresario", "run", "--unverifiable", "fail", "--dry-run"])
                .expect("parse fail");
        match fail.command {
            Some(Command::Run(args)) => {
                assert!(matches!(args.unverifiable, Some(UnverifiableArg::Fail)))
            }
            _ => panic!("expected run"),
        }
        let warn =
            Cli::try_parse_from(["impresario", "run", "--unverifiable", "warn", "--dry-run"])
                .expect("parse warn");
        match warn.command {
            Some(Command::Run(args)) => {
                assert!(matches!(args.unverifiable, Some(UnverifiableArg::Warn)))
            }
            _ => panic!("expected run"),
        }
        assert!(Cli::try_parse_from(["impresario", "run", "--unverifiable", "sometimes"]).is_err());
    }

    #[test]
    fn parse_accepts_both_file_kinds() {
        let dir = temp_workspace("parse");
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example"])),
            EXIT_OK
        );
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
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example"])),
            EXIT_OK
        );
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
    fn select_devices_all_names_and_unknown() {
        use impresario_core::platforms::Platform;
        let device = |name: &str| {
            let mut device = DeviceConfig::new(name, Platform::Android, "192.168.1.60");
            device.connection.serial = Some("emulator-5554".to_string());
            device
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
        assert_eq!(
            run_with(&argv(&dir, &["workspace", "new", "--example"])),
            EXIT_OK
        );
        std::fs::write(
            dir.join("plays/extra.play"),
            "Play: Extra\n\nAct: A\n    Perform: NavigateAndLogin\n",
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
