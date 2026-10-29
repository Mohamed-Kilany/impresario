//! `impresario package` and `impresario sign` (TASK-2 6.3, 6.4, 7.3, 7.4).
//!
//! Packaging and signing are pure Rust. No `ares-package`, `tizen`, `openssl`
//! or `key-manager` process is started (D-19). Passwords for imported Tizen
//! profiles are read from the OS keychain, or from the documented environment
//! variables in headless/test sessions; they are never accepted on the command
//! line because process listings would expose them.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use impresario_clients::tizen_cert;
use impresario_clients::tizen_sign;
use impresario_clients::webos_package;
use impresario_core::home::Home;

use crate::cli::{EXIT_INFRA, EXIT_OK, EXIT_USAGE};

#[derive(Debug, Args)]
pub struct PackageArgs {
    #[command(subcommand)]
    pub command: PackageCommand,
}

#[derive(Debug, Subcommand)]
pub enum PackageCommand {
    /// Build a deterministic webOS `.ipk` from an app directory.
    Webos {
        /// Directory containing `appinfo.json`.
        source: PathBuf,
        /// Output file (default: `<source>.ipk` in the current directory).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Build a deterministic unsigned Tizen `.wgt` from a web-app directory.
    Tizen {
        /// Directory containing `config.xml`.
        source: PathBuf,
        /// Output file (default: `<source>.wgt` in the current directory).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Read and validate an existing `.ipk` or `.wgt`.
    Inspect {
        file: PathBuf,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct SignArgs {
    #[command(subcommand)]
    pub command: SignCommand,
}

#[derive(Debug, Subcommand)]
pub enum SignCommand {
    /// Tizen signing profiles and WGT re-signing.
    Tizen {
        #[command(subcommand)]
        command: TizenSignCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TizenSignCommand {
    /// Import, list, show or remove signing profiles.
    Profile {
        #[command(subcommand)]
        command: TizenProfileCommand,
    },
    /// Re-sign a `.wgt` for one TV DUID.
    Package {
        package: PathBuf,
        /// Imported profile name (`impresario sign tizen profile list`).
        #[arg(long)]
        profile: String,
        /// The TV DUID (`0 getduid` is run automatically during `run`).
        #[arg(long)]
        duid: String,
        /// Output file (default: `<package>-signed.wgt` beside the input).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Verify author and distributor signatures and digests.
    Check { package: PathBuf },
}

#[derive(Debug, Subcommand)]
pub enum TizenProfileCommand {
    /// Sign in to Samsung and create a TV author/distributor profile.
    ///
    /// This is the guided alternative to Tizen Studio's Certificate Manager.
    /// It opens Samsung's normal account login in your browser and never asks
    /// for or stores the Samsung password. The generated profile covers the
    /// DUIDs passed with `--duid` (repeat or comma-separate the option).
    Create {
        /// Profile name (default: samsung).
        #[arg(long, default_value = "samsung")]
        name: String,
        /// TV DUID(s), repeatable or comma-separated.
        #[arg(long = "duid", value_delimiter = ',')]
        duid: Vec<String>,
        /// Samsung account email if the login response does not include it.
        #[arg(long)]
        email: Option<String>,
        /// Do not open the browser; print the Samsung login URL.
        #[arg(long)]
        no_browser: bool,
        /// Download fresh Samsung endpoints and CA certificates.
        #[arg(long)]
        refresh: bool,
        /// Override the Samsung Certificate Extension ZIP URL.
        #[arg(long)]
        extension_url: Option<String>,
        /// Seconds to wait for the browser login (default: 300).
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// Import an author and a distributor PKCS#12 profile.
    Add {
        name: String,
        /// Encrypted author PKCS#12 file.
        #[arg(long)]
        author: PathBuf,
        /// Encrypted distributor PKCS#12 file.
        #[arg(long)]
        distributor: PathBuf,
    },
    /// List imported profiles (no secrets are shown).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one imported profile.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Remove a profile, its PKCS#12 files and keychain passwords.
    Remove { name: String },
}

pub fn run_package(args: &PackageArgs) -> i32 {
    match &args.command {
        PackageCommand::Webos { source, output } => {
            let output = output
                .clone()
                .unwrap_or_else(|| default_output(source, "ipk"));
            match webos_package::build_ipk(source, &output) {
                Ok(info) => {
                    println!(
                        "packaged {} ({} {}, {} entries, {} bytes payload)",
                        output.display(),
                        info.id,
                        info.version,
                        info.entry_count,
                        info.installed_size
                    );
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("package webos: {error}");
                    EXIT_USAGE
                }
            }
        }
        PackageCommand::Tizen { source, output } => {
            let output = output
                .clone()
                .unwrap_or_else(|| default_output(source, "wgt"));
            match tizen_sign::build_wgt(source, &output) {
                Ok(info) => {
                    println!(
                        "packaged {} ({} {}, {} entries)",
                        output.display(),
                        info.id,
                        info.version,
                        info.entry_count
                    );
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("package tizen: {error}");
                    EXIT_USAGE
                }
            }
        }
        PackageCommand::Inspect { file, json } => inspect_package(file, *json),
    }
}

pub fn run_sign(home: &Home, args: &SignArgs) -> i32 {
    match &args.command {
        SignCommand::Tizen { command } => match command {
            TizenSignCommand::Profile { command } => run_profile(home, command),
            TizenSignCommand::Package {
                package,
                profile,
                duid,
                output,
            } => sign_package(home, package, profile, duid, output.as_deref()),
            TizenSignCommand::Check { package } => check_signed(package),
        },
    }
}

fn run_profile(home: &Home, command: &TizenProfileCommand) -> i32 {
    match command {
        TizenProfileCommand::Create {
            name,
            duid,
            email,
            no_browser,
            refresh,
            extension_url,
            timeout,
        } => create_samsung_profile(
            home,
            name,
            duid,
            email.as_deref(),
            *no_browser,
            *refresh,
            extension_url.as_deref(),
            *timeout,
        ),
        TizenProfileCommand::Add {
            name,
            author,
            distributor,
        } => {
            let author_password = match std::env::var("IMPRESARIO_TIZEN_AUTHOR_PASSWORD") {
                Ok(value) if !value.is_empty() => value,
                _ => {
                    eprintln!(
                        "sign tizen profile add: set IMPRESARIO_TIZEN_AUTHOR_PASSWORD \
                         (the author PKCS#12 password); it is stored in the OS keychain"
                    );
                    return EXIT_USAGE;
                }
            };
            let distributor_password = match std::env::var("IMPRESARIO_TIZEN_DISTRIBUTOR_PASSWORD")
            {
                Ok(value) if !value.is_empty() => value,
                _ => {
                    eprintln!(
                        "sign tizen profile add: set \
                             IMPRESARIO_TIZEN_DISTRIBUTOR_PASSWORD (the distributor PKCS#12 \
                             password); it is stored in the OS keychain"
                    );
                    return EXIT_USAGE;
                }
            };
            match tizen_sign::import_profile(
                home,
                name,
                author,
                &author_password,
                distributor,
                &distributor_password,
            ) {
                Ok(record) => {
                    println!(
                        "imported Tizen profile '{}' ({} DUID(s), distributor valid until {})",
                        record.name,
                        record.duids.len(),
                        record.distributor_not_after
                    );
                    if record.duids.is_empty() {
                        eprintln!(
                            "warning: the distributor certificate has no URN:tizen:deviceid SAN; \
                             it will not sign for a TV"
                        );
                    }
                    EXIT_OK
                }
                Err(error) => {
                    eprintln!("sign tizen profile add: {error}");
                    EXIT_USAGE
                }
            }
        }
        TizenProfileCommand::List { json } => match tizen_sign::list_profiles(home) {
            Ok(profiles) => {
                if *json {
                    match serde_json::to_string_pretty(&profiles) {
                        Ok(text) => println!("{text}"),
                        Err(error) => {
                            eprintln!("sign tizen profile list: JSON: {error}");
                            return EXIT_INFRA;
                        }
                    }
                } else if profiles.is_empty() {
                    println!("no Tizen signing profiles imported");
                } else {
                    for profile in profiles {
                        println!(
                            "{}\t{} DUID(s)\tauthor until {}\tdistributor until {}",
                            profile.name,
                            profile.duids.len(),
                            profile.author_not_after,
                            profile.distributor_not_after
                        );
                    }
                }
                EXIT_OK
            }
            Err(error) => {
                eprintln!("sign tizen profile list: {error}");
                EXIT_INFRA
            }
        },
        TizenProfileCommand::Show { name, json } => match tizen_sign::show_profile(home, name) {
            Ok(record) => {
                if *json {
                    match serde_json::to_string_pretty(&record) {
                        Ok(text) => println!("{text}"),
                        Err(error) => {
                            eprintln!("sign tizen profile show: JSON: {error}");
                            return EXIT_INFRA;
                        }
                    }
                } else {
                    println!("profile: {}", record.name);
                    println!("author: {}", record.author_subject);
                    println!("  expires: {}", record.author_not_after);
                    println!("distributor: {}", record.distributor_subject);
                    println!("  expires: {}", record.distributor_not_after);
                    println!(
                        "DUIDs: {}",
                        if record.duids.is_empty() {
                            "none".to_string()
                        } else {
                            record.duids.join(", ")
                        }
                    );
                }
                EXIT_OK
            }
            Err(error) => {
                eprintln!("sign tizen profile show: {error}");
                EXIT_USAGE
            }
        },
        TizenProfileCommand::Remove { name } => match tizen_sign::remove_profile(home, name) {
            Ok(()) => {
                println!("removed Tizen signing profile '{name}'");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("sign tizen profile remove: {error}");
                EXIT_USAGE
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn create_samsung_profile(
    home: &Home,
    name: &str,
    duids: &[String],
    email: Option<&str>,
    no_browser: bool,
    refresh: bool,
    extension_url: Option<&str>,
    timeout_seconds: u64,
) -> i32 {
    if timeout_seconds == 0 {
        eprintln!("sign tizen profile create: --timeout must be greater than zero");
        return EXIT_USAGE;
    }
    println!("Samsung TV certificate setup for profile '{name}'");
    println!(
        "This uses Samsung's normal account login in your browser. Impresario never sees or \
         stores your Samsung password."
    );

    let mut duids = duids.to_vec();
    if duids.iter().all(|duid| duid.trim().is_empty()) {
        match prompt_line(
            "TV DUID (from `impresario device check <name>` or the TV's Developer Mode screen): ",
        ) {
            Ok(value) if !value.is_empty() => duids.push(value),
            Ok(_) => {
                eprintln!("sign tizen profile create: a TV DUID is required");
                return EXIT_USAGE;
            }
            Err(error) => {
                eprintln!("sign tizen profile create: cannot read the TV DUID: {error}");
                return EXIT_USAGE;
            }
        }
    }

    let email = match email {
        Some(value) => Some(value.to_string()),
        None => match prompt_line(
            "Samsung account email (leave blank to use the email returned by the login): ",
        ) {
            Ok(value) if !value.is_empty() => Some(value),
            Ok(_) => None,
            Err(error) => {
                eprintln!("sign tizen profile create: cannot read the email: {error}");
                return EXIT_USAGE;
            }
        },
    };

    let options = tizen_cert::CreateOptions {
        profile_name: name.to_string(),
        duids,
        email,
        refresh,
        open_browser: !no_browser,
        extension_url: extension_url.map(str::to_string),
        login_timeout: std::time::Duration::from_secs(timeout_seconds),
    };
    match tizen_cert::create_profile(home, &options) {
        Ok(record) => {
            println!(
                "created Tizen profile '{}' ({} DUID(s), distributor valid until {})",
                record.name,
                record.duids.len(),
                record.distributor_not_after
            );
            println!(
                "next: set auth.profile: {} on the Tizen device, then run the app check or play",
                record.name
            );
            EXIT_OK
        }
        Err(error) => {
            eprintln!("sign tizen profile create: {error}");
            match error {
                tizen_cert::CertError::Network(_)
                | tizen_cert::CertError::Extension(_)
                | tizen_cert::CertError::Io(_)
                | tizen_cert::CertError::Zip(_) => EXIT_INFRA,
                _ => EXIT_USAGE,
            }
        }
    }
}

fn prompt_line(prompt: &str) -> std::io::Result<String> {
    use std::io::Write as _;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut value = String::new();
    std::io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_string())
}

fn sign_package(
    home: &Home,
    package: &Path,
    profile: &str,
    duid: &str,
    output: Option<&Path>,
) -> i32 {
    let cached = match tizen_sign::sign_package_for_device(home, profile, package, duid) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("sign tizen package: {error}");
            return EXIT_USAGE;
        }
    };
    let output = output.map_or_else(
        || {
            let name = package
                .file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("app");
            package.with_file_name(format!("{name}-signed.wgt"))
        },
        Path::to_path_buf,
    );
    if output == package {
        eprintln!("sign tizen package: refusing to overwrite the input package");
        return EXIT_USAGE;
    }
    match std::fs::copy(&cached, &output) {
        Ok(_) => {
            println!(
                "signed {} for DUID {duid} with profile '{profile}' -> {}",
                package.display(),
                output.display()
            );
            EXIT_OK
        }
        Err(error) => {
            eprintln!(
                "sign tizen package: cannot write {}: {error}",
                output.display()
            );
            EXIT_INFRA
        }
    }
}

fn check_signed(package: &Path) -> i32 {
    let bytes = match std::fs::read(package) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("sign tizen check: read {}: {error}", package.display());
            return EXIT_USAGE;
        }
    };
    if let Err(error) = tizen_sign::validate_signed_wgt(&bytes) {
        eprintln!("sign tizen check: {error}");
        return EXIT_USAGE;
    }
    match tizen_sign::inspect_wgt(package) {
        Ok(info) => {
            println!(
                "verified {} ({} {}, {} entries)",
                package.display(),
                info.id,
                info.version,
                info.entry_count
            );
            EXIT_OK
        }
        Err(error) => {
            eprintln!("sign tizen check: signatures verify but config.xml is invalid: {error}");
            EXIT_USAGE
        }
    }
}

fn inspect_package(path: &Path, json: bool) -> i32 {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "ipk" => match webos_package::inspect_ipk(path) {
            Ok(info) => {
                if json {
                    let value = serde_json::json!({
                        "platform": "webos",
                        "id": info.id,
                        "version": info.version,
                        "entry_count": info.entry_count,
                        "installed_size": info.installed_size,
                        "entries": info.entries,
                        "control": info.control,
                    });
                    match serde_json::to_string_pretty(&value) {
                        Ok(text) => println!("{text}"),
                        Err(error) => {
                            eprintln!("package inspect: JSON: {error}");
                            return EXIT_INFRA;
                        }
                    }
                } else {
                    println!("webOS IPK: {} {}", info.id, info.version);
                    println!(
                        "  {} entries, {} bytes payload",
                        info.entry_count, info.installed_size
                    );
                    println!("  control:");
                    for line in info.control.lines() {
                        println!("    {line}");
                    }
                }
                EXIT_OK
            }
            Err(error) => {
                eprintln!("package inspect: {error}");
                EXIT_USAGE
            }
        },
        "wgt" => match tizen_sign::inspect_wgt(path) {
            Ok(info) => {
                if json {
                    let value = serde_json::json!({
                        "platform": "tizen",
                        "id": info.id,
                        "version": info.version,
                        "entry_count": info.entry_count,
                    });
                    match serde_json::to_string_pretty(&value) {
                        Ok(text) => println!("{text}"),
                        Err(error) => {
                            eprintln!("package inspect: JSON: {error}");
                            return EXIT_INFRA;
                        }
                    }
                } else {
                    println!("Tizen WGT: {} {}", info.id, info.version);
                    println!("  {} entries", info.entry_count);
                }
                EXIT_OK
            }
            Err(error) => {
                eprintln!("package inspect: {error}");
                EXIT_USAGE
            }
        },
        _ => {
            eprintln!(
                "package inspect: {} is neither .ipk nor .wgt",
                path.display()
            );
            EXIT_USAGE
        }
    }
}

fn default_output(source: &Path, extension: &str) -> PathBuf {
    let stem = if source.is_dir() {
        source.file_name()
    } else {
        source.file_stem()
    }
    .and_then(|value| value.to_str())
    .filter(|value| !value.is_empty())
    .unwrap_or("app");
    PathBuf::from(format!("{stem}.{extension}"))
}
