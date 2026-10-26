//! The static planner (spec 8.2): compile the selected Plays into an
//! [`ExecutionPlan`] per device, before any device is touched.
//!
//! Compilation order per act: filter (platform, tags, names) â†’ scene
//! applicability â†’ piece resolution through the D-07 fallback chain â†’
//! selector grammar â†’ interpolation (CLI vars > play vars; env read now) â†’
//! support check against 6.1/6.2 â†’ effective timeout (5.6). Any error aborts
//! the run; **all** errors are collected and returned together.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use impresario_core::apps::AppConfig;
use impresario_core::ast::{
    ActDecl, ActionVerb, ActorFilterKind, AppOp, CheckCondition, PerformDecl, PlayFile,
    PlayerCheck, SceneFile, ShowingCheck, StageDecl, StageItem, Step, StepKind,
};
use impresario_core::command::{
    ActionType, CheckKind, Command, KeyPress, ProbeCommand, RunAction, ValidateCheck,
};
use impresario_core::config::{
    expand_actor_ref, expand_actor_refs, ActorResolveError, DeviceConfig, ImpresarioConfig,
};
use impresario_core::diag::{suggest, Code, Diagnostic, Diagnostics};
use impresario_core::platforms::Platform;
use impresario_core::probe::{self, Claim, ProbeAssertion, ProbeKind};
use impresario_core::selectors::{self, Selector};
use impresario_core::span::Span;
use impresario_core::strings::{ParsedString, Part};
use impresario_core::support;
use impresario_core::workspace::Workspace;

/// Act selection from the CLI (`--tag` / `--exclude-tag` / `--act`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    /// `--tag` (repeatable = OR).
    pub tags: Vec<String>,
    /// `--exclude-tag` (wins over `--tag`).
    pub exclude_tags: Vec<String>,
    /// `--act` (exact name).
    pub acts: Vec<String>,
}

impl Selection {
    pub(crate) fn tags_selected(&self, tags: &BTreeSet<String>) -> bool {
        if tags
            .iter()
            .any(|tag| self.exclude_tags.iter().any(|excluded| excluded == tag))
        {
            return false;
        }
        if !self.tags.is_empty() && !tags.iter().any(|tag| self.tags.contains(tag)) {
            return false;
        }
        true
    }

    fn act_selected(&self, act: &ActDecl, tags: &BTreeSet<String>) -> bool {
        self.tags_selected(tags)
            && (self.acts.is_empty() || self.acts.iter().any(|name| name == &act.name))
    }
}

/// What kind of master work a planned step is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlannedStage {
    pub name: String,
    /// Location of the stage declaration in its `.theatre` file.
    pub span: Span,
    pub steps: Vec<PlannedStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlannedAct {
    pub name: String,
    pub tags: Vec<String>,
    /// Values of the selected `Examples:` row (empty for a table-less Act).
    pub example: BTreeMap<String, String>,
    pub stages: Vec<PlannedStage>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlannedPlay {
    pub name: String,
    pub file: String,
    pub acts: Vec<PlannedAct>,
}

/// One selected device with everything it will run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DevicePlan {
    pub device: DeviceConfig,
    /// The app bound to this run, or `None` for an app-less web run.
    pub app: Option<AppConfig>,
    pub plays: Vec<PlannedPlay>,
}

/// The complete plan for a `run` invocation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Devices selected for this run (after `--device`/`--platform`).
    pub devices: &'a [DeviceConfig],
    /// Every configured device, used to resolve actors that were not selected.
    pub all_devices: &'a [DeviceConfig],
    /// Merged actor groups (`@name`).
    pub groups: &'a BTreeMap<String, Vec<String>>,
    /// App definitions from the global/workspace layers.
    pub apps: &'a [AppConfig],
    /// `--app` override (wins over every play's `App:` header).
    pub cli_app: Option<&'a str>,
    /// The selected Plays, in run order.
    pub plays: &'a [PlayFile],
    pub selection: &'a Selection,
    /// `--var K=V` overrides (beat play vars).
    pub cli_vars: &'a BTreeMap<String, String>,
    /// `--arg ACT.PARAM=VALUE` overrides (beat Examples and defaults).
    pub cli_args: &'a BTreeMap<String, String>,
    /// `--timeout <ms>` (beats `timeout_default_ms`).
    pub cli_timeout: Option<u64>,
}

/// Compile the plan. On any error returns every diagnostic collected.
pub fn build_plan(input: &PlanInput<'_>) -> Result<ExecutionPlan, Diagnostics> {
    let mut diagnostics = Diagnostics::new();
    diagnostics
        .extend(impresario_core::workspace::stage_implementation_diagnostics(input.workspace));
    validate_cli_args(input, &mut diagnostics);
    let mut secrets: Vec<String> = Vec::new();
    let mut devices = Vec::new();
    let mut reported_not_applicable: HashSet<(String, Platform)> = HashSet::new();
    let mut reported_selectors: HashSet<(String, Platform)> = HashSet::new();

    // Bind play -> app before any device work (TASK-2 section 5, D-21).
    let app = resolve_app(input, &mut diagnostics);
    validate_actor_filters(input, &mut diagnostics);

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
        // Each device gets its own resolved artifact path. The same app can be
        // planned for devices started from different working directories; the
        // transport must never interpret a relative package path itself.
        let device_app = app
            .as_ref()
            .map(|app| resolve_app_packages(input.base, app, device));
        let mut plays = Vec::new();
        for (play, vars) in input.plays.iter().zip(&play_vars) {
            let mut acts = Vec::new();
            for act in &play.acts {
                let tags = effective_tags(play, act, input.workspace);
                if !act_applies_to(act, platform)
                    || !input.selection.act_selected(act, &tags)
                    || !act_actors_allow(play, act, device, input)
                {
                    continue;
                }
                let act_args: BTreeMap<String, String> = input
                    .cli_args
                    .iter()
                    .filter_map(|(key, value)| {
                        key.strip_prefix(act.name.as_str())
                            .and_then(|rest| rest.strip_prefix('.'))
                            .map(|param| (param.to_string(), value.clone()))
                    })
                    .collect();
                for example in act_examples(act) {
                    let mut ctx = CompileContext {
                        input,
                        device,
                        platform,
                        stage_timeout: None,
                        vars,
                        declared_vars: &play.var_decls,
                        params: BTreeMap::new(),
                        examples: example.clone(),
                        act_args: act_args.clone(),
                        reported_not_applicable: &mut reported_not_applicable,
                        reported_selectors: &mut reported_selectors,
                        secrets: &mut secrets,
                        diagnostics: &mut diagnostics,
                    };
                    bind_act_params(act, &mut ctx);
                    let mut stages = Vec::new();
                    if !act.performs.is_empty() {
                        let mut stack = Vec::new();
                        for perform in &act.performs {
                            expand_perform(
                                perform,
                                &mut ctx,
                                input.workspace,
                                &mut stack,
                                &mut stages,
                            );
                        }
                    } else {
                        for stage in &act.stages {
                            compile_legacy_stage(stage, &mut ctx, &mut stages);
                        }
                    }
                    if !stages.is_empty() {
                        acts.push(PlannedAct {
                            name: act.name.clone(),
                            tags: tags.iter().cloned().collect(),
                            example: ctx.examples.clone(),
                            stages,
                        });
                    }
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
            app: device_app,
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

/// Return the app with its package for `device.platform` made absolute.
/// Other platforms are left untouched because another plan may use them.
fn resolve_app_packages(base: &Path, app: &AppConfig, device: &DeviceConfig) -> AppConfig {
    let mut resolved = app.clone();
    let Some(entry) = resolved.platforms.get_mut(&device.platform) else {
        return resolved;
    };
    let Some(package) = entry.package.as_mut() else {
        return resolved;
    };
    let path = PathBuf::from(&*package);
    if !path.is_absolute() {
        *package = base.join(path).to_string_lossy().into_owned();
    }
    resolved
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

/// Effective tags of an act: play tags plus act tags plus the tags of every
/// theatre/stage performed by the act, transitively and cycle-safely.
pub(crate) fn effective_tags(
    play: &PlayFile,
    act: &ActDecl,
    workspace: &Workspace,
) -> BTreeSet<String> {
    let mut tags: BTreeSet<String> = play.tags.iter().map(|tag| tag.name.clone()).collect();
    tags.extend(act.tags.iter().map(|tag| tag.name.clone()));
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for stage in &act.stages {
        tags.extend(stage.tags.iter().map(|tag| tag.name.clone()));
    }
    add_perform_tags(
        &act.performs,
        workspace,
        &mut tags,
        &mut visiting,
        &mut visited,
    );
    tags
}

fn add_perform_tags(
    performs: &[PerformDecl],
    workspace: &Workspace,
    tags: &mut BTreeSet<String>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) {
    for perform in performs {
        for (theatre, stage) in workspace.stage_instances(&perform.stage_name) {
            tags.extend(theatre.tags.iter().map(|tag| tag.name.clone()));
            tags.extend(stage.tags.iter().map(|tag| tag.name.clone()));
            let key = format!(
                "{}:{}:{}",
                stage.name_span.file().display(),
                stage.name_span.line,
                stage.name_span.col
            );
            if !visited.insert(key) || !visiting.insert(stage.name.clone()) {
                continue;
            }
            let nested: Vec<PerformDecl> = stage
                .items
                .iter()
                .filter_map(|item| match item {
                    StageItem::Perform(perform) => Some(perform.clone()),
                    StageItem::Scene(_) => None,
                })
                .collect();
            add_perform_tags(&nested, workspace, tags, visiting, visited);
            visiting.remove(&stage.name);
        }
    }
}

fn selected_actor_names(input: &PlanInput<'_>) -> BTreeSet<String> {
    input
        .devices
        .iter()
        .map(|device| device.name.clone())
        .collect()
}

fn expand_actor_names<'a>(
    references: impl IntoIterator<Item = &'a str>,
    input: &PlanInput<'_>,
) -> BTreeSet<String> {
    expand_actor_refs(input.all_devices, input.groups, references).unwrap_or_default()
}

fn act_actors_allow(
    play: &PlayFile,
    act: &ActDecl,
    device: &DeviceConfig,
    input: &PlanInput<'_>,
) -> bool {
    let references = play.effective_actors(act);
    if references.is_empty() {
        return true;
    }
    let configured: BTreeSet<String> =
        expand_actor_names(references.iter().map(|named| named.name.as_str()), input);
    configured.contains(&device.name)
}

fn filter_allows(
    filter: &impresario_core::ast::ActorFilter,
    device: &str,
    input: &PlanInput<'_>,
) -> bool {
    let configured: BTreeSet<String> =
        expand_actor_names(filter.actors.iter().map(|named| named.name.as_str()), input);
    match filter.kind {
        ActorFilterKind::For => configured.contains(device),
        ActorFilterKind::Except => !configured.contains(device),
    }
}

fn perform_allows(perform: &PerformDecl, device: &DeviceConfig, input: &PlanInput<'_>) -> bool {
    perform
        .filter
        .as_ref()
        .is_none_or(|filter| filter_allows(filter, &device.name, input))
}

fn actor_diagnostic(
    diagnostics: &mut Diagnostics,
    seen: &mut HashSet<String>,
    named: &impresario_core::ast::Named,
    error: &ActorResolveError,
) {
    let (code, message) = match error {
        ActorResolveError::Unknown(name) => (
            Code::DslUnknownActor,
            format!(
                "unknown actor or group '{name}'; it is not defined in the devices/groups configuration"
            ),
        ),
        ActorResolveError::Cycle(path) => (
            Code::DslGroupCycle,
            format!("actor group cycle: {}", path.join(" -> ")),
        ),
    };
    let key = format!("{}:{}", code, named.span);
    if seen.insert(key) {
        diagnostics.push(
            Diagnostic::error(code, message)
                .with_span(named.span.clone())
                .with_help(
                    "define the actor in impresario.devices.config or fix the group membership",
                ),
        );
    }
}

fn validate_filter(
    filter: &impresario_core::ast::ActorFilter,
    effective: &BTreeSet<String>,
    input: &PlanInput<'_>,
    diagnostics: &mut Diagnostics,
    seen: &mut HashSet<String>,
) {
    for named in &filter.actors {
        match expand_actor_ref(input.all_devices, input.groups, &named.name) {
            Ok(members) => {
                for member in members {
                    if effective.contains(&member) {
                        continue;
                    }
                    let key = format!("{}:{}", Code::DslActorFilter, named.span);
                    if seen.insert(key) {
                        diagnostics.push(
                            Diagnostic::error(
                                Code::DslActorFilter,
                                format!(
                                    "actor '{member}' in Perform {:?} is not among the act's effective actors",
                                    filter.kind
                                ),
                            )
                            .with_span(named.span.clone())
                            .with_help("use an actor from the act's Actors: list or remove it from the filter"),
                        );
                    }
                }
            }
            Err(error) => actor_diagnostic(diagnostics, seen, named, &error),
        }
    }
}

fn validate_perform_filters(
    performs: &[PerformDecl],
    workspace: &Workspace,
    effective: &BTreeSet<String>,
    input: &PlanInput<'_>,
    diagnostics: &mut Diagnostics,
    seen: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
) {
    for perform in performs {
        if let Some(filter) = &perform.filter {
            validate_filter(filter, effective, input, diagnostics, seen);
        }
        let implementations = workspace.stage_implementations(&perform.stage_name);
        for stage in implementations {
            if !visiting.insert(stage.name.clone()) {
                continue;
            }
            let nested: Vec<PerformDecl> = stage
                .items
                .iter()
                .filter_map(|item| match item {
                    StageItem::Perform(perform) => Some(perform.clone()),
                    StageItem::Scene(_) => None,
                })
                .collect();
            validate_perform_filters(
                &nested,
                workspace,
                effective,
                input,
                diagnostics,
                seen,
                visiting,
            );
            visiting.remove(&stage.name);
        }
    }
}

/// Resolve actor/group references and check every `For`/`Except` against the
/// act's effective actor set. Runs once so diagnostics are not repeated per
/// device.
fn validate_actor_filters(input: &PlanInput<'_>, diagnostics: &mut Diagnostics) {
    let mut seen: HashSet<String> = HashSet::new();
    for play in input.plays {
        for act in &play.acts {
            let tags = effective_tags(play, act, input.workspace);
            if !input.selection.act_selected(act, &tags) {
                continue;
            }
            let references = play.effective_actors(act);
            let mut configured = BTreeSet::new();
            for named in references {
                match expand_actor_ref(input.all_devices, input.groups, &named.name) {
                    Ok(members) => configured.extend(members),
                    Err(error) => actor_diagnostic(diagnostics, &mut seen, named, &error),
                }
            }
            let effective = if references.is_empty() {
                selected_actor_names(input)
            } else {
                configured
                    .into_iter()
                    .filter(|name| input.devices.iter().any(|device| &device.name == name))
                    .collect()
            };
            let mut visiting = HashSet::new();
            validate_perform_filters(
                &act.performs,
                input.workspace,
                &effective,
                input,
                diagnostics,
                &mut seen,
                &mut visiting,
            );
        }
    }
}

/// One execution per `Examples:` row; a table-less Act runs once.
fn act_examples(act: &ActDecl) -> Vec<BTreeMap<String, String>> {
    let Some(table) = &act.examples else {
        return vec![BTreeMap::new()];
    };
    if table.rows.is_empty() {
        return vec![BTreeMap::new()];
    }
    table
        .rows
        .iter()
        .map(|row| {
            table
                .columns
                .iter()
                .zip(&row.values)
                .filter_map(|(column, value)| {
                    value
                        .as_ref()
                        .map(|value| (column.name.clone(), value.clone()))
                })
                .collect()
        })
        .collect()
}

/// Bind Act parameters from the current Examples row, then from defaults.
/// Required parameters that are not supplied stay unbound; a `${param.*}`
/// reference in the Act then reports `IMP-DSL-036` with the exact span.
fn bind_act_params(act: &ActDecl, ctx: &mut CompileContext<'_, '_>) {
    for param in &act.params {
        if ctx.params.contains_key(&param.name) {
            continue;
        }
        if let Some(value) = ctx.act_args.get(&param.name).cloned() {
            ctx.params.insert(param.name.clone(), value.clone());
            ctx.examples.insert(param.name.clone(), value);
            continue;
        }
        if let Some(value) = ctx.examples.get(&param.name).cloned() {
            ctx.params.insert(param.name.clone(), value);
            continue;
        }
        if let Some(default) = &param.default {
            let mut local = Diagnostics::new();
            let resolved = resolve_string(default, ctx, &mut local);
            ctx.diagnostics.extend(local.into_vec());
            if let Some(value) = resolved {
                ctx.params.insert(param.name.clone(), value);
            }
        }
    }
}

fn compile_legacy_stage(
    stage: &StageDecl,
    ctx: &mut CompileContext<'_, '_>,
    out: &mut Vec<PlannedStage>,
) {
    let mut steps = Vec::new();
    let previous_timeout = ctx.stage_timeout;
    ctx.stage_timeout = stage.timeout;
    for block in &stage.scenes {
        compile_scene_block(
            block.scene_name.as_str(),
            &block.steps,
            &block.scene_span,
            ctx,
            &mut steps,
        );
    }
    ctx.stage_timeout = previous_timeout;
    flush_stage(&stage.name, &stage.name_span, &mut steps, out);
}

fn flush_stage(name: &str, span: &Span, steps: &mut Vec<PlannedStep>, out: &mut Vec<PlannedStage>) {
    if !steps.is_empty() {
        out.push(PlannedStage {
            name: name.to_string(),
            span: span.clone(),
            steps: std::mem::take(steps),
        });
    }
}

/// Expand one `Perform:` into ordered stage segments. Nested performances are
/// recursively expanded in place, so the executed order always matches the
/// written order of scenes and Perform lines.
fn expand_perform(
    perform: &PerformDecl,
    ctx: &mut CompileContext<'_, '_>,
    workspace: &Workspace,
    stack: &mut Vec<String>,
    out: &mut Vec<PlannedStage>,
) {
    if !perform_allows(perform, ctx.device, ctx.input) {
        return;
    }
    let Some(stage) = workspace.stage_for(&perform.stage_name, ctx.platform) else {
        let mut diag = if workspace.stage(&perform.stage_name).is_some() {
            Diagnostic::error(
                Code::DslStageCoverage,
                format!(
                    "no implementation of stage '{}' serves platform '{}' for actor '{}'",
                    perform.stage_name, ctx.platform, ctx.device.name
                ),
            )
            .with_span(perform.stage_span.clone())
            .with_help(format!(
                "add an implementation with `Audience: {}` under a matching theatre, or remove the actor from the act",
                ctx.platform
            ))
        } else {
            let mut diag = Diagnostic::error(
                Code::DslUnknownStage,
                format!("unknown stage '{}'", perform.stage_name),
            )
            .with_span(perform.stage_span.clone())
            .with_help("declare it in a .theatre file under the configured theatres_dir");
            if let Some(hint) = suggest(&perform.stage_name, &workspace.stage_names()) {
                diag = diag.with_help(hint);
            }
            diag
        };
        if workspace.stage(&perform.stage_name).is_some() {
            diag = diag.with_help(format!(
                "implementations exist for {}; the actor's device platform is '{}'",
                workspace
                    .stage_implementations(&perform.stage_name)
                    .iter()
                    .flat_map(|stage| {
                        workspace
                            .theatres
                            .iter()
                            .find(|theatre| {
                                theatre.stages.iter().any(|item| item.name == stage.name)
                            })
                            .map(|theatre| theatre.stage_audience(stage))
                            .unwrap_or_default()
                    })
                    .map(|platform| platform.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", "),
                ctx.platform
            ));
        }
        ctx.diagnostics.push(diag);
        return;
    };
    if let Some(cycle_start) = stack.iter().position(|name| name == &stage.name) {
        let cycle = stack
            .get(cycle_start..)
            .unwrap_or(&[])
            .iter()
            .cloned()
            .chain(std::iter::once(stage.name.clone()))
            .collect::<Vec<_>>()
            .join(" -> ");
        ctx.diagnostics.push(
            Diagnostic::error(Code::DslStageCycle, format!("stage cycle: {cycle}"))
                .with_span(perform.stage_span.clone())
                .with_help("a stage cannot perform itself, directly or through another stage"),
        );
        return;
    }
    if stack.len() >= 64 {
        ctx.diagnostics.push(
            Diagnostic::error(
                Code::DslStageDepth,
                format!("stage nesting is deeper than 64 levels at '{}'", stage.name),
            )
            .with_span(perform.stage_span.clone())
            .with_help("flatten the stage composition or split it into separate Acts"),
        );
        return;
    }

    let mut bound: BTreeMap<String, String> = BTreeMap::new();
    let mut provided: HashSet<String> = HashSet::new();
    for arg in &perform.args {
        if !stage.params.iter().any(|param| param.name == arg.name) {
            ctx.diagnostics.push(
                Diagnostic::error(
                    Code::DslInvalidStageArguments,
                    format!("stage '{}' has no parameter '{}'", stage.name, arg.name),
                )
                .with_span(arg.name_span.clone())
                .with_help(if stage.params.is_empty() {
                    "this stage declares no parameters".to_string()
                } else {
                    format!(
                        "parameters: {}",
                        stage
                            .params
                            .iter()
                            .map(|param| param.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
            );
            continue;
        }
        provided.insert(arg.name.clone());
        let mut local = Diagnostics::new();
        let resolved = resolve_string(&arg.value, ctx, &mut local);
        ctx.diagnostics.extend(local.into_vec());
        if let Some(value) = resolved {
            bound.insert(arg.name.clone(), value);
        }
    }

    for param in &stage.params {
        if bound.contains_key(&param.name) {
            continue;
        }
        if let Some(default) = &param.default {
            let saved = std::mem::replace(&mut ctx.params, bound.clone());
            let mut local = Diagnostics::new();
            let resolved = resolve_string(default, ctx, &mut local);
            ctx.params = saved;
            ctx.diagnostics.extend(local.into_vec());
            if let Some(value) = resolved {
                bound.insert(param.name.clone(), value);
            }
        } else if !provided.contains(&param.name) {
            ctx.diagnostics.push(
                Diagnostic::error(
                    Code::DslMissingStageArgument,
                    format!("stage '{}' needs argument '{}'", stage.name, param.name),
                )
                .with_span(perform.stage_span.clone())
                .with_help(format!(
                    "pass it with Perform: {}(... {} = 'value' ...)",
                    stage.name, param.name
                )),
            );
        }
    }

    let mut child = CompileContext {
        input: ctx.input,
        device: ctx.device,
        platform: ctx.platform,
        stage_timeout: stage.timeout,
        vars: ctx.vars,
        declared_vars: ctx.declared_vars,
        params: bound,
        examples: ctx.examples.clone(),
        act_args: ctx.act_args.clone(),
        reported_not_applicable: &mut *ctx.reported_not_applicable,
        reported_selectors: &mut *ctx.reported_selectors,
        secrets: &mut *ctx.secrets,
        diagnostics: &mut *ctx.diagnostics,
    };

    stack.push(stage.name.clone());
    let mut pending = Vec::new();
    if stage.items.is_empty() {
        for block in &stage.scenes {
            compile_scene_block(
                block.scene_name.as_str(),
                &block.steps,
                &block.scene_span,
                &mut child,
                &mut pending,
            );
        }
    } else {
        for item in &stage.items {
            match item {
                StageItem::Scene(block) => compile_scene_block(
                    block.scene_name.as_str(),
                    &block.steps,
                    &block.scene_span,
                    &mut child,
                    &mut pending,
                ),
                StageItem::Perform(nested) => {
                    flush_stage(&stage.name, &stage.name_span, &mut pending, out);
                    expand_perform(nested, &mut child, workspace, stack, out);
                }
            }
        }
    }
    flush_stage(&stage.name, &stage.name_span, &mut pending, out);
    stack.pop();
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
            Part::Param { name: param, span } => {
                diagnostics.push(
                    Diagnostic::error(
                        Code::DslUnknownParameter,
                        format!("parameter '{param}' is not available in a Var: declaration"),
                    )
                    .with_span(span.clone())
                    .with_help("Var: values are resolved once per play; use `${param.*}` in a Stage action instead"),
                );
                failed = true;
                break;
            }
            Part::Example { name: column, span } => {
                diagnostics.push(
                    Diagnostic::error(
                        Code::DslUnknownExampleColumn,
                        format!("example column '{column}' is not available in a Var: declaration"),
                    )
                    .with_span(span.clone())
                    .with_help("use `${example.*}` in an Act's Perform: arguments instead"),
                );
                failed = true;
                break;
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
    /// Stage/act parameters bound for the current Perform expansion.
    params: BTreeMap<String, String>,
    /// Examples row values bound for the current Act execution.
    examples: BTreeMap<String, String>,
    /// CLI `--arg ACT.PARAM=VALUE` values for this Act.
    act_args: BTreeMap<String, String>,
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
            match &check.condition {
                CheckCondition::Showing(showing) => match selector {
                    Some(Some(selector)) => {
                        let command =
                            build_showing_probe(&selector, showing, ctx, &mut diagnostics);
                        match command {
                            Some(probe) => Ok(PlannedStep {
                                span: step.span.clone(),
                                source_text: step.source_text.clone(),
                                label: format!("check {}", check.condition.as_str()),
                                timeout_ms,
                                command: Some(Command::Probe(probe)),
                                work: PlannedWork::Wire,
                            }),
                            None => Err(()),
                        }
                    }
                    _ => Err(()),
                },
                CheckCondition::Player(player) => match selector {
                    Some(Some(selector)) => {
                        let command = build_player_probe(&selector, player, ctx, &mut diagnostics);
                        match command {
                            Some(probe) => Ok(PlannedStep {
                                span: step.span.clone(),
                                source_text: step.source_text.clone(),
                                label: format!("check {}", check.condition.as_str()),
                                timeout_ms,
                                command: Some(Command::Probe(probe)),
                                work: PlannedWork::Wire,
                            }),
                            None => Err(()),
                        }
                    }
                    _ => Err(()),
                },
                _ => {
                    let condition = match &check.condition {
                        CheckCondition::IsVisible => Some((CheckKind::Visible, String::new(), 0)),
                        CheckCondition::IsHidden => Some((CheckKind::Hidden, String::new(), 0)),
                        CheckCondition::IsEnabled => Some((CheckKind::Enabled, String::new(), 0)),
                        CheckCondition::IsDisabled => Some((CheckKind::Disabled, String::new(), 0)),
                        CheckCondition::IsFocused => Some((CheckKind::Focused, String::new(), 0)),
                        CheckCondition::TextEquals { text } => {
                            resolve_string(text, ctx, &mut diagnostics)
                                .map(|text| (CheckKind::TextEquals, text, 0))
                        }
                        CheckCondition::TextContains { text } => {
                            resolve_string(text, ctx, &mut diagnostics)
                                .map(|text| (CheckKind::TextContains, text, 0))
                        }
                        CheckCondition::Count { count } => {
                            Some((CheckKind::Count, String::new(), *count))
                        }
                        CheckCondition::Showing(_) | CheckCondition::Player(_) => None,
                    };
                    if !support::check_support(
                        ctx.platform,
                        support::verb_of_check(&check.condition),
                    )
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

fn build_showing_probe(
    selector: &Selector,
    showing: &ShowingCheck,
    ctx: &CompileContext<'_, '_>,
    diagnostics: &mut Diagnostics,
) -> Option<ProbeCommand> {
    let row = probe::support(ctx.platform);
    let mut ok = row.showing;
    if !row.showing {
        unsupported_probe(
            ctx,
            &format!("Showing is not supported on platform '{}'", ctx.platform),
            None,
            diagnostics,
        );
    }
    let mut claims = vec![Claim {
        id: "showing=present".to_string(),
        negated: showing.negated,
    }];
    let mut command = ProbeCommand::new(ProbeKind::Showing, selector.clone(), Vec::new());
    for option in &showing.options {
        match option {
            impresario_core::ast::ShowingOption::Coverage { percent } => {
                command.min_coverage_pct = *percent;
                claims.push(Claim::new(format!("showing.coverage>={percent}%")));
            }
            impresario_core::ast::ShowingOption::Decoded => {
                command.require_decoded = true;
                claims.push(Claim::new("showing.decoded=true"));
            }
            impresario_core::ast::ShowingOption::Source { contains } => {
                if !matches!(
                    ctx.platform,
                    Platform::Web | Platform::WebOs | Platform::Tizen
                ) {
                    ok = false;
                    unsupported_probe(
                        ctx,
                        "Showing source is a web-only option",
                        Some("supported platforms: web, webOS, Tizen"),
                        diagnostics,
                    );
                }
                command.require_source = true;
                claims.push(Claim::new(format!("showing.source~{contains}")));
            }
        }
    }
    if !ok {
        return None;
    }
    command.claims = claims;
    Some(command)
}

fn build_player_probe(
    selector: &Selector,
    player: &PlayerCheck,
    ctx: &CompileContext<'_, '_>,
    diagnostics: &mut Diagnostics,
) -> Option<ProbeCommand> {
    let row = probe::support(ctx.platform);
    let drm_check = player.drm.is_some();
    let mut ok = if drm_check { row.drm } else { row.player };
    if !row.player {
        unsupported_probe(
            ctx,
            &format!("Player is not supported on platform '{}'", ctx.platform),
            None,
            diagnostics,
        );
    }
    if drm_check && !row.drm {
        unsupported_probe(
            ctx,
            &format!(
                "DRM probes are not supported on platform '{}'",
                ctx.platform
            ),
            None,
            diagnostics,
        );
    }
    let mut claims = Vec::new();
    if let Some(status) = &player.status {
        claims.push(Claim {
            id: format!("state={}", status.status.as_str()),
            negated: status.negated,
        });
    }
    if let Some(drm) = &player.drm {
        claims.push(Claim {
            id: format!("drm.enabled={}", drm.enabled),
            negated: false,
        });
        if let Some(provider) = drm.provider {
            if !row.supports_provider(provider) {
                ok = false;
                let supported = row
                    .providers
                    .iter()
                    .map(|provider| provider.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                unsupported_probe(
                    ctx,
                    &format!(
                        "DRM provider {} is not supported on platform '{}'",
                        provider.as_str(),
                        ctx.platform
                    ),
                    Some(&format!("supported providers: {supported}")),
                    diagnostics,
                );
            }
            claims.push(Claim {
                id: format!("drm.provider={}", provider.as_str()),
                negated: drm.provider_negated,
            });
        }
    }
    let mut assertions = Vec::new();
    for assertion in &player.assertions {
        if !row.supports_property(assertion.property) {
            ok = false;
            let supported = row
                .properties
                .iter()
                .map(|property| property.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            unsupported_probe(
                ctx,
                &format!(
                    "player property '{}' is not supported on platform '{}'",
                    assertion.property.as_str(),
                    ctx.platform
                ),
                Some(&format!("supported properties: {supported}")),
                diagnostics,
            );
        }
        assertions.push(ProbeAssertion {
            property: assertion.property.as_str().to_string(),
            operator: assertion.operator,
            value: assertion.value.clone(),
            negated: assertion.negated,
        });
    }
    if !ok {
        return None;
    }
    let kind = if drm_check {
        ProbeKind::Drm
    } else {
        ProbeKind::Player
    };
    Some(
        ProbeCommand::new(kind, selector.clone(), claims)
            .with_assertions(assertions)
            .with_window_ms(3000),
    )
}

fn unsupported_probe(
    ctx: &CompileContext<'_, '_>,
    message: &str,
    help: Option<&str>,
    diagnostics: &mut Diagnostics,
) {
    let mut diagnostic = Diagnostic::error(
        Code::DslUnsupportedActionOnPlatform,
        format!("{message} (device '{}')", ctx.device.name),
    );
    if let Some(help) = help {
        diagnostic = diagnostic.with_help(help.to_string());
    } else {
        diagnostic = diagnostic.with_help("see docs/PARITY.md for the probe matrix");
    }
    diagnostics.push(diagnostic);
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
            Part::Param { name, span } => match ctx.params.get(name) {
                Some(value) => out.push_str(value),
                None => {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::DslUnknownParameter,
                            format!("parameter '{name}' is not bound here"),
                        )
                        .with_span(span.clone())
                        .with_help("declare it in the Stage/Act header or pass it in the Perform: argument list"),
                    );
                    return None;
                }
            },
            Part::Example { name, span } => match ctx.examples.get(name) {
                Some(value) => out.push_str(value),
                None => {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::DslUnknownExampleColumn,
                            format!("example column '{name}' is not bound here"),
                        )
                        .with_span(span.clone())
                        .with_help("declare it in the Act's Examples: table"),
                    );
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

fn validate_cli_args(input: &PlanInput<'_>, diagnostics: &mut Diagnostics) {
    for key in input.cli_args.keys() {
        let Some((act_name, param)) = key.rsplit_once('.') else {
            diagnostics.push(
                Diagnostic::error(
                    Code::DslInvalidStageArguments,
                    format!("invalid --arg '{key}': expected ACT.PARAM=VALUE"),
                )
                .with_help("for example: --arg 'Login.user=alice'"),
            );
            continue;
        };
        let known = input.plays.iter().any(|play| {
            play.acts
                .iter()
                .any(|act| act.name == act_name && act.params.iter().any(|decl| decl.name == param))
        });
        if !known {
            diagnostics.push(
                Diagnostic::error(
                    Code::DslInvalidStageArguments,
                    format!("--arg '{key}' does not match an Act parameter"),
                )
                .with_help("declare it as `Act: Name(param)` and pass `--arg 'Name.param=value'`"),
            );
        }
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

/// Parse `--arg ACT.PARAM=VALUE` overrides.
pub fn parse_cli_args(pairs: &[String]) -> Result<BTreeMap<String, String>, Diagnostics> {
    let mut args = BTreeMap::new();
    let mut diagnostics = Diagnostics::new();
    for pair in pairs {
        match pair.split_once('=') {
            Some((key, value)) => {
                let Some((act, param)) = key.rsplit_once('.') else {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgInvalidValue,
                            format!("invalid --arg '{pair}': expected ACT.PARAM=VALUE"),
                        )
                        .with_help("for example: --arg 'Login.user=alice'"),
                    );
                    continue;
                };
                if act.trim().is_empty() || !is_variable_name(param) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::CfgInvalidValue,
                            format!("invalid --arg '{pair}': expected ACT.PARAM=VALUE"),
                        )
                        .with_help("act names may contain spaces; parameter names are identifiers"),
                    );
                    continue;
                }
                args.insert(key.to_string(), value.to_string());
            }
            None => {
                diagnostics.push(
                    Diagnostic::error(
                        Code::CfgInvalidValue,
                        format!("invalid --arg '{pair}': expected ACT.PARAM=VALUE"),
                    )
                    .with_help("pass arguments as --arg 'Act.param=value'"),
                );
            }
        }
    }
    if diagnostics.has_errors() {
        Err(diagnostics)
    } else {
        Ok(args)
    }
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
    use impresario_core::parser::{parse_play, parse_scene, parse_theatre};
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
            theatres: Vec::new(),
            plays: vec![play],
        }
    }

    fn theatrical_workspace(
        scene_source: &str,
        theatre_source: &str,
        play_source: &str,
    ) -> Workspace {
        let scene = parse_scene(scene_source, Path::new("S.scene"))
            .ast
            .expect("scene parses");
        let theatre = parse_theatre(theatre_source, Path::new("T.theatre"))
            .ast
            .expect("theatre parses");
        let play = parse_play(play_source, Path::new("p.play"))
            .ast
            .expect("play parses");
        Workspace {
            scenes: vec![scene],
            theatres: vec![theatre],
            plays: vec![play],
        }
    }

    fn compile_theatrical(
        scene_source: &str,
        theatre_source: &str,
        play_source: &str,
        platform: Platform,
    ) -> Result<ExecutionPlan, Diagnostics> {
        let workspace = theatrical_workspace(scene_source, theatre_source, play_source);
        let config = config();
        let devices = vec![device(platform)];
        let plays = workspace.plays.clone();
        build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_args: &BTreeMap::new(),
            cli_timeout: None,
        })
    }

    fn compile_theatrical_with_args(
        scene_source: &str,
        theatre_source: &str,
        play_source: &str,
        platform: Platform,
        cli_args: &BTreeMap<String, String>,
    ) -> Result<ExecutionPlan, Diagnostics> {
        let workspace = theatrical_workspace(scene_source, theatre_source, play_source);
        let config = config();
        let devices = vec![device(platform)];
        let plays = workspace.plays.clone();
        build_plan(&PlanInput {
            config: &config,
            workspace: &workspace,
            base: Path::new("."),
            devices: &devices,
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_args,
            cli_timeout: None,
        })
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
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps,
            cli_app,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: vars,
            cli_args: &BTreeMap::new(),
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
    fn theatrical_examples_params_and_nested_stages_compile_in_order() {
        let scene = r#"Scene: LoginScreen
Platforms: cross-platform

| Piece | web | android | ios |
|---|---|---|---|
| UsernameInput | #username | id:username | accessibilityId:username |
| PasswordInput | #password | id:password | accessibilityId:password |
| SubmitButton | button[type='submit'] | id:submit | accessibilityId:submit |
"#;
        let theatre = r#"Theatre: Authentication

Stage: Fill(user, password)
    Timeout: 7s
    Scene: LoginScreen
        Action: '${param.user}' => UsernameInput
        Action: '${param.password}' => PasswordInput
    Perform: Submit(remember = 'true')

Stage: Submit(remember = 'true')
    Scene: LoginScreen
        Action: click => SubmitButton
"#;
        let play = r#"Play: P

Act: Login(user, password)
    Examples:
        | user | password |
        | alice | secret |
    Perform: Fill(user = '${example.user}', password = '${example.password}')
"#;
        let plan = compile_theatrical(scene, theatre, play, Platform::Android).expect("plans");
        let acts = &plan.devices[0].plays[0].acts;
        assert_eq!(acts.len(), 1, "one execution per example row");
        let act = &acts[0];
        assert_eq!(act.name, "Login");
        assert_eq!(act.example.get("user").map(String::as_str), Some("alice"));
        let names: Vec<&str> = act.stages.iter().map(|stage| stage.name.as_str()).collect();
        assert_eq!(names, vec!["Fill", "Submit"]);
        assert_eq!(act.stages[0].steps.len(), 2);
        assert_eq!(act.stages[1].steps.len(), 1);
        assert_eq!(act.stages[0].steps[0].timeout_ms, 7_000);
        assert_eq!(act.stages[1].steps[0].timeout_ms, 10_000);
        match &act.stages[0].steps[0].command {
            Some(Command::RunAction(action)) => assert_eq!(action.text, "alice"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn theatrical_examples_create_one_act_execution_per_row() {
        let play = r#"Play: P

Act: Login(user)
    Examples:
        | user |
        | alice |
        | bob |
    Perform: Fill(user = '${example.user}')
"#;
        let theatre = r#"Theatre: A

Stage: Fill(user)
    Scene: S
        Check: Field
"#;
        let plan = compile_theatrical(SCENE, theatre, play, Platform::Android).expect("plans");
        let acts = &plan.devices[0].plays[0].acts;
        assert_eq!(acts.len(), 2);
        assert_eq!(
            acts[0].example.get("user").map(String::as_str),
            Some("alice")
        );
        assert_eq!(acts[1].example.get("user").map(String::as_str), Some("bob"));
    }

    #[test]
    fn cli_args_override_examples_and_defaults() {
        let play = r#"Play: P

Act: Login(user = 'default')
    Examples:
        | user |
        | alice |
    Perform: Fill(user = '${param.user}')
"#;
        let theatre = r#"Theatre: A

Stage: Fill(user)
    Scene: S
        Action: '${param.user}' => Field
"#;
        let args = BTreeMap::from([("Login.user".to_string(), "bob".to_string())]);
        let plan = compile_theatrical_with_args(SCENE, theatre, play, Platform::Android, &args)
            .expect("plans");
        let act = &plan.devices[0].plays[0].acts[0];
        assert_eq!(act.example.get("user").map(String::as_str), Some("bob"));
        match &act.stages[0].steps[0].command {
            Some(Command::RunAction(action)) => assert_eq!(action.text, "bob"),
            other => panic!("unexpected {other:?}"),
        }

        let unknown = BTreeMap::from([("Login.admin".to_string(), "yes".to_string())]);
        let err = compile_theatrical_with_args(SCENE, theatre, play, Platform::Android, &unknown)
            .expect_err("unknown argument");
        assert_eq!(codes(&err), vec!["IMP-DSL-032"]);
    }

    #[test]
    fn theatrical_unknown_stage_missing_argument_and_cycle_are_reported() {
        let play = r#"Play: P

Act: A
    Perform: Missing
"#;
        let theatre = "Theatre: T\n\nStage: Fill(user)\n    Scene: S\n        Check: Field\n";
        let err =
            compile_theatrical(SCENE, theatre, play, Platform::Android).expect_err("unknown stage");
        assert_eq!(codes(&err), vec!["IMP-DSL-031"]);

        let play = r#"Play: P

Act: A
    Perform: Fill
"#;
        let err = compile_theatrical(SCENE, theatre, play, Platform::Android)
            .expect_err("missing argument");
        assert_eq!(codes(&err), vec!["IMP-DSL-037"]);

        let theatre = r#"Theatre: T

Stage: A
    Perform: B

Stage: B
    Perform: A
"#;
        let play = "Play: P\n\nAct: A\n    Perform: A\n";
        let err = compile_theatrical(SCENE, theatre, play, Platform::Android).expect_err("cycle");
        assert_eq!(codes(&err), vec!["IMP-DSL-034"]);
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
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_args: &BTreeMap::new(),
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
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_args: &BTreeMap::new(),
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
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &selection,
            cli_vars: &BTreeMap::new(),
            cli_args: &BTreeMap::new(),
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
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &[],
            cli_app: None,
            plays: &plays,
            selection: &selection,
            cli_vars: &BTreeMap::new(),
            cli_args: &BTreeMap::new(),
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
            theatres: Vec::new(),
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
            all_devices: &devices,
            groups: &BTreeMap::new(),
            apps: &apps,
            cli_app: None,
            plays: &plays,
            selection: &Selection::default(),
            cli_vars: &BTreeMap::new(),
            cli_args: &BTreeMap::new(),
            cli_timeout: None,
        })
        .expect_err("conflict");
        assert_eq!(codes(&diagnostics), vec!["IMP-APP-004"]);
    }

    #[test]
    fn relative_packages_are_resolved_per_device_before_transport_prepare() {
        let apps = app_defs(
            "apps:\n- name: shop\n  platforms:\n    webos:\n      id: com.example.shop\n      package: build/shop.ipk\n    android:\n      id: com.example.shop\n      package: build/shop.apk\n",
        );
        let app = apps.iter().find(|app| app.name == "shop").expect("app");
        let base = if cfg!(windows) {
            Path::new("C:/workspace")
        } else {
            Path::new("/workspace")
        };
        let webos = resolve_app_packages(base, app, &device(Platform::WebOs));
        assert_eq!(
            webos
                .platform(Platform::WebOs)
                .and_then(|entry| entry.package.as_deref()),
            Some(base.join("build/shop.ipk").to_string_lossy().as_ref())
        );
        let android = resolve_app_packages(base, app, &device(Platform::Android));
        assert_eq!(
            android
                .platform(Platform::Android)
                .and_then(|entry| entry.package.as_deref()),
            Some(base.join("build/shop.apk").to_string_lossy().as_ref())
        );

        let absolute = if cfg!(windows) {
            "C:/artifacts/absolute.ipk"
        } else {
            "/artifacts/absolute.ipk"
        };
        let mut app = app.clone();
        app.platforms
            .get_mut(&Platform::WebOs)
            .expect("webos entry")
            .package = Some(absolute.to_string());
        let unchanged = resolve_app_packages(base, &app, &device(Platform::WebOs));
        assert_eq!(
            unchanged
                .platform(Platform::WebOs)
                .and_then(|entry| entry.package.as_deref()),
            Some(absolute)
        );
    }
    #[test]
    fn compiles_showing_and_player_probes() {
        let play = r#"Play: P

Act: A

Stage: S1
    Scene: S
        Check: Showing => Button with coverage>=50%, decoded
        Check: Player => Button Playing DRM-enabled Widevine with position>30s, resolution>=1080p, not muted timeout 20s
        Check: not Showing => Field
        Check: Player => Field not Buffering
"#;
        let plan = compile(SCENE, play, Platform::Web, &BTreeMap::new()).expect("plans");
        let act = plan.devices[0].plays[0].acts[0].clone();
        let steps: Vec<&PlannedStep> = act
            .stages
            .iter()
            .flat_map(|stage| stage.steps.iter())
            .collect();
        assert_eq!(steps.len(), 4);
        match &steps[0].command {
            Some(Command::Probe(probe)) => {
                assert_eq!(probe.kind, ProbeKind::Showing);
                assert_eq!(probe.min_coverage_pct, 50);
                assert!(probe.require_decoded);
                assert!(probe
                    .claims
                    .iter()
                    .any(|claim| claim.id == "showing=present"));
            }
            other => panic!("expected Showing probe, got {other:?}"),
        }
        match &steps[1].command {
            Some(Command::Probe(probe)) => {
                assert_eq!(probe.kind, ProbeKind::Drm);
                assert!(probe.claims.iter().any(|claim| claim.id == "state=Playing"));
                assert!(probe
                    .claims
                    .iter()
                    .any(|claim| claim.id == "drm.provider=Widevine"));
                assert_eq!(probe.assertions[0].value, "30");
                assert_eq!(probe.assertions[1].value, "1080");
                assert_eq!(steps[1].timeout_ms, 20_000);
            }
            other => panic!("expected Player probe, got {other:?}"),
        }
        match &steps[2].command {
            Some(Command::Probe(probe)) => {
                assert!(probe.claims[0].negated);
            }
            other => panic!("expected Showing probe, got {other:?}"),
        }
        match &steps[3].command {
            Some(Command::Probe(probe)) => {
                assert_eq!(probe.kind, ProbeKind::Player);
                assert!(probe.claims[0].negated);
            }
            other => panic!("expected Player probe, got {other:?}"),
        }
    }

    #[test]
    fn showing_source_is_web_only() {
        let play = r#"Play: P

Act: A

Stage: S1
    Scene: S
        Check: Showing => Button with source~"https://cdn.example"
"#;
        let plan = compile(SCENE, play, Platform::Web, &BTreeMap::new()).expect("plans");
        let command = plan.devices[0].plays[0].acts[0].stages[0].steps[0]
            .command
            .clone()
            .expect("command");
        match command {
            Command::Probe(probe) => assert!(probe.require_source),
            other => panic!("expected probe, got {other:?}"),
        }

        let diagnostics = compile(SCENE, play, Platform::Android, &BTreeMap::new())
            .expect_err("source is web-only");
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("web-only")));
    }
}
