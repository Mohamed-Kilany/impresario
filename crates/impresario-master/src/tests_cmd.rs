//! The test explorer and test-file operations (TASK-2 3.4).
//!
//! `impresario tests` renders the whole workspace as a filterable tree (or
//! JSON): scenes and their pieces, theatres and stages, plays and acts, with
//! tags, parameters, examples, platform applicability, file/line locations
//! and stage usage. `new`, `show`, `where`, `open`, `grep` and `rename` edit
//! and navigate the same files without a TUI.
//!
//! `rename` is deliberately conservative: it changes identifier-boundary
//! occurrences, parses every changed file and reloads the workspace; if any
//! step fails the original bytes are restored. `--dry-run` never writes.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use impresario_core::ast::{PlayFile, SceneFile, StageDecl, StageItem, TheatreFile};
use impresario_core::config::ImpresarioConfig;
use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
use impresario_core::platforms::{Platform, ScenePlatforms};
use impresario_core::workspace;
use serde_json::{json, Value};

use crate::cli::{self, EXIT_FAILURE, EXIT_INFRA, EXIT_OK, EXIT_USAGE};
use crate::render;

#[derive(Debug, Args)]
pub struct TestsArgs {
    #[command(subcommand)]
    pub command: Option<TestsCommand>,
    #[command(flatten)]
    pub list: TestsListArgs,
}

#[derive(Debug, Subcommand)]
pub enum TestsCommand {
    /// List the workspace tree (the default).
    List(TestsListArgs),
    /// Create a new `.scene`, `.theatre` or `.play` file.
    New {
        /// scene, theatre or play.
        kind: String,
        /// UpperCamelCase name written as the declaration.
        name: String,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Print a declaration with nearby source and every reference.
    Show {
        /// Declaration name.
        name: String,
    },
    /// Print every declaration/reference location for a name.
    Where {
        /// Declaration or reference name.
        name: String,
    },
    /// Open the file that declares `name` in $VISUAL/$EDITOR.
    Open {
        /// Declaration name or file path.
        target: String,
    },
    /// Search every workspace DSL file.
    Grep {
        /// Literal text.
        pattern: String,
        /// Case-insensitive match.
        #[arg(short = 'i', long)]
        ignore_case: bool,
    },
    /// Rename a declaration and every identifier-boundary reference.
    Rename {
        /// scene, theatre, play, stage, act, piece or any.
        kind: String,
        /// Current name.
        old: String,
        /// New name.
        new: String,
        /// Show changes without writing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args, Clone, Default)]
pub struct TestsListArgs {
    /// Machine-readable tree.
    #[arg(long)]
    pub json: bool,
    /// Play file, directory, glob or name (repeatable).
    #[arg(long, value_name = "FILE|GLOB|NAME")]
    pub play: Vec<String>,
    /// Only acts with this tag (repeatable; OR).
    #[arg(long)]
    pub tag: Vec<String>,
    /// Exclude acts with this tag (wins over --tag).
    #[arg(long)]
    pub exclude_tag: Vec<String>,
    /// Only acts with this name (repeatable).
    #[arg(long)]
    pub act: Vec<String>,
    /// Only acts applicable to this platform.
    #[arg(long)]
    pub platform: Vec<String>,
}

struct Loaded {
    config: ImpresarioConfig,
    scenes: Vec<SceneFile>,
    theatres: Vec<TheatreFile>,
    plays: Vec<PlayFile>,
}

pub fn run(base: &Path, config_path: &Path, args: &TestsArgs) -> i32 {
    match &args.command {
        Some(TestsCommand::List(list)) => list_tests(base, config_path, list),
        Some(TestsCommand::New { kind, name, force }) => {
            new_test(base, config_path, kind, name, *force)
        }
        Some(TestsCommand::Show { name }) => show(base, config_path, name),
        Some(TestsCommand::Where { name }) => where_is(base, config_path, name),
        Some(TestsCommand::Open { target }) => open(base, config_path, target),
        Some(TestsCommand::Grep {
            pattern,
            ignore_case,
        }) => grep(base, config_path, pattern, *ignore_case),
        Some(TestsCommand::Rename {
            kind,
            old,
            new,
            dry_run,
        }) => rename(base, config_path, kind, old, new, *dry_run),
        None => list_tests(base, config_path, &args.list),
    }
}

fn load(base: &Path, config_path: &Path) -> Result<Loaded, i32> {
    let outcome = cli::load_effective_config(base, config_path);
    render::eprint_all(&outcome.diagnostics);
    let Some(config) = outcome.config else {
        eprintln!("tests: configuration errors in {}", config_path.display());
        return Err(EXIT_USAGE);
    };
    let scenes = workspace::load_scenes(&base.join(&config.scenes_dir));
    let theatres = workspace::load_theatres(&base.join(&config.theatres_dir));
    let plays = workspace::load_plays(&base.join(&config.plays_dir));
    let mut diagnostics = scenes.diagnostics;
    diagnostics.extend(theatres.diagnostics);
    diagnostics.extend(plays.diagnostics);
    render::eprint_all(&diagnostics);
    if diagnostics.has_errors() {
        eprintln!("tests: workspace has parse errors; fix them first");
        return Err(EXIT_USAGE);
    }
    Ok(Loaded {
        config,
        scenes: scenes.workspace.scenes,
        theatres: theatres.workspace.theatres,
        plays: plays.workspace.plays,
    })
}

fn platform_names(platforms: &ScenePlatforms) -> Vec<String> {
    match platforms {
        ScenePlatforms::CrossPlatform => vec!["cross-platform".to_string()],
        ScenePlatforms::List(list) => list.iter().map(|p| p.as_str().to_string()).collect(),
    }
}

fn names(items: &[impresario_core::ast::Named]) -> Vec<String> {
    items.iter().map(|item| item.name.clone()).collect()
}

fn scene_json(scene: &SceneFile) -> Value {
    let pieces: Vec<Value> = scene
        .pieces
        .iter()
        .map(|piece| {
            let selectors: Vec<Value> = piece
                .selectors
                .iter()
                .map(|selector| {
                    json!({
                        "platform": selector.key.as_str(),
                        "selector": selector.raw,
                    })
                })
                .collect();
            json!({
                "name": piece.name,
                "file": piece.name_span.file,
                "line": piece.name_span.line,
                "index": piece.index,
                "selectors": selectors,
            })
        })
        .collect();
    json!({
        "name": scene.name,
        "file": scene.name_span.file,
        "line": scene.name_span.line,
        "platforms": platform_names(&scene.platforms),
        "pieces": pieces,
    })
}

fn stage_json(stage: &StageDecl) -> Value {
    let items: Vec<Value> = stage
        .items
        .iter()
        .map(|item| match item {
            StageItem::Scene(block) => json!({
                "kind": "scene",
                "name": block.scene_name,
                "line": block.scene_span.line,
                "steps": block.steps.len(),
            }),
            StageItem::Perform(perform) => json!({
                "kind": "perform",
                "name": perform.stage_name,
                "line": perform.stage_span.line,
                "steps": 0,
            }),
        })
        .collect();
    json!({
        "name": stage.name,
        "file": stage.name_span.file,
        "line": stage.name_span.line,
        "platforms": names(&stage.platforms),
        "params": stage.params.iter().map(|param| json!({
            "name": param.name,
            "required": param.default.is_none(),
        })).collect::<Vec<_>>(),
        "items": items,
    })
}

fn theatre_json(theatre: &TheatreFile) -> Value {
    json!({
        "name": theatre.name,
        "file": theatre.name_span.file,
        "line": theatre.name_span.line,
        "stages": theatre.stages.iter().map(stage_json).collect::<Vec<_>>(),
    })
}

fn act_stage_refs(act: &impresario_core::ast::ActDecl) -> Vec<(String, usize)> {
    let mut refs = Vec::new();
    for perform in &act.performs {
        refs.push((perform.stage_name.clone(), perform.stage_span.line));
    }
    for stage in &act.stages {
        refs.push((stage.name.clone(), stage.name_span.line));
    }
    refs
}

fn play_json(play: &PlayFile, stages: &BTreeMap<String, usize>) -> Value {
    let acts: Vec<Value> = play
        .acts
        .iter()
        .map(|act| {
            let stage_refs = act_stage_refs(act);
            let steps: usize = stage_refs
                .iter()
                .map(|(name, _)| stages.get(name).copied().unwrap_or(0))
                .sum();
            let examples = act.examples.as_ref().map_or_else(
                || json!({"columns": [], "rows": 0}),
                |table| {
                    json!({
                        "columns": table.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
                        "rows": table.rows.len(),
                    })
                },
            );
            json!({
                "name": act.name,
                "file": act.name_span.file,
                "line": act.name_span.line,
                "tags": names(&act.tags),
                "platforms": names(&act.platforms),
                "params": act.params.iter().map(|param| json!({
                    "name": param.name,
                    "required": param.default.is_none(),
                })).collect::<Vec<_>>(),
                "examples": examples,
                "stages": stage_refs.iter().map(|(name, line)| json!({
                    "name": name,
                    "line": line,
                    "steps": stages.get(name).copied().unwrap_or(0),
                })).collect::<Vec<_>>(),
                "steps": steps,
            })
        })
        .collect();
    json!({
        "name": play.name,
        "file": play.name_span.file,
        "line": play.name_span.line,
        "app": play.app.as_ref().map(|app| app.name.clone()),
        "acts": acts,
    })
}

fn stage_steps(theatres: &[TheatreFile]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for theatre in theatres {
        for stage in &theatre.stages {
            let steps: usize = stage
                .items
                .iter()
                .map(|item| match item {
                    StageItem::Scene(block) => block.steps.len(),
                    StageItem::Perform(_) => 0,
                })
                .sum();
            map.insert(stage.name.clone(), steps);
        }
    }
    map
}

fn list_tests(base: &Path, config_path: &Path, args: &TestsListArgs) -> i32 {
    let loaded = match load(base, config_path) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let stages = stage_steps(&loaded.theatres);
    let platform_filter: Vec<Platform> = match parse_platforms(&args.platform) {
        Ok(filter) => filter,
        Err(message) => {
            eprintln!("tests: {message}");
            return EXIT_USAGE;
        }
    };
    let plays: Vec<&PlayFile> = loaded
        .plays
        .iter()
        .filter(|play| play_matches(play, &args.play))
        .collect();
    let acts_by_play: Vec<(&PlayFile, Vec<&impresario_core::ast::ActDecl>)> = plays
        .iter()
        .map(|play| {
            let acts = play
                .acts
                .iter()
                .filter(|act| act_matches(act, args, &platform_filter, &loaded))
                .collect::<Vec<_>>();
            (*play, acts)
        })
        .collect();

    if args.json {
        let json_plays: Vec<Value> = acts_by_play
            .iter()
            .map(|(play, acts)| {
                let mut value = play_json(play, &stages);
                if let Some(map) = value.as_object_mut() {
                    map.insert(
                        "acts".to_string(),
                        Value::Array(
                            acts.iter()
                                .map(|act| {
                                    let mut act_value = play_json(play, &stages);
                                    let selected = act_value
                                        .get("acts")
                                        .and_then(Value::as_array)
                                        .and_then(|all| {
                                            all.iter()
                                                .find(|candidate| {
                                                    candidate.get("name").and_then(Value::as_str)
                                                        == Some(act.name.as_str())
                                                })
                                                .cloned()
                                        })
                                        .unwrap_or_else(|| json!({"name": act.name}));
                                    if let Some(object) = act_value.as_object_mut() {
                                        object.remove("acts");
                                    }
                                    selected
                                })
                                .collect(),
                        ),
                    );
                }
                value
            })
            .collect();
        let value = json!({
            "workspace": base,
            "scenes": loaded.scenes.iter().map(scene_json).collect::<Vec<_>>(),
            "theatres": loaded.theatres.iter().map(theatre_json).collect::<Vec<_>>(),
            "plays": json_plays,
        });
        match serde_json::to_string_pretty(&value) {
            Ok(text) => {
                println!("{text}");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("tests: cannot serialize the tree: {error}");
                EXIT_INFRA
            }
        }
    } else {
        print_tree(base, &loaded, &acts_by_play, &stages);
        EXIT_OK
    }
}

fn print_tree(
    base: &Path,
    loaded: &Loaded,
    plays: &[(&PlayFile, Vec<&impresario_core::ast::ActDecl>)],
    stages: &BTreeMap<String, usize>,
) {
    println!(
        "workspace {} | {} scene(s), {} theatre(s), {} play(s)",
        base.display(),
        loaded.scenes.len(),
        loaded.theatres.len(),
        plays.len()
    );
    println!("Scenes");
    for scene in &loaded.scenes {
        println!(
            "  {}  {}:{}  [{}] pieces: {}",
            scene.name,
            scene.name_span.file.display(),
            scene.name_span.line,
            platform_names(&scene.platforms).join(", "),
            scene
                .pieces
                .iter()
                .map(|piece| piece.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("Theatres");
    for theatre in &loaded.theatres {
        println!(
            "  {}  {}:{}",
            theatre.name,
            theatre.name_span.file.display(),
            theatre.name_span.line
        );
        for stage in &theatre.stages {
            let steps = stages.get(&stage.name).copied().unwrap_or(0);
            let platforms = names(&stage.platforms);
            println!(
                "    stage {}  {}:{}  {} item(s), {steps} step(s){}",
                stage.name,
                stage.name_span.file.display(),
                stage.name_span.line,
                stage.items.len(),
                if platforms.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", platforms.join(", "))
                }
            );
        }
    }
    println!("Plays");
    for (play, acts) in plays {
        println!(
            "  {}  {}:{}",
            play.name,
            play.name_span.file.display(),
            play.name_span.line
        );
        for act in acts {
            let tags = names(&act.tags);
            let examples = act
                .examples
                .as_ref()
                .map(|table| format!("{} example(s)", table.rows.len()))
                .unwrap_or_else(|| "no examples".to_string());
            let steps: usize = act_stage_refs(act)
                .iter()
                .map(|(name, _)| stages.get(name).copied().unwrap_or(0))
                .sum();
            println!(
                "    act {}  {}:{}  tags=[{}] platforms=[{}] {examples}, {steps} step(s)",
                act.name,
                act.name_span.file.display(),
                act.name_span.line,
                tags.join(", "),
                names(&act.platforms).join(", ")
            );
        }
    }
}

fn parse_platforms(raw: &[String]) -> Result<Vec<Platform>, String> {
    let mut result = Vec::new();
    for item in raw {
        for name in item.split(',') {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            let Some(platform) = Platform::ALL
                .iter()
                .find(|platform| platform.as_str() == name)
                .copied()
            else {
                return Err(format!(
                    "unknown platform '{name}' (expected web, webos, tizen, android, androidtv, ios or tvos)"
                ));
            };
            result.push(platform);
        }
    }
    Ok(result)
}

fn play_matches(play: &PlayFile, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let path = play.path.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|pattern| {
        let pattern = pattern.replace('\\', "/");
        if pattern.contains(['*', '?', '[']) {
            let full = format!("**/{pattern}");
            glob::Pattern::new(&full).is_ok_and(|glob| glob.matches(&path))
                || glob::Pattern::new(&pattern).is_ok_and(|glob| glob.matches(&path))
        } else {
            path.ends_with(&pattern) || play.name == pattern
        }
    })
}

fn act_matches(
    act: &impresario_core::ast::ActDecl,
    args: &TestsListArgs,
    platforms: &[Platform],
    loaded: &Loaded,
) -> bool {
    let tags = names(&act.tags);
    if tags
        .iter()
        .any(|tag| args.exclude_tag.iter().any(|excluded| excluded == tag))
    {
        return false;
    }
    if !args.tag.is_empty()
        && !tags
            .iter()
            .any(|tag| args.tag.iter().any(|wanted| wanted == tag))
    {
        return false;
    }
    if !args.act.is_empty() && !args.act.iter().any(|name| name == &act.name) {
        return false;
    }
    if !platforms.is_empty() {
        let declared = names(&act.platforms);
        if !declared.is_empty()
            && !platforms
                .iter()
                .any(|platform| declared.iter().any(|name| name == platform.as_str()))
        {
            return false;
        }
        let applicable = act_stage_refs(act).iter().any(|(stage, _)| {
            if let Some(inline) = act.stages.iter().find(|decl| &decl.name == stage) {
                // An inline stage with no platform restriction runs everywhere.
                return inline.platforms.is_empty()
                    || platforms.iter().any(|platform| {
                        inline
                            .platforms
                            .iter()
                            .any(|name| name.name == platform.as_str())
                    });
            }
            loaded.theatres.iter().any(|theatre| {
                theatre.stage(stage).is_some_and(|stage| {
                    platforms.iter().any(|platform| {
                        stage.platforms.is_empty()
                            || stage
                                .platforms
                                .iter()
                                .any(|name| name.name == platform.as_str())
                    })
                })
            })
        });
        if !applicable {
            return false;
        }
    }
    true
}

fn dsl_files(config: &ImpresarioConfig, base: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in [&config.scenes_dir, &config.theatres_dir, &config.plays_dir] {
        collect_files(&base.join(dir), &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("scene" | "theatre" | "play")
        ) {
            files.push(path);
        }
    }
}

fn declaration_files(loaded: &Loaded) -> Vec<(String, PathBuf, usize)> {
    let mut declarations = Vec::new();
    for scene in &loaded.scenes {
        declarations.push((
            scene.name.clone(),
            scene.name_span.file.clone(),
            scene.name_span.line,
        ));
    }
    for theatre in &loaded.theatres {
        declarations.push((
            theatre.name.clone(),
            theatre.name_span.file.clone(),
            theatre.name_span.line,
        ));
        for stage in &theatre.stages {
            declarations.push((
                stage.name.clone(),
                stage.name_span.file.clone(),
                stage.name_span.line,
            ));
        }
    }
    for play in &loaded.plays {
        declarations.push((
            play.name.clone(),
            play.name_span.file.clone(),
            play.name_span.line,
        ));
        for act in &play.acts {
            declarations.push((
                act.name.clone(),
                act.name_span.file.clone(),
                act.name_span.line,
            ));
        }
    }
    declarations
}

fn resolve_declaration(loaded: &Loaded, name: &str) -> Option<(PathBuf, usize)> {
    declaration_files(loaded)
        .into_iter()
        .find(|(candidate, _, _)| candidate == name)
        .map(|(_, file, line)| (file, line))
}

fn resolve_file(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn show(base: &Path, config_path: &Path, name: &str) -> i32 {
    let loaded = match load(base, config_path) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let Some((file, line)) = resolve_declaration(&loaded, name) else {
        eprintln!("tests: no declaration named '{name}'");
        return EXIT_USAGE;
    };
    let path = resolve_file(base, &file);
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("tests: cannot read {}", path.display());
        return EXIT_INFRA;
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = line.saturating_sub(4).max(1);
    let end = (line + 20).min(lines.len());
    println!("{}:{}:{}", file.display(), line, 1);
    for number in start..=end {
        if let Some(content) = lines.get(number - 1) {
            println!("{number:>5}  {content}");
        }
    }
    println!("references:");
    where_is(base, config_path, name);
    EXIT_OK
}

fn where_is(base: &Path, config_path: &Path, name: &str) -> i32 {
    let config_outcome = cli::load_effective_config(base, config_path);
    render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        return EXIT_USAGE;
    };
    let files = dsl_files(&config, base);
    let mut found = 0usize;
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            for (column, _) in identifier_occurrences(line, name) {
                println!(
                    "{}:{}:{}  {}",
                    path.strip_prefix(base).unwrap_or(&path).display(),
                    index + 1,
                    column + 1,
                    line.trim()
                );
                found += 1;
            }
        }
    }
    if found == 0 {
        eprintln!("tests: no occurrence of '{name}'");
        return EXIT_USAGE;
    }
    EXIT_OK
}

fn open(base: &Path, config_path: &Path, target: &str) -> i32 {
    let loaded = match load(base, config_path) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let path = if let Some((file, _)) = resolve_declaration(&loaded, target) {
        resolve_file(base, &file)
    } else {
        let direct = resolve_file(base, Path::new(target));
        if direct.is_file() {
            direct
        } else {
            eprintln!("tests: no declaration or file named '{target}'");
            return EXIT_USAGE;
        }
    };
    let (program, prefix) = match cli::editor_command() {
        Ok(command) => command,
        Err(message) => {
            eprintln!("tests: {message}");
            return EXIT_USAGE;
        }
    };
    let status = std::process::Command::new(program)
        .args(prefix)
        .arg(&path)
        .status();
    match status {
        Ok(status) if status.success() => EXIT_OK,
        Ok(status) => {
            eprintln!("tests: editor exited with {status}");
            EXIT_FAILURE
        }
        Err(error) => {
            eprintln!("tests: cannot start the editor: {error}");
            EXIT_INFRA
        }
    }
}

fn grep(base: &Path, config_path: &Path, pattern: &str, ignore_case: bool) -> i32 {
    let config_outcome = cli::load_effective_config(base, config_path);
    render::eprint_all(&config_outcome.diagnostics);
    let Some(config) = config_outcome.config else {
        return EXIT_USAGE;
    };
    let needle = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };
    let mut found = 0usize;
    for path in dsl_files(&config, base) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let haystack = if ignore_case {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if haystack.contains(&needle) {
                println!(
                    "{}:{}  {}",
                    path.strip_prefix(base).unwrap_or(&path).display(),
                    index + 1,
                    line.trim_end()
                );
                found += 1;
            }
        }
    }
    if found == 0 {
        println!("tests: no matches for '{pattern}'");
    }
    EXIT_OK
}

fn new_test(base: &Path, config_path: &Path, kind: &str, name: &str, force: bool) -> i32 {
    let loaded = match load(base, config_path) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    if name.is_empty()
        || name.contains(['/', '\\', ':', '"', '\'', '\n', '\r', '\t'])
        || name.split_whitespace().count() != 1
    {
        eprintln!("tests: '{name}' is not a valid declaration name");
        return EXIT_USAGE;
    }
    let (dir, extension, template) = match kind {
        "scene" => (
            &loaded.config.scenes_dir,
            "scene",
            format!(
                "Scene: {name}\nPlatform: cross-platform\n\nPieces:\n    Item: web=\"#item\", android=\"id:item\", ios=\"accessibilityId:item\"\n"
            ),
        ),
        "theatre" => (
            &loaded.config.theatres_dir,
            "theatre",
            format!(
                "Theatre: {name}\n\nStage: First\n    Scene: Sample\n        Check: Sample\n"
            ),
        ),
        "play" => (
            &loaded.config.plays_dir,
            "play",
            format!(
                "Play: {name}\nDescription: ''\n\nAct: First\n    Perform: First\n"
            ),
        ),
        other => {
            eprintln!("tests: new expects scene, theatre or play (got '{other}')");
            return EXIT_USAGE;
        }
    };
    let path = base.join(dir).join(format!("{name}.{extension}"));
    if path.exists() && !force {
        eprintln!("tests: {} already exists (use --force)", path.display());
        return EXIT_USAGE;
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("tests: cannot create {}: {error}", parent.display());
            return EXIT_INFRA;
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(force);
    let result = options
        .open(&path)
        .and_then(|mut file| file.write_all(template.as_bytes()));
    match result {
        Ok(()) => {
            println!("created {}", path.display());
            EXIT_OK
        }
        Err(error) => {
            eprintln!("tests: cannot write {}: {error}", path.display());
            EXIT_INFRA
        }
    }
}

fn rename(base: &Path, config_path: &Path, kind: &str, old: &str, new: &str, dry_run: bool) -> i32 {
    if !matches!(
        kind,
        "scene" | "theatre" | "play" | "stage" | "act" | "piece" | "any"
    ) {
        eprintln!("tests: rename kind must be scene, theatre, play, stage, act, piece or any");
        return EXIT_USAGE;
    }
    if old == new || old.is_empty() || new.is_empty() {
        eprintln!("tests: rename needs two different non-empty names");
        return EXIT_USAGE;
    }
    let loaded = match load(base, config_path) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    if !declaration_files(&loaded)
        .iter()
        .any(|(name, _, _)| name == old)
    {
        eprintln!("tests: no declaration named '{old}'");
        return EXIT_USAGE;
    }
    let files = dsl_files(&loaded.config, base);
    let mut changes: Vec<(PathBuf, String, String)> = Vec::new();
    let mut total = 0usize;
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (updated, count) = replace_identifier(&text, old, new);
        if count > 0 {
            total += count;
            changes.push((path, text, updated));
        }
    }
    if total == 0 {
        eprintln!("tests: no occurrence of '{old}'");
        return EXIT_USAGE;
    }
    if dry_run {
        for (path, before, after) in &changes {
            let before_lines: Vec<&str> = before.lines().collect();
            let after_lines: Vec<&str> = after.lines().collect();
            for (index, (left, right)) in before_lines.iter().zip(after_lines.iter()).enumerate() {
                if left != right {
                    println!(
                        "{}:{}: - {left}\n{}:{}: + {right}",
                        path.strip_prefix(base).unwrap_or(path).display(),
                        index + 1,
                        path.strip_prefix(base).unwrap_or(path).display(),
                        index + 1
                    );
                }
            }
        }
        println!(
            "tests: would rename {total} occurrence(s) of '{old}' to '{new}' in {} file(s)",
            changes.len()
        );
        return EXIT_OK;
    }
    for (path, _, updated) in &changes {
        if let Err(error) = std::fs::write(path, updated) {
            eprintln!("tests: cannot write {}: {error}; restoring", path.display());
            for (restore_path, original, _) in &changes {
                let _ = std::fs::write(restore_path, original);
            }
            return EXIT_INFRA;
        }
    }
    // Reload after the write; a syntactically valid file can still break a
    // reference elsewhere. Any error restores every changed file.
    let scenes = workspace::load_scenes(&base.join(&loaded.config.scenes_dir));
    let theatres = workspace::load_theatres(&base.join(&loaded.config.theatres_dir));
    let plays = workspace::load_plays(&base.join(&loaded.config.plays_dir));
    let mut diagnostics = scenes.diagnostics;
    diagnostics.extend(theatres.diagnostics);
    diagnostics.extend(plays.diagnostics);
    if diagnostics.has_errors() {
        render::eprint_all(&diagnostics);
        for (path, original, _) in &changes {
            let _ = std::fs::write(path, original);
        }
        eprintln!("tests: rename would break the workspace; all files were restored");
        return EXIT_USAGE;
    }
    println!(
        "renamed {total} occurrence(s) of '{old}' to '{new}' in {} file(s)",
        changes.len()
    );
    EXIT_OK
}

fn identifier_occurrences(line: &str, name: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    if name.is_empty() {
        return result;
    }
    let mut start = 0usize;
    while let Some(found) = line[start..].find(name) {
        let index = start + found;
        let before = line[..index].chars().next_back();
        let after = line[index + name.len()..].chars().next();
        if !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char) {
            result.push((index, name.len()));
        }
        start = index + name.len();
        if start >= line.len() {
            break;
        }
    }
    result
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn replace_identifier(text: &str, old: &str, new: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0usize;
    let mut cursor = 0usize;
    for (start, len) in identifier_occurrences(text, old) {
        let _ = len;
        out.push_str(&text[cursor..start]);
        out.push_str(new);
        cursor = start + old.len();
        count += 1;
    }
    out.push_str(&text[cursor..]);
    (out, count)
}

/// Parse one file with the parser that matches its extension.
fn parse_file(path: &Path, text: &str) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("scene") => !parse_scene(text, path).diagnostics.has_errors(),
        Some("theatre") => !parse_theatre(text, path).diagnostics.has_errors(),
        Some("play") => !parse_play(text, path).diagnostics.has_errors(),
        _ => true,
    }
}

/// Validate changed bytes before writing; used by tests and by rename's
/// dry-run path.
#[must_use]
pub fn validate_change(path: &Path, text: &str) -> bool {
    parse_file(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_replacement_respects_boundaries() {
        let (text, count) = replace_identifier(
            "Scene: Login\nPerform: Login\nLoginDialog\n",
            "Login",
            "SignIn",
        );
        assert_eq!(count, 2);
        assert_eq!(text, "Scene: SignIn\nPerform: SignIn\nLoginDialog\n");
    }

    #[test]
    fn occurrences_report_columns() {
        let found = identifier_occurrences("  Perform: Login", "Login");
        assert_eq!(found, vec![(11, 5)]);
    }
}
