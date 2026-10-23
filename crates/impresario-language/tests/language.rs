//! Language-service behaviour shared by the LSP and the standalone extension.

use std::path::{Path, PathBuf};

use impresario_core::platforms::Platform;
use impresario_core::span::Span;
use impresario_core::validation::DeviceRef;
use impresario_language::model::{AppInfo, SourceFile};
use impresario_language::service::{DocumentSymbol, Language};
use impresario_language::text::Position;

const SCENE: &str = r#"Scene: LoginScreen
Platform: cross-platform

| Piece | web | android |
|---|---|---|
| LoginButton | css:#login | id:login_button |
| UsernameInput | css:#username | id:username |
"#;

const PLAYS: &str = r#"Play: Login
Description: sign in
App: shop

Var: USER = 'alice'

Act: Successful Login(mode = 'fast')
Tags: smoke, auth
Platforms: android, webos
    Examples:
        | mode |
        |------|
        | fast |
    Perform: Auth.Fill(user = '${var.USER}', password = '${param.mode}')

Act: Basic
    Perform: Navigate
    Perform: Fill(user = 'bob')
"#;

const THEATRE: &str = r#"Theatre: Auth

Stage: Navigate
    Scene: LoginScreen
        Action: click => LoginButton

Stage: Fill(user, password = 'secret')
    Description: Fill the login form.
    Scene: LoginScreen
        Action: type '${param.user}' => UsernameInput
"#;

fn workspace_files() -> Vec<SourceFile> {
    vec![
        SourceFile {
            path: PathBuf::from("scenes/LoginScreen.scene"),
            text: SCENE.to_string(),
        },
        SourceFile {
            path: PathBuf::from("plays/login.play"),
            text: PLAYS.to_string(),
        },
        SourceFile {
            path: PathBuf::from("theatres/auth.theatre"),
            text: THEATRE.to_string(),
        },
    ]
}

fn language() -> Language {
    Language::analyze(
        &workspace_files(),
        &[DeviceRef::new("web1", Platform::Web)],
        &[AppInfo {
            name: "shop".to_string(),
            description: Some("Shop app".to_string()),
        }],
        Some(PathBuf::from(".")),
    )
}

fn play() -> PathBuf {
    PathBuf::from("plays/login.play")
}

fn scene() -> PathBuf {
    PathBuf::from("scenes/LoginScreen.scene")
}

fn caret(text: &str, marker: &str) -> Position {
    let offset = text.find(marker).expect("marker");
    position_at(text, offset)
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

/// Build a workspace whose play contains one `|` caret marker.
fn language_at_caret(source: &str) -> (Language, Position) {
    let offset = source.rfind('|').expect("caret");
    let mut clean = source.to_string();
    clean.remove(offset);
    let position = position_at(&clean, offset);
    let files = vec![
        SourceFile {
            path: scene(),
            text: SCENE.to_string(),
        },
        SourceFile {
            path: play(),
            text: clean,
        },
        SourceFile {
            path: PathBuf::from("theatres/auth.theatre"),
            text: THEATRE.to_string(),
        },
    ];
    (
        Language::analyze(
            &files,
            &[DeviceRef::new("web1", Platform::Web)],
            &[AppInfo {
                name: "shop".to_string(),
                description: Some("Shop app".to_string()),
            }],
            Some(PathBuf::from(".")),
        ),
        position,
    )
}

#[test]
fn partial_rename_and_references_are_workspace_wide() {
    let language = language();
    let position = caret(PLAYS, "Auth.Fill") + Position::new(0, 5);
    let prepared = language.prepare_rename(&play(), position).expect("prepare");
    assert_eq!(prepared.placeholder, "Fill");
    let edit = language
        .rename(&play(), position, "FillForm")
        .expect("rename");
    let changes = edit
        .changes
        .get("theatres/auth.theatre")
        .expect("definition");
    assert!(changes.iter().any(|change| change.new_text == "FillForm"));
    assert!(edit
        .changes
        .get("plays/login.play")
        .expect("call site")
        .iter()
        .any(|change| change.new_text == "FillForm"));
    let references = language.references(&play(), position, true);
    assert!(references.len() >= 2);
}

#[test]
fn definitions_resolve_qualified_stages_and_pieces() {
    let language = language();
    let stage = caret(PLAYS, "Auth.Fill") + Position::new(0, 6);
    let definitions = language.definition(&play(), stage);
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].file, PathBuf::from("theatres/auth.theatre"));
    let piece = caret(PLAYS, "Navigate") + Position::new(0, 10);
    let definitions = language.definition(&play(), piece);
    assert!(definitions.is_empty());
}

#[test]
fn every_renameable_symbol_kind_is_workspace_wide() {
    let language = language();
    let theatre = PathBuf::from("theatres/auth.theatre");

    // Scene: declaration in the `.scene`, `Scene:` blocks in the theatre.
    let position = caret(THEATRE, "Scene: LoginScreen") + Position::new(0, 8);
    let edit = language
        .rename(&theatre, position, "HomeScreen")
        .expect("scene rename");
    assert!(
        edit.changes.contains_key("scenes/LoginScreen.scene"),
        "{edit:?}"
    );
    assert!(
        edit.changes.contains_key("theatres/auth.theatre"),
        "{edit:?}"
    );

    // Piece: table/declaration and every step reference.
    let position = caret(THEATRE, "LoginButton");
    let edit = language
        .rename(&theatre, position, "SubmitButton")
        .expect("piece rename");
    assert!(
        edit.changes.contains_key("scenes/LoginScreen.scene"),
        "{edit:?}"
    );
    assert!(
        edit.changes.contains_key("theatres/auth.theatre"),
        "{edit:?}"
    );

    // Theatre: header and the `Auth.` prefix of a qualified stage reference.
    let position = caret(PLAYS, "Auth.Fill") + Position::new(0, 1);
    let edit = language
        .rename(&play(), position, "Login")
        .expect("theatre rename");
    assert!(
        edit.changes.contains_key("theatres/auth.theatre"),
        "{edit:?}"
    );
    assert!(edit.changes.contains_key("plays/login.play"), "{edit:?}");

    // Var: declaration and `${var.USER}` interpolation.
    let position = caret(PLAYS, "USER =") + Position::new(0, 1);
    let edit = language
        .rename(&play(), position, "ACCOUNT")
        .expect("var rename");
    assert_eq!(
        edit.changes
            .get("plays/login.play")
            .map(Vec::len)
            .unwrap_or_default(),
        2,
        "{edit:?}"
    );

    // Param: call argument, stage parameter and `${param.user}`.
    let position = caret(PLAYS, "user =") + Position::new(0, 1);
    let edit = language
        .rename(&play(), position, "username")
        .expect("param rename");
    assert!(
        edit.changes.contains_key("theatres/auth.theatre"),
        "{edit:?}"
    );
    assert!(edit.changes.contains_key("plays/login.play"), "{edit:?}");
}

#[test]
fn scene_references_inside_play_stages_resolve_to_the_scene() {
    let play_text = "Play: Login\n\nAct: A\n\nStage: Navigate\n    Scene: LoginScreen\n        Action: click => LoginButton\n";
    let files = vec![
        SourceFile {
            path: scene(),
            text: SCENE.to_string(),
        },
        SourceFile {
            path: play(),
            text: play_text.to_string(),
        },
    ];
    let language = Language::analyze(&files, &[], &[], Some(PathBuf::from(".")));
    let position = caret(play_text, "LoginScreen") + Position::new(0, 2);
    let definitions = language.definition(&play(), position);
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert_eq!(definitions[0].file, scene());
}

#[test]
fn completion_covers_latest_contexts() {
    let (language, position) = language_at_caret("Play: P\n\nAct: A\n    Perform: |\n");
    let stages = language
        .completion(&play(), position)
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert!(stages.contains(&"Fill".to_string()), "{stages:?}");

    let (language, position) = language_at_caret("Play: P\n\nAct: A\n    Perform: Fill(|)\n");
    let args = language
        .completion(&play(), position)
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert_eq!(args, vec!["user = ", "password = "], "{args:?}");

    let (language, position) = language_at_caret("Play: P\nApp: |\n");
    let apps = language
        .completion(&play(), position)
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert_eq!(apps, vec!["shop"]);
}

#[test]
fn interpolation_completion_offers_vars_params_env_and_examples() {
    let (language, position) = language_at_caret(
        "Play: P\n\nVar: USER = 'x'\n\nAct: A(mode = 'x')\n    Perform: Fill(user = '${|')\n",
    );
    let labels = language
        .completion(&play(), position)
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert!(labels.contains(&"var.".to_string()), "{labels:?}");
    assert!(labels.contains(&"param.".to_string()), "{labels:?}");
    assert!(labels.contains(&"env.".to_string()), "{labels:?}");

    let (language, position) = language_at_caret(
        "Play: P\n\nVar: USER = 'x'\n\nAct: A(mode = 'x')\n    Perform: Fill(user = '${var.|')\n",
    );
    let labels = language
        .completion(&play(), position)
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert!(labels.contains(&"USER".to_string()), "{labels:?}");

    let (language, position) = language_at_caret(
        "Play: P\n\nAct: A\n    Examples:\n        | mode |\n        |------|\n        | fast |\n    Perform: Fill(user = '${example.|')\n",
    );
    let labels = language
        .completion(&play(), position)
        .into_iter()
        .map(|item| item.label)
        .collect::<Vec<_>>();
    assert!(labels.contains(&"mode".to_string()), "{labels:?}");
}

#[test]
fn hover_shows_resolved_selectors_and_support_matrix() {
    let language = language();
    let position = caret(PLAYS, "Navigate");
    let hover = language.hover(&play(), position).expect("hover");
    assert!(hover.markdown.contains("Stage") || hover.markdown.contains("stage"));
}

#[test]
fn signature_help_uses_stage_params_and_defaults() {
    let (language, position) =
        language_at_caret("Play: P\n\nAct: A\n    Perform: Fill(user = 'x', |)\n");
    let help = language
        .signature_help(&play(), position)
        .expect("signature");
    assert_eq!(
        help.signatures[0].label,
        "Stage: Fill(user, password = 'secret')"
    );
    assert_eq!(help.active_parameter, 1);
}

#[test]
fn formatting_range_and_on_type_share_the_core_formatter() {
    let language = language();
    let edit = language
        .format_document(&scene())
        .expect("format")
        .expect("edit");
    let formatted = language.formatted_text(&scene()).expect("formatted");
    assert!(edit.new_text.contains("| Piece"), "{formatted}");
    assert!(formatted.contains("css:#login"), "{formatted}");
    assert!(formatted.contains("id:login_button"), "{formatted}");
    assert_eq!(
        language.formatted_text(&scene()).expect("formatted"),
        formatted
    );

    let range = language
        .format_range(&scene(), Position::new(1, 1), Position::new(2, 1))
        .expect("range");
    assert!(range.is_empty() || range[0].span.line == 1);

    let on_type = language
        .format_on_type(&scene(), Position::new(4, 2), '|')
        .expect("on type");
    assert!(on_type.len() <= 1);
}

#[test]
fn symbols_folding_tokens_inlay_hints_and_lenses_are_present() {
    let language = language();
    let symbols = language.document_symbols(&play());
    assert_eq!(symbols[0].name, "Login");
    assert_eq!(symbols[0].children[0].name, "Successful Login");
    assert!(language.document_symbols(&scene())[0]
        .children
        .iter()
        .any(|child| child.name == "LoginButton"));
    assert!(!language.folding_ranges(&play()).is_empty());
    assert!(!language.semantic_tokens(&play()).is_empty());
    let hints = language.inlay_hints(&play(), None);
    assert!(hints.iter().any(|hint| hint.label.contains("password")));
    let lenses = language.code_lenses(&play());
    assert!(lenses
        .iter()
        .any(|lens| lens.command.command == "impresario.runAct"));
    assert!(lenses
        .iter()
        .any(|lens| lens.command.command == "impresario.showReferences"));
    let help = language.workspace_symbols("login");
    assert!(help.iter().any(|symbol| symbol.name == "LoginScreen"));
}

fn span_end(text: &str, span: &Span) -> Position {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut line = span.line.max(1);
    let mut col = span.col.max(1);
    let mut remaining = span.len;
    while remaining > 0 {
        let raw = lines.get(line.saturating_sub(1)).copied().unwrap_or("");
        let content = raw.strip_suffix('\r').unwrap_or(raw);
        let length = content.chars().count();
        let available = length.saturating_sub(col.saturating_sub(1));
        if remaining <= available {
            col = col.saturating_add(remaining);
            remaining = 0;
        } else {
            remaining -= available;
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            line = line.saturating_add(1);
            col = 1;
        }
    }
    Position::new(
        u32::try_from(line).expect("line"),
        u32::try_from(col).expect("column"),
    )
}

fn assert_symbols_contain_selection(symbols: &[DocumentSymbol], text: &str) {
    for symbol in symbols {
        let start = Position::new(
            u32::try_from(symbol.span.line).expect("line"),
            u32::try_from(symbol.span.col).expect("column"),
        );
        let end = span_end(text, &symbol.span);
        let selection_start = Position::new(
            u32::try_from(symbol.selection_span.line).expect("line"),
            u32::try_from(symbol.selection_span.col).expect("column"),
        );
        let selection_end = span_end(text, &symbol.selection_span);
        assert!(
            start <= selection_start && selection_end <= end,
            "{} selection {:?}..{:?} outside {:?}..{:?}",
            symbol.name,
            selection_start,
            selection_end,
            start,
            end
        );
        assert_symbols_contain_selection(&symbol.children, text);
    }
}

#[test]
fn document_symbol_ranges_contain_their_selection_ranges() {
    let language = language();
    for (path, text) in [
        (play(), PLAYS),
        (scene(), SCENE),
        (PathBuf::from("theatres/auth.theatre"), THEATRE),
    ] {
        assert_symbols_contain_selection(&language.document_symbols(&path), text);
    }
}

#[test]
fn duplicate_declarations_are_reported_on_every_occurrence() {
    let files = vec![
        SourceFile {
            path: PathBuf::from("a.scene"),
            text: "Scene: Same\nPlatform: cross-platform\n\nPieces:\n    One: web=\"#a\"\n"
                .to_string(),
        },
        SourceFile {
            path: PathBuf::from("b.scene"),
            text: "Scene: Same\nPlatform: cross-platform\n\nPieces:\n    One: web=\"#b\"\n"
                .to_string(),
        },
    ];
    let language = Language::analyze(&files, &[], &[], None);
    let first = language.diagnostics_for(Path::new("a.scene"));
    let second = language.diagnostics_for(Path::new("b.scene"));
    let first_duplicate = first
        .iter()
        .find(|diagnostic| diagnostic.code == "IMP-DSL-006")
        .expect("duplicate scene diagnostic");
    let second_duplicate = second
        .iter()
        .find(|diagnostic| diagnostic.code == "IMP-DSL-006")
        .expect("duplicate scene diagnostic");
    assert_eq!(first_duplicate.related.len(), 1);
    assert_eq!(second_duplicate.related.len(), 1);
}

#[test]
fn code_actions_include_create_missing_scene_and_extract_steps() {
    let theatre_path = PathBuf::from("theatres/auth.theatre");
    let theatre_text = "Theatre: Auth\n\nStage: Navigate\n    Scene: LoginScreen\n        Action: click => Missing\n";
    let files = vec![
        SourceFile {
            path: theatre_path.clone(),
            text: theatre_text.to_string(),
        },
        SourceFile {
            path: scene(),
            text: SCENE.to_string(),
        },
        SourceFile {
            path: play(),
            text: "Play: P\n\nAct: A\n    Perform: Navigate\n".to_string(),
        },
    ];
    let language = Language::analyze(&files, &[], &[], Some(PathBuf::from(".")));
    let position = caret(theatre_text, "Missing");
    let actions = language.code_actions(&theatre_path, position, position);
    assert!(actions.iter().all(|action| !action.title.is_empty()));
    let diagnostic_actions =
        language.code_actions(&theatre_path, Position::new(5, 1), Position::new(5, 40));
    assert!(
        diagnostic_actions
            .iter()
            .any(|action| action.title.contains("Missing")),
        "{diagnostic_actions:?} diags={:?}",
        language.diagnostics_for(&theatre_path)
    );
}
