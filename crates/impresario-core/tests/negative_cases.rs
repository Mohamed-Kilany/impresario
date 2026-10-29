//! The Phase-2 negative-case matrix (spec 5.7/17).
//!
//! At least one case per `IMP-DSL-*` code, plus edge cases. Parser and
//! scanner cases assert that the expected code is *among* the diagnostics
//! (the parser recovers, so cascades are legitimate); semantic cases run the
//! `validation` pass with configured device refs, mirroring `impresario
//! validate`.

use std::path::Path;

use impresario_core::ast::TheatreFile;
use impresario_core::diag::Code;
use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
use impresario_core::platforms::Platform;
use impresario_core::validation::{validate, DeviceRef};
use impresario_core::workspace::{self, Workspace};

/// One parser-level case.
struct Case {
    name: &'static str,
    source: String,
    expect: &'static [Code],
}

/// One workspace-level case (parsed scene + theatre + play + device list).
struct SemanticCase {
    name: &'static str,
    scene: &'static str,
    scene_path: &'static str,
    theatre: &'static str,
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

/// A theatre whose single stage/scene block contains `step_lines`.
fn theatre_of(step_lines: &[&str]) -> String {
    let mut out = String::from("Theatre: T\n\nStage: S\n    Scene: Screen\n");
    for line in step_lines {
        out.push_str("        ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A theatre around `body` lines placed at depth 0 after a valid stage.
fn theatre_body(body: &str) -> String {
    format!("Theatre: T\n\nStage: S\n    Scene: Screen\n        Check: A\n{body}")
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
        scene(
            "scene-tab-indentation",
            "Scene: A\n\t| Piece | web |\n",
            &[Code::DslTabIndentation],
        ),
        scene(
            "scene-inconsistent-indent",
            "Scene: A\n\n| Piece | web |\n|---|---|\n    | P | #p |\n      | Q | #q |\n",
            &[Code::DslInconsistentIndent],
        ),
        scene(
            "scene-missing-header",
            "Platforms: web\n\n| Piece | web |\n",
            &[Code::DslMissingHeader],
        ),
        scene("scene-empty-file", "", &[Code::DslMissingHeader]),
        scene(
            "scene-name-missing",
            "Scene:\n\n| Piece | web |\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-legacy-pieces-form-is-rejected",
            "Scene: A\nPlatforms: web\nPieces:\n    P: web=\"#p\"\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-singular-platform-line-is-rejected",
            "Scene: A\nPlatform: web\n\n| Piece | web |\n|---|---|\n| P | #p |\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-header-first-column",
            "Scene: A\n\n| Item | web |\n|---|---|\n| P | #p |\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-empty-column",
            "Scene: A\n\n| Piece |  |\n|---|---|\n| P | #p |\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-unknown-column",
            "Scene: A\n\n| Piece | web | windows |\n|---|---|---|\n| P | #p | - |\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-duplicate-column",
            "Scene: A\n\n| Piece | web | web |\n|---|---|---|\n| P | #p | #q |\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-duplicate-index",
            "Scene: A\n\n| Piece | index | index |\n|---|---|---|\n| P | 1 | 2 |\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-row-width",
            "Scene: A\n\n| Piece | web | android |\n|---|---|---|\n| P | #p |\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-piece-name-invalid",
            "Scene: A\n\n| Piece | web |\n|---|---|\n| 1Bad | #p |\n",
            &[Code::DslDuplicatePiece],
        ),
        scene(
            "scene-piece-name-empty",
            "Scene: A\n\n| Piece | web |\n|---|---|\n|  | #p |\n",
            &[Code::DslDuplicatePiece],
        ),
        scene(
            "scene-duplicate-piece",
            "Scene: A\n\n| Piece | web |\n|---|---|\n| Title | #a |\n| Title | #b |\n",
            &[Code::DslDuplicatePiece],
        ),
        scene(
            "scene-index-not-integer",
            "Scene: A\n\n| Piece | web | index |\n|---|---|---|\n| P | #p | x |\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-selector-invalid",
            "Scene: A\n\n| Piece | android |\n|---|---|\n| P | id: |\n",
            &[Code::DslInvalidSelector],
        ),
        scene(
            "scene-no-pieces",
            "Scene: A\n\n| Piece | web |\n|---|---|\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-name-trailing-input",
            "Scene: A B\n\n| Piece | web |\n|---|---|\n| P | #p |\n",
            &[Code::DslMissingHeader],
        ),
        scene(
            "scene-platform-unknown",
            "Scene: A\nPlatforms: window\n\n| Piece | web |\n|---|---|\n| P | #p |\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-platform-empty",
            "Scene: A\nPlatforms:\n\n| Piece | web |\n|---|---|\n| P | #p |\n",
            &[Code::DslUnknownKeyword],
        ),
        scene(
            "scene-multiple-bad-pieces-recovered",
            "Scene: A\n\n| Piece | android |\n|---|---|\n| BadOne | id: |\n| BadTwo | id: |\n| Good | id:c |\n",
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
        play(
            "play-missing-header",
            "Act: A\n    Perform: S\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play("play-name-empty", "Play:\n".to_string(), &[Code::DslMissingHeader]),
        play(
            "play-tab-indentation",
            "Play: P\n\tAct: A\n".to_string(),
            &[Code::DslTabIndentation],
        ),
        play(
            "play-unknown-keyword",
            "Play: P\n\nBogus: x\n\nAct: A\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-description-misplaced",
            "Play: P\nVar: X = 'x'\nDescription: late\n\nAct: A\n    Perform: S\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-var-after-act",
            "Play: P\n\nAct: A\n    Perform: S\nVar: X = 'x'\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-var-missing-equals",
            "Play: P\nVar: X 'x'\n\nAct: A\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-var-value-not-quoted",
            "Play: P\nVar: X = x\n\nAct: A\n    Perform: S\n".to_string(),
            &[Code::DslUnterminatedString],
        ),
        play(
            "play-var-unterminated",
            "Play: P\nVar: X = 'x\n\nAct: A\n    Perform: S\n".to_string(),
            &[Code::DslUnterminatedString],
        ),
        play(
            "play-var-trailing-junk",
            "Play: P\nVar: X = 'x' junk\n\nAct: A\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "play-act-name-empty",
            "Play: P\n\nAct:\n    Perform: S\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play("play-no-acts", "Play: P\n".to_string(), &[Code::DslMissingHeader]),
        play(
            "play-act-without-perform",
            "Play: P\n\nAct: A\n\nAct: B\n    Perform: S\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-indented-without-block",
            "Play: P\n    stray\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "play-inline-stage-is-rejected",
            "Play: P\n\nAct: A\n    Perform: S\n    Stage: S\n        Scene: Screen\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "act-tags-invalid",
            "Play: P\n\nAct: A\nTags: Smoke\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "act-tags-empty",
            "Play: P\n\nAct: A\nTags:\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "act-platforms-unknown",
            "Play: P\n\nAct: A\nPlatforms: window\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "act-platforms-empty",
            "Play: P\n\nAct: A\nPlatforms:\n    Perform: S\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        play(
            "duplicate-act",
            "Play: P\n\nAct: A\n    Perform: S\n\nAct: A\n    Perform: T\n".to_string(),
            &[Code::DslDuplicateActStage],
        ),
        play(
            "perform-missing-stage-name",
            "Play: P\n\nAct: A\n    Perform:\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        play(
            "examples-duplicate-column",
            "Play: P\n\nAct: A\n    Examples:\n        | user | user |\n        | a | b |\n    Perform: S(user = '${example.user}')\n"
                .to_string(),
            &[Code::DslDuplicateExampleColumn],
        ),
        play(
            "examples-width",
            "Play: P\n\nAct: A\n    Examples:\n        | user | password |\n        | alice |\n    Perform: S(user = '${example.user}')\n"
                .to_string(),
            &[Code::DslExampleWidth],
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
// Theatre files (step grammar)
// ---------------------------------------------------------------------------

fn theatre_cases() -> Vec<Case> {
    let theatre = |name: &'static str, source: String, expect: &'static [Code]| Case {
        name,
        source,
        expect,
    };
    vec![
        theatre("theatre-empty-file", String::new(), &[Code::DslMissingHeader]),
        theatre(
            "theatre-missing-header",
            "Stage: S\n    Scene: Screen\n        Check: A\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        theatre(
            "theatre-name-empty",
            "Theatre:\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        theatre(
            "theatre-tab-indentation",
            "Theatre: T\n\tStage: S\n".to_string(),
            &[Code::DslTabIndentation],
        ),
        theatre(
            "theatre-duplicate-parameter",
            "Theatre: T\n\nStage: S(user, user)\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslDuplicateParameter],
        ),
        theatre(
            "theatre-stage-name-missing",
            "Theatre: T\n\nStage:\n    Scene: Screen\n        Check: A\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        theatre(
            "theatre-stage-unknown-keyword",
            "Theatre: T\n\nStage: S\n    Steps: x\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        theatre(
            "theatre-stage-legacy-platforms-is-rejected",
            "Theatre: T\n\nStage: S\n    Platforms: web\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslUnknownKeyword],
        ),
        theatre(
            "theatre-timeout-twice",
            "Theatre: T\n\nStage: S\n    Timeout: 1s\n    Timeout: 2s\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslDuplicateActStage],
        ),
        theatre(
            "theatre-timeout-bad-duration",
            "Theatre: T\n\nStage: S\n    Timeout: 0ms\n    Scene: Screen\n        Check: A\n"
                .to_string(),
            &[Code::DslInvalidDuration],
        ),
        theatre(
            "theatre-scene-block-name-missing",
            "Theatre: T\n\nStage: S\n    Scene:\n        Check: A\n".to_string(),
            &[Code::DslMissingHeader],
        ),
        theatre(
            "theatre-scene-block-trailing-input",
            "Theatre: T\n\nStage: S\n    Scene: Screen extra\n        Check: A\n".to_string(),
            &[Code::DslUnknownKeyword],
        ),
        theatre(
            "theatre-scene-block-no-steps",
            "Theatre: T\n\nStage: S\n    Scene: Screen\n    Scene: Screen2\n        Check: A\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        theatre(
            "theatre-scene-block-unknown-step",
            theatre_of(&["Touch: X"]),
            &[Code::DslUnknownKeyword],
        ),
        theatre(
            "theatre-scene-block-keyword",
            theatre_of(&["Scene: Other"]),
            &[Code::DslUnknownKeyword],
        ),
        theatre(
            "theatre-scene-block-bad-indent",
            "Theatre: T\n\nStage: S\n    Scene: Screen\n        Check: A\n            Check: B\n"
                .to_string(),
            &[Code::DslMissingHeader],
        ),
        theatre(
            "action-unknown-verb",
            theatre_of(&["Action: tap => P"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "action-shorthand-no-arrow",
            theatre_of(&["Action: 'text' P"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "action-type-no-piece",
            theatre_of(&["Action: type 'x' =>"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "action-unterminated-string",
            theatre_of(&["Action: type 'x => P"]),
            &[Code::DslUnterminatedString],
        ),
        theatre(
            "action-unknown-direction",
            theatre_of(&["Action: scroll sideways"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "action-missing-direction",
            theatre_of(&["Action: swipe"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "action-unknown-key-name",
            theatre_of(&["Action: key frobnicate"]),
            &[Code::DslUnknownKeyName],
        ),
        theatre(
            "action-wait-no-duration",
            theatre_of(&["Action: wait"]),
            &[Code::DslInvalidDuration],
        ),
        theatre(
            "action-wait-zero-duration",
            theatre_of(&["Action: wait 0s"]),
            &[Code::DslInvalidDuration],
        ),
        theatre(
            "action-wait-too-long",
            theatre_of(&["Action: wait 11m"]),
            &[Code::DslInvalidDuration],
        ),
        theatre(
            "action-timeout-bad-unit",
            theatre_of(&["Action: click => P timeout 5x"]),
            &[Code::DslInvalidDuration],
        ),
        theatre(
            "action-timeout-forbidden-back",
            theatre_of(&["Action: back timeout 5s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        theatre(
            "action-timeout-forbidden-key",
            theatre_of(&["Action: key ok timeout 5s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        theatre(
            "action-timeout-forbidden-wait",
            theatre_of(&["Action: wait 1s timeout 2s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        theatre(
            "action-timeout-forbidden-screenshot",
            theatre_of(&["Action: screenshot 's' timeout 1s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        theatre(
            "action-timeout-forbidden-app",
            theatre_of(&["Action: app launch timeout 1s"]),
            &[Code::DslTimeoutNotAllowed],
        ),
        theatre(
            "action-trailing-input",
            theatre_of(&["Action: click => P junk"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "action-app-unknown-op",
            theatre_of(&["Action: app stop"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "check-no-piece",
            theatre_of(&["Check:"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "check-unknown-condition",
            theatre_of(&["Check: P is frobnicated"]),
            &[Code::DslUnknownKeyword],
        ),
        theatre(
            "check-trailing-input",
            theatre_of(&["Check: P junk"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "check-text-unterminated",
            theatre_of(&["Check: P text 'x"]),
            &[Code::DslUnterminatedString],
        ),
        theatre(
            "check-count-not-integer",
            theatre_of(&["Check: P count x"]),
            &[Code::DslUnknownAction],
        ),
        theatre(
            "check-timeout-bad-duration",
            theatre_of(&["Check: P timeout 0ms"]),
            &[Code::DslInvalidDuration],
        ),
        theatre(
            "theatre-unknown-top-level-keyword",
            theatre_body("Bogus: x\n"),
            &[Code::DslUnknownKeyword],
        ),
    ]
}

#[test]
fn theatre_negative_cases() {
    for case in theatre_cases() {
        let outcome = parse_theatre(&case.source, Path::new("case.theatre"));
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
            scene: "Scene: S1\n\n| Piece | web |\n|---|---|\n| A | #a |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: Missing\n        Check: A\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: ANDROID,
            expect: &[Code::DslUnknownScene],
        },
        SemanticCase {
            name: "unknown-piece",
            scene: "Scene: S1\n\n| Piece | web |\n|---|---|\n| A | #a |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Check: Nope\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: ANDROID,
            expect: &[Code::DslUnknownPiece],
        },
        SemanticCase {
            name: "missing-selector-for-platform",
            scene: "Scene: S1\n\n| Piece | web |\n|---|---|\n| Title | h1 |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Check: Title\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: ANDROID,
            expect: &[Code::DslMissingSelectorForPlatform],
        },
        SemanticCase {
            name: "unsupported-action-on-platform",
            scene: "Scene: S1\n\n| Piece | web | android | ios |\n|---|---|---|---|\n| List | #l | id:l | accessibilityId:l |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Action: swipe up => List\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: &[("living_room_tv", Platform::WebOs)],
            expect: &[Code::DslUnsupportedActionOnPlatform],
        },
        SemanticCase {
            name: "unsupported-key-on-platform",
            scene: "Scene: S1\n\n| Piece | web | ios |\n|---|---|---|\n| A | #a | accessibilityId:a |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Action: key rewind\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: &[("test_iphone", Platform::Ios)],
            expect: &[Code::DslUnsupportedActionOnPlatform],
        },
        SemanticCase {
            name: "unsupported-check-on-platform",
            scene: "Scene: S1\n\n| Piece | web | ios |\n|---|---|---|\n| A | #a | accessibilityId:a |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Check: A is focused\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: &[("test_iphone", Platform::Ios)],
            expect: &[Code::DslUnsupportedActionOnPlatform],
        },
        SemanticCase {
            name: "undefined-variable",
            scene: "Scene: S1\n\n| Piece | web | android |\n|---|---|---|\n| Field | #f | id:f |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Action: '${var.MISSING}' => Field\n",
            play: "Play: P\nVar: USER = 'u'\n\nAct: A\n    Perform: S\n",
            devices: ANDROID,
            expect: &[Code::DslUndefinedVariable],
        },
        SemanticCase {
            name: "scene-not-applicable",
            scene: "Scene: TvOnly\nPlatforms: webos, tizen\n\n| Piece | web | android |\n|---|---|---|\n| Title | h1 | id:t |\n",
            scene_path: "TvOnly.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: TvOnly\n        Check: Title\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: &[
                ("living_room_tv", Platform::WebOs),
                ("flagship_android", Platform::Android),
            ],
            expect: &[Code::DslSceneNotApplicable],
        },
        SemanticCase {
            name: "lint-fixed-wait",
            scene: "Scene: S1\n\n| Piece | web |\n|---|---|\n| A | #a |\n",
            scene_path: "S1.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: S1\n        Action: wait 2s\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: &[("site", Platform::Web)],
            expect: &[Code::DslLintFixedWait],
        },
        SemanticCase {
            name: "lint-naming",
            scene: "Scene: login_screen\n\n| Piece | web |\n|---|---|\n| ok_button | #b |\n",
            scene_path: "login_screen.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: login_screen\n        Check: ok_button\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
            devices: &[("site", Platform::Web)],
            expect: &[Code::DslLintNaming],
        },
        SemanticCase {
            name: "lint-scene-file-mismatch",
            scene: "Scene: LoginScreen\n\n| Piece | web |\n|---|---|\n| A | #a |\n",
            scene_path: "other.scene",
            theatre: "Theatre: T\n\nStage: S\n    Scene: LoginScreen\n        Check: A\n",
            play: "Play: P\n\nAct: A\n    Perform: S\n",
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
        let theatre: TheatreFile = parse_theatre(case.theatre, Path::new("case.theatre"))
            .ast
            .unwrap_or_else(|| panic!("case '{}': theatre must parse", case.name));
        let play = parse_play(case.play, Path::new("case.play"))
            .ast
            .unwrap_or_else(|| panic!("case '{}': play must parse", case.name));
        let workspace = Workspace {
            scenes: vec![scene],
            theatres: vec![theatre],
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
        "Scene: Login\n\n| Piece | web |\n|---|---|\n| A | #a |\n",
    )
    .expect("write one");
    std::fs::write(
        &two,
        "Scene: Login\n\n| Piece | web |\n|---|---|\n| B | #b |\n",
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

/// Codes covered by dedicated tests rather than this table: `IMP-DSL-006`
/// (`duplicate_scene_across_workspace_is_imp_dsl_006`) and the theatrical/
/// workspace-lint family, whose fixtures need a `.theatre` or a full
/// workspace.
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
    let theatre = theatre_cases();
    let semantic = semantic_cases();
    let total = scene.len() + play.len() + theatre.len() + semantic.len() + DEDICATED.len();
    assert!(total >= 60, "negative matrix has only {total} cases");

    let mut expected: Vec<Code> = Vec::new();
    for case in scene.iter().chain(play.iter()).chain(theatre.iter()) {
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
        "\u{feff}Scene: A\r\n\r\n| Piece | web |\r\n|---|---|\r\n| P | #p |\r\n",
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
        &format!("Play: {ok}\n\nAct: A\n    Perform: S\n"),
        Path::new("case.play"),
    );
    assert!(
        !outcome.diagnostics.has_errors(),
        "{:?}",
        outcome.diagnostics
    );
    let outcome = parse_play(
        &format!("Play: P\n\nAct: {long}\n    Perform: S\n"),
        Path::new("case.play"),
    );
    assert!(outcome
        .diagnostics
        .iter()
        .any(|d| d.code == Code::DslMissingHeader));
}

#[test]
fn valid_entry_and_check_verbs_have_no_diagnostics() {
    let outcome = parse_theatre(
        &theatre_of(&[
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
        Path::new("case.theatre"),
    );
    assert!(
        !outcome.diagnostics.has_errors(),
        "{:?}",
        outcome.diagnostics
    );
}
