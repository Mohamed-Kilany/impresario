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

/// One play with a caret on every completion context; the line and character
/// pairs used by the test are the ends of the marked lines.
const CONTEXTS: &str = r#"Act: A

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

/// A scene entry on the `Pieces:` level, cursor after the key and inside the
/// selector string.
const SCENE_EDIT: &str = r#"Scene: Scratch
Platform: cross-platform

Pieces:
    NewPiece: android="
"#;

#[test]
fn completions_cover_every_documented_context() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        lsp::standard_workspace(base, &[("web1", "web")]);
        let contexts = base.join("plays/contexts.play");
        let edits = base.join("scenes/Edits.scene");

        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&contexts, CONTEXTS, 1).await;
        server.open(&edits, SCENE_EDIT, 1).await;
        server.wait_version(&contexts, 1, mark).await;
        server.wait_version(&edits, 1, mark).await; // both overlays indexed

        // `Scene:` offers the scenes of the workspace, with their platforms.
        let items = server.at("textDocument/completion", &contexts, 4, 11).await;
        assert_eq!(
            lsp::completion_labels(&items),
            vec!["LoginScreen", "Scratch"]
        );
        assert_eq!(
            items.as_array().expect("array")[0]["detail"],
            json!("cross-platform")
        );

        // `Action:` offers the verbs and the string starter.
        let verb_items = server.at("textDocument/completion", &contexts, 6, 16).await;
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
        let pieces = server.at("textDocument/completion", &contexts, 7, 25).await;
        assert_eq!(
            lsp::completion_labels(&pieces),
            vec!["LoginButton", "UsernameInput"]
        );
        assert_eq!(
            pieces.as_array().expect("array")[0]["detail"],
            json!("android=\"id:login_button\", web=\"#login\"")
        );

        // `Check:` starts with pieces, then conditions.
        let checks = server.at("textDocument/completion", &contexts, 8, 15).await;
        assert_eq!(
            lsp::completion_labels(&checks),
            vec!["LoginButton", "UsernameInput"]
        );
        let conditions = server.at("textDocument/completion", &contexts, 9, 29).await;
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
        let tags = server.at("textDocument/completion", &contexts, 2, 6).await;
        assert_eq!(lsp::completion_labels(&tags), vec!["smoke"]);

        // `Action: key` offers only what the configured web device supports.
        let keys = server
            .at("textDocument/completion", &contexts, 10, 20)
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

        // A `Pieces:` entry offers the platform keys, then the selector
        // prefixes of the key before the cursor.
        let platform_keys = server.at("textDocument/completion", &edits, 4, 14).await;
        assert_eq!(
            lsp::completion_labels(&platform_keys),
            vec![
                "web=",
                "webos=",
                "tizen=",
                "android=",
                "androidtv=",
                "ios=",
                "tvos="
            ]
        );
        let prefixes = server.at("textDocument/completion", &edits, 4, 23).await;
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
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&play, lsp::PLAY, 1).await;
        server.wait_version(&play, 1, mark).await;

        // Hover on a piece in a step: the selectors of every platform.
        let hover = server.at("textDocument/hover", &play, 8, 27).await;
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
        let definition = server.at("textDocument/definition", &play, 7, 12).await;
        assert_eq!(definition["uri"].as_str(), Some(scene_uri.as_str()));
        assert_eq!(
            definition["range"]["start"],
            json!({"line": 0, "character": 7})
        );

        // A piece in a step jumps to its declaration line.
        let definition = server.at("textDocument/definition", &play, 8, 27).await;
        assert_eq!(definition["uri"].as_str(), Some(scene_uri.as_str()));
        assert_eq!(
            definition["range"]["start"],
            json!({"line": 4, "character": 4})
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
