//! Actor, group and audience behaviour in the editor-neutral language service.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use impresario_core::platforms::Platform;
use impresario_core::validation::DeviceRef;
use impresario_language::model::SourceFile;
use impresario_language::service::{CompletionItem, DeviceConfigSource, Language, TextEdit};
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

fn language_with_play(text: &str) -> Language {
    let files = files()
        .into_iter()
        .map(|mut file| {
            if file.path == play() {
                file.text = text.to_string();
            }
            file
        })
        .collect::<Vec<_>>();
    Language::analyze_with_groups(
        &files,
        &[DeviceRef::new("web1", Platform::Web)],
        &groups(),
        &[],
        Some(PathBuf::from(".")),
    )
}

fn two_device_language(play_text: &str) -> Language {
    let files = files()
        .into_iter()
        .map(|mut file| {
            if file.path == play() {
                file.text = play_text.to_string();
            }
            file
        })
        .collect::<Vec<_>>();
    Language::analyze_with_groups(
        &files,
        &[
            DeviceRef::new("web1", Platform::Web),
            DeviceRef::new("android1", Platform::Android),
        ],
        &BTreeMap::from([(
            "both".to_string(),
            vec!["web1".to_string(), "android1".to_string()],
        )]),
        &[],
        Some(PathBuf::from(".")),
    )
}

fn theatre_language(theatre_text: &str) -> Language {
    let files = files()
        .into_iter()
        .map(|mut file| {
            if file.path == theatre() {
                file.text = theatre_text.to_string();
            }
            file
        })
        .collect::<Vec<_>>();
    Language::analyze_with_groups(
        &files,
        &[DeviceRef::new("web1", Platform::Web)],
        &groups(),
        &[],
        Some(PathBuf::from(".")),
    )
}

fn play_completion_items(source: &str) -> Vec<CompletionItem> {
    let offset = source.rfind('|').expect("caret");
    let mut clean = source.to_string();
    clean.remove(offset);
    let position = position_at(&clean, offset);
    two_device_language(&clean).completion(&play(), position)
}

fn theatre_completion_items(source: &str) -> Vec<CompletionItem> {
    let offset = source.rfind('|').expect("caret");
    let mut clean = source.to_string();
    clean.remove(offset);
    let position = position_at(&clean, offset);
    let files = files()
        .into_iter()
        .map(|mut file| {
            if file.path == theatre() {
                file.text = clean.clone();
            }
            file
        })
        .collect::<Vec<_>>();
    Language::analyze_with_groups(
        &files,
        &[
            DeviceRef::new("web1", Platform::Web),
            DeviceRef::new("android1", Platform::Android),
        ],
        &BTreeMap::from([(
            "both".to_string(),
            vec!["web1".to_string(), "android1".to_string()],
        )]),
        &[],
        Some(PathBuf::from(".")),
    )
    .completion(&theatre(), position)
}

fn position_of(text: &str, needle: &str) -> Position {
    position_at(text, text.find(needle).expect("needle"))
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
            if file.path.as_path() == path {
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
    assert!(labels.contains(&"cross-platform".to_string()), "{labels:?}");
}

#[test]
fn stage_completion_marks_actor_coverage_and_honours_filters() {
    let items = play_completion_items("Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform: |\n");
    let login = items
        .iter()
        .find(|item| item.label == "Login")
        .expect("stage completion");
    let detail = login.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("partial: 1/2 actors"), "{detail}");
    assert!(
        login
            .documentation
            .as_deref()
            .unwrap_or_default()
            .contains("android1"),
        "{login:?}"
    );

    let items = play_completion_items(
        "Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform For [web1]: |\n",
    );
    let login = items
        .iter()
        .find(|item| item.label == "Login")
        .expect("filtered stage completion");
    let detail = login.detail.as_deref().unwrap_or_default();
    assert!(detail.contains("all 1 actor(s)"), "{detail}");
}

#[test]
fn check_completion_offers_probe_forms_and_filters_providers() {
    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: |\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"Showing"), "{labels:?}");
    assert!(labels.contains(&"Player"), "{labels:?}");
    assert!(labels.contains(&"not"), "{labels:?}");

    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: Player => Login |\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"Playing"), "{labels:?}");
    assert!(labels.contains(&"DRM-enabled"), "{labels:?}");
    assert!(labels.contains(&"with"), "{labels:?}");

    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: Player => Login DRM-enabled |\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"Widevine"), "{labels:?}");
    assert!(!labels.contains(&"FairPlay"), "{labels:?}");

    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: Player => Login with |\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"position"), "{labels:?}");
    assert!(!labels.contains(&"fullscreen"), "{labels:?}");

    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: Player => Login with position>=|\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"30s"), "{labels:?}");

    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: Player => Login with muted=|\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"true"), "{labels:?}");
    assert!(labels.contains(&"false"), "{labels:?}");

    let items = theatre_completion_items(
        "Theatre: ActorTheatre\nAudience: web, android\n\nStage: Login\n    Scene: ActorScene\n        Check: Showing => Login with |\n",
    );
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"coverage>=50%"), "{labels:?}");
    assert!(labels.contains(&"decoded"), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label.starts_with("source~")),
        "{labels:?}"
    );
}

#[test]
fn unsupported_probe_options_report_imp_dsl_013() {
    let scene_path = scene();
    let theatre_path = theatre();
    let files = vec![
        SourceFile {
            path: scene_path,
            text: "Scene: ActorScene\nPlatforms: web, android\n\n| Piece | web | android |\n|---|---|---|\n| Button | css:#button | id:button |\n".to_string(),
        },
        SourceFile {
            path: theatre_path.clone(),
            text: "Theatre: ProbeTheatre\nAudience: web, android\n\nStage: Probe\n    Scene: ActorScene\n        Check: Showing => Button with source~\".m3u8\"\n        Check: Player => Button DRM-enabled FairPlay\n        Check: Player => Button with fullscreen\n".to_string(),
        },
        SourceFile {
            path: PathBuf::from("plays/probe.play"),
            text: "Play: ProbePlay\nActors: web1, android1\n\nAct: A\n    Perform: Probe\n".to_string(),
        },
    ];
    let language = Language::analyze_with_groups(
        &files,
        &[
            DeviceRef::new("web1", Platform::Web),
            DeviceRef::new("android1", Platform::Android),
        ],
        &BTreeMap::new(),
        &[],
        Some(PathBuf::from(".")),
    );
    let diagnostics = language.diagnostics_for(&theatre_path);
    let messages = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "IMP-DSL-013")
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages.iter().any(|message| message.contains("source")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|message| message.contains("FairPlay")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("fullscreen")),
        "{messages:?}"
    );
}

#[test]
fn probe_check_hover_shows_evidence_class_and_confidence() {
    let language = theatre_language(
        "Theatre: ActorTheatre\nAudience: web\n\nStage: Login\n    Scene: ActorScene\n        Check: Showing => Button\n",
    );
    let hover = language
        .hover(&theatre(), Position::new(6, 9))
        .expect("probe hover");
    assert!(
        hover.markdown.contains("Evidence class"),
        "{}",
        hover.markdown
    );
    assert!(
        hover.markdown.contains("authoritative"),
        "{}",
        hover.markdown
    );
    assert!(
        hover.markdown.contains("corroborated"),
        "{}",
        hover.markdown
    );
}

#[test]
fn coverage_diagnostics_offer_stage_scaffold_and_actor_fixes() {
    let source = "Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform: Login\n";
    let language = two_device_language(source);
    let actions = language.code_actions(&play(), Position::new(5, 1), Position::new(5, 40));
    let create = actions
        .iter()
        .find(|action| action.title.contains("implementation for android"))
        .expect("stage implementation quick fix");
    let created = &create.edit.as_ref().expect("create edit").create_files[0];
    assert!(
        created.text.contains("Audience: android"),
        "{}",
        created.text
    );
    assert!(created.text.contains("Stage: Login"), "{}", created.text);

    let except = actions
        .iter()
        .find(|action| action.title == "Add `Except [android1]`")
        .expect("Except quick fix");
    let edit = except.edit.as_ref().expect("Except edit");
    let changes = edit
        .changes
        .get(&play().display().to_string())
        .expect("play edit");
    assert!(
        changes
            .iter()
            .any(|change| change.new_text.contains("Perform Except [android1]")),
        "{changes:?}"
    );

    let source = "Play: ActorPlay\nActors: web1, android1\n\nAct: A\n    Perform: Login\n";
    let language = two_device_language(source);
    let actions = language.code_actions(&play(), Position::new(5, 1), Position::new(5, 40));
    let remove = actions
        .iter()
        .find(|action| action.title.contains("Remove actor 'android1'"))
        .expect("remove actor quick fix");
    let edit = remove.edit.as_ref().expect("remove edit");
    let changes = edit
        .changes
        .get(&play().display().to_string())
        .expect("play edit");
    assert!(
        changes
            .iter()
            .any(|change| change.new_text.contains("Actors: web1")),
        "{changes:?}"
    );
}

#[test]
fn actor_hover_shows_device_platform_and_stage_resolution() {
    let text = "Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform For [web1]: Login\n";
    let language = language_with_play(text);
    let hover = language
        .hover(&play(), position_of(text, "web1"))
        .expect("actor hover");
    assert!(hover.markdown.contains("web"), "{}", hover.markdown);
    assert!(
        hover.markdown.contains("ActorTheatre"),
        "{}",
        hover.markdown
    );
    assert!(hover.markdown.contains("Login"), "{}", hover.markdown);
}

#[test]
fn actor_group_hover_lists_members_and_filter_resolution() {
    let text = "Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform For [@both]: Login\n";
    let language = language_with_play(text);
    let offset = text.rfind("@both").expect("second group reference");
    let hover = language
        .hover(&play(), position_at(text, offset))
        .expect("group hover");
    assert!(hover.markdown.contains("actor group"), "{}", hover.markdown);
    assert!(hover.markdown.contains("web1"), "{}", hover.markdown);
    assert!(
        hover.markdown.contains("Stage resolution"),
        "{}",
        hover.markdown
    );
}

#[test]
fn stage_hover_lists_implementations_and_audiences() {
    let android_theatre = PathBuf::from("theatres/android.theatre");
    let android_text = "Theatre: AndroidTheatre\nAudience: android\n\nStage: Login\n    Scene: ActorScene\n        Action: click => Button\n";
    let mut files = files();
    files.push(SourceFile {
        path: android_theatre,
        text: android_text.to_string(),
    });
    let language = Language::analyze_with_groups(
        &files,
        &[DeviceRef::new("web1", Platform::Web)],
        &groups(),
        &[],
        Some(PathBuf::from(".")),
    );
    let hover = language
        .hover(&theatre(), position_of(THEATRE, "Login"))
        .expect("stage hover");
    assert!(
        hover.markdown.contains("2 implementation(s)"),
        "{}",
        hover.markdown
    );
    assert!(
        hover.markdown.contains("ActorTheatre"),
        "{}",
        hover.markdown
    );
    assert!(
        hover.markdown.contains("AndroidTheatre"),
        "{}",
        hover.markdown
    );
    assert!(hover.markdown.contains("android"), "{}", hover.markdown);
}

const DEVICES_CONFIG: &str = r#"# devices used by the actor tests
devices:
  - name: web1
    type: web
    ip: 127.0.0.1
  - name: android1
    type: android
    ip: 192.0.2.12

groups:
  # both mobile and desktop
  both:
    - web1
    - android1
  phones:
    - '@both'
"#;

fn rename_language(play_text: &str, devices_text: &str) -> Language {
    let files = files()
        .into_iter()
        .map(|mut file| {
            if file.path == play() {
                file.text = play_text.to_string();
            }
            file
        })
        .collect::<Vec<_>>();
    let path = PathBuf::from("impresario.devices.config");
    let outcome = impresario_core::config::parse_devices(devices_text, &path);
    let config = outcome.config.expect("devices config");
    let devices = config
        .devices
        .iter()
        .map(|device| DeviceRef::new(device.name.clone(), device.platform))
        .collect::<Vec<_>>();
    Language::analyze_with_groups(
        &files,
        &devices,
        &config.groups,
        &[],
        Some(PathBuf::from(".")),
    )
    .with_device_configs(vec![DeviceConfigSource {
        path,
        text: devices_text.to_string(),
    }])
}

fn apply_text_edits(text: &str, edits: &[TextEdit]) -> String {
    let mut lines = text
        .split('\n')
        .map(|line| line.chars().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let mut edits = edits.to_vec();
    edits.sort_by(|left, right| {
        (right.span.line, right.span.col).cmp(&(left.span.line, left.span.col))
    });
    for edit in edits {
        let line = &mut lines[edit.span.line - 1];
        let start = edit.span.col - 1;
        line.splice(start..start + edit.span.len, edit.new_text.chars());
    }
    lines
        .into_iter()
        .map(|line| line.into_iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn rename_group_edits_config_and_every_dsl_reference() {
    let text = "Play: ActorPlay\nActors: @both\n\nAct: A\n    Perform For [@both]: Login\n";
    let language = rename_language(text, DEVICES_CONFIG);
    let position = position_of(text, "@both");
    let prepared = language.prepare_rename(&play(), position).expect("prepare");
    assert_eq!(prepared.placeholder, "both");
    let edit = language
        .rename(&play(), position, "shared")
        .expect("rename");

    let play_edits = edit
        .changes
        .iter()
        .find(|(path, _)| path.ends_with("actor.play"))
        .map(|(_, edits)| edits)
        .expect("play edits");
    assert_eq!(
        play_edits
            .iter()
            .filter(|change| change.new_text == "@shared")
            .count(),
        2,
        "{play_edits:?}"
    );

    let config_edits = edit
        .changes
        .iter()
        .find(|(path, _)| path.ends_with("impresario.devices.config"))
        .map(|(_, edits)| edits)
        .expect("config edits");
    assert!(
        config_edits
            .iter()
            .any(|change| change.new_text == "shared"),
        "{config_edits:?}"
    );
    assert!(
        config_edits
            .iter()
            .any(|change| change.new_text == "\"@shared\""),
        "{config_edits:?}"
    );

    let candidate = apply_text_edits(DEVICES_CONFIG, config_edits);
    assert!(
        candidate.contains("# both mobile and desktop"),
        "{candidate}"
    );
    let outcome =
        impresario_core::config::parse_devices(&candidate, Path::new("impresario.devices.config"));
    let parsed = outcome.config.expect("valid renamed config");
    assert!(parsed.groups.contains_key("shared"), "{candidate}");
    assert!(!parsed.groups.contains_key("both"), "{candidate}");
    assert_eq!(parsed.groups["phones"], vec!["@shared".to_string()]);
}

#[test]
fn rename_actor_edits_device_name_and_group_members() {
    let text = "Play: ActorPlay\nActors: @both, web1\n\nAct: A\n    Perform For [web1]: Login\n";
    let language = rename_language(text, DEVICES_CONFIG);
    let position = position_of(text, "web1");
    let prepared = language.prepare_rename(&play(), position).expect("prepare");
    assert_eq!(prepared.placeholder, "web1");
    let edit = language.rename(&play(), position, "web9").expect("rename");

    let play_edits = edit
        .changes
        .iter()
        .find(|(path, _)| path.ends_with("actor.play"))
        .map(|(_, edits)| edits)
        .expect("play edits");
    assert_eq!(
        play_edits
            .iter()
            .filter(|change| change.new_text == "web9")
            .count(),
        2,
        "{play_edits:?}"
    );

    let config_edits = edit
        .changes
        .iter()
        .find(|(path, _)| path.ends_with("impresario.devices.config"))
        .map(|(_, edits)| edits)
        .expect("config edits");
    let candidate = apply_text_edits(DEVICES_CONFIG, config_edits);
    let outcome =
        impresario_core::config::parse_devices(&candidate, Path::new("impresario.devices.config"));
    let parsed = outcome.config.expect("valid renamed config");
    assert!(parsed.devices.iter().any(|device| device.name == "web9"));
    assert!(!parsed.devices.iter().any(|device| device.name == "web1"));
    assert_eq!(parsed.groups["both"], vec!["web9", "android1"]);
    assert_eq!(parsed.groups["phones"], vec!["@both".to_string()]);
}

#[test]
fn rename_actor_rejects_collisions_and_missing_configs() {
    let text = "Play: ActorPlay\nActors: @both, web1\n\nAct: A\n    Perform: Login\n";
    let language = rename_language(text, DEVICES_CONFIG);
    let position = position_of(text, "web1");
    let error = language
        .rename(&play(), position, "android1")
        .expect_err("collision");
    assert!(error.contains("already exists"), "{error}");

    let without_config = two_device_language(text);
    let error = without_config
        .rename(&play(), position, "web9")
        .expect_err("missing config");
    assert!(error.contains("no devices configuration"), "{error}");
}
