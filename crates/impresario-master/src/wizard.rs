//! Android discovery/pairing and the interactive device wizard (TASK-2
//! section 4.3, phase 3).
//!
//! All network and device access uses the native transport in
//! `impresario-clients`; nothing here spawns `adb` or any other platform tool
//! (D-19). Every prompt has a default, validates immediately, and supports
//! `b` (back) and `q` (cancel). The configuration is written only after the
//! final YAML/scope preview is accepted.

use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::Duration;

use clap::Args;
use impresario_clients::adb::{self, AdbClient, AdbTarget, MdnsAndroidDevice, UsbAndroidDevice};
use impresario_core::config::{
    self, DeviceAuth, DeviceConfig, DeviceConnection, DeviceOptions, TransportKind,
};
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use impresario_core::tools;
use serde::Serialize;

use crate::cli::{EXIT_INFRA, EXIT_OK, EXIT_USAGE};
use crate::definitions;

/// Interactive wizard options.
#[derive(Debug, Args)]
pub struct WizardArgs {
    /// Edit this device instead of adding a new one.
    #[arg(long, value_name = "NAME")]
    pub edit: Option<String>,
    /// Default the definition to the workspace layer.
    #[arg(long)]
    pub workspace: bool,
    /// Skip the connectivity check step.
    #[arg(long)]
    pub no_check: bool,
}

#[derive(Debug, Serialize)]
struct DiscoverReport {
    android_key: String,
    usb: Vec<UsbRow>,
    mdns: Vec<MdnsRow>,
    errors: Vec<String>,
}

#[derive(Debug, Serialize)]
struct UsbRow {
    serial: String,
    vendor_id: String,
    product_id: String,
    product: String,
    manufacturer: String,
    driver: Option<String>,
    adb_interface: bool,
}

#[derive(Debug, Serialize)]
struct MdnsRow {
    name: String,
    host: String,
    port: u16,
    kind: String,
}

/// `impresario device discover`.
pub fn run_discover(home: &Home, json: bool, timeout: u64) -> i32 {
    let timeout = Duration::from_secs(timeout.clamp(1, 60));
    let mut errors = Vec::new();
    let usb = match adb::discover_usb() {
        Ok(devices) => devices,
        Err(error) => {
            errors.push(format!("USB discovery failed: {error}"));
            Vec::new()
        }
    };
    let mdns = match adb::discover_mdns(timeout) {
        Ok(devices) => devices,
        Err(error) => {
            errors.push(format!("mDNS discovery failed: {error}"));
            Vec::new()
        }
    };
    let report = DiscoverReport {
        android_key: home.keys_dir().join("adbkey").display().to_string(),
        usb: usb.iter().map(usb_row).collect(),
        mdns: mdns.iter().map(mdns_row).collect(),
        errors,
    };
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("device discover: cannot render JSON: {error}");
                return EXIT_INFRA;
            }
        }
    } else {
        println!("Android devices (native ADB, no adb binary)");
        println!("host key: {}", report.android_key);
        println!();
        if report.usb.is_empty() {
            println!("USB: none");
        } else {
            println!("USB:");
            for device in &usb {
                let driver = device.driver.as_deref().unwrap_or("unknown driver");
                let adb = if device.adb_interface {
                    "ADB interface"
                } else {
                    "no ADB interface (charging-only or driver missing)"
                };
                println!(
                    "  {}  {}  {:04x}:{:04x}  {driver}  [{adb}]",
                    device.serial,
                    product_label(device),
                    device.vendor_id,
                    device.product_id
                );
            }
        }
        println!();
        if mdns.is_empty() {
            println!("mDNS: none (enable wireless debugging, or use USB)");
        } else {
            println!("mDNS:");
            for service in &mdns {
                println!(
                    "  {}  {}:{}  {}",
                    service.kind, service.host, service.port, service.name
                );
            }
        }
        for error in &report.errors {
            println!();
            println!("warning: {error}");
        }
    }
    if report.errors.len() == 2 {
        EXIT_INFRA
    } else {
        EXIT_OK
    }
}

fn usb_row(device: &UsbAndroidDevice) -> UsbRow {
    UsbRow {
        serial: device.serial.clone(),
        vendor_id: format!("{:04x}", device.vendor_id),
        product_id: format!("{:04x}", device.product_id),
        product: device.product.clone(),
        manufacturer: device.manufacturer.clone(),
        driver: device.driver.clone(),
        adb_interface: device.adb_interface,
    }
}

fn mdns_row(device: &MdnsAndroidDevice) -> MdnsRow {
    MdnsRow {
        name: device.name.clone(),
        host: device.host.clone(),
        port: device.port,
        kind: device.kind.clone(),
    }
}

fn product_label(device: &UsbAndroidDevice) -> String {
    if device.product.is_empty() {
        device.manufacturer.clone()
    } else if device.manufacturer.is_empty() {
        device.product.clone()
    } else {
        format!("{} {}", device.manufacturer, device.product)
    }
}

/// `impresario device pair`.
pub fn run_pair(home: &Home, target: &str, code: &str, json: bool) -> i32 {
    if !is_host_port(target) {
        eprintln!("device pair: target '{target}' must be the HOST:PORT shown on the device");
        return EXIT_USAGE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("device pair: cannot start the async runtime: {error}");
            return EXIT_INFRA;
        }
    };
    match runtime.block_on(adb::pair_wireless(target, code, &home.keys_dir())) {
        Ok(message) => {
            if json {
                let value = serde_json::json!({
                    "paired": true,
                    "target": target,
                    "key_dir": home.keys_dir().display().to_string(),
                    "message": message,
                });
                match serde_json::to_string_pretty(&value) {
                    Ok(text) => println!("{text}"),
                    Err(error) => {
                        eprintln!("device pair: cannot render JSON: {error}");
                        return EXIT_INFRA;
                    }
                }
            } else {
                println!("{message}");
                println!("host key: {}", home.keys_dir().join("adbkey").display());
                println!(
                    "next: `impresario device discover` to find the wireless connect port, then \
                     `impresario device add ...` or `impresario device wizard`"
                );
            }
            EXIT_OK
        }
        Err(error) => {
            eprintln!("device pair: {error}");
            match error {
                adb::AdbError::Usage(_) => EXIT_USAGE,
                _ => EXIT_INFRA,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Wizard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Stage {
    Scope,
    Platform,
    Name,
    AndroidRoute,
    AndroidManual,
    AndroidPairTarget,
    AndroidPairCode,
    AndroidAfterPair,
    TvosUdid,
    TvosEndpoint,
    WebMode,
    WebEndpoint,
    WebPageMatch,
    ConnectorAddress,
    AgentPort,
    Labels,
    Check,
    Review,
}

enum Answer<T> {
    Value(T),
    Back,
    Cancel,
}

struct Ui<R: BufRead, W: Write> {
    reader: R,
    writer: W,
}

impl<R: BufRead, W: Write> Ui<R, W> {
    fn say(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        writeln!(self.writer, "{}", text.as_ref())?;
        self.writer.flush()
    }

    fn ask_value<T, F, D>(
        &mut self,
        label: &str,
        default: Option<&T>,
        parse: F,
        display: D,
    ) -> io::Result<Answer<T>>
    where
        T: Clone,
        F: Fn(&str) -> Result<T, String>,
        D: Fn(&T) -> String,
    {
        loop {
            write!(self.writer, "{label}")?;
            if let Some(default) = default {
                write!(self.writer, " [{}]", display(default))?;
            }
            write!(self.writer, ": ")?;
            self.writer.flush()?;
            let mut line = String::new();
            let read = self.reader.read_line(&mut line)?;
            if read == 0 {
                return Ok(Answer::Cancel);
            }
            let text = line.trim();
            if text.eq_ignore_ascii_case("q") || text.eq_ignore_ascii_case("quit") {
                return Ok(Answer::Cancel);
            }
            if text.eq_ignore_ascii_case("b") {
                return Ok(Answer::Back);
            }
            if text.is_empty() {
                if let Some(default) = default {
                    return Ok(Answer::Value(default.clone()));
                }
                writeln!(self.writer, "! a value is required")?;
                continue;
            }
            match parse(text) {
                Ok(value) => return Ok(Answer::Value(value)),
                Err(error) => {
                    writeln!(self.writer, "! {error}")?;
                }
            }
        }
    }

    fn ask_optional(
        &mut self,
        label: &str,
        default: Option<&String>,
    ) -> io::Result<Answer<Option<String>>> {
        let parse = |text: &str| {
            if text == "-" || text.eq_ignore_ascii_case("none") {
                Ok(None)
            } else {
                Ok(Some(text.to_string()))
            }
        };
        let display = |value: &Option<String>| match value {
            Some(value) => value.clone(),
            None => "none".to_string(),
        };
        let default_value = default.cloned();
        self.ask_value(label, Some(&default_value), parse, display)
    }

    fn confirm(&mut self, label: &str, default: bool) -> io::Result<Answer<bool>> {
        let parse = |text: &str| match text.to_ascii_lowercase().as_str() {
            "y" | "yes" => Ok(true),
            "n" | "no" => Ok(false),
            _ => Err("answer y or n".to_string()),
        };
        let display = |value: &bool| if *value { "Y/n" } else { "y/N" }.to_string();
        self.ask_value(label, Some(&default), parse, display)
    }
}

#[derive(Debug, Clone)]
struct RouteOption {
    label: String,
    connection: DeviceConnection,
}

#[derive(Debug, Default)]
struct Discovery {
    routes: Vec<RouteOption>,
    pairing: Vec<(String, u16)>,
    notes: Vec<String>,
}

#[derive(Debug)]
struct Answers {
    scope_workspace: bool,
    platform: Option<Platform>,
    name: String,
    connection: DeviceConnection,
    auth: DeviceAuth,
    labels: Vec<String>,
    options: DeviceOptions,
    last_pair_host: Option<String>,
}

impl Answers {
    fn from_edit(device: &DeviceConfig) -> Self {
        Self {
            scope_workspace: false,
            platform: Some(device.platform),
            name: device.name.clone(),
            connection: device.connection.clone(),
            auth: device.auth.clone(),
            labels: device.labels.clone(),
            options: device.options.clone(),
            last_pair_host: None,
        }
    }

    fn reset_transport(&mut self, platform: Platform) {
        if self.platform != Some(platform) {
            self.connection = DeviceConnection::default();
            self.options = DeviceOptions::default();
            self.platform = Some(platform);
        }
    }

    fn device(&self) -> Option<DeviceConfig> {
        Some(DeviceConfig {
            name: self.name.clone(),
            platform: self.platform?,
            connection: self.connection.clone(),
            auth: self.auth.clone(),
            labels: self.labels.clone(),
            options: self.options.clone(),
        })
    }
}

struct Session<'a, R: BufRead, W: Write> {
    ui: Ui<R, W>,
    home: &'a Home,
    workspace_path: &'a Path,
    args: &'a WizardArgs,
    edit: Option<&'a DeviceConfig>,
    original_name: Option<String>,
    global_has_edit: bool,
    workspace_has_edit: bool,
    answers: Answers,
    runtime: tokio::runtime::Runtime,
    discovery: Option<Discovery>,
}

fn run_wizard_io<R: BufRead, W: Write>(
    args: &WizardArgs,
    home: &Home,
    workspace_path: &Path,
    reader: R,
    writer: &mut W,
) -> i32 {
    let outcome = config::load_devices_merged(workspace_path);
    if outcome.diagnostics.has_errors() {
        for diagnostic in outcome.diagnostics.iter() {
            let _ = writeln!(writer, "{diagnostic}");
        }
        let _ = writeln!(
            writer,
            "device wizard: fix {} before adding or editing devices",
            workspace_path.display()
        );
        return EXIT_USAGE;
    }
    let Some(devices) = outcome.config else {
        let _ = writeln!(
            writer,
            "device wizard: cannot read {}",
            workspace_path.display()
        );
        return EXIT_USAGE;
    };
    let edit = match args.edit.as_deref() {
        Some(name) => match devices.devices.iter().find(|device| device.name == name) {
            Some(device) => Some(device),
            None => {
                let _ = writeln!(writer, "device wizard: device '{name}' does not exist");
                return EXIT_USAGE;
            }
        },
        None => None,
    };
    let global_has_edit = match edit {
        Some(device) => match layer_has(&home.devices_path(), &device.name) {
            Ok(has) => has,
            Err(error) => {
                let _ = writeln!(writer, "device wizard: {error}");
                return EXIT_USAGE;
            }
        },
        None => false,
    };
    let workspace_has_edit = match edit {
        Some(device) => match layer_has(workspace_path, &device.name) {
            Ok(has) => has,
            Err(error) => {
                let _ = writeln!(writer, "device wizard: {error}");
                return EXIT_USAGE;
            }
        },
        None => false,
    };
    if edit.is_some() && !global_has_edit && !workspace_has_edit {
        let _ = writeln!(
            writer,
            "device wizard: the device is not present in either writable layer"
        );
        return EXIT_USAGE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(
                writer,
                "device wizard: cannot start the async runtime: {error}"
            );
            return EXIT_INFRA;
        }
    };
    let answers = match edit {
        Some(device) => Answers::from_edit(device),
        None => Answers {
            scope_workspace: args.workspace,
            platform: None,
            name: String::new(),
            connection: DeviceConnection::default(),
            auth: DeviceAuth::default(),
            labels: Vec::new(),
            options: DeviceOptions::default(),
            last_pair_host: None,
        },
    };
    let default_workspace = if edit.is_some() {
        workspace_has_edit
    } else {
        args.workspace
    };
    let mut session = Session {
        ui: Ui { reader, writer },
        home,
        workspace_path,
        args,
        edit,
        original_name: edit.map(|device| device.name.clone()),
        global_has_edit,
        workspace_has_edit,
        answers,
        runtime,
        discovery: None,
    };
    session.answers.scope_workspace = default_workspace;
    session.run()
}

impl<R: BufRead, W: Write> Session<'_, R, W> {
    fn run(&mut self) -> i32 {
        if self
            .ui
            .say("impresario device wizard\nEvery prompt: Enter = default, b = back, q = cancel.\nNothing is written before the final preview.")
            .is_err()
        {
            return EXIT_INFRA;
        }
        let mut stage = Stage::Scope;
        loop {
            let result = match stage {
                Stage::Scope => self.scope(),
                Stage::Platform => self.platform(),
                Stage::Name => self.name(),
                Stage::AndroidRoute => self.android_route(),
                Stage::AndroidManual => self.android_manual(),
                Stage::AndroidPairTarget => self.android_pair_target(),
                Stage::AndroidPairCode => self.android_pair_code(),
                Stage::AndroidAfterPair => self.android_after_pair(),
                Stage::TvosUdid => self.tvos_udid(),
                Stage::TvosEndpoint => self.tvos_endpoint(),
                Stage::WebMode => self.web_mode(),
                Stage::WebEndpoint => self.web_endpoint(),
                Stage::WebPageMatch => self.web_page_match(),
                Stage::ConnectorAddress => self.connector_address(),
                Stage::AgentPort => self.agent_port(),
                Stage::Labels => self.labels(),
                Stage::Check => self.check(),
                Stage::Review => self.review(),
            };
            match result {
                Ok(Step::Next(next)) => stage = next,
                Ok(Step::Back) => {
                    if let Some(previous) = previous_stage(stage, &self.answers) {
                        stage = previous;
                    } else {
                        let _ = self.ui.say("already at the first step");
                    }
                }
                Ok(Step::Done(code)) => return code,
                Err(error) => {
                    let _ = self.ui.say(format!("device wizard: {error}"));
                    return EXIT_INFRA;
                }
            }
        }
    }

    fn scope(&mut self) -> Result<Step, String> {
        let default = self.answers.scope_workspace;
        let edit = self.edit;
        let global_has = self.global_has_edit;
        let workspace_has = self.workspace_has_edit;
        let parse = |text: &str| match text.to_ascii_lowercase().as_str() {
            "global" => {
                if edit.is_some() && !global_has {
                    Err("this device exists only in the workspace layer".to_string())
                } else {
                    Ok(false)
                }
            }
            "workspace" => {
                if edit.is_some() && !workspace_has {
                    Err("this device exists only in the global layer".to_string())
                } else {
                    Ok(true)
                }
            }
            _ => Err("expected global or workspace".to_string()),
        };
        match self
            .ui
            .ask_value("Scope (global/workspace)", Some(&default), parse, |v| {
                if *v {
                    "workspace".to_string()
                } else {
                    "global".to_string()
                }
            })
            .map_err(|error| error.to_string())?
        {
            Answer::Value(workspace) => {
                self.answers.scope_workspace = workspace;
                Ok(Step::Next(Stage::Platform))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn platform(&mut self) -> Result<Step, String> {
        let default = self.answers.platform.unwrap_or(Platform::Android);
        let parse = |text: &str| match text.to_ascii_lowercase().as_str() {
            "android" | "phone" => Ok(Platform::Android),
            "androidtv" | "android-tv" | "tv" => Ok(Platform::AndroidTv),
            "web" => Ok(Platform::Web),
            "tvos" | "appletv" | "apple-tv" => Ok(Platform::Tvos),
            "webos" | "tizen" | "ios" => Err(format!(
                "'{text}' is not covered by this wizard yet; it covers android, androidtv, web and \
                 physical tvos"
            )),
            _ => Err("expected android, androidtv, web or tvos".to_string()),
        };
        match self
            .ui
            .ask_value(
                "Platform (android/androidtv/web/tvos)",
                Some(&default),
                parse,
                |value| value.as_str().to_string(),
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(platform) => {
                self.answers.reset_transport(platform);
                Ok(Step::Next(Stage::Name))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn name(&mut self) -> Result<Step, String> {
        let default = if self.answers.name.is_empty() {
            default_name(
                self.answers.platform.unwrap_or(Platform::Android),
                self.edit,
            )
        } else {
            self.answers.name.clone()
        };
        let original = self.original_name.clone();
        let parse = move |text: &str| {
            if !config::valid_device_name(text) {
                return Err("names match ^[a-z0-9][a-z0-9_-]{0,62}$".to_string());
            }
            if let Some(original) = original.as_deref() {
                if original != text {
                    return Err("use `impresario device rename` to change the name".to_string());
                }
            }
            Ok(text.to_string())
        };
        match self
            .ui
            .ask_value("Device name", Some(&default), parse, Clone::clone)
            .map_err(|error| error.to_string())?
        {
            Answer::Value(name) => {
                self.answers.name = name;
                let platform = self
                    .answers
                    .platform
                    .ok_or_else(|| "platform was not selected".to_string())?;
                Ok(Step::Next(match platform {
                    Platform::Android | Platform::AndroidTv => Stage::AndroidRoute,
                    Platform::Tvos => Stage::TvosUdid,
                    Platform::Web => Stage::WebMode,
                    _ => Stage::Name,
                }))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn android_route(&mut self) -> Result<Step, String> {
        if self.discovery.is_none() {
            self.discovery = Some(discover());
        }
        let discovery = self
            .discovery
            .as_ref()
            .ok_or_else(|| "discovery did not run".to_string())?;
        let _ = self.ui.say("");
        let _ = self
            .ui
            .say("Android devices found (native ADB; no adb binary is used):");
        if discovery.routes.is_empty() {
            let _ = self
                .ui
                .say("  none — enter an address manually, or pair a new wireless device");
        }
        let current = manual_default(&self.answers.connection);
        for (index, route) in discovery.routes.iter().enumerate() {
            let marker = if self.edit.is_some() && manual_default(&route.connection) == current {
                "  (current)"
            } else {
                ""
            };
            let _ = self
                .ui
                .say(format!("  {}) {}{marker}", index + 1, route.label));
        }
        for note in &discovery.notes {
            let _ = self.ui.say(format!("  note: {note}"));
        }
        if !discovery.pairing.is_empty() {
            let values: Vec<String> = discovery
                .pairing
                .iter()
                .map(|(host, port)| format!("{host}:{port}"))
                .collect();
            let _ = self.ui.say(format!(
                "  pairing endpoints: {} (use p)",
                values.join(", ")
            ));
        }
        let default = if self.edit.is_some() || discovery.routes.is_empty() {
            "m".to_string()
        } else {
            "1".to_string()
        };
        let parse = |text: &str| {
            let lower = text.to_ascii_lowercase();
            match lower.as_str() {
                "m" | "manual" | "p" | "pair" | "r" | "refresh" => Ok(lower),
                _ => match text.parse::<usize>() {
                    Ok(index) if (1..=discovery.routes.len()).contains(&index) => Ok(lower),
                    _ => Err(
                        "choose a listed number, m (manual), p (pair) or r (refresh)".to_string(),
                    ),
                },
            }
        };
        match self
            .ui
            .ask_value("Route (number/m/p/r)", Some(&default), parse, Clone::clone)
            .map_err(|error| error.to_string())?
        {
            Answer::Value(choice) => match choice.as_str() {
                "r" | "refresh" => {
                    self.discovery = None;
                    Ok(Step::Next(Stage::AndroidRoute))
                }
                "m" | "manual" => Ok(Step::Next(Stage::AndroidManual)),
                "p" | "pair" => Ok(Step::Next(Stage::AndroidPairTarget)),
                _ => match choice.parse::<usize>() {
                    Ok(index) => match discovery.routes.get(index - 1) {
                        Some(route) => {
                            self.answers.connection = route.connection.clone();
                            Ok(Step::Next(Stage::AgentPort))
                        }
                        None => Ok(Step::Next(Stage::AndroidRoute)),
                    },
                    Err(_) => Ok(Step::Next(Stage::AndroidRoute)),
                },
            },
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn android_manual(&mut self) -> Result<Step, String> {
        let default = manual_default(&self.answers.connection)
            .and_then(|value| parse_android_address(&value).ok());
        let parse = |text: &str| parse_android_address(text).map_err(|error| error.to_string());
        match self
            .ui
            .ask_value(
                "Address (HOST:PORT, host, or usb:SERIAL)",
                default.as_ref(),
                parse,
                AndroidAddress::display,
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(address) => {
                self.answers.connection = address.connection();
                Ok(Step::Next(Stage::AgentPort))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn android_pair_target(&mut self) -> Result<Step, String> {
        let discovery = self.discovery.as_ref();
        let default = discovery
            .and_then(|discovery| discovery.pairing.first())
            .map(|(host, port)| format!("{host}:{port}"))
            .or_else(|| {
                if self.answers.connection.host.is_some() {
                    self.answers.connection.host.clone()
                } else {
                    None
                }
            });
        if let Some(discovery) = discovery {
            if !discovery.pairing.is_empty() {
                let _ = self.ui.say("Pairing endpoints advertised over mDNS:");
                for (host, port) in &discovery.pairing {
                    let _ = self.ui.say(format!("  {host}:{port}"));
                }
            }
        }
        let _ = self.ui.say(
            "On the device: Developer options → Wireless debugging → Pair device with pairing code.",
        );
        let parse = |text: &str| {
            if is_host_port(text) {
                Ok(text.to_string())
            } else {
                Err("expected the HOST:PORT shown in the pairing dialog".to_string())
            }
        };
        match self
            .ui
            .ask_value(
                "Pairing endpoint (HOST:PORT)",
                default.as_ref(),
                parse,
                Clone::clone,
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(target) => {
                self.answers.last_pair_host =
                    target.rsplit_once(':').map(|(host, _)| host.to_string());
                self.answers.connection.endpoint = Some(target);
                Ok(Step::Next(Stage::AndroidPairCode))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn android_pair_code(&mut self) -> Result<Step, String> {
        let target = self
            .answers
            .connection
            .endpoint
            .clone()
            .ok_or_else(|| "pairing endpoint was not selected".to_string())?;
        let parse = |text: &str| {
            if text.len() == 6 && text.bytes().all(|byte| byte.is_ascii_digit()) {
                Ok(text.to_string())
            } else {
                Err("the pairing code is exactly six digits".to_string())
            }
        };
        match self
            .ui
            .ask_value("Six-digit pairing code", None, parse, Clone::clone)
            .map_err(|error| error.to_string())?
        {
            Answer::Value(code) => {
                let result = self.runtime.block_on(adb::pair_wireless(
                    &target,
                    &code,
                    &self.home.keys_dir(),
                ));
                match result {
                    Ok(message) => {
                        let _ = self.ui.say(message);
                        self.answers.connection.endpoint = None;
                        Ok(Step::Next(Stage::AndroidAfterPair))
                    }
                    Err(error) => {
                        let _ = self.ui.say(format!("pairing failed: {error}"));
                        match self
                            .ui
                            .confirm("Try another pairing code?", false)
                            .map_err(|error| error.to_string())?
                        {
                            Answer::Value(true) => Ok(Step::Next(Stage::AndroidPairCode)),
                            Answer::Value(false) | Answer::Cancel => {
                                Ok(Step::Next(Stage::AndroidRoute))
                            }
                            Answer::Back => Ok(Step::Next(Stage::AndroidRoute)),
                        }
                    }
                }
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn android_after_pair(&mut self) -> Result<Step, String> {
        let default = self
            .discovery
            .as_ref()
            .and_then(|discovery| {
                discovery.routes.iter().find_map(|route| {
                    if route.connection.transport == TransportKind::Tcp {
                        match (&route.connection.host, route.connection.port) {
                            (Some(host), Some(port)) => Some(format!("{host}:{port}")),
                            (Some(host), None) => Some(host.clone()),
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                self.answers
                    .last_pair_host
                    .as_ref()
                    .map(|host| format!("{host}:5555"))
            })
            .and_then(|value| parse_android_address(&value).ok());
        let _ = self.ui.say(
            "Pairing is complete. Enter the wireless-debugging connect endpoint (the device may \
             show a different port from the pairing port).",
        );
        let parse = |text: &str| parse_android_address(text).map_err(|error| error.to_string());
        match self
            .ui
            .ask_value(
                "Connect address (HOST:PORT)",
                default.as_ref(),
                parse,
                AndroidAddress::display,
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(address) => {
                self.answers.connection = address.connection();
                Ok(Step::Next(Stage::AgentPort))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn tvos_udid(&mut self) -> Result<Step, String> {
        let default = self.answers.connection.udid.clone();
        let _ = self.ui.say(
            "Pair the Apple TV once with `impresario apple pair --host <ip>` (or import an \
             existing pair plist with `impresario apple import-pair`). The UDID is the device's \
             Apple UDID; `impresario apple status` lists saved pairings.",
        );
        match self
            .ui
            .ask_value(
                "Physical Apple TV UDID",
                default.as_ref(),
                parse_tvos_udid,
                Clone::clone,
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(udid) => {
                self.answers.connection.udid = Some(udid);
                Ok(Step::Next(Stage::TvosEndpoint))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn tvos_endpoint(&mut self) -> Result<Step, String> {
        let default = self.answers.connection.endpoint.clone();
        let _ = self.ui.say(
            "Enter the Apple TV's LAN address and the agent port (usually 50062). Impresario \
             starts Impresario Agent on that device; this is not a Mac address.",
        );
        match self
            .ui
            .ask_value(
                "Agent endpoint (Apple TV LAN HOST:PORT, usually :50062)",
                default.as_ref(),
                parse_tvos_endpoint,
                Clone::clone,
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(endpoint) => {
                self.answers.connection.transport = TransportKind::Network;
                self.answers.connection.endpoint = Some(endpoint);
                Ok(Step::Next(Stage::Labels))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn web_mode(&mut self) -> Result<Step, String> {
        let default = match (
            self.answers.connection.transport,
            self.answers.connection.endpoint.as_deref(),
        ) {
            (TransportKind::Cdp, _) => "cdp",
            (_, Some("launch:headless")) => "headless",
            _ => "launch",
        };
        let parse = |text: &str| match text.to_ascii_lowercase().as_str() {
            "launch" | "headed" => Ok("launch"),
            "headless" | "launch-headless" | "launch:headless" => Ok("headless"),
            "cdp" | "connect" => Ok("cdp"),
            _ => Err("expected launch, headless or cdp".to_string()),
        };
        match self
            .ui
            .ask_value(
                "Web mode (launch/headless/cdp)",
                Some(&default),
                parse,
                |v| (*v).to_string(),
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(mode) => {
                match mode {
                    "headless" => {
                        self.answers.connection.transport = TransportKind::Launch;
                        self.answers.connection.endpoint = Some("launch:headless".to_string());
                    }
                    "cdp" => {
                        self.answers.connection.transport = TransportKind::Cdp;
                        self.answers.connection.endpoint = None;
                    }
                    _ => {
                        self.answers.connection.transport = TransportKind::Launch;
                        self.answers.connection.endpoint = None;
                    }
                }
                Ok(Step::Next(if mode == "cdp" {
                    Stage::WebEndpoint
                } else {
                    Stage::WebPageMatch
                }))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn web_endpoint(&mut self) -> Result<Step, String> {
        let default = self.answers.connection.endpoint.clone();
        let parse = |text: &str| {
            if !(text.starts_with("ws://") || text.starts_with("wss://")) {
                return Err("expected a ws:// or wss:// DevTools endpoint".to_string());
            }
            config::validate_endpoint(Platform::Web, text).map(|()| text.to_string())
        };
        match self
            .ui
            .ask_value(
                "DevTools endpoint (ws://...)",
                default.as_ref(),
                parse,
                Clone::clone,
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(endpoint) => {
                self.answers.connection.endpoint = Some(endpoint);
                Ok(Step::Next(Stage::WebPageMatch))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn web_page_match(&mut self) -> Result<Step, String> {
        let default = self.answers.options.page_match.clone();
        match self
            .ui
            .ask_optional("Page title/URL match (- to clear)", default.as_ref())
            .map_err(|error| error.to_string())?
        {
            Answer::Value(value) => {
                self.answers.options.page_match = value;
                Ok(Step::Next(Stage::ConnectorAddress))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn connector_address(&mut self) -> Result<Step, String> {
        let default = self.answers.options.connector_address.clone();
        let parse = |text: &str| {
            if text == "-" {
                Ok(None)
            } else if is_host_port(text) {
                Ok(Some(text.to_string()))
            } else {
                Err("expected HOST:PORT, or - for the default".to_string())
            }
        };
        let default_value = default.clone();
        match self
            .ui
            .ask_value(
                "Connector address (HOST:PORT, - for default)",
                Some(&default_value),
                parse,
                |value: &Option<String>| value.clone().unwrap_or_else(|| "none".to_string()),
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(value) => {
                self.answers.options.connector_address = value;
                Ok(Step::Next(Stage::AgentPort))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn agent_port(&mut self) -> Result<Step, String> {
        let default = self.answers.options.agent_port;
        let parse = |text: &str| {
            if text == "-" {
                return Ok(None);
            }
            text.parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .map(Some)
                .ok_or_else(|| {
                    "expected a port between 1 and 65535, or - for automatic".to_string()
                })
        };
        let default_value = default;
        match self
            .ui
            .ask_value(
                "Agent port (Enter for automatic, - for automatic)",
                Some(&default_value),
                parse,
                |value: &Option<u16>| {
                    value.map_or_else(|| "automatic".to_string(), |port| port.to_string())
                },
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(port) => {
                self.answers.options.agent_port = port;
                Ok(Step::Next(Stage::Labels))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn labels(&mut self) -> Result<Step, String> {
        let default = self.answers.labels.clone();
        let parse = |text: &str| {
            if text == "-" {
                return Ok(Vec::new());
            }
            let mut labels = Vec::new();
            let mut seen = HashSet::new();
            for label in text.split(',').map(str::trim) {
                if label.is_empty() {
                    continue;
                }
                if !valid_label(label) {
                    return Err(format!(
                        "invalid label '{label}'; expected ^[a-z0-9][a-z0-9_-]{{0,31}}$"
                    ));
                }
                if !seen.insert(label.to_string()) {
                    return Err(format!("duplicate label '{label}'"));
                }
                labels.push(label.to_string());
            }
            Ok(labels)
        };
        let default_value = default;
        match self
            .ui
            .ask_value(
                "Labels (comma separated, - to clear)",
                Some(&default_value),
                parse,
                |labels: &Vec<String>| {
                    if labels.is_empty() {
                        "none".to_string()
                    } else {
                        labels.join(",")
                    }
                },
            )
            .map_err(|error| error.to_string())?
        {
            Answer::Value(labels) => {
                self.answers.labels = labels;
                Ok(Step::Next(if self.args.no_check {
                    Stage::Review
                } else {
                    Stage::Check
                }))
            }
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn check(&mut self) -> Result<Step, String> {
        let platform = self
            .answers
            .platform
            .ok_or_else(|| "platform was not selected".to_string())?;
        match platform {
            Platform::Android | Platform::AndroidTv => {
                match self
                    .ui
                    .confirm("Check the connection now?", true)
                    .map_err(|error| error.to_string())?
                {
                    Answer::Value(true) => {}
                    Answer::Value(false) => return Ok(Step::Next(Stage::Review)),
                    Answer::Back => return Ok(Step::Back),
                    Answer::Cancel => return Ok(self.cancel()),
                }
                let Some(device) = self.answers.device() else {
                    return Ok(Step::Next(Stage::Review));
                };
                let target = match AdbTarget::for_device(&device) {
                    Ok(target) => target,
                    Err(error) => {
                        let _ = self.ui.say(format!("connection check: {error}"));
                        return Ok(Step::Next(Stage::Review));
                    }
                };
                let client = AdbClient::new(target, self.home);
                match self.runtime.block_on(client.probe()) {
                    Ok(probe) => {
                        let _ = self.ui.say(format!(
                            "connection ok: {} {} · Android {} · {} ({})",
                            probe.manufacturer,
                            probe.model,
                            probe.release,
                            probe.form_factor(),
                            device.target_endpoint()
                        ));
                        let role_ok = if platform == Platform::AndroidTv {
                            probe.tv
                        } else {
                            !probe.tv
                        };
                        if !role_ok {
                            let suggested = if probe.tv {
                                Platform::AndroidTv
                            } else {
                                Platform::Android
                            };
                            let _ = self.ui.say(format!(
                                "warning: the device reports '{}' but the definition says '{}'",
                                probe.form_factor(),
                                platform
                            ));
                            match self
                                .ui
                                .confirm(
                                    &format!("Save it as {} instead?", suggested.as_str()),
                                    true,
                                )
                                .map_err(|error| error.to_string())?
                            {
                                Answer::Value(true) => {
                                    self.answers.platform = Some(suggested);
                                }
                                Answer::Value(false) => {}
                                Answer::Back => return Ok(Step::Back),
                                Answer::Cancel => return Ok(self.cancel()),
                            }
                        }
                        Ok(Step::Next(Stage::Review))
                    }
                    Err(error) => {
                        let _ = self.ui.say(format!("connection check failed: {error}"));
                        let _ = self.ui.say(
                            "if the device is showing an 'Allow USB debugging?' prompt, accept it \
                             (and remember this computer), then retry",
                        );
                        match self
                            .ui
                            .confirm("Continue and save the definition anyway?", false)
                            .map_err(|error| error.to_string())?
                        {
                            Answer::Value(true) => Ok(Step::Next(Stage::Review)),
                            Answer::Value(false) => Ok(Step::Next(Stage::AndroidRoute)),
                            Answer::Back => Ok(Step::Back),
                            Answer::Cancel => Ok(self.cancel()),
                        }
                    }
                }
            }
            Platform::Tvos => {
                let _ = self.ui.say(
                    "tvOS is not probed while writing the definition. After saving, finish the \
                     machine-global setup (`impresario apple pairing/signing/products/ddi`) and \
                     run `impresario device check <name>`; it signs, installs and starts the \
                     agent over the pure-Rust path.",
                );
                if let Some(endpoint) = self.answers.connection.endpoint.as_deref() {
                    let _ = self
                        .ui
                        .say(format!("configured agent endpoint: {endpoint}"));
                }
                Ok(Step::Next(Stage::Review))
            }
            Platform::Web => {
                let browser = tools::find_chromium();
                let connector = impresario_clients::connector_binary();
                match (&browser, &connector) {
                    (Some(browser), Some(connector)) => {
                        let _ = self.ui.say(format!(
                            "web check ok: browser {}; connector {}",
                            browser.display(),
                            connector.display()
                        ));
                    }
                    _ => {
                        if browser.is_none() {
                            let _ = self.ui.say(
                                "web check: no Chromium found; run `impresario tools install \
                                 chromium` or set CHROME_BIN",
                            );
                        }
                        if connector.is_none() {
                            let _ = self.ui.say(
                                "web check: impresario-connector is not installed next to the CLI",
                            );
                        }
                    }
                }
                Ok(Step::Next(Stage::Review))
            }
            _ => Ok(Step::Next(Stage::Review)),
        }
    }

    fn review(&mut self) -> Result<Step, String> {
        let Some(device) = self.answers.device() else {
            return Ok(Step::Next(Stage::Name));
        };
        let path = definitions::device_layer_path(
            self.home,
            self.workspace_path,
            self.answers.scope_workspace,
        );
        let preview = match serde_yaml_ng::to_string(&config::DevicesConfig {
            devices: vec![device.clone()],
            groups: std::collections::BTreeMap::new(),
        }) {
            Ok(preview) => preview,
            Err(error) => {
                let _ = self.ui.say(format!("cannot render the preview: {error}"));
                return Ok(Step::Next(Stage::Labels));
            }
        };
        let outcome = config::parse_devices(&preview, &path);
        if outcome.diagnostics.has_errors() {
            let _ = self.ui.say("the definition is not valid:");
            for diagnostic in outcome.diagnostics.iter() {
                let _ = self.ui.say(format!("  {diagnostic}"));
            }
            return Ok(Step::Next(Stage::Labels));
        }
        for diagnostic in outcome.diagnostics.iter() {
            let _ = self.ui.say(format!("warning: {diagnostic}"));
        }
        let _ = self.ui.say("");
        let _ = self.ui.say(format!(
            "scope: {} ({})",
            if self.answers.scope_workspace {
                "workspace"
            } else {
                "global"
            },
            path.display()
        ));
        if self.original_name.is_some() && self.global_has_edit && self.workspace_has_edit {
            let _ = self.ui.say(
                "note: this name exists in both layers; the workspace definition wins at run time",
            );
        }
        let _ = self.ui.say("");
        let _ = self.ui.say(&preview);
        let parse = |text: &str| match text.to_ascii_lowercase().as_str() {
            "y" | "yes" => Ok(true),
            "n" | "no" => Ok(false),
            _ => Err("answer y (write), n (discard) or b (back)".to_string()),
        };
        match self
            .ui
            .ask_value("Write this definition?", Some(&true), parse, |value| {
                if *value { "Y/n" } else { "y/N" }.to_string()
            })
            .map_err(|error| error.to_string())?
        {
            Answer::Value(true) => match definitions::save_device(
                self.home,
                self.workspace_path,
                self.answers.scope_workspace,
                self.original_name.is_some(),
                &device,
            ) {
                Ok(path) => {
                    let _ = self.ui.say(format!(
                        "{} device '{}' in {}",
                        if self.original_name.is_some() {
                            "updated"
                        } else {
                            "added"
                        },
                        device.name,
                        path.display()
                    ));
                    let next = match device.platform {
                        Platform::Tvos => {
                            "next: bind an app with `impresario app add ...`; after the \
                             machine-global Apple setup, run `impresario device check <name>`"
                        }
                        _ => {
                            "next: bind an app with `impresario app add ...` and run `impresario device check`"
                        }
                    };
                    let _ = self.ui.say(next);
                    Ok(Step::Done(EXIT_OK))
                }
                Err(error) => {
                    let _ = self.ui.say(format!("not written: {error}"));
                    Ok(Step::Next(Stage::Name))
                }
            },
            Answer::Value(false) => Ok(self.cancel()),
            Answer::Back => Ok(Step::Back),
            Answer::Cancel => Ok(self.cancel()),
        }
    }

    fn cancel(&mut self) -> Step {
        let _ = self.ui.say("cancelled; no changes were written");
        Step::Done(EXIT_OK)
    }
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Next(Stage),
    Back,
    Done(i32),
}

fn previous_stage(stage: Stage, answers: &Answers) -> Option<Stage> {
    let platform = answers.platform.unwrap_or(Platform::Android);
    Some(match stage {
        Stage::Scope => return None,
        Stage::Platform => Stage::Scope,
        Stage::Name => Stage::Platform,
        Stage::AndroidRoute => Stage::Name,
        Stage::AndroidManual => Stage::AndroidRoute,
        Stage::AndroidPairTarget => Stage::AndroidRoute,
        Stage::AndroidPairCode => Stage::AndroidPairTarget,
        Stage::AndroidAfterPair => Stage::AndroidRoute,
        Stage::TvosUdid => Stage::Name,
        Stage::TvosEndpoint => Stage::TvosUdid,
        Stage::WebMode => Stage::Name,
        Stage::WebEndpoint => Stage::WebMode,
        Stage::WebPageMatch => {
            if answers.connection.transport == TransportKind::Cdp {
                Stage::WebEndpoint
            } else {
                Stage::WebMode
            }
        }
        Stage::ConnectorAddress => Stage::WebPageMatch,
        Stage::AgentPort => match platform {
            Platform::Web => Stage::ConnectorAddress,
            _ => Stage::AndroidRoute,
        },
        Stage::Labels => Stage::AgentPort,
        Stage::Check => Stage::Labels,
        Stage::Review => Stage::Labels,
    })
}

fn discover() -> Discovery {
    let mut discovery = Discovery::default();
    let mut seen = HashSet::new();
    match adb::discover_usb() {
        Ok(devices) => {
            let mut found = 0usize;
            for device in devices.iter().filter(|device| device.adb_interface) {
                found += 1;
                let label = format!("USB {} ({})", product_label(device), device.serial);
                discovery.routes.push(RouteOption {
                    label,
                    connection: DeviceConnection {
                        transport: TransportKind::Usb,
                        serial: Some(device.serial.clone()),
                        ..DeviceConnection::default()
                    },
                });
            }
            if found == 0 {
                discovery
                    .notes
                    .push("no USB device exposes the ADB interface".to_string());
            }
        }
        Err(error) => discovery
            .notes
            .push(format!("USB discovery failed: {error}")),
    }
    match adb::discover_mdns(Duration::from_secs(5)) {
        Ok(devices) => {
            let mut found = 0usize;
            for device in devices {
                found += 1;
                match device.kind.as_str() {
                    "pairing" => discovery.pairing.push((device.host, device.port)),
                    "wireless" | "classic"
                        if seen.insert(format!("{}:{}", device.host, device.port)) =>
                    {
                        discovery.routes.push(RouteOption {
                            label: format!(
                                "{} {}:{} ({})",
                                if device.kind == "wireless" {
                                    "wireless"
                                } else {
                                    "classic TCP"
                                },
                                device.host,
                                device.port,
                                short_mdns_name(&device.name)
                            ),
                            connection: DeviceConnection {
                                transport: TransportKind::Tcp,
                                host: Some(device.host),
                                port: Some(device.port),
                                ..DeviceConnection::default()
                            },
                        });
                    }
                    _ => {}
                }
            }
            if found == 0 {
                discovery
                    .notes
                    .push("no wireless-debugging service advertised over mDNS".to_string());
            }
        }
        Err(error) => discovery
            .notes
            .push(format!("mDNS discovery failed: {error}")),
    }
    discovery
}

fn short_mdns_name(name: &str) -> String {
    name.split("._adb").next().unwrap_or(name).to_string()
}

#[derive(Debug, Clone)]
enum AndroidAddress {
    Tcp { host: String, port: u16 },
    Usb { serial: String },
}

impl AndroidAddress {
    fn connection(&self) -> DeviceConnection {
        match self {
            AndroidAddress::Tcp { host, port } => DeviceConnection {
                transport: TransportKind::Tcp,
                host: Some(host.clone()),
                port: Some(*port),
                ..DeviceConnection::default()
            },
            AndroidAddress::Usb { serial } => DeviceConnection {
                transport: TransportKind::Usb,
                serial: Some(serial.clone()),
                ..DeviceConnection::default()
            },
        }
    }

    fn display(&self) -> String {
        match self {
            AndroidAddress::Tcp { host, port } => format!("{host}:{port}"),
            AndroidAddress::Usb { serial } => format!("usb:{serial}"),
        }
    }
}

fn parse_android_address(text: &str) -> Result<AndroidAddress, &'static str> {
    let text = text.trim();
    if let Some(serial) = text.strip_prefix("usb:") {
        if serial.is_empty() || serial.contains(char::is_whitespace) {
            return Err("expected a USB serial after usb:");
        }
        return Ok(AndroidAddress::Usb {
            serial: serial.to_string(),
        });
    }
    if let Some((host, port)) = text.rsplit_once(':') {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() || port.is_empty() {
            return Err("expected HOST:PORT");
        }
        let port: u16 = port.parse().map_err(|_| "invalid port")?;
        if port == 0 {
            return Err("the port must be between 1 and 65535");
        }
        return Ok(AndroidAddress::Tcp {
            host: host.to_string(),
            port,
        });
    }
    if !text.is_empty() && !text.contains(char::is_whitespace) {
        return Ok(AndroidAddress::Tcp {
            host: text.to_string(),
            port: adb::DEFAULT_TCP_PORT,
        });
    }
    Err("expected HOST:PORT, a host (default port 5555), or usb:SERIAL")
}

fn parse_tvos_udid(text: &str) -> Result<String, String> {
    let text = text.trim();
    if config::simulator_udid(text) {
        return Err(
            "a simulator UUID is not a physical Apple TV; simulator execution is out of scope"
                .to_string(),
        );
    }
    if !config::valid_udid(text) {
        return Err("expected the physical Apple TV UDID (for example \
             0123456789abcdef0123456789abcdef01234567 or \
             00008110-001B3456021E002F)"
            .to_string());
    }
    Ok(text.to_string())
}

fn parse_tvos_endpoint(text: &str) -> Result<String, String> {
    let text = text.trim();
    if !is_host_port(text) {
        return Err(
            "expected the Apple TV's LAN HOST:PORT for Impresario Agent (usually :50062)"
                .to_string(),
        );
    }
    Ok(text.to_string())
}

fn manual_default(connection: &DeviceConnection) -> Option<String> {
    match connection.transport {
        TransportKind::Usb => connection
            .serial
            .as_ref()
            .map(|serial| format!("usb:{serial}")),
        TransportKind::Tcp | TransportKind::Network => {
            connection.host.as_ref().map(|host| match connection.port {
                Some(port) => format!("{host}:{port}"),
                None => host.clone(),
            })
        }
        _ => None,
    }
}

fn default_name(platform: Platform, edit: Option<&DeviceConfig>) -> String {
    if let Some(device) = edit {
        return device.name.clone();
    }
    match platform {
        Platform::Android => "android".to_string(),
        Platform::AndroidTv => "android_tv".to_string(),
        Platform::Web => "web".to_string(),
        Platform::Tvos => "apple_tv".to_string(),
        _ => "device".to_string(),
    }
}

fn is_host_port(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    !host.is_empty()
        && !host.contains(char::is_whitespace)
        && port.parse::<u16>().is_ok_and(|port| port > 0)
}

fn valid_label(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    let mut count = 1usize;
    for character in chars {
        if !(character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-')
        {
            return false;
        }
        count += 1;
    }
    count <= 32
}

fn layer_has(path: &Path, name: &str) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let outcome = config::parse_devices(&source, path);
    if outcome.diagnostics.has_errors() {
        return Err(format!(
            "{} has configuration errors; run `impresario device check`",
            path.display()
        ));
    }
    Ok(outcome
        .config
        .is_some_and(|config| config.devices.iter().any(|device| device.name == name)))
}

/// Entry point used by the CLI (stdin/stdout).
pub fn run(args: &WizardArgs, home: &Home, workspace_path: &Path) -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    run_wizard_io(args, home, workspace_path, stdin.lock(), &mut writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = Home::at(dir.path().join("home"));
        (dir, home)
    }

    fn run_script(args: &WizardArgs, home: &Home, workspace: &Path, input: &str) -> i32 {
        let mut output = Vec::new();
        run_wizard_io(
            args,
            home,
            workspace,
            Cursor::new(input.as_bytes()),
            &mut output,
        )
    }

    #[test]
    fn parses_android_addresses() {
        let tcp = parse_android_address("10.0.0.5:5555").expect("tcp");
        assert!(matches!(tcp, AndroidAddress::Tcp { port: 5555, .. }));
        let default_port = parse_android_address("10.0.0.5").expect("default port");
        assert!(matches!(
            default_port,
            AndroidAddress::Tcp { port: 5555, .. }
        ));
        let usb = parse_android_address("usb:RMX3997").expect("usb");
        assert!(matches!(usb, AndroidAddress::Usb { .. }));
        assert!(parse_android_address("usb:").is_err());
        assert!(parse_android_address("host:0").is_err());
    }

    #[test]
    fn parses_physical_tvos_udids_and_agent_endpoints() {
        assert_eq!(
            parse_tvos_udid("00008110-001B3456021E002F").expect("physical udid"),
            "00008110-001B3456021E002F"
        );
        assert!(parse_tvos_udid("9E3C7A6D-0B4B-4F1A-9C7E-2A6D5B8F4E1C").is_err());
        assert!(parse_tvos_udid("nope").is_err());
        assert_eq!(
            parse_tvos_endpoint("192.168.93.10:50062").expect("endpoint"),
            "192.168.93.10:50062"
        );
        assert!(parse_tvos_endpoint("192.168.93.19").is_err());
        assert!(parse_tvos_endpoint("host:0").is_err());
    }

    #[test]
    fn cancelling_does_not_write() {
        let (dir, home) = test_home();
        let workspace = dir.path().join("impresario.devices.config");
        let args = WizardArgs {
            edit: None,
            workspace: false,
            no_check: true,
        };
        let code = run_script(&args, &home, &workspace, "global\nandroid\nphone\nq\n");
        assert_eq!(code, EXIT_OK);
        assert!(!home.devices_path().exists());
        assert!(!workspace.exists());
    }

    #[test]
    fn scripted_android_add_writes_a_valid_definition() {
        let (dir, home) = test_home();
        let workspace = dir.path().join("impresario.devices.config");
        let args = WizardArgs {
            edit: None,
            workspace: true,
            no_check: true,
        };
        let input = "workspace\nandroid\nphone\nm\n10.0.0.5:5555\n-\n-\ny\n";
        let code = run_script(&args, &home, &workspace, input);
        assert_eq!(code, EXIT_OK);
        let source = std::fs::read_to_string(&workspace).expect("workspace devices");
        assert!(source.contains("name: phone"));
        assert!(source.contains("host: 10.0.0.5"));
        assert!(source.contains("port: 5555"));
        assert!(source.contains("transport: tcp"));
    }

    #[test]
    fn scripted_tvos_add_writes_a_valid_definition() {
        let (dir, home) = test_home();
        let workspace = dir.path().join("impresario.devices.config");
        let args = WizardArgs {
            edit: None,
            workspace: true,
            no_check: true,
        };
        let input = "workspace\ntvos\napple_tv\n00008110-001B3456021E002F\n\
                     192.168.93.10:50062\n-\ny\n";
        let code = run_script(&args, &home, &workspace, input);
        assert_eq!(code, EXIT_OK);
        let source = std::fs::read_to_string(&workspace).expect("workspace devices");
        assert!(source.contains("name: apple_tv"));
        assert!(source.contains("platform: tvos"));
        assert!(source.contains("udid: 00008110-001B3456021E002F"));
        assert!(source.contains("transport: network"));
        assert!(source.contains("endpoint: 192.168.93.10:50062"));
    }

    #[test]
    fn back_navigation_reaches_an_earlier_step_without_writing() {
        let (dir, home) = test_home();
        let workspace = dir.path().join("impresario.devices.config");
        let args = WizardArgs {
            edit: None,
            workspace: true,
            no_check: true,
        };
        // Name -> back to platform; switch to web and finish.
        let input = "workspace\nandroid\nb\nweb\nweb\nlaunch\n-\n-\n-\n-\ny\n";
        let code = run_script(&args, &home, &workspace, input);
        assert_eq!(code, EXIT_OK);
        let source = std::fs::read_to_string(&workspace).expect("workspace devices");
        assert!(source.contains("name: web"));
        assert!(!source.contains("platform: android"));
    }

    #[test]
    fn scripted_edit_replaces_one_device() {
        let (dir, home) = test_home();
        let workspace = dir.path().join("impresario.devices.config");
        std::fs::write(
            &workspace,
            "devices:\n- name: phone\n  platform: android\n  connection:\n    transport: usb\n    serial: OLD\n",
        )
        .expect("seed workspace");
        let args = WizardArgs {
            edit: Some("phone".to_string()),
            workspace: false,
            no_check: true,
        };
        let input = "\n\n\nm\n10.0.0.9:5555\n-\n-\ny\n";
        let code = run_script(&args, &home, &workspace, input);
        assert_eq!(code, EXIT_OK);
        let source = std::fs::read_to_string(&workspace).expect("workspace devices");
        assert_eq!(source.matches("name: phone").count(), 1);
        assert!(source.contains("host: 10.0.0.9"));
        assert!(!source.contains("serial: OLD"));
    }

    #[test]
    fn scripted_web_add_writes_a_launch_device() {
        let (dir, home) = test_home();
        let workspace = dir.path().join("impresario.devices.config");
        let args = WizardArgs {
            edit: None,
            workspace: false,
            no_check: true,
        };
        let input = "global\nweb\nweb\nlaunch\n-\n-\n-\n-\ny\n";
        let code = run_script(&args, &home, &workspace, input);
        assert_eq!(code, EXIT_OK);
        let source = std::fs::read_to_string(home.devices_path()).expect("global devices");
        assert!(source.contains("name: web"));
        assert!(source.contains("platform: web"));
        assert!(source.contains("transport: launch"));
    }
}
