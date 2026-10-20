//! Device and app definition commands (TASK-2 sections 4.3 and 5) plus the
//! one-shot legacy migration (section 8.9, config part).
//!
//! Pure appends are written as text so comments in the existing file survive;
//! other edits round-trip the typed list. Every candidate file is parsed again
//! before it is written, so an invalid edit never reaches disk.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use impresario_core::apps::{self, AppConfig, AppOptions, AppPlatform, AppsConfig};
use impresario_core::config::{
    self, CdpBackend, DeviceAuth, DeviceConfig, DeviceConnection, DeviceOptions, DevicesConfig,
    KeyDispatch, TransportKind,
};
use impresario_core::diag::Diagnostics;
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use serde_yaml_ng::Value;

use crate::cli::{EXIT_OK, EXIT_USAGE};

/// Definitions live in the global layer by default (`~/.impresario/{devices,apps}.yaml`);
/// `--workspace` scopes the edit to the workspace file (TASK-2 4.3).
#[derive(Debug, Args)]
pub struct DefinitionScope {
    /// Edit the workspace file instead of the global file.
    #[arg(long)]
    pub workspace: bool,
}

#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    /// Add a device (non-interactive; the wizard is a separate screen).
    Add(DeviceAddArgs),
    /// Replace an existing device definition.
    Edit(DeviceAddArgs),
    /// Remove a device from one layer.
    Remove {
        name: String,
        #[command(flatten)]
        scope: DefinitionScope,
    },
    /// Rename a device in one layer.
    Rename {
        name: String,
        new_name: String,
        #[command(flatten)]
        scope: DefinitionScope,
    },
    /// List the effective devices (workspace overrides global).
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Show one effective device as YAML.
    Show { name: String },
    /// Transport prepare + handshake + DeviceInfo, then teardown.
    Check {
        /// Device names (default: all).
        names: Vec<String>,
    },
    /// Import devices from a YAML file into one layer.
    Import {
        file: PathBuf,
        #[command(flatten)]
        scope: DefinitionScope,
        /// Replace devices with the same name.
        #[arg(long)]
        force: bool,
    },
    /// Print one layer (or the merged view) as YAML.
    Export {
        #[command(flatten)]
        scope: DefinitionScope,
        /// Export the merged effective view instead of one layer.
        #[arg(long, conflicts_with = "workspace")]
        merged: bool,
    },
}

#[derive(Debug, Args)]
pub struct DeviceAddArgs {
    /// Device name (^[a-z0-9][a-z0-9_-]{0,62}$).
    #[arg(long)]
    pub name: String,
    /// web, webos, tizen, android, androidtv, ios or tvos.
    #[arg(long)]
    pub platform: String,
    /// auto, usb, tcp, network, cdp, launch, ssh or sdb.
    #[arg(long, default_value = "auto")]
    pub transport: String,
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub serial: Option<String>,
    #[arg(long)]
    pub udid: Option<String>,
    /// Device endpoint (CDP URL or agent host:port override).
    #[arg(long)]
    pub endpoint: Option<String>,
    /// Label (repeatable).
    #[arg(long = "label")]
    pub labels: Vec<String>,
    /// Credential reference (for example `keychain:android`).
    #[arg(long)]
    pub key: Option<String>,
    /// Signing identity reference (Apple).
    #[arg(long)]
    pub identity: Option<String>,
    /// Certificate profile reference (Tizen/Apple).
    #[arg(long)]
    pub profile: Option<String>,
    /// Device agent port override.
    #[arg(long)]
    pub agent_port: Option<u16>,
    /// Connector listen address for web runtimes (host:port).
    #[arg(long)]
    pub connector_address: Option<String>,
    /// Web CDP backend: raw or chromiumoxide.
    #[arg(long, default_value = "raw", value_name = "BACKEND")]
    pub cdp_backend: String,
    /// Web key dispatch: cdp or js.
    #[arg(long, default_value = "cdp", value_name = "DISPATCH")]
    pub key_dispatch: String,
    /// Web page title/URL substring used to pick the CDP page.
    #[arg(long)]
    pub page_match: Option<String>,
    /// Launch Apple apps with reduced-motion arguments.
    #[arg(long)]
    pub animations_disabled: bool,
    #[command(flatten)]
    pub scope: DefinitionScope,
}

#[derive(Debug, Subcommand)]
pub enum AppCommand {
    /// Add an app definition.
    Add(AppAddArgs),
    /// Replace an existing app definition.
    Edit(AppAddArgs),
    /// Remove an app from one layer.
    Remove {
        name: String,
        #[command(flatten)]
        scope: DefinitionScope,
    },
    /// Rename an app in one layer.
    Rename {
        name: String,
        new_name: String,
        #[command(flatten)]
        scope: DefinitionScope,
    },
    /// List effective apps.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one effective app as YAML.
    Show { name: String },
    /// Validate app references and package paths.
    Check,
    /// Print one layer (or the merged view) as YAML.
    Export {
        #[command(flatten)]
        scope: DefinitionScope,
        #[arg(long, conflicts_with = "workspace")]
        merged: bool,
    },
}

#[derive(Debug, Args)]
pub struct AppAddArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    /// `PLATFORM=ID` for native/TV platforms or `web=URL` (repeatable).
    #[arg(long = "platform", value_name = "PLATFORM=ID|URL")]
    pub platform: Vec<String>,
    /// `PLATFORM=PATH` artifact to install/re-sign (repeatable).
    #[arg(long = "package", value_name = "PLATFORM=PATH")]
    pub package: Vec<String>,
    /// Wipe the app's data before a reset.
    #[arg(long)]
    pub clear_data: bool,
    /// Android launch activity, as `PLATFORM=ACTIVITY` (repeatable).
    #[arg(long = "launch-activity", value_name = "PLATFORM=ACTIVITY")]
    pub launch_activity: Vec<String>,
    /// Platform-specific launch parameter, as `KEY=VALUE` (repeatable).
    #[arg(long = "launch-param", value_name = "KEY=VALUE")]
    pub launch_params: Vec<String>,
    #[command(flatten)]
    pub scope: DefinitionScope,
}

#[derive(Debug, Args)]
pub struct MigrateArgs {
    /// Print the converted files without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Device commands
// ---------------------------------------------------------------------------

pub fn run_device(command: &DeviceCommand, home: &Home, workspace_path: &Path) -> i32 {
    match command {
        DeviceCommand::Add(args) => add_device(args, home, workspace_path, false),
        DeviceCommand::Edit(args) => add_device(args, home, workspace_path, true),
        DeviceCommand::Remove { name, scope } => {
            remove_device(name, home, workspace_path, !scope.workspace)
        }
        DeviceCommand::Rename {
            name,
            new_name,
            scope,
        } => rename_device(name, new_name, home, workspace_path, !scope.workspace),
        DeviceCommand::List { json } => list_devices(home, workspace_path, *json),
        DeviceCommand::Show { name } => show_device(name, home, workspace_path),
        DeviceCommand::Check { names } => {
            // The actual connectivity check lives in cli.rs (it needs the
            // runtime); this arm is replaced there.
            let _ = (names, home, workspace_path);
            EXIT_USAGE
        }
        DeviceCommand::Import { file, scope, force } => {
            import_devices(file, home, workspace_path, !scope.workspace, *force)
        }
        DeviceCommand::Export { scope, merged } => {
            export_devices(home, workspace_path, !scope.workspace, *merged)
        }
    }
}

fn add_device(args: &DeviceAddArgs, home: &Home, workspace_path: &Path, replace: bool) -> i32 {
    let platform = match args.platform.parse::<Platform>() {
        Ok(platform) => platform,
        Err(()) => {
            eprintln!(
                "device add: unknown platform '{}'; expected web, webos, tizen, android, androidtv, ios or tvos",
                args.platform
            );
            return EXIT_USAGE;
        }
    };
    let transport = match parse_transport(&args.transport) {
        Some(transport) => transport,
        None => {
            eprintln!(
                "device add: unknown transport '{}'; expected auto, usb, tcp, network, cdp, launch, ssh or sdb",
                args.transport
            );
            return EXIT_USAGE;
        }
    };
    let cdp_backend = match args.cdp_backend.as_str() {
        "raw" => CdpBackend::Raw,
        "chromiumoxide" => CdpBackend::Chromiumoxide,
        other => {
            eprintln!("device add: unknown cdp backend '{other}'; expected raw or chromiumoxide");
            return EXIT_USAGE;
        }
    };
    let key_dispatch = match args.key_dispatch.as_str() {
        "cdp" => KeyDispatch::Cdp,
        "js" => KeyDispatch::Js,
        other => {
            eprintln!("device add: unknown key dispatch '{other}'; expected cdp or js");
            return EXIT_USAGE;
        }
    };
    let device = DeviceConfig {
        name: args.name.clone(),
        platform,
        connection: DeviceConnection {
            transport,
            host: args.host.clone(),
            port: args.port,
            serial: args.serial.clone(),
            udid: args.udid.clone(),
            endpoint: args.endpoint.clone(),
        },
        auth: DeviceAuth {
            key: args.key.clone(),
            identity: args.identity.clone(),
            profile: args.profile.clone(),
        },
        labels: args.labels.clone(),
        options: DeviceOptions {
            cdp_backend,
            key_dispatch,
            page_match: args.page_match.clone(),
            connector_address: args.connector_address.clone(),
            agent_port: args.agent_port,
            animations_disabled: args.animations_disabled,
        },
    };
    let path = layer_path(home, workspace_path, !args.scope.workspace, true);
    match mutate_devices(&path, |source| {
        let outcome = config::parse_devices(source, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        if let Some(index) = parsed
            .devices
            .iter()
            .position(|item| item.name == device.name)
        {
            if !replace {
                return Err(format!(
                    "device '{}' already exists; use `device edit` to replace it",
                    device.name
                ));
            }
            let Some(slot) = parsed.devices.get_mut(index) else {
                return Err(format!(
                    "device '{}' disappeared during the edit",
                    device.name
                ));
            };
            *slot = device.clone();
        } else if replace {
            return Err(format!("device '{}' does not exist", device.name));
        } else {
            parsed.devices.push(device.clone());
        }
        Ok(parsed)
    }) {
        Ok(()) => {
            println!(
                "{} device '{}' in {}",
                if replace { "updated" } else { "added" },
                device.name,
                path.display()
            );
            EXIT_OK
        }
        Err(message) => {
            eprintln!("device add: {message}");
            EXIT_USAGE
        }
    }
}

fn remove_device(name: &str, home: &Home, workspace_path: &Path, global: bool) -> i32 {
    let path = layer_path(home, workspace_path, global, true);
    match mutate_devices(&path, |source| {
        let outcome = config::parse_devices(source, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        let before = parsed.devices.len();
        parsed.devices.retain(|item| item.name != name);
        if parsed.devices.len() == before {
            return Err(format!(
                "device '{name}' does not exist in {}",
                path.display()
            ));
        }
        Ok(parsed)
    }) {
        Ok(()) => {
            println!("removed device '{name}' from {}", path.display());
            EXIT_OK
        }
        Err(message) => {
            eprintln!("device remove: {message}");
            EXIT_USAGE
        }
    }
}

fn rename_device(
    name: &str,
    new_name: &str,
    home: &Home,
    workspace_path: &Path,
    global: bool,
) -> i32 {
    if !config::valid_device_name(new_name) {
        eprintln!("device rename: '{new_name}' is not a valid device name");
        return EXIT_USAGE;
    }
    let path = layer_path(home, workspace_path, global, true);
    match mutate_devices(&path, |source| {
        let outcome = config::parse_devices(source, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        if parsed.devices.iter().any(|item| item.name == new_name) {
            return Err(format!("device '{new_name}' already exists"));
        }
        let Some(device) = parsed.devices.iter_mut().find(|item| item.name == name) else {
            return Err(format!(
                "device '{name}' does not exist in {}",
                path.display()
            ));
        };
        device.name = new_name.to_string();
        Ok(parsed)
    }) {
        Ok(()) => {
            println!(
                "renamed device '{name}' to '{new_name}' in {}",
                path.display()
            );
            EXIT_OK
        }
        Err(message) => {
            eprintln!("device rename: {message}");
            EXIT_USAGE
        }
    }
}

fn list_devices(home: &Home, workspace_path: &Path, json: bool) -> i32 {
    let outcome = config::load_devices_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        eprintln!("device list: configuration errors");
        return EXIT_USAGE;
    };
    let overrides = overridden_devices(home, workspace_path);
    if json {
        match serde_json::to_string_pretty(&devices.devices) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("device list: cannot serialize: {err}");
                return EXIT_USAGE;
            }
        }
        return EXIT_OK;
    }
    if devices.devices.is_empty() {
        println!("no devices configured (add one with `impresario device add`)");
        return EXIT_OK;
    }
    let global_names = device_names(&home.devices_path());
    let workspace_names = device_names(workspace_path);
    println!(
        "{:<20} {:<10} {:<8} {:<10} CONNECTION",
        "NAME", "PLATFORM", "TRANSPORT", "ORIGIN"
    );
    for device in &devices.devices {
        let origin = if overrides.contains(&device.name) {
            "workspace*"
        } else if workspace_names.contains(&device.name) {
            "workspace"
        } else if global_names.contains(&device.name) {
            "global"
        } else {
            "effective"
        };
        println!(
            "{:<20} {:<10} {:<8} {:<8} {}",
            device.name,
            device.platform.as_str(),
            device.effective_transport().as_str(),
            origin,
            device.connection_summary()
        );
    }
    if !overrides.is_empty() {
        println!("* overrides the global device of the same name");
    }
    EXIT_OK
}

fn device_names(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|source| config::parse_devices(&source, path).config)
        .map(|config| {
            config
                .devices
                .into_iter()
                .map(|device| device.name)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default()
}

fn overridden_devices(home: &Home, workspace_path: &Path) -> HashSet<String> {
    let global = device_names(&home.devices_path());
    let workspace = device_names(workspace_path);
    global.intersection(&workspace).cloned().collect()
}

fn show_device(name: &str, _home: &Home, workspace_path: &Path) -> i32 {
    let outcome = config::load_devices_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        eprintln!("device show: configuration errors");
        return EXIT_USAGE;
    };
    let Some(device) = devices.devices.iter().find(|item| item.name == name) else {
        eprintln!("device show: no device named '{name}'");
        return EXIT_USAGE;
    };
    match serde_yaml_ng::to_string(&DevicesConfig {
        devices: vec![device.clone()],
    }) {
        Ok(text) => {
            print!("{text}");
            EXIT_OK
        }
        Err(err) => {
            eprintln!("device show: cannot serialize: {err}");
            EXIT_USAGE
        }
    }
}

fn import_devices(
    file: &Path,
    home: &Home,
    workspace_path: &Path,
    global: bool,
    force: bool,
) -> i32 {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("device import: cannot read {}: {err}", file.display());
            return EXIT_USAGE;
        }
    };
    let imported = match config::parse_devices(&source, file).config {
        Some(config) => config,
        None => {
            eprintln!(
                "device import: {} is not a valid devices file",
                file.display()
            );
            return EXIT_USAGE;
        }
    };
    let path = layer_path(home, workspace_path, global, true);
    match mutate_devices(&path, |existing| {
        let outcome = config::parse_devices(existing, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        for device in &imported.devices {
            if let Some(index) = parsed
                .devices
                .iter()
                .position(|item| item.name == device.name)
            {
                if !force {
                    return Err(format!(
                        "device '{}' already exists (use --force to replace it)",
                        device.name
                    ));
                }
                let Some(slot) = parsed.devices.get_mut(index) else {
                    return Err(format!(
                        "device '{}' disappeared during the edit",
                        device.name
                    ));
                };
                *slot = device.clone();
            } else {
                parsed.devices.push(device.clone());
            }
        }
        Ok(parsed)
    }) {
        Ok(()) => {
            println!(
                "imported {} device(s) into {}",
                imported.devices.len(),
                path.display()
            );
            EXIT_OK
        }
        Err(message) => {
            eprintln!("device import: {message}");
            EXIT_USAGE
        }
    }
}

fn export_devices(home: &Home, workspace_path: &Path, global: bool, merged: bool) -> i32 {
    if !merged {
        let path = layer_path(home, workspace_path, global, false);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                print!("{text}");
                return EXIT_OK;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("devices: []");
                return EXIT_OK;
            }
            Err(err) => {
                eprintln!("device export: cannot read {}: {err}", path.display());
                return EXIT_USAGE;
            }
        }
    }
    let outcome = config::load_devices_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        return EXIT_USAGE;
    };
    match serde_yaml_ng::to_string(&devices) {
        Ok(text) => {
            print!("{text}");
            EXIT_OK
        }
        Err(err) => {
            eprintln!("device export: cannot serialize: {err}");
            EXIT_USAGE
        }
    }
}

// ---------------------------------------------------------------------------
// App commands
// ---------------------------------------------------------------------------

pub fn run_app(command: &AppCommand, home: &Home, workspace_path: &Path) -> i32 {
    match command {
        AppCommand::Add(args) => add_app(args, home, workspace_path, false),
        AppCommand::Edit(args) => add_app(args, home, workspace_path, true),
        AppCommand::Remove { name, scope } => {
            remove_app(name, home, workspace_path, !scope.workspace)
        }
        AppCommand::Rename {
            name,
            new_name,
            scope,
        } => rename_app(name, new_name, home, workspace_path, !scope.workspace),
        AppCommand::List { json } => list_apps(home, workspace_path, *json),
        AppCommand::Show { name } => show_app(name, home, workspace_path),
        AppCommand::Check => check_apps(home, workspace_path),
        AppCommand::Export { scope, merged } => {
            export_apps(home, workspace_path, !scope.workspace, *merged)
        }
    }
}

fn add_app(args: &AppAddArgs, home: &Home, workspace_path: &Path, replace: bool) -> i32 {
    if args.platform.is_empty() {
        eprintln!("app add: at least one --platform PLATFORM=ID|URL is required");
        return EXIT_USAGE;
    }
    let mut platforms: BTreeMap<Platform, AppPlatform> = BTreeMap::new();
    for spec in &args.platform {
        let Some((platform_text, value)) = spec.split_once('=') else {
            eprintln!("app add: --platform must be PLATFORM=ID or web=URL (got '{spec}')");
            return EXIT_USAGE;
        };
        let platform = match platform_text.trim().parse::<Platform>() {
            Ok(platform) => platform,
            Err(()) => {
                eprintln!("app add: unknown platform '{platform_text}'");
                return EXIT_USAGE;
            }
        };
        let value = value.trim().to_string();
        let entry = if platform == Platform::Web {
            AppPlatform {
                url: Some(value),
                ..AppPlatform::default()
            }
        } else {
            AppPlatform {
                id: Some(value),
                ..AppPlatform::default()
            }
        };
        if platforms.insert(platform, entry).is_some() {
            eprintln!("app add: platform '{platform}' is listed more than once",);
            return EXIT_USAGE;
        }
    }
    for spec in &args.package {
        let Some((platform_text, path)) = spec.split_once('=') else {
            eprintln!("app add: --package must be PLATFORM=PATH (got '{spec}')");
            return EXIT_USAGE;
        };
        let platform = match platform_text.trim().parse::<Platform>() {
            Ok(platform) => platform,
            Err(()) => {
                eprintln!("app add: unknown platform '{platform_text}'");
                return EXIT_USAGE;
            }
        };
        let Some(entry) = platforms.get_mut(&platform) else {
            eprintln!(
                "app add: --package {} has no matching --platform {} entry",
                spec, platform_text
            );
            return EXIT_USAGE;
        };
        if platform == Platform::Web {
            eprintln!("app add: the web platform has no package; use its URL");
            return EXIT_USAGE;
        }
        entry.package = Some(path.to_string());
    }
    for spec in &args.launch_activity {
        let Some((platform_text, activity)) = spec.split_once('=') else {
            eprintln!("app add: --launch-activity must be PLATFORM=ACTIVITY (got '{spec}')");
            return EXIT_USAGE;
        };
        let platform = match platform_text.trim().parse::<Platform>() {
            Ok(platform) => platform,
            Err(()) => {
                eprintln!("app add: unknown platform '{platform_text}'");
                return EXIT_USAGE;
            }
        };
        let Some(entry) = platforms.get_mut(&platform) else {
            eprintln!(
                "app add: --launch-activity {} has no matching --platform {} entry",
                spec, platform_text
            );
            return EXIT_USAGE;
        };
        if activity.trim().is_empty() {
            eprintln!("app add: --launch-activity {} has an empty activity", spec);
            return EXIT_USAGE;
        }
        entry.launch_activity = Some(activity.trim().to_string());
    }
    let mut launch_params = BTreeMap::new();
    for spec in &args.launch_params {
        let Some((key, value)) = spec.split_once('=') else {
            eprintln!("app add: --launch-param must be KEY=VALUE (got '{spec}')");
            return EXIT_USAGE;
        };
        if key.trim().is_empty() || value.is_empty() {
            eprintln!("app add: --launch-param needs a non-empty key and value (got '{spec}')");
            return EXIT_USAGE;
        }
        if launch_params
            .insert(key.trim().to_string(), value.to_string())
            .is_some()
        {
            eprintln!(
                "app add: launch parameter '{}' is listed more than once",
                key.trim()
            );
            return EXIT_USAGE;
        }
    }
    let app = AppConfig {
        name: args.name.clone(),
        description: args.description.clone(),
        platforms,
        options: AppOptions {
            clear_data: args.clear_data,
            launch_params,
        },
    };
    let path = layer_path(home, workspace_path, !args.scope.workspace, false);
    match mutate_apps(&path, |source| {
        let outcome = apps::parse_apps(source, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        if let Some(index) = parsed.apps.iter().position(|item| item.name == app.name) {
            if !replace {
                return Err(format!(
                    "app '{}' already exists; use `app edit` to replace it",
                    app.name
                ));
            }
            let Some(slot) = parsed.apps.get_mut(index) else {
                return Err(format!("app '{}' disappeared during the edit", app.name));
            };
            *slot = app.clone();
        } else if replace {
            return Err(format!("app '{}' does not exist", app.name));
        } else {
            parsed.apps.push(app.clone());
        }
        Ok(parsed)
    }) {
        Ok(()) => {
            println!(
                "{} app '{}' in {}",
                if replace { "updated" } else { "added" },
                app.name,
                path.display()
            );
            EXIT_OK
        }
        Err(message) => {
            eprintln!("app add: {message}");
            EXIT_USAGE
        }
    }
}

fn remove_app(name: &str, home: &Home, workspace_path: &Path, global: bool) -> i32 {
    let path = layer_path(home, workspace_path, global, false);
    match mutate_apps(&path, |source| {
        let outcome = apps::parse_apps(source, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        let before = parsed.apps.len();
        parsed.apps.retain(|item| item.name != name);
        if parsed.apps.len() == before {
            return Err(format!("app '{name}' does not exist in {}", path.display()));
        }
        Ok(parsed)
    }) {
        Ok(()) => {
            println!("removed app '{name}' from {}", path.display());
            EXIT_OK
        }
        Err(message) => {
            eprintln!("app remove: {message}");
            EXIT_USAGE
        }
    }
}

fn rename_app(name: &str, new_name: &str, home: &Home, workspace_path: &Path, global: bool) -> i32 {
    if !config::valid_device_name(new_name) {
        eprintln!("app rename: '{new_name}' is not a valid app name");
        return EXIT_USAGE;
    }
    let path = layer_path(home, workspace_path, global, false);
    match mutate_apps(&path, |source| {
        let outcome = apps::parse_apps(source, &path);
        let Some(mut parsed) = outcome.config else {
            return Err(render_diagnostics(&outcome.diagnostics));
        };
        if parsed.apps.iter().any(|item| item.name == new_name) {
            return Err(format!("app '{new_name}' already exists"));
        }
        let Some(app) = parsed.apps.iter_mut().find(|item| item.name == name) else {
            return Err(format!("app '{name}' does not exist in {}", path.display()));
        };
        app.name = new_name.to_string();
        Ok(parsed)
    }) {
        Ok(()) => {
            println!("renamed app '{name}' to '{new_name}' in {}", path.display());
            EXIT_OK
        }
        Err(message) => {
            eprintln!("app rename: {message}");
            EXIT_USAGE
        }
    }
}

fn list_apps(home: &Home, workspace_path: &Path, json: bool) -> i32 {
    let outcome = apps::load_apps_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(app_list) = outcome.config else {
        eprintln!("app list: configuration errors");
        return EXIT_USAGE;
    };
    let overrides = overridden_apps(home, workspace_path);
    let workspace_names = app_names(workspace_path);
    if json {
        match serde_json::to_string_pretty(&app_list.apps) {
            Ok(text) => println!("{text}"),
            Err(err) => {
                eprintln!("app list: cannot serialize: {err}");
                return EXIT_USAGE;
            }
        }
        return EXIT_OK;
    }
    if app_list.apps.is_empty() {
        println!("no apps configured (add one with `impresario app add`)");
        return EXIT_OK;
    }
    for app in &app_list.apps {
        let platforms: Vec<&str> = app.platforms.keys().map(|p| p.as_str()).collect();
        let mark = if overrides.contains(&app.name) {
            " (overrides global)"
        } else if workspace_names.contains(&app.name) {
            " (workspace)"
        } else {
            ""
        };
        println!("{:<24} {}{}", app.name, platforms.join(", "), mark);
    }
    EXIT_OK
}

fn app_names(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|source| apps::parse_apps(&source, path).config)
        .map(|config| {
            config
                .apps
                .into_iter()
                .map(|app| app.name)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default()
}

fn overridden_apps(home: &Home, workspace_path: &Path) -> HashSet<String> {
    let global = app_names(&home.apps_path());
    let workspace = app_names(workspace_path);
    global.intersection(&workspace).cloned().collect()
}

fn show_app(name: &str, _home: &Home, workspace_path: &Path) -> i32 {
    let outcome = apps::load_apps_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(app_list) = outcome.config else {
        eprintln!("app show: configuration errors");
        return EXIT_USAGE;
    };
    let Some(app) = app_list.apps.iter().find(|item| item.name == name) else {
        eprintln!("app show: no app named '{name}'");
        return EXIT_USAGE;
    };
    match serde_yaml_ng::to_string(&AppsConfig {
        apps: vec![app.clone()],
    }) {
        Ok(text) => {
            print!("{text}");
            EXIT_OK
        }
        Err(err) => {
            eprintln!("app show: cannot serialize: {err}");
            EXIT_USAGE
        }
    }
}

fn check_apps(_home: &Home, workspace_path: &Path) -> i32 {
    let outcome = apps::load_apps_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(app_list) = outcome.config else {
        eprintln!("app check: configuration errors");
        return EXIT_USAGE;
    };
    let base = workspace_path.parent().unwrap_or(Path::new("."));
    let mut failures = 0usize;
    for app in &app_list.apps {
        for (platform, entry) in &app.platforms {
            if let Some(package) = &entry.package {
                let path = PathBuf::from(package);
                let resolved = if path.is_absolute() {
                    path
                } else {
                    base.join(path)
                };
                if !resolved.exists() {
                    failures += 1;
                    eprintln!(
                        "IMP-APP-003: app '{}' ({}): package does not exist: {}",
                        app.name,
                        platform,
                        resolved.display()
                    );
                }
            }
            if *platform != Platform::Web && entry.id.is_none() {
                failures += 1;
                eprintln!("IMP-APP-002: app '{}' ({}): missing id", app.name, platform);
            }
        }
    }
    if failures > 0 {
        eprintln!("app check: {failures} problem(s)");
        EXIT_USAGE
    } else {
        println!("app check: {} app(s) ok", app_list.apps.len());
        EXIT_OK
    }
}

fn export_apps(home: &Home, workspace_path: &Path, global: bool, merged: bool) -> i32 {
    if !merged {
        let path = layer_path(home, workspace_path, global, false);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                print!("{text}");
                return EXIT_OK;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("apps: []");
                return EXIT_OK;
            }
            Err(err) => {
                eprintln!("app export: cannot read {}: {err}", path.display());
                return EXIT_USAGE;
            }
        }
    }
    let outcome = apps::load_apps_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(app_list) = outcome.config else {
        return EXIT_USAGE;
    };
    match serde_yaml_ng::to_string(&app_list) {
        Ok(text) => {
            print!("{text}");
            EXIT_OK
        }
        Err(err) => {
            eprintln!("app export: cannot serialize: {err}");
            EXIT_USAGE
        }
    }
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

pub fn migrate(base: &Path, dry_run: bool) -> i32 {
    let devices_path = base.join("impresario.devices.config");
    let apps_path = base.join("impresario.apps.config");
    let source = match std::fs::read_to_string(&devices_path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("migrate: cannot read {}: {err}", devices_path.display());
            return EXIT_USAGE;
        }
    };
    let value = match serde_yaml_ng::from_str::<Value>(&source) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("migrate: invalid YAML in {}: {err}", devices_path.display());
            return EXIT_USAGE;
        }
    };
    let Some(devices) = value.get("devices").and_then(Value::as_sequence) else {
        eprintln!("migrate: {} has no 'devices' list", devices_path.display());
        return EXIT_USAGE;
    };
    let legacy_count = devices
        .iter()
        .filter(|device| device.get("target_endpoint").is_some())
        .count();
    if legacy_count == 0 {
        println!("migrate: devices are already in the device/app split; nothing to do");
        return EXIT_OK;
    }
    if legacy_count != devices.len() {
        eprintln!(
            "migrate: {} mixes legacy and new device entries; convert or remove the new ones first",
            devices_path.display()
        );
        return EXIT_USAGE;
    }

    let mut new_devices: Vec<DeviceConfig> = Vec::new();
    let mut new_apps: Vec<AppConfig> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for device in devices {
        let legacy_name = device.get("name").and_then(Value::as_str).unwrap_or("?");
        let legacy_platform = device
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let legacy_app_url = device.get("app_url").and_then(Value::as_str);
        let legacy_app_path = device
            .get("options")
            .and_then(Value::as_mapping)
            .and_then(|options| options.get("app_path"))
            .and_then(Value::as_str);
        let legacy_clear_data = device
            .get("options")
            .and_then(Value::as_mapping)
            .and_then(|options| options.get("clear_app_data"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match migrate_device(device) {
            Ok((device, app)) => {
                if legacy_app_url.is_some() && app.is_none() {
                    warnings.push(format!(
                        "device '{legacy_name}' ({legacy_platform}) had app_url but no app_id; \
                         the URL was not migrated because only the web platform stores a URL"
                    ));
                }
                if legacy_app_path.is_some()
                    && !matches!(device.platform, Platform::Ios | Platform::Tvos)
                {
                    warnings.push(format!(
                        "device '{legacy_name}' ({legacy_platform}) had app_path; \
                         it was dropped because packages are only meaningful for ios/tvos"
                    ));
                }
                if legacy_clear_data && app.is_none() {
                    warnings.push(format!(
                        "device '{legacy_name}' ({legacy_platform}) had clear_app_data but no \
                         app_id/app_url; the reset flag was not migrated because there is no app \
                         to attach it to"
                    ));
                }
                new_devices.push(device);
                if let Some(app) = app {
                    new_apps.push(app);
                }
            }
            Err(message) => {
                eprintln!("migrate: {message}");
                return EXIT_USAGE;
            }
        }
    }
    let devices_yaml = match serde_yaml_ng::to_string(&DevicesConfig {
        devices: new_devices,
    }) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("migrate: cannot render devices: {err}");
            return EXIT_USAGE;
        }
    };
    let migrated_app_count = new_apps.len();
    let apps_yaml = match serde_yaml_ng::to_string(&AppsConfig {
        apps: new_apps.clone(),
    }) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("migrate: cannot render apps: {err}");
            return EXIT_USAGE;
        }
    };

    for warning in &warnings {
        eprintln!("migrate: warning: {warning}");
    }
    if dry_run {
        println!("# {}", devices_path.display());
        print!("{devices_yaml}");
        if migrated_app_count > 0 {
            println!("# {}", apps_path.display());
            print!("{apps_yaml}");
        }
        return EXIT_OK;
    }

    if let Err(err) = std::fs::write(format!("{}.bak", devices_path.display()), &source) {
        eprintln!("migrate: cannot write the backup: {err}");
        return EXIT_USAGE;
    }
    if apps_path.exists() {
        if let Ok(existing) = std::fs::read(&apps_path) {
            let _ = std::fs::write(format!("{}.bak", apps_path.display()), existing);
        }
    }
    if let Err(err) = std::fs::write(&devices_path, &devices_yaml) {
        eprintln!("migrate: cannot write {}: {err}", devices_path.display());
        return EXIT_USAGE;
    }
    let existing_apps = std::fs::read_to_string(&apps_path)
        .ok()
        .and_then(|source| apps::parse_apps(&source, &apps_path).config)
        .unwrap_or(AppsConfig { apps: Vec::new() });
    let mut merged = existing_apps;
    for app in new_apps {
        let mut name = app.name.clone();
        let mut suffix = 2u32;
        while merged.apps.iter().any(|item| item.name == name) {
            name = format!("{}_{suffix}", app.name);
            suffix += 1;
        }
        let mut app = app;
        app.name = name;
        merged.apps.push(app);
    }
    if migrated_app_count > 0 {
        match serde_yaml_ng::to_string(&merged) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&apps_path, text) {
                    eprintln!("migrate: cannot write {}: {err}", apps_path.display());
                    return EXIT_USAGE;
                }
            }
            Err(err) => {
                eprintln!("migrate: cannot render merged apps: {err}");
                return EXIT_USAGE;
            }
        }
    }
    println!(
        "migrate: converted {} device(s) and {} app(s); backup at {}.bak",
        devices.len(),
        migrated_app_count,
        devices_path.display()
    );
    if migrated_app_count > 0 {
        println!(
            "migrate: apps written to {}; declare `App: <name>` in each play or pass --app",
            apps_path.display()
        );
    }
    for warning in &warnings {
        eprintln!("migrate: warning: {warning}");
    }
    EXIT_OK
}

fn migrate_device(value: &Value) -> Result<(DeviceConfig, Option<AppConfig>), String> {
    let map = value
        .as_mapping()
        .ok_or_else(|| "each device must be a mapping".to_string())?;
    let name = map
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a legacy device has no name".to_string())?
        .to_string();
    if !config::valid_device_name(&name) {
        return Err(format!("invalid device name '{name}'"));
    }
    let platform = map
        .get("platform")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("device '{name}' has no platform"))?
        .parse::<Platform>()
        .map_err(|()| format!("device '{name}' has an unknown platform"))?;
    let target = map
        .get("target_endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("device '{name}' has no target_endpoint"))?
        .to_string();
    let app_id = map
        .get("app_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let app_url = map
        .get("app_url")
        .and_then(Value::as_str)
        .map(str::to_string);
    let agent_address = map
        .get("agent_address")
        .or_else(|| map.get("connector"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let options = map.get("options").and_then(Value::as_mapping);

    let mut connection = DeviceConnection::default();
    match platform {
        Platform::Web => {
            if target == "launch" || target == "launch:headless" {
                connection.transport = TransportKind::Launch;
                if target == "launch:headless" {
                    connection.endpoint = Some(target.clone());
                }
            } else {
                connection.transport = TransportKind::Cdp;
                connection.endpoint = Some(target.clone());
            }
        }
        Platform::WebOs | Platform::Tizen => {
            connection.transport = if platform == Platform::WebOs {
                TransportKind::Ssh
            } else {
                TransportKind::Sdb
            };
            connection.host = Some(url_host(&target).ok_or_else(|| {
                format!("device '{name}': target_endpoint '{target}' is not a ws://host:port URL")
            })?);
            connection.endpoint = Some(target.clone());
        }
        Platform::Android | Platform::AndroidTv => {
            if let Some((host, port)) = target.rsplit_once(':') {
                if !host.is_empty() && port.parse::<u16>().is_ok() {
                    connection.transport = TransportKind::Tcp;
                    connection.host = Some(host.to_string());
                    connection.port = port.parse().ok();
                } else {
                    connection.transport = TransportKind::Usb;
                    connection.serial = Some(target.clone());
                }
            } else {
                connection.transport = TransportKind::Usb;
                connection.serial = Some(target.clone());
            }
        }
        Platform::Ios | Platform::Tvos => {
            connection.udid = Some(target.clone());
            connection.transport = if agent_address.is_some() {
                TransportKind::Network
            } else {
                TransportKind::Usb
            };
            if platform == Platform::Tvos || agent_address.is_some() {
                connection.endpoint = agent_address.clone();
            }
        }
    }

    let mut device_options = DeviceOptions::default();
    let mut clear_data = false;
    let mut app_path = None;
    if let Some(options) = options {
        if let Some(value) = options.get("cdp_backend").and_then(Value::as_str) {
            device_options.cdp_backend = if value == "chromiumoxide" {
                CdpBackend::Chromiumoxide
            } else {
                CdpBackend::Raw
            };
        }
        if let Some(value) = options.get("key_dispatch").and_then(Value::as_str) {
            device_options.key_dispatch = if value == "js" {
                KeyDispatch::Js
            } else {
                KeyDispatch::Cdp
            };
        }
        device_options.page_match = options
            .get("page_match")
            .and_then(Value::as_str)
            .map(str::to_string);
        device_options.agent_port = options
            .get("agent_port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok());
        device_options.animations_disabled = options
            .get("animations_disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        clear_data = options
            .get("clear_app_data")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        app_path = options
            .get("app_path")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if matches!(platform, Platform::Web | Platform::WebOs | Platform::Tizen) {
        device_options.connector_address = agent_address;
    }

    let app = migrate_app(&name, platform, app_id, app_url, clear_data, app_path);
    Ok((
        DeviceConfig {
            name,
            platform,
            connection,
            auth: DeviceAuth::default(),
            labels: Vec::new(),
            options: device_options,
        },
        app,
    ))
}

fn migrate_app(
    device_name: &str,
    platform: Platform,
    app_id: Option<String>,
    app_url: Option<String>,
    clear_data: bool,
    app_path: Option<String>,
) -> Option<AppConfig> {
    let entry = AppPlatform {
        id: if platform == Platform::Web {
            None
        } else {
            app_id
        },
        package: if matches!(platform, Platform::Ios | Platform::Tvos) {
            app_path
        } else {
            None
        },
        launch_activity: None,
        url: if platform == Platform::Web {
            app_url
        } else {
            None
        },
    };
    if platform == Platform::Web && entry.url.is_none() {
        return None;
    }
    if platform != Platform::Web && entry.id.is_none() {
        return None;
    }
    let mut platforms = BTreeMap::new();
    platforms.insert(platform, entry);
    Some(AppConfig {
        name: format!("{device_name}_app"),
        description: Some(format!("Migrated from device '{device_name}'")),
        platforms,
        options: AppOptions {
            clear_data,
            launch_params: BTreeMap::new(),
        },
    })
}

fn url_host(endpoint: &str) -> Option<String> {
    let rest = endpoint
        .strip_prefix("ws://")
        .or_else(|| endpoint.strip_prefix("wss://"))?;
    let authority = rest.split('/').next()?;
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    (!host.is_empty()).then(|| host.to_string())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn parse_transport(text: &str) -> Option<TransportKind> {
    Some(match text {
        "auto" => TransportKind::Auto,
        "usb" => TransportKind::Usb,
        "tcp" => TransportKind::Tcp,
        "network" => TransportKind::Network,
        "cdp" => TransportKind::Cdp,
        "launch" => TransportKind::Launch,
        "ssh" => TransportKind::Ssh,
        "sdb" => TransportKind::Sdb,
        _ => return None,
    })
}

fn layer_path(home: &Home, workspace_path: &Path, global: bool, devices: bool) -> PathBuf {
    if global {
        if devices {
            home.devices_path()
        } else {
            home.apps_path()
        }
    } else {
        workspace_path.to_path_buf()
    }
}

/// Apply `mutate` to the typed file, validate the candidate and only then
/// write it. Pure appends are done as text so existing comments survive;
/// other edits round-trip the typed list (the file is a generated
/// definition layer, not the settings file that `config set` edits losslessly).
fn mutate_devices<F>(path: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&str) -> Result<DevicesConfig, String>,
{
    let source = read_layer(path, "devices");
    let parsed = mutate(&source)?;
    let existing = config::parse_devices(&source, path)
        .config
        .ok_or_else(|| "the current devices file is invalid".to_string())?;
    let candidate = if is_append_only(&existing.devices, &parsed.devices) {
        let appended: Vec<DeviceConfig> = parsed
            .devices
            .iter()
            .skip(existing.devices.len())
            .cloned()
            .collect();
        let body = serde_yaml_ng::to_string(&DevicesConfig { devices: appended })
            .map_err(|err| err.to_string())?;
        append_items(&source, "devices", &body)
    } else {
        serde_yaml_ng::to_string(&parsed).map_err(|err| err.to_string())?
    };
    write_checked(
        path,
        &candidate,
        |source| config::parse_devices(source, path).config.is_some(),
        || render_diagnostics(&config::parse_devices(&candidate, path).diagnostics),
    )
}

fn mutate_apps<F>(path: &Path, mutate: F) -> Result<(), String>
where
    F: FnOnce(&str) -> Result<AppsConfig, String>,
{
    let source = read_layer(path, "apps");
    let parsed = mutate(&source)?;
    let existing = apps::parse_apps(&source, path)
        .config
        .ok_or_else(|| "the current apps file is invalid".to_string())?;
    let candidate = if is_append_only(&existing.apps, &parsed.apps) {
        let appended: Vec<AppConfig> = parsed
            .apps
            .iter()
            .skip(existing.apps.len())
            .cloned()
            .collect();
        let body = serde_yaml_ng::to_string(&AppsConfig { apps: appended })
            .map_err(|err| err.to_string())?;
        append_items(&source, "apps", &body)
    } else {
        serde_yaml_ng::to_string(&parsed).map_err(|err| err.to_string())?
    };
    let outcome = apps::parse_apps(&candidate, path);
    if outcome.config.is_none() {
        return Err(render_diagnostics(&outcome.diagnostics));
    }
    write_candidate(path, &candidate)
}

fn read_layer(path: &Path, key: &str) -> String {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    if source.trim().is_empty() {
        format!("{key}: []\n")
    } else {
        source
    }
}

fn is_append_only<T: PartialEq>(existing: &[T], parsed: &[T]) -> bool {
    parsed.len() > existing.len()
        && existing
            .iter()
            .enumerate()
            .all(|(index, item)| parsed.get(index) == Some(item))
}

/// Add serialized list items after the existing block list, matching its
/// indentation. `body` is a serialized `key:\n- ...` document.
fn append_items(source: &str, key: &str, body: &str) -> String {
    let prefix = format!("{key}:\n");
    let body = body.strip_prefix(&prefix).unwrap_or(body);
    let empty = format!("{key}: []");
    if let Some(position) = source.find(&empty) {
        let mut result = source.to_string();
        let replacement = format!("{key}:\n{}", body.trim_end_matches('\n'));
        result.replace_range(position..position + empty.len(), &replacement);
        return result;
    }
    let indent = sequence_indent(source);
    let mut addition = String::new();
    for line in body.lines() {
        if line.is_empty() {
            addition.push('\n');
        } else {
            addition.push_str(&indent);
            addition.push_str(line);
            addition.push('\n');
        }
    }
    let mut result = source.to_string();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&addition);
    result
}

fn sequence_indent(source: &str) -> String {
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- ") {
            return line[..line.len() - trimmed.len()].to_string();
        }
    }
    String::new()
}

fn write_checked<F, G>(path: &Path, candidate: &str, valid: F, diagnostics: G) -> Result<(), String>
where
    F: Fn(&str) -> bool,
    G: Fn() -> String,
{
    if !valid(candidate) {
        return Err(diagnostics());
    }
    write_candidate(path, candidate)
}

fn write_candidate(path: &Path, candidate: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(path, candidate).map_err(|err| err.to_string())
}
fn render_all(diagnostics: &Diagnostics) {
    for diagnostic in diagnostics.iter() {
        eprintln!("{diagnostic}");
    }
}

fn render_diagnostics(diagnostics: &Diagnostics) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
