//! Deterministic webOS `.ipk` packaging and inspection (TASK-2 6.3).
//!
//! The layout follows `@webosose/ares-cli`'s packager:
//! `debian-binary`, `control.tar.gz` with a Debian control file, and
//! `data.tar.gz` containing
//! `usr/palm/applications/<id>/...` plus
//! `usr/palm/packages/<id>/packageinfo.json`.
//!
//! Every archive metadata value is fixed (mtime zero, uid/gid zero, sorted
//! entries), so packaging the same directory twice produces byte-identical
//! output. No external `ares-package`, `ar`, `tar` or `gzip` process is used
//! (D-19).

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use ar::Header;
use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tar::{Builder, Header as TarHeader};

/// Metadata read back from an `.ipk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpkInfo {
    pub id: String,
    pub version: String,
    /// Number of regular files in `data.tar.gz`.
    pub entry_count: usize,
    /// Uncompressed size of the payload, as written to `Installed-Size`.
    pub installed_size: u64,
    /// File paths inside `data.tar.gz`, sorted.
    pub entries: Vec<String>,
    /// The control file text.
    pub control: String,
}

/// Packaging failures, including deliberately refused unsafe paths.
#[derive(Debug, thiserror::Error)]
pub enum IpkError {
    #[error("webOS package: {0}")]
    Package(String),
    #[error("webOS package i/o: {0}")]
    Io(#[from] std::io::Error),
}

/// Build `source` (a directory containing `appinfo.json`) into an `.ipk`.
///
/// File names are relative and sorted. Symlinks are refused because an `.ipk`
/// that follows them can escape the staging directory on the host.
pub fn build_ipk(source: &Path, output: &Path) -> Result<IpkInfo, IpkError> {
    if !source.is_dir() {
        return Err(IpkError::Package(format!(
            "{} is not a directory containing appinfo.json",
            source.display()
        )));
    }
    let appinfo_path = source.join("appinfo.json");
    let appinfo_text = std::fs::read_to_string(&appinfo_path)
        .map_err(|error| IpkError::Package(format!("read {}: {error}", appinfo_path.display())))?;
    let appinfo: Value = serde_json::from_str(&appinfo_text)
        .map_err(|error| IpkError::Package(format!("appinfo.json: {error}")))?;
    let object = appinfo
        .as_object()
        .ok_or_else(|| IpkError::Package("appinfo.json must be a JSON object".to_string()))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| IpkError::Package("appinfo.json has no id".to_string()))?;
    validate_id(id)?;
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("1.0.0");
    validate_version(version)?;

    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if !files.iter().any(|(name, _)| name == "appinfo.json") {
        return Err(IpkError::Package(
            "source must contain appinfo.json at its root".to_string(),
        ));
    }

    let packageinfo = format!(
        "{{\n  \"id\": {},\n  \"version\": {},\n  \"app\": {}\n}}\n",
        json_string(id),
        json_string(version),
        json_string(id)
    );
    let mut payload = Vec::new();
    for (_, path) in &files {
        payload.extend_from_slice(&std::fs::read(path)?);
    }
    payload.extend_from_slice(packageinfo.as_bytes());

    let control = format!(
        "Package: {id}\n\
         Version: {version}\n\
         Section: misc\n\
         Priority: optional\n\
         Architecture: all\n\
         Installed-Size: {}\n\
         Maintainer: N/A <nobody@example.com>\n\
         Description: This is a webOS application.\n\
         webOS-Package-Format-Version: 2\n\
         webOS-Packager-Version: impresario\n",
        payload.len()
    );

    let data = build_data_tar(&files, id, &packageinfo)?;
    let control_tar = build_control_tar(&control)?;
    let data_gz = gzip(&data)?;
    let control_gz = gzip(&control_tar)?;

    let mut archive = Vec::new();
    {
        let mut builder = ar::Builder::new(&mut archive);
        append_ar(&mut builder, b"debian-binary", b"2.0\n")?;
        append_ar(&mut builder, b"control.tar.gz", &control_gz)?;
        append_ar(&mut builder, b"data.tar.gz", &data_gz)?;
    }
    write_atomic(output, &archive)?;

    let entries = files
        .iter()
        .map(|(name, _)| format!("usr/palm/applications/{id}/{name}"))
        .chain(std::iter::once(format!(
            "usr/palm/packages/{id}/packageinfo.json"
        )))
        .collect::<Vec<_>>();
    Ok(IpkInfo {
        id: id.to_string(),
        version: version.to_string(),
        entry_count: entries.len(),
        installed_size: payload.len() as u64,
        entries,
        control,
    })
}

/// Read and validate an existing `.ipk` without extracting it to disk.
pub fn inspect_ipk(path: &Path) -> Result<IpkInfo, IpkError> {
    let bytes = std::fs::read(path)?;
    inspect_ipk_bytes(&bytes)
}

/// Read and validate `.ipk` bytes.
pub fn inspect_ipk_bytes(bytes: &[u8]) -> Result<IpkInfo, IpkError> {
    let mut archive = ar::Archive::new(Cursor::new(bytes));
    let mut parts = Vec::new();
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry?;
        let name = String::from_utf8_lossy(entry.header().identifier()).into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        parts.push((name, data));
    }
    let find = |name: &str| {
        parts
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, data)| data)
    };
    let debian = find("debian-binary")
        .ok_or_else(|| IpkError::Package("debian-binary is missing".to_string()))?;
    if debian != b"2.0\n" {
        return Err(IpkError::Package(
            "debian-binary is not the required 2.0 newline".to_string(),
        ));
    }
    let control_gz = find("control.tar.gz")
        .ok_or_else(|| IpkError::Package("control.tar.gz is missing".to_string()))?;
    let data_gz = find("data.tar.gz")
        .ok_or_else(|| IpkError::Package("data.tar.gz is missing".to_string()))?;
    let control_tar = gunzip(control_gz)?;
    let control = read_control(&control_tar)?;
    let data_tar = gunzip(data_gz)?;
    let entries = read_tar_entries(&data_tar)?;

    let mut id = None;
    let mut version = None;
    for line in control.lines() {
        if let Some(value) = line.strip_prefix("Package: ") {
            id = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Version: ") {
            version = Some(value.trim().to_string());
        }
    }
    let id = id.ok_or_else(|| IpkError::Package("control has no Package field".to_string()))?;
    validate_id(&id)?;
    let version =
        version.ok_or_else(|| IpkError::Package("control has no Version field".to_string()))?;
    validate_version(&version)?;
    let installed_size = control
        .lines()
        .find_map(|line| line.strip_prefix("Installed-Size: "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    let expected_packageinfo = format!("usr/palm/packages/{id}/packageinfo.json");
    if !entries.iter().any(|entry| entry == &expected_packageinfo) {
        return Err(IpkError::Package(format!(
            "{expected_packageinfo} is missing"
        )));
    }
    let expected_appinfo = format!("usr/palm/applications/{id}/appinfo.json");
    if !entries.iter().any(|entry| entry == &expected_appinfo) {
        return Err(IpkError::Package(format!("{expected_appinfo} is missing")));
    }
    Ok(IpkInfo {
        id,
        version,
        entry_count: entries.len(),
        installed_size,
        entries,
        control,
    })
}

/// Resolve an app's `package` field. A file is validated and used as-is; a
/// directory is packaged deterministically under `cache_dir/<sha256>.ipk`.
pub fn prepare_package(source: &Path, cache_dir: &Path) -> Result<PathBuf, IpkError> {
    if source.is_file() {
        inspect_ipk(source)?;
        return Ok(source.to_path_buf());
    }
    if !source.is_dir() {
        return Err(IpkError::Package(format!(
            "{} is neither an .ipk file nor an app directory",
            source.display()
        )));
    }
    let key = source_hash(source)?;
    std::fs::create_dir_all(cache_dir)?;
    let output = cache_dir.join(format!("{key}.ipk"));
    if output.is_file() && inspect_ipk(&output).is_ok() {
        return Ok(output);
    }
    let temporary = cache_dir.join(format!(".{key}.{}.tmp", std::process::id()));
    build_ipk(source, &temporary)?;
    if output.exists() {
        std::fs::remove_file(&output)?;
    }
    std::fs::rename(&temporary, &output)?;
    Ok(output)
}

fn source_hash(source: &Path) -> Result<String, IpkError> {
    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (name, path) in files {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        let data = std::fs::read(path)?;
        hasher.update((data.len() as u64).to_le_bytes());
        hasher.update(&data);
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    Ok(output)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), IpkError> {
    let mut children = std::fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(IpkError::Package(format!(
                "refusing to package symlink {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            // Previously built packages are build products, not app content.
            if path.extension().is_some_and(|extension| extension == "ipk") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| IpkError::Package(format!("{} is outside source", path.display())))?
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, path));
        }
    }
    Ok(())
}

fn build_data_tar(
    files: &[(String, PathBuf)],
    id: &str,
    packageinfo: &str,
) -> Result<Vec<u8>, IpkError> {
    let mut builder = Builder::new(Vec::new());
    for directory in [
        "usr".to_string(),
        "usr/palm".to_string(),
        "usr/palm/applications".to_string(),
        format!("usr/palm/applications/{id}"),
        "usr/palm/packages".to_string(),
        format!("usr/palm/packages/{id}"),
    ] {
        append_dir(&mut builder, &directory)?;
    }
    for (name, path) in files {
        let data = std::fs::read(path)?;
        let archive_path = format!("usr/palm/applications/{id}/{name}");
        let mut header = archive_header(0o644);
        header.set_path(&archive_path)?;
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, &archive_path, Cursor::new(data))?;
    }
    let package_path = format!("usr/palm/packages/{id}/packageinfo.json");
    let mut header = archive_header(0o644);
    header.set_path(&package_path)?;
    header.set_size(packageinfo.len() as u64);
    header.set_cksum();
    builder.append_data(&mut header, &package_path, packageinfo.as_bytes())?;
    Ok(builder.into_inner()?)
}

fn build_control_tar(control: &str) -> Result<Vec<u8>, IpkError> {
    let mut builder = Builder::new(Vec::new());
    let mut header = archive_header(0o644);
    header.set_path("./control")?;
    header.set_size(control.len() as u64);
    header.set_cksum();
    builder.append_data(&mut header, "./control", control.as_bytes())?;
    Ok(builder.into_inner()?)
}

fn archive_header(mode: u32) -> TarHeader {
    let mut header = TarHeader::new_ustar();
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("").ok();
    header.set_groupname("").ok();
    header
}

fn append_dir(builder: &mut Builder<Vec<u8>>, path: &str) -> Result<(), IpkError> {
    let mut header = archive_header(0o755);
    let directory = format!("{}/", path.trim_end_matches('/'));
    header.set_path(&directory)?;
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Directory);
    header.set_cksum();
    builder.append_data(&mut header, &directory, std::io::empty())?;
    Ok(())
}

fn append_ar<W: Write>(
    builder: &mut ar::Builder<W>,
    name: &[u8],
    data: &[u8],
) -> Result<(), IpkError> {
    let mut header = Header::new(name.to_vec(), data.len() as u64);
    header.set_mode(0o100644);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    builder.append(&header, data)?;
    Ok(())
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, IpkError> {
    let mut encoder: GzEncoder<Vec<u8>> = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>, IpkError> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output)?;
    Ok(output)
}

fn read_control(tar: &[u8]) -> Result<String, IpkError> {
    let mut archive = tar::Archive::new(Cursor::new(tar));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if path == "./control" || path == "control" {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            return Ok(text);
        }
    }
    Err(IpkError::Package(
        "control.tar.gz has no control file".to_string(),
    ))
}

fn read_tar_entries(tar: &[u8]) -> Result<Vec<String>, IpkError> {
    let mut archive = tar::Archive::new(Cursor::new(tar));
    let mut entries = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.header().entry_type().is_file() {
            entries.push(entry.path()?.to_string_lossy().into_owned());
        }
    }
    entries.sort();
    Ok(entries)
}

fn validate_id(id: &str) -> Result<(), IpkError> {
    if id.is_empty()
        || id.len() > 128
        || !id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '+' | '-')
        })
    {
        return Err(IpkError::Package(format!(
            "invalid webOS package id '{id}' (lowercase letters, digits, '.', '+' and '-' only)"
        )));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), IpkError> {
    if version.is_empty()
        || version.len() > 64
        || !version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '+' | '-' | '_' | '~')
        })
    {
        return Err(IpkError::Package(format!(
            "invalid webOS package version '{version}'"
        )));
    }
    Ok(())
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), IpkError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("assets"))?;
        std::fs::write(
            root.join("appinfo.json"),
            r#"{"id":"com.example.app","version":"1.2.3","title":"Example","main":"index.html"}"#,
        )?;
        std::fs::write(
            root.join("index.html"),
            "<!doctype html><title>Example</title>",
        )?;
        std::fs::write(root.join("assets/app.js"), "console.log('ok');")?;
        Ok(())
    }

    #[test]
    fn packaging_is_deterministic_and_inspectable() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let app = temporary.path().join("app");
        source(&app)?;
        let first = temporary.path().join("one.ipk");
        let second = temporary.path().join("two.ipk");
        let info = build_ipk(&app, &first)?;
        build_ipk(&app, &second)?;
        assert_eq!(std::fs::read(&first)?, std::fs::read(&second)?);
        assert_eq!(info.id, "com.example.app");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(
            info.control,
            format!(
                "Package: com.example.app\n\
                 Version: 1.2.3\n\
                 Section: misc\n\
                 Priority: optional\n\
                 Architecture: all\n\
                 Installed-Size: {}\n\
                 Maintainer: N/A <nobody@example.com>\n\
                 Description: This is a webOS application.\n\
                 webOS-Package-Format-Version: 2\n\
                 webOS-Packager-Version: impresario\n",
                info.installed_size
            )
        );
        let bytes = std::fs::read(&first)?;
        let mut archive = ar::Archive::new(Cursor::new(&bytes));
        let mut members = Vec::new();
        while let Some(entry) = archive.next_entry() {
            members.push(String::from_utf8_lossy(entry?.header().identifier()).into_owned());
        }
        assert_eq!(
            members,
            vec!["debian-binary", "control.tar.gz", "data.tar.gz"]
        );
        let inspected = inspect_ipk(&first)?;
        assert_eq!(inspected.id, info.id);
        assert_eq!(inspected.entries, info.entries);
        assert!(inspected
            .entries
            .contains(&"usr/palm/applications/com.example.app/index.html".to_string()));
        assert!(inspected
            .entries
            .contains(&"usr/palm/packages/com.example.app/packageinfo.json".to_string()));
        Ok(())
    }

    #[test]
    fn prepare_package_caches_directories_and_reuses_ipks() -> Result<(), Box<dyn std::error::Error>>
    {
        let temporary = tempfile::tempdir()?;
        let app = temporary.path().join("app");
        source(&app)?;
        let first = prepare_package(&app, &temporary.path().join("cache"))?;
        let second = prepare_package(&app, &temporary.path().join("cache"))?;
        assert_eq!(first, second);
        assert!(first.is_file());
        let direct = temporary.path().join("direct.ipk");
        build_ipk(&app, &direct)?;
        assert_eq!(
            prepare_package(&direct, &temporary.path().join("other"))?,
            direct
        );
        Ok(())
    }

    #[test]
    fn refuses_traversal_names_and_symlinks() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let app = temporary.path().join("app");
        source(&app)?;
        std::fs::write(
            app.join("appinfo.json"),
            r#"{"id":"../evil","version":"1.0.0"}"#,
        )?;
        assert!(build_ipk(&app, &temporary.path().join("evil.ipk")).is_err());

        std::fs::write(
            app.join("appinfo.json"),
            r#"{"id":"com.example.app","version":"1.0.0"}"#,
        )?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", app.join("leak"))?;
            assert!(build_ipk(&app, &temporary.path().join("symlink.ipk")).is_err());
        }
        Ok(())
    }
}
