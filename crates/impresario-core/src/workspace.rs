//! Loading a workspace: every `.scene` and `.play` file plus its diagnostics.
//!
//! The loader is recovering like the parsers: unreadable or broken files
//! produce diagnostics, everything else still loads.

use std::path::{Path, PathBuf};

use crate::ast::{PlayFile, SceneFile};
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::parser::{parse_play, parse_scene};

/// All parsed scenes and plays of one run.
#[derive(Debug, Default)]
pub struct Workspace {
    pub scenes: Vec<SceneFile>,
    pub plays: Vec<PlayFile>,
}

impl Workspace {
    #[must_use]
    pub fn scene(&self, name: &str) -> Option<&SceneFile> {
        self.scenes.iter().find(|scene| scene.name == name)
    }

    #[must_use]
    pub fn scene_names(&self) -> Vec<&str> {
        self.scenes
            .iter()
            .map(|scene| scene.name.as_str())
            .collect()
    }

    #[must_use]
    pub fn piece_names(&self, scene: &str) -> Vec<&str> {
        self.scene(scene)
            .map(|scene| {
                scene
                    .pieces
                    .iter()
                    .map(|piece| piece.name.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Result of loading a workspace.
#[derive(Debug, Default)]
pub struct LoadedWorkspace {
    pub workspace: Workspace,
    pub diagnostics: Diagnostics,
}

/// Load the workspace from the configured `scenes_dir` and `plays_dir`.
#[must_use]
pub fn load_dirs(scenes_dir: &Path, plays_dir: &Path) -> LoadedWorkspace {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_dir(scenes_dir, &mut paths);
    collect_dir(plays_dir, &mut paths);
    paths.sort();
    paths.dedup();
    load_paths(&paths)
}

/// Load only the `.scene` files under `dir` (recursively).
#[must_use]
pub fn load_scenes(dir: &Path) -> LoadedWorkspace {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_dir(dir, &mut paths);
    paths.retain(|path| is_scene_file(path));
    paths.sort();
    load_paths(&paths)
}

/// Load only the `.play` files under `dir` (recursively).
#[must_use]
pub fn load_plays(dir: &Path) -> LoadedWorkspace {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_dir(dir, &mut paths);
    paths.retain(|path| is_play_file(path));
    paths.sort();
    load_paths(&paths)
}

/// Load explicit paths: directories are scanned recursively, files are
/// dispatched on their extension (`.scene`, `.play`).
#[must_use]
pub fn load_paths(paths: &[PathBuf]) -> LoadedWorkspace {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut missing = Diagnostics::new();
    for path in paths {
        if path.is_dir() {
            collect_dir(path, &mut files);
        } else if is_scene_file(path) || is_play_file(path) {
            files.push(path.clone());
        } else if path.exists() {
            missing.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!("{} is neither a .scene nor a .play file", path.display()),
            ));
        } else {
            missing.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!("{} does not exist", path.display()),
            ));
        }
    }
    files.sort();
    files.dedup();
    let mut loaded = load_files(&files);
    loaded.diagnostics.extend(missing);
    loaded
}

fn is_scene_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "scene")
}

fn is_play_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "play")
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| !is_hidden(path))
        .collect();
    children.sort();
    for child in children {
        let Ok(file_type) = child.symlink_metadata() else {
            continue;
        };
        if file_type.is_dir() {
            collect_dir(&child, out);
        } else if file_type.is_file() {
            out.push(child);
        }
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn load_files(files: &[PathBuf]) -> LoadedWorkspace {
    let mut loaded = LoadedWorkspace::default();
    for path in files {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(err) => {
                loaded.diagnostics.push(Diagnostic::error(
                    Code::CfgInvalidValue,
                    format!("cannot read {}: {err}", path.display()),
                ));
                continue;
            }
        };
        if is_scene_file(path) {
            let outcome = parse_scene(&source, path);
            loaded.diagnostics.extend(outcome.diagnostics);
            if let Some(scene) = outcome.ast {
                loaded.workspace.scenes.push(scene);
            }
        } else if is_play_file(path) {
            let outcome = parse_play(&source, path);
            loaded.diagnostics.extend(outcome.diagnostics);
            if let Some(play) = outcome.ast {
                loaded.workspace.plays.push(play);
            }
        }
    }
    // IMP-DSL-006 duplicate scene names across the workspace.
    let mut seen: Vec<String> = Vec::new();
    for scene in &loaded.workspace.scenes {
        if seen.iter().any(|name| name == &scene.name) {
            loaded.diagnostics.push(
                Diagnostic::error(
                    Code::DslDuplicateScene,
                    format!("duplicate scene '{}'", scene.name),
                )
                .with_span(scene.name_span.clone())
                .with_help("scene names must be unique across the workspace"),
            );
        } else {
            seen.push(scene.name.clone());
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("impresario-ws-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        fs::write(&path, content).expect("write file");
        path
    }

    #[test]
    fn loads_dirs_recursively_in_sorted_order() {
        let dir = temp_dir("dirs");
        write(
            &dir,
            "scenes/b.scene",
            "Scene: Second\nPlatform: cross-platform\n\nPieces:\n    B: web=\"#b\"\n",
        );
        write(
            &dir,
            "scenes/a.scene",
            "Scene: First\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
        );
        write(
            &dir,
            "plays/one.play",
            "Play: One\n\nAct: A\n\nStage: S\n    Scene: First\n        Check: A\n",
        );
        let loaded = load_dirs(&dir.join("scenes"), &dir.join("plays"));
        assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
        assert_eq!(loaded.workspace.scenes.len(), 2);
        assert_eq!(loaded.workspace.scenes[0].name, "First");
        assert_eq!(loaded.workspace.scenes[1].name, "Second");
        assert_eq!(loaded.workspace.plays.len(), 1);
        assert_eq!(loaded.workspace.piece_names("First"), vec!["A"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_scene_names_are_reported_per_file() {
        let dir = temp_dir("dupes");
        write(
            &dir,
            "one.scene",
            "Scene: Login\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
        );
        write(
            &dir,
            "two.scene",
            "Scene: Login\nPlatform: cross-platform\n\nPieces:\n    B: web=\"#b\"\n",
        );
        let loaded = load_paths(&[dir.join("one.scene"), dir.join("two.scene")]);
        let dupes: Vec<_> = loaded
            .diagnostics
            .errors()
            .filter(|d| d.code == Code::DslDuplicateScene)
            .collect();
        assert_eq!(dupes.len(), 1);
        assert_eq!(
            dupes[0].span.as_ref().and_then(|s| s.file().file_name()),
            Some(std::ffi::OsStr::new("two.scene"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_paths_are_reported() {
        let loaded = load_paths(&[PathBuf::from("does/not/exist.scene")]);
        assert_eq!(loaded.diagnostics.errors().count(), 1);
        assert!(loaded.workspace.scenes.is_empty());
    }

    #[test]
    fn parse_errors_do_not_stop_loading() {
        let dir = temp_dir("recover");
        write(
            &dir,
            "bad.play",
            "Play: Broken\n\nAct: A\n\nStage: S\n    Scene: Missing\n        Check: Nope\n",
        );
        write(
            &dir,
            "good.scene",
            "Scene: Good\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
        );
        let loaded = load_paths(&[dir.join("bad.play"), dir.join("good.scene")]);
        assert_eq!(loaded.workspace.scenes.len(), 1);
        assert_eq!(loaded.workspace.plays.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
