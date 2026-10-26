//! AFC upload and InstallationProxy install/uninstall.
//!
//! InstallationProxy installs an `.ipa` already present in the device's AFC
//! jail. Impresario builds the IPA locally with the pure-Rust `zip` crate,
//! uploads it to `/PublicStaging`, asks InstallationProxy to install it, then
//! verifies the bundle id in a Lookup. No `ideviceinstaller` is involved.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use idevice::services::afc::opcode::AfcFopenMode;
use idevice::services::afc::AfcClient;
use idevice::services::installation_proxy::InstallationProxyClient;
use uuid::Uuid;

use crate::tunnel::AppleTunnel;
use crate::{Error, Result};

/// Result of a successful install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub bundle_id: String,
    pub version: Option<String>,
    pub ipa_bytes: u64,
}

/// Read `CFBundleIdentifier` from an app bundle's `Info.plist`.
pub fn bundle_id(app: &Path) -> Result<String> {
    let info = app.join("Info.plist");
    let value = plist::Value::from_file(&info)
        .map_err(|error| Error::Device(format!("read {}: {error}", info.display())))?;
    value
        .as_dictionary()
        .and_then(|dict| dict.get("CFBundleIdentifier"))
        .and_then(plist::Value::as_string)
        .map(str::to_string)
        .ok_or_else(|| Error::Device(format!("{} is missing CFBundleIdentifier", info.display())))
}

/// Install `app` (a `.app` directory) after verifying its bundle id.
pub async fn install_bundle(
    tunnel: &mut AppleTunnel,
    app: &Path,
    expected_bundle_id: Option<&str>,
) -> Result<InstallReport> {
    if !app.is_dir() {
        return Err(Error::Device(format!(
            "{} is not an app bundle directory",
            app.display()
        )));
    }
    let actual = bundle_id(app)?;
    check_expected(&actual, expected_bundle_id, app)?;

    let ipa_path = unique_ipa_path(app)?;
    let _guard = TempIpa(ipa_path.clone());
    let bytes = create_ipa(app, &ipa_path)?;
    install_ipa_file(tunnel, &ipa_path, Some(&actual), bytes).await
}

/// Install a signed `.app` directory or a prebuilt `.ipa`.
pub async fn install_package(
    tunnel: &mut AppleTunnel,
    package: &Path,
    expected_bundle_id: Option<&str>,
) -> Result<InstallReport> {
    if package.is_dir() {
        return install_bundle(tunnel, package, expected_bundle_id).await;
    }
    if !package.is_file() {
        return Err(Error::Device(format!(
            "{} is neither an app bundle nor an IPA",
            package.display()
        )));
    }
    if !package
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipa"))
    {
        return Err(Error::Device(format!(
            "{} is not a .ipa; pass a signed .app directory or a .ipa",
            package.display()
        )));
    }
    let (actual, version) = ipa_bundle_id(package)?;
    check_expected(&actual, expected_bundle_id, package)?;
    let bytes = std::fs::metadata(package)
        .map_err(|error| Error::io(format!("stat {}", package.display()), error))?
        .len();
    let mut report = install_ipa_file(tunnel, package, Some(&actual), bytes).await?;
    if report.version.is_none() {
        report.version = version;
    }
    Ok(report)
}

/// Uninstall the bundle if present, then install `package`.
///
/// A missing install must not silently count as a reset, so the install result
/// is returned and checked by the caller.
pub async fn reset_package(
    tunnel: &mut AppleTunnel,
    package: &Path,
    bundle_id: &str,
) -> Result<InstallReport> {
    if let Err(error) = uninstall_bundle(tunnel, bundle_id).await {
        tracing::debug!(%error, "uninstall before reset was not needed");
    }
    install_package(tunnel, package, Some(bundle_id)).await
}

fn check_expected(actual: &str, expected: Option<&str>, source: &Path) -> Result<()> {
    if let Some(expected) = expected {
        if expected != actual {
            return Err(Error::Device(format!(
                "{} has bundle id {actual}, expected {expected}",
                source.display()
            )));
        }
    }
    Ok(())
}

async fn install_ipa_file(
    tunnel: &mut AppleTunnel,
    ipa_path: &Path,
    actual: Option<&str>,
    bytes: u64,
) -> Result<InstallReport> {
    let actual = match actual {
        Some(actual) => actual.to_string(),
        None => ipa_bundle_id(ipa_path)?.0,
    };
    let remote_name = format!(
        "impresario-{}-{}",
        Uuid::new_v4(),
        ipa_path
            .file_name()
            .ok_or_else(|| Error::Device("IPA path has no file name".into()))?
            .to_string_lossy()
    );
    let remote_path = format!("/PublicStaging/{remote_name}");
    tracing::info!(
        bundle_id = %actual,
        path = %ipa_path.display(),
        bytes,
        remote = %remote_path,
        "uploading app"
    );

    let mut afc: AfcClient = tunnel
        .handshake
        .connect(&mut tunnel.handle)
        .await
        .map_err(|error| Error::Device(format!("AFC service: {error}")))?;
    afc.mk_dir("/PublicStaging").await.ok();
    let mut file = afc
        .open(&remote_path, AfcFopenMode::WrOnly)
        .await
        .map_err(|error| Error::Device(format!("open {remote_path}: {error}")))?;
    let data = tokio::fs::read(ipa_path)
        .await
        .map_err(|error| Error::io(format!("read {}", ipa_path.display()), error))?;
    file.write_entire(&data)
        .await
        .map_err(|error| Error::Device(format!("upload {remote_path}: {error}")))?;
    file.close()
        .await
        .map_err(|error| Error::Device(format!("close {remote_path}: {error}")))?;

    let mut installation: InstallationProxyClient = tunnel
        .handshake
        .connect(&mut tunnel.handle)
        .await
        .map_err(|error| Error::Device(format!("InstallationProxy: {error}")))?;
    installation
        .install(&remote_path, None)
        .await
        .map_err(|error| Error::Device(format!("install {actual}: {error}")))?;

    let installed = installation
        .get_apps(Some("Any"), Some(vec![actual.clone()]))
        .await
        .map_err(|error| Error::Device(format!("verify {actual}: {error}")))?;
    let Some(info) = installed.get(&actual) else {
        return Err(Error::Device(format!(
            "InstallationProxy reported success but {actual} is absent from Lookup"
        )));
    };
    let version = info
        .as_dictionary()
        .and_then(|dict| dict.get("CFBundleShortVersionString"))
        .and_then(plist::Value::as_string)
        .map(str::to_string);
    tracing::info!(bundle_id = %actual, version = ?version, "installed app");
    Ok(InstallReport {
        bundle_id: actual,
        version,
        ipa_bytes: bytes,
    })
}

/// Read `Payload/*.app/Info.plist` from an IPA.
fn ipa_bundle_id(path: &Path) -> Result<(String, Option<String>)> {
    let file = std::fs::File::open(path)
        .map_err(|error| Error::io(format!("open {}", path.display()), error))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| Error::Device(format!("read {}: {error}", path.display())))?;
    let mut info_name = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| Error::Device(format!("read {}: {error}", path.display())))?;
        let name = entry.name().to_string();
        let is_app_info = name.starts_with("Payload/")
            && name.ends_with(".app/Info.plist")
            && name.matches('/').count() == 2;
        if is_app_info && info_name.replace(name).is_some() {
            return Err(Error::Device(format!(
                "{} contains more than one Payload/*.app/Info.plist",
                path.display()
            )));
        }
    }
    let info_name = info_name.ok_or_else(|| {
        Error::Device(format!(
            "{} has no Payload/*.app/Info.plist",
            path.display()
        ))
    })?;
    let mut reader = archive
        .by_name(&info_name)
        .map_err(|error| Error::Device(format!("read {info_name}: {error}")))?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Device(format!("read {info_name}: {error}")))?;
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|error| Error::Device(format!("parse {info_name}: {error}")))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| Error::Device(format!("{info_name} is not a dictionary")))?;
    let bundle_id = dict
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        .ok_or_else(|| Error::Device(format!("{info_name} has no CFBundleIdentifier")))?
        .to_string();
    let version = dict
        .get("CFBundleShortVersionString")
        .and_then(plist::Value::as_string)
        .map(str::to_string);
    Ok((bundle_id, version))
}

/// Uninstall a bundle, tolerating "not installed".
pub async fn uninstall_bundle(tunnel: &mut AppleTunnel, bundle_id: &str) -> Result<()> {
    let mut installation: InstallationProxyClient = tunnel
        .handshake
        .connect(&mut tunnel.handle)
        .await
        .map_err(|error| Error::Device(format!("InstallationProxy: {error}")))?;
    installation
        .uninstall(bundle_id, None)
        .await
        .map_err(|error| Error::Device(format!("uninstall {bundle_id}: {error}")))
}

/// Query whether a bundle is installed and return its short version.
pub async fn installed_version(
    tunnel: &mut AppleTunnel,
    bundle_id: &str,
) -> Result<Option<String>> {
    let mut installation: InstallationProxyClient = tunnel
        .handshake
        .connect(&mut tunnel.handle)
        .await
        .map_err(|error| Error::Device(format!("InstallationProxy: {error}")))?;
    let apps = installation
        .get_apps(Some("Any"), Some(vec![bundle_id.to_string()]))
        .await
        .map_err(|error| Error::Device(format!("lookup {bundle_id}: {error}")))?;
    Ok(apps.get(bundle_id).and_then(|info| {
        info.as_dictionary()
            .and_then(|dict| dict.get("CFBundleShortVersionString"))
            .and_then(plist::Value::as_string)
            .map(str::to_string)
    }))
}

fn unique_ipa_path(app: &Path) -> Result<PathBuf> {
    let stem = app
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "app".to_string());
    let name = format!("{stem}-{}.ipa", Uuid::new_v4());
    let dir = std::env::temp_dir().join("impresario-apple-ipa");
    std::fs::create_dir_all(&dir)
        .map_err(|error| Error::io(format!("create {}", dir.display()), error))?;
    Ok(dir.join(name))
}

/// A temp IPA removed when the install finishes or fails.
struct TempIpa(PathBuf);

impl Drop for TempIpa {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Build an `.ipa` and return its size.
fn create_ipa(app: &Path, output: &Path) -> Result<u64> {
    let file = std::fs::File::create(output)
        .map_err(|error| Error::io(format!("create {}", output.display()), error))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);
    let app_name = app
        .file_name()
        .ok_or_else(|| Error::Device(format!("{} has no file name", app.display())))?
        .to_string_lossy()
        .to_string();
    add_dir_to_zip(&mut zip, app, &format!("Payload/{app_name}"), options)?;
    zip.finish()
        .map_err(|error| Error::Device(format!("finish IPA: {error}")))?;
    let size = std::fs::metadata(output)
        .map_err(|error| Error::io(format!("stat {}", output.display()), error))?
        .len();
    Ok(size)
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<std::fs::File>,
    dir: &Path,
    prefix: &str,
    options: zip::write::SimpleFileOptions,
) -> Result<()> {
    zip.add_directory(prefix, options)?;
    let mut entries = std::fs::read_dir(dir)
        .map_err(|error| Error::io(format!("read {}", dir.display()), error))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| Error::io(format!("read {}", dir.display()), error))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".dSYM") {
            continue;
        }
        let rel = format!("{prefix}/{name}");
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| Error::io(format!("stat {}", path.display()), error))?;
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map_err(|error| Error::io(format!("readlink {}", path.display()), error))?;
            let target = target.to_string_lossy().replace('\\', "/");
            zip.add_symlink(rel, target, options)?;
        } else if metadata.is_dir() {
            add_dir_to_zip(zip, &path, &rel, options)?;
        } else if metadata.is_file() {
            zip.start_file(rel, options)?;
            let data = std::fs::read(&path)
                .map_err(|error| Error::io(format!("read {}", path.display()), error))?;
            zip.write_all(&data)
                .map_err(|error| Error::io(format!("write IPA entry {}", path.display()), error))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_app() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "impresario-apple-install-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let app = dir.join("Demo.app");
        std::fs::create_dir_all(&app).unwrap();
        let info = plist::Value::Dictionary({
            let mut dict = plist::Dictionary::new();
            dict.insert(
                "CFBundleIdentifier".into(),
                plist::Value::String("com.example.demo".into()),
            );
            dict.insert(
                "CFBundleShortVersionString".into(),
                plist::Value::String("1.2.3".into()),
            );
            dict
        });
        info.to_file_xml(app.join("Info.plist")).unwrap();
        std::fs::write(app.join("Demo"), b"binary").unwrap();
        dir
    }

    #[test]
    fn ipa_has_the_expected_payload_and_unique_name() {
        let root = temp_app();
        let app = root.join("Demo.app");
        let ipa = root.join("Demo.ipa");
        let bytes = create_ipa(&app, &ipa).unwrap();
        assert!(bytes > 0);
        let file = std::fs::File::open(&ipa).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut names = Vec::new();
        for index in 0..zip.len() {
            names.push(zip.by_index(index).unwrap().name().to_string());
        }
        assert!(names
            .iter()
            .any(|name| name == "Payload/Demo.app/Info.plist"));
        assert!(names.iter().any(|name| name == "Payload/Demo.app/Demo"));
        assert!(bundle_id(&app).is_ok_and(|id| id == "com.example.demo"));
        let (id, version) = ipa_bundle_id(&ipa).unwrap();
        assert_eq!(id, "com.example.demo");
        assert_eq!(version.as_deref(), Some("1.2.3"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
