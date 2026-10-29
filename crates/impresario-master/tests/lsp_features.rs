//! Language-server features over real LSP framing (spec 13): completions in
//! every documented context, hover, go-to-definition and document symbols.

mod common;

use common::lsp::{self, TestLsp};
use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::support;
use serde_json::json;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// 0-based LSP line/character of the first occurrence of `needle`.
fn position_of(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("needle");
    let mut line = 0u32;
    let mut col = 0u32;
    for ch in source.get(..offset).expect("prefix").chars() {
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// One theatre with a caret on every completion context; the line and
/// character pairs used by the test are the ends of the marked lines.
const CONTEXTS: &str = r#"Theatre: Contexts
Tags: 
Stage: S
    Scene: 
    Scene: LoginScreen
        Action: 
        Action: click => 
        Check: 
        Check: UsernameInput 
        Action: key 
"#;

/// A table scene with the cursor inside an Android selector cell.
const SCENE_EDIT: &str = r#"Scene: Scratch
Platforms: cross-platform

| Piece | android |
|---|---|
| NewPiece | "
"#;

#[test]
fn completions_cover_every_documented_context() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        lsp::standard_workspace(base, &[("web1", "web")]);
        let contexts = base.join("theatres/contexts.theatre");
        let edits = base.join("scenes/Edits.scene");

        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&contexts, CONTEXTS, 1).await;
        server.open(&edits, SCENE_EDIT, 1).await;
        server.wait_version(&contexts, 1, mark).await;
        server.wait_version(&edits, 1, mark).await; // both overlays indexed

        // `Scene:` offers the scenes of the workspace, with their platforms.
        let items = server.at("textDocument/completion", &contexts, 4, 12).await;
        assert_eq!(
            lsp::completion_labels(&items),
            vec!["LoginScreen", "Scratch"]
        );
        assert_eq!(
            items.as_array().expect("array")[0]["detail"],
            json!("cross-platform")
        );

        // `Action:` offers the verbs and the string starter.
        let verb_items = server.at("textDocument/completion", &contexts, 6, 17).await;
        let labels = lsp::completion_labels(&verb_items);
        assert!(labels.contains(&"click".to_string()), "{labels:?}");
        assert!(labels.contains(&"app background".to_string()), "{labels:?}");
        assert!(labels.contains(&"scrollto".to_string()), "{labels:?}");
        let starter = verb_items
            .as_array()
            .expect("array")
            .last()
            .expect("starter");
        assert_eq!(starter["label"], json!("'"));
        assert_eq!(starter["insertText"], json!("''"));

        // `Action: click =>` offers the sorted pieces of the enclosing scene.
        let pieces = server.at("textDocument/completion", &contexts, 7, 26).await;
        assert_eq!(
            lsp::completion_labels(&pieces),
            vec!["LoginButton", "UsernameInput"]
        );
        assert_eq!(
            pieces.as_array().expect("array")[0]["detail"],
            json!("android=\"id:login_button\", web=\"#login\"")
        );

        // `Check:` starts with pieces, then conditions.
        let checks = server.at("textDocument/completion", &contexts, 8, 16).await;
        assert_eq!(
            lsp::completion_labels(&checks),
            vec!["LoginButton", "UsernameInput"]
        );
        let conditions = server.at("textDocument/completion", &contexts, 9, 30).await;
        assert_eq!(
            lsp::completion_labels(&conditions),
            vec![
                "is visible",
                "is hidden",
                "is enabled",
                "is disabled",
                "is focused",
                "text",
                "textContains",
                "count",
                "timeout"
            ]
        );

        // `Tags:` offers the tags of the workspace.
        let tags = server.at("textDocument/completion", &contexts, 1, 7).await;
        assert_eq!(lsp::completion_labels(&tags), vec!["smoke"]);

        // `Action: key` offers only what the configured web device supports.
        let keys = server
            .at("textDocument/completion", &contexts, 10, 21)
            .await;
        let expected: Vec<String> = RemoteKey::ALL
            .iter()
            .filter(|key| support::key_support(Platform::Web, **key).is_supported())
            .map(|key| key.as_str().to_string())
            .collect();
        assert_eq!(lsp::completion_labels(&keys), expected);
        assert!(
            expected.len() < RemoteKey::ALL.len(),
            "filtering must drop keys: {} of {}",
            expected.len(),
            RemoteKey::ALL.len()
        );

        // A table cell offers the selector prefixes for its platform column.
        let prefixes = server.at("textDocument/completion", &edits, 5, 17).await;
        assert_eq!(
            lsp::completion_labels(&prefixes),
            vec!["id:", "text:", "textContains:", "desc:", "class:"]
        );
    });
}

#[test]
fn hover_definitions_and_symbols_come_from_the_index() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        let theatre = base.join("theatres/login.theatre");
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&play, lsp::PLAY, 1).await;
        server.open(&theatre, lsp::THEATRE, 1).await;
        server.wait_version(&play, 1, mark).await;
        server.wait_version(&theatre, 1, mark).await;

        // Hover on a piece in a step: the selectors of every platform.
        let (piece_line, piece_col) = position_of(lsp::THEATRE, "LoginButton");
        let hover = server
            .at("textDocument/hover", &theatre, piece_line, piece_col)
            .await;
        assert_eq!(hover["contents"]["kind"].as_str(), Some("markdown"));
        let value = hover["contents"]["value"]
            .as_str()
            .expect("markdown hover")
            .to_string();
        assert!(value.contains("**LoginButton**"), "{value}");
        assert!(value.contains("| android | `id:login_button` |"), "{value}");
        assert!(value.contains("| web | `#login` |"), "{value}");

        // The scene reference jumps to the scene header.
        let scene_uri = TestLsp::uri(&scene);
        let (scene_line, scene_col) = position_of(lsp::THEATRE, "LoginScreen");
        let definition = server
            .at("textDocument/definition", &theatre, scene_line, scene_col)
            .await;
        assert_eq!(definition["uri"].as_str(), Some(scene_uri.as_str()));
        assert_eq!(
            definition["range"]["start"],
            json!({"line": 0, "character": 7})
        );

        // A piece in a step jumps to its declaration line.
        let definition = server
            .at("textDocument/definition", &theatre, piece_line, piece_col)
            .await;
        assert_eq!(definition["uri"].as_str(), Some(scene_uri.as_str()));
        assert_eq!(
            definition["range"]["start"],
            json!({"line": 5, "character": 2})
        );

        // The outline of the play: Play -> Act -> Stage.
        let symbols = server
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": TestLsp::uri(&play)}}),
            )
            .await;
        let root = &symbols[0];
        assert_eq!(root["name"], json!("Login"));
        assert_eq!(root["detail"], json!("sign in"));
        assert_eq!(root["children"][0]["name"], json!("Successful Login"));
        assert_eq!(
            root["children"][0]["children"][0]["name"],
            json!("Navigate")
        );

        // The outline of the scene: Scene -> Pieces.
        let symbols = server
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": TestLsp::uri(&scene)}}),
            )
            .await;
        let root = &symbols[0];
        assert_eq!(root["name"], json!("LoginScreen"));
        assert_eq!(root["children"][0]["name"], json!("LoginButton"));
        assert_eq!(
            root["children"][0]["detail"],
            json!("android=\"id:login_button\", web=\"#login\"")
        );
    });
}
