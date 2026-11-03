//! TASK-3 Phase 2 actors/groups/audiences: parser, formatter and validation.
//!
//! These are core-level tests so the CLI, planner, LSP and standalone
//! extension all share the same grammar and semantic expectations. The
//! mock-agent end-to-end test lives in `impresario-master/tests/run_mock.rs`.

use std::collections::BTreeMap;
use std::path::Path;

use impresario_core::ast::{ActorFilterKind, Named};
use impresario_core::config::{ActorResolveError, DeviceConfig, DevicesConfig};
use impresario_core::diag::{Code, Diagnostics};
use impresario_core::fmt::format_source;
use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
use impresario_core::platforms::Platform;
use impresario_core::validation::{validate_with_actors, DeviceRef};
use impresario_core::workspace::Workspace;

const SCENE: &str = r##"Scene: Screen
Platforms: cross-platform

| Piece | web | android | ios |
|---|---|---|---|
| A | #a | id:a | accessibilityId:a |
"##;

const PLAY: &str = r#"Play: Streaming Journey
App: shop
Actors: tv1, @tvs

Act: Watch a title
Tags: smoke, streaming
Actors: tv1, phone2
    Perform: Login
    Perform For [tv1, @tvs]: OpenRemote
    Perform Except [tv1]: OpenTouch
"#;

fn names(items: &[Named]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

fn has_code(diagnostics: &Diagnostics, code: Code) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.code == code)
}

fn scene() -> impresario_core::ast::SceneFile {
    parse_scene(SCENE, Path::new("scenes/Screen.scene"))
        .ast
        .expect("test scene parses")
}

fn theatre(source: &str, path: &str) -> impresario_core::ast::TheatreFile {
    parse_theatre(source, Path::new(path))
        .ast
        .expect("test theatre parses")
}

fn play(source: &str) -> impresario_core::ast::PlayFile {
    parse_play(source, Path::new("plays/test.play"))
        .ast
        .expect("test play parses")
}

fn workspace(
    theatres: Vec<impresario_core::ast::TheatreFile>,
    play: impresario_core::ast::PlayFile,
) -> Workspace {
    Workspace {
        scenes: vec![scene()],
        theatres,
        plays: vec![play],
    }
}

#[test]
fn actors_and_filters_parse_into_the_ast() {
    let outcome = parse_play(PLAY, Path::new("plays/streaming.play"));
    assert!(
        outcome.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        outcome.diagnostics
    );
    let play = outcome.ast.expect("play ast");

    assert_eq!(names(&play.actors), ["tv1", "@tvs"]);
    let act = &play.acts[0];
    assert_eq!(names(&act.actors), ["tv1", "phone2"]);
    assert_eq!(
        act.performs
            .iter()
            .map(|perform| perform.stage_name.as_str())
            .collect::<Vec<_>>(),
        ["Login", "OpenRemote", "OpenTouch"]
    );

    let remote = act.performs[1].filter.as_ref().expect("For filter");
    assert_eq!(remote.kind, ActorFilterKind::For);
    assert_eq!(names(&remote.actors), ["tv1", "@tvs"]);
    assert_eq!(remote.actors[1].span.line, 9);

    let touch = act.performs[2].filter.as_ref().expect("Except filter");
    assert_eq!(touch.kind, ActorFilterKind::Except);
    assert_eq!(names(&touch.actors), ["tv1"]);
}

#[test]
fn invalid_actor_and_filter_spellings_are_diagnostics() {
    let duplicate = parse_play(
        "Play: P\nActors: tv1, tv1\n\nAct: A\n    Perform: X\n",
        Path::new("plays/duplicate.play"),
    );
    assert!(
        has_code(&duplicate.diagnostics, Code::DslDuplicateActor),
        "duplicate actor diagnostics: {:?}",
        duplicate.diagnostics
    );

    let invalid = parse_play(
        "Play: P\nActors: TV1\n\nAct: A\n    Perform: X\n",
        Path::new("plays/invalid.play"),
    );
    assert!(
        has_code(&invalid.diagnostics, Code::DslUnknownActor),
        "invalid actor diagnostics: {:?}",
        invalid.diagnostics
    );

    let missing_bracket = parse_play(
        "Play: P\n\nAct: A\n    Perform For tv1: X\n",
        Path::new("plays/filter.play"),
    );
    assert!(
        has_code(&missing_bracket.diagnostics, Code::DslActorFilter),
        "filter diagnostics: {:?}",
        missing_bracket.diagnostics
    );

    let duplicate_audience = parse_theatre(
        "Theatre: T\nAudience: web, web\n\nStage: S\n    Scene: Screen\n        Check: A\n",
        Path::new("theatres/duplicate.theatre"),
    );
    assert!(
        has_code(&duplicate_audience.diagnostics, Code::DslDuplicateAudience),
        "audience diagnostics: {:?}",
        duplicate_audience.diagnostics
    );
}

#[test]
fn groups_expand_recursively_and_reject_cycles() {
    let mut config = DevicesConfig::default();
    config
        .devices
        .push(DeviceConfig::new("tv1", Platform::Web, "127.0.0.1"));
    config
        .devices
        .push(DeviceConfig::new("tv2", Platform::Android, "192.0.2.2"));
    config.groups.insert(
        "tvs".to_string(),
        vec!["tv1".to_string(), "@living".to_string()],
    );
    config
        .groups
        .insert("living".to_string(), vec!["tv2".to_string()]);

    let expected: std::collections::BTreeSet<String> =
        ["tv1".to_string(), "tv2".to_string()].into_iter().collect();
    assert_eq!(config.expand_actor("@tvs").expect("nested group"), expected);

    config
        .groups
        .insert("a".to_string(), vec!["@b".to_string()]);
    config
        .groups
        .insert("b".to_string(), vec!["@a".to_string()]);
    assert!(matches!(
        config.expand_actor("@a"),
        Err(ActorResolveError::Cycle(_))
    ));
    assert_eq!(
        config.expand_actor("@missing"),
        Err(ActorResolveError::Unknown("@missing".to_string()))
    );
}

#[test]
fn validation_resolves_actors_groups_and_filters() {
    let workspace = workspace(
        vec![theatre(
            "Theatre: Playback\nAudience: cross-platform\n\nStage: Open\n    Scene: Screen\n        Check: A\n",
            "theatres/playback.theatre",
        )],
        play(
            "Play: P\nActors: tv1, @tvs\n\nAct: Watch\n    Perform For [tv1]: Open\n    Perform Except [tv1]: Open\n",
        ),
    );
    let devices = vec![
        DeviceRef::new("tv1", Platform::Web),
        DeviceRef::new("tv2", Platform::Android),
        DeviceRef::new("phone", Platform::Ios),
    ];
    let groups = BTreeMap::from([("tvs".to_string(), vec!["tv2".to_string()])]);
    let diagnostics = validate_with_actors(&workspace, &devices, &groups);
    assert!(
        !diagnostics.has_errors(),
        "unexpected validation errors: {diagnostics:?}"
    );
}

#[test]
fn unknown_actor_cycle_and_filter_subset_are_errors() {
    let base_theatre = || {
        vec![theatre(
            "Theatre: Playback\nAudience: cross-platform\n\nStage: Open\n    Scene: Screen\n        Check: A\n",
            "theatres/playback.theatre",
        )]
    };
    let web = vec![
        DeviceRef::new("tv1", Platform::Web),
        DeviceRef::new("tv2", Platform::Web),
    ];

    let unknown = validate_with_actors(
        &workspace(
            base_theatre(),
            play("Play: P\nActors: tv1, missing\n\nAct: A\n    Perform: Open\n"),
        ),
        &web,
        &BTreeMap::new(),
    );
    assert!(
        has_code(&unknown, Code::DslUnknownActor),
        "unknown actor diagnostics: {unknown:?}"
    );

    let cycle_groups = BTreeMap::from([
        ("a".to_string(), vec!["@b".to_string()]),
        ("b".to_string(), vec!["@a".to_string()]),
    ]);
    let cycle = validate_with_actors(
        &workspace(
            base_theatre(),
            play("Play: P\nActors: @a\n\nAct: A\n    Perform: Open\n"),
        ),
        &web,
        &cycle_groups,
    );
    assert!(
        has_code(&cycle, Code::DslGroupCycle),
        "group cycle diagnostics: {cycle:?}"
    );

    let filter = validate_with_actors(
        &workspace(
            base_theatre(),
            play("Play: P\nActors: tv1\n\nAct: A\n    Perform For [tv2]: Open\n"),
        ),
        &web,
        &BTreeMap::new(),
    );
    assert!(
        has_code(&filter, Code::DslActorFilter),
        "filter subset diagnostics: {filter:?}"
    );
}

#[test]
fn audience_is_an_exact_stage_implementation_match() {
    let web_theatre = theatre(
        "Theatre: WebPlayback\nAudience: web\n\nStage: Open\n    Scene: Screen\n        Check: A\n",
        "theatres/web.theatre",
    );
    let tv_theatre = theatre(
        "Theatre: TvPlayback\nAudience: webos\n\nStage: Open\n    Scene: Screen\n        Check: A\n",
        "theatres/tv.theatre",
    );
    let workspace = workspace(
        vec![web_theatre, tv_theatre],
        play("Play: P\nActors: phone\n\nAct: A\n    Perform: Open\n"),
    );

    assert!(workspace.stage_for("Open", Platform::Web).is_some());
    assert!(workspace.stage_for("Open", Platform::WebOs).is_some());
    assert!(workspace.stage_for("Open", Platform::Android).is_none());

    let diagnostics = validate_with_actors(
        &workspace,
        &[DeviceRef::new("phone", Platform::Android)],
        &BTreeMap::new(),
    );
    assert!(
        has_code(&diagnostics, Code::DslStageCoverage),
        "coverage diagnostics: {diagnostics:?}"
    );
}

#[test]
fn formatter_canonicalizes_actor_lists_and_filter_spacing() {
    let source = "Play: Actors\nDescription: actor filters\nApp: shop\n\nAct: Filtered\nActors: web1,android1\n    Perform For [ web1 ,android1 ]:ActorStage\n    Perform Except [ web1 ]:ActorStage\n";
    let formatted = format_source(source, Path::new("actors.play")).expect("formats");
    assert!(
        formatted.contains("Actors: web1, android1"),
        "actor list not canonicalized: {formatted}"
    );
    assert!(
        formatted.contains("Perform For [web1, android1]: ActorStage"),
        "For filter not canonicalized: {formatted}"
    );
    assert!(
        formatted.contains("Perform Except [web1]: ActorStage"),
        "Except filter not canonicalized: {formatted}"
    );

    let again = format_source(&formatted, Path::new("actors.play")).expect("formatted text parses");
    assert_eq!(again, formatted, "formatter is not idempotent");
}
