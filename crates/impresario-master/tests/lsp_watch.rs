//! The file watcher of the embedded language server (spec 13): external
//! edits are analysed, fresh directories are picked up, editor noise is not.

mod common;

use std::time::Duration;

use common::lsp::{self, TestLsp};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// The same scene as `common::lsp::SCENE`, but with an unknown selector key
/// (so the file itself carries a diagnostic after the external edit).
const BROKEN_SCENE: &str = r##"Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginButton: android="id:login_button", bada="x"
    UsernameInput: android="id:username", ios="accessibilityId:username", web="#username"
"##;

#[test]
fn external_scene_edits_are_republished() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&play, lsp::PLAY, 1).await;
        server.wait_version(&play, 1, mark).await; // the index is ready

        let mark = server.mark();
        lsp::write(&scene, BROKEN_SCENE);
        let published = server.wait_diagnostics(&scene, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-011".to_string()),
            "the external edit is analysed: {published}"
        );
        let diagnostic = lsp::diagnostics(&published)
            .into_iter()
            .next()
            .expect("diagnostic");
        // The bad key sits after `    LoginButton: android="id:login_button", `.
        assert_eq!(lsp::range_start(&diagnostic), (4, 44));
    });
}

/// A scene with an unknown selector key on its only piece line.
const LATE_SCENE: &str = r##"Scene: Late
Platform: cross-platform

Pieces:
    LateButton: bada="x"
"##;

#[test]
fn a_scenes_directory_created_after_start_is_watched() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        lsp::write(&base.join("impresario.config"), lsp::config_text());
        lsp::write(
            &base.join("impresario.devices.config"),
            &lsp::devices_text(&[("web1", "web")]),
        );
        let play = base.join("plays/late.play");
        lsp::write(&play, lsp::PLAY);
        // No scenes/ directory yet: the watcher falls back to the root.

        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&play, lsp::PLAY, 1).await;
        server.wait_version(&play, 1, mark).await;

        // Creating the directory triggers an analysis that registers the
        // recursive watch; the open play is re-published.
        let mark = server.mark();
        std::fs::create_dir_all(base.join("scenes")).expect("mkdir scenes");
        server.wait_diagnostics(&play, mark).await;

        // A scene written into the fresh directory is picked up.
        let mark = server.mark();
        let scene = base.join("scenes/Late.scene");
        lsp::write(&scene, LATE_SCENE);
        let published = server.wait_diagnostics(&scene, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-011".to_string()),
            "{published}"
        );
    });
}

#[test]
fn editor_noise_files_never_trigger_an_analysis() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (_scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&play, lsp::PLAY, 1).await;
        server.wait_version(&play, 1, mark).await;

        let mark = server.mark();
        let scenes = base.join("scenes");
        lsp::write(&scenes.join(".LoginScreen.scene.swp"), "swap");
        lsp::write(&scenes.join("LoginScreen.scene~"), "backup");
        lsp::write(&scenes.join("draft.play.tmp"), "temp");
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert_eq!(
            server.mark(),
            mark,
            "editor noise must not trigger an analysis"
        );
    });
}
