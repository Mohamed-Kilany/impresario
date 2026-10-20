//! `impresario tools` (TASK-2 section 6.0 rung 3): explicit, checksum-
//! verified managed downloads. Doctor only reports; this command installs.

use std::fmt::Write as _;
use std::io::{IsTerminal, Read, Write as _};
use std::path::Path;
use std::time::Duration;

use clap::{Args, Subcommand};
use impresario_core::home::Home;
use impresario_core::tools::{self, ChromiumArtifact};
use sha2::{Digest, Sha256};

use crate::cli::{EXIT_INFRA, EXIT_OK, EXIT_USAGE};

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    /// List pinned and installed tools.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Download and install a managed tool after explicit consent.
    Install(ToolInstallArgs),
    /// Remove an installed managed tool.
    Remove {
        /// Tool name (`chromium`).
        name: String,
    },
}

#[derive(Debug, Args)]
pub struct ToolInstallArgs {
    /// Tool name (`chromium`).
    pub name: String,
    /// Consent to the download without the interactive prompt (scripts).
    #[arg(long)]
    pub yes: bool,
    /// Reinstall even when the pinned build is already present.
    #[arg(long)]
    pub force: bool,
}

pub fn run(command: &ToolsCommand, home: &Home) -> i32 {
    match command {
        ToolsCommand::List { json } => list(home, *json),
        ToolsCommand::Install(args) => match install(home, args) {
            Ok(()) => EXIT_OK,
            Err(Failure::Usage(message)) => {
                eprintln!("tools install: {message}");
                EXIT_USAGE
            }
            Err(Failure::Infra(message)) => {
                eprintln!("tools install: {message}");
                EXIT_INFRA
            }
        },
        ToolsCommand::Remove { name } => match remove(home, name) {
            Ok(()) => EXIT_OK,
            Err(message) => {
                eprintln!("tools remove: {message}");
                EXIT_USAGE
            }
        },
    }
}

#[derive(Debug)]
enum Failure {
    Usage(String),
    Infra(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure::Infra(message)
    }
}

fn list(home: &Home, json: bool) -> i32 {
    let artifact = tools::chromium_artifact();
    let installed = tools::managed_chromium_binary(home);
    if json {
        let value = serde_json::json!({
            "chromium": match artifact {
                Some(artifact) => serde_json::json!({
                    "version": artifact.version,
                    "platform": artifact.platform,
                    "url": artifact.url,
                    "sha256": artifact.sha256,
                    "size_bytes": artifact.size_bytes,
                    "installed": installed.as_ref().map(|path| path.display().to_string()),
                }),
                None => serde_json::Value::Null,
            }
        });
        match serde_json::to_string_pretty(&value) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("tools list: cannot render JSON: {error}");
                return EXIT_INFRA;
            }
        }
        return EXIT_OK;
    }
    match artifact {
        Some(artifact) => {
            println!(
                "chromium {} ({}) — {} MB",
                artifact.version,
                artifact.platform,
                artifact.size_bytes / 1_000_000
            );
            println!("  url:    {}", artifact.url);
            println!("  sha256: {}", artifact.sha256);
            match installed {
                Some(path) => println!("  installed: {}", path.display()),
                None => println!("  installed: no (run `impresario tools install chromium`)"),
            }
        }
        None => println!(
            "chromium: no managed build for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    }
    EXIT_OK
}

fn install(home: &Home, args: &ToolInstallArgs) -> Result<(), Failure> {
    if args.name != "chromium" {
        return Err(Failure::Usage(format!(
            "unknown tool '{}'; the managed tools are: chromium",
            args.name
        )));
    }
    let artifact = tools::chromium_artifact().ok_or_else(|| {
        Failure::Usage(format!(
            "no managed Chromium build for {}-{}; set CHROME_BIN instead",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let install_dir = tools::chromium_install_dir(home, artifact.platform);
    let binary = install_dir.join(artifact.binary);
    if binary.is_file() && !args.force {
        println!(
            "chromium {} is already installed at {}",
            artifact.version,
            binary.display()
        );
        println!("use --force to reinstall");
        return Ok(());
    }

    println!(
        "managed Chromium: Chrome for Testing {} ({})",
        artifact.version, artifact.platform
    );
    println!("source: {}", artifact.url);
    println!("sha256: {}", artifact.sha256);
    println!("size:   {} MB", artifact.size_bytes / 1_000_000);
    if !args.yes {
        if !std::io::stdin().is_terminal() {
            return Err(Failure::Usage(
                "refusing a non-interactive managed download without --yes".to_string(),
            ));
        }
        print!("Download and install? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| Failure::Infra(format!("cannot read the answer: {error}")))?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("cancelled; nothing was downloaded");
            return Ok(());
        }
    }

    home.ensure_dirs()
        .map_err(|error| Failure::Infra(format!("cannot create the home layout: {error}")))?;
    let downloads = home.cache_dir().join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|error| {
        Failure::Infra(format!(
            "cannot create the download directory {}: {error}",
            downloads.display()
        ))
    })?;
    let archive = downloads.join(format!(
        "chrome-{}-{}.zip",
        artifact.version, artifact.platform
    ));
    download(artifact, &archive)?;
    let actual = sha256_file(&archive)?;
    if !actual.eq_ignore_ascii_case(artifact.sha256) {
        let _ = std::fs::remove_file(&archive);
        return Err(Failure::Infra(format!(
            "checksum mismatch for {}: expected {}, got {}; the archive was deleted",
            artifact.url, artifact.sha256, actual
        )));
    }
    println!("checksum ok");

    let version_dir = install_dir
        .parent()
        .ok_or_else(|| Failure::Infra("invalid install directory".to_string()))?;
    std::fs::create_dir_all(version_dir).map_err(|error| {
        Failure::Infra(format!("cannot create {}: {error}", version_dir.display()))
    })?;
    let staging = version_dir.join(format!(
        "{}.partial-{}",
        artifact.platform,
        std::process::id()
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| {
            Failure::Infra(format!(
                "cannot remove stale staging directory {}: {error}",
                staging.display()
            ))
        })?;
    }
    extract(artifact, &archive, &staging)?;
    let staged_binary = staging.join(artifact.binary);
    if !staged_binary.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Failure::Infra(format!(
            "the archive did not contain {}",
            artifact.binary
        )));
    }
    if install_dir.exists() {
        std::fs::remove_dir_all(&install_dir).map_err(|error| {
            Failure::Infra(format!("cannot replace {}: {error}", install_dir.display()))
        })?;
    }
    std::fs::rename(&staging, &install_dir).map_err(|error| {
        let _ = std::fs::remove_dir_all(&staging);
        Failure::Infra(format!(
            "cannot move the extracted browser into {}: {error}",
            install_dir.display()
        ))
    })?;
    let _ = std::fs::remove_file(&archive);
    println!(
        "installed Chrome for Testing {} at {}",
        artifact.version,
        install_dir.join(artifact.binary).display()
    );
    println!("run `impresario doctor` to confirm the web platform is ready");
    Ok(())
}

fn download(artifact: &ChromiumArtifact, destination: &Path) -> Result<(), Failure> {
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .build()
        .get(artifact.url)
        .call()
        .map_err(|error| Failure::Infra(format!("cannot download {}: {error}", artifact.url)))?;
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(destination).map_err(|error| {
        Failure::Infra(format!("cannot create {}: {error}", destination.display()))
    })?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            Failure::Infra(format!("download of {} failed: {error}", artifact.url))
        })?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).ok_or_else(|| {
            Failure::Infra("download returned an invalid chunk length".to_string())
        })?;
        file.write_all(chunk).map_err(|error| {
            Failure::Infra(format!("cannot write {}: {error}", destination.display()))
        })?;
        total += read as u64;
    }
    file.flush().map_err(|error| {
        Failure::Infra(format!("cannot flush {}: {error}", destination.display()))
    })?;
    if total == 0 {
        return Err(Failure::Infra(format!(
            "{} returned an empty archive",
            artifact.url
        )));
    }
    println!("downloaded {} MB", total / 1_000_000);
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, Failure> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| Failure::Infra(format!("cannot read {}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| Failure::Infra(format!("cannot read {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        let chunk = buffer.get(..read).ok_or_else(|| {
            Failure::Infra("checksum read returned an invalid chunk length".to_string())
        })?;
        hasher.update(chunk);
    }
    let digest = hasher.finalize();
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

/// Chrome's Windows sandbox executes a helper in an AppContainer. A browser
/// unpacked from the Chrome for Testing archive does not get the two
/// AppContainer ACEs that the MSI adds, so the helper cannot read `chrome.exe`
/// ("Sandbox cannot access executable"). Grant the same read/execute ACEs on
/// the staging directory before extraction; children inherit them.
#[cfg(windows)]
fn grant_sandbox_access(destination: &Path) -> Result<(), Failure> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{LocalFree, WIN32_ERROR};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
        EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
        TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    };

    const ERROR_SUCCESS: WIN32_ERROR = 0;
    // FILE_GENERIC_READ | FILE_GENERIC_EXECUTE, matching `icacls (RX)`.
    const READ_EXECUTE: u32 = 0x0012_00a9;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let mut path = destination.as_os_str().encode_wide().collect::<Vec<_>>();
    path.push(0);

    let mut old_dacl: *mut ACL = std::ptr::null_mut();
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let mut app_sid: PSID = std::ptr::null_mut();
    let mut restricted_sid: PSID = std::ptr::null_mut();
    let app_sid_text = wide("S-1-15-2-1");
    let restricted_sid_text = wide("S-1-15-2-2");

    let result = (|| {
        let status = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut old_dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "cannot read the ACL of {} (Windows error {status})",
                destination.display()
            ));
        }
        if unsafe { ConvertStringSidToSidW(app_sid_text.as_ptr(), &mut app_sid) } == 0 {
            return Err(format!(
                "cannot build the AppContainer SID for {}",
                destination.display()
            ));
        }
        if unsafe { ConvertStringSidToSidW(restricted_sid_text.as_ptr(), &mut restricted_sid) } == 0
        {
            return Err(format!(
                "cannot build the restricted AppContainer SID for {}",
                destination.display()
            ));
        }
        let entries = [
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: READ_EXECUTE,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: app_sid.cast(),
                },
            },
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: READ_EXECUTE,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: restricted_sid.cast(),
                },
            },
        ];
        let status = unsafe {
            SetEntriesInAclW(
                entries.len() as u32,
                entries.as_ptr(),
                old_dacl,
                &mut new_dacl,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "cannot build the browser ACL for {} (Windows error {status})",
                destination.display()
            ));
        }
        let status = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "cannot grant the Windows sandbox access to {} (Windows error {status})",
                destination.display()
            ));
        }
        Ok(())
    })();

    unsafe {
        if !new_dacl.is_null() {
            LocalFree(new_dacl.cast());
        }
        if !descriptor.is_null() {
            LocalFree(descriptor);
        }
        if !app_sid.is_null() {
            LocalFree(app_sid);
        }
        if !restricted_sid.is_null() {
            LocalFree(restricted_sid);
        }
    }
    result.map_err(Failure::Infra)
}

#[cfg(not(windows))]
fn grant_sandbox_access(_destination: &Path) -> Result<(), Failure> {
    Ok(())
}

fn extract(
    artifact: &ChromiumArtifact,
    archive_path: &Path,
    destination: &Path,
) -> Result<(), Failure> {
    let file = std::fs::File::open(archive_path).map_err(|error| {
        Failure::Infra(format!("cannot open {}: {error}", archive_path.display()))
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        Failure::Infra(format!(
            "cannot read the zip archive {}: {error}",
            archive_path.display()
        ))
    })?;
    std::fs::create_dir_all(destination).map_err(|error| {
        Failure::Infra(format!("cannot create {}: {error}", destination.display()))
    })?;
    grant_sandbox_access(destination)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            Failure::Infra(format!(
                "cannot read archive entry {index} of {}: {error}",
                archive_path.display()
            ))
        })?;
        let Some(name) = entry.enclosed_name() else {
            let _ = std::fs::remove_dir_all(destination);
            return Err(Failure::Infra(format!(
                "archive entry {:?} escapes the extraction root",
                entry.name()
            )));
        };
        let Ok(relative) = name.strip_prefix(artifact.archive_root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let out = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|error| {
                Failure::Infra(format!("cannot create {}: {error}", out.display()))
            })?;
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            let _ = std::fs::remove_dir_all(destination);
            return Err(Failure::Infra(format!(
                "archive entry {:?} is a symbolic link; refusing to extract it",
                entry.name()
            )));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Failure::Infra(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        let mut output = std::fs::File::create(&out)
            .map_err(|error| Failure::Infra(format!("cannot create {}: {error}", out.display())))?;
        std::io::copy(&mut entry, &mut output).map_err(|error| {
            Failure::Infra(format!("cannot extract {}: {error}", out.display()))
        })?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

fn remove(home: &Home, name: &str) -> Result<(), String> {
    if name != "chromium" {
        return Err(format!(
            "unknown tool '{name}'; the managed tools are: chromium"
        ));
    }
    let Some(artifact) = tools::chromium_artifact() else {
        return Err(format!(
            "no managed Chromium build for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    };
    let install_dir = tools::chromium_install_dir(home, artifact.platform);
    if !install_dir.exists() {
        println!(
            "chromium {} is not installed at {}",
            artifact.version,
            install_dir.display()
        );
        return Ok(());
    }
    std::fs::remove_dir_all(&install_dir)
        .map_err(|error| format!("cannot remove {}: {error}", install_dir.display()))?;
    println!("removed {}", install_dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_of_empty_bytes_matches_sha256() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty");
        std::fs::write(&path, b"").expect("write");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let home = Home::at(std::env::temp_dir().join("impresario-tools-test"));
        let args = ToolInstallArgs {
            name: "not-a-tool".to_string(),
            yes: true,
            force: false,
        };
        assert!(matches!(install(&home, &args), Err(Failure::Usage(_))));
    }
}
