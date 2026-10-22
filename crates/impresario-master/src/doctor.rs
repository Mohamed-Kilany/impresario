//! `impresario doctor` (TASK-2 section 6.6).
//!
//! Reports, per platform, what a run needs, what was found, which rung of the
//! dependency ladder provides it, and the exact fix for anything missing.
//! Doctor never installs, downloads or modifies a device: managed downloads
//! stay behind `impresario tools install` and its explicit consent prompt.

use std::path::{Path, PathBuf};
use std::time::Duration;

use impresario_clients::adb::{self, AdbClient, AdbTarget};
use impresario_core::config::{self, DeviceConfig};
use impresario_core::home::Home;
use impresario_core::platforms::Platform;
use impresario_core::tools;
use serde::Serialize;

use crate::cli::{load_effective_config, EXIT_INFRA, EXIT_OK, EXIT_USAGE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ready,
    Degraded,
    Unavailable,
    Planned,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ready => "READY",
            Status::Degraded => "DEGRADED",
            Status::Unavailable => "UNAVAILABLE",
            Status::Planned => "PLANNED",
        }
    }
}

#[derive(Debug, Serialize)]
struct DeviceReport {
    name: String,
    status: Status,
    detail: String,
}

#[derive(Debug, Serialize)]
struct PlatformReport {
    platform: String,
    status: Status,
    needed: Vec<String>,
    found: Vec<String>,
    rung: String,
    fixes: Vec<String>,
    devices: Vec<DeviceReport>,
}

impl PlatformReport {
    fn new(platform: Platform) -> Self {
        Self {
            platform: platform.as_str().to_string(),
            status: Status::Ready,
            needed: Vec::new(),
            found: Vec::new(),
            rung: String::new(),
            fixes: Vec::new(),
            devices: Vec::new(),
        }
    }

    fn fail(&mut self) {
        self.status = Status::Unavailable;
    }

    fn degrade(&mut self) {
        if self.status == Status::Ready {
            self.status = Status::Degraded;
        }
    }
}

#[derive(Debug, Serialize)]
struct ToolReport {
    name: String,
    status: Status,
    detail: String,
    fix: Option<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    ok: bool,
    home: String,
    workspace: String,
    platforms: Vec<PlatformReport>,
    tools: Vec<ToolReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    config_errors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    device_errors: Vec<String>,
}

/// Execute doctor and return a process exit code.
///
/// Doctor is intentionally resilient: a broken workspace config or a broken
/// device file is a finding, not a reason to stop before probing the toolchain.
pub fn run(home: &Home, base: &Path, config_path: &Path, devices_path: &Path, json: bool) -> i32 {
    let config_outcome = load_effective_config(base, config_path);
    let mut config_errors = Vec::new();
    if config_outcome.diagnostics.has_errors() {
        crate::render::eprint_all(&config_outcome.diagnostics);
        config_errors.extend(
            config_outcome
                .diagnostics
                .errors()
                .map(|diagnostic| diagnostic.one_line()),
        );
        eprintln!("doctor: configuration errors in {}", config_path.display());
    }
    let outcome = config::load_devices_merged(devices_path);
    let mut device_errors = Vec::new();
    if outcome.diagnostics.has_errors() {
        crate::render::eprint_all(&outcome.diagnostics);
        device_errors.extend(
            outcome
                .diagnostics
                .errors()
                .map(|diagnostic| diagnostic.one_line()),
        );
        eprintln!("doctor: configuration errors in {}", devices_path.display());
    }
    let devices = outcome
        .config
        .map_or_else(Vec::new, |config| config.devices);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("doctor: cannot start the async runtime: {error}");
            return EXIT_INFRA;
        }
    };
    let mut report = runtime.block_on(collect(home, base, &devices));
    report.config_errors = config_errors;
    report.device_errors = device_errors;
    if !report.config_errors.is_empty() || !report.device_errors.is_empty() {
        report.ok = false;
    }

    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("doctor: cannot render JSON: {error}");
                return EXIT_INFRA;
            }
        }
    } else {
        print_human(&report);
    }
    if !report.config_errors.is_empty() || !report.device_errors.is_empty() {
        EXIT_USAGE
    } else if report.ok {
        EXIT_OK
    } else {
        EXIT_INFRA
    }
}

async fn collect(home: &Home, base: &Path, devices: &[DeviceConfig]) -> Report {
    let mut platforms = Vec::new();
    let android = collect_android(home, devices, Platform::Android).await;
    let android_tv = collect_android(home, devices, Platform::AndroidTv).await;
    let web = collect_web(home, devices);
    platforms.push(android);
    platforms.push(android_tv);
    platforms.push(web);
    platforms.push(collect_planned(Platform::WebOs, devices));
    platforms.push(collect_planned(Platform::Tizen, devices));
    platforms.push(collect_planned(Platform::Ios, devices));
    platforms.push(collect_planned(Platform::Tvos, devices));

    let tools_report = collect_tools(home);
    let ok = platforms
        .iter()
        .all(|platform| !matches!(platform.status, Status::Unavailable))
        && tools_report
            .iter()
            .all(|tool| !matches!(tool.status, Status::Unavailable));

    Report {
        ok,
        home: home.root().display().to_string(),
        workspace: base.display().to_string(),
        platforms,
        tools: tools_report,
        config_errors: Vec::new(),
        device_errors: Vec::new(),
    }
}

async fn collect_android(
    home: &Home,
    devices: &[DeviceConfig],
    platform: Platform,
) -> PlatformReport {
    let mut report = PlatformReport::new(platform);
    report.rung = "rung 1: pure-Rust libadb 0.6.0 (TCP, USB, TLS, SPAKE2 pairing)".to_string();
    report.needed = vec![
        "native ADB transport (no adb binary)".to_string(),
        "host RSA key under ~/.impresario/keys".to_string(),
        "reachable device over TCP/wireless debugging or USB".to_string(),
    ];
    report.found.push(format!(
        "libadb {} (compiled in)",
        libadb_version().unwrap_or("0.6.x")
    ));
    let key = home.keys_dir().join("adbkey");
    report.found.push(if key.is_file() {
        format!("host key: {}", key.display())
    } else {
        "host key: absent; generated on first connection (confirm the device prompt)".to_string()
    });

    match adb::discover_usb() {
        Ok(found) => {
            let adb_devices: Vec<_> = found.iter().filter(|device| device.adb_interface).collect();
            if adb_devices.is_empty() {
                report
                    .found
                    .push("USB: no device exposing the ADB interface".to_string());
            } else {
                for device in &adb_devices {
                    let driver = device.driver.as_deref().unwrap_or("unknown driver");
                    report.found.push(format!(
                        "USB: {} ({:04x}:{:04x}, {driver})",
                        device.product, device.vendor_id, device.product_id
                    ));
                    #[cfg(target_os = "windows")]
                    if !driver.eq_ignore_ascii_case("WinUSB") {
                        report.degrade();
                        report.fixes.push(format!(
                            "USB device {} is not bound to WinUSB; bind it with the OEM/WinUSB \
                             driver, or use wireless debugging (recommended)",
                            device.serial
                        ));
                    }
                }
            }
        }
        Err(error) => {
            report.degraded_finding(format!("USB discovery unavailable: {error}"));
        }
    }

    match adb::discover_mdns(Duration::from_millis(1200)) {
        Ok(found) if found.is_empty() => {
            report
                .found
                .push("mDNS: no _adb-tls-connect/_adb services advertised".to_string());
        }
        Ok(found) => {
            for service in found {
                report.found.push(format!(
                    "mDNS: {} at {}:{} ({})",
                    service.name, service.host, service.port, service.kind
                ));
            }
        }
        Err(error) => report.degraded_finding(format!("mDNS discovery unavailable: {error}")),
    }

    let mut configured = 0usize;
    for device in devices.iter().filter(|device| device.platform == platform) {
        configured += 1;
        let target = match AdbTarget::for_device(device) {
            Ok(target) => target,
            Err(error) => {
                report.devices.push(DeviceReport {
                    name: device.name.clone(),
                    status: Status::Unavailable,
                    detail: error.to_string(),
                });
                report.fail();
                report.fixes.push(format!(
                    "device '{}': fix its connection fields",
                    device.name
                ));
                continue;
            }
        };
        let client = AdbClient::new(target, home);
        match client.probe().await {
            Ok(probe) => {
                let role_ok = match platform {
                    Platform::AndroidTv => probe.tv,
                    _ => !probe.tv,
                };
                if !role_ok {
                    report.degrade();
                    report.fixes.push(format!(
                        "device '{}' is configured as {} but reports {}; change platform or role",
                        device.name,
                        platform,
                        probe.form_factor()
                    ));
                }
                report.devices.push(DeviceReport {
                    name: device.name.clone(),
                    status: Status::Ready,
                    detail: format!(
                        "{} {} · Android {} · {} via {}",
                        probe.manufacturer,
                        probe.model,
                        probe.release,
                        probe.form_factor(),
                        client.target().label()
                    ),
                });
            }
            Err(error) => {
                report.devices.push(DeviceReport {
                    name: device.name.clone(),
                    status: Status::Unavailable,
                    detail: error.to_string(),
                });
                report.fail();
                report.fixes.push(format!(
                    "device '{}': check that wireless debugging is on and the host key is \
                     authorized, or run `impresario device check {}`",
                    device.name, device.name
                ));
            }
        }
    }
    if configured == 0 {
        report.found.push(
            "no device of this platform configured; discovery and connection are available"
                .to_string(),
        );
    }
    report
}

fn collect_web(home: &Home, devices: &[DeviceConfig]) -> PlatformReport {
    let mut report = PlatformReport::new(Platform::Web);
    report.rung =
        "rung 1: native CDP connector; browser rung 1 (system) or rung 3 (managed Chromium)"
            .to_string();
    report.needed = vec![
        "impresario-connector executable".to_string(),
        "a Chromium-family browser or the managed Chrome for Testing build".to_string(),
    ];
    match impresario_clients::connector_binary() {
        Some(path) => report.found.push(format!("connector: {}", path.display())),
        None => {
            report.fail();
            report
                .fixes
                .push("install the release archive (it ships impresario-connector), or build with `cargo build -p impresario-master -p impresario-connector`".to_string());
        }
    }
    match tools::find_chromium() {
        Some(path) => report.found.push(format!(
            "browser: {} ({})",
            path.display(),
            tools::chromium_source(home, &path)
        )),
        None => {
            report.fail();
            report.fixes.push(
                "run `impresario tools install chromium` (pinned Chrome for Testing, checksum verified), or set CHROME_BIN"
                    .to_string(),
            );
        }
    }
    for device in devices
        .iter()
        .filter(|device| device.platform == Platform::Web)
    {
        report.devices.push(DeviceReport {
            name: device.name.clone(),
            status: Status::Ready,
            detail: format!(
                "{} · {}",
                device.effective_transport().as_str(),
                device.target_endpoint()
            ),
        });
    }
    report
}

fn collect_planned(platform: Platform, devices: &[DeviceConfig]) -> PlatformReport {
    let mut report = PlatformReport::new(platform);
    report.rung = "not selected yet: TASK-2 phase 6/7 replaces this transport".to_string();
    report.needed = vec![
        "native transport without ssh/sdb/xcodebuild (D-19)".to_string(),
        "device pairing and signing flow".to_string(),
    ];
    let configured: Vec<_> = devices
        .iter()
        .filter(|device| device.platform == platform)
        .collect();
    if configured.is_empty() {
        report.status = Status::Planned;
        report.found.push(
            "no device of this platform configured; the native transport lands in phase 6/7"
                .to_string(),
        );
        return report;
    }
    report.status = Status::Unavailable;
    for device in configured {
        report.devices.push(DeviceReport {
            name: device.name.clone(),
            status: Status::Unavailable,
            detail: "configured, but the tool-free transport is not implemented in phase 3"
                .to_string(),
        });
    }
    report.fixes.push(
        "this platform is configured but its D-19 transport is phase 6/7 work; impresario will \
         not silently fall back to ssh/sdb/xcodebuild"
            .to_string(),
    );
    report
}

fn collect_tools(home: &Home) -> Vec<ToolReport> {
    let mut tools_report = Vec::new();
    match tools::chromium_artifact() {
        Some(artifact) => match tools::managed_chromium_binary(home) {
            Some(path) => tools_report.push(ToolReport {
                name: "chromium".to_string(),
                status: Status::Ready,
                detail: format!(
                    "Chrome for Testing {} installed at {}",
                    artifact.version,
                    path.display()
                ),
                fix: None,
            }),
            None => tools_report.push(ToolReport {
                name: "chromium".to_string(),
                status: Status::Ready,
                detail: format!(
                    "Chrome for Testing {} available ({} MB, sha256 {}) but not installed",
                    artifact.version,
                    artifact.size_bytes / 1_000_000,
                    artifact.sha256
                ),
                fix: Some(
                    "run `impresario tools install chromium` (asks for consent and verifies the checksum)"
                        .to_string(),
                ),
            }),
        },
        None => tools_report.push(ToolReport {
            name: "chromium".to_string(),
            status: Status::Degraded,
            detail: format!(
                "no managed Chrome for Testing build for this host ({}-{})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
            fix: Some("set CHROME_BIN to a Chromium-family browser".to_string()),
        }),
    }
    tools_report.push(ToolReport {
        name: "android host key".to_string(),
        status: if home.keys_dir().join("adbkey").is_file() {
            Status::Ready
        } else {
            Status::Degraded
        },
        detail: if home.keys_dir().join("adbkey").is_file() {
            format!("{}", home.keys_dir().join("adbkey").display())
        } else {
            "absent; generated on the first connection".to_string()
        },
        fix: None,
    });
    if let Some(path) = find_on_path("adb") {
        tools_report.push(ToolReport {
            name: "adb on PATH".to_string(),
            status: Status::Ready,
            detail: format!("found at {}; not used (D-19)", path.display()),
            fix: None,
        });
    } else {
        tools_report.push(ToolReport {
            name: "adb on PATH".to_string(),
            status: Status::Ready,
            detail: "not present; not needed".to_string(),
            fix: None,
        });
    }
    tools_report
}

fn libadb_version() -> Option<&'static str> {
    // The protocol crate is pinned in Cargo.toml; doctor only needs the
    // version for the human report. Keeping it here avoids a build script.
    Some("0.6.0")
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

impl PlatformReport {
    fn degraded_finding(&mut self, message: String) {
        self.found.push(message);
        self.degrade();
    }
}

fn print_human(report: &Report) {
    println!("impresario doctor");
    println!("home:      {}", report.home);
    println!("workspace: {}", report.workspace);
    if !report.config_errors.is_empty() {
        println!();
        println!("configuration errors:");
        for error in &report.config_errors {
            println!("  {error}");
        }
    }
    if !report.device_errors.is_empty() {
        println!();
        println!("device configuration errors:");
        for error in &report.device_errors {
            println!("  {error}");
        }
    }
    println!();
    for platform in &report.platforms {
        println!(
            "[{}] {} — {}",
            platform.status.label(),
            platform.platform,
            platform.rung
        );
        for needed in &platform.needed {
            println!("  needs: {needed}");
        }
        for found in &platform.found {
            println!("  found: {found}");
        }
        for device in &platform.devices {
            println!(
                "  device {} [{}]: {}",
                device.name,
                match device.status {
                    Status::Ready => "ok",
                    Status::Degraded => "warning",
                    Status::Unavailable => "unavailable",
                    Status::Planned => "planned",
                },
                device.detail
            );
        }
        for fix in &platform.fixes {
            println!("  fix:   {fix}");
        }
        println!();
    }
    println!("tools:");
    for tool in &report.tools {
        println!("  [{}] {}: {}", tool.status.label(), tool.name, tool.detail);
        if let Some(fix) = &tool.fix {
            println!("         fix: {fix}");
        }
    }
    let ready = report
        .platforms
        .iter()
        .filter(|platform| platform.status == Status::Ready)
        .count();
    let degraded = report
        .platforms
        .iter()
        .filter(|platform| platform.status == Status::Degraded)
        .count();
    let unavailable = report
        .platforms
        .iter()
        .filter(|platform| platform.status == Status::Unavailable)
        .count();
    let planned = report
        .platforms
        .iter()
        .filter(|platform| platform.status == Status::Planned)
        .count();
    println!();
    println!(
        "doctor: {ready} ready, {degraded} degraded, {unavailable} unavailable, {planned} planned"
    );
    if report.ok {
        println!("doctor: ok");
    } else {
        println!("doctor: attention needed (see fix lines)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_are_stable() {
        assert_eq!(Status::Ready.label(), "READY");
        assert_eq!(Status::Planned.label(), "PLANNED");
    }

    #[test]
    fn tool_path_lookup_is_explicit() {
        assert!(find_on_path("definitely-not-a-real-tool-name-12345").is_none());
    }
}
