//! Parser and scanner unit tests: grammar coverage, recovery and spans.
//!
//! The large negative-case matrix lives in `tests/negative_cases.rs`; this
//! module covers the happy paths and the fine grammar details (type/append/
//! clear, defaults, spans, recovery).

use std::path::Path;

use super::{parse_play, parse_scene, ParseOutcome};
use crate::ast::{
    ActionVerb, AppOp, Check, CheckCondition, Direction, PlayFile, SceneFile, Step, StepKind,
};
use crate::diag::{Code, Diagnostics};
use crate::keys::RemoteKey;
use crate::platforms::{Platform, ScenePlatforms};

fn codes(diagnostics: &Diagnostics) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn scene_outcome(source: &str) -> ParseOutcome<SceneFile> {
    parse_scene(source, Path::new("test.scene"))
}

fn play_outcome(source: &str) -> ParseOutcome<PlayFile> {
    parse_play(source, Path::new("test.play"))
}

fn scene_ok(source: &str) -> SceneFile {
    let outcome = scene_outcome(source);
    assert!(
        !outcome.diagnostics.has_errors(),
        "unexpected diagnostics: {:?}",
        outcome.diagnostics
    );
    outcome.ast.expect("scene AST")
}

fn play_ok(source: &str) -> PlayFile {
    let outcome = play_outcome(source);
    assert!(
        !outcome.diagnostics.has_errors(),
        "unexpected diagnostics: {:?}",
        outcome.diagnostics
    );
    outcome.ast.expect("play AST")
}

fn action(step: &Step) -> &ActionVerb {
    match step.kind() {
        StepKind::Action(action) => &action.verb,
        StepKind::Check(_) => panic!("step is a check, not an action"),
    }
}

fn check(step: &Step) -> &Check {
    match step.kind() {
        StepKind::Check(check) => check,
        StepKind::Action(_) => panic!("step is an action, not a check"),
    }
}

fn steps(play: &PlayFile) -> Vec<&Step> {
    play.acts
        .iter()
        .flat_map(|act| act.stages.iter())
        .flat_map(|stage| stage.scenes.iter())
        .flat_map(|block| block.steps.iter())
        .collect()
}

/// Wrap `Action:`/`Check:` lines into a minimal well-formed play.
fn play_with_steps(step_lines: &[&str]) -> String {
    let mut out = String::from("Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n");
    for line in step_lines {
        out.push_str("        ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

const LOGIN_SCENE: &str = "\
Scene: LoginScreen
Platform: cross-platform

Pieces:
    LoginTitle:    web=\"h1.title\",  android=\"id:login_title\"
    UsernameInput: web=\"#username\", index=2
";

const LOGIN_PLAY: &str = "\
Play: User Authentication Feature
Description: Validating secure user entry across platforms.
Var: USER = 'test_user_impresario'

Act: Successful Login Flow
Tags: smoke, auth
Platforms: android, ios, webos

Stage: Navigate and Login
    Timeout: 15s
    Scene: LoginScreen
        Check: LoginTitle
        Action: '${var.USER}' => UsernameInput
        Action: click => SubmitButton

Stage: Verify Outcome
    Scene: DashboardScreen
        Check: WelcomeBanner is visible timeout 20s
";

// ---------------------------------------------------------------------------
// Scene files
// ---------------------------------------------------------------------------

#[test]
fn scene_header_and_pieces_parse_clean() {
    let scene = scene_ok(LOGIN_SCENE);
    assert_eq!(scene.name, "LoginScreen");
    assert_eq!(scene.platforms, ScenePlatforms::CrossPlatform);
    assert_eq!(scene.pieces.len(), 2);
    let title = scene.piece("LoginTitle").expect("LoginTitle");
    assert_eq!(title.index, 0);
    assert_eq!(
        title
            .selectors
            .iter()
            .map(|selector| (selector.key, selector.raw.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (Platform::Web, "h1.title"),
            (Platform::Android, "id:login_title"),
        ]
    );
    let username = scene.piece("UsernameInput").expect("UsernameInput");
    assert_eq!(username.index, 2);
}

#[test]
fn scene_spans_are_precise() {
    let scene = scene_ok(LOGIN_SCENE);
    assert_eq!(scene.name_span.line, 1);
    assert_eq!(scene.name_span.col, 8);
    assert_eq!(scene.name_span.len, 11);
    assert_eq!(scene.platforms_span.line, 2);
    let title = scene.piece("LoginTitle").expect("LoginTitle");
    assert_eq!(title.name_span.line, 5);
    assert_eq!(title.name_span.col, 5);
    assert_eq!(title.name_span.len, 10);
    assert_eq!(title.span.line, 5);
    let selector = title.selectors.first().expect("selector");
    assert_eq!(selector.span.line, 5);
    // The span covers the quoted literal, quotes included.
    assert_eq!(selector.span.len, "\"h1.title\"".chars().count());
}

#[test]
fn scene_platform_lists_parse_and_deduplicate() {
    let scene =
        scene_ok("Scene: A\nPlatform: android, ios, android\n\nPieces:\n    P: web=\"#p\"\n");
    assert_eq!(
        scene.platforms,
        ScenePlatforms::List(vec![Platform::Android, Platform::Ios])
    );
    let outcome = scene_outcome("Scene: A\nPlatform: webostv\n\nPieces:\n    P: web=\"#p\"\n");
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-004"]);
    let message = outcome
        .diagnostics
        .iter()
        .next()
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(message.contains("webos"), "message: {message}");
}

#[test]
fn scene_errors_are_recovered_so_later_lines_still_parse() {
    let outcome = scene_outcome(
        "Scene: Good\nPlatform: cross-platform\n\nPieces:\n    First: web=\"#first\"\n    Second: windows=\"#second\"\n    Third: web=\"#third\"\n",
    );
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-011"]);
    let scene = outcome.ast.expect("scene still produced");
    assert_eq!(scene.pieces.len(), 2);
    assert!(scene.piece("Third").is_some());
    assert!(scene.piece("Second").is_none());
}

#[test]
fn scanner_errors_are_collected_with_parse_errors() {
    let outcome = scene_outcome("Scene: A\n\tPieces:\nPlatform: web\n");
    let codes = codes(&outcome.diagnostics);
    assert!(codes.contains(&"IMP-DSL-001"), "{codes:?}");
    assert!(codes.contains(&"IMP-DSL-005"), "{codes:?}");
}

#[test]
fn empty_scene_reports_missing_header() {
    let outcome = scene_outcome("");
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-005"]);
    assert!(outcome.ast.is_none());
}

// ---------------------------------------------------------------------------
// Play files
// ---------------------------------------------------------------------------

#[test]
fn play_example_structure_parses_clean() {
    let play = play_ok(LOGIN_PLAY);
    assert_eq!(play.name, "User Authentication Feature");
    assert_eq!(
        play.description.as_deref(),
        Some("Validating secure user entry across platforms.")
    );
    assert_eq!(play.var_decls.len(), 1);
    assert_eq!(play.var_decls[0].name, "USER");
    assert_eq!(play.var_decls[0].value.raw, "test_user_impresario");

    let bound = play_ok(
        "Play: Bound\nDescription: One app.\nApp: shop\n\nAct: A\nStage: S\n    Scene: S\n        Action: click => B\n",
    );
    let named = bound.app.expect("app header");
    assert_eq!(named.name, "shop");

    let act = play.acts.first().expect("act");
    assert_eq!(act.name, "Successful Login Flow");
    assert_eq!(
        act.tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        vec!["smoke", "auth"]
    );
    assert_eq!(
        act.platforms
            .iter()
            .map(|platform| platform.name.as_str())
            .collect::<Vec<_>>(),
        vec!["android", "ios", "webos"]
    );

    let stage = act.stages.first().expect("stage");
    assert_eq!(stage.name, "Navigate and Login");
    assert_eq!(stage.timeout.map(|timeout| timeout.millis()), Some(15_000));
    let block = stage.scenes.first().expect("scene block");
    assert_eq!(block.scene_name, "LoginScreen");
    assert_eq!(block.steps.len(), 3);
    // `Check: LoginTitle` means `is visible`.
    assert!(matches!(
        check(&block.steps[0]).condition,
        CheckCondition::IsVisible
    ));
    // The shorthand `'text' => P` is `type`.
    match action(&block.steps[1]) {
        ActionVerb::Type { text, piece } => {
            assert_eq!(text.raw, "${var.USER}");
            assert_eq!(text.var_names(), vec!["USER"]);
            assert_eq!(piece.name, "UsernameInput");
        }
        other => panic!("expected Type, got {other:?}"),
    }
    assert!(matches!(action(&block.steps[2]), ActionVerb::Click { .. }));

    let verify = play.acts[0].stages[1].scenes[0].steps[0].clone();
    assert!(matches!(
        check(&verify).condition,
        CheckCondition::IsVisible
    ));
    assert_eq!(verify.timeout.map(|timeout| timeout.millis()), Some(20_000));
}

#[test]
fn duplicate_var_declarations_last_wins() {
    let play = play_ok(
        "Play: P\nVar: USER = 'first'\nVar: USER = 'second'\n\nAct: A\n\nStage: S\n    Scene: X\n        Check: A\n",
    );
    assert_eq!(play.var_decls.len(), 1);
    assert_eq!(play.var_decls[0].value.raw, "second");
}

#[test]
fn duplicate_act_and_stage_names_are_reported() {
    let source = "\
Play: P

Act: A

Stage: S
    Scene: Screen
        Check: A

Stage: S
    Scene: Screen
        Check: A

Act: A

Stage: T
    Scene: Screen
        Check: A
";
    let outcome = play_outcome(source);
    assert_eq!(
        codes(&outcome.diagnostics),
        vec!["IMP-DSL-016", "IMP-DSL-016"]
    );
}

#[test]
fn play_recovery_keeps_good_steps() {
    let outcome = play_outcome(&play_with_steps(&[
        "Check: A",
        "Action: tap => B",
        "Check: C",
    ]));
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-010"]);
    let play = outcome.ast.expect("play AST");
    assert_eq!(steps(&play).len(), 2);
}

#[test]
fn empty_play_reports_missing_header() {
    let outcome = play_outcome("");
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-005"]);
    assert!(outcome.ast.is_none());
}

#[test]
fn unknown_keywords_carry_hints() {
    // A near miss of a depth-0 keyword gets a "did you mean" hint.
    let outcome = play_outcome(
        "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Check: A\n\nDescrption: late\n",
    );
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-004"]);
    let message = outcome
        .diagnostics
        .iter()
        .next()
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(message.contains("Description"), "message: {message}");
}

#[test]
fn misplaced_keywords_use_imp_dsl_005() {
    // `Act:` inside a Stage is valid syntax elsewhere but invalid here.
    let outcome = play_outcome(
        "Play: P\n\nAct: A\n\nStage: S\n    Act: B\n    Scene: Screen\n        Check: A\n",
    );
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-005"]);
    // `Scene:` inside a scene block.
    let outcome = play_outcome(&play_with_steps(&["Check: A", "Scene: Other"]));
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-005"]);
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

#[test]
fn type_append_and_clear_are_distinct() {
    let play = play_ok(&play_with_steps(&[
        "Action: 'abc' => Field",
        "Action: type 'abc' => Field",
        "Action: append 'abc' => Field",
        "Action: clear => Field",
    ]));
    let all = steps(&play);
    assert_eq!(all.len(), 4);
    // `'abc' => P` is exactly `type 'abc' => P` (D-06).
    for step in [all[0], all[1]] {
        match action(step) {
            ActionVerb::Type { text, piece } => {
                assert_eq!(text.raw, "abc");
                assert_eq!(piece.name, "Field");
            }
            other => panic!("expected Type, got {other:?}"),
        }
    }
    match action(all[2]) {
        ActionVerb::Append { text, piece } => {
            assert_eq!(text.raw, "abc");
            assert_eq!(piece.name, "Field");
        }
        other => panic!("expected Append, got {other:?}"),
    }
    match action(all[3]) {
        ActionVerb::Clear { piece } => assert_eq!(piece.name, "Field"),
        other => panic!("expected Clear, got {other:?}"),
    }
}

#[test]
fn step_timeouts_are_captured() {
    let play = play_ok(&play_with_steps(&[
        "Action: click => Button timeout 2s",
        "Check: Title timeout 500ms",
    ]));
    let all = steps(&play);
    assert_eq!(all[0].timeout.map(|timeout| timeout.millis()), Some(2000));
    assert_eq!(all[1].timeout.map(|timeout| timeout.millis()), Some(500));
}

#[test]
fn timeout_is_rejected_on_master_and_key_actions() {
    for line in [
        "Action: back timeout 5s",
        "Action: wait 1s timeout 2s",
        "Action: key ok timeout 1s",
        "Action: screenshot 's' timeout 1s",
        "Action: app launch timeout 1s",
    ] {
        let outcome = play_outcome(&play_with_steps(&[line]));
        assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-022"], "{line}");
    }
    // Allowed elsewhere.
    let play = play_ok(&play_with_steps(&[
        "Action: click => P timeout 5s",
        "Action: type 'x' => P timeout 5s",
        "Action: longpress 2s => P timeout 5s",
        "Action: scroll down => P timeout 5s",
        "Action: swipe up => P timeout 5s",
        "Action: focus => P timeout 5s",
        "Action: select 'x' => P timeout 5s",
        "Action: clear => P timeout 5s",
    ]));
    assert_eq!(steps(&play).len(), 8);
}

#[test]
fn longpress_defaults_to_one_second() {
    let play = play_ok(&play_with_steps(&[
        "Action: longpress => Card",
        "Action: longpress 250ms => Card",
    ]));
    let all = steps(&play);
    match action(all[0]) {
        ActionVerb::LongPress { duration, piece } => {
            assert_eq!(duration.millis(), 1000);
            assert_eq!(piece.name, "Card");
        }
        other => panic!("expected LongPress, got {other:?}"),
    }
    match action(all[1]) {
        ActionVerb::LongPress { duration, .. } => assert_eq!(duration.millis(), 250),
        other => panic!("expected LongPress, got {other:?}"),
    }
}

#[test]
fn scroll_and_swipe_variants() {
    let play = play_ok(&play_with_steps(&[
        "Action: scroll down",
        "Action: scroll up 3 => List",
        "Action: scrollto => Footer",
        "Action: swipe left",
        "Action: swipe right => Carousel",
    ]));
    let all = steps(&play);
    match action(all[0]) {
        ActionVerb::Scroll {
            direction,
            count,
            piece,
        } => {
            assert_eq!(*direction, Direction::Down);
            assert_eq!(*count, 1);
            assert!(piece.is_none());
        }
        other => panic!("expected Scroll, got {other:?}"),
    }
    match action(all[1]) {
        ActionVerb::Scroll {
            direction,
            count,
            piece,
        } => {
            assert_eq!(*direction, Direction::Up);
            assert_eq!(*count, 3);
            assert_eq!(piece.as_ref().map(|p| p.name.as_str()), Some("List"));
        }
        other => panic!("expected Scroll, got {other:?}"),
    }
    match action(all[2]) {
        ActionVerb::ScrollTo { piece } => assert_eq!(piece.name, "Footer"),
        other => panic!("expected ScrollTo, got {other:?}"),
    }
    match action(all[3]) {
        ActionVerb::Swipe { direction, piece } => {
            assert_eq!(*direction, Direction::Left);
            assert!(piece.is_none());
        }
        other => panic!("expected Swipe, got {other:?}"),
    }
    match action(all[4]) {
        ActionVerb::Swipe { direction, piece } => {
            assert_eq!(*direction, Direction::Right);
            assert_eq!(piece.as_ref().map(|p| p.name.as_str()), Some("Carousel"));
        }
        other => panic!("expected Swipe, got {other:?}"),
    }
}

#[test]
fn key_wait_screenshot_and_app_verbs() {
    let play = play_ok(&play_with_steps(&[
        "Action: key ok",
        "Action: key rewind 3",
        "Action: back",
        "Action: wait 2s",
        "Action: screenshot 'after login'",
        "Action: app launch",
        "Action: app terminate",
        "Action: app restart",
        "Action: app background",
    ]));
    let all = steps(&play);
    match action(all[0]) {
        ActionVerb::Key { key, repeat } => {
            assert_eq!(key.key, RemoteKey::Ok);
            assert_eq!(*repeat, 1);
        }
        other => panic!("expected Key, got {other:?}"),
    }
    match action(all[1]) {
        ActionVerb::Key { key, repeat } => {
            assert_eq!(key.key, RemoteKey::Rewind);
            assert_eq!(*repeat, 3);
        }
        other => panic!("expected Key, got {other:?}"),
    }
    assert!(matches!(action(all[2]), ActionVerb::Back));
    match action(all[3]) {
        ActionVerb::Wait { duration } => assert_eq!(duration.millis(), 2000),
        other => panic!("expected Wait, got {other:?}"),
    }
    match action(all[4]) {
        ActionVerb::Screenshot { name } => assert_eq!(name.raw, "after login"),
        other => panic!("expected Screenshot, got {other:?}"),
    }
    match action(all[5]) {
        ActionVerb::App { op } => assert_eq!(*op, AppOp::Launch),
        other => panic!("expected App, got {other:?}"),
    }
    match action(all[6]) {
        ActionVerb::App { op } => assert_eq!(*op, AppOp::Terminate),
        other => panic!("expected App, got {other:?}"),
    }
    match action(all[7]) {
        ActionVerb::App { op } => assert_eq!(*op, AppOp::Restart),
        other => panic!("expected App, got {other:?}"),
    }
    match action(all[8]) {
        ActionVerb::App { op } => assert_eq!(*op, AppOp::Background),
        other => panic!("expected App, got {other:?}"),
    }
}

#[test]
fn unknown_directions_and_keys_report_precisely() {
    let outcome = play_outcome(&play_with_steps(&["Action: scroll sideways"]));
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-010"]);
    let message = outcome
        .diagnostics
        .iter()
        .next()
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(message.contains("sideways"), "message: {message}");

    let outcome = play_outcome(&play_with_steps(&["Action: swipe"]));
    let message = outcome
        .diagnostics
        .iter()
        .next()
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(message.contains("needs a direction"), "message: {message}");

    let outcome = play_outcome(&play_with_steps(&["Action: key rewindd"]));
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-018"]);
}

#[test]
fn invalid_strings_report_dsl_003() {
    for line in [
        "Action: type 'unterminated => Field",
        "Action: type 'bad \\q escape' => Field",
        "Action: type 'bad \\u{zz}' => Field",
        "Action: screenshot 'x",
        "Check: Field text 'x",
    ] {
        let outcome = play_outcome(&play_with_steps(&[line]));
        assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-003"], "{line}");
    }
}

#[test]
fn duration_errors_report_dsl_015() {
    for line in [
        "Action: wait",
        "Action: wait 0s",
        "Action: wait 11m",
        "Action: click => P timeout 5x",
        "Check: P timeout 0ms",
    ] {
        let outcome = play_outcome(&play_with_steps(&[line]));
        assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-015"], "{line}");
    }
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

#[test]
fn check_conditions_all_parse() {
    let play = play_ok(&play_with_steps(&[
        "Check: A",
        "Check: B is visible",
        "Check: C is hidden",
        "Check: D is enabled",
        "Check: E is disabled",
        "Check: F is focused",
        "Check: G text 'hello world'",
        "Check: H textContains 'ell'",
        "Check: I count 3",
    ]));
    let all = steps(&play);
    assert_eq!(check(all[0]).piece.name, "A");
    assert!(matches!(check(all[0]).condition, CheckCondition::IsVisible));
    assert!(matches!(check(all[1]).condition, CheckCondition::IsVisible));
    assert!(matches!(check(all[2]).condition, CheckCondition::IsHidden));
    assert!(matches!(check(all[3]).condition, CheckCondition::IsEnabled));
    assert!(matches!(
        check(all[4]).condition,
        CheckCondition::IsDisabled
    ));
    assert!(matches!(check(all[5]).condition, CheckCondition::IsFocused));
    match &check(all[6]).condition {
        CheckCondition::TextEquals { text } => assert_eq!(text.raw, "hello world"),
        other => panic!("expected TextEquals, got {other:?}"),
    }
    match &check(all[7]).condition {
        CheckCondition::TextContains { text } => assert_eq!(text.raw, "ell"),
        other => panic!("expected TextContains, got {other:?}"),
    }
    match &check(all[8]).condition {
        CheckCondition::Count { count } => assert_eq!(*count, 3),
        other => panic!("expected Count, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics quality
// ---------------------------------------------------------------------------

#[test]
fn diagnostics_point_at_the_offending_span() {
    // Unknown action verb `tap` on line 7: the span covers the verb itself.
    let source = "Play: P\n\nAct: A\n\nStage: S\n    Scene: Screen\n        Action: tap => P\n";
    let outcome = play_outcome(source);
    let diagnostic = outcome
        .diagnostics
        .iter()
        .find(|d| d.code == Code::DslUnknownAction)
        .expect("IMP-DSL-010");
    let span = diagnostic.span.clone().expect("span");
    assert_eq!(span.line, 7);
    assert_eq!(span.col, 17);
    assert_eq!(span.len, 3);
    assert!(
        diagnostic.message.contains("type"),
        "message: {}",
        diagnostic.message
    );

    // Trailing input on a Check line: `junk` at the end.
    let outcome = play_outcome(&play_with_steps(&["Check: P junk"]));
    let diagnostic = outcome.diagnostics.iter().next().expect("diagnostic");
    let span = diagnostic.span.clone().expect("span");
    assert_eq!(span.line, 7);
    assert_eq!(span.col, 18);
    assert_eq!(span.len, 4);
}

#[test]
fn duplicate_piece_names_are_reported() {
    let outcome = scene_outcome(
        "Scene: A\nPlatform: web\n\nPieces:\n    Title: web=\"#a\"\n    Title: web=\"#b\"\n",
    );
    assert_eq!(codes(&outcome.diagnostics), vec!["IMP-DSL-007"]);
    let help = outcome
        .diagnostics
        .iter()
        .next()
        .and_then(|d| d.help.clone())
        .unwrap_or_default();
    assert!(help.contains("line 5"), "help: {help}");
}
