//! `impresario apple`: machine-global Apple TV setup and agent sessions.
//!
//! Like `impresario account`, this command runs before workspace resolution.
//! Pairing records, signing profiles, product bundles and personalized DDIs are
//! machine-global under `~/.impresario/apple/`, so a project only names the
//! device host and never carries certificates or pair secrets.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};
use impresario_apple_auth::account::{choose_team, load_session, AppleStore, CredentialStore};
use impresario_apple_auth::dev::app_store_connect::AppStoreConnectClient;
use impresario_apple_auth::dev::developer_session::DeveloperSession;
use impresario_apple_auth::dev::teams::{DeveloperTeam, TeamsApi};
use impresario_apple_device::ddi::{self, DdiAssets, DdiState};
use impresario_apple_device::pairing::{
    pair_device, ApplePaths, PairingSummary, IMPRESARIO_DEVICE_NAME,
};
use impresario_apple_device::products::ProductSet;
use impresario_apple_device::provision::{
    self, CertificateSummary, ExistingIdentity, ProvisionError, ProvisionPlatform, ProvisionRequest,
};
use impresario_apple_device::session::{self, SessionOptions};
use impresario_apple_device::signing_set::{
    self as signing_store, SigningSet, DEFAULT_SIGNING_SET, PASSWORD_ENV,
};
use impresario_apple_device::tunnel::AppleTunnel;
use impresario_apple_device::{Error, RunnerOptions};
use impresario_core::home::Home;
use zeroize::Zeroizing;

use crate::cli::{EXIT_FAILURE, EXIT_INFRA, EXIT_OK};

/// `impresario apple ...`
#[derive(Debug, Args)]
pub struct AppleArgs {
    #[command(subcommand)]
    pub command: AppleCommand,
}

#[derive(Debug, Subcommand)]
pub enum AppleCommand {
    /// Put the Apple TV in pairing mode and pair this machine.
    Pair(PairArgs),
    /// Import an existing pair plist (for example from pymobiledevice3).
    ImportPair(ImportPairArgs),
    /// Show what this machine has for Apple without touching a device.
    Status,
    /// Check pairing, signing, products and DDI against a live Apple TV.
    Doctor(DoctorArgs),
    /// Manage the unsigned agent app bundles.
    Products {
        #[command(subcommand)]
        command: ProductsCommand,
    },
    /// Manage signing identities, profiles and entitlements.
    Signing {
        #[command(subcommand)]
        command: SigningCommand,
    },
    /// Manage personalized Developer Disk Images.
    Ddi {
        #[command(subcommand)]
        command: DdiCommand,
    },
    /// Start the XCUITest agent and keep it alive until interrupted.
    Session(SessionArgs),
}

#[derive(Debug, Args)]
pub struct PairArgs {
    /// Apple TV IP address or hostname.
    #[arg(long)]
    pub host: String,
    /// Manual-pairing port; omit to discover it over mDNS.
    #[arg(long)]
    pub port: Option<u16>,
    /// Save the pairing under this key instead of the device UDID.
    #[arg(long)]
    pub save: Option<String>,
}

#[derive(Debug, Args)]
pub struct ImportPairArgs {
    /// Existing pair plist from pymobiledevice3 or another RemotePairing host.
    pub file: PathBuf,
    /// Pairing key (normally the device UDID).
    #[arg(long)]
    pub save: String,
    /// Host identifier to embed when the source plist has none.
    #[arg(long)]
    pub host_id: Option<String>,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Apple TV IP address or hostname.
    #[arg(long)]
    pub host: String,
    /// Pairing key/device UDID; omit when exactly one pairing is stored.
    #[arg(long)]
    pub udid: Option<String>,
    /// Signing set name under `apple/signing/`.
    #[arg(long, default_value = DEFAULT_SIGNING_SET)]
    pub signing: String,
    /// Product set name under `apple/products/`.
    #[arg(long, default_value = "current")]
    pub products: String,
    /// Explicit DDI directory; defaults to `apple/ddi/<build>/`.
    #[arg(long)]
    pub ddi: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum ProductsCommand {
    /// Classify and import Xcode's `Debug-appletvos`/`Debug-iphoneos` output.
    Import {
        /// Directory containing the unsigned `.app` bundles.
        source: PathBuf,
        /// Product set name under `apple/products/`.
        #[arg(long, default_value = "current")]
        name: String,
    },
    /// Show an imported product set.
    Status {
        #[arg(long, default_value = "current")]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SigningCommand {
    /// Validate a signing tree and copy it into the global signing store.
    Import {
        /// Directory containing profiles, entitlements and identity.p12.
        source: PathBuf,
        /// Signing set name under `apple/signing/`.
        #[arg(long, default_value = DEFAULT_SIGNING_SET)]
        name: String,
    },
    /// Create or refresh development certificates and profiles through Apple.
    Provision(ProvisionArgs),
    /// List the team's development certificates without changing them.
    Certificates(CertificatesArgs),
    /// Sign an imported product set into a directory (no device is contacted).
    Verify {
        #[arg(long, default_value = DEFAULT_SIGNING_SET)]
        name: String,
        #[arg(long, default_value = "current")]
        products: String,
        /// Also sign and include the fixture stub.
        #[arg(long)]
        with_stub: bool,
        /// Keep the signed bundles here; omit for a temporary directory.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Show team, profiles, expirations and whether the identity is usable.
    Status {
        #[arg(long, default_value = DEFAULT_SIGNING_SET)]
        name: String,
    },
    /// Forget the PKCS#12 password stored in the OS credential store.
    ClearPassword {
        #[arg(long, default_value = DEFAULT_SIGNING_SET)]
        name: String,
    },
}

/// Apple platform for certificate and profile operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ApplePlatform {
    Ios,
    Tvos,
}

impl ApplePlatform {
    fn provision(self) -> ProvisionPlatform {
        match self {
            Self::Ios => ProvisionPlatform::Ios,
            Self::Tvos => ProvisionPlatform::Tvos,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ios => "iOS",
            Self::Tvos => "tvOS",
        }
    }
}

#[derive(Debug, Args)]
pub struct ProvisionArgs {
    /// Signing set name under `apple/signing/`.
    #[arg(long, default_value = DEFAULT_SIGNING_SET)]
    pub name: String,
    /// Product set whose host, runner and nested bundles are provisioned.
    #[arg(long, default_value = "current")]
    pub products: String,
    /// Also create the App ID and profile for the fixture stub.
    #[arg(long)]
    pub with_stub: bool,
    /// Ignore the target signing set's PKCS#12 and generate a new RSA key.
    #[arg(long, conflicts_with = "identity")]
    pub new_identity: bool,
    /// Reuse a specific PKCS#12 identity instead of the target signing set's.
    #[arg(long, conflicts_with = "new_identity")]
    pub identity: Option<PathBuf>,
    /// Device UDID; defaults to the only stored pairing key.
    #[arg(long)]
    pub udid: Option<String>,
    /// Device name shown in the Apple Developer portal.
    #[arg(long)]
    pub device_name: Option<String>,
    /// Certificate machine name; defaults to this computer's name.
    #[arg(long)]
    pub machine_name: Option<String>,
    /// Developer team id; defaults to the team stored by `account login`.
    ///
    /// Required with `--api-key` unless `IMPRESARIO_APPLE_TEAM_ID` is set.
    #[arg(long)]
    pub team: Option<String>,
    /// App Store Connect API key (`.p8`) for unattended provisioning.
    ///
    /// This bypasses Apple ID password and 2FA. Also accepts
    /// `IMPRESARIO_APPLE_API_KEY` as a path.
    #[arg(
        long,
        value_name = "AUTH_KEY.p8",
        requires_all = ["issuer_id", "key_id"]
    )]
    pub api_key: Option<PathBuf>,
    /// App Store Connect API issuer id, shown above the key list.
    #[arg(long, requires = "api_key")]
    pub issuer_id: Option<String>,
    /// App Store Connect API key id, the `KEYID` in `AuthKey_KEYID.p8`.
    #[arg(long, requires = "api_key")]
    pub key_id: Option<String>,
    /// Platform for the development certificate and profiles.
    #[arg(long, value_enum, default_value = "tvos")]
    pub platform: ApplePlatform,
    /// Revoke this certificate serial before asking Apple for a new identity
    /// (repeatable; useful when the certificate limit is full).
    #[arg(long = "revoke", value_name = "SERIAL")]
    pub revoke: Vec<String>,
}

#[derive(Debug, Args)]
pub struct CertificatesArgs {
    /// Developer team id; defaults to the team stored by `account login`.
    #[arg(long)]
    pub team: Option<String>,
    /// App Store Connect API key (`.p8`) for unattended certificate listing.
    #[arg(
        long,
        value_name = "AUTH_KEY.p8",
        requires_all = ["issuer_id", "key_id"]
    )]
    pub api_key: Option<PathBuf>,
    /// App Store Connect API issuer id.
    #[arg(long, requires = "api_key")]
    pub issuer_id: Option<String>,
    /// App Store Connect API key id.
    #[arg(long, requires = "api_key")]
    pub key_id: Option<String>,
    /// Platform whose development certificates should be listed.
    #[arg(long, value_enum, default_value = "tvos")]
    pub platform: ApplePlatform,
}

#[derive(Debug, Subcommand)]
pub enum DdiCommand {
    /// Copy `Image.dmg`, trust cache and BuildManifest into `apple/ddi/<build>/`.
    Import {
        /// Directory containing the three personalized DDI files.
        source: PathBuf,
        /// Exact OS build reported by RSD, for example `23J582`.
        #[arg(long)]
        build: String,
    },
    /// List imported DDI builds.
    Status,
}

#[derive(Debug, Args)]
pub struct SessionArgs {
    /// Apple TV IP address or hostname.
    #[arg(long)]
    pub host: String,
    /// Pairing key/device UDID; omit when exactly one pairing is stored.
    #[arg(long)]
    pub udid: Option<String>,
    /// Signing set name under `apple/signing/`.
    #[arg(long, default_value = DEFAULT_SIGNING_SET)]
    pub signing: String,
    /// Product set name under `apple/products/`.
    #[arg(long, default_value = "current")]
    pub products: String,
    /// Explicit DDI directory; defaults to `apple/ddi/<build>/`.
    #[arg(long)]
    pub ddi: Option<PathBuf>,
    /// Signed app under test (`.app` or `.ipa`), installed before the agent starts.
    #[arg(long)]
    pub app: Option<PathBuf>,
    /// Expected bundle id of `--app`.
    #[arg(long)]
    pub app_bundle_id: Option<String>,
    /// Also install the fixture stub from the product set.
    #[arg(long)]
    pub with_stub: bool,
    /// Port the agent listens on inside the Apple TV.
    #[arg(long, default_value_t = 50062)]
    pub agent_port: u16,
    /// Seconds to wait for the agent port after starting XCUITest.
    #[arg(long, default_value_t = 120)]
    pub startup_timeout: u64,
}

/// Run the Apple command.
#[must_use]
pub fn run(args: &AppleArgs, home: &Home) -> i32 {
    let paths = ApplePaths::new(home.root());
    match &args.command {
        AppleCommand::Pair(args) => run_pair(args, &paths),
        AppleCommand::ImportPair(args) => run_import_pair(args, &paths),
        AppleCommand::Status => run_status(&paths),
        AppleCommand::Doctor(args) => run_doctor(args, &paths),
        AppleCommand::Products { command } => run_products(command, &paths),
        AppleCommand::Signing { command } => run_signing(command, &paths, home),
        AppleCommand::Ddi { command } => run_ddi(command, &paths),
        AppleCommand::Session(args) => run_session(args, &paths),
    }
}

fn run_pair(args: &PairArgs, paths: &ApplePaths) -> i32 {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    println!(
        "Pairing with {} as host {}...",
        args.host,
        paths.host_identifier()
    );
    println!("Enter the code shown on the Apple TV when prompted.");
    let pin = || async {
        tokio::task::spawn_blocking(|| {
            rpassword::prompt_password("Pairing code: ").unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    };
    let mut summary = match runtime.block_on(pair_device(&args.host, args.port, paths, pin)) {
        Ok(summary) => summary,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    if let Some(save) = args.save.as_deref() {
        if save != summary.saved_as {
            match move_pairing(paths, &summary.saved_as, save) {
                Ok(()) => summary.saved_as = save.to_string(),
                Err(error) => {
                    eprintln!("error: {error}");
                    return EXIT_FAILURE;
                }
            }
        }
    }
    print_pairing(&summary, paths);
    EXIT_OK
}

fn run_import_pair(args: &ImportPairArgs, paths: &ApplePaths) -> i32 {
    match paths.import_pairing_to(&args.file, &args.save, args.host_id.as_deref()) {
        Ok(file) => {
            println!(
                "Imported pairing as {} (host identifier {}).",
                paths.pairing_path(&args.save).display(),
                file.identifier
            );
            println!("The pair keys are stored in the OS user profile and are never printed.");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("error: {error}");
            EXIT_FAILURE
        }
    }
}

fn run_status(paths: &ApplePaths) -> i32 {
    println!("Apple root: {}", paths.root().display());
    match pairing_keys(paths) {
        Ok(keys) if keys.is_empty() => println!("Pairings: none"),
        Ok(keys) => println!("Pairings: {}", keys.join(", ")),
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_INFRA;
        }
    }
    match set_names(&paths.signing_dir()) {
        Ok(names) if names.is_empty() => println!("Signing sets: none"),
        Ok(names) => println!("Signing sets: {}", names.join(", ")),
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_INFRA;
        }
    }
    match set_names(&paths.products_dir()) {
        Ok(names) if names.is_empty() => println!("Product sets: none"),
        Ok(names) => println!("Product sets: {}", names.join(", ")),
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_INFRA;
        }
    }
    match set_names(&paths.ddi_dir()) {
        Ok(names) if names.is_empty() => println!("DDI builds: none"),
        Ok(names) => println!("DDI builds: {}", names.join(", ")),
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_INFRA;
        }
    }
    EXIT_OK
}

fn run_doctor(args: &DoctorArgs, paths: &ApplePaths) -> i32 {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let pairing = match load_pairing(paths, args.udid.as_deref()) {
        Ok(pairing) => pairing,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let signing = match SigningSet::load(&paths.signing_dir(), &args.signing) {
        Ok(set) => Some(set),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    };
    let products = match ProductSet::load(&paths.product_set_dir(&args.products)) {
        Ok(set) => Some(set),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    };
    if let Some(signing) = &signing {
        match signing.password() {
            Ok(_) => println!("Signing: {} (password available)", signing.name()),
            Err(error) => errors.push(error.to_string()),
        }
        println!(
            "Signing team: {}; bundles: {}",
            signing.team_id(),
            signing.bundle_ids().collect::<Vec<_>>().join(", ")
        );
        if let Some(products) = &products {
            for (bundle_id, required) in product_bundle_ids(products) {
                match signing.require(&bundle_id) {
                    Ok(materials) => {
                        let info = &materials.info;
                        println!(
                            "  {bundle_id}: profile {} expires {}",
                            info.name.as_deref().unwrap_or("unnamed"),
                            info.expiration.as_deref().unwrap_or("unknown")
                        );
                        if !info.allows_bundle(&bundle_id) {
                            errors.push(format!(
                                "profile for {bundle_id} does not cover that bundle id"
                            ));
                        }
                        if info.get_task_allow != Some(true) {
                            warnings.push(format!(
                                "profile for {bundle_id} is not marked get-task-allow"
                            ));
                        }
                    }
                    Err(error) if !required => warnings.push(format!(
                        "fixture stub is not provisioned; stub plays need `--with-stub`: {error}"
                    )),
                    Err(error) => errors.push(error.to_string()),
                }
            }
        }
    }
    if let Some(products) = &products {
        println!("Products: {}", products.root().display());
        match products.host_bundle_id() {
            Ok(id) => println!("  host:   {id}"),
            Err(error) => errors.push(error.to_string()),
        }
        match products.stub_bundle_id() {
            Ok(Some(id)) => println!("  stub:   {id}"),
            Ok(None) => warnings.push("product set has no stub app".to_string()),
            Err(error) => errors.push(error.to_string()),
        }
        match products.runner_bundle_id() {
            Ok(id) => println!("  tests:  {id}"),
            Err(error) => errors.push(error.to_string()),
        }
        if products.nested_relative_path().is_none() {
            warnings.push("test bundle has no nested .xctest".to_string());
        }
    }

    let result = runtime.block_on(async {
        let mut tunnel = AppleTunnel::connect(&args.host, &pairing).await?;
        let summary = tunnel.summary();
        let udid = tunnel.udid().map(str::to_string);
        let build = tunnel.build_version().map(str::to_string);
        let state = ddi::query(&mut tunnel).await?;
        Ok::<_, Error>((summary, udid, build, state))
    });
    match result {
        Ok((summary, udid, build, state)) => {
            println!("Device: {summary}");
            println!(
                "  UDID: {}  DDI: {}",
                udid.as_deref().unwrap_or("unknown"),
                match state {
                    DdiState::Mounted => "mounted",
                    DdiState::NotMounted => "not mounted",
                }
            );
            if let (Some(build), None) = (build.as_deref(), args.ddi.as_deref()) {
                match DdiAssets::from_dir(&paths.ddi_build_dir(build)) {
                    Ok(_) => println!("DDI assets: {}", paths.ddi_build_dir(build).display()),
                    Err(error) => errors.push(error.to_string()),
                }
            }
            if let Some(ddi) = args.ddi.as_deref() {
                match DdiAssets::from_dir(ddi) {
                    Ok(_) => println!("DDI assets: {}", ddi.display()),
                    Err(error) => errors.push(error.to_string()),
                }
            }
            if let (Some(expected), Some(actual)) = (args.udid.as_deref(), udid.as_deref()) {
                if !expected.eq_ignore_ascii_case(actual) {
                    errors.push(format!("connected to UDID {actual}, expected {expected}"));
                }
            }
        }
        Err(error) => errors.push(error.to_string()),
    }

    for warning in &warnings {
        println!("warning: {warning}");
    }
    for error in &errors {
        eprintln!("error: {error}");
    }
    if errors.is_empty() {
        println!("Apple doctor: ready.");
        EXIT_OK
    } else {
        EXIT_FAILURE
    }
}

fn run_products(command: &ProductsCommand, paths: &ApplePaths) -> i32 {
    match command {
        ProductsCommand::Import { source, name } => {
            let destination = paths.product_set_dir(name);
            match ProductSet::import(source, &destination) {
                Ok(set) => {
                    println!("Imported products into {}", set.root().display());
                    print_products(&set);
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    EXIT_FAILURE
                }
            }
        }
        ProductsCommand::Status { name } => {
            let destination = paths.product_set_dir(name);
            match ProductSet::load(&destination) {
                Ok(set) => {
                    print_products(&set);
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    EXIT_FAILURE
                }
            }
        }
    }
}

fn run_signing(command: &SigningCommand, paths: &ApplePaths, home: &Home) -> i32 {
    match command {
        SigningCommand::Provision(args) => run_provision(args, paths, home),
        SigningCommand::Certificates(args) => run_certificates(args, home),
        SigningCommand::Import { source, name } => {
            let password = match read_password(true) {
                Ok(password) => password,
                Err(error) => {
                    eprintln!("error: {error}");
                    return EXIT_FAILURE;
                }
            };
            match SigningSet::import(source, &paths.signing_dir(), name, &password) {
                Ok(report) => {
                    println!(
                        "Imported signing set '{}' ({}) into {}",
                        report.name,
                        report.team_id,
                        report.root.display()
                    );
                    println!("Bundles: {}", report.bundles.join(", "));
                    if let Some(warning) = report.warning {
                        println!("warning: {warning}");
                    } else if report.password_stored {
                        println!("The PKCS#12 password is stored in the OS credential store.");
                    }
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    EXIT_FAILURE
                }
            }
        }
        SigningCommand::Verify {
            name,
            products,
            with_stub,
            output,
        } => {
            let signing = match SigningSet::load(&paths.signing_dir(), name) {
                Ok(signing) => signing,
                Err(error) => {
                    eprintln!("error: {error}");
                    return EXIT_FAILURE;
                }
            };
            let products = match ProductSet::load(&paths.product_set_dir(products)) {
                Ok(products) => products,
                Err(error) => {
                    eprintln!("error: {error}");
                    return EXIT_FAILURE;
                }
            };
            let temporary = output.is_none();
            let destination = output.clone().unwrap_or_else(|| {
                paths
                    .tmp_dir()
                    .join(format!("sign-verify-{}", uuid::Uuid::new_v4()))
            });
            let result = session::sign_products(&products, &signing, &destination, *with_stub);
            match result {
                Ok(signed) => {
                    println!("Signed products into {}", destination.display());
                    println!(
                        "  host:   {} ({})",
                        signed.host.display(),
                        signed.host_bundle_id
                    );
                    if let (Some(stub), Some(id)) = (&signed.stub, &signed.stub_bundle_id) {
                        println!("  stub:   {} ({id})", stub.display());
                    }
                    println!(
                        "  tests:  {} ({})",
                        signed.runner.display(),
                        signed.runner_bundle_id
                    );
                    if temporary {
                        let _ = std::fs::remove_dir_all(&destination);
                    }
                    EXIT_OK
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&destination);
                    eprintln!("error: {error}");
                    EXIT_FAILURE
                }
            }
        }
        SigningCommand::Status { name } => match SigningSet::load(&paths.signing_dir(), name) {
            Ok(set) => {
                println!("Signing set '{}': {}", set.name(), set.root().display());
                println!("Team: {}", set.team_id());
                match set.password() {
                    Ok(_) => println!("PKCS#12 password: available"),
                    Err(error) => println!("PKCS#12 password: unavailable ({error})"),
                }
                for bundle_id in set.bundle_ids() {
                    match set.require(bundle_id) {
                        Ok(materials) => println!(
                            "  {bundle_id}: {} (expires {})",
                            materials.info.name.as_deref().unwrap_or("unnamed"),
                            materials.info.expiration.as_deref().unwrap_or("unknown")
                        ),
                        Err(error) => println!("  {bundle_id}: {error}"),
                    }
                }
                EXIT_OK
            }
            Err(error) => {
                eprintln!("error: {error}");
                EXIT_FAILURE
            }
        },
        SigningCommand::ClearPassword { name } => {
            match SigningSet::load(&paths.signing_dir(), name) {
                Ok(set) => match set.clear_password() {
                    Ok(()) => {
                        println!("Removed the stored PKCS#12 password for '{}'.", set.name());
                        println!("Signing runs now need {PASSWORD_ENV} or a new import.");
                        EXIT_OK
                    }
                    Err(error) => {
                        eprintln!("error: {error}");
                        EXIT_FAILURE
                    }
                },
                Err(error) => {
                    eprintln!("error: {error}");
                    EXIT_FAILURE
                }
            }
        }
    }
}

fn run_certificates(args: &CertificatesArgs, home: &Home) -> i32 {
    let api_key = match resolve_api_key(
        args.api_key.as_deref(),
        args.issuer_id.as_deref(),
        args.key_id.as_deref(),
    ) {
        Ok(api_key) => api_key,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let result = runtime.block_on(async {
        if let Some(client) = api_key.as_ref() {
            let certificates = provision::list_certificates_api(client)
                .await
                .map_err(|error| error.to_string())?;
            let label = args
                .team
                .as_deref()
                .map(str::trim)
                .filter(|team| !team.is_empty())
                .map_or_else(
                    || "App Store Connect API key".to_string(),
                    |team| team.to_string(),
                );
            return Ok::<_, String>((label, certificates));
        }
        let (mut session, team) = load_developer(home, args.team.as_deref()).await?;
        let certificates =
            provision::list_certificates(&mut session, &team, args.platform.provision())
                .await
                .map_err(|error| error.to_string())?;
        Ok((
            format!(
                "{} ({})",
                team.name.as_deref().unwrap_or("unnamed"),
                team.team_id
            ),
            certificates,
        ))
    });
    match result {
        Ok((team_label, certificates)) => {
            println!("Team: {team_label}");
            if certificates.is_empty() {
                println!("No {} development certificates.", args.platform.label());
            } else {
                println!("{} development certificates:", args.platform.label());
                for certificate in certificates {
                    println!(
                        "  {}  status {}  expires {}  machine {}  name {}",
                        certificate.serial_number,
                        certificate.status.as_deref().unwrap_or("unknown"),
                        certificate.expiration.as_deref().unwrap_or("unknown"),
                        certificate.machine_name.as_deref().unwrap_or("unknown"),
                        certificate.name.as_deref().unwrap_or("unnamed")
                    );
                }
            }
            EXIT_OK
        }
        Err(error) => {
            eprintln!("error: {error}");
            EXIT_FAILURE
        }
    }
}

fn run_provision(args: &ProvisionArgs, paths: &ApplePaths, home: &Home) -> i32 {
    let products = match ProductSet::load(&paths.product_set_dir(&args.products)) {
        Ok(products) => products,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let bundle_ids = match provision_bundle_ids(&products, args.with_stub) {
        Ok(bundle_ids) => bundle_ids,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let device_key = match resolve_pairing_key(paths, args.udid.as_deref()) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    // The Apple Developer portal shows this name, not the Apple TV/iOS device
    // name and not the legacy pyatv host name.
    let device_name = provision_device_name(args.device_name.as_deref());
    let machine_name = args
        .machine_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(default_machine_name, str::to_string);
    let existing = match load_existing_identity(args, paths) {
        Ok(identity) => identity,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    // A fresh identity gets a randomly generated PKCS#12 password that is
    // never printed. Do not create an Apple certificate unless that password
    // can actually be stored, unless the caller supplied one through the
    // headless environment override.
    if existing.is_none() && std::env::var_os(PASSWORD_ENV).is_none() {
        if let Err(error) = signing_store::SigningSet::probe_credential_store() {
            eprintln!(
                "error: {error}; provisioning needs the OS credential store, or set \
                 {PASSWORD_ENV} for a headless run"
            );
            return EXIT_FAILURE;
        }
    }
    let api_key = match resolve_api_key(
        args.api_key.as_deref(),
        args.issuer_id.as_deref(),
        args.key_id.as_deref(),
    ) {
        Ok(api_key) => api_key,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let api_team_id = if api_key.is_some() {
        match resolve_api_team_id(args.team.as_deref()) {
            Ok(team_id) => Some(team_id),
            Err(error) => {
                eprintln!("error: {error}");
                return EXIT_FAILURE;
            }
        }
    } else {
        None
    };
    if api_key.is_some() {
        println!("Using an App Store Connect API key; no Apple ID session or 2FA is needed.");
    }

    let request = ProvisionRequest {
        platform: args.platform.provision(),
        machine_name,
        device_name,
        udid: device_key,
        bundle_ids,
        revoke_serials: args.revoke.clone(),
    };

    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    let result = runtime.block_on(async {
        if let Some(client) = api_key.as_ref() {
            let team_id = api_team_id.as_deref().ok_or_else(|| {
                ProvisionError::Invalid(
                    "an App Store Connect API key needs --team or IMPRESARIO_APPLE_TEAM_ID"
                        .to_string(),
                )
            })?;
            let mut revoke = args.revoke.clone();
            let mut prompts = 0;
            loop {
                let request = ProvisionRequest {
                    revoke_serials: revoke.clone(),
                    ..request.clone()
                };
                match provision::provision_with_api_key(
                    client,
                    team_id,
                    &request,
                    existing.as_ref(),
                )
                .await
                {
                    Ok(report) => {
                        return Ok::<_, ProvisionError>((
                            "App Store Connect API key".to_string(),
                            report,
                        ));
                    }
                    Err(ProvisionError::CertificatesFull(certificates)) => {
                        if prompts >= 2 {
                            return Err(ProvisionError::CertificatesFull(certificates));
                        }
                        revoke =
                            prompt_revocation(&certificates).map_err(ProvisionError::Invalid)?;
                        prompts += 1;
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        let (mut session, team) = load_developer(home, args.team.as_deref())
            .await
            .map_err(ProvisionError::Apple)?;
        let team_label = team.name.clone().unwrap_or_else(|| "unnamed".to_string());
        let mut revoke = args.revoke.clone();
        let mut prompts = 0;
        loop {
            let request = ProvisionRequest {
                revoke_serials: revoke.clone(),
                ..request.clone()
            };
            match provision::provision(&mut session, &team, &request, existing.as_ref()).await {
                Ok(report) => return Ok((team_label, report)),
                Err(ProvisionError::CertificatesFull(certificates)) => {
                    if prompts >= 2 {
                        return Err(ProvisionError::CertificatesFull(certificates));
                    }
                    revoke = prompt_revocation(&certificates).map_err(ProvisionError::Invalid)?;
                    prompts += 1;
                }
                Err(error) => return Err(error),
            }
        }
    });
    let (team_label, report) = match result {
        Ok(result) => result,
        Err(ProvisionError::CertificatesFull(certificates)) => {
            print_certificates(&certificates);
            eprintln!(
                "error: the Apple development certificate limit is still full; revoke a listed \
                 serial with --revoke or in the Apple Developer account"
            );
            return EXIT_FAILURE;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };

    let installed = match provision::install(&report, &paths.signing_dir(), &args.name) {
        Ok(installed) => installed,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };

    println!("Team: {} ({})", team_label, report.team_id);
    println!("Device: {} ({})", report.device_name, report.device_udid);
    if !report.app_ids_created.is_empty() {
        println!("App IDs created: {}", report.app_ids_created.join(", "));
    }
    if !report.app_ids_existing.is_empty() {
        println!("App IDs reused: {}", report.app_ids_existing.join(", "));
    }
    println!(
        "Identity: certificate {} ({}, key {})",
        report
            .identity
            .certificate_serial
            .as_deref()
            .unwrap_or("unknown"),
        if report.identity.reused_certificate {
            "reused"
        } else {
            "new"
        },
        if report.identity.reused_key {
            "reused"
        } else {
            "new"
        }
    );
    for bundle in &report.bundles {
        println!(
            "Profile {}: {} ({}) expires {}",
            bundle.bundle_id,
            bundle.profile_name,
            bundle.profile_uuid,
            bundle.expiration.as_deref().unwrap_or("unknown")
        );
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    println!(
        "Signing set '{}' installed at {}",
        installed.name,
        installed.root.display()
    );
    println!("Bundles: {}", installed.bundles.join(", "));
    if let Some(warning) = &installed.warning {
        println!("warning: {warning}");
    } else if installed.password_stored {
        println!("The PKCS#12 password is stored in the OS credential store.");
    }
    EXIT_OK
}

fn resolve_api_key(
    path: Option<&Path>,
    issuer_id: Option<&str>,
    key_id: Option<&str>,
) -> Result<Option<AppStoreConnectClient>, String> {
    let path = path
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("IMPRESARIO_APPLE_API_KEY").map(PathBuf::from));
    let issuer_id = issuer_id
        .map(str::to_string)
        .or_else(|| std::env::var("IMPRESARIO_APPLE_API_ISSUER_ID").ok());
    let key_id = key_id
        .map(str::to_string)
        .or_else(|| std::env::var("IMPRESARIO_APPLE_API_KEY_ID").ok());
    match (path, issuer_id, key_id) {
        (None, None, None) => Ok(None),
        (Some(path), Some(issuer_id), Some(key_id)) => {
            AppStoreConnectClient::from_file(issuer_id, key_id, &path)
                .map(Some)
                .map_err(|error| error.to_string())
        }
        _ => Err(
            "an App Store Connect API key needs --api-key, --issuer-id and --key-id \
             (or IMPRESARIO_APPLE_API_KEY, IMPRESARIO_APPLE_API_ISSUER_ID and \
             IMPRESARIO_APPLE_API_KEY_ID)"
                .to_string(),
        ),
    }
}

fn resolve_api_team_id(team: Option<&str>) -> Result<String, String> {
    team.map(str::trim)
        .filter(|team| !team.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("IMPRESARIO_APPLE_TEAM_ID")
                .ok()
                .map(|team| team.trim().to_string())
                .filter(|team| !team.is_empty())
        })
        .ok_or_else(|| {
            "an App Store Connect API key needs the Apple Developer team id; pass --team \
             GR2NDRW99B or set IMPRESARIO_APPLE_TEAM_ID"
                .to_string()
        })
}

async fn load_developer(
    home: &Home,
    team_id: Option<&str>,
) -> Result<(DeveloperSession, DeveloperTeam), String> {
    let store = AppleStore::new(home.root().join("apple"));
    let mut credentials = store
        .load()
        .map_err(|error| format!("could not read the global Apple account: {error}"))?
        .ok_or_else(|| {
            "no Apple account is stored; run `impresario account login` first".to_string()
        })?;
    let mut session = load_session(&mut credentials, &store, store.anisette_storage())
        .await
        .map_err(|error| {
            format!(
                "could not use the stored Apple session: {error}; run \
                 `impresario account login --force` if Apple invalidated it"
            )
        })?;
    let teams = session
        .list_teams()
        .await
        .map_err(|error| format!("could not list developer teams: {error}"))?;
    let selected = team_id.unwrap_or(&credentials.team_id);
    let team = choose_team(&teams, Some(selected)).map_err(|error| error.to_string())?;
    Ok((session, team))
}

fn load_existing_identity(
    args: &ProvisionArgs,
    paths: &ApplePaths,
) -> Result<Option<ExistingIdentity>, String> {
    if args.new_identity {
        return Ok(None);
    }
    if let Some(path) = &args.identity {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("read PKCS#12 {}: {error}", path.display()))?;
        let password = read_password(false)?;
        return ExistingIdentity::from_pkcs12(bytes, password)
            .map(Some)
            .map_err(|error| error.to_string());
    }
    let directory = signing_store::set_dir(&paths.signing_dir(), &args.name);
    if !directory.is_dir() {
        return Ok(None);
    }
    let set = SigningSet::load(&paths.signing_dir(), &args.name)
        .map_err(|error| format!("load signing set '{}': {error}", args.name))?;
    let bytes = std::fs::read(set.identity_p12()).map_err(|error| {
        format!(
            "read existing identity {}: {error}",
            set.identity_p12().display()
        )
    })?;
    let password = set.password().map_err(|error| error.to_string())?;
    ExistingIdentity::from_pkcs12(bytes, password)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn provision_device_name(explicit: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| IMPRESARIO_DEVICE_NAME.to_string(), str::to_string)
}

fn provision_bundle_ids(products: &ProductSet, with_stub: bool) -> Result<Vec<String>, String> {
    let host = products
        .host_bundle_id()
        .map_err(|error| error.to_string())?;
    let stub = if with_stub {
        Some(
            products
                .stub_bundle_id()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    "the product set has no stub app; import an agent build with a stub or drop \
                     --with-stub"
                        .to_string()
                })?,
        )
    } else {
        None
    };
    let runner = products
        .runner_bundle_id()
        .map_err(|error| error.to_string())?;
    let nested = if products.nested_relative_path().is_some() {
        Some(
            products
                .nested_bundle_id()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "the runner has a nested .xctest without a bundle id".to_string())?,
        )
    } else {
        None
    };
    Ok(selected_provision_bundle_ids(
        &host,
        stub.as_deref(),
        &runner,
        nested.as_deref(),
        with_stub,
    ))
}

fn selected_provision_bundle_ids(
    host: &str,
    stub: Option<&str>,
    runner: &str,
    nested: Option<&str>,
    with_stub: bool,
) -> Vec<String> {
    let mut ids = Vec::new();
    for id in [Some(host), stub.filter(|_| with_stub), Some(runner), nested]
        .into_iter()
        .flatten()
    {
        if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

fn prompt_revocation(certificates: &[CertificateSummary]) -> Result<Vec<String>, String> {
    print_certificates(certificates);
    print!("Serial(s) to revoke (comma-separated), or Enter to cancel: ");
    std::io::stdout()
        .flush()
        .map_err(|error| format!("could not write the prompt: {error}"))?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("could not read the certificate serial: {error}"))?;
    let selected = line
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(
            "no certificate was selected; pass --revoke <serial> when running non-interactively"
                .to_string(),
        );
    }
    for serial in &selected {
        if !certificates
            .iter()
            .any(|certificate| certificate.serial_number == *serial)
        {
            return Err(format!(
                "{serial} is not one of the listed development certificates"
            ));
        }
    }
    Ok(selected)
}

fn print_certificates(certificates: &[CertificateSummary]) {
    println!("Development certificates:");
    for certificate in certificates {
        println!(
            "  {}  status {}  expires {}  machine {}  name {}",
            certificate.serial_number,
            certificate.status.as_deref().unwrap_or("unknown"),
            certificate.expiration.as_deref().unwrap_or("unknown"),
            certificate.machine_name.as_deref().unwrap_or("unknown"),
            certificate.name.as_deref().unwrap_or("unnamed")
        );
    }
}

fn default_machine_name() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "impresario".to_string()
}

fn run_ddi(command: &DdiCommand, paths: &ApplePaths) -> i32 {
    match command {
        DdiCommand::Import { source, build } => match import_ddi(source, build, paths) {
            Ok(destination) => {
                println!(
                    "Imported DDI for build {build} into {}",
                    destination.display()
                );
                EXIT_OK
            }
            Err(error) => {
                eprintln!("error: {error}");
                EXIT_FAILURE
            }
        },
        DdiCommand::Status => {
            let names = match set_names(&paths.ddi_dir()) {
                Ok(names) => names,
                Err(error) => {
                    eprintln!("error: {error}");
                    return EXIT_INFRA;
                }
            };
            if names.is_empty() {
                println!("No DDI builds imported. Run `impresario apple ddi import <dir> --build <build>`.");
            }
            for name in names {
                let dir = paths.ddi_build_dir(&name);
                match DdiAssets::from_dir(&dir) {
                    Ok(_) => println!("{name}: complete ({})", dir.display()),
                    Err(error) => println!("{name}: incomplete ({error})"),
                }
            }
            EXIT_OK
        }
    }
}

fn run_session(args: &SessionArgs, paths: &ApplePaths) -> i32 {
    let pairing = match load_pairing(paths, args.udid.as_deref()) {
        Ok(pairing) => pairing,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let signing = match SigningSet::load(&paths.signing_dir(), &args.signing) {
        Ok(set) => set,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let products = match ProductSet::load(&paths.product_set_dir(&args.products)) {
        Ok(set) => set,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    let ddi = match args.ddi.as_deref() {
        Some(dir) => match DdiAssets::from_dir(dir) {
            Ok(assets) => Some(assets),
            Err(error) => {
                eprintln!("error: {error}");
                return EXIT_FAILURE;
            }
        },
        None => None,
    };
    let options = SessionOptions {
        host: args.host.clone(),
        pairing,
        paths: paths.clone(),
        products,
        signing,
        ddi,
        app: args.app.clone(),
        app_bundle_id: args.app_bundle_id.clone(),
        install_stub: args.with_stub,
        expected_udid: args.udid.clone(),
        runner: RunnerOptions {
            agent_port: args.agent_port,
            startup_timeout: Duration::from_secs(args.startup_timeout),
            ..RunnerOptions::default()
        },
    };
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(code) => return code,
    };
    println!("Preparing the Apple TV at {}...", args.host);
    let mut prepared = match runtime.block_on(session::prepare(options)) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILURE;
        }
    };
    for install in prepared.installs() {
        println!(
            "Installed {} ({} bytes{}).",
            install.bundle_id,
            install.ipa_bytes,
            install
                .version
                .as_deref()
                .map(|version| format!(", version {version}"))
                .unwrap_or_default()
        );
    }
    println!("Agent endpoint: {}", prepared.endpoint());
    println!("Press Ctrl-C to stop XCUITest and close the tunnels.");
    match runtime.block_on(prepared.wait()) {
        Ok(()) => {
            println!("XCUITest finished.");
            EXIT_OK
        }
        Err(error) => {
            eprintln!("error: {error}");
            EXIT_FAILURE
        }
    }
}

fn import_ddi(source: &Path, build: &str, paths: &ApplePaths) -> Result<PathBuf, String> {
    let assets = DdiAssets::from_dir(source).map_err(|error| error.to_string())?;
    let destination = paths.ddi_build_dir(build);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let temporary = destination.with_file_name(format!(
        ".ddi-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    let result = (|| {
        for source_file in [&assets.image, &assets.trust_cache, &assets.build_manifest] {
            let name = source_file
                .file_name()
                .ok_or_else(|| format!("{} has no file name", source_file.display()))?;
            std::fs::copy(source_file, temporary.join(name))
                .map_err(|error| format!("copy {}: {error}", source_file.display()))?;
        }
        DdiAssets::from_dir(&temporary).map_err(|error| error.to_string())?;
        if destination.exists() {
            std::fs::remove_dir_all(&destination)
                .map_err(|error| format!("replace {}: {error}", destination.display()))?;
        }
        std::fs::rename(&temporary, &destination)
            .map_err(|error| format!("move {}: {error}", temporary.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result.map(|()| destination)
}

fn print_pairing(summary: &PairingSummary, paths: &ApplePaths) {
    println!(
        "Paired {} with {}.",
        summary.name.as_deref().unwrap_or("Apple TV"),
        paths.pairing_path(&summary.saved_as).display()
    );
    if let Some(model) = &summary.model {
        println!("Model: {model}");
    }
    if let Some(udid) = &summary.udid {
        println!("UDID: {udid}");
    }
    println!("Pairing key: {}", summary.saved_as);
}

fn print_products(set: &ProductSet) {
    println!("Products: {}", set.root().display());
    match set.host_bundle_id() {
        Ok(id) => println!("  host:   {id}"),
        Err(error) => println!("  host:   error: {error}"),
    }
    match set.stub_bundle_id() {
        Ok(Some(id)) => println!("  stub:   {id}"),
        Ok(None) => println!("  stub:   none"),
        Err(error) => println!("  stub:   error: {error}"),
    }
    match set.runner_bundle_id() {
        Ok(id) => println!("  tests:  {id}"),
        Err(error) => println!("  tests:  error: {error}"),
    }
    if let Some(relative) = set.nested_relative_path() {
        match set.nested_bundle_id() {
            Ok(Some(id)) => println!("  tests:  {id} ({relative})"),
            Ok(None) => println!("  tests:  none"),
            Err(error) => println!("  tests:  error: {error}"),
        }
    }
}

fn product_bundle_ids(products: &ProductSet) -> Vec<(String, bool)> {
    let mut ids: Vec<(String, bool)> = Vec::new();
    for (id, required) in [
        (products.host_bundle_id().ok(), true),
        (products.stub_bundle_id().ok().flatten(), false),
        (products.runner_bundle_id().ok(), true),
        (products.nested_bundle_id().ok().flatten(), true),
    ] {
        if let Some(id) = id {
            if !ids.iter().any(|(existing, _)| *existing == id) {
                ids.push((id, required));
            }
        }
    }
    ids
}

fn move_pairing(paths: &ApplePaths, from: &str, to: &str) -> Result<(), Error> {
    let file = paths.load_pairing(from)?;
    paths.save_pairing(to, &file)?;
    let _ = std::fs::remove_file(paths.pairing_path(from));
    Ok(())
}

fn resolve_pairing_key(paths: &ApplePaths, udid: Option<&str>) -> Result<String, String> {
    if let Some(udid) = udid.map(str::trim).filter(|value| !value.is_empty()) {
        paths
            .load_pairing(udid)
            .map_err(|error| format!("no pairing stored under {udid}: {error}"))?;
        return Ok(udid.to_string());
    }
    let keys = pairing_keys(paths).map_err(|error| error.to_string())?;
    match keys.as_slice() {
        [] => Err(
            "no Apple pairing is stored; run `impresario apple pair --host <host>` first"
                .to_string(),
        ),
        [key] => Ok(key.clone()),
        _ => Err(format!(
            "several Apple pairings are stored ({}); pass --udid",
            keys.join(", ")
        )),
    }
}

fn load_pairing(
    paths: &ApplePaths,
    udid: Option<&str>,
) -> Result<impresario_apple_device::RpPairingFile, Error> {
    if let Some(udid) = udid {
        return paths.load_pairing(udid).map_err(|error| {
            Error::Device(format!(
                "no pairing stored under {udid}: {error}; run `impresario apple pair --host <host>`"
            ))
        });
    }
    let keys = pairing_keys(paths)?;
    match keys.as_slice() {
        [] => Err(Error::Device(
            "no Apple pairing is stored; run `impresario apple pair --host <host>`".to_string(),
        )),
        [key] => paths.load_pairing(key),
        _ => Err(Error::Device(format!(
            "several Apple pairings are stored ({}); pass --udid",
            keys.join(", ")
        ))),
    }
}

fn pairing_keys(paths: &ApplePaths) -> Result<Vec<String>, Error> {
    let dir = paths.pairing_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|error| Error::io(format!("read {}", dir.display()), error))?
    {
        let entry = entry.map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("plist") {
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                keys.push(stem.to_string());
            }
        }
    }
    keys.sort();
    Ok(keys)
}

fn set_names(dir: &Path) -> Result<Vec<String>, Error> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)
        .map_err(|error| Error::io(format!("read {}", dir.display()), error))?
    {
        let entry = entry.map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
        if entry
            .file_type()
            .map_err(|error| Error::io("stat Apple directory", error))?
            .is_dir()
        {
            if let Some(name) = entry.file_name().to_str() {
                if !name.starts_with('.') {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    Ok(names)
}

fn read_password(confirm: bool) -> Result<Zeroizing<String>, String> {
    if let Ok(value) = std::env::var(PASSWORD_ENV) {
        return Ok(Zeroizing::new(value));
    }
    let password = rpassword::prompt_password("PKCS#12 password: ")
        .map_err(|error| format!("could not read the PKCS#12 password: {error}"))?;
    if password.is_empty() {
        return Err("the PKCS#12 password is empty".to_string());
    }
    if confirm {
        let again = rpassword::prompt_password("Confirm PKCS#12 password: ")
            .map_err(|error| format!("could not read the PKCS#12 password: {error}"))?;
        if password != again {
            return Err("the PKCS#12 passwords do not match".to_string());
        }
    }
    Ok(Zeroizing::new(password))
}

fn runtime() -> Result<tokio::runtime::Runtime, i32> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            eprintln!("error: could not start the async runtime: {error}");
            EXIT_INFRA
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_env_is_returned_without_echo() {
        let key = PASSWORD_ENV;
        let previous = std::env::var_os(key);
        std::env::set_var(key, "secret-value");
        let password = read_password(false).unwrap();
        assert_eq!(&*password, "secret-value");
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn provisioning_defaults_to_the_impresario_device_name() {
        assert_eq!(provision_device_name(None), "impresario");
        assert_eq!(provision_device_name(Some("  ")), "impresario");
        assert_eq!(provision_device_name(Some("  stage-tv  ")), "stage-tv");
    }

    #[test]
    fn provisioning_excludes_the_stub_unless_requested() {
        let host = "com.impresario.agent";
        let stub = "com.impresario.stub";
        let runner = "com.impresario.agent.tests";
        let nested = "com.impresario.agent.tests.xctrunner";
        assert_eq!(
            selected_provision_bundle_ids(host, Some(stub), runner, Some(nested), false),
            vec![host, runner, nested]
        );
        assert_eq!(
            selected_provision_bundle_ids(host, Some(stub), runner, Some(nested), true),
            vec![host, stub, runner, nested]
        );
    }

    #[test]
    fn provisioning_bundle_ids_are_deduplicated() {
        assert_eq!(
            selected_provision_bundle_ids(
                "com.impresario.agent",
                Some("com.impresario.agent"),
                "com.impresario.agent.tests",
                None,
                true,
            ),
            vec!["com.impresario.agent", "com.impresario.agent.tests"]
        );
    }
}
