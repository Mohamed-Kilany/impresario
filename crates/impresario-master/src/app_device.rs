//! Device-aware app shortcuts (TASK-2 section 5, T2-R3).
//!
//! `app install`, `app uninstall` and `app check --device` resolve the app's
//! exact platform entry (no selector fallback, like the planner), reject
//! definition problems before hardware is touched, then reuse the transport
//! install path a run uses. No host runtime tool (`adb`, `sdb`, `ares-*`,
//! `ideviceinstaller`) is spawned.
//!
//! Signature checks stay explicit: Tizen verifies the configured signing
//! profile covers the TV's DUID and is unexpired; Android, Apple and webOS
//! report the check as unsupported because no native installed-signature
//! reader exists. Install success is still enforced by the platform install
//! service.

use std::path::{Path, PathBuf};

use impresario_clients::AdbTarget;
use impresario_core::apps::{self, AppConfig, AppsConfig};
use impresario_core::config::{self, DeviceConfig, DevicesConfig};
use impresario_core::home::Home;
use impresario_core::platforms::Platform;

use crate::cli::{select_devices_with_groups, EXIT_INFRA, EXIT_OK, EXIT_USAGE};
use crate::render;

const IMP_APP_UNKNOWN: &str = "IMP-APP-001";
const IMP_APP_PLATFORM: &str = "IMP-APP-002";
const IMP_APP_PACKAGE: &str = "IMP-APP-003";
const IMP_APP_WEB: &str = "IMP-APP-005";
const IMP_APP_NO_PACKAGE: &str = "IMP-APP-006";
const IMP_APP_DEVICE: &str = "IMP-APP-007";

/// Install the app's configured package on each selected device.
pub fn install(
    home: &Home,
    apps_path: &Path,
    devices_path: &Path,
    app_name: &str,
    requested: &[String],
) -> i32 {
    let (apps, devices) = match load_definitions(apps_path, devices_path, "app install") {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let targets = match install_targets(&apps, &devices, app_name, requested, apps_path) {
        Ok(targets) => targets,
        Err(problems) => {
            report("app install", &problems);
            return EXIT_USAGE;
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let results = runtime.block_on(async {
        let mut results = Vec::with_capacity(targets.len());
        for target in &targets {
            results.push(install_one(home, target).await);
        }
        results
    });
    finish("app install", results)
}

/// Uninstall the app from each selected device.
pub fn uninstall(
    home: &Home,
    apps_path: &Path,
    devices_path: &Path,
    app_name: &str,
    requested: &[String],
) -> i32 {
    let (apps, devices) = match load_definitions(apps_path, devices_path, "app uninstall") {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let targets = match target_pairs(&apps, &devices, app_name, requested, apps_path) {
        Ok(targets) => targets,
        Err(problems) => {
            report("app uninstall", &problems);
            return EXIT_USAGE;
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let results = runtime.block_on(async {
        let mut results = Vec::with_capacity(targets.len());
        for target in &targets {
            results.push(uninstall_one(home, target).await);
        }
        results
    });
    finish("app uninstall", results)
}

/// Validate definitions and, with `--device`, the installed state.
pub fn check(
    home: &Home,
    apps_path: &Path,
    devices_path: &Path,
    names: &[String],
    requested: &[String],
) -> i32 {
    let (apps, devices) = match load_definitions(apps_path, devices_path, "app check") {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let targets = match check_targets(&apps, &devices, names, requested, apps_path) {
        Ok(targets) => targets,
        Err(problems) => {
            report("app check", &problems);
            return EXIT_USAGE;
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let results = runtime.block_on(async {
        let mut results = Vec::with_capacity(targets.len());
        for target in &targets {
            results.push(check_one(home, target).await);
        }
        results
    });
    finish("app check", results)
}

// ---------------------------------------------------------------------------
// definitions and targets
// ---------------------------------------------------------------------------

fn load_definitions(
    apps_path: &Path,
    devices_path: &Path,
    command: &str,
) -> Result<(AppsConfig, DevicesConfig), i32> {
    let apps_outcome = apps::load_apps_merged(apps_path);
    render::eprint_all(&apps_outcome.diagnostics);
    let Some(apps) = apps_outcome.config else {
        eprintln!("{command}: configuration errors in {}", apps_path.display());
        return Err(EXIT_USAGE);
    };
    let devices_outcome = config::load_devices_merged(devices_path);
    render::eprint_all(&devices_outcome.diagnostics);
    let Some(devices) = devices_outcome.config else {
        eprintln!(
            "{command}: configuration errors in {}",
            devices_path.display()
        );
        return Err(EXIT_USAGE);
    };
    Ok((apps, devices))
}

/// One app/device pair after the definition checks passed.
#[derive(Debug, Clone)]
struct Target {
    device: DeviceConfig,
    /// The app definition, with `package` replaced by its resolved path when
    /// the transport install helper reads it from the definition.
    app: AppConfig,
    app_id: String,
    package: Option<PathBuf>,
}

fn install_targets(
    apps: &AppsConfig,
    devices: &DevicesConfig,
    app_name: &str,
    requested: &[String],
    apps_path: &Path,
) -> Result<Vec<Target>, Vec<String>> {
    let app = find_app(apps, app_name);
    let selected = select_devices(devices, requested);
    let mut targets = Vec::new();
    let mut problems = Vec::new();
    let app = match app {
        Ok(app) => app,
        Err(problem) => return Err(vec![problem]),
    };
    let selected = match selected {
        Ok(selected) => selected,
        Err(problem) => return Err(vec![problem]),
    };
    let base = workspace_base(apps_path);
    for device in &selected {
        match install_target(app, device, &base) {
            Ok(target) => targets.push(target),
            Err(problem) => problems.push(problem),
        }
    }
    if problems.is_empty() {
        Ok(targets)
    } else {
        Err(problems)
    }
}

fn check_targets(
    apps: &AppsConfig,
    devices: &DevicesConfig,
    names: &[String],
    requested: &[String],
    apps_path: &Path,
) -> Result<Vec<Target>, Vec<String>> {
    let selected_apps = match select_apps(apps, names) {
        Ok(selected) => selected,
        Err(problem) => return Err(vec![problem]),
    };
    let selected_devices = match select_devices(devices, requested) {
        Ok(selected) => selected,
        Err(problem) => return Err(vec![problem]),
    };
    let base = workspace_base(apps_path);
    let mut targets = Vec::new();
    let mut problems = Vec::new();
    for app in selected_apps {
        for device in &selected_devices {
            match check_target(app, device, &base) {
                Ok(target) => targets.push(target),
                Err(problem) => problems.push(problem),
            }
        }
    }
    if problems.is_empty() {
        Ok(targets)
    } else {
        Err(problems)
    }
}

fn target_pairs(
    apps: &AppsConfig,
    devices: &DevicesConfig,
    app_name: &str,
    requested: &[String],
    apps_path: &Path,
) -> Result<Vec<Target>, Vec<String>> {
    let app = match find_app(apps, app_name) {
        Ok(app) => app,
        Err(problem) => return Err(vec![problem]),
    };
    let selected = match select_devices(devices, requested) {
        Ok(selected) => selected,
        Err(problem) => return Err(vec![problem]),
    };
    let base = workspace_base(apps_path);
    let mut targets = Vec::new();
    let mut problems = Vec::new();
    for device in &selected {
        match base_target(app, device, &base, false) {
            Ok(target) => targets.push(target),
            Err(problem) => problems.push(problem),
        }
    }
    if problems.is_empty() {
        Ok(targets)
    } else {
        Err(problems)
    }
}

fn install_target(app: &AppConfig, device: &DeviceConfig, base: &Path) -> Result<Target, String> {
    let mut target = check_target(app, device, base)?;
    let Some(package) = target.package.take() else {
        return Err(format!(
            "{IMP_APP_NO_PACKAGE}: app '{}' ({}) has no package; add platforms.{}.package or \
             use `app check` to attach to an installed app",
            app.name, device.platform, device.platform
        ));
    };
    if !package_allows_platform(&package, device.platform) {
        return Err(format!(
            "{IMP_APP_PACKAGE}: app '{}' ({}): package is not usable for this platform: {}",
            app.name,
            device.platform,
            package.display()
        ));
    }
    if let Some(entry) = target.app.platforms.get_mut(&device.platform) {
        entry.package = Some(package.display().to_string());
    }
    target.package = Some(package);
    Ok(target)
}

fn check_target(app: &AppConfig, device: &DeviceConfig, base: &Path) -> Result<Target, String> {
    base_target(app, device, base, true)
}

fn base_target(
    app: &AppConfig,
    device: &DeviceConfig,
    base: &Path,
    package_must_exist: bool,
) -> Result<Target, String> {
    if device.platform == Platform::Web {
        return Err(format!(
            "{IMP_APP_WEB}: app '{}': web devices have a URL, not an installable package",
            app.name
        ));
    }
    let Some(entry) = app.platform(device.platform) else {
        return Err(format!(
            "{IMP_APP_PLATFORM}: app '{}' has no entry for {} (device '{}')",
            app.name, device.platform, device.name
        ));
    };
    let Some(app_id) = entry.id.as_deref().filter(|value| !value.trim().is_empty()) else {
        return Err(format!(
            "{IMP_APP_PLATFORM}: app '{}' ({}) has no id",
            app.name, device.platform
        ));
    };
    let package = entry
        .package
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|package| resolve_package(base, package));
    if let Some(missing) = package
        .as_deref()
        .filter(|_| package_must_exist)
        .filter(|path| !path.exists())
    {
        return Err(format!(
            "{IMP_APP_PACKAGE}: app '{}' ({}): package does not exist: {}",
            app.name,
            device.platform,
            missing.display()
        ));
    }
    Ok(Target {
        device: device.clone(),
        app: app.clone(),
        app_id: app_id.to_string(),
        package,
    })
}

fn find_app<'a>(apps: &'a AppsConfig, name: &str) -> Result<&'a AppConfig, String> {
    apps.apps
        .iter()
        .find(|app| app.name == name)
        .ok_or_else(|| format!("{IMP_APP_UNKNOWN}: no app named '{name}'"))
}

fn select_apps<'a>(apps: &'a AppsConfig, names: &[String]) -> Result<Vec<&'a AppConfig>, String> {
    if names.is_empty() {
        return Ok(apps.apps.iter().collect());
    }
    let mut selected = Vec::new();
    for name in names {
        selected.push(find_app(apps, name)?);
    }
    Ok(selected)
}

fn select_devices(
    devices: &DevicesConfig,
    requested: &[String],
) -> Result<Vec<DeviceConfig>, String> {
    let selected = select_devices_with_groups(&devices.devices, &devices.groups, requested)
        .map_err(|error| format!("{IMP_APP_DEVICE}: {error}"))?;
    if selected.is_empty() {
        return Err(format!("{IMP_APP_DEVICE}: no devices configured"));
    }
    Ok(selected)
}

fn workspace_base(apps_path: &Path) -> PathBuf {
    apps_path.parent().unwrap_or(Path::new(".")).to_path_buf()
}

fn resolve_package(base: &Path, package: &str) -> PathBuf {
    let path = PathBuf::from(package);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn package_allows_platform(package: &Path, platform: Platform) -> bool {
    match platform {
        Platform::Android | Platform::AndroidTv | Platform::Tizen => package.is_file(),
        Platform::Ios | Platform::Tvos | Platform::WebOs => package.is_file() || package.is_dir(),
        Platform::Web => false,
    }
}

// ---------------------------------------------------------------------------
// execution
// ---------------------------------------------------------------------------

fn runtime() -> Result<tokio::runtime::Runtime, i32> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            eprintln!("app: cannot start the async runtime: {error}");
            EXIT_INFRA
        })
}

async fn install_one(home: &Home, target: &Target) -> Result<String, String> {
    let device = &target.device;
    match device.platform {
        Platform::Android | Platform::AndroidTv => {
            let adb = AdbTarget::for_device(device)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let client = impresario_clients::AdbClient::new(adb, home);
            client
                .install_apk(package_of(target)?)
                .await
                .map_err(|error| {
                    format!(
                        "{}: app '{}' on '{}': {error}",
                        IMP_APP_DEVICE, target.app.name, device.name
                    )
                })?;
            let installed = client
                .package_installed(&target.app_id)
                .await
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            if !installed {
                return Err(format!(
                    "{}: app '{}' on '{}': install reported success but '{}' is missing from \
                     pm list packages",
                    IMP_APP_DEVICE, target.app.name, device.name, target.app_id
                ));
            }
            Ok(format!(
                "installed {} on {} (android)",
                target.app_id, device.name
            ))
        }
        Platform::WebOs => {
            let session = impresario_clients::webos::WebOsSession::connect(device, home)
                .await
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let report =
                impresario_clients::webos::install_configured_app(&session, home, &target.app)
                    .await
                    .map_err(|error| {
                        format!(
                            "{}: app '{}' on '{}': {error}",
                            IMP_APP_DEVICE, target.app.name, device.name
                        )
                    })?;
            Ok(format!(
                "installed {} on {} (webOS)",
                report.app_id, device.name
            ))
        }
        Platform::Tizen => {
            let mut transport = impresario_clients::tizen::TizenTransport::new();
            let report = transport
                .install_app(device, home, &target.app)
                .await
                .map_err(|error| {
                    format!(
                        "{}: app '{}' on '{}': {error}",
                        IMP_APP_DEVICE, target.app.name, device.name
                    )
                })?;
            let Some(report) = report else {
                return Err(format!(
                    "{}: app '{}' on '{}': no package to install",
                    IMP_APP_DEVICE, target.app.name, device.name
                ));
            };
            Ok(format!(
                "installed {} on {} (Tizen{})",
                report.app_id,
                device.name,
                if report.signed { ", re-signed" } else { "" }
            ))
        }
        Platform::Ios | Platform::Tvos => {
            let host = apple_host(device)?;
            let paths = impresario_apple_device::ApplePaths::new(home.root());
            let pairing = crate::apple::load_pairing(&paths, None, &host)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let mut tunnel = impresario_apple_device::AppleTunnel::connect(&host, &pairing)
                .await
                .map_err(|error| format!("{}: connect to {}: {error}", IMP_APP_DEVICE, host))?;
            let report = impresario_apple_device::install::install_package(
                &mut tunnel,
                package_of(target)?,
                Some(target.app_id.as_str()),
            )
            .await
            .map_err(|error| {
                format!(
                    "{}: app '{}' on '{}': {error}",
                    IMP_APP_DEVICE, target.app.name, device.name
                )
            })?;
            Ok(format!(
                "installed {} on {} ({})",
                report.bundle_id, device.name, device.platform
            ))
        }
        Platform::Web => Err(format!(
            "{}: web devices are rejected before execution",
            IMP_APP_DEVICE
        )),
    }
}

async fn uninstall_one(home: &Home, target: &Target) -> Result<String, String> {
    let device = &target.device;
    match device.platform {
        Platform::Android | Platform::AndroidTv => {
            let adb = AdbTarget::for_device(device)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let client = impresario_clients::AdbClient::new(adb, home);
            let removed = client
                .uninstall_package(&target.app_id)
                .await
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            Ok(removal_message(
                removed,
                &target.app_id,
                &device.name,
                device.platform,
            ))
        }
        Platform::WebOs => {
            let session = impresario_clients::webos::WebOsSession::connect(device, home)
                .await
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let removed = session.uninstall(&target.app_id).await.map_err(|error| {
                format!(
                    "{}: app '{}' on '{}': {error}",
                    IMP_APP_DEVICE, target.app.name, device.name
                )
            })?;
            Ok(removal_message(
                removed,
                &target.app_id,
                &device.name,
                device.platform,
            ))
        }
        Platform::Tizen => {
            let mut transport = impresario_clients::tizen::TizenTransport::new();
            transport
                .bind(device, home)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let removed = transport
                .uninstall_app(&target.app_id)
                .await
                .map_err(|error| {
                    format!(
                        "{}: app '{}' on '{}': {error}",
                        IMP_APP_DEVICE, target.app.name, device.name
                    )
                })?;
            Ok(removal_message(
                removed,
                &target.app_id,
                &device.name,
                device.platform,
            ))
        }
        Platform::Ios | Platform::Tvos => {
            let host = apple_host(device)?;
            let paths = impresario_apple_device::ApplePaths::new(home.root());
            let pairing = crate::apple::load_pairing(&paths, None, &host)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let mut tunnel = impresario_apple_device::AppleTunnel::connect(&host, &pairing)
                .await
                .map_err(|error| format!("{}: connect to {}: {error}", IMP_APP_DEVICE, host))?;
            let installed =
                impresario_apple_device::install::installed_app(&mut tunnel, &target.app_id)
                    .await
                    .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            if installed {
                impresario_apple_device::install::uninstall_bundle(&mut tunnel, &target.app_id)
                    .await
                    .map_err(|error| {
                        format!(
                            "{}: app '{}' on '{}': {error}",
                            IMP_APP_DEVICE, target.app.name, device.name
                        )
                    })?;
            }
            Ok(removal_message(
                installed,
                &target.app_id,
                &device.name,
                device.platform,
            ))
        }
        Platform::Web => Err(format!(
            "{}: web devices are rejected before execution",
            IMP_APP_DEVICE
        )),
    }
}

async fn check_one(home: &Home, target: &Target) -> Result<String, String> {
    let device = &target.device;
    let (installed, signature) = match device.platform {
        Platform::Android | Platform::AndroidTv => {
            let adb = AdbTarget::for_device(device)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let client = impresario_clients::AdbClient::new(adb, home);
            (
                client
                    .package_installed(&target.app_id)
                    .await
                    .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?,
                "not checked (Android has no native installed-signature reader; the install \
                 service accepted the artifact)"
                    .to_string(),
            )
        }
        Platform::WebOs => {
            let session = impresario_clients::webos::WebOsSession::connect(device, home)
                .await
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            (
                session
                    .app_version(&target.app_id)
                    .await
                    .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?
                    .is_some(),
                "not checked (webOS has no native installed-signature reader; the install \
                 service accepted the artifact)"
                    .to_string(),
            )
        }
        Platform::Tizen => check_tizen(home, target).await?,
        Platform::Ios | Platform::Tvos => {
            let host = apple_host(device)?;
            let paths = impresario_apple_device::ApplePaths::new(home.root());
            let pairing = crate::apple::load_pairing(&paths, None, &host)
                .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
            let mut tunnel = impresario_apple_device::AppleTunnel::connect(&host, &pairing)
                .await
                .map_err(|error| format!("{}: connect to {}: {error}", IMP_APP_DEVICE, host))?;
            (
                impresario_apple_device::install::installed_app(&mut tunnel, &target.app_id)
                    .await
                    .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?,
                "not checked (Apple has no native installed-signature reader; \
                 InstallationProxy accepted the artifact)"
                    .to_string(),
            )
        }
        Platform::Web => {
            return Err(format!(
                "{}: web devices are rejected before execution",
                IMP_APP_DEVICE
            ));
        }
    };
    if !installed {
        return Err(format!(
            "app '{}' on '{}' ({}): {} is not installed",
            target.app.name, device.name, device.platform, target.app_id
        ));
    }
    Ok(format!(
        "app '{}' on '{}' ({}): {} installed; signature {signature}",
        target.app.name, device.name, device.platform, target.app_id
    ))
}

async fn check_tizen(home: &Home, target: &Target) -> Result<(bool, String), String> {
    let mut transport = impresario_clients::tizen::TizenTransport::new();
    transport
        .bind(&target.device, home)
        .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
    let capabilities = transport
        .capabilities()
        .await
        .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
    let installed = transport
        .app_installed(&target.app_id)
        .await
        .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
    if !installed {
        return Ok((false, String::new()));
    }
    if !capabilities.requires_signing() {
        return Ok((
            true,
            format!(
                "not required (Tizen {} installs the original package)",
                capabilities.platform_version()
            ),
        ));
    }
    let duid = transport
        .duid()
        .await
        .map_err(|error| format!("{}: {error}", IMP_APP_DEVICE))?;
    let profile_name = impresario_clients::tizen::tizen_profile_name();
    let mut notes = Vec::new();
    if let Some(package) = &target.package {
        let bytes = std::fs::read(package).map_err(|error| {
            format!(
                "{}: app '{}' ({}): cannot read {}: {error}",
                IMP_APP_DEVICE,
                target.app.name,
                target.device.platform,
                package.display()
            )
        })?;
        // The runner accepts an unsigned source `.wgt` and signs it for the
        // TV, so a missing/invalid source signature is not an installed-state
        // failure. Validate it when it is signed; otherwise say so and rely
        // on the profile policy below.
        impresario_clients::tizen_sign::inspect_wgt(package).map_err(|error| {
            format!(
                "app '{}' on '{}' (Tizen): configured package is not a usable .wgt: {error}",
                target.app.name, target.device.name
            )
        })?;
        match impresario_clients::tizen_sign::validate_signed_wgt(&bytes) {
            Ok(()) => notes.push("configured package signature verifies".to_string()),
            Err(_) => notes.push(
                "configured package is unsigned or its signature does not verify; the runner \
                 re-signs it for the device, so the installed signature is not readable"
                    .to_string(),
            ),
        }
    }
    let profile = impresario_clients::tizen_sign::show_profile(home, &profile_name)
        .map_err(|error| error.to_string())?;
    let now = chrono::Utc::now().timestamp();
    if profile.author_not_after_unix <= 0 || profile.distributor_not_after_unix <= 0 {
        return Err(format!(
            "app '{}' on '{}' (Tizen): profile '{profile_name}' has no recorded expiry; \
             re-import it",
            target.app.name, target.device.name
        ));
    }
    if profile.author_not_after_unix <= now || profile.distributor_not_after_unix <= now {
        return Err(format!(
            "IMP-SIGN-004: app '{}' on '{}' (Tizen): profile '{profile_name}' has an expired \
             certificate",
            target.app.name, target.device.name
        ));
    }
    if !profile.duids.iter().any(|candidate| candidate == &duid) {
        return Err(format!(
            "IMP-SIGN-006: app '{}' on '{}' (Tizen): DUID '{duid}' is not covered by profile \
             '{profile_name}'",
            target.app.name, target.device.name
        ));
    }
    notes.push(format!(
        "profile '{profile_name}' covers DUID {duid} and is unexpired"
    ));
    if target.package.is_none() {
        notes.push("installed signature not readable; checked profile policy".to_string());
    }
    Ok((true, notes.join("; ")))
}

fn package_of(target: &Target) -> Result<&Path, String> {
    target.package.as_deref().ok_or_else(|| {
        format!(
            "{}: app '{}' has no package",
            IMP_APP_DEVICE, target.app.name
        )
    })
}

fn apple_host(device: &DeviceConfig) -> Result<String, String> {
    let host = device.host().trim().trim_matches(['[', ']']).to_string();
    if host.is_empty() || host.parse::<std::net::IpAddr>().is_err() {
        return Err(format!(
            "{}: Apple device '{}' needs a LAN host IP, got '{host}'",
            IMP_APP_DEVICE, device.name
        ));
    }
    Ok(host)
}

fn removal_message(removed: bool, app_id: &str, device: &str, platform: Platform) -> String {
    if removed {
        format!("removed {app_id} from {device} ({platform})")
    } else {
        format!("was not installed: {app_id} on {device} ({platform})")
    }
}

fn report(command: &str, problems: &[String]) {
    for problem in problems {
        eprintln!("{command}: {problem}");
    }
}

fn finish(command: &str, results: Vec<Result<String, String>>) -> i32 {
    let mut failures = 0usize;
    for result in results {
        match result {
            Ok(message) => println!("{message}"),
            Err(message) => {
                failures += 1;
                eprintln!("{command}: {message}");
            }
        }
    }
    if failures > 0 {
        eprintln!("{command}: {failures} operation(s) failed");
        EXIT_INFRA
    } else {
        EXIT_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::apps::{AppOptions, AppPlatform};

    fn app(platform: Platform, id: &str, package: Option<&str>) -> AppConfig {
        let mut platforms = std::collections::BTreeMap::new();
        platforms.insert(
            platform,
            AppPlatform {
                id: Some(id.to_string()),
                package: package.map(str::to_string),
                ..AppPlatform::default()
            },
        );
        AppConfig {
            name: "shop".to_string(),
            description: None,
            platforms,
            options: AppOptions::default(),
        }
    }

    fn device(name: &str, platform: Platform) -> DeviceConfig {
        DeviceConfig::new(name, platform, "192.0.2.10")
    }

    #[test]
    fn exact_platform_entry_wins_and_relative_packages_resolve() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let package = temporary.path().join("build/app.apk");
        std::fs::create_dir_all(package.parent().expect("parent")).expect("build");
        std::fs::write(&package, b"apk").expect("apk");
        let mut app = app(Platform::Android, "com.example.shop", Some("build/app.apk"));
        app.platforms.insert(
            Platform::AndroidTv,
            AppPlatform {
                id: Some("com.example.shop.tv".to_string()),
                ..AppPlatform::default()
            },
        );
        let target = check_target(&app, &device("tv", Platform::AndroidTv), temporary.path())
            .expect("androidtv entry");
        assert_eq!(target.app_id, "com.example.shop.tv");
        assert!(target.package.is_none());

        let target = check_target(&app, &device("phone", Platform::Android), temporary.path())
            .expect("android entry");
        assert_eq!(
            target.package.as_deref(),
            Some(package.as_path()),
            "relative packages resolve against the workspace base"
        );
    }

    #[test]
    fn missing_platform_entry_is_not_replaced_by_the_family_fallback() {
        let app = app(Platform::Android, "com.example.shop", None);
        let problem = check_target(&app, &device("tv", Platform::AndroidTv), Path::new("."))
            .expect_err("androidtv has no entry");
        assert!(problem.contains("IMP-APP-002"), "{problem}");
    }

    #[test]
    fn web_devices_are_rejected_before_hardware() {
        let app = app(Platform::Web, "https://example.test", None);
        let problem = check_target(&app, &device("browser", Platform::Web), Path::new("."))
            .expect_err("web is not installable");
        assert!(problem.contains("IMP-APP-005"), "{problem}");
    }

    #[test]
    fn install_requires_a_package() {
        let app = app(Platform::Android, "com.example.shop", None);
        let problem = install_target(&app, &device("phone", Platform::Android), Path::new("."))
            .expect_err("no package");
        assert!(problem.contains("IMP-APP-006"), "{problem}");
    }

    #[test]
    fn packages_are_type_checked_per_platform() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let directory = temporary.path().join("app-bundle");
        std::fs::create_dir_all(&directory).expect("bundle");
        assert!(!package_allows_platform(&directory, Platform::Android));
        assert!(package_allows_platform(&directory, Platform::Ios));
        assert!(package_allows_platform(&directory, Platform::WebOs));
        let file = temporary.path().join("app.apk");
        std::fs::write(&file, b"apk").expect("apk");
        assert!(package_allows_platform(&file, Platform::Android));
        assert!(package_allows_platform(&file, Platform::Tizen));
    }

    #[test]
    fn check_names_filter_and_unknown_names_fail() {
        let mut other = app(Platform::Android, "com.example.other", None);
        other.name = "other".to_string();
        let apps = AppsConfig {
            apps: vec![app(Platform::Android, "com.example.shop", None), other],
        };
        let selected = select_apps(&apps, &["shop".to_string()]).expect("filtered");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "shop");
        let problem = select_apps(&apps, &["missing".to_string()]).expect_err("unknown");
        assert!(problem.contains("IMP-APP-001"), "{problem}");
    }
}
