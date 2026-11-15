//! Copy the host app's icon into Xcode's UI-test runner.
//!
//! Xcode builds a `-Runner.app` from a generic `XCTRunner.app` shell. A UI-test
//! target's asset catalog is compiled into the nested `.xctest`, never into the
//! outer shell, so the runner keeps Xcode's placeholder icon even when the host
//! app built from the same catalog shows the Impresario mark. The runner is the
//! bundle the device lists while a UI test session runs, so it should carry the
//! product icon too.
//!
//! [`apply_host_icon`] copies the host's compiled icon artifacts and icon
//! `Info.plist` keys into the runner before signing seals it. It is idempotent
//! and leaves runners/hosts without icons untouched.

use std::path::Path;

use crate::{Error, Result};

/// `Info.plist` keys that nominate the bundle's icon. `CFBundleIcons` is iOS,
/// `CFBundleIcons~ipad` the iPad variant, and tvOS uses the same primary-icon
/// dictionary for its layered brand assets.
const ICON_PLIST_KEYS: [&str; 2] = ["CFBundleIcons", "CFBundleIcons~ipad"];

/// Give `runner` the app icon compiled into `host`.
///
/// Returns `true` when at least one artifact or plist entry changed. The host
/// must be the canonical `host.app` of the same product set, which is built
/// from the catalog named by `ASSETCATALOG_COMPILER_APPICON_NAME` in
/// `agents/apple/project.yml`.
pub fn apply_host_icon(host: &Path, runner: &Path) -> Result<bool> {
    for path in [host, runner] {
        if !path.is_dir() {
            return Err(Error::Device(format!(
                "{} is not an app bundle",
                path.display()
            )));
        }
    }
    let copied = copy_icon_artifacts(host, runner)?;
    let plist_changed = copy_icon_plist(host, runner)?;
    Ok(copied || plist_changed)
}

fn copy_icon_artifacts(host: &Path, runner: &Path) -> Result<bool> {
    let entries = std::fs::read_dir(host)
        .map_err(|error| Error::io(format!("read {}", host.display()), error))?;
    let mut changed = false;
    for entry in entries {
        let entry = entry.map_err(|error| Error::io(format!("read {}", host.display()), error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !is_icon_artifact(&name) {
            continue;
        }
        let source = entry.path();
        if !entry
            .file_type()
            .map_err(|error| Error::io(format!("stat {}", source.display()), error))?
            .is_file()
        {
            continue;
        }
        let destination = runner.join(&*name);
        if copy_file_if_changed(&source, &destination)? {
            changed = true;
        }
    }
    Ok(changed)
}

/// Compiled app icons live at the bundle root as `AppIcon*.png` and/or in
/// `Assets.car`, which `CFBundleIconName` resolves through the asset catalog.
fn is_icon_artifact(name: &str) -> bool {
    name == "Assets.car" || (name.starts_with("AppIcon") && name.ends_with(".png"))
}

fn copy_file_if_changed(source: &Path, destination: &Path) -> Result<bool> {
    let source_bytes = std::fs::read(source)
        .map_err(|error| Error::io(format!("read {}", source.display()), error))?;
    if destination.is_file() {
        let existing = std::fs::read(destination)
            .map_err(|error| Error::io(format!("read {}", destination.display()), error))?;
        if existing == source_bytes {
            return Ok(false);
        }
    }
    std::fs::copy(source, destination)
        .map_err(|error| Error::io(format!("copy {}", destination.display()), error))?;
    Ok(true)
}

fn copy_icon_plist(host: &Path, runner: &Path) -> Result<bool> {
    let source_path = host.join("Info.plist");
    let destination_path = runner.join("Info.plist");
    let source = read_plist(&source_path)?;
    let mut destination = read_plist(&destination_path)?;
    let source = source
        .as_dictionary()
        .ok_or_else(|| Error::Device(format!("{} is not a dictionary", source_path.display())))?;
    let destination = destination.as_dictionary_mut().ok_or_else(|| {
        Error::Device(format!(
            "{} is not a dictionary",
            destination_path.display()
        ))
    })?;
    let mut changed = false;
    for key in ICON_PLIST_KEYS {
        let Some(value) = source.get(key) else {
            continue;
        };
        if destination.get(key) == Some(value) {
            continue;
        }
        destination.insert(key.to_string(), value.clone());
        changed = true;
    }
    if changed {
        // Xcode emits binary plists and iOS reads them fastest; keep the format.
        let value = plist::Value::Dictionary(destination.clone());
        value.to_file_binary(&destination_path).map_err(|error| {
            Error::Device(format!("write {}: {error}", destination_path.display()))
        })?;
    }
    Ok(changed)
}

fn read_plist(path: &Path) -> Result<plist::Value> {
    plist::Value::from_file(path)
        .map_err(|error| Error::Device(format!("read {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "impresario-icon-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_info(app: &Path, icon: Option<plist::Value>) {
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String("com.impresario.test".into()),
        );
        info.insert(
            "CFBundleExecutable".into(),
            plist::Value::String("Test".into()),
        );
        if let Some(icon) = icon {
            info.insert("CFBundleIcons".into(), icon);
        }
        plist::Value::Dictionary(info)
            .to_file_binary(app.join("Info.plist"))
            .unwrap();
    }

    #[test]
    fn copies_compiled_icon_artifacts_and_plist_entries() {
        let root = temp_root("copy");
        let host = root.join("host.app");
        let runner = root.join("runner.app");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::create_dir_all(&runner).unwrap();
        let mut primary = plist::Dictionary::new();
        primary.insert(
            "CFBundleIconName".into(),
            plist::Value::String("AppIcon".into()),
        );
        let mut icons = plist::Dictionary::new();
        icons.insert(
            "CFBundlePrimaryIcon".into(),
            plist::Value::Dictionary(primary),
        );
        write_info(&host, Some(plist::Value::Dictionary(icons)));
        write_info(&runner, None);
        std::fs::write(host.join("AppIcon60x60@2x.png"), b"png-60").unwrap();
        std::fs::write(host.join("Assets.car"), b"car").unwrap();
        std::fs::write(host.join("ImpresarioAgent"), b"host binary").unwrap();

        assert!(apply_host_icon(&host, &runner).unwrap());
        assert_eq!(
            std::fs::read(runner.join("AppIcon60x60@2x.png")).unwrap(),
            b"png-60"
        );
        assert_eq!(std::fs::read(runner.join("Assets.car")).unwrap(), b"car");
        assert!(!runner.join("ImpresarioAgent").exists());
        let copied = plist::Value::from_file(runner.join("Info.plist")).unwrap();
        let copied = copied.as_dictionary().unwrap();
        assert!(copied.contains_key("CFBundleIcons"));
        assert_eq!(
            copied
                .get("CFBundleIdentifier")
                .and_then(plist::Value::as_string),
            Some("com.impresario.test")
        );
        assert!(!apply_host_icon(&host, &runner).unwrap());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn leaves_a_host_without_icons_and_the_runner_untouched() {
        let root = temp_root("empty");
        let host = root.join("host.app");
        let runner = root.join("runner.app");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::create_dir_all(&runner).unwrap();
        write_info(&host, None);
        write_info(&runner, None);
        let before = std::fs::read(runner.join("Info.plist")).unwrap();

        assert!(!apply_host_icon(&host, &runner).unwrap());
        assert_eq!(std::fs::read(runner.join("Info.plist")).unwrap(), before);

        std::fs::remove_dir_all(root).unwrap();
    }
}
