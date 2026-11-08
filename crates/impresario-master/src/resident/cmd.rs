//! CLI surface for schedules, watchers, notifications and the resident service.

use std::io::BufRead as _;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use impresario_core::home::Home;
use impresario_core::resident::{
    BusyPolicy, MissedPolicy, NotifySpec, Pacing, ResidentSections, RunSelection, Schedule,
    ScheduleWhen, Watcher, WatcherLease,
};

use crate::cli::{EXIT_FAILURE, EXIT_INFRA, EXIT_OK, EXIT_USAGE};

use super::config::{self, Kind, Layer};
use super::{notify, service};

#[derive(Debug, Args)]
pub struct ScheduleArgs {
    #[command(subcommand)]
    pub command: ScheduleCommand,
}

#[derive(Debug, Subcommand)]
pub enum ScheduleCommand {
    /// List effective schedules with their next fire.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one schedule as YAML.
    Show { name: String },
    /// Print the next fire times (all schedules unless a name is given).
    Next {
        name: Option<String>,
        #[arg(long, default_value_t = 5)]
        count: usize,
    },
    /// Add a schedule from YAML or from flags.
    Add(AddScheduleArgs),
    /// Replace a schedule from YAML or open the layer in $EDITOR.
    Edit(EditScheduleArgs),
    /// Remove a schedule.
    Rm {
        name: String,
        #[arg(long)]
        global: bool,
    },
    /// Enable a schedule in its layer.
    Enable {
        name: String,
        #[arg(long)]
        global: bool,
    },
    /// Disable a schedule in its layer without removing it.
    Disable {
        name: String,
        #[arg(long)]
        global: bool,
    },
    /// Run one schedule immediately, outside its next-fire time.
    RunNow { name: String },
}

#[derive(Debug, Args)]
pub struct AddScheduleArgs {
    pub name: String,
    /// Full schedule YAML from a file.
    #[arg(long, value_name = "FILE", conflicts_with = "yaml")]
    pub file: Option<PathBuf>,
    /// Full schedule YAML on the command line.
    #[arg(long, conflicts_with = "file")]
    pub yaml: Option<String>,
    /// Short form: the `when` mapping, for example `kind: daily, at: "02:30"`.
    #[arg(long, value_name = "YAML", conflicts_with_all = ["file", "yaml"])]
    pub when: Option<String>,
    /// Run only this play (repeatable).
    #[arg(long = "play", value_name = "NAME|GLOB")]
    pub plays: Vec<String>,
    /// Run only acts with this tag (repeatable).
    #[arg(long)]
    pub tag: Vec<String>,
    #[arg(long)]
    pub exclude_tag: Vec<String>,
    #[arg(long)]
    pub actor: Vec<String>,
    #[arg(long)]
    pub app: Option<String>,
    #[arg(long)]
    pub retries: Option<u32>,
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long, value_enum, default_value_t = MissedArg::Skip)]
    pub on_missed: MissedArg,
    #[arg(long, value_enum, default_value_t = BusyArg::Queue)]
    pub on_busy: BusyArg,
    #[arg(long)]
    pub disabled: bool,
    #[arg(long)]
    pub global: bool,
}

#[derive(Debug, Args)]
pub struct EditScheduleArgs {
    pub name: Option<String>,
    #[arg(long, value_name = "FILE", conflicts_with = "yaml")]
    pub file: Option<PathBuf>,
    #[arg(long, conflicts_with = "file")]
    pub yaml: Option<String>,
    #[arg(long)]
    pub global: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum MissedArg {
    Skip,
    RunOnce,
}

impl From<MissedArg> for MissedPolicy {
    fn from(value: MissedArg) -> Self {
        match value {
            MissedArg::Skip => Self::Skip,
            MissedArg::RunOnce => Self::RunOnce,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LeaseArg {
    Gap,
    #[value(name = "no-gap")]
    NoGap,
}

impl From<LeaseArg> for WatcherLease {
    fn from(value: LeaseArg) -> Self {
        match value {
            LeaseArg::Gap => Self::Gap,
            LeaseArg::NoGap => Self::NoGap,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BusyArg {
    Queue,
    Skip,
    Fail,
}

impl From<BusyArg> for BusyPolicy {
    fn from(value: BusyArg) -> Self {
        match value {
            BusyArg::Queue => Self::Queue,
            BusyArg::Skip => Self::Skip,
            BusyArg::Fail => Self::Fail,
        }
    }
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    #[command(subcommand)]
    pub command: WatchCommand,
}

#[derive(Debug, Subcommand)]
pub enum WatchCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        name: String,
    },
    Add(AddWatchArgs),
    Edit(EditWatchArgs),
    Rm {
        name: String,
        #[arg(long)]
        global: bool,
    },
    Enable {
        name: String,
        #[arg(long)]
        global: bool,
    },
    Disable {
        name: String,
        #[arg(long)]
        global: bool,
    },
    Start {
        name: String,
        #[arg(long)]
        global: bool,
    },
    Stop {
        name: String,
        #[arg(long)]
        global: bool,
    },
    Pause {
        name: String,
    },
    Resume {
        name: String,
    },
    Status {
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Dumps {
        name: Option<String>,
    },
    RunNow {
        name: String,
    },
}

#[derive(Debug, Args)]
pub struct AddWatchArgs {
    pub name: String,
    #[arg(long, value_name = "FILE", conflicts_with = "yaml")]
    pub file: Option<PathBuf>,
    #[arg(long, conflicts_with = "file")]
    pub yaml: Option<String>,
    #[arg(long = "play", value_name = "NAME|GLOB")]
    pub plays: Vec<String>,
    #[arg(long)]
    pub tag: Vec<String>,
    #[arg(long)]
    pub exclude_tag: Vec<String>,
    #[arg(long)]
    pub actor: Vec<String>,
    #[arg(long)]
    pub app: Option<String>,
    #[arg(long)]
    pub retries: Option<u32>,
    #[arg(long)]
    pub profile: Option<String>,
    /// Minimum gap between iterations, e.g. `30s`; absent means back-to-back.
    #[arg(long)]
    pub gap: Option<String>,
    /// Device lease mode: `gap` releases between iterations; `no-gap` holds
    /// the devices for the watcher lifetime and can starve other runs.
    #[arg(long, value_enum, default_value_t = LeaseArg::Gap)]
    pub lease: LeaseArg,
    #[arg(long)]
    pub disabled: bool,
    #[arg(long)]
    pub global: bool,
}

#[derive(Debug, Args)]
pub struct EditWatchArgs {
    pub name: Option<String>,
    #[arg(long, value_name = "FILE", conflicts_with = "yaml")]
    pub file: Option<PathBuf>,
    #[arg(long, conflicts_with = "file")]
    pub yaml: Option<String>,
    #[arg(long)]
    pub global: bool,
}

#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(Debug, Args)]
pub struct NotifyArgs {
    #[command(subcommand)]
    pub command: NotifyCommand,
}

#[derive(Debug, Subcommand)]
pub enum NotifyCommand {
    /// List recent notification events.
    List {
        #[arg(long)]
        json: bool,
        /// Show at most this many of the newest events.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Open the report behind a notification or job id.
    Open {
        /// Notification id, job id, or a report path.
        id: String,
        /// Print the resolved report path without opening it.
        #[arg(long)]
        print: bool,
    },
    /// Manage webhook bearer secrets in the OS credential store.
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretCommand {
    /// Store a webhook secret read from stdin.
    Set {
        name: String,
        /// Read the secret from stdin (required; secrets are never arguments).
        #[arg(long)]
        stdin: bool,
    },
    /// Remove a stored webhook secret.
    Clear { name: String },
    /// Report whether a secret is stored, without printing it.
    Status { name: String },
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Run the resident runner in the foreground.
    Run,
    /// Install the per-user logon autostart hook.
    Install,
    /// Remove the logon hook (history/config stay unless --purge).
    Uninstall {
        #[arg(long)]
        purge: bool,
    },
    /// Start a detached resident runner for this workspace.
    Start,
    /// Ask the runner to stop.
    Stop,
    /// Stop and start again.
    Restart,
    /// Show the runner and its schedules/watchers.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Print or follow the resident log.
    Logs {
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Pause schedule and watcher execution.
    Pause,
    /// Resume schedule and watcher execution.
    Resume,
}

pub fn run_schedule(
    args: &ScheduleArgs,
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
) -> i32 {
    match &args.command {
        ScheduleCommand::List { json } => match config::load_effective(home, config_path) {
            Ok(sections) => {
                if *json {
                    print_json(&sections.schedules);
                } else {
                    list_schedules(&sections);
                }
                EXIT_OK
            }
            Err(error) => usage_error(&error),
        },
        ScheduleCommand::Show { name } => match config::load_effective(home, config_path) {
            Ok(sections) => match sections.schedules.iter().find(|value| value.name == *name) {
                Some(schedule) => match config::to_yaml(schedule) {
                    Ok(text) => {
                        println!("{text}");
                        EXIT_OK
                    }
                    Err(error) => usage_error(&error),
                },
                None => usage_error(&format!("no schedule named '{name}'")),
            },
            Err(error) => usage_error(&error),
        },
        ScheduleCommand::Next { name, count } => {
            next_schedules(home, config_path, name.as_deref(), *count)
        }
        ScheduleCommand::Add(args) => add_schedule(args, home, config_path),
        ScheduleCommand::Edit(args) => edit_schedule(args, home, config_path),
        ScheduleCommand::Rm { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Schedule,
            name,
            |layer, path| config::remove(path, layer, name),
        ),
        ScheduleCommand::Enable { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Schedule,
            name,
            |layer, path| config::set_enabled(path, layer, name, true),
        ),
        ScheduleCommand::Disable { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Schedule,
            name,
            |layer, path| config::set_enabled(path, layer, name, false),
        ),
        ScheduleCommand::RunNow { name } => {
            run_now_schedule(home, base, config_path, devices_path, name)
        }
    }
}

pub fn run_watch(
    args: &WatchArgs,
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
) -> i32 {
    match &args.command {
        WatchCommand::List { json } => match config::load_effective(home, config_path) {
            Ok(sections) => {
                if *json {
                    print_json(&sections.watchers);
                } else {
                    list_watchers(&sections);
                }
                EXIT_OK
            }
            Err(error) => usage_error(&error),
        },
        WatchCommand::Show { name } => match config::load_effective(home, config_path) {
            Ok(sections) => match sections.watchers.iter().find(|value| value.name == *name) {
                Some(watcher) => match config::to_yaml(watcher) {
                    Ok(text) => {
                        println!("{text}");
                        EXIT_OK
                    }
                    Err(error) => usage_error(&error),
                },
                None => usage_error(&format!("no watcher named '{name}'")),
            },
            Err(error) => usage_error(&error),
        },
        WatchCommand::Add(args) => add_watch(args, home, config_path),
        WatchCommand::Edit(args) => edit_watch(args, home, config_path),
        WatchCommand::Rm { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Watcher,
            name,
            |layer, path| config::remove(path, layer, name),
        ),
        WatchCommand::Enable { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Watcher,
            name,
            |layer, path| {
                config::set_enabled(path, layer, name, true)?;
                service::clear_watch_stop(home, name)?;
                service::request_pause_watch(home, name, false)
            },
        ),
        WatchCommand::Disable { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Watcher,
            name,
            |layer, path| config::set_enabled(path, layer, name, false),
        ),
        WatchCommand::Start { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Watcher,
            name,
            |layer, path| {
                config::set_enabled(path, layer, name, true)?;
                service::clear_watch_stop(home, name)?;
                service::request_pause_watch(home, name, false)
            },
        ),
        WatchCommand::Stop { name, global } => mutate(
            home,
            config_path,
            *global,
            Kind::Watcher,
            name,
            |layer, path| {
                config::set_enabled(path, layer, name, false)?;
                service::request_stop_watch(home, name)
            },
        ),
        WatchCommand::Pause { name } => service::request_pause_watch(home, name, true)
            .map(|()| EXIT_OK)
            .unwrap_or_else(|error| usage_error(&error)),
        WatchCommand::Resume { name } => service::request_pause_watch(home, name, false)
            .map(|()| EXIT_OK)
            .unwrap_or_else(|error| usage_error(&error)),
        WatchCommand::Status { name, json } => {
            watch_status(home, config_path, name.as_deref(), *json)
        }
        WatchCommand::Dumps { name } => watch_dumps(base, name.as_deref()),
        WatchCommand::RunNow { name } => run_now_watch(home, base, config_path, devices_path, name),
    }
}

pub fn run_notify(args: &NotifyArgs, home: &Home) -> i32 {
    match &args.command {
        NotifyCommand::List { json, limit } => match notify::notification_events(home, *limit) {
            Ok(events) => {
                if *json {
                    print_json(&events);
                } else if events.is_empty() {
                    println!("no notifications");
                } else {
                    for event in events {
                        println!("{}", notification_line(&event));
                    }
                }
                EXIT_OK
            }
            Err(error) => usage_error(&error),
        },
        NotifyCommand::Open { id, print } => match notify::resolve_report(home, id) {
            Ok(path) => {
                if *print {
                    println!("{}", path.display());
                    EXIT_OK
                } else {
                    match crate::open::open_external(&path) {
                        Ok(()) => {
                            println!("opened {}", path.display());
                            EXIT_OK
                        }
                        Err(error) => usage_error(&error),
                    }
                }
            }
            Err(error) => usage_error(&error),
        },
        NotifyCommand::Secret { command } => match command {
            SecretCommand::Set { name, stdin } => {
                if !notify::webhook_name_ok(name) {
                    return usage_error(&format!("'{name}' is not a valid webhook name"));
                }
                if !*stdin {
                    return usage_error(
                        "pass --stdin to read the webhook secret from standard input",
                    );
                }
                let mut source = String::new();
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                match input.read_line(&mut source) {
                    Ok(0) => {
                        return usage_error("no webhook secret was provided on stdin");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        return usage_error(&format!("cannot read the secret from stdin: {error}"));
                    }
                }
                let secret = source.trim_end_matches(&['\r', '\n'][..]);
                match notify::set_webhook_secret(name, secret) {
                    Ok(()) => {
                        println!("stored webhook '{name}' secret");
                        EXIT_OK
                    }
                    Err(error) => usage_error(&error),
                }
            }
            SecretCommand::Clear { name } => {
                if !notify::webhook_name_ok(name) {
                    return usage_error(&format!("'{name}' is not a valid webhook name"));
                }
                match notify::clear_webhook_secret(name) {
                    Ok(true) => {
                        println!("removed webhook '{name}' secret");
                        EXIT_OK
                    }
                    Ok(false) => {
                        println!("no stored secret for webhook '{name}'");
                        EXIT_OK
                    }
                    Err(error) => usage_error(&error),
                }
            }
            SecretCommand::Status { name } => {
                if !notify::webhook_name_ok(name) {
                    return usage_error(&format!("'{name}' is not a valid webhook name"));
                }
                let env_name = notify::env_webhook_secret_name(name);
                let environment =
                    std::env::var_os(&env_name).is_some_and(|value| !value.is_empty());
                match notify::stored_webhook_secret(name) {
                    Ok(stored) => {
                        println!(
                            "webhook '{name}': stored={} environment={} ({env_name})",
                            stored, environment
                        );
                        EXIT_OK
                    }
                    Err(error) => usage_error(&error),
                }
            }
        },
    }
}

fn notification_line(event: &serde_json::Value) -> String {
    let at = event
        .get("at")
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    let kind = event
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    let source = event
        .get("watcher")
        .or_else(|| event.get("schedule"))
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    let name = event
        .get("event")
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    let id = event
        .get("id")
        .or_else(|| event.get("job"))
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    let channels = event
        .get("channels")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    format!("{at} {kind} {source} {name} id={id} channels={channels}")
}

pub fn run_service(
    args: &ServiceArgs,
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
) -> i32 {
    match &args.command {
        ServiceCommand::Run => service::run_foreground(home, base, config_path, devices_path),
        ServiceCommand::Install => {
            match service::install_autostart(home, base, config_path, devices_path) {
                Ok(path) => {
                    println!("installed autostart: {}", path.display());
                    EXIT_OK
                }
                Err(error) => usage_error(&error),
            }
        }
        ServiceCommand::Uninstall { purge } => {
            if let Err(error) = service::request_stop(home) {
                eprintln!("service: {error}");
            }
            for _ in 0..100 {
                if !service::is_running(home) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            match service::uninstall_autostart() {
                Ok(Some(path)) => println!("removed autostart: {}", path.display()),
                Ok(None) => println!("no autostart hook found"),
                Err(error) => return usage_error(&error),
            }
            if *purge {
                if let Err(error) = std::fs::remove_dir_all(home.service_dir()) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        eprintln!(
                            "service: cannot purge {}: {error}",
                            home.service_dir().display()
                        );
                        return EXIT_INFRA;
                    }
                }
                println!("purged {}", home.service_dir().display());
            }
            EXIT_OK
        }
        ServiceCommand::Start => match service::start(home, base, config_path, devices_path) {
            Ok(pid) => {
                println!("resident runner started (pid {pid})");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("service: {error}");
                EXIT_INFRA
            }
        },
        ServiceCommand::Stop => stop_service(home),
        ServiceCommand::Restart => {
            let code = stop_service(home);
            if code != EXIT_OK {
                return code;
            }
            match service::start(home, base, config_path, devices_path) {
                Ok(pid) => {
                    println!("resident runner restarted (pid {pid})");
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("service: {error}");
                    EXIT_INFRA
                }
            }
        }
        ServiceCommand::Status { json } => service_status(home, config_path, *json),
        ServiceCommand::Logs { follow } => match service::print_logs(home, *follow) {
            Ok(()) => EXIT_OK,
            Err(error) => usage_error(&error),
        },
        ServiceCommand::Pause => service::request_pause(home, true)
            .map(|()| EXIT_OK)
            .unwrap_or_else(|error| usage_error(&error)),
        ServiceCommand::Resume => service::request_pause(home, false)
            .map(|()| EXIT_OK)
            .unwrap_or_else(|error| usage_error(&error)),
    }
}

/// Parse the short `--when` form. Users normally write a one-line mapping
/// such as `kind: daily, at: "02:30"`; YAML only treats that as a flow
/// mapping with braces, so retry with them before reporting an error.
fn parse_when_argument(text: &str) -> Result<ScheduleWhen, String> {
    if let Ok(when) = serde_yaml_ng::from_str::<ScheduleWhen>(text) {
        return Ok(when);
    }
    serde_yaml_ng::from_str::<ScheduleWhen>(&format!("{{{text}}}"))
        .map_err(|error| format!("invalid --when: {error}"))
}

fn add_schedule(args: &AddScheduleArgs, home: &Home, config_path: &Path) -> i32 {
    let schedule = if let Some(yaml) = args.yaml.as_deref() {
        match serde_yaml_ng::from_str::<Schedule>(yaml) {
            Ok(mut schedule) => {
                if schedule.name.is_empty() {
                    schedule.name = args.name.clone();
                }
                if schedule.name != args.name {
                    return usage_error("the schedule YAML name does not match the command name");
                }
                schedule
            }
            Err(error) => return usage_error(&format!("invalid schedule YAML: {error}")),
        }
    } else if let Some(path) = &args.file {
        match std::fs::read_to_string(path) {
            Ok(source) => match serde_yaml_ng::from_str::<Schedule>(&source) {
                Ok(mut schedule) => {
                    if schedule.name.is_empty() {
                        schedule.name = args.name.clone();
                    }
                    if schedule.name != args.name {
                        return usage_error(
                            "the schedule file name does not match the command name",
                        );
                    }
                    schedule
                }
                Err(error) => {
                    return usage_error(&format!("{}: {error}", path.display()));
                }
            },
            Err(error) => return usage_error(&format!("cannot read {}: {error}", path.display())),
        }
    } else {
        let Some(when) = args.when.as_deref() else {
            return usage_error("provide --when, --yaml or --file");
        };
        let when = match parse_when_argument(when) {
            Ok(when) => when,
            Err(error) => return usage_error(&error),
        };
        Schedule {
            name: args.name.clone(),
            when,
            run: RunSelection {
                plays: args.plays.clone(),
                tags: args.tag.clone(),
                exclude_tags: args.exclude_tag.clone(),
                actors: args.actor.clone(),
                app: args.app.clone(),
                retries: args.retries,
                profile: args.profile.clone(),
            },
            on_missed: args.on_missed.into(),
            on_busy: args.on_busy.into(),
            notify: NotifySpec::default(),
            enabled: !args.disabled,
        }
    };
    let sections = ResidentSections {
        schedules: vec![schedule.clone()],
        watchers: Vec::new(),
        notifications: impresario_core::resident::Notifications::default(),
    };
    if let Err(errors) = sections.validate() {
        return usage_error(&errors.join("; "));
    }
    let layer = if args.global {
        Layer::Global
    } else {
        Layer::Workspace
    };
    let path = config::layer_path(layer, home, config_path);
    match config::read_layer(&path) {
        Ok(existing) => {
            if existing
                .schedules
                .iter()
                .any(|value| value.name == schedule.name)
            {
                return usage_error(&format!("schedule '{}' already exists", schedule.name));
            }
        }
        Err(error) => return usage_error(&error),
    }
    let yaml = match config::to_yaml(&schedule) {
        Ok(yaml) => yaml,
        Err(error) => return usage_error(&error),
    };
    match config::append(&path, Kind::Schedule, &yaml) {
        Ok(()) => {
            println!("added schedule '{}' to {}", schedule.name, path.display());
            EXIT_OK
        }
        Err(error) => usage_error(&error),
    }
}

fn edit_schedule(args: &EditScheduleArgs, home: &Home, config_path: &Path) -> i32 {
    if let Some(yaml) = args.yaml.as_deref() {
        return edit_yaml(
            args.name.as_deref(),
            yaml,
            args.global,
            home,
            config_path,
            Kind::Schedule,
        );
    }
    if let Some(path) = &args.file {
        return match std::fs::read_to_string(path) {
            Ok(source) => edit_yaml(
                args.name.as_deref(),
                &source,
                args.global,
                home,
                config_path,
                Kind::Schedule,
            ),
            Err(error) => usage_error(&format!("cannot read {}: {error}", path.display())),
        };
    }
    let layer = if args.global {
        Layer::Global
    } else {
        Layer::Workspace
    };
    let path = config::layer_path(layer, home, config_path);
    match open_editor(&path) {
        Ok(()) => match config::read_layer(&path) {
            Ok(sections) => {
                if let Some(name) = &args.name {
                    if !sections.schedules.iter().any(|value| value.name == *name) {
                        return usage_error(&format!(
                            "schedule '{name}' was not found after editing"
                        ));
                    }
                }
                EXIT_OK
            }
            Err(error) => usage_error(&error),
        },
        Err(error) => usage_error(&error),
    }
}

fn edit_watch(args: &EditWatchArgs, home: &Home, config_path: &Path) -> i32 {
    if let Some(yaml) = args.yaml.as_deref() {
        return edit_yaml(
            args.name.as_deref(),
            yaml,
            args.global,
            home,
            config_path,
            Kind::Watcher,
        );
    }
    if let Some(path) = &args.file {
        return match std::fs::read_to_string(path) {
            Ok(source) => edit_yaml(
                args.name.as_deref(),
                &source,
                args.global,
                home,
                config_path,
                Kind::Watcher,
            ),
            Err(error) => usage_error(&format!("cannot read {}: {error}", path.display())),
        };
    }
    let layer = if args.global {
        Layer::Global
    } else {
        Layer::Workspace
    };
    let path = config::layer_path(layer, home, config_path);
    match open_editor(&path) {
        Ok(()) => match config::read_layer(&path) {
            Ok(sections) => {
                if let Some(name) = &args.name {
                    if !sections.watchers.iter().any(|value| value.name == *name) {
                        return usage_error(&format!(
                            "watcher '{name}' was not found after editing"
                        ));
                    }
                }
                EXIT_OK
            }
            Err(error) => usage_error(&error),
        },
        Err(error) => usage_error(&error),
    }
}

fn edit_yaml(
    name: Option<&str>,
    yaml: &str,
    global: bool,
    home: &Home,
    config_path: &Path,
    kind: Kind,
) -> i32 {
    let layer = if global {
        Layer::Global
    } else {
        Layer::Workspace
    };
    let path = config::layer_path(layer, home, config_path);
    let yaml = name.map_or_else(
        || yaml.to_string(),
        |name| {
            if let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml) {
                let mut map = value.as_mapping().cloned().unwrap_or_default();
                if map
                    .get(serde_yaml_ng::Value::String("name".to_string()))
                    .is_none()
                {
                    map.insert(
                        serde_yaml_ng::Value::String("name".to_string()),
                        serde_yaml_ng::Value::String(name.to_string()),
                    );
                }
                serde_yaml_ng::to_string(&map).unwrap_or_else(|_| yaml.to_string())
            } else {
                yaml.to_string()
            }
        },
    );
    let name = match serde_yaml_ng::from_str::<Schedule>(&yaml) {
        Ok(value) if matches!(kind, Kind::Schedule) => value.name,
        _ => match serde_yaml_ng::from_str::<Watcher>(&yaml) {
            Ok(value) if matches!(kind, Kind::Watcher) => value.name,
            _ => return usage_error("the YAML is not a valid schedule or watcher"),
        },
    };
    match config::replace(&path, kind, &name, &yaml) {
        Ok(()) => {
            println!(
                "updated {} '{}' in {}",
                kind_name(kind),
                name,
                path.display()
            );
            EXIT_OK
        }
        Err(error) => usage_error(&error),
    }
}

fn add_watch(args: &AddWatchArgs, home: &Home, config_path: &Path) -> i32 {
    let watcher = if let Some(yaml) = args.yaml.as_deref() {
        match serde_yaml_ng::from_str::<Watcher>(yaml) {
            Ok(mut watcher) => {
                if watcher.name.is_empty() {
                    watcher.name = args.name.clone();
                }
                if watcher.name != args.name {
                    return usage_error("the watcher YAML name does not match the command name");
                }
                watcher
            }
            Err(error) => return usage_error(&format!("invalid watcher YAML: {error}")),
        }
    } else if let Some(path) = &args.file {
        match std::fs::read_to_string(path) {
            Ok(source) => match serde_yaml_ng::from_str::<Watcher>(&source) {
                Ok(mut watcher) => {
                    if watcher.name.is_empty() {
                        watcher.name = args.name.clone();
                    }
                    if watcher.name != args.name {
                        return usage_error(
                            "the watcher file name does not match the command name",
                        );
                    }
                    watcher
                }
                Err(error) => {
                    return usage_error(&format!("{}: {error}", path.display()));
                }
            },
            Err(error) => return usage_error(&format!("cannot read {}: {error}", path.display())),
        }
    } else {
        let pacing = match &args.gap {
            Some(gap) => Pacing::MinGap { gap: gap.clone() },
            None => Pacing::BackToBack,
        };
        Watcher {
            name: args.name.clone(),
            run: RunSelection {
                plays: args.plays.clone(),
                tags: args.tag.clone(),
                exclude_tags: args.exclude_tag.clone(),
                actors: args.actor.clone(),
                app: args.app.clone(),
                retries: args.retries,
                profile: args.profile.clone(),
            },
            pacing,
            lease: args.lease.into(),
            priority: impresario_core::resident::WatcherPriority::Normal,
            on_failure: impresario_core::resident::FailurePolicy::default(),
            device_health: impresario_core::resident::DeviceHealth::default(),
            retention: impresario_core::resident::Retention::default(),
            enabled: !args.disabled,
        }
    };
    let sections = ResidentSections {
        schedules: Vec::new(),
        watchers: vec![watcher.clone()],
        notifications: impresario_core::resident::Notifications::default(),
    };
    if let Err(errors) = sections.validate() {
        return usage_error(&errors.join("; "));
    }
    let layer = if args.global {
        Layer::Global
    } else {
        Layer::Workspace
    };
    let path = config::layer_path(layer, home, config_path);
    match config::read_layer(&path) {
        Ok(existing) => {
            if existing
                .watchers
                .iter()
                .any(|value| value.name == watcher.name)
            {
                return usage_error(&format!("watcher '{}' already exists", watcher.name));
            }
        }
        Err(error) => return usage_error(&error),
    }
    let yaml = match config::to_yaml(&watcher) {
        Ok(yaml) => yaml,
        Err(error) => return usage_error(&error),
    };
    match config::append(&path, Kind::Watcher, &yaml) {
        Ok(()) => {
            println!("added watcher '{}' to {}", watcher.name, path.display());
            EXIT_OK
        }
        Err(error) => usage_error(&error),
    }
}

fn mutate<F>(
    home: &Home,
    config_path: &Path,
    global: bool,
    kind: Kind,
    name: &str,
    action: F,
) -> i32
where
    F: FnOnce(Kind, &Path) -> Result<(), String>,
{
    let layer = if global {
        Layer::Global
    } else {
        Layer::Workspace
    };
    let mut path = config::layer_path(layer, home, config_path);
    if !global {
        let workspace = config::read_layer(&path);
        let present = match (&kind, workspace) {
            (Kind::Schedule, Ok(sections)) => {
                sections.schedules.iter().any(|value| value.name == name)
            }
            (Kind::Watcher, Ok(sections)) => {
                sections.watchers.iter().any(|value| value.name == name)
            }
            _ => false,
        };
        if !present {
            let global_path = config::layer_path(Layer::Global, home, config_path);
            let present = match (&kind, config::read_layer(&global_path)) {
                (Kind::Schedule, Ok(sections)) => {
                    sections.schedules.iter().any(|value| value.name == name)
                }
                (Kind::Watcher, Ok(sections)) => {
                    sections.watchers.iter().any(|value| value.name == name)
                }
                _ => false,
            };
            if present {
                path = global_path;
            }
        }
    }
    match action(kind, &path) {
        Ok(()) => EXIT_OK,
        Err(error) => usage_error(&error),
    }
}

fn list_schedules(sections: &ResidentSections) {
    if sections.schedules.is_empty() {
        println!("no schedules");
        return;
    }
    let now = chrono::Local::now();
    for schedule in &sections.schedules {
        let next = schedule.next_after(&now).ok().flatten();
        println!(
            "{:<24} {:<8} next {}  {}",
            schedule.name,
            if schedule.enabled {
                "enabled"
            } else {
                "disabled"
            },
            next.map(|value| value.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "-".to_string()),
            when_label(&schedule.when)
        );
    }
}

fn list_watchers(sections: &ResidentSections) {
    if sections.watchers.is_empty() {
        println!("no watchers");
        return;
    }
    for watcher in &sections.watchers {
        println!(
            "{:<24} {:<8} lease={:<6} {}  {}",
            watcher.name,
            if watcher.enabled {
                "enabled"
            } else {
                "disabled"
            },
            watcher.lease.as_str(),
            pacing_label(&watcher.pacing),
            plays_label(&watcher.run)
        );
    }
}

fn next_schedules(home: &Home, config_path: &Path, name: Option<&str>, count: usize) -> i32 {
    let sections = match config::load_effective(home, config_path) {
        Ok(sections) => sections,
        Err(error) => return usage_error(&error),
    };
    let now = chrono::Local::now();
    let selected: Vec<&Schedule> = match name {
        Some(name) => match sections.schedules.iter().find(|value| value.name == name) {
            Some(schedule) => vec![schedule],
            None => return usage_error(&format!("no schedule named '{name}'")),
        },
        None => sections.schedules.iter().collect(),
    };
    for schedule in selected {
        println!("{}:", schedule.name);
        match impresario_core::resident::next_fires(&schedule.when, &now, count) {
            Ok(fires) if fires.is_empty() => println!("  (no future fire)"),
            Ok(fires) => {
                for fire in fires {
                    println!("  {}", fire.format("%Y-%m-%d %H:%M:%S %:z"));
                }
            }
            Err(error) => println!("  (invalid: {error})"),
        }
    }
    EXIT_OK
}

fn run_now_schedule(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    name: &str,
) -> i32 {
    let sections = match config::load_effective(home, config_path) {
        Ok(sections) => sections,
        Err(error) => return usage_error(&error),
    };
    let Some(schedule) = sections.schedules.iter().find(|value| value.name == name) else {
        return usage_error(&format!("no schedule named '{name}'"));
    };
    run_selection(
        home,
        base,
        config_path,
        devices_path,
        &schedule.run,
        &format!("schedule {name}"),
    )
}

fn run_now_watch(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    name: &str,
) -> i32 {
    let sections = match config::load_effective(home, config_path) {
        Ok(sections) => sections,
        Err(error) => return usage_error(&error),
    };
    let Some(watcher) = sections.watchers.iter().find(|value| value.name == name) else {
        return usage_error(&format!("no watcher named '{name}'"));
    };
    run_selection(
        home,
        base,
        config_path,
        devices_path,
        &watcher.run,
        &format!("watcher {name}"),
    )
}

fn run_selection(
    home: &Home,
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    selection: &RunSelection,
    source: &str,
) -> i32 {
    let mut args = super::service::run_args_for_cli(selection);
    args.job_label = Some(source.to_string());
    if let Some(profile) = args.profile.clone() {
        if let Err(error) = crate::profile_cmd::apply(home, &profile, &mut args) {
            return usage_error(&error);
        }
    }
    let outcome = crate::cli::run_workspace(base, config_path, devices_path, &args, false, true);
    if outcome.exit_code == EXIT_OK {
        EXIT_OK
    } else if outcome.exit_code == EXIT_FAILURE {
        EXIT_FAILURE
    } else {
        outcome.exit_code
    }
}

fn when_label(when: &ScheduleWhen) -> String {
    match when {
        ScheduleWhen::Once { at } => format!("once {at}"),
        ScheduleWhen::Daily { at } => format!("daily {at}"),
        ScheduleWhen::Weekly { days, at } => format!("weekly {} {at}", days.join(",")),
        ScheduleWhen::Interval { every } => format!("every {every}"),
        ScheduleWhen::Cron { expr } => format!("cron {expr}"),
    }
}

fn watcher_lease(sections: Option<&ResidentSections>, name: &str) -> &'static str {
    sections
        .and_then(|sections| {
            sections
                .watchers
                .iter()
                .find(|watcher| watcher.name == name)
        })
        .map_or("-", |watcher| watcher.lease.as_str())
}

fn pacing_label(pacing: &Pacing) -> String {
    match pacing {
        Pacing::BackToBack => "back-to-back".to_string(),
        Pacing::MinGap { gap } => format!("gap {gap}"),
    }
}

fn plays_label(run: &RunSelection) -> String {
    if run.plays.is_empty() {
        "all plays".to_string()
    } else {
        run.plays.join(",")
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Schedule => "schedule",
        Kind::Watcher => "watcher",
    }
}

fn print_json<T: serde::Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("json: {error}"),
    }
}

fn service_status(home: &Home, config_path: &Path, json: bool) -> i32 {
    let state = match service::read_state(home) {
        Ok(state) => state,
        Err(error) => return usage_error(&error),
    };
    let sections = config::load_effective(home, config_path).ok();
    let running = state.pid != 0 && crate::jobs::pid_alive(state.pid);
    if json {
        let watchers: Vec<serde_json::Value> = state
            .watchers
            .iter()
            .map(|(name, runtime)| {
                serde_json::json!({
                    "name": name,
                    "lease": watcher_lease(sections.as_ref(), name),
                    "iterations": runtime.iterations,
                    "failures": runtime.failures,
                    "last_status": runtime.last_status,
                })
            })
            .collect();
        let value = serde_json::json!({
            "running": running,
            "state": state,
            "watchers": watchers,
        });
        print_json(&value);
        return EXIT_OK;
    }
    println!("resident: {}", if running { "running" } else { "stopped" });
    if state.pid != 0 {
        println!("pid: {}", state.pid);
    }
    if !state.started_at.is_empty() {
        println!("started: {}", service::display_local(&state.started_at));
    }
    println!("workspace: {}", state.workspace);
    println!("paused: {}", state.paused);
    if !state.schedules.is_empty() {
        println!("schedules:");
        for (name, runtime) in &state.schedules {
            println!(
                "  {name:<24} runs={} failures={} last={}",
                runtime.runs,
                runtime.failures,
                runtime
                    .last_fire
                    .as_deref()
                    .map(service::display_local)
                    .unwrap_or_else(|| "-".to_string())
            );
        }
    }
    if !state.watchers.is_empty() {
        println!("watchers:");
        for (name, runtime) in &state.watchers {
            println!(
                "  {name:<24} lease={} iterations={} failures={} status={}",
                watcher_lease(sections.as_ref(), name),
                runtime.iterations,
                runtime.failures,
                runtime.last_status.as_deref().unwrap_or("-")
            );
        }
    }
    EXIT_OK
}

fn stop_service(home: &Home) -> i32 {
    if !service::is_running(home) {
        println!("resident runner is not running");
        return EXIT_OK;
    }
    if let Err(error) = service::request_stop(home) {
        eprintln!("service: {error}");
        return EXIT_INFRA;
    }
    for _ in 0..100 {
        if !service::is_running(home) {
            println!("resident runner stopped");
            return EXIT_OK;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("service: stop requested but the runner is still active");
    EXIT_INFRA
}

fn watch_status(home: &Home, config_path: &Path, name: Option<&str>, json: bool) -> i32 {
    let state = match service::read_state(home) {
        Ok(state) => state,
        Err(error) => return usage_error(&error),
    };
    let sections = match config::load_effective(home, config_path) {
        Ok(sections) => sections,
        Err(error) => return usage_error(&error),
    };
    let watches: Vec<serde_json::Value> = sections
        .watchers
        .iter()
        .filter(|watcher| name.is_none_or(|name| watcher.name == name))
        .map(|watcher| {
            let runtime = state
                .watchers
                .get(&watcher.name)
                .cloned()
                .unwrap_or_default();
            serde_json::json!({
                "name": watcher.name,
                "configured_enabled": watcher.enabled,
                "lease": watcher.lease.as_str(),
                "iterations": runtime.iterations,
                "failures": runtime.failures,
                "last_status": runtime.last_status,
                "last_dump": runtime.last_dump,
                "paused": super::service::watch_paused(home, &watcher.name),
            })
        })
        .collect();
    if json {
        print_json(&serde_json::json!({"running": service::is_running(home), "watchers": watches}));
    } else {
        for value in watches {
            println!(
                "{} enabled={} lease={} iterations={} failures={} status={} paused={}",
                value
                    .get("name")
                    .and_then(|item| item.as_str())
                    .unwrap_or("?"),
                value
                    .get("configured_enabled")
                    .map_or_else(|| "?".to_string(), ToString::to_string),
                value
                    .get("lease")
                    .and_then(|item| item.as_str())
                    .unwrap_or("-"),
                value
                    .get("iterations")
                    .map_or_else(|| "?".to_string(), ToString::to_string),
                value
                    .get("failures")
                    .map_or_else(|| "?".to_string(), ToString::to_string),
                value
                    .get("last_status")
                    .and_then(|item| item.as_str())
                    .unwrap_or("-"),
                value
                    .get("paused")
                    .map_or_else(|| "?".to_string(), ToString::to_string),
            );
        }
    }
    EXIT_OK
}

fn watch_dumps(base: &Path, name: Option<&str>) -> i32 {
    let root = base.join("reports").join("watch");
    let names: Vec<String> = match name {
        Some(name) => vec![name.to_string()],
        None => match std::fs::read_dir(&root) {
            Ok(read) => read
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(error) => {
                eprintln!("watch: cannot read {}: {error}", root.display());
                return EXIT_FAILURE;
            }
        },
    };
    for name in names {
        let dir = root.join(&name);
        if !dir.is_dir() {
            continue;
        }
        println!("{name}:");
        let mut entries: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(read) => read.flatten().collect(),
            Err(error) => {
                eprintln!("watch: cannot read {}: {error}", dir.display());
                continue;
            }
        };
        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().into_owned());
        for entry in entries {
            if entry.path().is_dir() {
                println!("  {}", entry.path().display());
            }
        }
    }
    EXIT_OK
}

fn open_editor(path: &Path) -> Result<(), String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .map_err(|_| "set VISUAL or EDITOR to edit the configuration".to_string())?;
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        return Err("set VISUAL or EDITOR to edit the configuration".to_string());
    };
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("editor exited with {status}"))
    }
}

fn usage_error(message: &str) -> i32 {
    eprintln!("{message}");
    EXIT_USAGE
}
