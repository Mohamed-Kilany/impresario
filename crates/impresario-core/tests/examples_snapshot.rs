//! `insta` snapshots for the committed example files and for the AST JSON
//! document shape that `impresario parse --json` prints (spec 5.7/Phase 2).
//!
//! The examples are parsed with stable relative paths so the snapshots are
//! identical on every machine. A broken file pins the diagnostics data
//! (codes, messages, spans, hints) that miette renders at the CLI boundary.

use std::path::{Path, PathBuf};

use impresario_core::parser::{parse_play, parse_scene, parse_theatre};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn read(relative: &str) -> String {
    let path = examples_dir().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

#[test]
fn login_scene_parses_clean_and_snapshots() {
    let source = read("scenes/LoginScreen.scene");
    let outcome = parse_scene(&source, Path::new("examples/scenes/LoginScreen.scene"));
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let ast = outcome.ast.expect("scene AST");
    let json = serde_json::to_value(&ast).expect("serialize AST");
    insta::assert_yaml_snapshot!("login_scene_ast", ast);
    insta::assert_json_snapshot!("login_scene_ast_json", json);
}

#[test]
fn dashboard_scene_parses_clean_and_snapshots() {
    let source = read("scenes/DashboardScreen.scene");
    let outcome = parse_scene(&source, Path::new("examples/scenes/DashboardScreen.scene"));
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let ast = outcome.ast.expect("scene AST");
    let json = serde_json::to_value(&ast).expect("serialize AST");
    insta::assert_yaml_snapshot!("dashboard_scene_ast", ast);
    insta::assert_json_snapshot!("dashboard_scene_ast_json", json);
}

#[test]
fn login_play_parses_clean_and_snapshots() {
    let source = read("plays/login.play");
    let outcome = parse_play(&source, Path::new("examples/plays/login.play"));
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let ast = outcome.ast.expect("play AST");
    let json = serde_json::to_value(&ast).expect("serialize AST");
    insta::assert_yaml_snapshot!("login_play_ast", ast);
    insta::assert_json_snapshot!("login_play_ast_json", json);
}

#[test]
fn login_theatre_parses_clean_and_snapshots() {
    let source = read("theatres/login.theatre");
    let outcome = parse_theatre(&source, Path::new("examples/theatres/login.theatre"));
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let ast = outcome.ast.expect("theatre AST");
    let json = serde_json::to_value(&ast).expect("serialize AST");
    insta::assert_yaml_snapshot!("login_theatre_ast", ast);
    insta::assert_json_snapshot!("login_theatre_ast_json", json);
}

#[test]
fn examples_are_migrated_already_and_formatted() {
    let root = examples_dir();
    let loaded = impresario_core::workspace::load_dirs(
        &root.join("scenes"),
        &root.join("theatres"),
        &root.join("plays"),
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let migrated = impresario_core::migrate::migrate(&loaded.workspace, &root.join("theatres"));
    assert!(!migrated.errors.has_errors(), "{migrated:?}");
    assert!(migrated.is_empty(), "{migrated:?}");

    for path in loaded
        .workspace
        .scenes
        .iter()
        .map(|scene| scene.path.clone())
        .chain(
            loaded
                .workspace
                .theatres
                .iter()
                .map(|theatre| theatre.path.clone()),
        )
        .chain(loaded.workspace.plays.iter().map(|play| play.path.clone()))
    {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        let formatted = impresario_core::fmt::format_source(&source, &path)
            .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", path.display()));
        assert_eq!(formatted, source, "{} is not formatted", path.display());
    }
}

#[test]
fn broken_play_diagnostics_snapshot() {
    // One file exercising the scanner and several parser codes at once; the
    // snapshot pins the documented codes, messages, spans and hints.
    let source = "\
Play: Broken Example
Var: TOKEN = '${env.TEST_TOKEN}

Act: First

Stage: One
    Timeout: 0ms
    Scene: MissingScene
        Action: tap 'x'
        Check: Nope text 'unterminated
";
    let outcome = parse_play(source, Path::new("examples/plays/broken.play"));
    assert!(outcome.diagnostics.has_errors());
    let codes: Vec<String> = outcome
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str().to_string())
        .collect();
    assert_eq!(
        codes,
        vec![
            "IMP-DSL-003", // unterminated Var value
            "IMP-DSL-015", // Timeout: 0ms
            "IMP-DSL-010", // unknown action 'tap'
            "IMP-DSL-003", // unterminated Check text
        ],
        "{:?}",
        outcome.diagnostics
    );
    insta::assert_yaml_snapshot!(
        "broken_play_diagnostics",
        outcome.diagnostics.iter().cloned().collect::<Vec<_>>()
    );
}
