//! Semantic validation (spec sections 5.7 and 6.1): resolution of scenes,
//! pieces and variables; selector availability per platform; unsupported
//! actions/checks; applicability; lints.
//!
//! Validation is device-aware: `IMP-DSL-012`, `IMP-DSL-013` and
//! `IMP-DSL-017` need the set of configured devices, so `validate` receives
//! one [`DeviceRef`] per configured device. Results are deduplicated per
//! platform, which is the granularity that actually drives these rules.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::ast::{
    ActDecl, Action, ActionVerb, ActorFilter, CheckCondition, Ident, PerformDecl, PlayFile,
    SceneBlock, SceneFile, StageDecl, StageItem, Step, StepKind,
};
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
    validate_with_actors(workspace, devices, &BTreeMap::new())
}

/// Validate with the merged actor groups available for `Actors:` and
/// `For`/`Except` resolution.
#[must_use]
pub fn validate_with_actors(
    workspace: &Workspace,
    devices: &[DeviceRef],
    groups: &BTreeMap<String, Vec<String>>,
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    validate_tag_lints(workspace, &mut diagnostics);
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
        validate_play(
            play,
            workspace,
            &platforms,
            devices,
            groups,
            &mut diagnostics,
        );
    }
    lint_workspace(workspace, &platforms, &mut diagnostics);
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

/// Warn about tags that differ only by case or by a `-`/`_` suffix, such as
/// `smoke` and `smoke-test` (IMP-DSL-043).
fn validate_tag_lints(workspace: &Workspace, diagnostics: &mut Diagnostics) {
    let mut tags: Vec<(&str, &Span)> = Vec::new();
    for play in &workspace.plays {
        tags.extend(play.tags.iter().map(|tag| (tag.name.as_str(), &tag.span)));
        for act in &play.acts {
            tags.extend(act.tags.iter().map(|tag| (tag.name.as_str(), &tag.span)));
        }
    }
    for theatre in &workspace.theatres {
        tags.extend(
            theatre
                .tags
                .iter()
                .map(|tag| (tag.name.as_str(), &tag.span)),
        );
        for stage in &theatre.stages {
            tags.extend(stage.tags.iter().map(|tag| (tag.name.as_str(), &tag.span)));
        }
    }
    let mut warned: HashSet<(String, String)> = HashSet::new();
    for (index, (tag, span)) in tags.iter().enumerate() {
        let lower = tag.to_ascii_lowercase();
        for (previous, previous_span) in tags.iter().take(index) {
            let previous_lower = previous.to_ascii_lowercase();
            if previous_lower == lower {
                continue;
            }
            let near = lower.strip_prefix(&format!("{previous_lower}-")).is_some()
                || lower.strip_prefix(&format!("{previous_lower}_")).is_some()
                || previous_lower.strip_prefix(&format!("{lower}-")).is_some()
                || previous_lower.strip_prefix(&format!("{lower}_")).is_some();
            if !near {
                continue;
            }
            let mut pair = [lower.clone(), previous_lower.clone()];
            pair.sort();
            let key = (pair[0].clone(), pair[1].clone());
            if !warned.insert(key) {
                continue;
            }
            diagnostics.push(
                Diagnostic::error(
                    Code::DslLintNearDuplicateTag,
                    format!("near-duplicate tags '{previous}' and '{tag}'"),
                )
                .with_span((*span).clone())
                .with_help(format!(
                    "'{}' is first declared at {}:{}; merge the tags or make the distinction clear",
                    previous,
                    previous_span.file.display(),
                    previous_span.line
                )),
            );
        }
    }
}

fn validate_play(
    play: &PlayFile,
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    devices: &[DeviceRef],
    groups: &BTreeMap<String, Vec<String>>,
    diagnostics: &mut Diagnostics,
) {
    validate_actors(play, workspace, devices, groups, diagnostics);
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

        if !act.performs.is_empty() {
            let act_params: Vec<&str> = act.params.iter().map(|p| p.name.as_str()).collect();
            let example_names: Vec<&str> = act
                .examples
                .as_ref()
                .map(|table| table.columns.iter().map(|c| c.name.as_str()).collect())
                .unwrap_or_default();
            for param in &act.params {
                if let Some(default) = &param.default {
                    check_string_refs(default, &declared, &act_params, &example_names, diagnostics);
                }
            }
            let mut stack = Vec::new();
            for perform in &act.performs {
                validate_perform(
                    perform,
                    workspace,
                    platforms,
                    &declared,
                    &act_params,
                    &example_names,
                    &applies,
                    &mut stack,
                    &mut reported_selectors,
                    &mut reported_not_applicable,
                    diagnostics,
                );
            }
        } else {
            let mut stack = Vec::new();
            for stage in &act.stages {
                validate_stage_body(
                    stage,
                    workspace,
                    platforms,
                    &declared,
                    &[],
                    &[],
                    &applies,
                    &mut stack,
                    &mut reported_selectors,
                    &mut reported_not_applicable,
                    diagnostics,
                );
            }
        }
    }
}

#[derive(Debug)]
enum ActorLookupError {
    Unknown(String),
    Cycle(Vec<String>),
}

fn expand_actor_ref(
    devices: &[DeviceRef],
    groups: &BTreeMap<String, Vec<String>>,
    reference: &str,
    out: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), ActorLookupError> {
    if let Some(group) = reference.strip_prefix('@') {
        if let Some(position) = stack.iter().position(|item| item == group) {
            let mut cycle = stack.get(position..).unwrap_or(&[]).to_vec();
            cycle.push(group.to_string());
            return Err(ActorLookupError::Cycle(cycle));
        }
        let Some(members) = groups.get(group) else {
            return Err(ActorLookupError::Unknown(reference.to_string()));
        };
        stack.push(group.to_string());
        for member in members {
            expand_actor_ref(devices, groups, member, out, stack)?;
        }
        stack.pop();
        return Ok(());
    }
    if devices.iter().any(|device| device.name == reference) {
        out.insert(reference.to_string());
        Ok(())
    } else {
        Err(ActorLookupError::Unknown(reference.to_string()))
    }
}

fn push_actor_lookup_error(
    named: &crate::ast::Named,
    error: &ActorLookupError,
    seen: &mut HashSet<String>,
    diagnostics: &mut Diagnostics,
) {
    let (code, message) = match error {
        ActorLookupError::Unknown(name) => (
            Code::DslUnknownActor,
            format!(
                "unknown actor or group '{name}'; it is not defined in the devices/groups configuration"
            ),
        ),
        ActorLookupError::Cycle(path) => (
            Code::DslGroupCycle,
            format!("actor group cycle: {}", path.join(" -> ")),
        ),
    };
    let key = format!("actor-error:{}:{}", code, named.span);
    if seen.insert(key) {
        diagnostics.push(
            Diagnostic::error(code, message)
                .with_span(named.span.clone())
                .with_help("configure the actor in impresario.devices.config or fix the group"),
        );
    }
}

/// Resolve actors and validate `For`/`Except` plus audience coverage for every
/// perform, including nested ones (TASK-3 sections 9.1/9.2).
fn validate_actors(
    play: &PlayFile,
    workspace: &Workspace,
    devices: &[DeviceRef],
    groups: &BTreeMap<String, Vec<String>>,
    diagnostics: &mut Diagnostics,
) {
    let mut seen: HashSet<String> = HashSet::new();
    for act in &play.acts {
        let references = play.effective_actors(act);
        let mut configured: BTreeSet<String> = BTreeSet::new();
        for named in references {
            if let Err(error) = expand_actor_ref(
                devices,
                groups,
                &named.name,
                &mut configured,
                &mut Vec::new(),
            ) {
                push_actor_lookup_error(named, &error, &mut seen, diagnostics);
            }
        }
        let effective: BTreeSet<String> = if references.is_empty() {
            devices.iter().map(|device| device.name.clone()).collect()
        } else {
            configured
        };
        let act_platforms: Vec<Platform> = act
            .platforms
            .iter()
            .filter_map(|named| named.name.parse::<Platform>().ok())
            .collect();
        for perform in &act.performs {
            validate_actor_coverage(
                perform,
                workspace,
                play,
                act,
                devices,
                groups,
                &effective,
                &act_platforms,
                &mut seen,
                &mut Vec::new(),
                diagnostics,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_actor_coverage(
    perform: &PerformDecl,
    workspace: &Workspace,
    play: &PlayFile,
    act: &ActDecl,
    devices: &[DeviceRef],
    groups: &BTreeMap<String, Vec<String>>,
    effective: &BTreeSet<String>,
    act_platforms: &[Platform],
    seen: &mut HashSet<String>,
    stack: &mut Vec<String>,
    diagnostics: &mut Diagnostics,
) {
    let mut active = effective.clone();
    if let Some(filter) = &perform.filter {
        let mut filtered: BTreeSet<String> = BTreeSet::new();
        for named in &filter.actors {
            if let Err(error) =
                expand_actor_ref(devices, groups, &named.name, &mut filtered, &mut Vec::new())
            {
                push_actor_lookup_error(named, &error, seen, diagnostics);
            }
        }
        for actor in &filtered {
            if effective.contains(actor) {
                continue;
            }
            let key = format!(
                "filter:{}:{}:{actor}",
                filter_kind_name(filter),
                named_span(perform)
            );
            if seen.insert(key) {
                diagnostics.push(
                    Diagnostic::error(
                        Code::DslActorFilter,
                        format!(
                            "actor '{actor}' in Perform {} is not among the current effective actors",
                            filter_kind_name(filter)
                        ),
                    )
                    .with_span(perform.stage_span.clone())
                    .with_help(
                        "use an actor from the act's Actors: list, or remove it from the filter",
                    ),
                );
            }
        }
        active = match filter.kind {
            crate::ast::ActorFilterKind::For => active
                .into_iter()
                .filter(|actor| filtered.contains(actor))
                .collect(),
            crate::ast::ActorFilterKind::Except => active
                .into_iter()
                .filter(|actor| !filtered.contains(actor))
                .collect(),
        };
    }

    if workspace.stage(&perform.stage_name).is_none() {
        return;
    }
    if stack.iter().any(|name| name == &perform.stage_name) {
        return;
    }
    stack.push(perform.stage_name.clone());

    for device in devices {
        if !active.contains(&device.name) {
            continue;
        }
        if !act_platforms.is_empty() && !act_platforms.contains(&device.platform) {
            continue;
        }
        if workspace
            .stage_for(&perform.stage_name, device.platform)
            .is_some()
        {
            continue;
        }
        let key = format!("coverage:{}:{}", named_span(perform), device.name);
        if seen.insert(key) {
            diagnostics.push(
                Diagnostic::error(
                    Code::DslStageCoverage,
                    format!(
                        "stage '{}' has no implementation for actor '{}' on platform '{}' \
                         (act '{}' in {})",
                        perform.stage_name,
                        device.name,
                        device.platform,
                        act.name,
                        play.path.display()
                    ),
                )
                .with_span(perform.stage_span.clone())
                .with_help(format!(
                    "implement the stage for audience '{}', remove '{}' from this Perform \
                     (For/Except), or remove it from the act's Actors",
                    device.platform, device.name
                )),
            );
        }
    }

    // Nested performances run under the calling act's actors. Narrow by each
    // implementation's audience so a web-only implementation is not checked
    // for Android actors.
    for (theatre, stage) in workspace.stage_instances(&perform.stage_name) {
        let audience: BTreeSet<Platform> = theatre.stage_audience(stage).into_iter().collect();
        let implementation_active: BTreeSet<String> = active
            .iter()
            .filter(|actor| {
                devices.iter().any(|device| {
                    &device.name == *actor
                        && audience.contains(&device.platform)
                        && (act_platforms.is_empty() || act_platforms.contains(&device.platform))
                })
            })
            .cloned()
            .collect();
        if implementation_active.is_empty() {
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
        for nested_perform in &nested {
            validate_actor_coverage(
                nested_perform,
                workspace,
                play,
                act,
                devices,
                groups,
                &implementation_active,
                act_platforms,
                seen,
                stack,
                diagnostics,
            );
        }
    }
    stack.pop();
}

fn named_span(perform: &PerformDecl) -> String {
    format!(
        "{}:{}:{}",
        perform.stage_span.file().display(),
        perform.stage_span.line,
        perform.stage_span.col
    )
}

fn filter_kind_name(filter: &ActorFilter) -> &'static str {
    match filter.kind {
        crate::ast::ActorFilterKind::For => "For",
        crate::ast::ActorFilterKind::Except => "Except",
    }
}

/// Validate one `Perform:` call: argument names/arity, string references in
/// the arguments and the recursive stage body.
#[allow(clippy::too_many_arguments)]
fn validate_perform(
    perform: &PerformDecl,
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    declared_vars: &[&str],
    caller_params: &[&str],
    example_names: &[&str],
    caller_applies: &dyn Fn(Platform) -> bool,
    stack: &mut Vec<String>,
    reported_selectors: &mut HashSet<(String, Platform)>,
    reported_not_applicable: &mut HashSet<(String, Platform)>,
    diagnostics: &mut Diagnostics,
) {
    let Some(stage) = workspace.stage(&perform.stage_name) else {
        let mut diag = Diagnostic::error(
            Code::DslUnknownStage,
            format!("unknown stage '{}'", perform.stage_name),
        )
        .with_span(perform.stage_span.clone())
        .with_help("declare it in a .theatre file");
        // A qualified reference (`Theatre.Stage`) must be compared against
        // qualified candidates, otherwise every near miss looks unrelated.
        let mut candidates = workspace.stage_names();
        let qualified = workspace
            .theatres
            .iter()
            .flat_map(|theatre| {
                theatre
                    .stages
                    .iter()
                    .map(move |stage| format!("{}.{}", theatre.name, stage.name))
            })
            .collect::<Vec<_>>();
        candidates.extend(qualified.iter().map(String::as_str));
        if let Some(hint) = suggest(&perform.stage_name, &candidates) {
            diag = diag.with_help(hint);
        }
        diagnostics.push(diag);
        return;
    };

    let mut provided: HashSet<&str> = HashSet::new();
    for arg in &perform.args {
        check_string_refs(
            &arg.value,
            declared_vars,
            caller_params,
            example_names,
            diagnostics,
        );
        if !stage.params.iter().any(|param| param.name == arg.name) {
            diagnostics.push(
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
        }
        provided.insert(arg.name.as_str());
    }
    for param in &stage.params {
        if param.default.is_none() && !provided.contains(param.name.as_str()) {
            diagnostics.push(
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
        if let Some(default) = &param.default {
            let stage_params: Vec<&str> = stage.params.iter().map(|p| p.name.as_str()).collect();
            check_string_refs(
                default,
                declared_vars,
                &stage_params,
                example_names,
                diagnostics,
            );
        }
    }

    if let Some(cycle_start) = stack.iter().position(|name| name == &stage.name) {
        let cycle = stack
            .get(cycle_start..)
            .unwrap_or(&[])
            .iter()
            .cloned()
            .chain(std::iter::once(stage.name.clone()))
            .collect::<Vec<_>>()
            .join(" -> ");
        diagnostics.push(
            Diagnostic::error(Code::DslStageCycle, format!("stage cycle: {cycle}"))
                .with_span(perform.stage_span.clone())
                .with_help("a stage cannot perform itself, directly or through another stage"),
        );
        return;
    }
    if stack.len() >= 64 {
        diagnostics.push(
            Diagnostic::error(
                Code::DslStageDepth,
                format!("stage nesting is deeper than 64 levels at '{}'", stage.name),
            )
            .with_span(perform.stage_span.clone())
            .with_help("flatten the stage composition or split it into separate Acts"),
        );
        return;
    }

    let combined = |platform: Platform| caller_applies(platform) && stage_applies(stage, platform);
    let stage_params: Vec<&str> = stage.params.iter().map(|p| p.name.as_str()).collect();
    validate_stage_body(
        stage,
        workspace,
        platforms,
        declared_vars,
        &stage_params,
        example_names,
        &combined,
        stack,
        reported_selectors,
        reported_not_applicable,
        diagnostics,
    );
}

fn stage_applies(stage: &StageDecl, platform: Platform) -> bool {
    stage.platforms.is_empty()
        || stage
            .platforms
            .iter()
            .filter_map(|named| named.name.parse::<Platform>().ok())
            .any(|p| p == platform)
}

#[allow(clippy::too_many_arguments)]
fn validate_stage_body(
    stage: &StageDecl,
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    declared_vars: &[&str],
    params: &[&str],
    example_names: &[&str],
    applies: &dyn Fn(Platform) -> bool,
    stack: &mut Vec<String>,
    reported_selectors: &mut HashSet<(String, Platform)>,
    reported_not_applicable: &mut HashSet<(String, Platform)>,
    diagnostics: &mut Diagnostics,
) {
    stack.push(stage.name.clone());
    if stage.items.is_empty() {
        for block in &stage.scenes {
            validate_scene_block(
                block,
                workspace,
                platforms,
                declared_vars,
                params,
                example_names,
                applies,
                reported_selectors,
                reported_not_applicable,
                diagnostics,
            );
        }
    } else {
        for item in &stage.items {
            match item {
                StageItem::Scene(block) => validate_scene_block(
                    block,
                    workspace,
                    platforms,
                    declared_vars,
                    params,
                    example_names,
                    applies,
                    reported_selectors,
                    reported_not_applicable,
                    diagnostics,
                ),
                StageItem::Perform(nested) => validate_perform(
                    nested,
                    workspace,
                    platforms,
                    declared_vars,
                    params,
                    example_names,
                    applies,
                    stack,
                    reported_selectors,
                    reported_not_applicable,
                    diagnostics,
                ),
            }
        }
    }
    stack.pop();
}

#[allow(clippy::too_many_arguments)]
fn validate_scene_block(
    block: &SceneBlock,
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    declared_vars: &[&str],
    params: &[&str],
    example_names: &[&str],
    applies: &dyn Fn(Platform) -> bool,
    reported_selectors: &mut HashSet<(String, Platform)>,
    reported_not_applicable: &mut HashSet<(String, Platform)>,
    diagnostics: &mut Diagnostics,
) {
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
            check_string_refs(string, declared_vars, params, example_names, diagnostics);
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
            applies,
            reported_selectors,
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

/// `IMP-DSL-014` for undefined variables, plus the parameter/example
/// equivalents. `params`/`example_names` are the names visible in the current
/// stage/act scope; pass empty slices where those references are not allowed.
fn check_string_refs(
    string: &ParsedString,
    declared: &[&str],
    params: &[&str],
    example_names: &[&str],
    diagnostics: &mut Diagnostics,
) {
    for part in &string.parts {
        match part {
            Part::Var { name, span } => {
                if !declared.contains(&name.as_str()) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::DslUndefinedVariable,
                            format!("undefined variable '{name}'"),
                        )
                        .with_span(span.clone())
                        .with_help(
                            "declare it with: Var: NAME = 'value' (or pass --var NAME=value)",
                        ),
                    );
                }
            }
            Part::Param { name, span } => {
                if !params.contains(&name.as_str()) {
                    diagnostics.push(
                        Diagnostic::error(
                            Code::DslUnknownParameter,
                            format!("unknown parameter '{name}'"),
                        )
                        .with_span(span.clone())
                        .with_help(if params.is_empty() {
                            "this scope declares no parameters".to_string()
                        } else {
                            format!("parameters in scope: {}", params.join(", "))
                        }),
                    );
                }
            }
            Part::Example { name, span } => {
                if !example_names.contains(&name.as_str()) {
                    let mut diag = Diagnostic::error(
                        Code::DslUnknownExampleColumn,
                        format!("unknown example column '{name}'"),
                    )
                    .with_span(span.clone());
                    if example_names.is_empty() {
                        diag = diag.with_help("add an Examples: table to the Act");
                    } else {
                        diag = diag.with_help(format!(
                            "example columns in scope: {}",
                            example_names.join(", ")
                        ));
                    }
                    diagnostics.push(diag);
                }
            }
            Part::Env { .. } | Part::Literal(_) => {}
        }
    }
}

fn check_variables(string: &ParsedString, declared: &[&str], diagnostics: &mut Diagnostics) {
    check_string_refs(string, declared, &[], &[], diagnostics);
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

/// Workspace-wide lints from TASK-2 section 8.7: unused declarations,
/// duplicate `Var:` names, unused `Examples:` columns, Acts without
/// `Platforms:` that cannot run everywhere, and required stage parameters
/// that no call site supplies.
fn lint_workspace(
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    diagnostics: &mut Diagnostics,
) {
    let mut used_scenes: HashSet<String> = HashSet::new();
    let mut used_pieces: HashSet<(String, String)> = HashSet::new();
    let mut called_stages: HashSet<String> = HashSet::new();
    for play in &workspace.plays {
        for act in &play.acts {
            for stage in &act.stages {
                collect_stage_usage(
                    stage,
                    &mut used_scenes,
                    &mut used_pieces,
                    &mut called_stages,
                );
            }
            for perform in &act.performs {
                called_stages.insert(perform.stage_name.clone());
            }
        }
    }
    for theatre in &workspace.theatres {
        for stage in &theatre.stages {
            collect_stage_usage(
                stage,
                &mut used_scenes,
                &mut used_pieces,
                &mut called_stages,
            );
        }
    }

    for scene in &workspace.scenes {
        if !used_scenes.contains(&scene.name) {
            diagnostics.push(
                Diagnostic::warning(
                    Code::DslLintUnusedScene,
                    format!("scene '{}' is never used by a stage", scene.name),
                )
                .with_span(scene.name_span.clone())
                .with_help("add a `Scene:` block to a stage, or remove the scene file"),
            );
            continue;
        }
        for piece in &scene.pieces {
            if !used_pieces.contains(&(scene.name.clone(), piece.name.clone())) {
                diagnostics.push(
                    Diagnostic::warning(
                        Code::DslLintUnusedPiece,
                        format!(
                            "piece '{}' in scene '{}' is never used by a step",
                            piece.name, scene.name
                        ),
                    )
                    .with_span(piece.name_span.clone())
                    .with_help("reference it from an Action or Check, or remove it"),
                );
            }
        }
    }

    // `Perform:` may use a theatre-qualified name (`Theatre.Stage`), while
    // reachability and the unused lints use the declaration name. Normalize
    // through the workspace resolver (which also honours exact dotted stage
    // names) before walking the call graph.
    let canonical_stage = |name: &str| {
        workspace
            .stage(name)
            .map_or_else(|| name.to_string(), |stage| stage.name.clone())
    };
    let mut reachable: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = called_stages
        .iter()
        .map(|name| canonical_stage(name))
        .collect();
    while let Some(name) = stack.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(stage) = workspace.stage(&name) else {
            continue;
        };
        for item in &stage.items {
            if let StageItem::Perform(perform) = item {
                stack.push(canonical_stage(&perform.stage_name));
            }
        }
    }
    for theatre in &workspace.theatres {
        if !theatre
            .stages
            .iter()
            .any(|stage| reachable.contains(&stage.name))
        {
            diagnostics.push(
                Diagnostic::warning(
                    Code::DslLintUnusedTheatre,
                    format!(
                        "theatre '{}' has no stage performed by an Act",
                        theatre.name
                    ),
                )
                .with_span(theatre.name_span.clone())
                .with_help("reference one of its stages with Perform: or remove the theatre"),
            );
        }
        for stage in &theatre.stages {
            if !reachable.contains(&stage.name) {
                diagnostics.push(
                    Diagnostic::warning(
                        Code::DslLintUnusedStage,
                        format!("stage '{}' is never performed", stage.name),
                    )
                    .with_span(stage.name_span.clone())
                    .with_help("call it from an Act or another stage with Perform:"),
                );
            }
        }
    }

    let mut var_refs: HashSet<String> = HashSet::new();
    let mut example_refs: HashSet<String> = HashSet::new();
    for play in &workspace.plays {
        let mut seen: HashSet<&str> = HashSet::new();
        for decl in &play.var_decls {
            if !seen.insert(decl.name.as_str()) {
                diagnostics.push(
                    Diagnostic::warning(
                        Code::DslLintShadowedVar,
                        format!("variable '{}' is declared more than once", decl.name),
                    )
                    .with_span(decl.name_span.clone())
                    .with_help("the last declaration wins; rename or remove one"),
                );
            }
            collect_string_refs(&decl.value, &mut var_refs, &mut example_refs);
        }
        for act in &play.acts {
            for param in &act.params {
                if let Some(default) = &param.default {
                    collect_string_refs(default, &mut var_refs, &mut example_refs);
                }
            }
            for perform in &act.performs {
                collect_perform_refs(perform, &mut var_refs, &mut example_refs);
            }
            for stage in &act.stages {
                collect_stage_refs(stage, &mut var_refs, &mut example_refs);
            }
            lint_examples(act, workspace, diagnostics);
        }
    }
    for theatre in &workspace.theatres {
        for stage in &theatre.stages {
            collect_stage_refs(stage, &mut var_refs, &mut example_refs);
        }
    }
    for play in &workspace.plays {
        for decl in &play.var_decls {
            if !var_refs.contains(&decl.name) {
                diagnostics.push(
                    Diagnostic::warning(
                        Code::DslLintUnusedVar,
                        format!("variable '{}' is never referenced", decl.name),
                    )
                    .with_span(decl.name_span.clone())
                    .with_help("reference it as ${var.NAME}, or remove it"),
                );
            }
        }
    }

    lint_act_platform_coverage(workspace, platforms, diagnostics);
    lint_required_stage_parameters(workspace, diagnostics);
}

/// `IMP-DSL-049`: an Act without `Platforms:` is planned for every configured
/// device. If a piece it uses has no selector for one of those platforms, the
/// step will fail with `IMP-DSL-012`; warn early and list what is missing so
/// the author can either restrict the Act or add the selectors.
fn lint_act_platform_coverage(
    workspace: &Workspace,
    platforms: &[(Platform, &str)],
    diagnostics: &mut Diagnostics,
) {
    if platforms.is_empty() {
        return;
    }
    for play in &workspace.plays {
        for act in &play.acts {
            if !act.platforms.is_empty() {
                continue;
            }
            let needed = act_piece_requirements(act, workspace);
            if needed.is_empty() {
                continue;
            }
            for (platform, device) in platforms {
                let missing: Vec<&(String, String)> = needed
                    .iter()
                    .filter(|(scene_name, piece_name)| {
                        let Some(scene) = workspace.scene(scene_name) else {
                            return false;
                        };
                        let Some(piece) = scene.piece(piece_name) else {
                            return false;
                        };
                        !piece
                            .selectors
                            .iter()
                            .any(|selector| platform.selector_chain().contains(&selector.key))
                    })
                    .collect();
                if missing.is_empty() {
                    continue;
                }
                let listed = missing
                    .iter()
                    .take(5)
                    .map(|(scene, piece)| format!("{piece} ({scene})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = missing.len().saturating_sub(5);
                diagnostics.push(
                    Diagnostic::warning(
                        Code::DslLintActPlatformCoverage,
                        format!(
                            "Act '{}' has no Platforms: but {} piece(s) have no selector for platform '{}' (device '{}'): {}{}",
                            act.name,
                            missing.len(),
                            platform,
                            device,
                            listed,
                            if more > 0 {
                                format!(" and {more} more")
                            } else {
                                String::new()
                            }
                        ),
                    )
                    .with_span(act.name_span.clone())
                    .with_help(
                        "add `Platforms:` to the Act, or add the missing selectors to the scene table",
                    ),
                );
            }
        }
    }
}

/// Every `(scene, piece)` an Act reaches through its stages, in first-use
/// order and deduplicated. Cycles are guarded with `seen_stages`; the cycle
/// error itself is reported by validation.
fn act_piece_requirements(act: &ActDecl, workspace: &Workspace) -> Vec<(String, String)> {
    let mut seen_stages = HashSet::new();
    let mut seen_pieces = HashSet::new();
    let mut needed = Vec::new();
    for perform in &act.performs {
        collect_perform_requirements(
            perform,
            workspace,
            &mut seen_stages,
            &mut seen_pieces,
            &mut needed,
        );
    }
    for stage in &act.stages {
        collect_stage_requirements(
            stage,
            workspace,
            &mut seen_stages,
            &mut seen_pieces,
            &mut needed,
        );
    }
    needed
}

fn collect_perform_requirements(
    perform: &PerformDecl,
    workspace: &Workspace,
    seen_stages: &mut HashSet<String>,
    seen_pieces: &mut HashSet<(String, String)>,
    needed: &mut Vec<(String, String)>,
) {
    let Some(stage) = workspace.stage(&perform.stage_name) else {
        return;
    };
    collect_stage_requirements(stage, workspace, seen_stages, seen_pieces, needed);
}

fn collect_stage_requirements(
    stage: &StageDecl,
    workspace: &Workspace,
    seen_stages: &mut HashSet<String>,
    seen_pieces: &mut HashSet<(String, String)>,
    needed: &mut Vec<(String, String)>,
) {
    if !seen_stages.insert(stage.name.clone()) {
        return;
    }
    let blocks = if stage.items.is_empty() {
        stage.scenes.iter().collect::<Vec<_>>()
    } else {
        stage
            .items
            .iter()
            .filter_map(|item| match item {
                StageItem::Scene(block) => Some(block),
                StageItem::Perform(_) => None,
            })
            .collect()
    };
    for block in blocks {
        for step in &block.steps {
            let Some(piece) = step_piece(step) else {
                continue;
            };
            let key = (block.scene_name.clone(), piece.name.clone());
            if seen_pieces.insert(key.clone()) {
                needed.push(key);
            }
        }
    }
    for item in &stage.items {
        if let StageItem::Perform(perform) = item {
            collect_perform_requirements(perform, workspace, seen_stages, seen_pieces, needed);
        }
    }
}

/// `IMP-DSL-050`: a required stage parameter is omitted by every `Perform:`
/// call that reaches the stage. The first omission is already an
/// `IMP-DSL-037` error; this lint points at the declaration so the author can
/// add a default or fix every call site.
fn lint_required_stage_parameters(workspace: &Workspace, diagnostics: &mut Diagnostics) {
    let mut calls: std::collections::HashMap<String, Vec<&PerformDecl>> =
        std::collections::HashMap::new();
    for perform in all_performs(workspace) {
        if let Some(stage) = workspace.stage(&perform.stage_name) {
            calls.entry(stage.name.clone()).or_default().push(perform);
        }
    }
    for theatre in &workspace.theatres {
        for stage in &theatre.stages {
            for param in &stage.params {
                if param.default.is_some() {
                    continue;
                }
                let Some(sites) = calls.get(&stage.name) else {
                    continue;
                };
                if sites.is_empty()
                    || sites
                        .iter()
                        .any(|call| call.args.iter().any(|arg| arg.name == param.name))
                {
                    continue;
                }
                diagnostics.push(
                    Diagnostic::warning(
                        Code::DslLintRequiredParamNeverSupplied,
                        format!(
                            "required parameter '{}' of stage '{}' is never supplied",
                            param.name, stage.name
                        ),
                    )
                    .with_span(param.name_span.clone())
                    .with_help(
                        "pass it from every `Perform:` call, or give it a default in the Stage header",
                    ),
                );
            }
        }
    }
}

fn all_performs(workspace: &Workspace) -> Vec<&PerformDecl> {
    let mut out: Vec<&PerformDecl> = Vec::new();
    for play in &workspace.plays {
        for act in &play.acts {
            out.extend(act.performs.iter());
            for stage in &act.stages {
                collect_stage_performs(stage, &mut out);
            }
        }
    }
    for theatre in &workspace.theatres {
        for stage in &theatre.stages {
            collect_stage_performs(stage, &mut out);
        }
    }
    out
}

fn collect_stage_performs<'a>(stage: &'a StageDecl, out: &mut Vec<&'a PerformDecl>) {
    for item in &stage.items {
        if let StageItem::Perform(perform) = item {
            out.push(perform);
        }
    }
}

fn collect_stage_usage(
    stage: &StageDecl,
    used_scenes: &mut HashSet<String>,
    used_pieces: &mut HashSet<(String, String)>,
    called_stages: &mut HashSet<String>,
) {
    if stage.items.is_empty() {
        for block in &stage.scenes {
            collect_block_usage(block, used_scenes, used_pieces);
        }
    } else {
        for item in &stage.items {
            match item {
                StageItem::Scene(block) => {
                    collect_block_usage(block, used_scenes, used_pieces);
                }
                StageItem::Perform(perform) => {
                    called_stages.insert(perform.stage_name.clone());
                }
            }
        }
    }
}

fn collect_block_usage(
    block: &SceneBlock,
    used_scenes: &mut HashSet<String>,
    used_pieces: &mut HashSet<(String, String)>,
) {
    used_scenes.insert(block.scene_name.clone());
    for step in &block.steps {
        if let Some(piece) = step_piece(step) {
            used_pieces.insert((block.scene_name.clone(), piece.name.clone()));
        }
    }
}

fn collect_perform_refs(
    perform: &PerformDecl,
    vars: &mut HashSet<String>,
    examples: &mut HashSet<String>,
) {
    for arg in &perform.args {
        collect_string_refs(&arg.value, vars, examples);
    }
}

fn collect_stage_refs(
    stage: &StageDecl,
    vars: &mut HashSet<String>,
    examples: &mut HashSet<String>,
) {
    for param in &stage.params {
        if let Some(default) = &param.default {
            collect_string_refs(default, vars, examples);
        }
    }
    if stage.items.is_empty() {
        for block in &stage.scenes {
            for step in &block.steps {
                for string in step_strings(step) {
                    collect_string_refs(string, vars, examples);
                }
            }
        }
    } else {
        for item in &stage.items {
            match item {
                StageItem::Scene(block) => {
                    for step in &block.steps {
                        for string in step_strings(step) {
                            collect_string_refs(string, vars, examples);
                        }
                    }
                }
                StageItem::Perform(perform) => {
                    collect_perform_refs(perform, vars, examples);
                }
            }
        }
    }
}

fn collect_string_refs(
    string: &ParsedString,
    vars: &mut HashSet<String>,
    examples: &mut HashSet<String>,
) {
    for part in &string.parts {
        match part {
            Part::Var { name, .. } => {
                vars.insert(name.clone());
            }
            Part::Example { name, .. } => {
                examples.insert(name.clone());
            }
            _ => {}
        }
    }
}

fn lint_examples(act: &ActDecl, workspace: &Workspace, diagnostics: &mut Diagnostics) {
    let Some(table) = &act.examples else {
        return;
    };
    let mut used: HashSet<String> = act
        .params
        .iter()
        .filter(|param| table.columns.iter().any(|column| column.name == param.name))
        .map(|param| param.name.clone())
        .collect();
    for param in &act.params {
        if let Some(default) = &param.default {
            collect_example_refs(default, &mut used);
        }
    }
    let mut seen = HashSet::new();
    for perform in &act.performs {
        collect_perform_example_refs(perform, workspace, &mut seen, &mut used);
    }
    for stage in &act.stages {
        collect_stage_example_refs(stage, workspace, &mut seen, &mut used);
    }
    for column in &table.columns {
        if !used.contains(&column.name) {
            diagnostics.push(
                Diagnostic::warning(
                    Code::DslLintUnusedExampleColumn,
                    format!("Examples column '{}' is never used", column.name),
                )
                .with_span(column.span.clone())
                .with_help("reference it as ${example.NAME}, or remove the column"),
            );
        }
    }
}

fn collect_perform_example_refs(
    perform: &PerformDecl,
    workspace: &Workspace,
    seen: &mut HashSet<String>,
    used: &mut HashSet<String>,
) {
    for arg in &perform.args {
        collect_example_refs(&arg.value, used);
    }
    if !seen.insert(perform.stage_name.clone()) {
        return;
    }
    if let Some(stage) = workspace.stage(&perform.stage_name) {
        collect_stage_example_refs(stage, workspace, seen, used);
    }
}

fn collect_stage_example_refs(
    stage: &StageDecl,
    workspace: &Workspace,
    seen: &mut HashSet<String>,
    used: &mut HashSet<String>,
) {
    for param in &stage.params {
        if let Some(default) = &param.default {
            collect_example_refs(default, used);
        }
    }
    if stage.items.is_empty() {
        for block in &stage.scenes {
            for step in &block.steps {
                for string in step_strings(step) {
                    collect_example_refs(string, used);
                }
            }
        }
    } else {
        for item in &stage.items {
            match item {
                StageItem::Scene(block) => {
                    for step in &block.steps {
                        for string in step_strings(step) {
                            collect_example_refs(string, used);
                        }
                    }
                }
                StageItem::Perform(perform) => {
                    collect_perform_example_refs(perform, workspace, seen, used);
                }
            }
        }
    }
}

fn collect_example_refs(string: &ParsedString, used: &mut HashSet<String>) {
    for part in &string.parts {
        if let Part::Example { name, .. } = part {
            used.insert(name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::parser::{parse_play, parse_scene, parse_theatre};

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

    fn theatre(source: &str, name: &str) -> crate::ast::TheatreFile {
        parse_theatre(source, Path::new(name))
            .ast
            .expect("theatre parses")
    }

    fn workspace(scenes: Vec<SceneFile>, plays: Vec<PlayFile>) -> Workspace {
        Workspace {
            scenes,
            theatres: Vec::new(),
            plays,
        }
    }

    fn workspace_with_theatre(
        scenes: Vec<SceneFile>,
        theatres: Vec<crate::ast::TheatreFile>,
        plays: Vec<PlayFile>,
    ) -> Workspace {
        Workspace {
            scenes,
            theatres,
            plays,
        }
    }

    fn codes(diagnostics: &Diagnostics) -> Vec<&'static str> {
        diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    fn error_codes(diagnostics: &Diagnostics) -> Vec<&'static str> {
        diagnostics.errors().map(|d| d.code.as_str()).collect()
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
    fn theatrical_workspace_validates_params_examples_and_scene_steps() {
        let scenes = vec![scene(
            "Scene: LoginScreen\nPlatforms: cross-platform\n\n| Piece | web | android | ios |\n|---|---|---|---|\n| LoginTitle | h1.title | id:login_title | accessibilityId:login_title |\n| UsernameInput | #username | id:username | accessibilityId:username |\n| SubmitButton | #submit | id:submit | accessibilityId:submit |\n",
            "LoginScreen.scene",
        )];
        let theatres = vec![theatre(
            "Theatre: Authentication\n\nStage: Login(user, password, remember = 'false')\n    Scene: LoginScreen\n        Check: LoginTitle\n        Action: '${param.user}' => UsernameInput\n        Action: '${param.password}' => UsernameInput\n        Action: click => SubmitButton\n",
            "auth.theatre",
        )];
        let plays = vec![play(
            "Play: Auth\n\nAct: Login\n    Examples:\n        | user | password |\n        | alice | wrong |\n    Perform: Login(user = '${example.user}', password = '${example.password}')\n",
            "auth.play",
        )];
        let devices = vec![
            DeviceRef::new("flagship_android", Platform::Android),
            DeviceRef::new("test_iphone", Platform::Ios),
        ];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, plays), &devices);
        assert!(
            !diagnostics.has_errors(),
            "{:?}",
            diagnostics.errors().collect::<Vec<_>>()
        );
    }

    #[test]
    fn theatrical_unknown_param_and_example_are_reported() {
        let scenes = vec![scene(
            "Scene: S\nPlatforms: cross-platform\n\n| Piece | android |\n|---|---|\n| Field | id:f |\n",
            "S.scene",
        )];
        let theatres = vec![theatre(
            "Theatre: T\n\nStage: Fill(user)\n    Scene: S\n        Action: '${param.nope}' => Field\n",
            "t.theatre",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n    Examples:\n        | user |\n        | alice |\n    Perform: Fill(user = '${example.nope}')\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, plays), &android());
        let codes = codes(&diagnostics);
        assert!(codes.contains(&"IMP-DSL-036"), "{codes:?}");
        assert!(codes.contains(&"IMP-DSL-051"), "{codes:?}");
    }

    #[test]
    fn theatrical_missing_argument_and_unknown_stage_are_reported() {
        let scenes = vec![scene(
            "Scene: S\nPlatforms: cross-platform\n\n| Piece | android |\n|---|---|\n| Field | id:f |\n",
            "S.scene",
        )];
        let theatres = vec![theatre(
            "Theatre: T\n\nStage: Fill(user)\n    Scene: S\n        Check: Field\n",
            "t.theatre",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\n    Perform: Fill\n    Perform: Missing\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, plays), &android());
        let codes = codes(&diagnostics);
        assert!(codes.contains(&"IMP-DSL-037"), "{codes:?}");
        assert!(codes.contains(&"IMP-DSL-031"), "{codes:?}");
    }

    #[test]
    fn workspace_lints_report_unused_declarations_and_columns() {
        let scenes = vec![
            scene(
                "Scene: UsedScene\nPlatform: cross-platform\n\nPieces:\n    UsedPiece: android=\"id:used\"\n    UnusedPiece: android=\"id:unused\"\n",
                "UsedScene.scene",
            ),
            scene(
                "Scene: UnusedScene\nPlatform: cross-platform\n\nPieces:\n    Other: android=\"id:other\"\n",
                "UnusedScene.scene",
            ),
        ];
        let theatres = vec![
            theatre(
                "Theatre: T\n\nStage: UsedStage(user)\n    Scene: UsedScene\n        Action: type '${var.USED}' => UsedPiece\n        Check: UsedPiece\n",
                "T.theatre",
            ),
            theatre("Theatre: Other\n\nStage: Never\n", "Other.theatre"),
        ];
        let plays = vec![play(
            "Play: P\nVar: USED = 'x'\nVar: DEAD = 'y'\nVar: USED = 'z'\n\nAct: A\n    Examples:\n        | user | unused |\n        | alice | x |\n    Perform: UsedStage(user = '${example.user}')\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, plays), &android());
        assert!(diagnostics.errors().next().is_none(), "{diagnostics:?}");
        let codes = codes(&diagnostics);
        for expected in [
            "IMP-DSL-053",
            "IMP-DSL-054",
            "IMP-DSL-055",
            "IMP-DSL-056",
            "IMP-DSL-057",
            "IMP-DSL-058",
        ] {
            assert!(codes.contains(&expected), "missing {expected}: {codes:?}");
        }
    }

    #[test]
    fn act_without_platforms_warns_about_missing_piece_selectors() {
        let scenes = vec![scene(
            "Scene: S\nPlatforms: cross-platform\n\n| Piece | android |\n|---|---|\n| Field | id:f |\n",
            "S.scene",
        )];
        let theatres = vec![theatre(
            "Theatre: T\n\nStage: Fill\n    Scene: S\n        Check: Field\n",
            "t.theatre",
        )];
        let plays = vec![play("Play: P\n\nAct: A\n    Perform: Fill\n", "p.play")];
        let devices = vec![
            DeviceRef::new("pixel", Platform::Android),
            DeviceRef::new("iphone", Platform::Ios),
        ];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, plays), &devices);
        let warning = diagnostics
            .warnings()
            .find(|diagnostic| diagnostic.code == Code::DslLintActPlatformCoverage)
            .unwrap_or_else(|| panic!("missing IMP-DSL-049: {diagnostics:?}"));
        assert!(warning.message.contains("ios"), "{}", warning.message);
        assert!(warning.message.contains("Field (S)"), "{}", warning.message);
        assert!(
            warning
                .help
                .as_deref()
                .is_some_and(|help| help.contains("Platforms:")),
            "{warning:?}"
        );
    }

    #[test]
    fn act_platform_coverage_lint_is_quiet_when_the_act_is_restricted() {
        let scenes = vec![scene(
            "Scene: S\nPlatforms: cross-platform\n\n| Piece | android |\n|---|---|\n| Field | id:f |\n",
            "S.scene",
        )];
        let theatres = vec![theatre(
            "Theatre: T\n\nStage: Fill\n    Scene: S\n        Check: Field\n",
            "t.theatre",
        )];
        let plays = vec![play(
            "Play: P\n\nAct: A\nPlatforms: android\n    Perform: Fill\n",
            "p.play",
        )];
        let devices = vec![
            DeviceRef::new("pixel", Platform::Android),
            DeviceRef::new("iphone", Platform::Ios),
        ];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, plays), &devices);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.code == Code::DslLintActPlatformCoverage),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn required_stage_parameter_never_supplied_is_linted() {
        let scenes = vec![scene(
            "Scene: S\nPlatforms: cross-platform\n\n| Piece | android |\n|---|---|\n| Field | id:f |\n",
            "S.scene",
        )];
        let theatres = vec![theatre(
            "Theatre: T\n\nStage: Fill(user)\n    Scene: S\n        Check: Field\n",
            "t.theatre",
        )];
        let bad = vec![play("Play: P\n\nAct: A\n    Perform: T.Fill\n", "p.play")];
        let diagnostics = validate(
            &workspace_with_theatre(scenes.clone(), theatres.clone(), bad),
            &android(),
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == Code::DslMissingStageArgument),
            "{diagnostics:?}"
        );
        let warning = diagnostics
            .warnings()
            .find(|d| d.code == Code::DslLintRequiredParamNeverSupplied)
            .unwrap_or_else(|| panic!("missing IMP-DSL-050: {diagnostics:?}"));
        assert!(warning.message.contains("user"), "{}", warning.message);

        let good = vec![play(
            "Play: P\n\nAct: A\n    Perform: T.Fill(user = 'alice')\n",
            "p.play",
        )];
        let diagnostics = validate(&workspace_with_theatre(scenes, theatres, good), &android());
        assert!(diagnostics.errors().next().is_none(), "{diagnostics:?}");
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.code == Code::DslLintRequiredParamNeverSupplied),
            "{diagnostics:?}"
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
        assert_eq!(error_codes(&diagnostics), vec!["IMP-DSL-008"], "{errors:?}");
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
        assert_eq!(error_codes(&diagnostics), vec!["IMP-DSL-009"]);
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
        // android does not: IMP-DSL-012, plus the actionable IMP-DSL-049 lint.
        let bad = validate(&workspace(scenes, plays), &android());
        assert_eq!(error_codes(&bad), vec!["IMP-DSL-012"]);
        assert!(
            bad.iter()
                .any(|diagnostic| diagnostic.code == Code::DslLintActPlatformCoverage),
            "{bad:?}"
        );
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
