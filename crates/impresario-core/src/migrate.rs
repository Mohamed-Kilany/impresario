//! DSL migration (TASK-2 section 8.9): legacy piece lines become tabular
//! scenes, and inline `Stage:` blocks become reusable stages in a generated
//! theatre with the Act rewritten to `Perform:` references.
//!
//! The migration is pure: it returns the files to write. The CLI prints them
//! for `--dry-run`, writes `.bak` backups and applies them otherwise.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::ast::{ActDecl, PlayFile, SceneFile, StageDecl};
use crate::diag::Diagnostics;
use crate::fmt;
use crate::platforms::{Platform, ScenePlatforms};
use crate::workspace::Workspace;

/// One file produced by the migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedFile {
    pub path: PathBuf,
    pub contents: String,
    /// True when the file is newly created (no `.bak` is needed).
    pub created: bool,
}

/// Result of migrating the DSL part of a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DslMigration {
    pub files: Vec<MigratedFile>,
    pub warnings: Vec<String>,
    /// Generated files that did not parse/format. They are never written;
    /// the CLI must print these and fail instead of emitting broken DSL.
    pub errors: Diagnostics,
}

impl DslMigration {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Build the migrated scene/theatre/play files for `workspace`.
///
/// Only legacy files change. Running the migration again is a no-op: tabular
/// scenes and Acts with `Perform:` are left untouched.
#[must_use]
pub fn migrate(workspace: &Workspace, theatres_dir: &Path) -> DslMigration {
    let mut migration = DslMigration::default();
    let mut theatre_names: BTreeSet<String> = workspace
        .theatres
        .iter()
        .map(|theatre| theatre.name.clone())
        .collect();
    let mut stage_names: BTreeSet<String> = workspace
        .theatres
        .iter()
        .flat_map(|theatre| theatre.stages.iter())
        .map(|stage| stage.name.clone())
        .collect();

    for scene in &workspace.scenes {
        if !scene.columns.is_empty() {
            continue;
        }
        let contents = render_scene(scene);
        push_migrated_file(&mut migration, scene.path.clone(), &contents, false);
    }

    for play in &workspace.plays {
        let legacy_acts = play
            .acts
            .iter()
            .filter(|act| !act.stages.is_empty())
            .count();
        if legacy_acts == 0 {
            continue;
        }
        if legacy_acts != play.acts.len() {
            migration.warnings.push(format!(
                "play '{}' mixes inline Stage blocks and Perform lines; \
                 it was left untouched, convert the mixed Act by hand",
                play.name
            ));
            continue;
        }

        let theatre_name = unique_name(
            &sanitize_theatre_name(&format!("{} stages", play.name)),
            &theatre_names,
        );
        theatre_names.insert(theatre_name.clone());
        let mut generated_stages = Vec::new();
        let mut act_refs: Vec<Vec<String>> = Vec::new();
        for act in &play.acts {
            let mut refs = Vec::new();
            for stage in &act.stages {
                let wanted = sanitize_stage_name(&stage.name);
                let assigned = unique_name(&wanted, &stage_names);
                stage_names.insert(assigned.clone());
                if assigned != stage.name {
                    migration.warnings.push(format!(
                        "stage '{}' in play '{}' was renamed to '{}' \
                         because stage names are globally unique",
                        stage.name, play.name, assigned
                    ));
                }
                generated_stages.push((assigned.clone(), stage.clone()));
                refs.push(assigned);
            }
            act_refs.push(refs);
        }

        let theatre_path = unique_theatre_path(theatres_dir, &play.path);
        let theatre_source = render_theatre(&theatre_name, &generated_stages);
        push_migrated_file(&mut migration, theatre_path, &theatre_source, true);

        let play_source = render_play(play, &act_refs);
        push_migrated_file(&mut migration, play.path.clone(), &play_source, false);
    }

    migration
}

/// Format one generated file and queue it. A generated file that does not
/// parse or format is an error, never a silently copied raw file.
fn push_migrated_file(migration: &mut DslMigration, path: PathBuf, source: &str, created: bool) {
    match fmt::format_source(source, &path) {
        Ok(contents) => migration.files.push(MigratedFile {
            path,
            contents,
            created,
        }),
        Err(diagnostics) => migration.errors.extend(diagnostics),
    }
}

fn unique_name(wanted: &str, used: &BTreeSet<String>) -> String {
    if !used.contains(wanted) {
        return wanted.to_string();
    }
    let mut suffix = 2u32;
    loop {
        let candidate = format!("{wanted}_{suffix}");
        if !used.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn sanitize_stage_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if matches!(ch, '(' | ')' | '{' | '}' | '\n' | '\r') {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        "Stage".to_string()
    } else {
        cleaned
    }
}

fn sanitize_theatre_name(name: &str) -> String {
    sanitize_stage_name(name)
}

fn unique_theatre_path(dir: &Path, play_path: &Path) -> PathBuf {
    let stem = play_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("play");
    let mut suffix = 0u32;
    loop {
        let name = if suffix == 0 {
            format!("{stem}.theatre")
        } else {
            format!("{stem}.{suffix}.theatre")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}

fn render_scene(scene: &SceneFile) -> String {
    let mut out = String::new();
    out.push_str("Scene: ");
    out.push_str(&scene.name);
    out.push('\n');
    if let ScenePlatforms::List(platforms) = &scene.platforms {
        out.push_str("Platforms: ");
        out.push_str(
            &platforms
                .iter()
                .map(|platform| platform.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    out.push('\n');

    let used: BTreeSet<Platform> = scene
        .pieces
        .iter()
        .flat_map(|piece| piece.selectors.iter().map(|selector| selector.key))
        .collect();
    let columns: Vec<Platform> = Platform::ALL
        .into_iter()
        .filter(|platform| used.contains(platform))
        .collect();
    let include_index = scene.pieces.iter().any(|piece| piece.index != 0);

    out.push_str("| Piece |");
    for platform in &columns {
        out.push(' ');
        out.push_str(platform.as_str());
        out.push_str(" |");
    }
    if include_index {
        out.push_str(" index |");
    }
    out.push('\n');
    out.push('|');
    for _ in 0..columns.len() + usize::from(include_index) + 1 {
        out.push_str("---|");
    }
    out.push('\n');

    for piece in &scene.pieces {
        out.push_str("| ");
        out.push_str(&piece.name);
        out.push_str(" |");
        for platform in &columns {
            out.push(' ');
            match piece
                .selectors
                .iter()
                .find(|selector| selector.key == *platform)
            {
                Some(selector) => out.push_str(&table_cell(&selector.raw)),
                None => out.push('-'),
            }
            out.push_str(" |");
        }
        if include_index {
            out.push(' ');
            if piece.index == 0 {
                out.push('-');
            } else {
                out.push_str(&piece.index.to_string());
            }
            out.push_str(" |");
        }
        out.push('\n');
    }
    out
}

fn table_cell(text: &str) -> String {
    let needs_quotes = text.trim() != text || text.contains('|');
    if !needs_quotes {
        return text.to_string();
    }
    let quote = if text.contains('\'') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn render_theatre(name: &str, stages: &[(String, StageDecl)]) -> String {
    let mut out = String::new();
    out.push_str("Theatre: ");
    out.push_str(name);
    out.push('\n');
    for (assigned, stage) in stages {
        out.push('\n');
        out.push_str("Stage: ");
        out.push_str(assigned);
        out.push('\n');
        if let Some(description) = &stage.description {
            out.push_str("    Description: ");
            out.push_str(description);
            out.push('\n');
        }
        if !stage.platforms.is_empty() {
            out.push_str("    Platforms: ");
            out.push_str(
                &stage
                    .platforms
                    .iter()
                    .map(|platform| platform.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push('\n');
        }
        if let Some(timeout) = stage.timeout {
            out.push_str("    Timeout: ");
            out.push_str(&timeout.to_string());
            out.push('\n');
        }
        for block in &stage.scenes {
            out.push_str("    Scene: ");
            out.push_str(&block.scene_name);
            out.push('\n');
            for step in &block.steps {
                out.push_str("        ");
                out.push_str(&step.source_text);
                out.push('\n');
            }
        }
    }
    out
}

fn render_play(play: &PlayFile, refs: &[Vec<String>]) -> String {
    let mut out = String::new();
    out.push_str("Play: ");
    out.push_str(&play.name);
    out.push('\n');
    if let Some(description) = &play.description {
        out.push_str("Description: ");
        out.push_str(description);
        out.push('\n');
    }
    if let Some(app) = &play.app {
        out.push_str("App: ");
        out.push_str(&app.name);
        out.push('\n');
    }
    for var in &play.var_decls {
        out.push_str("Var: ");
        out.push_str(&var.name);
        out.push_str(" = ");
        out.push_str(&quote_string(&var.value.raw));
        out.push('\n');
    }
    for (act, stage_refs) in play.acts.iter().zip(refs) {
        out.push('\n');
        render_act(&mut out, act, stage_refs);
    }
    out
}

fn render_act(out: &mut String, act: &ActDecl, stage_refs: &[String]) {
    out.push_str("Act: ");
    out.push_str(&act.name);
    out.push('\n');
    if !act.tags.is_empty() {
        out.push_str("Tags: ");
        out.push_str(
            &act.tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    if !act.platforms.is_empty() {
        out.push_str("Platforms: ");
        out.push_str(
            &act.platforms
                .iter()
                .map(|platform| platform.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    for stage in stage_refs {
        out.push_str("    Perform: ");
        out.push_str(stage);
        out.push('\n');
    }
}

fn quote_string(text: &str) -> String {
    let quote = if text.contains('\'') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::parser::{parse_play, parse_scene, parse_theatre};

    fn scene(source: &str, name: &str) -> SceneFile {
        parse_scene(source, Path::new(name)).ast.expect("scene")
    }

    fn play(source: &str, name: &str) -> PlayFile {
        parse_play(source, Path::new(name)).ast.expect("play")
    }

    fn theatre(source: &str, name: &str) -> crate::ast::TheatreFile {
        parse_theatre(source, Path::new(name)).ast.expect("theatre")
    }

    #[test]
    fn migrates_legacy_scenes_plays_and_duplicate_stage_names() {
        let workspace = Workspace {
            scenes: vec![scene(
                "Scene: S\nPlatform: web, android\n\nPieces:\n    A: web=\"#a\", android=\"id:a\"\n    B: web=\"#b\", android=\"id:b\", index=2\n",
                "S.scene",
            )],
            theatres: vec![theatre(
                "Theatre: Existing\n\nStage: Keep\n    Scene: S\n        Check: A\n",
                "existing.theatre",
            )],
            plays: vec![play(
                "Play: P\n\nAct: One\n\nStage: Fill\n    Scene: S\n        Action: click => A\n\nAct: Two\n\nStage: Fill\n    Scene: S\n        Check: B\n",
                "p.play",
            )],
        };
        let migration = migrate(&workspace, Path::new("theatres"));
        assert!(!migration.errors.has_errors(), "{migration:?}");
        assert_eq!(migration.files.len(), 3, "{migration:?}");
        assert!(migration.warnings.iter().any(|w| w.contains("renamed")));

        let scene_file = migration
            .files
            .iter()
            .find(|file| file.path.ends_with("S.scene"))
            .expect("scene migration");
        let migrated_scene = parse_scene(&scene_file.contents, &scene_file.path)
            .ast
            .expect("migrated scene parses");
        assert_eq!(migrated_scene.columns.len(), 3, "{}", scene_file.contents);

        let play_file = migration
            .files
            .iter()
            .find(|file| file.path.ends_with("p.play"))
            .expect("play migration");
        let migrated_play = parse_play(&play_file.contents, &play_file.path)
            .ast
            .expect("migrated play parses");
        assert_eq!(migrated_play.acts.len(), 2);
        assert_eq!(migrated_play.acts[0].performs.len(), 1);
        assert_eq!(migrated_play.acts[0].performs[0].stage_name, "Fill");
        assert_eq!(migrated_play.acts[1].performs[0].stage_name, "Fill_2");

        let theatre_file = migration
            .files
            .iter()
            .find(|file| file.created)
            .expect("generated theatre");
        let migrated_theatre = parse_theatre(&theatre_file.contents, &theatre_file.path)
            .ast
            .expect("generated theatre parses");
        assert_eq!(migrated_theatre.stages.len(), 2);
        assert_eq!(migrated_theatre.stages[1].name, "Fill_2");
    }

    #[test]
    fn migrating_the_migrated_workspace_is_a_no_op() {
        let legacy_scene = scene(
            "Scene: S\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\", android=\"id:a\"\n",
            "S.scene",
        );
        let legacy_play = play(
            "Play: P\n\nAct: One\n\nStage: Fill\n    Scene: S\n        Check: A\n",
            "p.play",
        );
        let workspace = Workspace {
            scenes: vec![legacy_scene],
            theatres: Vec::new(),
            plays: vec![legacy_play],
        };
        let first = migrate(&workspace, Path::new("theatres"));
        assert!(!first.errors.has_errors(), "{first:?}");
        assert_eq!(first.files.len(), 3);

        let migrated_scene = first
            .files
            .iter()
            .find(|file| file.path.ends_with("S.scene"))
            .expect("scene");
        let migrated_play = first
            .files
            .iter()
            .find(|file| file.path.ends_with("p.play"))
            .expect("play");
        let migrated_theatre = first
            .files
            .iter()
            .find(|file| file.created)
            .expect("theatre");
        let second_workspace = Workspace {
            scenes: vec![parse_scene(&migrated_scene.contents, &migrated_scene.path)
                .ast
                .expect("scene")],
            theatres: vec![
                parse_theatre(&migrated_theatre.contents, &migrated_theatre.path)
                    .ast
                    .expect("theatre"),
            ],
            plays: vec![parse_play(&migrated_play.contents, &migrated_play.path)
                .ast
                .expect("play")],
        };
        let second = migrate(&second_workspace, Path::new("theatres"));
        assert!(second.is_empty(), "{second:?}");
    }
}
