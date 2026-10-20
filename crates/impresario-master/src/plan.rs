//! The static planner (spec 8.2): compile the selected Plays into an
//! [`ExecutionPlan`] per device, before any device is touched.
//!
//! Compilation order per act: filter (platform, tags, names) → scene
//! applicability → piece resolution through the D-07 fallback chain →
//! selector grammar → interpolation (CLI vars > play vars; env read now) →
//! support check against 6.1/6.2 → effective timeout (5.6). Any error aborts
//! the run; **all** errors are collected and returned together.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use impresario_core::apps::AppConfig;
use impresario_core::ast::{
    ActDecl, ActionVerb, AppOp, CheckCondition, PlayFile, SceneFile, Step, StepKind,
};
use impresario_core::command::{
    ActionType, CheckKind, Command, KeyPress, RunAction, ValidateCheck,
};
use impresario_core::config::{DeviceConfig, ImpresarioConfig};
use impresario_core::diag::{suggest, Code, Diagnostic, Diagnostics};
use impresario_core::platforms::Platform;
use impresario_core::selectors::{self, Selector};
use impresario_core::span::Span;
use impresario_core::strings::{ParsedString, Part};
use impresario_core::support;
use impresario_core::workspace::Workspace;

/// Act selection from the CLI (`--tag` / `--exclude-tag` / `--act`).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// `--tag` (repeatable = OR).
    pub tags: Vec<String>,
    /// `--exclude-tag` (wins over `--tag`).
    pub exclude_tags: Vec<String>,
    /// `--act` (exact name).
    pub acts: Vec<String>,
}

impl Selection {
    fn act_selected(&self, act: &ActDecl) -> bool {
        let tags: Vec<&str> = act.tags.iter().map(|t| t.name.as_str()).collect();
        if tags
            .iter()
            .any(|t| self.exclude_tags.iter().any(|e| e == t))
        {
            return false;
        }
        if !self.tags.is_empty() && !tags.iter().any(|t| self.tags.iter().any(|s| s == t)) {
            return false;
        }
        if !self.acts.is_empty() && !self.acts.iter().any(|name| name == &act.name) {
            return false;
        }
        true
    }
}

/// What kind of master work a planned step is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedWork {
    /// A command that crosses the wire.
    Wire,
    /// Master-side sleep (D-10); `command` is `None`.
    Wait { duration_ms: u64 },
    /// A `Screenshot` command; the master writes the PNG to
    /// `screenshots/<device>/<file_name>` (spec 5.6).
    Screenshot { file_name: String },
}

/// One compiled step (spec 8.2: `PlannedStep`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    pub span: Span,
    /// The source text as written (never the resolved string, 5.6).
    pub source_text: String,
    /// Reporting label, e.g. `click`, `check visible`, `wait`.
    pub label: String,
    /// Effective timeout in ms (5.6 precedence), for the wire deadline.
    pub timeout_ms: u32,
    /// The proto-free command; `None` for master-side steps.
    pub command: Option<Command>,
    pub work: PlannedWork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStage {
    pub name: String,
    pub steps: Vec<PlannedStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAct {
    pub name: String,
    pub tags: Vec<String>,
    pub stages: Vec<PlannedStage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPlay {
    pub name: String,
    pub file: String,
    pub acts: Vec<PlannedAct>,
}

/// One selected device with everything it will run.
#[derive(Debug, Clone)]
pub struct DevicePlan {
    pub device: DeviceConfig,
    /// The app bound to this run, or `None` for an app-less web run.
    pub app: Option<AppConfig>,
    pub plays: Vec<PlannedPlay>,
}

/// The complete plan for a `run` invocation.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub devices: Vec<DevicePlan>,
    /// Resolved `${env.*}` values, for report/log redaction (5.6).
    pub secrets: Vec<String>,
}

/// Everything the planner reads.
pub struct PlanInput<'a> {
    pub config: &'a ImpresarioConfig,
    pub workspace: &'a Workspace,
    /// Workspace root, used to resolve relative app package paths.
    pub base: &'a Path,
    pub devices: &'a [DeviceConfig],
    /// App definitions from the global/workspace layers.
    pub apps: &'a [AppConfig],
    /// `--app` override (wins over every play's `App:` header).
    pub cli_app: Option<&'a str>,
    /// The selected Plays, in run order.
    pub plays: &'a [PlayFile],
    pub selection: &'a Selection,
    /// `--var K=V` overrides (beat play vars).
    pub cli_vars: &'a BTreeMap<String, String>,
    /// `--timeout <ms>` (beats `timeout_default_ms`).
    pub cli_timeout: Option<u64>,
}

/// Compile the plan. On any error returns every diagnostic collected.
pub fn build_plan(input: &PlanInput<'_>) -> Result<ExecutionPlan, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    let mut secrets: Vec<String> = Vec::new();
    let mut devices = Vec::new();
    let mut reported_not_applicable: HashSet<(String, Platform)> = HashSet::new();
    let mut reported_selectors: HashSet<(String, Platform)> = HashSet::new();

    // Bind play -> app before any device work (TASK-2 section 5, D-21).
    let app = resolve_app(input, &mut diagnostics);

    // Play variable values, resolved once per play: env is read now, `Var:`
    // values may interpolate, I-04 last-wins, CLI `--var` overrides win.
    let play_vars: Vec<BTreeMap<String, String>> = input
        .plays
        .iter()
        .map(|play| play_var_values(play, input.cli_vars, &mut secrets, &mut diagnostics))
        .collect();

    for device in input.devices {
        let platform = device.platform;
        if let Some(app) = &app {
            check_app_for_device(input.base, app, device, &mut diagnostics);
        }
        let mut plays = Vec::new();
        for (play, vars) in input.plays.iter().zip(&play_vars) {
            let mut acts = Vec::new();
            for act in &play.acts {
                if !act_applies_to(act, platform) || !input.selection.act_selected(act) {
                    continue;
                }
                let mut stages = Vec::new();
                for stage in &act.stages {
                    let mut steps = Vec::new();
                    let mut ctx = CompileContext {
                        input,
                        device,
                        platform,
                        stage_timeout: stage.timeout,
                        vars,
                        declared_vars: &play.var_decls,
                        reported_not_applicable: &mut reported_not_applicable,
                        reported_selectors: &mut reported_selectors,
                        secrets: &mut secrets,
                        diagnostics: &mut diagnostics,
                    };
                    for block in &stage.scenes {
                        compile_scene_block(
                            block.scene_name.as_str(),
                            &block.steps,
                            &block.scene_span,
                            &mut ctx,
                            &mut steps,
                        );
                    }
                    if !steps.is_empty() {
                        stages.push(PlannedStage {
                            name: stage.name.clone(),
                            steps,
                        });
                    }
                }
                if !stages.is_empty() {
                    acts.push(PlannedAct {
                        name: act.name.clone(),
                        tags: act.tags.iter().map(|t| t.name.clone()).collect(),
                        stages,
                    });
                }
            }
            if !acts.is_empty() {
                plays.push(PlannedPlay {
                    name: play.name.clone(),
                    file: play.path.display().to_string(),
                    acts,
                });
            }
        }
        devices.push(DevicePlan {
            device: device.clone(),
            app: app.clone(),
            plays,
        });
    }

    if diagnostics.has_errors() {
        return Err(diagnostics);
    }
    Ok(ExecutionPlan { devices, secrets })
}

/// Resolve the single app a run binds to (TASK-2 section 5): `--app` wins,
/// otherwise every selected play must agree on its optional `App:` header.
fn resolve_app(input: &PlanInput<'_>, diagnostics: &mut Diagnostics) -> Option<AppConfig> {
    let mut requested: Option<(String, Option<Span>)> =
        input.cli_app.map(|name| (name.to_string(), None));
    if requested.is_none() {
        for play in input.plays {
            let Some(named) = &play.app else {
                continue;
            };
            match &requested {
                None => requested = Some((named.name.clone(), Some(named.span.clone()))),
                Some((name, _)) if name != &named.name => {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::AppConflict,
                            format!(
                                "plays bind different apps ('{name}' and '{}'); split the run or \
                                 pass --app to choose one",
                                named.name
                            ),
                        )
                        .with_span(named.span.clone())
                        .with_help("one device connection can run one app at a time"),
                    );
                }
                Some(_) => {}
            }
        }
    }
    let (name, span) = requested?;
    if let Some(app) = input.apps.iter().find(|app| app.name == name) {
        return Some(app.clone());
    }
    let mut diag = Diagnostic::error(
        Code::AppUnknown,
        format!("unknown app '{name}'; it is not defined in the workspace or global apps layer"),
    )
    .with_help(if input.apps.is_empty() {
        "add an apps file or run `impresario app add`".to_string()
    } else {
        let names: Vec<&str> = input.apps.iter().map(|app| app.name.as_str()).collect();
        format!("available apps: {}", names.join(", "))
    });
    if let Some(span) = span {
        diag = diag.with_span(span);
    }
    diagnostics.push(diag);
    None
}

fn check_app_for_device(
    base: &Path,
    app: &AppConfig,
    device: &DeviceConfig,
    diagnostics: &mut Diagnostics,
) {
    let Some(entry) = app.platform(device.platform) else {
        diagnostics.push(
            Diagnostic::error(
                Code::AppPlatformMissing,
                format!(
                    "app '{}' has no '{}' entry needed by device '{}'",
                    app.name, device.platform, device.name
                ),
            )
            .with_help(format!(
                "add platforms.{} to the app definition or run a device of a supported platform",
                device.platform
            )),
        );
        return;
    };
    if device.platform != Platform::Web && entry.id.is_none() {
        diagnostics.push(Diagnostic::error(
            Code::AppPlatformMissing,
            format!(
                "app '{}' has no id for platform '{}'",
                app.name, device.platform
            ),
        ));
    }
    if let Some(package) = &entry.package {
        let path = PathBuf::from(package);
        let resolved = if path.is_absolute() {
            path
        } else {
            base.join(path)
        };
        if !resolved.exists() {
            diagnostics.push(
                Diagnostic::error(
                    Code::AppPackageMissing,
                    format!(
                        "app '{}' package for '{}' does not exist: {}",
                        app.name,
                        device.platform,
                        resolved.display()
                    ),
                )
                .with_help(format!(
                    "fix platforms.{}.package in the app definition or build the artifact",
                    device.platform
                )),
            );
        }
    }
}

fn act_applies_to(act: &ActDecl, platform: Platform) -> bool {
    if act.platforms.is_empty() {
        return true;
    }
    act.platforms
        .iter()
        .filter_map(|named| named.name.parse::<Platform>().ok())
        .any(|p| p == platform)
}

/// Resolve every `Var:` value of a play: values may interpolate other vars
/// and `${env.*}`; cycles are `IMP-DSL-014`; CLI `--var` wins; duplicate
/// declarations are last-wins (I-04).
fn play_var_values(
    play: &PlayFile,
    cli_vars: &BTreeMap<String, String>,
    secrets: &mut Vec<String>,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<String, String> {
    let mut decls: BTreeMap<&str, &ParsedString> = BTreeMap::new();
    for decl in &play.var_decls {
        decls.insert(decl.name.as_str(), &decl.value);
    }
    let mut resolved = BTreeMap::new();
    let mut reported_cycles: HashSet<String> = HashSet::new();
    for name in decls.keys() {
        resolve_var_decl(
            name,
            &decls,
            cli_vars,
            &mut resolved,
            &mut HashSet::new(),
            &mut reported_cycles,
            secrets,
            diagnostics,
        );
    }
    for (key, value) in cli_vars {
        resolved.insert(key.clone(), value.clone());
    }
    resolved
}

#[allow(clippy::too_many_arguments)]
fn resolve_var_decl(
    name: &str,
    decls: &BTreeMap<&str, &ParsedString>,
    cli_vars: &BTreeMap<String, String>,
    resolved: &mut BTreeMap<String, String>,
    visiting: &mut HashSet<String>,
    reported_cycles: &mut HashSet<String>,
    secrets: &mut Vec<String>,
    diagnostics: &mut Diagnostics,
) -> Option<String> {
    if let Some(value) = resolved.get(name) {
        return Some(value.clone());
    }
    if let Some(value) = cli_vars.get(name) {
        return Some(value.clone());
    }
    let decl = decls.get(name).copied()?;
    if !visiting.insert(name.to_string()) {
        if reported_cycles.insert(name.to_string()) {
            diagnostics.push(
                Diagnostic::error(
                    Code::DslUndefinedVariable,
                    format!("variable '{name}' is defined in terms of itself"),
                )
                .with_span(decl.span.clone())
                .with_help("break the cycle between the Var: declarations"),
            );
        }
        return None;
    }

    let mut out = String::new();
    let mut failed = false;
    for part in &decl.parts {
        match part {
            Part::Literal(text) => out.push_str(text),
            Part::Var { name: other, span } => {
                if let Some(value) = resolve_var_decl(
                    other,
                    decls,
                    cli_vars,
                    resolved,
                    visiting,
                    reported_cycles,
                    secrets,
                    diagnostics,
                ) {
                    out.push_str(&value);
                } else if decls.contains_key(other.as_str()) {
                    // The failing declaration already reported its error.
                    failed = true;
                    break;
                } else {
                    diagnostics.push(undefined_var(other, span));
                    failed = true;
                    break;
                }
            }
            Part::Env {
                name: env_name,
                span,
            } => match std::env::var(env_name) {
                Ok(value) => {
                    if !value.is_empty() {
                        secrets.push(value.clone());
                    }
                    out.push_str(&value);
                }
                Err(_) => {
                    diagnostics.push(undefined_env(env_name, span));
                    failed = true;
                    break;
                }
            },
        }
    }
    visiting.remove(name);
    if failed {
        return None;
    }
    resolved.insert(name.to_string(), out.clone());
    Some(out)
}

fn undefined_var(name: &str, span: &Span) -> Diagnostic {
    Diagnostic::error(
        Code::DslUndefinedVariable,
        format!("undefined variable '{name}'"),
    )
    .with_span(span.clone())
    .with_help("declare it with: Var: NAME = 'value' (or pass --var NAME=value)")
}

fn undefined_env(name: &str, span: &Span) -> Diagnostic {
    Diagnostic::error(
        Code::DslUndefinedVariable,
        format!("environment variable '{name}' is not set"),
    )
    .with_span(span.clone())
    .with_help(format!(
        "set {name} in the environment before running (I-17)"
    ))
}

struct CompileContext<'a, 'b> {
    input: &'a PlanInput<'b>,
    device: &'a DeviceConfig,
    platform: Platform,
    stage_timeout: Option<impresario_core::duration::DslDuration>,
    vars: &'a BTreeMap<String, String>,
    declared_vars: &'a [impresario_core::ast::VarDecl],
    reported_not_applicable: &'a mut HashSet<(String, Platform)>,
    reported_selectors: &'a mut HashSet<(String, Platform)>,
    secrets: &'a mut Vec<String>,
    diagnostics: &'a mut Diagnostics,
}

fn compile_scene_block(
    scene_name: &str,
    steps: &[Step],
    scene_span: &Span,
    ctx: &mut CompileContext<'_, '_>,
    out: &mut Vec<PlannedStep>,
) {
    let scene = ctx.input.workspace.scene(scene_name);
    if scene.is_none() {
        let mut diag = Diagnostic::error(
            Code::DslUnknownScene,
            format!("unknown scene '{scene_name}'"),
        )
        .with_span(scene_span.clone());
        if let Some(hint) = suggest(scene_name, &ctx.input.workspace.scene_names()) {
            diag = diag.with_help(hint);
        }
        ctx.diagnostics.push(diag);
    }

    if let Some(scene) = scene {
        if !scene.platforms.applies_to(ctx.platform)
            && ctx
                .reported_not_applicable
                .insert((scene.name.clone(), ctx.platform))
        {
            ctx.diagnostics.push(
                Diagnostic::error(
                    Code::DslSceneNotApplicable,
                    format!(
                        "scene '{}' does not apply to platform '{}' (device '{}')",
                        scene.name, ctx.platform, ctx.device.name
                    ),
                )
                .with_span(scene_span.clone())
                .with_help(format!(
                    "the scene applies to: {}",
                    describe_scene_platforms(scene)
                )),
            );
        }
    }

    for step in steps {
        match compile_step(step, ctx, scene) {
            Ok(planned) => out.push(planned),
            Err(diags) => ctx.diagnostics.extend(diags.into_vec()),
        }
    }
}

fn describe_scene_platforms(scene: &SceneFile) -> String {
    match &scene.platforms {
        impresario_core::platforms::ScenePlatforms::CrossPlatform => "all platforms".to_string(),
        impresario_core::platforms::ScenePlatforms::List(list) => {
            let names: Vec<&str> = list.iter().map(|p| p.as_str()).collect();
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        }
    }
}

fn compile_step(
    step: &Step,
    ctx: &mut CompileContext<'_, '_>,
    scene: Option<&SceneFile>,
) -> Result<PlannedStep, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    let timeout_ms = effective_timeout(step, ctx);
    let result = match &step.kind {
        StepKind::Action(action) => match &action.verb {
            ActionVerb::Type { text, piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                let text = resolve_string(text, ctx, &mut diagnostics);
                match (selector, text) {
                    (Some(Some(selector)), Some(text)) => {
                        check_action_support(ActionType::Type, &action.span, ctx, &mut diagnostics);
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "type".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::type_text(selector, text))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Append { text, piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                let text = resolve_string(text, ctx, &mut diagnostics);
                match (selector, text) {
                    (Some(Some(selector)), Some(text)) => {
                        check_action_support(
                            ActionType::Append,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "append".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::append(selector, text))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Clear { piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                match selector {
                    Some(Some(selector)) => {
                        check_action_support(
                            ActionType::Clear,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "clear".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::clear(selector))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Click { piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                match selector {
                    Some(Some(selector)) => {
                        check_action_support(
                            ActionType::Click,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "click".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::click(selector))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::LongPress { duration, piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                match selector {
                    Some(Some(selector)) => {
                        check_action_support(
                            ActionType::LongPress,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        let duration_ms = u32::try_from(duration.millis()).unwrap_or(u32::MAX);
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "longpress".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::long_press(
                                selector,
                                duration_ms,
                            ))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Focus { piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                match selector {
                    Some(Some(selector)) => {
                        check_action_support(
                            ActionType::Focus,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "focus".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::focus(selector))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Select { option, piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                let text = resolve_string(option, ctx, &mut diagnostics);
                match (selector, text) {
                    (Some(Some(selector)), Some(text)) => {
                        check_action_support(
                            ActionType::Select,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "select".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::select(selector, text))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Scroll {
                direction,
                count,
                piece,
            } => {
                let selector = match piece {
                    Some(piece) => resolve_piece(
                        piece.name.as_str(),
                        &piece.span,
                        scene,
                        ctx,
                        &mut diagnostics,
                    ),
                    None => Some(None),
                };
                match selector {
                    Some(selector) => {
                        check_action_support(
                            ActionType::Scroll,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "scroll".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::scroll(
                                *direction,
                                (*count).max(1),
                                selector,
                            ))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::ScrollTo { piece } => {
                let selector = resolve_piece(
                    piece.name.as_str(),
                    &piece.span,
                    scene,
                    ctx,
                    &mut diagnostics,
                );
                match selector {
                    Some(Some(selector)) => {
                        check_action_support(
                            ActionType::ScrollTo,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "scrollto".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::scroll_to(selector))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Swipe { direction, piece } => {
                let selector = match piece {
                    Some(piece) => resolve_piece(
                        piece.name.as_str(),
                        &piece.span,
                        scene,
                        ctx,
                        &mut diagnostics,
                    ),
                    None => Some(None),
                };
                match selector {
                    Some(selector) => {
                        check_action_support(
                            ActionType::Swipe,
                            &action.span,
                            ctx,
                            &mut diagnostics,
                        );
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "swipe".to_string(),
                            timeout_ms,
                            command: Some(Command::RunAction(RunAction::swipe(
                                *direction, selector,
                            ))),
                            work: PlannedWork::Wire,
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::Key { key, repeat } => {
                if !support::key_support(ctx.platform, key.key).is_supported() {
                    ctx.diagnostics.push(
                        Diagnostic::error(
                            Code::DslUnsupportedActionOnPlatform,
                            format!(
                                "key '{}' is not supported on platform '{}' (device '{}')",
                                key.key.as_str(),
                                ctx.platform,
                                ctx.device.name
                            ),
                        )
                        .with_span(action.span.clone())
                        .with_help("see docs/PARITY.md for the support matrix"),
                    );
                }
                Ok(PlannedStep {
                    span: step.span.clone(),
                    source_text: step.source_text.clone(),
                    label: format!("key {}", key.key.as_str()),
                    timeout_ms,
                    command: Some(Command::KeyPress(KeyPress::new(key.key, *repeat))),
                    work: PlannedWork::Wire,
                })
            }
            ActionVerb::Back => {
                check_action_support(ActionType::Back, &action.span, ctx, &mut diagnostics);
                Ok(PlannedStep {
                    span: step.span.clone(),
                    source_text: step.source_text.clone(),
                    label: "back".to_string(),
                    timeout_ms,
                    command: Some(Command::RunAction(RunAction::back())),
                    work: PlannedWork::Wire,
                })
            }
            ActionVerb::Wait { duration } => Ok(PlannedStep {
                span: step.span.clone(),
                source_text: step.source_text.clone(),
                label: "wait".to_string(),
                timeout_ms,
                command: None,
                work: PlannedWork::Wait {
                    duration_ms: duration.millis(),
                },
            }),
            ActionVerb::Screenshot { name } => {
                let resolved = resolve_string(name, ctx, &mut diagnostics);
                match resolved {
                    Some(resolved) => {
                        let file_name = sanitize_file_name(&resolved);
                        Ok(PlannedStep {
                            span: step.span.clone(),
                            source_text: step.source_text.clone(),
                            label: "screenshot".to_string(),
                            timeout_ms,
                            command: Some(Command::Screenshot {
                                label: file_name.clone(),
                            }),
                            work: PlannedWork::Screenshot { file_name },
                        })
                    }
                    _ => Err(()),
                }
            }
            ActionVerb::App { op } => {
                let verb = match op {
                    AppOp::Launch => support::Verb::AppLaunch,
                    AppOp::Terminate => support::Verb::AppTerminate,
                    AppOp::Restart => support::Verb::AppRestart,
                    AppOp::Background => support::Verb::AppBackground,
                };
                check_verb_support(verb, &action.span, ctx, &mut diagnostics);
                Ok(PlannedStep {
                    span: step.span.clone(),
                    source_text: step.source_text.clone(),
                    label: format!("app {}", op.as_str()),
                    timeout_ms,
                    command: Some(Command::AppControl {
                        op: *op,
                        clear_data: false,
                    }),
                    work: PlannedWork::Wire,
                })
            }
        },
        StepKind::Check(check) => {
            let selector = resolve_piece(
                check.piece.name.as_str(),
                &check.piece.span,
                scene,
                ctx,
                &mut diagnostics,
            );
            let condition = match &check.condition {
                CheckCondition::IsVisible => Some((CheckKind::Visible, String::new(), 0)),
                CheckCondition::IsHidden => Some((CheckKind::Hidden, String::new(), 0)),
                CheckCondition::IsEnabled => Some((CheckKind::Enabled, String::new(), 0)),
                CheckCondition::IsDisabled => Some((CheckKind::Disabled, String::new(), 0)),
                CheckCondition::IsFocused => Some((CheckKind::Focused, String::new(), 0)),
                CheckCondition::TextEquals { text } => resolve_string(text, ctx, &mut diagnostics)
                    .map(|text| (CheckKind::TextEquals, text, 0)),
                CheckCondition::TextContains { text } => {
                    resolve_string(text, ctx, &mut diagnostics)
                        .map(|text| (CheckKind::TextContains, text, 0))
                }
                CheckCondition::Count { count } => Some((CheckKind::Count, String::new(), *count)),
            };
            if !support::check_support(ctx.platform, support::verb_of_check(&check.condition))
                .is_supported()
            {
                ctx.diagnostics.push(
                    Diagnostic::error(
                        Code::DslUnsupportedActionOnPlatform,
                        format!(
                            "check '{}' is not supported on platform '{}' (device '{}')",
                            check.condition.as_str(),
                            ctx.platform,
                            ctx.device.name
                        ),
                    )
                    .with_span(check.span.clone())
                    .with_help("see docs/PARITY.md for the support matrix"),
                );
            }
            match (selector, condition) {
                (Some(Some(selector)), Some((kind, expected_text, expected_count))) => {
                    Ok(PlannedStep {
                        span: step.span.clone(),
                        source_text: step.source_text.clone(),
                        label: format!("check {}", check.condition.as_str()),
                        timeout_ms,
                        command: Some(Command::ValidateCheck(ValidateCheck {
                            selector,
                            kind,
                            expected_text,
                            expected_count,
                        })),
                        work: PlannedWork::Wire,
                    })
                }
                _ => Err(()),
            }
        }
    };

    match result {
        Ok(step) => {
            // A step can compile cleanly yet still carry errors (e.g. an
            // unsupported action); the run must not start in that case.
            if diagnostics.has_errors() {
                Err(diagnostics)
            } else {
                Ok(step)
            }
        }
        Err(()) => Err(diagnostics),
    }
}

fn effective_timeout(step: &Step, ctx: &CompileContext<'_, '_>) -> u32 {
    let millis = step
        .timeout
        .or(ctx.stage_timeout)
        .map(impresario_core::duration::DslDuration::millis)
        .or(ctx.input.cli_timeout)
        .unwrap_or(ctx.input.config.timeout_default_ms);
    u32::try_from(millis.min(u64::from(u32::MAX))).unwrap_or(u32::MAX)
}

fn check_action_support(
    action: ActionType,
    span: &Span,
    ctx: &CompileContext<'_, '_>,
    diagnostics: &mut Diagnostics,
) {
    let verb = match action {
        ActionType::Click => support::Verb::Click,
        ActionType::Type => support::Verb::Type,
        ActionType::Append => support::Verb::Append,
        ActionType::Clear => support::Verb::Clear,
        ActionType::LongPress => support::Verb::LongPress,
        ActionType::Focus => support::Verb::Focus,
        ActionType::Select => support::Verb::Select,
        ActionType::Scroll => support::Verb::Scroll,
        ActionType::ScrollTo => support::Verb::ScrollTo,
        ActionType::Swipe => support::Verb::Swipe,
        ActionType::Back => support::Verb::Back,
    };
    check_verb_support(verb, span, ctx, diagnostics);
}

fn check_verb_support(
    verb: support::Verb,
    span: &Span,
    ctx: &CompileContext<'_, '_>,
    diagnostics: &mut Diagnostics,
) {
    if support::action_support(ctx.platform, verb).is_supported() {
        return;
    }
    diagnostics.push(
        Diagnostic::error(
            Code::DslUnsupportedActionOnPlatform,
            format!(
                "action '{}' is not supported on platform '{}' (device '{}')",
                verb.as_str(),
                ctx.platform,
                ctx.device.name
            ),
        )
        .with_span(span.clone())
        .with_help("see docs/PARITY.md for the support matrix"),
    );
}

/// Resolve a piece reference through the D-07 fallback chain. `Ok(None)`
/// means "no piece given" (valid for screen-level scroll/swipe).
#[allow(clippy::option_option)]
fn resolve_piece(
    piece_name: &str,
    piece_span: &Span,
    scene: Option<&SceneFile>,
    ctx: &mut CompileContext<'_, '_>,
    diagnostics: &mut Diagnostics,
) -> Option<Option<Selector>> {
    let Some(scene) = scene else {
        // The unknown scene was already reported; do not cascade.
        return Some(None);
    };
    let Some(piece) = scene.piece(piece_name) else {
        let mut diag = Diagnostic::error(
            Code::DslUnknownPiece,
            format!("unknown piece '{piece_name}' in scene '{}'", scene.name),
        )
        .with_span(piece_span.clone());
        let candidates: Vec<&str> = scene.pieces.iter().map(|p| p.name.as_str()).collect();
        if let Some(hint) = suggest(piece_name, &candidates) {
            diag = diag.with_help(hint);
        }
        diagnostics.push(diag);
        return None;
    };

    let mut matched = None;
    for key in ctx.platform.selector_chain() {
        if let Some(selector) = piece.selectors.iter().find(|s| s.key == *key) {
            matched = Some((*key, selector));
            break;
        }
    }
    let Some((key, selector)) = matched else {
        if ctx
            .reported_selectors
            .insert((piece.name.clone(), ctx.platform))
        {
            let written: Vec<&str> = piece.selectors.iter().map(|s| s.key.as_str()).collect();
            let chain: Vec<&str> = ctx
                .platform
                .selector_chain()
                .iter()
                .map(|p| p.as_str())
                .collect();
            diagnostics.push(
                Diagnostic::error(
                    Code::DslMissingSelectorForPlatform,
                    format!(
                        "piece '{}' has no selector for platform '{}' (device '{}')",
                        piece.name, ctx.platform, ctx.device.name
                    ),
                )
                .with_span(piece_span.clone())
                .with_help(format!(
                    "selectors written: {} (fallback chain: {})",
                    if written.is_empty() {
                        "none".to_string()
                    } else {
                        written.join(", ")
                    },
                    chain.join(", ")
                )),
            );
        }
        return None;
    };

    match selectors::parse(key, &selector.raw, piece.index, Some(selector.span.clone())) {
        Ok(selector) => Some(Some(selector)),
        Err(diag) => {
            diagnostics.push(diag);
            None
        }
    }
}

/// Resolve a string's interpolations (CLI vars > play vars; env read now).
/// Resolved env values are recorded as secrets for redaction.
fn resolve_string(
    string: &ParsedString,
    ctx: &mut CompileContext<'_, '_>,
    diagnostics: &mut Diagnostics,
) -> Option<String> {
    let mut out = String::new();
    for part in &string.parts {
        match part {
            Part::Literal(text) => out.push_str(text),
            Part::Var { name, span } => match ctx.vars.get(name) {
                Some(value) => out.push_str(value),
                None if ctx.declared_vars.iter().any(|decl| decl.name == *name) => {
                    // The declaration failed to resolve; that error stands.
                    return None;
                }
                None => {
                    diagnostics.push(undefined_var(name, span));
                    return None;
                }
            },
            Part::Env { name, span } => match std::env::var(name) {
                Ok(value) => {
                    if !value.is_empty() {
                        ctx.secrets.push(value.clone());
                    }
                    out.push_str(&value);
                }
                Err(_) => {
                    diagnostics.push(undefined_env(name, span));
                    return None;
                }
            },
        }
    }
    Some(out)
}

/// Sanitize a `screenshot 'name'` file name to `[A-Za-z0-9._-]`, at most 80
/// characters (spec 5.6); the `.png` suffix is appended by the executor.
#[must_use]
pub fn sanitize_file_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for ch in name.chars() {
        let keep = ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_';
        if keep {
            out.push(ch);
            last_underscore = ch == '_';
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '_' || c == '.');
    let capped: String = trimmed.chars().take(80).collect();
    if capped.is_empty() {
        "screenshot".to_string()
    } else {
        capped
    }
}

/// Parse `--var K=V` pairs into a map (CLI beats play vars).
///
/// Errors use `IMP-CFG-003` (invalid value) with a `--var` hint.
pub fn parse_cli_vars(pairs: &[String]) -> Result<BTreeMap<String, String>, Diagnostics> {
    let mut vars = BTreeMap::new();
    let mut diagnostics = Diagnostics::new();
    for pair in pairs {
        match pair.split_once('=') {
            Some((name, value)) if is_variable_name(name) => {
                vars.insert(name.to_string(), value.to_string());
            }
            Some((name, _)) if !is_variable_name(name) => {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgInvalidValue,
                        format!("invalid --var name '{name}'"),
                    )
                    .with_help(
                        "variable names start with an ASCII letter and continue with letters, digits or '_'",
                    ),
                );
            }
            None => {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgInvalidValue,
                        format!("invalid --var '{pair}': expected K=V"),
                    )
                    .with_help("pass variables as --var NAME=value"),
                );
            }
            Some(_) => {}
        }
    }
    if diagnostics.has_errors() {
        Err(diagnostics)
    } else {
        Ok(vars)
    }
}

fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `--timeout` bounds (spec 4.1: `timeout_default_ms` is `[100, 600000]`).
#[must_use]
pub fn timeout_in_bounds(ms: u64) -> bool {
    (100..=600_000).contains(&ms)
}

/// `--retries` bounds (spec 4.1: `retries` is `0..=5`).
#[must_use]
pub fn retries_in_bounds(retries: u32) -> bool {
    retries <= 5
}

/// The command used to (re)start the app before an Act (spec 7.3 step 4).
#[must_use]
pub fn app_start_command(first_of_run: bool, clear_data: bool) -> Command {
    Command::AppControl {
        op: if first_of_run {
            AppOp::Launch
        } else {
            AppOp::Restart
        },
        clear_data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::config;
    use impresario_core::parser::{parse_play, parse_scene};
    use std::path::Path;

    fn workspace(scene_source: &str, play_source: &str) -> Workspace {
        let scene = parse_scene(scene_source, Path::new("S.scene"))
            .ast
            .expect("scene parses");
        let play = parse_play(play_source, Path::new("p.play"))
            .ast
            .expect("play parses");
        Workspace {
            scenes: vec![scene],
            plays: vec![play],
        }
    }

    fn config() -> ImpresarioConfig {
        let outcome = config::parse_config(
            "project: t\ntimeout_default_ms: 10000\nscreenshot_on_failure: false\nreport_output: ./reports\n",
            Path::new("impresario.config"),
        );
        outcome.config.expect("config parses")
    }

    fn device(platform: Platform) -> DeviceConfig {
        let mut connection = config::DeviceConnection::default();
        match platform {
            Platform::Android | Platform::AndroidTv => {
                connection.serial = Some("emulator-5554".to_string());
            }
            Platform::Ios | Platform::Tvos => {
                connection.udid = Some("00008110-001A2345021E001E".to_string());
                if platform == Platform::Tvos {
                    connection.endpoint = Some("127.0.0.1:50062".to_string());
                }
            }
            Platform::Web | Platform::WebOs | Platform::Tizen => {
                connection.endpoint = Some("ws://127.0.0.1:9999".to_string());
                if platform != Platform::Web {
                    connection.host = Some("127.0.0.1".to_string());
                }
            }
        }
        DeviceConfig {
            name: "dev".to_string(),
            platform,
            connection,
            auth: config::DeviceAuth::default(),
            labels: Vec::new(),
            options: config::DeviceOptions::default(),
        }
    }

    fn compile(
        scene_source: &str,
        play_source: &str,
        platform: Platform,
        vars: &BTreeMap<String, String>,
    ) -> Result<ExecutionPlan, Diagnostics> {
        compile_apps(scene_source, play_source, platform, vars, &[], None)
    }

    fn compile_apps(
        scene_source: &str,
        play_source: &str,
        platform: Platform,
        vars: &BTreeMap<String, String>,
        apps: &[AppConfig],
        cli_app: Option<&str>,
    ) -> Result<ExecutionPlan, Diagnostics> {
        let workspace = workspace(scene_source, play_source);
        let config = config();
        let devices = vec![device(platform)];
        let plays: Vec<&PlayFile> = workspace.plays.iter().collect();
        let plays: Vec<PlayFile> = plays.into_iter().cloned().collect();
        build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            apps,
            cli_app,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: vars,
            cli_timeout: None,
        })
    }

    fn app_defs(source: &str) -> Vec<AppConfig> {
        impresario_core::apps::parse_apps(source, Path::new("impresario.apps.config"))
            .config
            .expect("apps parse")
            .apps
    }

    fn codes(diagnostics: &Diagnostics) -> Vec<&'static str> {
        diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    const SCENE: &str = r##"Scene: S
Platform: cross-platform

Pieces:
    Field: web="#f", android="id:f", ios="accessibilityId:f"
    Button: web="#b", android="id:b", ios="accessibilityId:b"
"##;

    #[test]
    fn compiles_a_clean_act() {
        let play = r#"Play: P
Var: USER = 'u1'

Act: A
Tags: smoke

Stage: S1
    Timeout: 5s
    Scene: S
        Check: Field
        Action: '${var.USER}' => Field
        Action: append 'x' => Field
        Action: clear => Field
        Action: click => Button timeout 2s
"#;
        let plan = compile(SCENE, play, Platform::Android, &BTreeMap::new()).expect("plans");
        let device = plan.devices.first().expect("device");
        let act = device
            .plays
            .first()
            .expect("play")
            .acts
            .first()
            .expect("act");
        let steps: Vec<&PlannedStep> = act
            .stages
            .iter()
            .flat_map(|stage| stage.steps.iter())
            .collect();
        assert_eq!(steps.len(), 5);
        // Check: default timeout from config (5s stage beats it).
        assert_eq!(steps[0].timeout_ms, 5_000);
        assert!(matches!(steps[0].command, Some(Command::ValidateCheck(_))));
        // type with resolved var.
        match &steps[1].command {
            Some(Command::RunAction(action)) => {
                assert_eq!(action.action, ActionType::Type);
                assert_eq!(action.text, "u1");
            }
            other => panic!("unexpected {other:?}"),
        }
        match &steps[2].command {
            Some(Command::RunAction(action)) => assert_eq!(action.action, ActionType::Append),
            other => panic!("unexpected {other:?}"),
        }
        match &steps[3].command {
            Some(Command::RunAction(action)) => assert_eq!(action.action, ActionType::Clear),
            other => panic!("unexpected {other:?}"),
        }
        // Step timeout beats stage timeout.
        assert_eq!(steps[4].timeout_ms, 2_000);
    }

    #[test]
    fn timeout_precedence_cli_beats_default_stage_beats_cli_beat_by_step() {
        let play = r#"Play: P

Act: A

Stage: S1
    Timeout: 5s
    Scene: S
        Check: Field
"#;
        let workspace = workspace(SCENE, play);
        let config = config();
        let devices = vec![device(Platform::Android)];
        let plays: Vec<PlayFile> = workspace.plays.to_vec();
        // CLI timeout only.
        let plan = build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_timeout: Some(7000),
        })
        .expect("plans");
        let step = &plan.devices[0].plays[0].acts[0].stages[0].steps[0];
        assert_eq!(step.timeout_ms, 5_000, "stage timeout beats --timeout");
    }

    #[test]
    fn cli_timeout_applies_when_no_stage_timeout() {
        let play = "Play: P\n\nAct: A\n\nStage: S1\n    Scene: S\n        Check: Field\n";
        let workspace = workspace(SCENE, play);
        let config = config();
        let devices = vec![device(Platform::Android)];
        let plays: Vec<PlayFile> = workspace.plays.to_vec();
        let plan = build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_timeout: Some(7000),
        })
        .expect("plans");
        let step = &plan.devices[0].plays[0].acts[0].stages[0].steps[0];
        assert_eq!(step.timeout_ms, 7_000);
    }

    #[test]
    fn unsupported_action_is_an_error() {
        let play =
            "Play: P\n\nAct: A\n\nStage: S1\n    Scene: S\n        Action: swipe up => Field\n";
        let diagnostics =
            compile(SCENE, play, Platform::AndroidTv, &BTreeMap::new()).expect_err("fails");
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-013"]);
    }

    #[test]
    fn unsupported_key_is_an_error() {
        let play = "Play: P\n\nAct: A\n\nStage: S1\n    Scene: S\n        Action: key rewind\n";
        let diagnostics = compile(SCENE, play, Platform::Web, &BTreeMap::new()).expect_err("fails");
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-013"]);
    }

    #[test]
    fn missing_selector_for_platform_is_an_error() {
        let scene = "Scene: S\nPlatform: cross-platform\n\nPieces:\n    Field: web=\"#f\"\n";
        let play = "Play: P\n\nAct: A\n\nStage: S1\n    Scene: S\n        Check: Field\n";
        let diagnostics =
            compile(scene, play, Platform::Android, &BTreeMap::new()).expect_err("fails");
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-012"]);
    }

    #[test]
    fn undefined_var_and_missing_env_are_errors_together() {
        let play = r#"Play: P

Act: A

Stage: S1
    Scene: S
        Action: '${var.NOPE}' => Field
        Action: '${env.IMPRESARIO_TEST_UNSET_VAR_X}' => Field
"#;
        // Make sure the env var really is unset.
        std::env::remove_var("IMPRESARIO_TEST_UNSET_VAR_X");
        let diagnostics =
            compile(SCENE, play, Platform::Android, &BTreeMap::new()).expect_err("fails");
        assert_eq!(
            codes(&diagnostics),
            vec!["IMP-DSL-014", "IMP-DSL-014"],
            "all errors are collected"
        );
    }

    #[test]
    fn cli_vars_define_and_override() {
        let play = "Play: P\nVar: V = 'play'\n\nAct: A\n\nStage: S1\n    Scene: S\n        Action: '${var.V}' => Field\n";
        let mut vars = BTreeMap::new();
        vars.insert("V".to_string(), "cli".to_string());
        let plan = compile(SCENE, play, Platform::Android, &vars).expect("plans");
        match &plan.devices[0].plays[0].acts[0].stages[0].steps[0].command {
            Some(Command::RunAction(action)) => assert_eq!(action.text, "cli"),
            other => panic!("unexpected {other:?}"),
        }

        // A variable that only exists on the CLI is defined.
        let play = "Play: P\n\nAct: A\n\nStage: S1\n    Scene: S\n        Action: '${var.ONLY_CLI}' => Field\n";
        let mut vars = BTreeMap::new();
        vars.insert("ONLY_CLI".to_string(), "yes".to_string());
        let plan = compile(SCENE, play, Platform::Android, &vars).expect("plans");
        match &plan.devices[0].plays[0].acts[0].stages[0].steps[0].command {
            Some(Command::RunAction(action)) => assert_eq!(action.text, "yes"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unknown_scene_and_piece_do_not_cascade() {
        let play = "Play: P\n\nAct: A\n\nStage: S1\n    Scene: Missing\n        Check: Whatever\n";
        let diagnostics =
            compile(SCENE, play, Platform::Android, &BTreeMap::new()).expect_err("fails");
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-008"]);
    }

    #[test]
    fn scene_not_applicable_is_an_error() {
        let scene =
            "Scene: OnlyTv\nPlatform: webos\n\nPieces:\n    Field: webos=\"#f\", android=\"id:f\"\n";
        let play = "Play: P\n\nAct: A\n\nStage: S1\n    Scene: OnlyTv\n        Check: Field\n";
        let diagnostics =
            compile(scene, play, Platform::Android, &BTreeMap::new()).expect_err("fails");
        assert_eq!(codes(&diagnostics), vec!["IMP-DSL-017"]);
    }

    #[test]
    fn act_filters_apply() {
        let play = r#"Play: P

Act: Keep
Tags: smoke
Platforms: android

Stage: S1
    Scene: S
        Check: Field

Act: DropTag
Tags: slow

Stage: S2
    Scene: S
        Check: Field

Act: DropPlatform
Platforms: ios

Stage: S3
    Scene: S
        Check: Field
"#;
        let workspace = workspace(SCENE, play);
        let config = config();
        let devices = vec![device(Platform::Android)];
        let plays: Vec<PlayFile> = workspace.plays.to_vec();
        let selection = Selection {
            tags: vec!["smoke".to_string()],
            exclude_tags: Vec::new(),
            acts: Vec::new(),
        };
        let plan = build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &selection,
            cli_vars: &BTreeMap::new(),
            cli_timeout: None,
        })
        .expect("plans");
        let acts: Vec<&str> = plan.devices[0].plays[0]
            .acts
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(acts, vec!["Keep"]);

        // exclude-tag wins over tag.
        let selection = Selection {
            tags: vec!["smoke".to_string()],
            exclude_tags: vec!["smoke".to_string()],
            acts: Vec::new(),
        };
        let plan = build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &selection,
            cli_vars: &BTreeMap::new(),
            cli_timeout: None,
        })
        .expect("plans");
        assert!(plan.devices[0].plays.is_empty(), "all acts excluded");
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize_file_name("checkout page!"), "checkout_page");
        assert_eq!(sanitize_file_name("a b  c"), "a_b_c");
        assert_eq!(sanitize_file_name("__"), "screenshot");
        assert_eq!(sanitize_file_name(""), "screenshot");
        assert_eq!(sanitize_file_name(&"x".repeat(100)).len(), 80);
        assert_eq!(sanitize_file_name("v1.2-final"), "v1.2-final");
    }

    #[test]
    fn cli_var_parsing() {
        let vars = parse_cli_vars(&["A=1".to_string(), "B=".to_string()]).expect("ok");
        assert_eq!(vars.get("A").map(String::as_str), Some("1"));
        assert_eq!(vars.get("B").map(String::as_str), Some(""));
        let bad = parse_cli_vars(&["NOEQUAL".to_string()]).expect_err("bad");
        assert_eq!(codes(&bad), vec!["IMP-CFG-003"]);
        let bad = parse_cli_vars(&["1BAD=x".to_string()]).expect_err("bad");
        assert_eq!(codes(&bad), vec!["IMP-CFG-003"]);
    }

    #[test]
    fn bounds() {
        assert!(timeout_in_bounds(100));
        assert!(timeout_in_bounds(600_000));
        assert!(!timeout_in_bounds(99));
        assert!(!timeout_in_bounds(600_001));
        assert!(retries_in_bounds(0));
        assert!(retries_in_bounds(5));
        assert!(!retries_in_bounds(6));
    }

    const APP_PLAY: &str = r#"Act: A
Tags: smoke

Stage: S1
    Scene: S
        Action: click => Button
"#;

    const APPS_ANDROID: &str =
        "apps:\n- name: shop\n  platforms:\n    android:\n      id: com.example.shop\n";

    fn app_play(header: &str) -> String {
        format!("Play: P\nApp: {header}\n\n{APP_PLAY}")
    }

    #[test]
    fn app_header_binds_the_plan() {
        let play = format!("Play: P\nApp: shop\n\n{APP_PLAY}");
        let plan = compile_apps(
            SCENE,
            &play,
            Platform::Android,
            &BTreeMap::new(),
            &app_defs(APPS_ANDROID),
            None,
        )
        .expect("plans");
        assert_eq!(
            plan.devices[0].app.as_ref().map(|app| app.name.as_str()),
            Some("shop")
        );
    }

    #[test]
    fn no_app_header_leaves_the_plan_app_less() {
        let play = format!("Play: P\n\n{APP_PLAY}");
        let plan = compile(SCENE, &play, Platform::Android, &BTreeMap::new()).expect("plans");
        assert!(plan.devices[0].app.is_none());
    }

    #[test]
    fn unknown_app_header_is_reported() {
        let diagnostics = compile_apps(
            SCENE,
            &app_play("missing"),
            Platform::Android,
            &BTreeMap::new(),
            &app_defs(APPS_ANDROID),
            None,
        )
        .expect_err("unknown app");
        assert_eq!(codes(&diagnostics), vec!["IMP-APP-001"]);
        assert!(diagnostics
            .iter()
            .next()
            .and_then(|diag| diag.help.as_deref())
            .unwrap_or("")
            .contains("shop"));
    }

    #[test]
    fn app_missing_the_device_platform_is_reported() {
        let apps = app_defs(
            "apps:\n- name: shop\n  platforms:\n    web:\n      url: http://localhost:8080/\n",
        );
        let diagnostics = compile_apps(
            SCENE,
            &app_play("shop"),
            Platform::Android,
            &BTreeMap::new(),
            &apps,
            None,
        )
        .expect_err("no android entry");
        assert_eq!(codes(&diagnostics), vec!["IMP-APP-002"]);
    }

    #[test]
    fn app_package_must_exist() {
        let apps = app_defs(
            "apps:\n- name: shop\n  platforms:\n    android:\n      id: com.example.shop\n      package: ./definitely-missing.apk\n",
        );
        let diagnostics = compile_apps(
            SCENE,
            &app_play("shop"),
            Platform::Android,
            &BTreeMap::new(),
            &apps,
            None,
        )
        .expect_err("missing package");
        assert_eq!(codes(&diagnostics), vec!["IMP-APP-003"]);
    }

    #[test]
    fn cli_app_beats_headers_and_conflicting_headers_are_reported() {
        let apps = app_defs(
            "apps:\n- name: shop\n  platforms:\n    android:\n      id: com.example.shop\n- name: other\n  platforms:\n    android:\n      id: com.example.other\n",
        );
        let plan = compile_apps(
            SCENE,
            &app_play("missing"),
            Platform::Android,
            &BTreeMap::new(),
            &apps,
            Some("shop"),
        )
        .expect("cli app wins");
        assert_eq!(
            plan.devices[0].app.as_ref().map(|app| app.name.as_str()),
            Some("shop")
        );

        let first = parse_play(&app_play("shop"), Path::new("a.play"))
            .ast
            .expect("first play");
        let second = parse_play(&app_play("other"), Path::new("b.play"))
            .ast
            .expect("second play");
        let scene = parse_scene(SCENE, Path::new("S.scene")).ast.expect("scene");
        let workspace = Workspace {
            scenes: vec![scene],
            plays: vec![first, second],
        };
        let config = config();
        let devices = vec![device(Platform::Android)];
        let plays: Vec<PlayFile> = workspace.plays.to_vec();
        let diagnostics = build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            apps: &apps,
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_timeout: None,
        })
        .expect_err("conflict");
        assert_eq!(codes(&diagnostics), vec!["IMP-APP-004"]);
    }
}
