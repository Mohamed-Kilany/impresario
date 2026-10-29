//! The complete editor feature set of TASK-2 section 9.1 over real LSP
//! framing: navigation, refactoring, symbols, folding, semantic tokens,
//! signature help, code actions, formatting and the `.theatre` file kind.

mod common;

use common::lsp::{self, TestLsp};
use serde_json::{json, Value};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

const SCENE: &str = r#"Scene: LoginScreen
Platforms: cross-platform

| Piece | web | android |
|---|---|---|
| LoginButton | css:#login | id:login_button |
| UsernameInput | css:#username | id:username |
"#;

const THEATRE: &str = r#"Theatre: Auth

Stage: Fill(user, password = 'secret')
    Description: Fill the login form.
    Scene: LoginScreen
        Action: type '${param.user}' => UsernameInput
"#;

const PLAY: &str = r#"Play: Login
Description: sign in
App: shop
Var: USER = 'alice'

Act: Successful Login
Tags: smoke
    Perform: Auth.Fill(user = '${var.USER}')
"#;

/// A workspace with a scene, a theatre and a play.
fn workspace(
    base: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    lsp::write(&base.join("impresario.config"), lsp::config_text());
    lsp::write(
        &base.join("impresario.devices.config"),
        &lsp::devices_text(&[("web1", "web"), ("android1", "android")]),
    );
    let scene = base.join("scenes/LoginScreen.scene");
    let theatre = base.join("theatres/auth.theatre");
    let play = base.join("plays/login.play");
    lsp::write(&scene, SCENE);
    lsp::write(&theatre, THEATRE);
    lsp::write(&play, PLAY);
    (scene, theatre, play)
}

/// Zero-based `(line, character)` of the first `offset` bytes of `marker` in
/// `text` (all fixtures are ASCII before the marker).
fn at(text: &str, marker: &str, offset: usize) -> (u64, u64) {
    let start = text.find(marker).expect("marker");
    let prefix = text.get(..start + offset).expect("prefix");
    let mut line = 0u64;
    let mut character = 0u64;
    for ch in prefix.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    (line, character)
}

async fn document_request(
    server: &mut TestLsp,
    method: &str,
    path: &std::path::Path,
    extra: Value,
) -> Value {
    let mut params = json!({"textDocument": {"uri": TestLsp::uri(path)}});
    if let (Some(target), Some(source)) = (params.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    server.request(method, params).await
}

#[test]
fn all_capabilities_are_advertised() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        workspace(base);
        let mut server = TestLsp::start(base).await;
        let result = server.start_session(Some(&["utf-8", "utf-16"])).await;
        let capabilities = &result["capabilities"];
        for key in [
            "referencesProvider",
            "workspaceSymbolProvider",
            "foldingRangeProvider",
            "semanticTokensProvider",
            "signatureHelpProvider",
            "codeActionProvider",
            "documentRangeFormattingProvider",
            "documentOnTypeFormattingProvider",
            "inlayHintProvider",
            "codeLensProvider",
        ] {
            assert!(
                !capabilities[key].is_null(),
                "missing capability {key}: {capabilities}"
            );
        }
        assert_eq!(
            capabilities["renameProvider"]["prepareProvider"],
            json!(true)
        );
        assert_eq!(capabilities["positionEncoding"], json!("utf-8"));
    });
}

#[test]
fn symbols_folding_tokens_help_hints_and_lenses() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (scene, theatre, play) = workspace(base);
        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-8"])).await;
        server.open(&play, PLAY, 1).await;
        server.wait_version(&play, 1, mark).await;

        let symbols = server
            .request("workspace/symbol", json!({"query": "fill"}))
            .await;
        let names = symbols
            .as_array()
            .expect("workspace symbol array")
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"Fill"), "{symbols}");
        assert!(
            symbols
                .as_array()
                .expect("array")
                .iter()
                .all(|symbol| !symbol["location"]["uri"].as_str().unwrap_or("").is_empty()),
            "{symbols}"
        );

        let theatre_symbols = document_request(
            &mut server,
            "textDocument/documentSymbol",
            &theatre,
            json!({}),
        )
        .await;
        assert_eq!(theatre_symbols[0]["name"], json!("Auth"));

        let scene_symbols = document_request(
            &mut server,
            "textDocument/documentSymbol",
            &scene,
            json!({}),
        )
        .await;
        let pieces = scene_symbols[0]["children"]
            .as_array()
            .expect("scene pieces");
        assert!(pieces.iter().any(|piece| piece["name"] == "LoginButton"));

        let folding =
            document_request(&mut server, "textDocument/foldingRange", &play, json!({})).await;
        assert!(
            folding.as_array().is_some_and(|ranges| !ranges.is_empty()),
            "{folding}"
        );

        let tokens = document_request(
            &mut server,
            "textDocument/semanticTokens/full",
            &play,
            json!({}),
        )
        .await;
        let data = tokens["data"].as_array().expect("semantic data");
        assert!(!data.is_empty() && data.len() % 5 == 0, "{tokens}");

        let (line, character) = at(PLAY, "Auth.Fill(", "Auth.Fill(".len());
        let help = server
            .at("textDocument/signatureHelp", &play, line, character)
            .await;
        assert!(
            help["signatures"][0]["label"]
                .as_str()
                .unwrap_or_default()
                .contains("user"),
            "{help}"
        );
        assert_eq!(help["activeParameter"], json!(0));

        let (performs_line, _) = at(PLAY, "Perform: Auth.Fill", 0);
        let hints = document_request(
            &mut server,
            "textDocument/inlayHint",
            &play,
            json!({"range": {
                "start": {"line": performs_line, "character": 0},
                "end": {"line": performs_line, "character": 80}
            }}),
        )
        .await;
        assert!(
            hints
                .as_array()
                .expect("hints")
                .iter()
                .any(|hint| hint["label"].as_str().unwrap_or("").contains("password")),
            "{hints}"
        );

        let lenses = document_request(&mut server, "textDocument/codeLens", &play, json!({})).await;
        let commands = lenses
            .as_array()
            .expect("lenses")
            .iter()
            .filter_map(|lens| lens["command"]["command"].as_str())
            .collect::<Vec<_>>();
        assert!(commands.contains(&"impresario.runAct"), "{lenses}");
        assert!(commands.contains(&"impresario.showReferences"), "{lenses}");
    });
}

#[test]
fn prepare_rename_references_and_rename_are_workspace_wide() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (_scene, theatre, play) = workspace(base);
        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-16"])).await;
        server.open(&play, PLAY, 1).await;
        server.wait_version(&play, 1, mark).await;

        let (line, character) = at(PLAY, "Auth.Fill", "Auth.".len());
        let prepared = server
            .at("textDocument/prepareRename", &play, line, character)
            .await;
        assert_eq!(prepared["placeholder"], json!("Fill"), "{prepared}");
        assert_eq!(prepared["range"]["start"]["line"], json!(line));

        let references = server
            .request(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": TestLsp::uri(&play)},
                    "position": {"line": line, "character": character},
                    "context": {"includeDeclaration": true}
                }),
            )
            .await;
        let locations = references.as_array().expect("references");
        assert!(
            locations.len() >= 2,
            "stage declaration and call site: {references}"
        );
        assert!(
            locations
                .iter()
                .any(|location| location["uri"] == json!(TestLsp::uri(&theatre))),
            "{references}"
        );

        let rename = server
            .request(
                "textDocument/rename",
                json!({
                    "textDocument": {"uri": TestLsp::uri(&play)},
                    "position": {"line": line, "character": character},
                    "newName": "FillForm"
                }),
            )
            .await;
        let changes = rename["changes"].as_object().expect("changes");
        assert!(changes.contains_key(&TestLsp::uri(&theatre)), "{rename}");
        assert!(changes.contains_key(&TestLsp::uri(&play)), "{rename}");
        assert!(
            changes
                .values()
                .filter_map(Value::as_array)
                .flatten()
                .any(|edit| edit["newText"] == json!("FillForm")),
            "{rename}"
        );
    });
}

#[test]
fn formatting_range_and_on_type_cover_all_three_file_kinds() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (scene, theatre, play) = workspace(base);
        let unformatted = "Scene: Fmt\nPlatforms: cross-platform\n\n|Piece|web|android|\n|---|---|---|\n|LoginButton|css:#\u{1f600}|id:login_button|";
        lsp::write(&scene, unformatted);
        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-16"])).await;
        server.open(&scene, unformatted, 1).await;
        server.wait_version(&scene, 1, mark).await;

        let full = document_request(
            &mut server,
            "textDocument/formatting",
            &scene,
            json!({"options": {"tabSize": 4, "insertSpaces": true}}),
        )
        .await;
        let edit = &full[0];
        assert_eq!(edit["range"]["start"], json!({"line": 0, "character": 0}));
        let last_line = unformatted.lines().last().unwrap_or_default();
        let expected_end = if unformatted.ends_with('\n') {
            json!({"line": unformatted.lines().count() as u64, "character": 0})
        } else {
            json!({
                "line": (unformatted.lines().count() - 1) as u64,
                "character": last_line.encode_utf16().count() as u64
            })
        };
        assert_eq!(edit["range"]["end"], expected_end, "{full}");

        // The same document under UTF-8: the emoji occupies four bytes.
        let mut utf8_server = TestLsp::start(base).await;
        let utf8_mark = utf8_server.mark();
        utf8_server.start_session(Some(&["utf-8"])).await;
        utf8_server.open(&scene, unformatted, 1).await;
        utf8_server.wait_version(&scene, 1, utf8_mark).await;
        let utf8_full = document_request(
            &mut utf8_server,
            "textDocument/formatting",
            &scene,
            json!({"options": {"tabSize": 4, "insertSpaces": true}}),
        )
        .await;
        assert_eq!(
            utf8_full[0]["range"]["end"],
            json!({
                "line": (unformatted.lines().count() - 1) as u64,
                "character": last_line.len() as u64
            }),
            "{utf8_full}"
        );
        assert!(
            edit["newText"].as_str().unwrap_or("").contains("| Piece"),
            "{full}"
        );

        let range = document_request(
            &mut server,
            "textDocument/rangeFormatting",
            &scene,
            json!({
                "range": {
                    "start": {"line": 3, "character": 0},
                    "end": {"line": 5, "character": 5}
                },
                "options": {"tabSize": 4, "insertSpaces": true}
            }),
        )
        .await;
        assert!(!range.as_array().expect("range edits").is_empty(), "{range}");

        let on_type = document_request(
            &mut server,
            "textDocument/onTypeFormatting",
            &scene,
            json!({
                "position": {"line": 5, "character": 30},
                "ch": "|",
                "options": {"tabSize": 4, "insertSpaces": true}
            }),
        )
        .await;
        assert!(on_type.is_array(), "{on_type}");

        for path in [&theatre, &play] {
            let formatted = document_request(
                &mut server,
                "textDocument/formatting",
                path,
                json!({"options": {"tabSize": 4, "insertSpaces": true}}),
            )
            .await;
            // Canonical fixtures either need no edit (null) or a valid edit array.
            assert!(formatted.is_null() || formatted.is_array(), "{formatted}");
        }
    });
}

#[test]
fn duplicate_diagnostics_have_related_locations_and_theatre_watches_recover() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        lsp::write(&base.join("impresario.config"), lsp::config_text());
        lsp::write(
            &base.join("impresario.devices.config"),
            &lsp::devices_text(&[("web1", "web")]),
        );
        let scene = "Scene: Same\nPlatforms: cross-platform\n\n| Piece | web |\n|---|---|\n| One | #a |\n";
        let first = base.join("scenes/First.scene");
        let second = base.join("scenes/Second.scene");
        lsp::write(&first, scene);
        lsp::write(&second, scene);
        let play = base.join("plays/login.play");
        let play_text =
            "Play: Login\nApp: shop\n\nAct: A\n    Perform: Auth.Fill(user = 'alice')\n";
        lsp::write(&play, play_text);
        let theatre = base.join("theatres/auth.theatre");

        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-8"])).await;
        server.open(&play, play_text, 1).await;
        let published = server.wait_version(&play, 1, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-031".to_string()),
            "missing stage before the theatre file exists: {published}"
        );

        for path in [&first, &second] {
            let published = server.wait_diagnostics(path, mark).await;
            let diagnostics = lsp::diagnostics(&published);
            let duplicate = diagnostics
                .iter()
                .find(|diagnostic| diagnostic["code"] == json!("IMP-DSL-006"))
                .expect("duplicate scene diagnostic");
            assert_eq!(
                duplicate["relatedInformation"]
                    .as_array()
                    .map(Vec::len),
                Some(1),
                "{published}"
            );
        }

        // A `.theatre` directory created after start is watched, and the
        // stage definition clears the play diagnostic.
        let mark = server.mark();
        std::fs::create_dir_all(base.join("theatres")).expect("mkdir theatres");
        server.wait_diagnostics(&play, mark).await;
        let mark = server.mark();
        lsp::write(
            &theatre,
            "Theatre: Auth\n\nStage: Fill(user, password = 'secret')\n    Scene: Same\n        Check: One\n",
        );
        let published = server.wait_diagnostics(&play, mark).await;
        assert!(
            !lsp::codes(&published).contains(&"IMP-DSL-031".to_string()),
            "stage definition after watch: {published}"
        );
    });
}

#[test]
fn code_actions_extract_steps_and_create_missing_pieces() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (scene, theatre, play) = workspace(base);
        let theatre_text = "Theatre: Auth\n\nStage: Fill(user, password = 'secret')\n    Scene: LoginScreen\n        Action: click => Missing\n        Action: click => LoginButton\n        Action: click => UsernameInput\n";
        lsp::write(&theatre, theatre_text);
        let broken = "Play: Login\nApp: shop\n\nAct: A\n    Perform: Auth.Fill(user = 'alice')\n";
        lsp::write(&play, broken);
        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(Some(&["utf-8"])).await;
        server.open(&play, broken, 1).await;
        server.wait_version(&play, 1, mark).await;
        let published = server.wait_diagnostics(&theatre, mark).await;
        let codes = lsp::codes(&published);
        assert!(
            codes.iter().any(|code| code == "IMP-DSL-009"),
            "missing piece diagnostic: {published}"
        );

        let (line, _) = at(theatre_text, "Missing", 0);
        let actions = server
            .request(
                "textDocument/codeAction",
                json!({
                    "textDocument": {"uri": TestLsp::uri(&theatre)},
                    "range": {
                        "start": {"line": line, "character": 0},
                        "end": {"line": line, "character": 80}
                    },
                    "context": {"diagnostics": []}
                }),
            )
            .await;
        let titles = actions
            .as_array()
            .expect("actions")
            .iter()
            .filter_map(|action| action["title"].as_str())
            .collect::<Vec<_>>();
        assert!(
            titles.iter().any(|title| title.contains("Create piece")),
            "{actions}"
        );

        let (start, _) = at(theatre_text, "Action:", 0);
        let extract = server
            .request(
                "textDocument/codeAction",
                json!({
                    "textDocument": {"uri": TestLsp::uri(&theatre)},
                    "range": {
                        "start": {"line": start, "character": 0},
                        "end": {"line": start + 1, "character": 0}
                    },
                    "context": {"diagnostics": []}
                }),
            )
            .await;
        let titles = extract
            .as_array()
            .expect("extract actions")
            .iter()
            .filter_map(|action| action["title"].as_str())
            .collect::<Vec<_>>();
        assert!(
            titles.iter().any(|title| title.contains("Extract selected")),
            "{extract}"
        );
        let _ = scene;
    });
}
