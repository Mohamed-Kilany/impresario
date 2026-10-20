//! File watching for the language server: `notify` debounced by 200 ms,
//! recursive directory watches, editor temp/swap files filtered out
//! (spec section 13).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

/// The debounce of the file watcher (spec section 13).
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(200);

/// One path the workspace index depends on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WatchSpec {
    pub path: PathBuf,
    /// Directories are watched recursively; files and placeholder parents are
    /// watched non-recursively.
    pub recursive: bool,
}

/// The desired watch set: the scenes and plays directories (their nearest
/// existing parent until they appear) plus both config files.
///
/// When a directory does not exist yet, its parent is watched until it shows
/// up; the next analysis registers the directory itself.
#[must_use]
pub fn specs_for(
    scenes_dir: &Path,
    plays_dir: &Path,
    config: &Path,
    devices: &Path,
    apps: &Path,
) -> Vec<WatchSpec> {
    let mut specs = Vec::new();
    for dir in [scenes_dir, plays_dir] {
        if dir.is_dir() {
            specs.push(WatchSpec {
                path: dir.to_path_buf(),
                recursive: true,
            });
        } else if let Some(parent) = dir.parent().filter(|parent| parent.is_dir()) {
            specs.push(WatchSpec {
                path: parent.to_path_buf(),
                recursive: false,
            });
        }
    }
    for file in [config, devices] {
        if file.is_file() {
            specs.push(WatchSpec {
                path: file.to_path_buf(),
                recursive: false,
            });
        } else if let Some(parent) = file.parent().filter(|parent| parent.is_dir()) {
            specs.push(WatchSpec {
                path: parent.to_path_buf(),
                recursive: false,
            });
        }
    }
    // A missing apps file is treated as an empty layer. Watching its parent
    // (the workspace root) on Windows would turn every subdirectory event into
    // an analysis, so only an existing apps file is watched.
    if apps.is_file() {
        specs.push(WatchSpec {
            path: apps.to_path_buf(),
            recursive: false,
        });
    }
    specs
}

/// Editor temp/swap files and other noise that must never trigger an
/// analysis.
#[must_use]
pub fn is_noise(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    if name.starts_with('.') {
        return true;
    }
    if name.ends_with('~') {
        return true;
    }
    if name.starts_with('#') && name.ends_with('#') {
        return true;
    }
    if name.contains(".goutputstream-") {
        return true;
    }
    const SUFFIXES: &[&str] = &[
        ".swp", ".swx", ".swpx", ".swo", ".tmp", ".temp", ".bak", ".orig", ".crswap",
    ];
    SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// The live, debounced watch set of one workspace.
pub struct Watches {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    registered: BTreeSet<WatchSpec>,
}

impl Watches {
    /// Start a watcher that calls `on_change` once per quiet period whose
    /// events touched at least one interesting file.
    pub fn new(on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let debouncer = new_debouncer(WATCH_DEBOUNCE, None, move |result: DebounceEventResult| {
            let interesting = match result {
                Ok(events) => events.iter().any(|event| {
                    event
                        .event
                        .paths
                        .iter()
                        .any(|path| !is_noise(path.as_path()))
                }),
                Err(_) => true,
            };
            if interesting {
                on_change();
            }
        })?;
        Ok(Self {
            debouncer,
            registered: BTreeSet::new(),
        })
    }

    /// Make the live watch set match `desired`, adding and removing paths as
    /// needed. Failures are ignored: the next analysis retries.
    pub fn sync(&mut self, desired: &[WatchSpec]) {
        let desired: BTreeSet<WatchSpec> = desired.iter().cloned().collect();
        let stale: Vec<WatchSpec> = self.registered.difference(&desired).cloned().collect();
        for spec in stale {
            let _ = self.debouncer.unwatch(&spec.path);
            self.registered.remove(&spec);
        }
        let fresh: Vec<WatchSpec> = desired.difference(&self.registered).cloned().collect();
        for spec in fresh {
            let mode = if spec.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            if self.debouncer.watch(&spec.path, mode).is_ok() {
                self.registered.insert(spec);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_filter_covers_editor_files() {
        for noisy in [
            ".play.swp",
            "login.play~",
            "#login.play#",
            "login.play.tmp",
            "login.play.crswap",
            ".hidden",
        ] {
            assert!(is_noise(Path::new(noisy)), "{noisy} must be ignored");
        }
        for kept in ["login.play", "LoginScreen.scene", "impresario.config"] {
            assert!(!is_noise(Path::new(kept)), "{kept} must be watched");
        }
    }

    #[test]
    fn specs_fall_back_to_the_parent_until_directories_appear() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path();
        let scenes = base.join("scenes");
        let plays = base.join("plays");
        let config = base.join("impresario.config");
        let devices = base.join("impresario.devices.config");

        let apps = base.join("impresario.apps.config");
        let specs = specs_for(&scenes, &plays, &config, &devices, &apps);
        assert!(
            specs
                .iter()
                .all(|spec| spec.path.as_path() == base && !spec.recursive),
            "missing children fall back to the workspace root: {specs:?}"
        );

        std::fs::create_dir_all(&scenes).expect("scenes");
        std::fs::write(&config, "project: x\n").expect("config");
        let apps = base.join("impresario.apps.config");
        let specs = specs_for(&scenes, &plays, &config, &devices, &apps);
        assert!(specs.contains(&WatchSpec {
            path: scenes.clone(),
            recursive: true
        }));
        assert!(specs.contains(&WatchSpec {
            path: config.clone(),
            recursive: false
        }));
    }
}
