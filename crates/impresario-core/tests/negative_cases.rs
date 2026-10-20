//! The Phase-2 negative-case matrix (spec 5.7/17).
//!
//! At least one case per `IMP-DSL-*` code, plus edge cases. Parser and
//! scanner cases assert that the expected code is *among* the diagnostics
//! (the parser recovers, so cascades are legitimate); semantic cases run the
//! `validation` pass with configured device refs, mirroring `impresario
//! validate`.

use std::path::Path;

use impresario_core::diag::Code;
use impresario_core::parser::{parse_play, parse_scene};
use impresario_core::platforms::Platform;
use impresario_core::validation::{validate, DeviceRef};
use impresario_core::workspace::{self, Workspace};

/// One parser-level case.
struct Case {
    name: &'static str,
    source: String,
    expect: &'static [Code],
}

/// One workspace-level case (parsed scene + play + device list).
struct SemanticCase {
    name: &'static str,
    scene: &'static str,
    scene_path: &'static str,
    play: &'static str,
    devices: &'static [(&'static str, Platform)],
    expect: &'static [Code],
}

fn assert_codes(name: &str, found: &[Code], expect: &[Code]) {
    assert!(!found.is_empty(), "case '{name}': no diagnostics produced");
    for code in expect {
        assert!(
            found.contains(code),
            "case '{name}': expected {code} ({}) among {found:?}",
            code.as_str()
        );
    }
}

/// A play whose single stage/scene block contains `step_lines`.
fn play_of(step_lines: &[&str]) -> String {
    let mut out = String::from("Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n");
    for line in step_lines {
        out.push_str("        ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A play around `body` lines placed after the `Act: A` line at depth 0.
fn play_body(body: &str) -> String {
    format!("Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n{body}")
}

// ---------------------------------------------------------------------------
// Scene files
// ---------------------------------------------------------------------------

fn scene_cases() -> Vec<Case> {
    let scene = |name: &'static str, source: &str, expect: &'static [Code]| Case {
        name,
        source: source.to_string(),
        expect,
    };
    vec![
        scene("scene-tab-indentation", "Scene: A\n\tPieces:\n", &[Code::DslTabIndentation]),
        scene(
            "scene-inconsistent-indent",
            "Scene: A\nPlatform: web\nPieces:\n    A: web=\"#a\"\n  B: web=\"#b\"\n",
            &[Code::DslInconsistentIndent],
        ),
        scene("scene-missing-header", "Platform: web\n", &[Code::DslMissingHeader]),
        scene("scene-empty-file", "", &[Code::DslMissingHeader]),
        scene("scene-name-missing", "Scene:\nPlatform: web\n", &[Code::DslMissingHeader]),
        scene(
            "scene-platform-line-missing",
            "Scene: A\nPieces:\n    P: web=\"#p\"\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-platform-empty",
            "Scene: A\nPlatform:\nPieces:\n    P: web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-platform-unknown",
            "Scene: A\nPlatform: window\nPieces:\n    P: web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-platform-no-comma",
            "Scene: A\nPlatform: web webos\nPieces:\n    P: web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-pieces-header-missing",
            "Scene: A\nPlatform: web\n\nSomething: x\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-piece-not-indented",
            "Scene: A\nPlatform: web\nPieces:\nP: web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-piece-no-name",
            "Scene: A\nPlatform: web\nPieces:\n    : web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-piece-no-colon",
            "Scene: A\nPlatform: web\nPieces:\n    P web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-piece-no-selector",
            "Scene: A\nPlatform: web\nPieces:\n    P:\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-selector-unknown-key",
            "Scene: A\nPlatform: web\nPieces:\n    P: windows=\"#p\"\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-selector-missing-equals",
            "Scene: A\nPlatform: web\nPieces:\n    P: web \"#p\"\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-selector-not-double-quoted",
            "Scene: A\nPlatform: web\nPieces:\n    P: web='#p'\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-selector-unterminated",
            "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#p\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-selector-key-twice",
            "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#a\", web=\"#b\"\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-index-twice",
            "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#a\", index=1, index=2\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-index-not-integer",
            "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#a\", index=x\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-piece-trailing-junk",
            "Scene: A\nPlatform: web\nPieces:\n    P: web=\"#a\" junk\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-duplicate-piece",
            "Scene: A\nPlatform: web\nPieces:\n    Title: web=\"#a\"\n    Title: web=\"#b\"\n",
            &[Code::DslDuplicatePiece],
        ),
        scene(
            "scene-name-trailing-input",
            "Scene: A B\nPlatform: web\nPieces:\n    P: web=\"#p\"\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-no-pieces",
            "Scene: A\nPlatform: web\nPieces:\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-multiple-bad-pieces-recovered",
            "Scene: A\nPlatform: web\nPieces:\n    BadOne: nope=\"#a\"\n    BadTwo: web='#b'\n    Good: web=\"#c\"\n",
            &[Code::DslInvalidSelector, Code::DslInvalidSelector],
        ),
    ]
}

#[test]
fn scene_negative_cases() {
    for case in scene_cases() {
        let outcome = parse_scene(&case.source, Path::new("case.scene"));
        let found: Vec<Code> = outcome.diagnostics.iter().map(|d| d.code).collect();
        assert_codes(case.name, &found, case.expect);
    }
}

// ---------------------------------------------------------------------------
// Play files
// ---------------------------------------------------------------------------

fn play_cases() -> Vec<Case> {
    let play = |name: &'static str, source: String, expect: &'static [Code]| Case {
        name,
        source,
        expect,
    };
    vec![
        play("play-empty-file", String::new(), &[Code::DslMissingHeader]),
        play("play-missing-header", "Act: A\n".to_string(), &[Code::DslMissingHeader]),
        play("play-name-empty", "Play:\n".to_string(), &[Code::DslMissingHeader]),
        play(
            "play-tab-indentation",
            "Play: P\n\tAct: A\n".to_string(),
            &[Code::DslTabIndentation],
        ),
        play(
            "play-unknown-keyword",
            "Play: P\n\nBogus: x\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-description-misplaced",
            "Play: P\nVar: X = 'x'\nDescription: late\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-var-after-act",
            format!("{}Var: X = 'x'\n", play_body("")),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-var-missing-equals",
            "Play: P\nVar: X 'x'\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-var-value-not-quoted",
            "Play: P\nVar: X = x\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnterminatedString],
        ),
        play(
            "play-var-unterminated",
            "Play: P\nVar: X = 'x\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnterminatedString],
        ),
        play(
            "play-var-trailing-junk",
            "Play: P\nVar: X = 'x' junk\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-act-name-empty",
            "Play: P\n\nAct:\n\nStage: S\n    Scene: Screen\n        Check: A\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play("play-no-acts", "Play: P\n".to_string(), &[Code::DslMissingHeader]),
        play(
            "play-act-without-stage",
            "Play: P\n\nAct: A\n\nAct: B\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-indented-without-block",
            "Play: P\n    stray\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-stage-unknown-keyword",
            "Play: P\n\nAct: A\n\nStage: S\n    Steps: x\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-stage-misplaced-keyword",
            "Play: P\n\nAct: A\n\nStage: S\n    Act: B\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-scene-block-name-missing",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene:\n        Check: A\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-scene-block-trailing-input",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen extra\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-scene-block-no-steps",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n    Scene: Screen2\n        Check: A\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-scene-block-unknown-step",
            play_of(&["Touch: X"]),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-scene-block-keyword",
            play_of(&["Scene: Other"]),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-scene-block-bad-indent",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n            Check: B\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "action-unknown-verb",
            play_of(&["Action: tap => P"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "action-shorthand-no-arrow",
            play_of(&["Action: 'text' P"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "action-type-no-piece",
            play_of(&["Action: type 'x' =>"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "action-unterminated-string",
            play_of(&["Action: type 'x => P"]),
            &[Code::DslUnterminatedString],
        ),
        play(
            "action-unknown-direction",
            play_of(&["Action: scroll sideways"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "action-missing-direction",
            play_of(&["Action: swipe"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "action-unknown-key-name",
            play_of(&["Action: key stop"]),
            &[Code::DslUnknownKeyName],
        ),
        play(
            "action-wait-no-duration",
            play_of(&["Action: wait"]),
            &[Code::DslInvalidDuration],
        ),
        play(
            "action-wait-zero-duration",
            play_of(&["Action: wait 0s"]),
            &[Code::DslInvalidDuration],
        ),
        play(
            "action-wait-too-long",
            play_of(&["Action: wait 11m"]),
            &[Code::DslInvalidDuration],
        ),
        play(
            "action-timeout-bad-unit",
            play_of(&["Action: click => P timeout 5x"]),
            &[Code::DslInvalidDuration],
        ),
        play(
            "action-timeout-forbidden-back",
            play_of(&["Action: back timeout 5s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        play(
            "action-timeout-forbidden-key",
            play_of(&["Action: key ok timeout 5s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        play(
            "action-timeout-forbidden-wait",
            play_of(&["Action: wait 1s timeout 2s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        play(
            "action-timeout-forbidden-screenshot",
            play_of(&["Action: screenshot 's' timeout 1s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        play(
            "action-timeout-forbidden-app",
            play_of(&["Action: app launch timeout 1s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        play(
            "action-trailing-input",
            play_of(&["Action: click => P junk"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "action-app-unknown-op",
            play_of(&["Action: app stop"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "check-no-piece",
            play_of(&["Check:"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "check-unknown-condition",
            play_of(&["Check: P is frobnicated"]),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "check-trailing-input",
            play_of(&["Check: P junk"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "check-text-unterminated",
            play_of(&["Check: P text 'x"]),
            &[Code::DslUnterminatedString],
        ),
        play(
            "check-count-not-integer",
            play_of(&["Check: P count x"]),
            &[Code::DslUnknownAction],
        ),
        play(
            "check-timeout-bad-duration",
            play_of(&["Check: P timeout 0ms"]),
            &[Code::DslInvalidDuration],
        ),
        play(
            "act-tags-invalid",
            "Play: P\n\nAct: A\nTags: Smoke\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "act-tags-empty",
            "Play: P\n\nAct: A\nTags:\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "act-platforms-unknown",
            "Play: P\n\nAct: A\nPlatforms: window\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "act-platforms-empty",
            "Play: P\n\nAct: A\nPlatforms:\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "duplicate-act",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n\nAct: A\n\nStage: T\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslDuplicateActStage],
        ),
        play(
            "duplicate-stage",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslDuplicateActStage],
        ),
        play(
            "stage-timeout-twice",
            "Play: P\n\nAct: A\n\nStage: S\n    Timeout: 1s\n    Timeout: 2s\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslDuplicateActStage],
        ),
        play(
            "stage-timeout-after-scene",
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n    Timeout: 1s\n    Scene: Screen2\n        Check: A\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "stage-timeout-bad-duration",
            "Play: P\n\nAct: A\n\nStage: S\n    Timeout: 0ms\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslInvalidDuration],
        ),
    ]
}

#[test]
fn play_negative_cases() {
    for case in play_cases() {
        let outcome = parse_play(&case.source, Path::new("case.play"));
        let found: Vec<Code> = outcome.diagnostics.iter().map(|d| d.code).collect();
        assert_codes(case.name, &found, case.expect);
    }
}

// ---------------------------------------------------------------------------
// Semantic validation (scene/piece/variable resolution, platform rules)
// ---------------------------------------------------------------------------

fn semantic_cases() -> Vec<SemanticCase> {
    const ANDROID: &[(&str, Platform)] = &[("flagship_android", Platform::Android)];
    vec![
        SemanticCase {
            name: "unknown-scene",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: Missing\n        Check: A\n",
            devices: ANDROID,
            expect: &[Code::DslUnknownScene],
        },
        SemanticCase {
            name: "unknown-piece",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Check: Nope\n",
            devices: ANDROID,
            expect: &[Code::DslUnknownPiece],
        },
        SemanticCase {
            name: "missing-selector-for-platform",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    Title: web=\"h1\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Check: Title\n",
            devices: ANDROID,
            expect: &[Code::DslMissingSelectorForPlatform],
        },
        SemanticCase {
            name: "unsupported-action-on-platform",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    List: web=\"#l\", android=\"id:l\", ios=\"accessibilityId:l\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Action: swipe up => List\n",
            devices: &[("living_room_tv", Platform::WebOs)],
            expect: &[Code::DslUnsupportedActionOnPlatform],
        },
        SemanticCase {
            name: "unsupported-key-on-platform",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\", ios=\"accessibilityId:a\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Action: key rewind\n",
            devices: &[("test_iphone", Platform::Ios)],
            expect: &[Code::DslUnsupportedActionOnPlatform],
        },
        SemanticCase {
            name: "unsupported-check-on-platform",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\", ios=\"accessibilityId:a\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Check: A is focused\n",
            devices: &[("test_iphone", Platform::Ios)],
            expect: &[Code::DslUnsupportedActionOnPlatform],
        },
        SemanticCase {
            name: "undefined-variable",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    Field: web=\"#f\", android=\"id:f\"\n",
            scene_path: "S1.scene",
            play: "Play: P\nVar: USER = 'u'\n\nAct: A\n\nStage: S\n    Scene: S1\n        Action: '${var.MISSING}' => Field\n",
            devices: ANDROID,
            expect: &[Code::DslUndefinedVariable],
        },
        SemanticCase {
            name: "scene-not-applicable",
            scene: "Scene: TvOnly\nPlatform: webos, tizen\n\nPieces:\n    Title: web=\"h1\", android=\"id:t\"\n",
            scene_path: "TvOnly.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: TvOnly\n        Check: Title\n",
            devices: &[
                ("living_room_tv", Platform::WebOs),
                ("flagship_android", Platform::Android),
            ],
            expect: &[Code::DslSceneNotApplicable],
        },
        SemanticCase {
            name: "lint-fixed-wait",
            scene: "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
            scene_path: "S1.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Action: wait 2s\n",
            devices: &[("site", Platform::Web)],
            expect: &[Code::DslLintFixedWait],
        },
        SemanticCase {
            name: "lint-naming",
            scene: "Scene: login_screen\nPlatform: cross-platform\n\nPieces:\n    ok_button: web=\"#b\"\n",
            scene_path: "login_screen.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: login_screen\n        Check: ok_button\n",
            devices: &[("site", Platform::Web)],
            expect: &[Code::DslLintNaming],
        },
        SemanticCase {
            name: "lint-scene-file-mismatch",
            scene: "Scene: LoginScreen\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
            scene_path: "other.scene",
            play: "Play: P\n\nAct: A\n\nStage: S\n    Scene: LoginScreen\n        Check: A\n",
            devices: &[("site", Platform::Web)],
            expect: &[Code::DslLintSceneFileMismatch],
        },
    ]
}

#[test]
fn semantic_negative_cases() {
    for case in semantic_cases() {
        let scene = parse_scene(case.scene, Path::new(case.scene_path))
            .ast
            .unwrap_or_else(|| panic!("case '{}': scene must parse", case.name));
        let play = parse_play(case.play, Path::new("case.play"))
            .ast
            .unwrap_or_else(|| panic!("case '{}': play must parse", case.name));
        let workspace = Workspace {
            scenes: vec![scene],
            theatres: Vec::new(),
            plays: vec![play],
        };
        let refs: Vec<DeviceRef> = case
            .devices
            .iter()
            .map(|(name, platform)| DeviceRef::new(*name, *platform))
            .collect();
        let found: Vec<Code> = validate(&workspace, &refs).iter().map(|d| d.code).collect();
        assert_codes(case.name, &found, case.expect);
    }
}

#[test]
fn duplicate_scene_across_workspace_is_imp_dsl_006() {
    let dir = std::env::temp_dir().join(format!("impresario-negcases-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let one = dir.join("one.scene");
    let two = dir.join("two.scene");
    std::fs::write(
        &one,
        "Scene: Login\nPlatform: cross-platform\n\nPieces:\n    A: web=\"#a\"\n",
    )
    .expect("write one");
    std::fs::write(
        &two,
        "Scene: Login\nPlatform: cross-platform\n\nPieces:\n    B: web=\"#b\"\n",
    )
    .expect("write two");
    let loaded = workspace::load_paths(&[one, two]);
    let found: Vec<Code> = loaded.diagnostics.iter().map(|d| d.code).collect();
    assert!(
        found.contains(&Code::DslDuplicateScene),
        "expected IMP-DSL-006 among {found:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Matrix bookkeeping: size and per-code coverage
// ---------------------------------------------------------------------------

/// Codes covered by dedicated tests rather than this table:
/// `IMP-DSL-006` (`duplicate_scene_across_workspace_is_imp_dsl_006`) and the
/// theatrical/workspace-lint family, whose fixtures need a `.theatre` or a
/// full workspace: parser tests (`duplicate_stage`, `duplicate_parameter`,
/// `examples_width`, `unknown_example_column`), validation tests (031/032/
/// 034/036/037/040/041 and 042..050), and the planner's `--arg` tests (032).
const DEDICATED: &[Code] = &[
    Code::DslDuplicateScene,
    Code::DslDuplicateStage,
    Code::DslUnknownStage,
    Code::DslInvalidStageArguments,
    Code::DslDuplicateTheatre,
    Code::DslStageCycle,
    Code::DslDuplicateParameter,
    Code::DslUnknownParameter,
    Code::DslMissingStageArgument,
    Code::DslDuplicateExampleColumn,
    Code::DslExampleWidth,
    Code::DslUnknownExampleColumn,
    Code::DslStageDepth,
    Code::DslLintUnusedScene,
    Code::DslLintUnusedPiece,
    Code::DslLintUnusedStage,
    Code::DslLintUnusedTheatre,
    Code::DslLintUnusedVar,
    Code::DslLintUnusedExampleColumn,
    Code::DslLintShadowedVar,
    Code::DslLintActPlatformCoverage,
    Code::DslLintRequiredParamNeverSupplied,
];

const ALL_DSL_CODES: &[Code] = &[
    Code::DslTabIndentation,
    Code::DslInconsistentIndent,
    Code::DslUnterminatedString,
    Code::DslUnknownKeyword,
    Code::DslMissingHeader,
    Code::DslDuplicateScene,
    Code::DslDuplicatePiece,
    Code::DslUnknownScene,
    Code::DslUnknownPiece,
    Code::DslUnknownAction,
    Code::DslInvalidSelector,
    Code::DslMissingSelectorForPlatform,
    Code::DslUnsupportedActionOnPlatform,
    Code::DslUndefinedVariable,
    Code::DslInvalidDuration,
    Code::DslDuplicateActStage,
    Code::DslSceneNotApplicable,
    Code::DslUnknownKeyName,
    Code::DslLintFixedWait,
    Code::DslLintNaming,
    Code::DslLintSceneFileMismatch,
    Code::DslTimeoutNotAllowed,
    Code::DslDuplicateStage,
    Code::DslUnknownStage,
    Code::DslInvalidStageArguments,
    Code::DslDuplicateTheatre,
    Code::DslStageCycle,
    Code::DslDuplicateParameter,
    Code::DslUnknownParameter,
    Code::DslMissingStageArgument,
    Code::DslDuplicateExampleColumn,
    Code::DslExampleWidth,
    Code::DslUnknownExampleColumn,
    Code::DslStageDepth,
    Code::DslLintUnusedScene,
    Code::DslLintUnusedPiece,
    Code::DslLintUnusedStage,
    Code::DslLintUnusedTheatre,
    Code::DslLintUnusedVar,
    Code::DslLintUnusedExampleColumn,
    Code::DslLintShadowedVar,
    Code::DslLintActPlatformCoverage,
    Code::DslLintRequiredParamNeverSupplied,
];

#[test]
fn matrix_covers_at_least_60_cases_and_every_code() {
    let scene = scene_cases();
    let play = play_cases();
    let semantic = semantic_cases();
    let total = scene.len() + play.len() + semantic.len() + DEDICATED.len();
    assert!(total >= 60, "negative matrix has only {total} cases");

    let mut expected: Vec<Code> = Vec::new();
    for case in scene.iter().chain(play.iter()) {
        expected.extend(case.expect);
    }
    for case in &semantic {
        expected.extend(case.expect);
    }
    expected.extend(DEDICATED);
    for code in ALL_DSL_CODES {
        assert!(
            expected.contains(code),
            "{code} ({}) has no negative case",
            code.as_str()
        );
    }
}

// ---------------------------------------------------------------------------
// Edge cases that are positive (must NOT produce the code)
// ---------------------------------------------------------------------------

#[test]
fn bom_and_crlf_are_tolerated() {
    let outcome = parse_scene(
        "\u{feff}Scene: A\r\nPlatform: web\r\nPieces:\r\n    P: web=\"#p\"\r\n",
        Path::new("A.scene"),
    );
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    assert!(outcome.ast.is_some());
}

#[test]
fn free_text_limit_is_200_characters() {
    let long = "x".repeat(201);
    let ok = "x".repeat(200);
    let outcome = parse_play(&format!("Play: {long}\n"), Path::new("case.play"));
    assert!(outcome
        .diagnostics
        .iter()
        .any(|d| d.code == Code::DslMissingHeader));
    let outcome = parse_play(
        &format!("Play: {ok}\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n"),
        Path::new("case.play"),
    );
    assert!(
        !outcome.diagnostics.has_errors(),
        "{:?}",
        outcome.diagnostics
    );
    let outcome = parse_play(
        &format!("Play: P\n\nAct: {long}\n\nStage: S\n    Scene: Screen\n        Check: A\n"),
        Path::new("case.play"),
    );
    assert!(outcome
        .diagnostics
        .iter()
        .any(|d| d.code == Code::DslMissingHeader));
}

#[test]
fn valid_entry_and_check_verbs_have_no_diagnostics() {
    let outcome = parse_play(
        &play_of(&[
            "Action: 'abc' => Field",
            "Action: type 'abc' => Field",
            "Action: append 'abc' => Field",
            "Action: clear => Field",
            "Action: key ok",
            "Action: back",
            "Action: wait 1s",
            "Action: screenshot 's'",
            "Action: app launch",
            "Check: Field",
            "Check: Field textContains 'abc'",
        ]),
        Path::new("case.play"),
    );
    assert!(
        !outcome.diagnostics.has_errors(),
        "{:?}",
        outcome.diagnostics
    );
}
