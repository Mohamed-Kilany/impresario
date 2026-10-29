//! Diagnostics of the embedded language server (spec 13): open / change /
//! close, overlay precedence over disk, debounced processing, position
//! encoding negotiation, config/device inputs and unsaved files.

mod common;

use common::lsp::{self, TestLsp};
use serde_json::{json, Value};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn broken_play() -> String {
    lsp::PLAY.replace("Check: UsernameInput", "Check: Ghost")
}

#[test]
fn open_reports_and_change_clears_diagnostics() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (_scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;

        let mark = server.mark();
        server.open(&play, &broken_play(), 1).await;
        let published = server.wait_version(&play, 1, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-009".to_string()),
            "the unknown piece is reported: {published}"
        );
        let diagnostic = lsp::diagnostics(&published)
            .into_iter()
            .next()
            .expect("one diagnostic");
        assert_eq!(
            diagnostic.get("severity").and_then(Value::as_i64),
            Some(1),
            "errors are severity 1"
        );
        assert_eq!(
            diagnostic.get("source").and_then(Value::as_str),
            Some("impresario")
        );

        let mark = server.mark();
        server.change(&play, lsp::PLAY, 2).await;
        let published = server.wait_version(&play, 2, mark).await;
        assert!(
            lsp::diagnostics(&published).is_empty(),
            "the fixed buffer is clean: {published}"
        );
    });
}

#[test]
fn overlays_win_over_disk_and_close_restores_disk() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (_scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        // Disk is broken; the editor opens the fixed buffer.
        lsp::write(&play, &broken_play());
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;

        let mark = server.mark();
        server.open(&play, lsp::PLAY, 1).await;
        let published = server.wait_version(&play, 1, mark).await;
        assert!(
            lsp::diagnostics(&published).is_empty(),
            "the overlay wins over the broken file: {published}"
        );

        let mark = server.mark();
        server.close(&play).await;
        let published = server.wait_diagnostics(&play, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-009".to_string()),
            "closing falls back to the broken disk text: {published}"
        );
    });
}

#[test]
fn unopened_files_report_disk_diagnostics() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (_scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        lsp::write(&play, &broken_play());

        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(None).await;
        let published = server.wait_diagnostics(&play, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-009".to_string()),
            "{published}"
        );
        assert!(
            published.get("version").is_none_or(Value::is_null),
            "no overlay, no version: {published}"
        );
    });
}

#[test]
fn a_burst_of_changes_converges_on_the_last_version() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let (_scene, play) = lsp::standard_workspace(base, &[("web1", "web")]);
        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;

        let mark = server.mark();
        server.open(&play, &broken_play(), 1).await;
        let published = server.wait_version(&play, 1, mark).await;
        assert!(lsp::codes(&published).contains(&"IMP-DSL-009".to_string()));

        // Rapid fixes: the debounce coalesces the burst, the last version wins.
        let mark = server.mark();
        for version in 2..=6 {
            let text = if version % 2 == 0 {
                lsp::PLAY
            } else {
                &broken_play()
            };
            server.change(&play, text, version).await;
        }
        let published = server.wait_version(&play, 6, mark).await;
        assert!(
            lsp::diagnostics(&published).is_empty(),
            "the last buffer is clean: {published}"
        );
    });
}

const EMOJI_SCENE: &str = r##"Scene: Emoji
Platforms: cross-platform

| Piece | web | 🍎bada |
|---|---|---|
| A | #a | x |
"##;

/// `🍎bada` is an unknown platform column: its span starts before the emoji,
/// so the end column depends on the negotiated encoding (the emoji is a
/// surrogate pair in UTF-16 and four bytes in UTF-8).
async fn encoding_case(offered: Option<&[&str]>, expected_encoding: &str, start: u64, end: u64) {
    let temp = tempfile::tempdir().expect("tempdir");
    let base = temp.path();
    lsp::write(&base.join("impresario.config"), lsp::config_text());
    lsp::write(
        &base.join("impresario.devices.config"),
        &lsp::devices_text(&[("web1", "web")]),
    );
    let scene = base.join("scenes/Emoji.scene");
    lsp::write(&scene, EMOJI_SCENE);

    let mut server = TestLsp::start(base).await;
    let result = server.initialize(offered).await;
    assert_eq!(
        result["capabilities"]["positionEncoding"].as_str(),
        Some(expected_encoding),
        "negotiated encoding"
    );
    server.notify("initialized", json!({})).await;

    let mark = server.mark();
    server.open(&scene, EMOJI_SCENE, 1).await;
    let published = server.wait_version(&scene, 1, mark).await;
    let diagnostic = lsp::diagnostics(&published)
        .into_iter()
        .find(|diagnostic| diagnostic.get("code").and_then(Value::as_str) == Some("IMP-DSL-011"))
        .expect("the unknown selector key is reported");
    assert_eq!(lsp::range_start(&diagnostic), (3, start));
    assert_eq!(lsp::range_end(&diagnostic), (3, end));
}

#[test]
fn the_default_encoding_is_utf16_with_surrogate_pairs() {
    runtime().block_on(encoding_case(None, "utf-16", 16, 22));
}

#[test]
fn utf8_negotiation_shifts_offsets_past_wide_characters() {
    runtime().block_on(encoding_case(Some(&["utf-8"]), "utf-8", 16, 24));
}

#[test]
fn config_files_report_their_own_diagnostics() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        let config = base.join("impresario.config");
        lsp::write(&config, "timeout_default_ms: banana\n");
        lsp::write(
            &base.join("impresario.devices.config"),
            &lsp::devices_text(&[("web1", "web")]),
        );

        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(None).await;
        let published = server.wait_diagnostics(&config, mark).await;
        // Since TASK-2 section 4.1 every key has a default, so a file with only
        // an invalid value reports that value rather than a missing key.
        assert!(
            lsp::codes(&published).contains(&"IMP-CFG-003".to_string()),
            "the invalid timeout value is reported: {published}"
        );
        let diagnostic = lsp::diagnostics(&published)
            .into_iter()
            .find(|diagnostic| {
                diagnostic.get("code").and_then(Value::as_str) == Some("IMP-CFG-003")
            })
            .expect("config diagnostic");
        assert_eq!(diagnostic.get("severity").and_then(Value::as_i64), Some(1));
    });
}

const PARTIAL_SCENE: &str = r##"Scene: Partial
Platforms: cross-platform

| Piece | android |
|---|---|
| OnlyAndroid | id:only |
"##;

const PARTIAL_THEATRE: &str = r#"Theatre: Partial
Stage: S
    Scene: Partial
        Check: OnlyAndroid
"#;

const PARTIAL_PLAY: &str = r#"Play: Partial

Act: A
    Perform: S
"#;

#[test]
fn device_warnings_follow_did_change_configuration() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        lsp::write(&base.join("impresario.config"), lsp::config_text());
        lsp::write(
            &base.join("impresario.devices.config"),
            &lsp::devices_text(&[("web1", "web")]),
        );
        lsp::write(
            &base.join("android.devices.config"),
            &lsp::devices_text(&[("android1", "android")]),
        );
        let scene = base.join("scenes/Partial.scene");
        lsp::write(&scene, PARTIAL_SCENE);
        let theatre = base.join("theatres/partial.theatre");
        lsp::write(&theatre, PARTIAL_THEATRE);
        let play = base.join("plays/partial.play");
        lsp::write(&play, PARTIAL_PLAY);

        let mut server = TestLsp::start(base).await;
        let mark = server.mark();
        server.start_session(None).await;
        // The warning sits on the step that uses the piece.
        let published = server.wait_diagnostics(&play, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-012".to_string()),
            "a web device wants a web selector: {published}"
        );

        // Switch to an android-only device set: the warning disappears.
        let mark = server.mark();
        server
            .did_change_configuration(json!({
                "impresario": {"devices": "android.devices.config"}
            }))
            .await;
        let published = server.wait_diagnostics(&play, mark).await;
        assert!(
            lsp::diagnostics(&published).is_empty(),
            "android covers the piece: {published}"
        );

        // And back through the dotted settings shape.
        let mark = server.mark();
        server
            .did_change_configuration(json!({
                "impresario.devices": "impresario.devices.config"
            }))
            .await;
        let published = server.wait_diagnostics(&play, mark).await;
        assert!(
            lsp::codes(&published).contains(&"IMP-DSL-012".to_string()),
            "back on the web device set: {published}"
        );
    });
}

#[test]
fn unsaved_new_plays_outside_the_configured_directories_still_report() {
    runtime().block_on(async {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path();
        lsp::standard_workspace(base, &[("web1", "web")]);
        let scratch = base.join("scratch.play"); // never written to disk

        let mut server = TestLsp::start(base).await;
        server.start_session(None).await;
        let mark = server.mark();
        server.open(&scratch, "Act: A\n", 1).await;
        let published = server.wait_version(&scratch, 1, mark).await;
        assert!(
            !lsp::diagnostics(&published).is_empty(),
            "a play without its 'Play:' header: {published}"
        );
    });
}
