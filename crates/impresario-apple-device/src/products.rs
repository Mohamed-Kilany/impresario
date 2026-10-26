//! The three unsigned Impresario agent apps installed on a device.
//!
//! Xcode emits the host app, the fixture stub and the UI-test runner. The
//! product copies them into a machine-global directory and renames the
//! top-level bundles so the host does not have to guess Xcode's names:
//!
//! ```text
//! ~/.impresario/apple/products/current/
//!   host.app
//!   stub.app
//!   runner.app
//! ```
//!
//! Bundle identifiers and the nested `.xctest` path are read from the bundles
//! themselves. Renaming the outer directory does not change the bundle id or
//! the executable that `CFBundleExecutable` names.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::install;
use crate::{Error, Result};

/// The host, stub and runner bundles of one agent build.
#[derive(Debug, Clone)]
pub struct ProductSet {
    root: PathBuf,
    host: PathBuf,
    stub: Option<PathBuf>,
    runner: PathBuf,
    nested_relative_path: Option<String>,
}

impl ProductSet {
    /// Load a directory previously created by [`ProductSet::import`].
    pub fn load(dir: &Path) -> Result<Self> {
        let host = dir.join("host.app");
        let runner = dir.join("runner.app");
        let stub = dir.join("stub.app");
        for path in [&host, &runner] {
            if !path.is_dir() {
                return Err(Error::Device(format!(
                    "{} is missing; import the unsigned Apple agent build with \
                     `impresario apple products import <dir>`",
                    path.display()
                )));
            }
        }
        let nested_relative_path = discover_nested(&runner)?;
        let set = Self {
            root: dir.to_path_buf(),
            host,
            stub: stub.is_dir().then_some(stub),
            runner,
            nested_relative_path,
        };
        set.validate()?;
        Ok(set)
    }

    /// Import Xcode's `Debug-appletvos`/`Debug-iphoneos` output directory.
    ///
    /// Exactly one host, one stub and one `-Runner.app` are required. The
    /// destination is replaced only after the whole source has been classified
    /// and the resulting directory validates.
    pub fn import(source: &Path, destination: &Path) -> Result<Self> {
        if !source.is_dir() {
            return Err(Error::Device(format!(
                "product source {} is not a directory",
                source.display()
            )));
        }
        let mut apps = Vec::new();
        for entry in std::fs::read_dir(source)
            .map_err(|error| Error::io(format!("read {}", source.display()), error))?
        {
            let entry =
                entry.map_err(|error| Error::io(format!("read {}", source.display()), error))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("app") && path.is_dir() {
                apps.push(path);
            }
        }
        apps.sort();
        if apps.is_empty() {
            return Err(Error::Device(format!(
                "{} contains no .app bundles",
                source.display()
            )));
        }

        let mut runner = None;
        let mut stub = None;
        let mut hosts = Vec::new();
        for app in apps {
            let name = app
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let bundle = install::bundle_id(&app)?;
            if name.ends_with("-Runner.app") || bundle.ends_with(".xctrunner") {
                if runner.replace(app).is_some() {
                    return Err(Error::Device(
                        "the source contains more than one XCUITest runner".to_string(),
                    ));
                }
            } else if name.to_ascii_lowercase().contains("stub") {
                if stub.replace(app).is_some() {
                    return Err(Error::Device(
                        "the source contains more than one stub app".to_string(),
                    ));
                }
            } else {
                hosts.push(app);
            }
        }
        let host = match hosts.as_slice() {
            [host] => host.clone(),
            [] => {
                return Err(Error::Device(format!(
                    "{} has no host app (an .app that is not the stub or a -Runner.app)",
                    source.display()
                )));
            }
            _ => {
                return Err(Error::Device(format!(
                    "{} has several possible host apps: {}",
                    source.display(),
                    hosts
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        };
        let runner = runner.ok_or_else(|| {
            Error::Device(format!(
                "{} has no XCUITest runner (a -Runner.app or .xctrunner bundle)",
                source.display()
            ))
        })?;

        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::io(format!("create {}", parent.display()), error))?;
        }
        let temporary = destination.with_file_name(format!(
            ".products-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let guard = RemoveOnDrop(temporary.clone());
        std::fs::create_dir_all(&temporary)
            .map_err(|error| Error::io("create product staging directory", error))?;
        copy_dir_all(&host, &temporary.join("host.app"))?;
        if let Some(stub) = &stub {
            copy_dir_all(stub, &temporary.join("stub.app"))?;
        }
        copy_dir_all(&runner, &temporary.join("runner.app"))?;
        Self::load(&temporary)?;
        if destination.exists() {
            std::fs::remove_dir_all(destination)
                .map_err(|error| Error::io(format!("replace {}", destination.display()), error))?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| {
            Error::io(
                format!("move {} to {}", temporary.display(), destination.display()),
                error,
            )
        })?;
        drop(guard);
        Self::load(destination)
    }

    fn validate(&self) -> Result<()> {
        let host_id = install::bundle_id(&self.host)?;
        let runner_id = install::bundle_id(&self.runner)?;
        if host_id == runner_id {
            return Err(Error::Device(format!(
                "host and runner both use bundle id {host_id}"
            )));
        }
        if let Some(nested) = &self.nested_relative_path {
            let nested_dir = self.runner.join(nested);
            let nested_id = install::bundle_id(&nested_dir)?;
            if nested_id == runner_id {
                return Err(Error::Device(format!(
                    "runner and nested test bundle both use bundle id {runner_id}"
                )));
            }
        }
        Ok(())
    }

    /// The imported product directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The XCTest host app.
    #[must_use]
    pub fn host(&self) -> &Path {
        &self.host
    }

    /// The fixture stub app, when the build produced one.
    #[must_use]
    pub fn stub(&self) -> Option<&Path> {
        self.stub.as_deref()
    }

    /// The UI-test runner app.
    #[must_use]
    pub fn runner(&self) -> &Path {
        &self.runner
    }

    /// Relative path of the nested `.xctest` inside the runner, if any.
    #[must_use]
    pub fn nested_relative_path(&self) -> Option<&str> {
        self.nested_relative_path.as_deref()
    }

    /// The bundle id of the XCUITest host.
    pub fn host_bundle_id(&self) -> Result<String> {
        install::bundle_id(&self.host)
    }

    /// The bundle id of the fixture stub.
    pub fn stub_bundle_id(&self) -> Result<Option<String>> {
        self.stub.as_deref().map(install::bundle_id).transpose()
    }

    /// The bundle id of the XCUITest runner.
    pub fn runner_bundle_id(&self) -> Result<String> {
        install::bundle_id(&self.runner)
    }

    /// The bundle id of the nested `.xctest`, when present.
    pub fn nested_bundle_id(&self) -> Result<Option<String>> {
        self.nested_relative_path
            .as_deref()
            .map(|relative| install::bundle_id(&self.runner.join(relative)))
            .transpose()
    }
}

fn discover_nested(runner: &Path) -> Result<Option<String>> {
    let plugins = runner.join("PlugIns");
    if !plugins.is_dir() {
        return Ok(None);
    }
    let mut nested = Vec::new();
    for entry in std::fs::read_dir(&plugins)
        .map_err(|error| Error::io(format!("read {}", plugins.display()), error))?
    {
        let entry =
            entry.map_err(|error| Error::io(format!("read {}", plugins.display()), error))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry
            .file_type()
            .map_err(|error| Error::io("stat nested bundle", error))?
            .is_dir()
            && name.ends_with(".xctest")
        {
            nested.push(name);
        }
    }
    nested.sort();
    match nested.as_slice() {
        [] => Ok(None),
        [name] => Ok(Some(format!("PlugIns/{name}"))),
        _ => Err(Error::Device(format!(
            "{} contains more than one .xctest bundle: {}",
            plugins.display(),
            nested.join(", ")
        ))),
    }
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)
        .map_err(|error| Error::io(format!("create {}", destination.display()), error))?;
    for entry in std::fs::read_dir(source)
        .map_err(|error| Error::io(format!("read {}", source.display()), error))?
    {
        let entry =
            entry.map_err(|error| Error::io(format!("read {}", source.display()), error))?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| Error::io(format!("stat {}", path.display()), error))?;
        if file_type.is_dir() {
            copy_dir_all(&path, &target)?;
        } else if file_type.is_symlink() {
            #[cfg(unix)]
            {
                let link = std::fs::read_link(&path)
                    .map_err(|error| Error::io(format!("readlink {}", path.display()), error))?;
                std::os::unix::fs::symlink(&link, &target)
                    .map_err(|error| Error::io(format!("symlink {}", target.display()), error))?;
            }
            #[cfg(windows)]
            std::fs::copy(&path, &target)
                .map_err(|error| Error::io(format!("copy {}", target.display()), error))?;
        } else {
            std::fs::copy(&path, &target)
                .map_err(|error| Error::io(format!("copy {}", target.display()), error))?;
        }
    }
    Ok(())
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(root: &Path, name: &str, bundle_id: &str) -> PathBuf {
        let app = root.join(name);
        std::fs::create_dir_all(&app).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String(bundle_id.into()),
        );
        info.insert(
            "CFBundleExecutable".into(),
            plist::Value::String(name.trim_end_matches(".app").into()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(app.join("Info.plist"))
            .unwrap();
        std::fs::write(app.join(name.trim_end_matches(".app")), b"binary").unwrap();
        app
    }

    fn fixture(root: &Path) -> PathBuf {
        app(root, "ImpresarioAgent-tvOS.app", "com.impresario.agent");
        app(root, "ImpresarioAgentStub-tvOS.app", "com.impresario.stub");
        let runner = app(
            root,
            "ImpresarioAgentTests-tvOS-Runner.app",
            "com.impresario.agent.tests.xctrunner",
        );
        let nested = runner
            .join("PlugIns")
            .join("ImpresarioAgentTests-tvOS.xctest");
        std::fs::create_dir_all(&nested).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String("com.impresario.agent.tests".into()),
        );
        info.insert(
            "CFBundleExecutable".into(),
            plist::Value::String("ImpresarioAgentTests".into()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(nested.join("Info.plist"))
            .unwrap();
        root.to_path_buf()
    }

    #[test]
    fn import_classifies_host_stub_runner_and_nested_tests() {
        let root = std::env::temp_dir().join(format!(
            "impresario-products-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let source = root.join("build");
        let destination = root.join("products");
        std::fs::create_dir_all(&source).unwrap();
        fixture(&source);
        let set = ProductSet::import(&source, &destination).unwrap();
        assert_eq!(set.host_bundle_id().unwrap(), "com.impresario.agent");
        assert_eq!(
            set.stub_bundle_id().unwrap().as_deref(),
            Some("com.impresario.stub")
        );
        assert_eq!(
            set.runner_bundle_id().unwrap(),
            "com.impresario.agent.tests.xctrunner"
        );
        assert_eq!(
            set.nested_bundle_id().unwrap().as_deref(),
            Some("com.impresario.agent.tests")
        );
        assert_eq!(
            set.nested_relative_path(),
            Some("PlugIns/ImpresarioAgentTests-tvOS.xctest")
        );
        assert!(destination.join("host.app").is_dir());
        assert!(destination.join("runner.app").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_requires_a_runner_and_a_unique_host() {
        let root = std::env::temp_dir().join(format!(
            "impresario-products-error-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let source = root.join("build");
        std::fs::create_dir_all(&source).unwrap();
        app(&source, "ImpresarioAgent-tvOS.app", "com.impresario.agent");
        let error = ProductSet::import(&source, &root.join("products")).unwrap_err();
        assert!(error.to_string().contains("no XCUITest runner"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
