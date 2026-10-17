//! Semantic validation (spec sections 5.7 and 6.1): resolution of scenes,
//! pieces and variables; selector availability per platform; unsupported
//! actions/checks; applicability; lints.
//!
//! Validation is device-aware: `IMP-DSL-012`, `IMP-DSL-013` and
//! `IMP-DSL-017` need the set of configured devices, so `validate` receives
//! one [`DeviceRef`] per configured device. Results are deduplicated per
//! platform, which is the granularity that actually drives these rules.

use std::collections::HashSet;

use crate::ast::{Action, ActionVerb, CheckCondition, Ident, PlayFile, SceneFile, Step, StepKind};
use crate::diag::{suggest, Code, Diagnostic, Diagnostics};
use crate::platforms::{Platform, ScenePlatforms};
use crate::span::Span;
use crate::strings::{ParsedString, Part};
use crate::support::{self, CheckVerb, Verb};
use crate::workspace::Workspace;

/// One configured device, as far as validation is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRef {
    pub name: String,
    pub platform: Platform,
}

impl DeviceRef {
    #[must_use]
    pub fn new(name: impl Into<String>, platform: Platform) -> Self {
        Self {
            name: name.into(),
            platform,
        }
    }
}

/// Validate the whole workspace against the configured devices.
#[must_use]
pub fn validate(workspace: &Workspace, devices: &[DeviceRef]) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    // Distinct platforms, in the order devices were configured; the first
    // device of a platform names it in messages.
    let mut platforms: Vec<(Platform, &str)> = Vec::new();
    for device in devices {
        if !platforms.iter().any(|(p, _)| *p == device.platform) {
            platforms.push((device.platform, device.name.as_str()));
        }
    }

    for scene in &workspace.scenes {
        check_scene_identity(scene, &mut diagnostics);
    }
    for play in &workspace.plays {
        validate_play(play, workspace, &platforms, &mut diagnostics);
    }
    diagnostics
}

/// Lints that need only the scene file itself: `IMP-DSL-021` (file/name
/// mismatch) and `IMP-DSL-020` (naming).
fn check_scene_identity(scene: &SceneFile, diagnostics: &mut Diagnostics) {
    let stem = scene
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if stem != scene.name {
        diagnostics.push(
            Diagnostic::warning(
                Code::DslLintSceneFileMismatch,
                format!(
                    "scene '{}' does not match its file name '{}'",
                    scene.name,
                    scene.path.display()
                ),
            )
            .with_span(scene.name_span.clone())
            .with_help("name the file after the scene, e.g. LoginScreen.scene"),
        );
    }
    if !is_upper_camel(&scene.name) {
        diagnostics.push(naming_warning("scene", &scene.name, &scene.name_span));
    }
    for piece in &scene.pieces {
        if !is_upper_camel(&piece.name) {
            diagnostics.push(naming_warning("piece", &piece.name, &piece.name_span));
        }
    }
}

fn naming_warning(what: &str, name: &str, span: &Span) -> Diagnostic {
    let camel = to_upper_camel(name);
    let mut diag = Diagnostic::warning(
        Code::DslLintNaming,
        format!("{what} name '{name}' is not UpperCamelCase"),
    )
    .with_span(span.clone());
    if camel != name && !camel.is_empty() {
        diag = diag.with_help(format!("rename it to '{camel}'"));
    } else {
        diag = diag.with_help("UpperCamelCase is recommended for scene and piece names");
    }
    diag
}

/// `^[A-Z][A-Za-z0-9]*$` (lint recommendation, not a hard rule).
#[must_use]
pub fn is_upper_camel(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_uppercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// Convert `snake_case`, `kebab-case` or spaces to UpperCamelCase.
#[must_use]
pub fn to_upper_camel(name: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;
    for c in name.chars() {
        if c == '_' || c == '-' || c.is_whitespace() {
            capitalize = true;
        } else if capitalize {
            out.extend(c.to_uppercase());
            capitalize = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn validate_play(
    play: &PlayFile,
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    diagnostics: &mut Diagnostics,
) {
    let declared: Vec<&str> = play
        .var_decls
        .iter()
        .map(|decl| decl.name.as_str())
        .collect();
    for decl in &play.var_decls {
        check_variables(&decl.value, &declared, diagnostics);
    }

    let mut reported_selectors: HashSet<(String, Platform)> = HashSet::new();
    let mut reported_not_applicable: HashSet<(String, Platform)> = HashSet::new();

    for act in &play.acts {
        let act_platforms: Vec<Platform> = act
            .platforms
            .iter()
            .filter_map(|named| named.name.parse::<Platform>().ok())
            .collect();
        let applies =
            |platform: Platform| act_platforms.is_empty() || act_platforms.contains(&platform);
        for stage in &act.stages {
            for block in &stage.scenes {
                let scene = workspace.scene(&block.scene_name);
                if scene.is_none() {
                    let mut diag = Diagnostic::error(
                        Code::DslUnknownScene,
                        format!("unknown scene '{}'", block.scene_name),
                    )
                    .with_span(block.scene_span.clone());
                    if let Some(hint) = suggest(&block.scene_name, &workspace.scene_names()) {
                        diag = diag.with_help(hint);
                    }
                    diagnostics.push(diag);
                }

                // Scene applicability (IMP-DSL-017) per platform.
                if let Some(scene) = scene {
                    for (platform, device) in platforms {
                        if !applies(*platform) || scene.platforms.applies_to(*platform) {
                            continue;
                        }
                        if reported_not_applicable.insert((scene.name.clone(), *platform)) {
                            diagnostics.push(
                                Diagnostic::error(
                                    Code::DslSceneNotApplicable,
                                    format!(
                                        "scene '{}' does not apply to platform '{platform}' (device '{device}')",
                                        scene.name
                                    ),
                                )
                                .with_span(block.scene_span.clone())
                                .with_help(format!(
                                    "the scene applies to: {}",
                                    describe_platforms(&scene.platforms)
                                )),
                            );
                        }
                    }
                }

                for step in &block.steps {
                    for string in step_strings(step) {
                        check_variables(string, &declared, diagnostics);
                    }
                    if let Some(Action {
                        verb: ActionVerb::Wait { .. },
                        ..
                    }) = action_of(step)
                    {
                        diagnostics.push(
                            Diagnostic::warning(
                                Code::DslLintFixedWait,
                                "fixed waits are fragile; prefer a Check with a timeout",
                            )
                            .with_span(step.span.clone()),
                        );
                    }
                    check_piece_resolution(
                        scene,
                        step,
                        platforms,
                        &applies,
                        &mut reported_selectors,
                        diagnostics,
                    );
                    // Action/check support per platform (IMP-DSL-013).
                    for (platform, device) in platforms {
                        if !applies(*platform) {
                            continue;
                        }
                        if let Some(diag) = unsupported_step(step, *platform, device) {
                            diagnostics.push(diag);
                        }
                    }
                }
            }
        }
    }
}

/// `IMP-DSL-009` and `IMP-DSL-012` for one step.
fn check_piece_resolution(
    scene: Option<&SceneFile>,
    step: &Step,
    platforms: &[(Platform, &str)],
    applies: &dyn Fn(Platform) -> bool,
    reported_selectors: &mut HashSet<(String, Platform)>,
    diagnostics: &mut Diagnostics,
) {
    let (Some(scene), Some(piece_ref)) = (scene, step_piece(step)) else {
        return;
    };
    let Some(piece) = scene.piece(&piece_ref.name) else {
        let mut diag = Diagnostic::error(
            Code::DslUnknownPiece,
            format!(
                "unknown piece '{}' in scene '{}'",
                piece_ref.name, scene.name
            ),
        )
        .with_span(piece_ref.span.clone());
        let candidates: Vec<&str> = scene
            .pieces
            .iter()
            .map(|piece| piece.name.as_str())
            .collect();
        if let Some(hint) = suggest(&piece_ref.name, &candidates) {
            diag = diag.with_help(hint);
        }
        diagnostics.push(diag);
        return;
    };
    for (platform, device) in platforms {
        if !applies(*platform) {
            continue;
        }
        let available = piece
            .selectors
            .iter()
            .any(|selector| platform.selector_chain().contains(&selector.key));
        if available || !reported_selectors.insert((piece.name.clone(), *platform)) {
            continue;
        }
        let written: Vec<&str> = piece
            .selectors
            .iter()
            .map(|selector| selector.key.as_str())
            .collect();
        diagnostics.push(
            Diagnostic::error(
                Code::DslMissingSelectorForPlatform,
                format!(
                    "piece '{}' has no selector for platform '{platform}' (device '{device}')",
                    piece.name
                ),
            )
            .with_span(piece_ref.span.clone())
            .with_help(format!(
                "selectors written: {} (fallback chain: {})",
                if written.is_empty() {
                    "none".to_string()
                } else {
                    written.join(", ")
                },
                platform
                    .selector_chain()
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        );
    }
}

fn describe_platforms(platforms: &ScenePlatforms) -> String {
    match platforms {
        ScenePlatforms::CrossPlatform => "all platforms".to_string(),
        ScenePlatforms::List(list) => list
            .iter()
            .map(|platform| platform.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// `IMP-DSL-014` for every `${var.X}` whose `X` is not declared.
fn check_variables(string: &ParsedString, declared: &[&str], diagnostics: &mut Diagnostics) {
    for part in &string.parts {
        if let Part::Var { name, span } = part {
            if !declared.contains(&name.as_str()) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::DslUndefinedVariable,
                        format!("undefined variable '{name}'"),
                    )
                    .with_span(span.clone())
                    .with_help("declare it with: Var: NAME = 'value' (or pass --var NAME=value)"),
                );
            }
        }
    }
}

fn action_of(step: &Step) -> Option<&Action> {
    match &step.kind {
        StepKind::Action(action) => Some(action),
        StepKind::Check(_) => None,
    }
}

fn step_piece(step: &Step) -> Option<&Ident> {
    match &step.kind {
        StepKind::Action(action) => action.verb.piece(),
        StepKind::Check(check) => Some(&check.piece),
    }
}

/// All interpolatable strings of a step (for undefined-variable checks).
fn step_strings(step: &Step) -> Vec<&ParsedString> {
    match &step.kind {
        StepKind::Action(action) => match &action.verb {
            ActionVerb::Type { text, .. } => vec![text],
            ActionVerb::Append { text, .. } => vec![text],
            ActionVerb::Select { option, .. } => vec![option],
            ActionVerb::Screenshot { name } => vec![name],
            _ => Vec::new(),
        },
        StepKind::Check(check) => match &check.condition {
            CheckCondition::TextEquals { text } | CheckCondition::TextContains { text } => {
                vec![text]
            }
            _ => Vec::new(),
        },
    }
}

fn verb_of(verb: &ActionVerb) -> Verb {
    support::verb_of_action(verb)
}

fn check_verb_of(condition: &CheckCondition) -> CheckVerb {
    support::verb_of_check(condition)
}

/// The `IMP-DSL-013` diagnostic for a step on one platform, if any.
fn unsupported_step(step: &Step, platform: Platform, device: &str) -> Option<Diagnostic> {
    match &step.kind {
        StepKind::Action(action) => match &action.verb {
            ActionVerb::Key { key, .. } => {
                if support::key_support(platform, key.key).is_supported() {
                    return None;
                }
                Some(
                    Diagnostic::error(
                        Code::DslUnsupportedActionOnPlatform,
                        format!(
                            "key '{}' is not supported on platform '{platform}' (device '{device}')",
                            key.key.as_str()
                        ),
                    )
                    .with_span(action.span.clone())
                    .with_help("see docs/PARITY.md for the support matrix"),
                )
            }
            verb => {
                if support::action_support(platform, verb_of(verb)).is_supported() {
                    return None;
                }
                Some(
                    Diagnostic::error(
                        Code::DslUnsupportedActionOnPlatform,
                        format!(
                            "action '{}' is not supported on platform '{platform}' (device '{device}')",
                            verb.name()
                        ),
                    )
                    .with_span(action.span.clone())
                    .with_help("see docs/PARITY.md for the support matrix"),
                )
            }
        },
        StepKind::Check(check) => {
            if support::check_support(platform, check_verb_of(&check.condition)).is_supported() {
                return None;
            }
            Some(
                Diagnostic::error(
                    Code::DslUnsupportedActionOnPlatform,
                    format!(
                        "check '{}' is not supported on platform '{platform}' (device '{device}')",
                        check.condition.as_str()
                    ),
                )
                .with_span(check.span.clone())
                .with_help("see docs/PARITY.md for the support matrix"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::parser::{parse_play, parse_scene};

    fn scene(source: &str, name: &str) -> SceneFile {
        parse_scene(source, Path::new(name))
            .ast
            .expect("scene parses")
    }

    fn play(source: &str, name: &str) -> PlayFile {
        parse_play(source, Path::new(name))
            .ast
            .expect("play parses")
    }

    fn workspace(scenes: Vec<SceneFile>, plays: Vec<PlayFile>) -> Workspace {
        Workspace { scenes, plays }
    }

    fn codes(diagnostics: &Diagnostics) -> Vec<&'static str> {
        diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    fn android() -> Vec<DeviceRef> {
        vec![DeviceRef::new("flagship_android", Platform::Android)]
    }

    #[test]
    fn clean_workspace_has_no_errors() {
        let scenes = vec![scene(
            "Scene: LoginScreen\nPlatform: cross-platform\n\nPieces:\n    LoginTitle: web=\"h1.title\", android=\"id:login_title\", ios=\"accessibilityId:login_title\"\n",
            "LoginScreen.scene",
        )];
        let plays = vec![play(
            "Play: Auth\nDescription: Login.\nVar: USER = 'test_user'\n\nAct: Login\nTags: smoke\n\nStage: Enter\n    Timeout: 15s\n    Scene: LoginScreen\n        Check: LoginTitle\n        Action: '${var.USER}' => LoginTitle\n        Action: click => LoginTitle timeout 5s\n",
            "auth.play",
        )];
        let devices = vec![
            DeviceRef::new("flagship_android", Platform::Android),
            DeviceRef::new("test_iphone", Platform::Ios),
        ];
        let diagnostics = validate(&workspace(scenes, plays), &devices);
        assert!(
            !diagnostics.has_errors(),
            "{:?}",
            diagnostics.errors().collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_scene_reports_with_hint() {
        let scenes = vec![scene(
            "Scene: LoginScreen\nPlatform: cross-platform\n\nPieces:\n    Title: web=\"h1\"\n",
            "LoginScreen.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: LoginScreeen\n        Check: Title\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace(scenes, plays), &android());
        let errors: Vec<_> = diagnostics.errors().collect();
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-008"], "{errors:?}");
        assert!(errors[0]
            .help
            .as_deref()
            .unwrap_or("")
            .contains("LoginScreen"));
    }

    #[test]
    fn unknown_piece_reports_with_hint() {
        let scenes = vec![scene(
            "Scene: LoginScreen\nPlatform: cross-platform\n\nPieces:\n    LoginTitle: web=\"h1\"\n",
            "LoginScreen.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: LoginScreen\n        Check: LoginTitel\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace(scenes, plays), &android());
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-009"]);
        assert!(diagnostics
            .errors()
            .next()
            .and_then(|d| d.help.clone())
            .unwrap_or_default()
            .contains("LoginTitle"));
    }

    #[test]
    fn missing_selector_uses_the_fallback_chain() {
        let scenes = vec![scene(
            "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    Title: web=\"h1\"\n",
            "S1.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Check: Title\n",
            "p.play",
        )];
        // webos falls back to web: fine.
        let ok = validate(
            &workspace(scenes.clone(), plays.clone()),
            &[DeviceRef::new("living_room_tv", Platform::WebOs)],
        );
        assert!(!ok.has_errors(), "{:?}", ok.errors().collect::<Vec<_>>());
        // android does not: IMP-DSL-012.
        let bad = validate(&workspace(scenes, plays), &android());
        assert_eq!(codes(&bad), vec!["IMP-DSL-012"]);
    }

    #[test]
    fn unsupported_action_and_key_and_check() {
        let scenes = vec![scene(
            "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    List: web=\"#l\", android=\"id:l\", ios=\"accessibilityId:l\"\n",
            "S1.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: S1\n        Action: swipe up => List\n        Action: key rewind\n        Check: List is focused\n",
            "p.play",
        )];
        // On webos: swipe ✖; rewind ✅ (415); focused ✅.
        let webos = validate(
            &workspace(scenes.clone(), plays.clone()),
            &[DeviceRef::new("tv", Platform::WebOs)],
        );
        assert_eq!(codes(&webos), vec!["IMP-DSL-013"], "{webos:?}");
        assert!(webos
            .errors()
            .next()
            .is_some_and(|d| d.message.contains("swipe")));
        // On ios: swipe ✅; key rewind ✖; the focused check ✖.
        let ios = validate(
            &workspace(scenes, plays),
            &[DeviceRef::new("iphone", Platform::Ios)],
        );
        assert_eq!(codes(&ios), vec!["IMP-DSL-013", "IMP-DSL-013"], "{ios:?}");
        assert!(ios.errors().any(|d| d.message.contains("focused")));
        assert!(ios.errors().any(|d| d.message.contains("rewind")));
    }

    #[test]
    fn scene_not_applicable_is_reported_per_platform() {
        let scenes = vec![scene(
            "Scene: TvOnly\nPlatform: webos, tizen\n\nPieces:\n    Title: web=\"h1\", android=\"id:t\"\n",
            "TvOnly.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: TvOnly\n        Check: Title\n",
            "p.play",
        )];
        let devices = vec![
            DeviceRef::new("living_room_tv", Platform::WebOs),
            DeviceRef::new("flagship_android", Platform::Android),
        ];
        let diagnostics = validate(&workspace(scenes, plays), &devices);
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-017"], "{diagnostics:?}");
        assert!(diagnostics
            .errors()
            .next()
            .is_some_and(|d| d.message.contains("android")));
    }

    #[test]
    fn undefined_variable_is_reported_at_the_reference() {
        let scenes = vec![scene(
            "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    Field: web=\"#f\", android=\"id:f\"\n",
            "S1.scene",
        )];
        let plays = vec![play(
            "Play: P\nVar: USER = 'u'\n\nAct: A\n\nStage: S\n    Scene: S1\n        Action: '${var.PASSWORD}' => Field\n        Action: '${var.USER}' => Field\n        Action: '${env.HOME}' => Field\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace(scenes, plays), &android());
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-014"]);
        let error = diagnostics.errors().next().expect("one error");
        assert!(error.message.contains("PASSWORD"));
        let span = error.span.clone().expect("span");
        assert_eq!(span.line, 8);
    }

    #[test]
    fn lints_fire_as_warnings_only() {
        let scenes = vec![scene(
            "Scene: login_screen\nPlatform: cross-platform\n\nPieces:\n    submit_button: web=\"#s\"\n",
            "other_file.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n\nStage: S\n    Scene: login_screen\n        Action: wait 2s\n        Check: submit_button\n",
            "p.play",
        )];
        let diagnostics = validate(
            &workspace(scenes, plays),
            &[DeviceRef::new("site", Platform::Web)],
        );
        assert!(!diagnostics.has_errors());
        let warnings: Vec<&'static str> = diagnostics.warnings().map(|d| d.code.as_str()).collect();
        assert!(warnings.contains(&"IMP-DSL-019"), "{warnings:?}");
        assert!(warnings.contains(&"IMP-DSL-020"), "{warnings:?}");
        assert!(warnings.contains(&"IMP-DSL-021"), "{warnings:?}");
    }

    #[test]
    fn act_platforms_filter_checks() {
        let scenes = vec![scene(
            "Scene: S1\nPlatform: cross-platform\n\nPieces:\n    List: web=\"#l\", android=\"id:l\", ios=\"accessibilityId:l\"\n",
            "S1.scene",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: OnlyTv\nPlatforms: webos\n\nStage: S\n    Scene: S1\n        Action: swipe up => List\n",
            "p.play",
        )];
        // swipe is ✖ on webos which the act targets → error; on android the
        // act is skipped entirely → no error.
        let webos = validate(
            &workspace(scenes.clone(), plays.clone()),
            &[DeviceRef::new("tv", Platform::WebOs)],
        );
        assert_eq!(codes(&webos), vec!["IMP-DSL-013"]);
        let android_only = validate(
            &workspace(scenes, plays),
            &[DeviceRef::new("phone", Platform::Android)],
        );
        assert!(
            !android_only.has_errors(),
            "{:?}",
            android_only.errors().collect::<Vec<_>>()
        );
    }

    #[test]
    fn naming_helpers() {
        assert!(is_upper_camel("LoginScreen"));
        assert!(is_upper_camel("Welcome2"));
        assert!(!is_upper_camel("loginScreen"));
        assert!(!is_upper_camel("login_screen"));
        assert!(!is_upper_camel(""));
        assert_eq!(to_upper_camel("login_screen"), "LoginScreen");
        assert_eq!(to_upper_camel("submit-button"), "SubmitButton");
        assert_eq!(to_upper_camel("LoginScreen"), "LoginScreen");
    }
}
