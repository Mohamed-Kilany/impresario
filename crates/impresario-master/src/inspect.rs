//! Live probe inspection (`impresario probe`, TASK-3 sections 3.3 and 10).
//!
//! A probe is executed as a synthetic one-step plan through [`exec::execute`],
//! so app reset/launch, transport preparation, retries, screenshots,
//! canonicalisation and redaction are the same code paths a `run` uses. The
//! finished run directory receives `report.json` plus an `evidence.json`
//! bundle that the MCP artifact tools expose.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use impresario_core::apps::{self, AppConfig};
use impresario_core::command::{Command, ProbeCommand};
use impresario_core::config::{self, AppReset, DeviceConfig, Reporter};
use impresario_core::diag::{Diagnostic, Diagnostics};
use impresario_core::platforms::Platform;
use impresario_core::probe::{Claim, Confidence, ProbeKind, ProbeResult, Verdict};
use impresario_core::selectors::{self, Selector};
use impresario_core::span::Span;
use impresario_core::workspace;
use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::cli::{self, EXIT_FAILURE, EXIT_INFRA, EXIT_INTERRUPTED, EXIT_OK};
use crate::exec::{self, ExecOptions};
use crate::plan::{
    DevicePlan, ExecutionPlan, PlannedAct, PlannedPlay, PlannedStage, PlannedStep, PlannedWork,
};
use crate::report::{RunReport, RunStatus};
use crate::reporters;
use crate::time_fmt;

/// One live inspection request, shared by the CLI and the MCP tool.
pub struct ProbeRequest<'a> {
    pub base: &'a Path,
    pub config_path: &'a Path,
    pub devices_path: &'a Path,
    pub kind: ProbeKind,
    pub device: &'a str,
    pub app: Option<&'a str>,
    pub piece: Option<&'a str>,
    pub selector: Option<&'a str>,
    pub claims: &'a [String],
    pub confidence: Option<Confidence>,
}

/// What one inspection produced.
pub struct ProbeOutcome {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub report: RunReport,
    /// `None` when the executor failed before returning probe evidence.
    pub probe: Option<ProbeResult>,
    pub exit_code: i32,
    pub poll_interval_ms: u64,
    pub kind: ProbeKind,
    pub device: String,
    pub platform: Platform,
    pub selector: Selector,
    pub selector_label: String,
    pub claims: Vec<Claim>,
}

/// Run one inspection in a fresh runtime (used by the MCP handler).
pub fn probe(request: &ProbeRequest<'_>) -> Result<ProbeOutcome, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("cannot start the async runtime: {err}"))?;
    runtime.block_on(probe_async(request, CancellationToken::new()))
}

/// CLI entry point: print one result, or repeat until Ctrl-C with `--watch`.
pub fn run_command(
    base: &Path,
    config_path: &Path,
    devices_path: &Path,
    args: &crate::cli::ProbeArgs,
) -> i32 {
    let request = ProbeRequest {
        base,
        config_path,
        devices_path,
        kind: args.kind.kind(),
        device: &args.device,
        app: args.app.as_deref(),
        piece: args.piece.as_deref(),
        selector: args.selector.as_deref(),
        claims: &args.claim,
        confidence: args.confidence.map(crate::cli::ConfidenceArg::confidence),
    };
    run_cli(&request, args.watch)
}

fn run_cli(request: &ProbeRequest<'_>, watch: bool) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("probe: cannot start the async runtime: {err}");
            return EXIT_INFRA;
        }
    };
    runtime.block_on(async {
        let shutdown = CancellationToken::new();
        let watcher = tokio::spawn(watch_interrupts(shutdown.clone()));
        let code = loop {
            if shutdown.is_cancelled() {
                break EXIT_INTERRUPTED;
            }
            match probe_async(request, shutdown.child_token()).await {
                Ok(outcome) => {
                    print_outcome(&outcome, watch);
                    if !watch
                        || outcome.exit_code == EXIT_INTERRUPTED
                        || outcome.exit_code == EXIT_INFRA
                    {
                        break outcome.exit_code;
                    }
                    let pause = Duration::from_millis(outcome.poll_interval_ms.max(200));
                    tokio::select! {
                        _ = shutdown.cancelled() => break EXIT_INTERRUPTED,
                        _ = tokio::time::sleep(pause) => {}
                    }
                }
                Err(message) => {
                    eprintln!("probe: {message}");
                    break EXIT_INFRA;
                }
            }
        };
        watcher.abort();
        code
    })
}

/// First Ctrl-C cancels the in-flight probe; a second one exits immediately.
async fn watch_interrupts(shutdown: CancellationToken) {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        if shutdown.is_cancelled() {
            std::process::exit(EXIT_INTERRUPTED);
        }
        eprintln!("probe: interrupted; cancelling the in-flight probe (Ctrl-C again to exit now)");
        shutdown.cancel();
    }
}

async fn probe_async(
    request: &ProbeRequest<'_>,
    shutdown: CancellationToken,
) -> Result<ProbeOutcome, String> {
    let config_outcome = cli::load_effective_config(request.base, request.config_path);
    let Some(mut config) = config_outcome.config else {
        return Err(format!(
            "configuration errors in {}:\n{}",
            request.config_path.display(),
            diagnostics_text(&config_outcome.diagnostics)
        ));
    };
    let devices_outcome = config::load_devices_merged(request.devices_path);
    let Some(device_list) = devices_outcome.config else {
        return Err(format!(
            "configuration errors in {}:\n{}",
            request.devices_path.display(),
            diagnostics_text(&devices_outcome.diagnostics)
        ));
    };
    let apps_path = request.base.join("impresario.apps.config");
    let apps_outcome = apps::load_apps_merged(&apps_path);
    let Some(app_list) = apps_outcome.config else {
        return Err(format!(
            "configuration errors in {}:\n{}",
            apps_path.display(),
            diagnostics_text(&apps_outcome.diagnostics)
        ));
    };

    let device = resolve_device(request, &device_list.devices, &device_list.groups)?;
    let app = resolve_app(request.app, &app_list.apps, &device)?;
    if app.is_none() {
        // Nothing to launch or wipe: do not require APP_CONTROL just because
        // the workspace defaults to per-act resets.
        config.app_reset = AppReset::Never;
    }
    let (selector, selector_label) = resolve_selector(request, &config, &device)?;
    let claims = build_claims(request.kind, request.claims)?;

    let mut command = ProbeCommand::new(request.kind, selector.clone(), claims.clone());
    if let Some(confidence) = request.confidence {
        command.min_confidence = confidence;
    }
    let timeout_ms = u32::try_from(
        config
            .timeout_default_ms
            .max(u64::from(command.window_ms) + 1_000),
    )
    .unwrap_or(u32::MAX);
    let span = Span::new("<probe>", 1, 1, 1);
    let step = PlannedStep {
        span: span.clone(),
        source_text: format!("probe {} => {selector_label}", request.kind.as_str()),
        label: format!("probe {}", request.kind.as_str()),
        timeout_ms,
        command: Some(Command::Probe(command)),
        work: PlannedWork::Wire,
    };
    let plan = ExecutionPlan {
        devices: vec![DevicePlan {
            device: device.clone(),
            app,
            plays: vec![PlannedPlay {
                name: "Probe".to_string(),
                file: String::new(),
                acts: vec![PlannedAct {
                    name: format!("Probe {}", request.kind.as_str()),
                    tags: Vec::new(),
                    example: BTreeMap::new(),
                    stages: vec![PlannedStage {
                        name: "Probe".to_string(),
                        span,
                        steps: vec![step],
                    }],
                }],
            }],
        }],
        // A raw selector and claim ids carry no `${env.*}` values.
        secrets: Vec::new(),
    };

    let report_root = request.base.join(&config.report_output);
    if let Some(diagnostic) = config::check_report_output(&report_root) {
        return Err(diagnostic.one_line());
    }
    let run_id = time_fmt::run_id(std::time::SystemTime::now());
    let options = ExecOptions {
        config: config.clone(),
        run_id: run_id.clone(),
        report_dir: report_root.clone(),
        // One observation per invocation: a retry would hide a flaky signal.
        retries: 0,
        fail_fast: false,
        token: cli::agent_token(),
        shutdown,
        master_version: env!("CARGO_PKG_VERSION").to_string(),
        session_options: impresario_clients::agent_client::SessionOptions::default(),
        events: None,
    };
    let executed = exec::execute(&plan, &options).await;
    let run_dir = report_root.join(&run_id);
    let reporter_errors = reporters::write(&executed.report, &run_dir, &[Reporter::Json], false);
    if !reporter_errors.is_empty() {
        return Err(reporter_errors.join("; "));
    }

    let probe = find_probe(&executed.report);
    let exit_code = probe_exit_code(&executed.report, probe.as_ref());
    if let Some(probe) = &probe {
        let bundle = EvidenceBundle {
            schema_version: 1,
            run_id: &run_id,
            captured_at: time_fmt::rfc3339_utc(time_fmt::now_millis()),
            kind: request.kind.as_str(),
            device: &device.name,
            platform: device.platform.as_str(),
            selector: &selector,
            claims: &claims,
            result: probe,
        };
        let text = serde_json::to_string_pretty(&bundle)
            .map_err(|err| format!("cannot serialize evidence.json: {err}"))?;
        reporters::write_file(&run_dir, "evidence.json", text.as_bytes())?;
    }

    Ok(ProbeOutcome {
        run_id,
        run_dir,
        report: executed.report,
        probe,
        exit_code,
        poll_interval_ms: config.poll_interval_ms,
        kind: request.kind,
        device: device.name,
        platform: device.platform,
        selector,
        selector_label,
        claims,
    })
}

fn resolve_device(
    request: &ProbeRequest<'_>,
    devices: &[DeviceConfig],
    groups: &BTreeMap<String, Vec<String>>,
) -> Result<DeviceConfig, String> {
    let selected = cli::select_devices_with_groups(devices, groups, &[request.device.to_string()])?;
    match selected.as_slice() {
        [] => Err(format!(
            "device '{}' is not configured (see {})",
            request.device,
            request.devices_path.display()
        )),
        [device] => Ok(device.clone()),
        _ => Err(format!(
            "probe needs exactly one device; '{}' resolves to {} devices",
            request.device,
            selected.len()
        )),
    }
}

fn resolve_app(
    name: Option<&str>,
    apps: &[AppConfig],
    device: &DeviceConfig,
) -> Result<Option<AppConfig>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    let app = apps
        .iter()
        .find(|app| app.name == name)
        .ok_or_else(|| format!("unknown app '{name}'"))?;
    if app.platform(device.platform).is_none() {
        return Err(format!(
            "app '{name}' has no entry for platform '{}' (device '{}')",
            device.platform, device.name
        ));
    }
    Ok(Some(app.clone()))
}

fn resolve_selector(
    request: &ProbeRequest<'_>,
    config: &impresario_core::config::ImpresarioConfig,
    device: &DeviceConfig,
) -> Result<(Selector, String), String> {
    match (request.piece, request.selector) {
        (Some(_), Some(_)) => Err("use either --piece or --selector, not both".to_string()),
        (None, None) => Err("one of --piece Scene.Piece or --selector is required".to_string()),
        (Some(piece_ref), None) => {
            let (scene_name, piece_name) = piece_ref
                .split_once('.')
                .ok_or_else(|| format!("--piece must be Scene.Piece (got '{piece_ref}')"))?;
            if scene_name.is_empty() || piece_name.is_empty() {
                return Err(format!("--piece must be Scene.Piece (got '{piece_ref}')"));
            }
            let scenes = workspace::load_scenes(&request.base.join(&config.scenes_dir));
            if scenes.diagnostics.has_errors() {
                return Err(format!(
                    "workspace errors:\n{}",
                    diagnostics_text(&scenes.diagnostics)
                ));
            }
            let scene = scenes
                .workspace
                .scene(scene_name)
                .ok_or_else(|| format!("unknown scene '{scene_name}'"))?;
            let piece = scene
                .piece(piece_name)
                .ok_or_else(|| format!("unknown piece '{piece_name}' in scene '{scene_name}'"))?;
            let mut matched = None;
            for key in device.platform.selector_chain() {
                if let Some(selector) = piece.selectors.iter().find(|selector| selector.key == *key)
                {
                    matched = Some((*key, selector));
                    break;
                }
            }
            let (key, selector) = matched.ok_or_else(|| {
                format!(
                    "piece '{piece_name}' has no selector for platform '{}' (device '{}')",
                    device.platform, device.name
                )
            })?;
            let parsed = selectors::parse(key, &selector.raw, piece.index, None)
                .map_err(|diagnostic| diagnostic.one_line())?;
            Ok((parsed, format!("{scene_name}.{piece_name}")))
        }
        (None, Some(raw)) => {
            let parsed = selectors::parse(device.platform, raw, 0, None)
                .map_err(|diagnostic| diagnostic.one_line())?;
            Ok((parsed, raw.to_string()))
        }
    }
}

fn build_claims(kind: ProbeKind, raw_claims: &[String]) -> Result<Vec<Claim>, String> {
    let mut claims = Vec::new();
    if kind == ProbeKind::Showing {
        claims.push(Claim::new("showing=present"));
    }
    for raw in raw_claims {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("--claim cannot be empty".to_string());
        }
        let claim = if let Some(rest) = raw.strip_prefix("not ") {
            let id = rest.trim();
            if id.is_empty() {
                return Err("--claim 'not' needs a claim id".to_string());
            }
            Claim::negated(id)
        } else {
            Claim::new(raw)
        };
        if !claims
            .iter()
            .any(|existing| existing.id == claim.id && existing.negated == claim.negated)
        {
            claims.push(claim);
        }
    }
    Ok(claims)
}

/// Parse the MCP/CLI confidence vocabulary.
#[must_use]
pub fn parse_confidence(value: &str) -> Option<Confidence> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Some(Confidence::None),
        "inferred" => Some(Confidence::Inferred),
        "corroborated" => Some(Confidence::Corroborated),
        "authoritative" => Some(Confidence::Authoritative),
        _ => None,
    }
}

fn diagnostics_text(diagnostics: &Diagnostics) -> String {
    diagnostics
        .iter()
        .map(Diagnostic::one_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_probe(report: &RunReport) -> Option<ProbeResult> {
    let mut found = None;
    for device in &report.devices {
        for play in &device.plays {
            for act in &play.acts {
                for attempt in &act.attempts {
                    for stage in &attempt.stages {
                        for step in &stage.steps {
                            if let Some(probe) = &step.probe {
                                found = Some(probe.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    found
}

fn probe_exit_code(report: &RunReport, probe: Option<&ProbeResult>) -> i32 {
    if report.exit_code == EXIT_INTERRUPTED {
        return EXIT_INTERRUPTED;
    }
    if report.status == RunStatus::Error || report.exit_code == EXIT_INFRA {
        return EXIT_INFRA;
    }
    if report.exit_code == EXIT_FAILURE {
        return EXIT_FAILURE;
    }
    match probe.map(ProbeResult::overall) {
        Some(Verdict::Pass) => EXIT_OK,
        Some(Verdict::Fail | Verdict::Unverifiable) => EXIT_FAILURE,
        None => EXIT_INFRA,
    }
}

fn print_outcome(outcome: &ProbeOutcome, watch: bool) {
    if watch {
        println!("--- {} ---", time_fmt::rfc3339_utc(time_fmt::now_millis()));
    }
    println!(
        "probe {} on {} ({}) -> {}",
        outcome.kind.as_str(),
        outcome.device,
        outcome.platform,
        outcome.selector_label
    );
    println!("  run: {}", outcome.run_id);
    if let Some(probe) = &outcome.probe {
        if probe.verdicts.is_empty() {
            println!("  no claims; evidence only");
        }
        for verdict in &probe.verdicts {
            let reason = verdict
                .reason
                .as_deref()
                .map(|reason| format!(" · {reason}"))
                .unwrap_or_default();
            println!(
                "  {:<12} {} ({}){reason}",
                verdict.verdict.as_str(),
                verdict.claim,
                verdict.confidence.as_str()
            );
        }
        for evidence in &probe.evidence {
            let mut claims = evidence
                .supports
                .iter()
                .map(|claim| format!("+{claim}"))
                .collect::<Vec<_>>();
            claims.extend(evidence.contradicts.iter().map(|claim| format!("-{claim}")));
            let claims = if claims.is_empty() {
                String::new()
            } else {
                format!(" [{}]", claims.join(", "))
            };
            println!(
                "  - {} [{}] {}: {}{claims}",
                evidence.source.as_str(),
                evidence.class.as_str(),
                evidence.kind,
                evidence.observation.render()
            );
        }
        if !probe.observed.is_empty() {
            let observed = probe
                .observed
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  observed: {observed}");
        }
        if let Some(reason) = &probe.unverifiable_reason {
            println!("  unverifiable: {reason}");
        }
        println!(
            "  result: {} ({}) · window {} ms",
            probe.overall().as_str(),
            probe.confidence.as_str(),
            probe.window_ms
        );
    } else if outcome.exit_code == EXIT_INTERRUPTED {
        println!("  interrupted");
    } else {
        println!("  no probe evidence");
    }
    println!("  run dir: {}", outcome.run_dir.display());
}

/// Structured result for the MCP tool (the text block is pretty JSON).
#[must_use]
pub fn outcome_json(outcome: &ProbeOutcome) -> Value {
    let probe = outcome.probe.as_ref();
    json!({
        "ok": outcome.exit_code == EXIT_OK,
        "run_id": outcome.run_id.as_str(),
        "run_dir": outcome.run_dir.display().to_string(),
        "status": outcome.report.status.as_str(),
        "exit_code": outcome.exit_code,
        "kind": outcome.kind.as_str(),
        "device": outcome.device.as_str(),
        "platform": outcome.platform.as_str(),
        "selector": &outcome.selector,
        "selector_label": outcome.selector_label.as_str(),
        "claims": &outcome.claims,
        "verdict": probe.map(|probe| probe.overall().as_str()),
        "confidence": probe.map(|probe| probe.confidence.as_str()),
        "window_ms": probe.map(|probe| probe.window_ms),
        "verdicts": probe.map(|probe| &probe.verdicts),
        "evidence": probe.map(|probe| &probe.evidence),
        "observed": probe.map(|probe| &probe.observed),
        "unverifiable_reason": probe.and_then(|probe| probe.unverifiable_reason.as_deref()),
        "report": "report.json",
        "evidence_artifact": probe.map(|_| "evidence.json"),
    })
}

#[derive(Serialize)]
struct EvidenceBundle<'a> {
    schema_version: u32,
    run_id: &'a str,
    captured_at: String,
    kind: &'a str,
    device: &'a str,
    platform: &'a str,
    selector: &'a Selector,
    claims: &'a [Claim],
    result: &'a ProbeResult,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{
        ActReport, AttemptReport, DeviceReport, PlayReport, SpanJson, StageReport, Status,
        StepReport,
    };

    fn report(probe: ProbeResult, exit_code: i32, status: RunStatus) -> RunReport {
        RunReport {
            schema_version: 3,
            run_id: "20261110-120000-test".to_string(),
            started_at: "2026-11-10T12:00:00.000Z".to_string(),
            duration_ms: 1,
            status,
            exit_code,
            devices: vec![DeviceReport {
                name: "d".to_string(),
                platform: "web".to_string(),
                device_info: None,
                error: None,
                plays: vec![PlayReport {
                    name: "Probe".to_string(),
                    file: String::new(),
                    acts: vec![ActReport {
                        name: "Probe showing".to_string(),
                        tags: vec![],
                        example: BTreeMap::new(),
                        status: Status::Passed,
                        flaky: false,
                        attempts: vec![AttemptReport {
                            n: 1,
                            status: Status::Passed,
                            duration_ms: 1,
                            stages: vec![StageReport {
                                name: "Probe".to_string(),
                                span: None,
                                status: Status::Passed,
                                steps: vec![StepReport {
                                    source: "probe showing".to_string(),
                                    span: SpanJson {
                                        file: "<probe>".to_string(),
                                        line: 1,
                                        col: 1,
                                        len: 1,
                                    },
                                    status: Status::Passed,
                                    duration_ms: 1,
                                    error: None,
                                    probe: Some(probe),
                                }],
                            }],
                            failure: None,
                        }],
                    }],
                }],
            }],
        }
    }

    fn passing_probe() -> ProbeResult {
        ProbeResult {
            verdicts: vec![],
            evidence: vec![],
            confidence: Confidence::None,
            unverifiable_reason: None,
            observed: BTreeMap::new(),
            window_ms: 1,
        }
    }

    #[test]
    fn confidence_vocabulary_round_trips() {
        for confidence in Confidence::ALL {
            assert_eq!(parse_confidence(confidence.as_str()), Some(confidence));
        }
        assert_eq!(parse_confidence("nope"), None);
    }

    #[test]
    fn exit_code_follows_the_probe_verdict() {
        let report = report(passing_probe(), 0, RunStatus::Passed);
        assert_eq!(
            probe_exit_code(&report, find_probe(&report).as_ref()),
            EXIT_OK
        );

        let report = report(passing_probe(), 1, RunStatus::Failed);
        assert_eq!(
            probe_exit_code(&report, find_probe(&report).as_ref()),
            EXIT_FAILURE
        );

        let report = report(passing_probe(), 3, RunStatus::Error);
        assert_eq!(
            probe_exit_code(&report, find_probe(&report).as_ref()),
            EXIT_INFRA
        );

        let report = report(passing_probe(), 130, RunStatus::Interrupted);
        assert_eq!(
            probe_exit_code(&report, find_probe(&report).as_ref()),
            EXIT_INTERRUPTED
        );
    }

    #[test]
    fn claims_default_to_showing_present_and_support_not() {
        let claims = build_claims(
            ProbeKind::Showing,
            &["state=Playing".to_string(), "not drm.enabled".to_string()],
        )
        .expect("claims");
        assert_eq!(claims.len(), 3);
        assert_eq!(claims[0], Claim::new("showing=present"));
        assert_eq!(claims[1], Claim::new("state=Playing"));
        assert_eq!(claims[2], Claim::negated("drm.enabled"));

        let claims = build_claims(ProbeKind::Player, &[]).expect("claims");
        assert!(claims.is_empty());
    }
}
