//! Loading a workspace: every `.scene`, `.theatre` and `.play` file plus its diagnostics.
//!
//! The loader is recovering like the parsers: unreadable or broken files
//! produce diagnostics, everything else still loads.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::ast::{PlayFile, SceneFile, TheatreFile};
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::parser::{parse_play, parse_scene, parse_theatre};

/// All parsed scenes, theatres and plays of one run.
#[derive(Debug, Default)]
pub struct Workspace {
    pub scenes: Vec<SceneFile>,
    pub theatres: Vec<TheatreFile>,
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

    /// Resolve a stage by its globally unique name.
    ///
    /// An exact stage name wins (stage names may themselves contain dots).
    /// Otherwise a theatre-qualified `Theatre.Stage` reference is accepted,
    /// which TASK-2 section 8.3 allows for readability even though stage
    /// names are globally unique.
    #[must_use]
    pub fn stage(&self, name: &str) -> Option<&crate::ast::StageDecl> {
        if let Some(stage) = self
            .theatres
            .iter()
            .flat_map(|theatre| theatre.stages.iter())
            .find(|stage| stage.name == name)
        {
            return Some(stage);
        }
        for (index, _) in name.match_indices('.').rev() {
            let (theatre_name, rest) = name.split_at(index);
            let Some(stage_name) = rest.get(1..) else {
                continue;
            };
            if let Some(theatre) = self
                .theatres
                .iter()
                .find(|theatre| theatre.name == theatre_name)
            {
                if let Some(stage) = theatre.stage(stage_name) {
                    return Some(stage);
                }
            }
        }
        None
    }

    #[must_use]
    pub fn stage_names(&self) -> Vec<&str> {
        self.theatres
            .iter()
            .flat_map(|theatre| theatre.stages.iter())
            .map(|stage| stage.name.as_str())
            .collect()
    }
}

/// Result of loading a workspace.
#[derive(Debug, Default)]
pub struct LoadedWorkspace {
    pub workspace: Workspace,
    pub diagnostics: Diagnostics,
}

/// Load the workspace from the configured `scenes_dir`, `theatres_dir` and
/// `plays_dir`.
#[must_use]
pub fn load_dirs(scenes_dir: &Path, theatres_dir: &Path, plays_dir: &Path) -> LoadedWorkspace {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_dir(scenes_dir, &mut paths);
    collect_dir(theatres_dir, &mut paths);
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

/// Load only the `.theatre` files under `dir` (recursively).
#[must_use]
pub fn load_theatres(dir: &Path) -> LoadedWorkspace {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_dir(dir, &mut paths);
    paths.retain(|path| is_theatre_file(path));
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
        } else if is_scene_file(path) || is_theatre_file(path) || is_play_file(path) {
            files.push(path.clone());
        } else if path.exists() {
            missing.push(Diagnostic::error(
                Code::CfgInvalidValue,
                format!(
                    "{} is neither a .scene, .theatre nor a .play file",
                    path.display()
                ),
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

fn is_theatre_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "theatre")
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
        } else if file_type.is_file()
            && (is_scene_file(&child) || is_theatre_file(&child) || is_play_file(&child))
        {
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
        } else if is_theatre_file(path) {
            let outcome = parse_theatre(&source, path);
            loaded.diagnostics.extend(outcome.diagnostics);
            if let Some(theatre) = outcome.ast {
                loaded.workspace.theatres.push(theatre);
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

    // IMP-DSL-033 duplicate theatre names.
    let mut theatres: Vec<String> = Vec::new();
    for theatre in &loaded.workspace.theatres {
        if theatres.iter().any(|name| name == &theatre.name) {
            loaded.diagnostics.push(
                Diagnostic::error(
                    Code::DslDuplicateTheatre,
                    format!("duplicate theatre '{}'", theatre.name),
                )
                .with_span(theatre.name_span.clone())
                .with_help("theatre names must be unique across the workspace"),
            );
        } else {
            theatres.push(theatre.name.clone());
        }
    }

    // IMP-DSL-030 stage names are globally unique across all theatres.
    let mut stages: Vec<(String, crate::span::Span)> = Vec::new();
    for theatre in &loaded.workspace.theatres {
        for stage in &theatre.stages {
            if let Some((_, previous)) = stages.iter().find(|(name, _)| name == &stage.name) {
                loaded.diagnostics.push(
                    Diagnostic::error(
                        Code::DslDuplicateStage,
                        format!("duplicate stage '{}'", stage.name),
                    )
                    .with_span(stage.name_span.clone())
                    .with_help(format!(
                        "first declared at {}:{}",
                        previous.file().display(),
                        previous.line
                    )),
                );
            } else {
                stages.push((stage.name.clone(), stage.name_span.clone()));
            }
        }
    }
    loaded
}

/// Where the workspace root came from (TASK-2 section 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSource {
    /// `--workspace <path>`.
    Flag,
    /// `IMPRESARIO_WORKSPACE`.
    Environment,
    /// `.impresario/` or `impresario.config` found walking up from the CWD.
    Marker,
    /// The `workspace` key in the global config.
    GlobalConfig,
    /// No other source matched; the current directory.
    CurrentDir,
}

impl WorkspaceSource {
    /// A short human-readable label for `workspace current`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceSource::Flag => "--workspace",
            WorkspaceSource::Environment => "IMPRESARIO_WORKSPACE",
            WorkspaceSource::Marker => "workspace marker",
            WorkspaceSource::GlobalConfig => "global config",
            WorkspaceSource::CurrentDir => "current directory",
        }
    }
}

/// The resolved workspace root and the layer that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLocation {
    /// Absolute or caller-relative workspace root.
    pub root: PathBuf,
    /// Winning resolution layer.
    pub source: WorkspaceSource,
}

/// Resolve the workspace root in the documented order:
/// flag, environment, marker walking up from `cwd`, global config, `cwd`.
///
/// Relative paths are resolved against `cwd`; the function never touches the
/// process environment itself, which keeps it testable.
#[must_use]
pub fn resolve_root(
    flag: Option<&Path>,
    environment: Option<&OsStr>,
    cwd: &Path,
    global: Option<&Path>,
) -> WorkspaceLocation {
    if let Some(path) = flag {
        return WorkspaceLocation {
            root: absolute(path, cwd),
            source: WorkspaceSource::Flag,
        };
    }
    if let Some(value) = environment {
        if !value.is_empty() {
            return WorkspaceLocation {
                root: absolute(Path::new(value), cwd),
                source: WorkspaceSource::Environment,
            };
        }
    }
    if let Some(root) = find_marker(cwd) {
        return WorkspaceLocation {
            root,
            source: WorkspaceSource::Marker,
        };
    }
    if let Some(path) = global {
        return WorkspaceLocation {
            root: absolute(path, cwd),
            source: WorkspaceSource::GlobalConfig,
        };
    }
    WorkspaceLocation {
        root: cwd.to_path_buf(),
        source: WorkspaceSource::CurrentDir,
    }
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Walk up from `start` looking for a workspace marker.
///
/// A directory containing `.impresario/` or `impresario.config` is a
/// workspace. The marker lookup stops at the filesystem root. The global home
/// (`~/.impresario`) is not a workspace marker: without this exclusion every
/// directory below the user's profile would resolve to the home directory.
#[must_use]
pub fn find_marker(start: &Path) -> Option<PathBuf> {
    let global_home = crate::home::Home::resolve()
        .ok()
        .map(|home| home.root().to_path_buf());
    find_marker_excluding(start, global_home.as_deref())
}

fn find_marker_excluding(start: &Path, global_home: Option<&Path>) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let dot_marker = dir.join(".impresario");
        let is_global_home = is_global_home(&dot_marker, global_home);
        if (dot_marker.is_dir() && !is_global_home) || dir.join("impresario.config").is_file() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// The configured home, plus the platform default (`%USERPROFILE%\.impresario`
/// or `$HOME/.impresario`), is not a workspace marker. A portable
/// `IMPRESARIO_HOME` must not turn an old default home into a workspace root
/// for every directory below the user profile.
fn is_global_home(candidate: &Path, configured: Option<&Path>) -> bool {
    if configured.is_some_and(|home| same_path(home, candidate)) {
        return true;
    }
    ["USERPROFILE", "HOME"].iter().any(|variable| {
        std::env::var_os(variable).is_some_and(|base| {
            !base.is_empty() && same_path(&PathBuf::from(base).join(".impresario"), candidate)
        })
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
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
        let loaded = load_dirs(
            &dir.join("scenes"),
            &dir.join("theatres"),
            &dir.join("plays"),
        );
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

    fn location_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "impresario-loc-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn stages_resolve_unqualified_and_theatre_qualified_names() {
        use crate::parser::parse_theatre;

        let source = "Theatre: Auth\n\nStage: Login\n    Scene: Home\n        Check: Title\n";
        let theatre = parse_theatre(source, Path::new("auth.theatre"))
            .ast
            .expect("theatre parses");
        let workspace = Workspace {
            theatres: vec![theatre],
            ..Workspace::default()
        };
        assert_eq!(
            workspace.stage("Login").map(|s| s.name.as_str()),
            Some("Login")
        );
        assert_eq!(
            workspace.stage("Auth.Login").map(|s| s.name.as_str()),
            Some("Login")
        );
        assert!(workspace.stage("Other.Login").is_none());
    }

    #[test]
    fn an_exact_stage_name_containing_a_dot_wins_over_theatre_qualification() {
        use crate::parser::parse_theatre;

        let source = "Theatre: Auth\n\nStage: Auth.Login\n    Scene: Home\n        Check: Title\n";
        let theatre = parse_theatre(source, Path::new("auth.theatre"))
            .ast
            .expect("theatre parses");
        let workspace = Workspace {
            theatres: vec![theatre],
            ..Workspace::default()
        };
        assert_eq!(
            workspace.stage("Auth.Login").map(|s| s.name.as_str()),
            Some("Auth.Login")
        );
    }

    #[test]
    fn marker_walks_up_to_the_workspace() {
        let dir = location_dir("marker");
        let nested = dir.join("a/b/c");
        fs::create_dir_all(&nested).expect("nested");
        fs::write(dir.join("impresario.config"), "project: x\n").expect("config");
        let found = resolve_root(None, None, &nested, None);
        assert_eq!(found.root, dir);
        assert_eq!(found.source, WorkspaceSource::Marker);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dot_impresario_directory_is_a_marker() {
        let dir = location_dir("dot");
        let nested = dir.join("x");
        fs::create_dir_all(&nested).expect("nested");
        fs::create_dir_all(dir.join(".impresario")).expect("marker");
        assert_eq!(find_marker(&nested), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_global_home_is_not_a_workspace_marker() {
        // The default home is itself called `.impresario`, so its parent must
        // not resolve to a workspace just because that directory exists.
        let home = crate::home::Home::resolve().expect("home resolves");
        assert_eq!(find_marker_excluding(home.root(), Some(home.root())), None);
        // Only the configured home is excluded: a workspace that happens to
        // contain a `.impresario` directory still wins.
        let dir = location_dir("global-home");
        let other = location_dir("other-home");
        fs::create_dir_all(dir.join(".impresario")).expect("marker");
        let nested = dir.join("scratch");
        fs::create_dir_all(&nested).expect("nested");
        assert_eq!(
            find_marker_excluding(&nested, Some(other.as_path())),
            Some(dir.clone())
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&other);
    }

    #[test]
    fn flag_beats_environment_and_marker() {
        let dir = location_dir("flag");
        let flag = dir.join("flag-workspace");
        fs::create_dir_all(&flag).expect("flag");
        fs::write(dir.join("impresario.config"), "project: x\n").expect("config");
        let found = resolve_root(
            Some(Path::new("flag-workspace")),
            Some(OsStr::new("env-workspace")),
            &dir,
            Some(Path::new("global-workspace")),
        );
        assert_eq!(found.root, flag);
        assert_eq!(found.source, WorkspaceSource::Flag);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn environment_beats_marker_and_empty_values_are_ignored() {
        let dir = location_dir("env");
        fs::write(dir.join("impresario.config"), "project: x\n").expect("config");
        let found = resolve_root(
            None,
            Some(OsStr::new("elsewhere")),
            &dir,
            Some(Path::new("global")),
        );
        assert_eq!(found.root, dir.join("elsewhere"));
        assert_eq!(found.source, WorkspaceSource::Environment);
        let found = resolve_root(None, Some(OsStr::new("")), &dir, None);
        assert_eq!(found.source, WorkspaceSource::Marker);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn global_config_then_current_dir_are_the_last_resort() {
        // Do not use a temp directory here: on Windows it lives under the
        // user profile, and a real `~/.impresario` would be found as a marker
        // above it. This test is only about the fallback order, not files.
        let cwd = Path::new("impresario-no-marker-cwd");
        let found = resolve_root(None, None, cwd, Some(Path::new("w")));
        assert_eq!(found.root, cwd.join("w"));
        assert_eq!(found.source, WorkspaceSource::GlobalConfig);
        let found = resolve_root(None, None, cwd, None);
        assert_eq!(found.root, cwd);
        assert_eq!(found.source, WorkspaceSource::CurrentDir);
    }
}
