//! Device and app definition commands (TASK-2 sections 4.3 and 5).
//!
//! Pure appends are written as text so comments in the existing file survive;
//! other edits round-trip the typed list. Every candidate file is parsed again
//! before it is written, so an invalid edit never reaches disk.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use clap::{Args, Subcommand};
use impresario_core::apps::{self, AppConfig, AppOptions, AppPlatform, AppsConfig};
use impresario_core::config::{self, ActorResolveError, DeviceConfig, DevicesConfig};
use impresario_core::diag::Diagnostics;
use impresario_core::home::Home;
use impresario_core::platforms::{DeviceType, Platform};
use yaml_edit::{SequenceBuilder, YamlFile};

use crate::cli::{EXIT_INFRA, EXIT_OK, EXIT_USAGE};

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
    /// List USB and wireless-debugging devices using the native transport.
    Discover {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// mDNS browse window in seconds.
        #[arg(long, default_value_t = 2)]
        timeout: u64,
    },
    /// Pair an Android 11+ wireless-debugging device (six-digit code).
    Pair {
        /// Pairing endpoint shown on the device (`HOST:PORT`).
        target: String,
        /// Six-digit pairing code shown on the device.
        #[arg(long)]
        code: String,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Save, inspect or clear the webOS Developer Mode passphrase (global).
    Passphrase {
        /// Device name from the merged device configuration.
        name: String,
        /// Remove the saved passphrase.
        #[arg(long, conflicts_with_all = ["status", "stdin"])]
        clear: bool,
        /// Report whether a passphrase is saved (never prints the value).
        #[arg(long, conflicts_with_all = ["clear", "stdin"])]
        status: bool,
        /// Read the passphrase from standard input instead of prompting.
        #[arg(long, conflicts_with_all = ["clear", "status"])]
        stdin: bool,
    },
    /// Interactive wizard with discovery, pairing, preview and scope choice.
    Wizard(crate::wizard::WizardArgs),
}

#[derive(Debug, Args)]
pub struct DeviceAddArgs {
    /// Optional device label (^[a-z0-9][a-z0-9_-]{0,62}$). Derived from type+IP when omitted.
    #[arg(long)]
    pub name: Option<String>,
    /// Exact device type: web, webOS, tizenOS, android, androidtv, iOS or tvOS.
    #[arg(long = "type", value_name = "TYPE")]
    pub device_type: String,
    /// Device IPv4 or IPv6 address (127.0.0.1 for the local web).
    #[arg(long)]
    pub ip: String,
    #[command(flatten)]
    pub scope: DefinitionScope,
}

/// Actor groups (`groups:` in `impresario.devices.config`).
#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Add a group to one layer (fails when the name already exists there).
    Add(GroupEditArgs),
    /// Create or replace a group in one layer.
    Set(GroupEditArgs),
    /// Remove a group from one layer.
    Remove {
        name: String,
        #[command(flatten)]
        scope: DefinitionScope,
    },
    /// List the effective groups with expanded devices and platforms.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct GroupEditArgs {
    /// Group name (^[a-z0-9][a-z0-9_-]{0,62}$).
    pub name: String,
    /// Members: device names or @group references (comma or space separated).
    pub members: Vec<String>,
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
        DeviceCommand::Discover { json, timeout } => {
            crate::wizard::run_discover(home, *json, *timeout)
        }
        DeviceCommand::Pair { target, code, json } => {
            crate::wizard::run_pair(home, target, code, *json)
        }
        DeviceCommand::Passphrase {
            name,
            clear,
            status,
            stdin,
        } => webos_passphrase(name, *clear, *status, *stdin, workspace_path),
        DeviceCommand::Wizard(args) => crate::wizard::run(args, home, workspace_path),
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

// ---------------------------------------------------------------------------
// Actor group commands
// ---------------------------------------------------------------------------

pub fn run_group(command: &GroupCommand, home: &Home, workspace_path: &Path) -> i32 {
    match command {
        GroupCommand::Add(args) => save_group(args, home, workspace_path, false),
        GroupCommand::Set(args) => save_group(args, home, workspace_path, true),
        GroupCommand::Remove { name, scope } => {
            remove_group(name, home, workspace_path, !scope.workspace)
        }
        GroupCommand::List { json } => list_groups(home, workspace_path, *json),
    }
}

/// One effective group row shared by `group list` and the TUI Groups screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupSource {
    Global,
    Workspace,
}

impl GroupSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedDevice {
    pub name: String,
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupInfo {
    pub name: String,
    pub members: Vec<String>,
    pub devices: Vec<ResolvedDevice>,
    pub source: GroupSource,
    pub error: Option<String>,
}

/// Read one devices layer, returning `None` when the file is absent or empty.
fn read_group_layer(path: &Path) -> Result<Option<DevicesConfig>, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if source.trim().is_empty() {
        return Ok(None);
    }
    let outcome = config::parse_devices(&source, path);
    match outcome.config {
        Some(config) => Ok(Some(config)),
        None => Err(render_diagnostics(&outcome.diagnostics)),
    }
}

fn merge_group_layer(merged: &mut DevicesConfig, layer: DevicesConfig) {
    for device in layer.devices {
        if let Some(existing) = merged
            .devices
            .iter_mut()
            .find(|item| item.name == device.name)
        {
            *existing = device;
        } else {
            merged.devices.push(device);
        }
    }
    for (name, members) in layer.groups {
        merged.groups.insert(name, members);
    }
}

/// Effective groups with their source layer and expanded device/platform view.
pub(crate) fn group_rows(home: &Home, workspace_path: &Path) -> Result<Vec<GroupInfo>, String> {
    let global_path = home.devices_path();
    let mut merged = DevicesConfig::default();
    if let Some(layer) = read_group_layer(&global_path)? {
        merge_group_layer(&mut merged, layer);
    }
    let workspace_groups: std::collections::BTreeSet<String> =
        match read_group_layer(workspace_path)? {
            Some(layer) => {
                let names = layer.groups.keys().cloned().collect();
                merge_group_layer(&mut merged, layer);
                names
            }
            None => std::collections::BTreeSet::new(),
        };
    let mut rows = Vec::with_capacity(merged.groups.len());
    for (name, members) in &merged.groups {
        let source = if workspace_groups.contains(name) {
            GroupSource::Workspace
        } else {
            GroupSource::Global
        };
        let mut devices = Vec::new();
        let mut error = None;
        match merged.expand_actor(&format!("@{name}")) {
            Ok(expanded) => {
                for device_name in expanded {
                    if let Some(device) = merged.device(&device_name) {
                        devices.push(ResolvedDevice {
                            name: device_name,
                            platform: device.platform.as_str().to_string(),
                        });
                    }
                }
            }
            Err(ActorResolveError::Unknown(reference)) => {
                error = Some(format!("unknown actor '{reference}'"));
            }
            Err(ActorResolveError::Cycle(path)) => {
                error = Some(format!("membership cycle: {}", path.join(" -> ")));
            }
        }
        rows.push(GroupInfo {
            name: name.clone(),
            members: members.clone(),
            devices,
            source,
            error,
        });
    }
    Ok(rows)
}

fn normalize_members(members: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for member in members {
        for item in member.split(|character: char| character == ',' || character.is_whitespace()) {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if !config::valid_actor_ref(item) {
                return Err(format!(
                    "invalid actor reference '{item}'; use a device name or @group"
                ));
            }
            if !normalized.iter().any(|existing| existing == item) {
                normalized.push(item.to_string());
            }
        }
    }
    Ok(normalized)
}

fn save_group(args: &GroupEditArgs, home: &Home, workspace_path: &Path, replace: bool) -> i32 {
    if !config::valid_device_name(&args.name) {
        eprintln!(
            "group: invalid name '{}'; expected ^[a-z0-9][a-z0-9_-]{{0,62}}$",
            args.name
        );
        return EXIT_USAGE;
    }
    let members = match normalize_members(&args.members) {
        Ok(members) => members,
        Err(message) => {
            eprintln!("group: {message}");
            return EXIT_USAGE;
        }
    };
    if members.is_empty() {
        eprintln!("group: a group needs at least one member");
        return EXIT_USAGE;
    }
    let path = layer_path(home, workspace_path, !args.scope.workspace, true);
    match edit_group(&path, &args.name, &members, replace, home, workspace_path) {
        Ok(()) => {
            let verb = if replace { "saved" } else { "added" };
            println!("{verb} group '{}' in {}", args.name, path.display());
            EXIT_OK
        }
        Err(message) => {
            eprintln!("group: {message}");
            EXIT_USAGE
        }
    }
}

fn edit_group(
    path: &Path,
    name: &str,
    members: &[String],
    replace: bool,
    home: &Home,
    workspace_path: &Path,
) -> Result<(), String> {
    let source = read_layer(path, "devices");
    let file = YamlFile::from_str(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    let document = file.ensure_document();
    let mut sequence = SequenceBuilder::new();
    for member in members {
        sequence = sequence.item(member.as_str());
    }
    let sequence_document = sequence.build_document();
    let Some(value) = sequence_document.as_sequence() else {
        return Err("cannot build the group member list".to_string());
    };
    let candidate = match document.get_mapping("groups") {
        Some(groups) => {
            if !replace && groups.contains_key(name) {
                return Err(format!(
                    "group '{name}' already exists in {}",
                    path.display()
                ));
            }
            groups.set(name, value);
            file.to_string()
        }
        None => {
            if document.contains_key("groups") {
                return Err(format!("groups in {} is not a mapping", path.display()));
            }
            // Appending the whole block keeps the nested indentation
            // unambiguous; an in-tree `Mapping::set` handles the other path.
            // Serialize the members through the same builder as the in-tree
            // path so values that need quoting (`@group` starts a YAML
            // reserved indicator) stay valid YAML.
            let mut candidate = source;
            if !candidate.ends_with('\n') {
                candidate.push('\n');
            }
            candidate.push_str("groups:\n");
            candidate.push_str(&format!("  {name}:\n"));
            for line in sequence_document.to_string().lines() {
                candidate.push_str("    ");
                candidate.push_str(line);
                candidate.push('\n');
            }
            candidate
        }
    };
    validate_group_candidate(path, &candidate, home, workspace_path)?;
    write_candidate(path, &candidate)
}

fn remove_group(name: &str, home: &Home, workspace_path: &Path, global: bool) -> i32 {
    let path = layer_path(home, workspace_path, global, true);
    match delete_group(&path, name, home, workspace_path) {
        Ok(()) => {
            println!("removed group '{name}' from {}", path.display());
            EXIT_OK
        }
        Err(message) => {
            eprintln!("group: {message}");
            EXIT_USAGE
        }
    }
}

fn delete_group(path: &Path, name: &str, home: &Home, workspace_path: &Path) -> Result<(), String> {
    let source = read_layer(path, "devices");
    let file = YamlFile::from_str(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    let document = file.ensure_document();
    let (found, empty) = match document.get_mapping("groups") {
        Some(groups) => {
            let found = groups.remove(name).is_some();
            (found, groups.is_empty())
        }
        None => (false, false),
    };
    if !found {
        return Err(format!("no group named '{name}' in {}", path.display()));
    }
    if empty {
        document.remove("groups");
    }
    let candidate = file.to_string();
    validate_group_candidate(path, &candidate, home, workspace_path)?;
    write_candidate(path, &candidate)
}

/// Validate the prospective file on its own and against the other layer, so a
/// group edit can never leave the effective `groups:` map with an unknown
/// member or a cycle.
fn validate_group_candidate(
    path: &Path,
    candidate: &str,
    home: &Home,
    workspace_path: &Path,
) -> Result<(), String> {
    let outcome = config::parse_devices(candidate, path);
    let Some(target) = outcome.config else {
        return Err(render_diagnostics(&outcome.diagnostics));
    };
    let target_is_workspace = path == workspace_path;
    let mut merged = DevicesConfig::default();
    if target_is_workspace {
        if let Some(layer) = read_group_layer(&home.devices_path())? {
            merge_group_layer(&mut merged, layer);
        }
        merge_group_layer(&mut merged, target);
    } else {
        merge_group_layer(&mut merged, target);
        if let Some(layer) = read_group_layer(workspace_path)? {
            merge_group_layer(&mut merged, layer);
        }
    }
    if let Some((code, message)) = merged.validate_groups().into_iter().next() {
        return Err(format!("{}: {message}", code.as_str()));
    }
    Ok(())
}

fn list_groups(home: &Home, workspace_path: &Path, json: bool) -> i32 {
    let groups = match group_rows(home, workspace_path) {
        Ok(groups) => groups,
        Err(message) => {
            eprintln!("group list: {message}");
            return EXIT_USAGE;
        }
    };
    if json {
        let rows: Vec<serde_json::Value> = groups
            .iter()
            .map(|group| {
                serde_json::json!({
                    "name": group.name,
                    "members": group.members,
                    "source": group.source.as_str(),
                    "devices": group.devices.iter().map(|device| {
                        serde_json::json!({
                            "name": device.name,
                            "platform": device.platform,
                        })
                    }).collect::<Vec<_>>(),
                    "error": group.error,
                })
            })
            .collect();
        match serde_json::to_string_pretty(&rows) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("group list: cannot serialize: {error}");
                return EXIT_USAGE;
            }
        }
        return EXIT_OK;
    }
    if groups.is_empty() {
        println!("no groups configured (add one with `impresario group add`)");
        return EXIT_OK;
    }
    println!("{:<20} {:<10} {:<34} RESOLVED", "NAME", "SOURCE", "MEMBERS");
    for group in groups {
        let members = group.members.join(", ");
        let resolved = match &group.error {
            Some(error) => format!("! {error}"),
            None if group.devices.is_empty() => "(empty)".to_string(),
            None => group
                .devices
                .iter()
                .map(|device| format!("{} [{}]", device.name, device.platform))
                .collect::<Vec<_>>()
                .join(", "),
        };
        println!(
            "{:<20} {:<10} {:<34} {}",
            group.name,
            group.source.as_str(),
            members,
            resolved
        );
    }
    EXIT_OK
}

/// Save, inspect or clear the global webOS Developer Mode passphrase. The OS
/// credential-store account is the device name, so the secret is shared by
/// every workspace that uses the same device definition.
fn webos_passphrase(
    name: &str,
    clear: bool,
    status: bool,
    from_stdin: bool,
    workspace_path: &Path,
) -> i32 {
    // `--clear` and `--status` operate on the keychain entry alone, so they
    // still work after a device was removed or renamed.
    if clear {
        return match impresario_clients::webos::clear_webos_passphrase(name) {
            Ok(true) => {
                println!("cleared the webOS passphrase for '{name}'");
                EXIT_OK
            }
            Ok(false) => {
                println!("no webOS passphrase is saved for '{name}'");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("device passphrase: {error}");
                EXIT_INFRA
            }
        };
    }
    if status {
        return match impresario_clients::webos::webos_passphrase_saved(name) {
            Ok(true) => {
                println!("webOS passphrase saved for '{name}'");
                EXIT_OK
            }
            Ok(false) => {
                println!("no webOS passphrase is saved for '{name}'");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("device passphrase: {error}");
                EXIT_INFRA
            }
        };
    }

    let outcome = config::load_devices_merged(workspace_path);
    render_all(&outcome.diagnostics);
    let Some(devices) = outcome.config else {
        eprintln!("device passphrase: configuration errors");
        return EXIT_USAGE;
    };
    let Some(device) = devices.devices.iter().find(|item| item.name == name) else {
        eprintln!(
            "device passphrase: device '{name}' does not exist in {}",
            workspace_path.display()
        );
        return EXIT_USAGE;
    };
    if device.platform != Platform::WebOs {
        eprintln!(
            "device passphrase: '{name}' is {}; only webOS devices have a Developer Mode passphrase",
            device.device_type.as_str()
        );
        return EXIT_USAGE;
    }

    let passphrase = if from_stdin {
        match read_webos_passphrase_from_stdin() {
            Ok(passphrase) => passphrase,
            Err(message) => {
                eprintln!("device passphrase: {message}");
                return EXIT_USAGE;
            }
        }
    } else if let Ok(value) = std::env::var("IMPRESARIO_WEBOS_PASSPHRASE") {
        if value.is_empty() {
            eprintln!("device passphrase: IMPRESARIO_WEBOS_PASSPHRASE is empty");
            return EXIT_USAGE;
        }
        zeroize::Zeroizing::new(value)
    } else {
        match prompt_webos_passphrase(name) {
            Ok(passphrase) => passphrase,
            Err(message) => {
                eprintln!("device passphrase: {message}");
                return EXIT_USAGE;
            }
        }
    };

    match impresario_clients::webos::store_webos_passphrase(name, passphrase.as_str()) {
        Ok(()) => {
            println!("saved the webOS passphrase for '{name}' in the OS credential store");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("device passphrase: {error}");
            EXIT_INFRA
        }
    }
}

fn prompt_webos_passphrase(name: &str) -> Result<zeroize::Zeroizing<String>, String> {
    let passphrase = rpassword::prompt_password(format!(
        "Developer Mode passphrase for {name} (not echoed): "
    ))
    .map_err(|error| format!("cannot read the passphrase: {error}"))?;
    let passphrase = zeroize::Zeroizing::new(passphrase);
    if passphrase.is_empty() {
        return Err("the passphrase cannot be empty".to_string());
    }
    let confirm = rpassword::prompt_password("Confirm passphrase: ")
        .map_err(|error| format!("cannot read the confirmation: {error}"))?;
    let confirm = zeroize::Zeroizing::new(confirm);
    if passphrase.as_str() != confirm.as_str() {
        return Err("the passphrases do not match".to_string());
    }
    Ok(passphrase)
}

fn read_webos_passphrase_from_stdin() -> Result<zeroize::Zeroizing<String>, String> {
    use std::io::BufRead as _;

    let mut line = zeroize::Zeroizing::new(String::new());
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("cannot read the passphrase from stdin: {error}"))?;
    let passphrase = line.trim_end_matches(['\r', '\n']).to_string();
    if passphrase.is_empty() {
        return Err("the passphrase read from stdin is empty".to_string());
    }
    Ok(zeroize::Zeroizing::new(passphrase))
}

fn add_device(args: &DeviceAddArgs, home: &Home, workspace_path: &Path, replace: bool) -> i32 {
    let device_type = match args.device_type.parse::<DeviceType>() {
        Ok(device_type) => device_type,
        Err(()) => {
            eprintln!(
                "device add: unknown type '{}'; expected web, webOS, tizenOS, android, androidtv, iOS or tvOS",
                args.device_type
            );
            return EXIT_USAGE;
        }
    };
    let ip = match args.ip.trim().parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => {
            eprintln!("device add: '{}' is not an IPv4 or IPv6 address", args.ip);
            return EXIT_USAGE;
        }
    };
    let platform = device_type.platform();
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| format!("{}-{}", device_type.slug(), ip.replace([':', '.'], "-")));
    let mut device = DeviceConfig::new(name, platform, ip);
    device.device_type = device_type;
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

/// Path of the layer a wizard/tool write would touch.
pub(crate) fn device_layer_path(
    home: &Home,
    workspace_path: &Path,
    workspace_scope: bool,
) -> PathBuf {
    layer_path(home, workspace_path, !workspace_scope, true)
}

/// Upsert one device definition through the same validated mutation path as
/// `device add`/`device edit`. The wizard uses this after its preview, so a
/// cancelled or invalid session never writes anything.
pub(crate) fn save_device(
    home: &Home,
    workspace_path: &Path,
    workspace_scope: bool,
    replace: bool,
    device: &DeviceConfig,
) -> Result<PathBuf, String> {
    let path = device_layer_path(home, workspace_path, workspace_scope);
    mutate_devices(&path, |source| {
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
    })?;
    Ok(path)
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
        "NAME", "TYPE", "TRANSPORT", "ORIGIN"
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
            device.device_type.as_str(),
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

pub(crate) fn device_names(path: &Path) -> HashSet<String> {
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
        groups: std::collections::BTreeMap::new(),
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

pub(crate) fn app_names(path: &Path) -> HashSet<String> {
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
// Shared helpers
// ---------------------------------------------------------------------------

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
        let body = serde_yaml_ng::to_string(&DevicesConfig {
            devices: appended,
            groups: std::collections::BTreeMap::new(),
        })
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
