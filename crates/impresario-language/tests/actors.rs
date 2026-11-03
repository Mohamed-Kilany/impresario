//! Actor, group and audience behaviour in the editor-neutral language service.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use impresario_core::platforms::Platform;
use impresario_core::validation::DeviceRef;
use impresario_language::model::SourceFile;
use impresario_language::service::Language;
use impresario_language::text::Position;

const SCENE: &str = r#"Scene: ActorScene
Platforms: web

| Piece | web |
|---|---|
| Button | css:#button |
"#;

const THEATRE: &str = r#"Theatre: ActorTheatre
Audience: web

Stage: Login
    Scene: ActorScene
        Action: click => Button
"#;

const PLAY: &str = r#"Play: ActorPlay
Actors: @both

Act: A
    Perform: Login
"#;

fn play() -> PathBuf {
    PathBuf::from("plays/actor.play")
}

fn theatre() -> PathBuf {
    PathBuf::from("theatres/actor.theatre")
}

fn scene() -> PathBuf {
    PathBuf::from("scenes/ActorScene.scene")
}

fn groups() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([("both".to_string(), vec!["web1".to_string()])])
}

fn files() -> Vec<SourceFile> {
    vec![
        SourceFile {
            path: scene(),
            text: SCENE.to_string(),
        },
        SourceFile {
            path: theatre(),
            text: THEATRE.to_string(),
        },
        SourceFile {
            path: play(),
            text: PLAY.to_string(),
        },
    ]
}

fn language() -> Language {
    Language::analyze_with_groups(
        &files(),
        &[DeviceRef::new("web1", Platform::Web)],
        &groups(),
        &[],
        Some(PathBuf::from(".")),
    )
}

#[test]
fn actor_groups_resolve_without_unknown_actor_diagnostics() {
    let language = language();
    let codes = language
        .diagnostics_for(&play())
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(
        !codes.iter().any(|code| {
            *code == "IMP-DSL-044" || *code == "IMP-DSL-045" || *code == "IMP-DSL-047"
        }),
        "the configured group must resolve: {codes:?}"
    );
}

fn labels_at(source: &str, path: &Path) -> Vec<String> {
    let offset = source.rfind('|').expect("caret");
    let mut clean = source.to_string();
    clean.remove(offset);
    let position = position_at(&clean, offset);
    let files = files()
        .into_iter()
        .map(|mut file| {
            if file.path == path {
                file.text = clean.clone();
            }
            file
        })
        .collect::<Vec<_>>();
    let language = Language::analyze_with_groups(
        &files,
        &[DeviceRef::new("web1", Platform::Web)],
        &groups(),
        &[],
        Some(PathBuf::from(".")),
    );
    language
        .completion(path, position)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

fn position_at(text: &str, offset: usize) -> Position {
    let mut line = 1u32;
    let mut col = 1u32;
    for ch in text.get(..offset).expect("prefix").chars() {
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

#[test]
fn actors_and_filters_complete_devices_and_groups() {
    let labels = labels_at(
        "Play: ActorPlay\nActors: |\n\nAct: A\n    Perform: Login\n",
        &play(),
    );
    assert!(labels.contains(&"web1".to_string()), "{labels:?}");
    assert!(labels.contains(&"@both".to_string()), "{labels:?}");

    let labels = labels_at(
        "Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform For [|]: Login\n",
        &play(),
    );
    assert!(labels.contains(&"web1".to_string()), "{labels:?}");
    assert!(labels.contains(&"@both".to_string()), "{labels:?}");
}

#[test]
fn audience_completion_offers_platforms() {
    let labels = labels_at(
        "Theatre: ActorTheatre\nAudience: |\n\nStage: Login\n    Scene: ActorScene\n        Action: click => Button\n",
        &theatre(),
    );
    assert!(labels.contains(&"web".to_string()), "{labels:?}");
}
