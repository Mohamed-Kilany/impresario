//! Conversions between the wire-neutral `impresario-core` model and the
//! generated protobuf types (D-16: core never depends on the proto crate).
//!
//! All failures are malformed-wire errors with code `PROTOCOL` (spec 7.2.6:
//! unspecified or unknown enum values are protocol violations). The
//! exception is log severity, which is read leniently: log reading must never
//! fail a session.

use impresario_core::ast::{AppOp, Direction};
use impresario_core::command::{
    ActionType, CheckKind, Command, CommandOutcome, KeyPress, ProbeCommand, RunAction,
    ValidateCheck, WireError, WireErrorCode,
};
use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
use impresario_core::probe::{
    Claim, ClaimVerdict, Confidence, DrmProvider, Evidence, EvidenceClass, EvidenceSource,
    Observation, Operator, ProbeAssertion, ProbeKind, ProbeProperty, ProbeResult, Verdict,
};
use impresario_core::selectors::{Selector, Strategy};
use impresario_proto::v1 as pb;

use crate::log::{LogLevel, LogRecord};

/// A malformed wire value (always code `PROTOCOL`).
#[must_use]
pub fn protocol_error(message: impl Into<String>) -> WireError {
    WireError::new(WireErrorCode::Protocol, message)
}

// ---------------------------------------------------------------------------
// enums
// ---------------------------------------------------------------------------

#[must_use]
pub fn platform_to_proto(platform: Platform) -> pb::Platform {
    match platform {
        Platform::Web => pb::Platform::Web,
        Platform::WebOs => pb::Platform::Webos,
        Platform::Tizen => pb::Platform::Tizen,
        Platform::Android => pb::Platform::Android,
        Platform::AndroidTv => pb::Platform::AndroidTv,
        Platform::Ios => pb::Platform::Ios,
        Platform::Tvos => pb::Platform::Tvos,
    }
}

/// Unknown or unspecified platform values are `PROTOCOL`.
pub fn platform_from_proto(value: i32) -> Result<Platform, WireError> {
    match pb::Platform::try_from(value) {
        Ok(pb::Platform::Web) => Ok(Platform::Web),
        Ok(pb::Platform::Webos) => Ok(Platform::WebOs),
        Ok(pb::Platform::Tizen) => Ok(Platform::Tizen),
        Ok(pb::Platform::Android) => Ok(Platform::Android),
        Ok(pb::Platform::AndroidTv) => Ok(Platform::AndroidTv),
        Ok(pb::Platform::Ios) => Ok(Platform::Ios),
        Ok(pb::Platform::Tvos) => Ok(Platform::Tvos),
        Ok(pb::Platform::Unspecified) | Err(_) => Err(protocol_error(format!(
            "unknown platform enum value {value}"
        ))),
    }
}

#[must_use]
pub fn strategy_to_proto(strategy: Strategy) -> pb::SelectorStrategy {
    match strategy {
        Strategy::Css => pb::SelectorStrategy::Css,
        Strategy::XPath => pb::SelectorStrategy::Xpath,
        Strategy::Text => pb::SelectorStrategy::Text,
        Strategy::TextContains => pb::SelectorStrategy::TextContains,
        Strategy::AndroidId => pb::SelectorStrategy::AndroidId,
        Strategy::AndroidDesc => pb::SelectorStrategy::AndroidDesc,
        Strategy::AndroidClass => pb::SelectorStrategy::AndroidClass,
        Strategy::AccessibilityId => pb::SelectorStrategy::AccessibilityId,
        Strategy::Label => pb::SelectorStrategy::Label,
        Strategy::LabelContains => pb::SelectorStrategy::LabelContains,
        Strategy::Predicate => pb::SelectorStrategy::Predicate,
        Strategy::IosType => pb::SelectorStrategy::IosType,
    }
}

/// Unknown or unspecified selector strategy values are `PROTOCOL`.
pub fn strategy_from_proto(value: i32) -> Result<Strategy, WireError> {
    match pb::SelectorStrategy::try_from(value) {
        Ok(pb::SelectorStrategy::Css) => Ok(Strategy::Css),
        Ok(pb::SelectorStrategy::Xpath) => Ok(Strategy::XPath),
        Ok(pb::SelectorStrategy::Text) => Ok(Strategy::Text),
        Ok(pb::SelectorStrategy::TextContains) => Ok(Strategy::TextContains),
        Ok(pb::SelectorStrategy::AndroidId) => Ok(Strategy::AndroidId),
        Ok(pb::SelectorStrategy::AndroidDesc) => Ok(Strategy::AndroidDesc),
        Ok(pb::SelectorStrategy::AndroidClass) => Ok(Strategy::AndroidClass),
        Ok(pb::SelectorStrategy::AccessibilityId) => Ok(Strategy::AccessibilityId),
        Ok(pb::SelectorStrategy::Label) => Ok(Strategy::Label),
        Ok(pb::SelectorStrategy::LabelContains) => Ok(Strategy::LabelContains),
        Ok(pb::SelectorStrategy::Predicate) => Ok(Strategy::Predicate),
        Ok(pb::SelectorStrategy::IosType) => Ok(Strategy::IosType),
        Ok(pb::SelectorStrategy::Unspecified) | Err(_) => Err(protocol_error(format!(
            "unknown selector strategy enum value {value}"
        ))),
    }
}

#[must_use]
pub fn action_type_to_proto(action: ActionType) -> pb::ActionType {
    match action {
        ActionType::Click => pb::ActionType::Click,
        ActionType::Type => pb::ActionType::Type,
        ActionType::Append => pb::ActionType::Append,
        ActionType::Clear => pb::ActionType::Clear,
        ActionType::LongPress => pb::ActionType::LongPress,
        ActionType::Focus => pb::ActionType::Focus,
        ActionType::Select => pb::ActionType::Select,
        ActionType::Scroll => pb::ActionType::Scroll,
        ActionType::ScrollTo => pb::ActionType::ScrollTo,
        ActionType::Swipe => pb::ActionType::Swipe,
        ActionType::Back => pb::ActionType::Back,
    }
}

#[must_use]
pub fn direction_to_proto(direction: Direction) -> pb::Direction {
    match direction {
        Direction::Up => pb::Direction::Up,
        Direction::Down => pb::Direction::Down,
        Direction::Left => pb::Direction::Left,
        Direction::Right => pb::Direction::Right,
    }
}

#[must_use]
pub fn check_kind_to_proto(kind: CheckKind) -> pb::CheckKind {
    match kind {
        CheckKind::Visible => pb::CheckKind::Visible,
        CheckKind::Hidden => pb::CheckKind::Hidden,
        CheckKind::Enabled => pb::CheckKind::Enabled,
        CheckKind::Disabled => pb::CheckKind::Disabled,
        CheckKind::Focused => pb::CheckKind::Focused,
        CheckKind::TextEquals => pb::CheckKind::TextEquals,
        CheckKind::TextContains => pb::CheckKind::TextContainsCheck,
        CheckKind::Count => pb::CheckKind::Count,
    }
}

#[must_use]
pub fn remote_key_to_proto(key: RemoteKey) -> pb::RemoteKey {
    match key {
        RemoteKey::Ok => pb::RemoteKey::KeyOk,
        RemoteKey::Enter => pb::RemoteKey::KeyEnter,
        RemoteKey::Back => pb::RemoteKey::KeyBack,
        RemoteKey::Up => pb::RemoteKey::KeyUp,
        RemoteKey::Down => pb::RemoteKey::KeyDown,
        RemoteKey::Left => pb::RemoteKey::KeyLeft,
        RemoteKey::Right => pb::RemoteKey::KeyRight,
        RemoteKey::Home => pb::RemoteKey::KeyHome,
        RemoteKey::Menu => pb::RemoteKey::KeyMenu,
        RemoteKey::Play => pb::RemoteKey::KeyPlay,
        RemoteKey::Pause => pb::RemoteKey::KeyPause,
        RemoteKey::PlayPause => pb::RemoteKey::KeyPlaypause,
        RemoteKey::Rewind => pb::RemoteKey::KeyRewind,
        RemoteKey::FastForward => pb::RemoteKey::KeyFastforward,
        RemoteKey::Red => pb::RemoteKey::KeyRed,
        RemoteKey::Green => pb::RemoteKey::KeyGreen,
        RemoteKey::Yellow => pb::RemoteKey::KeyYellow,
        RemoteKey::Blue => pb::RemoteKey::KeyBlue,
        RemoteKey::Backspace => pb::RemoteKey::KeyBackspace,
    }
}

#[must_use]
pub fn app_op_to_proto(op: AppOp) -> pb::AppOp {
    match op {
        AppOp::Launch => pb::AppOp::Launch,
        AppOp::Terminate => pb::AppOp::Terminate,
        AppOp::Restart => pb::AppOp::Restart,
        AppOp::Background => pb::AppOp::Background,
    }
}

#[must_use]
pub fn error_code_to_proto(code: WireErrorCode) -> pb::ErrorCode {
    match code {
        WireErrorCode::ElementNotFound => pb::ErrorCode::ElementNotFound,
        WireErrorCode::Timeout => pb::ErrorCode::Timeout,
        WireErrorCode::NotInteractable => pb::ErrorCode::NotInteractable,
        WireErrorCode::StaleElement => pb::ErrorCode::StaleElement,
        WireErrorCode::FocusUnreachable => pb::ErrorCode::FocusUnreachable,
        WireErrorCode::UnsupportedAction => pb::ErrorCode::UnsupportedAction,
        WireErrorCode::InvalidSelector => pb::ErrorCode::InvalidSelector,
        WireErrorCode::CheckFailed => pb::ErrorCode::CheckFailed,
        WireErrorCode::AppCrashed => pb::ErrorCode::AppCrashed,
        WireErrorCode::Cancelled => pb::ErrorCode::Cancelled,
        WireErrorCode::Transport => pb::ErrorCode::Transport,
        WireErrorCode::Protocol => pb::ErrorCode::Protocol,
        WireErrorCode::Internal => pb::ErrorCode::Internal,
        WireErrorCode::AlreadyInUse => pb::ErrorCode::AlreadyInUse,
        WireErrorCode::DeviceUnavailable => pb::ErrorCode::DeviceUnavailable,
        WireErrorCode::Unauthenticated => pb::ErrorCode::Unauthenticated,
        WireErrorCode::Unverifiable => pb::ErrorCode::Unverifiable,
    }
}

/// Unknown `ErrorCode` values are themselves `PROTOCOL` violations.
pub fn error_code_from_proto(value: i32) -> Result<WireErrorCode, WireError> {
    match pb::ErrorCode::try_from(value) {
        Ok(pb::ErrorCode::ElementNotFound) => Ok(WireErrorCode::ElementNotFound),
        Ok(pb::ErrorCode::Timeout) => Ok(WireErrorCode::Timeout),
        Ok(pb::ErrorCode::NotInteractable) => Ok(WireErrorCode::NotInteractable),
        Ok(pb::ErrorCode::StaleElement) => Ok(WireErrorCode::StaleElement),
        Ok(pb::ErrorCode::FocusUnreachable) => Ok(WireErrorCode::FocusUnreachable),
        Ok(pb::ErrorCode::UnsupportedAction) => Ok(WireErrorCode::UnsupportedAction),
        Ok(pb::ErrorCode::InvalidSelector) => Ok(WireErrorCode::InvalidSelector),
        Ok(pb::ErrorCode::CheckFailed) => Ok(WireErrorCode::CheckFailed),
        Ok(pb::ErrorCode::AppCrashed) => Ok(WireErrorCode::AppCrashed),
        Ok(pb::ErrorCode::Cancelled) => Ok(WireErrorCode::Cancelled),
        Ok(pb::ErrorCode::Transport) => Ok(WireErrorCode::Transport),
        Ok(pb::ErrorCode::Protocol) => Ok(WireErrorCode::Protocol),
        Ok(pb::ErrorCode::Internal) => Ok(WireErrorCode::Internal),
        Ok(pb::ErrorCode::AlreadyInUse) => Ok(WireErrorCode::AlreadyInUse),
        Ok(pb::ErrorCode::DeviceUnavailable) => Ok(WireErrorCode::DeviceUnavailable),
        Ok(pb::ErrorCode::Unauthenticated) => Ok(WireErrorCode::Unauthenticated),
        Ok(pb::ErrorCode::Unverifiable) => Ok(WireErrorCode::Unverifiable),
        Ok(pb::ErrorCode::Unspecified) | Err(_) => Err(protocol_error(format!(
            "unknown error code enum value {value}"
        ))),
    }
}

#[must_use]
pub fn log_level_to_proto(level: LogLevel) -> pb::LogLevel {
    match level {
        LogLevel::Debug => pb::LogLevel::Debug,
        LogLevel::Info => pb::LogLevel::Info,
        LogLevel::Warn => pb::LogLevel::Warn,
        LogLevel::Error => pb::LogLevel::Error,
    }
}

/// Log severity is read leniently (unspecified/unknown -> `DEBUG`): reading
/// logs must never fail a session.
#[must_use]
pub fn log_level_from_proto(value: i32) -> LogLevel {
    match pb::LogLevel::try_from(value) {
        Ok(pb::LogLevel::Error) => LogLevel::Error,
        Ok(pb::LogLevel::Warn) => LogLevel::Warn,
        Ok(pb::LogLevel::Info) => LogLevel::Info,
        Ok(pb::LogLevel::Debug) | Ok(pb::LogLevel::Unspecified) | Err(_) => LogLevel::Debug,
    }
}

// ---------------------------------------------------------------------------
// messages
// ---------------------------------------------------------------------------

#[must_use]
pub fn selector_to_proto(selector: &Selector) -> pb::Selector {
    pb::Selector {
        strategy: strategy_to_proto(selector.strategy) as i32,
        value: selector.value.clone(),
        index: selector.index,
    }
}

/// The proto `Selector` has no platform key, so callers supply the device
/// platform the selector was resolved for.
pub fn selector_from_proto(key: Platform, selector: &pb::Selector) -> Result<Selector, WireError> {
    Ok(Selector {
        key,
        strategy: strategy_from_proto(selector.strategy)?,
        value: selector.value.clone(),
        index: selector.index,
    })
}

#[must_use]
pub fn run_action_to_proto(action: &RunAction) -> pb::RunAction {
    pb::RunAction {
        r#type: action_type_to_proto(action.action) as i32,
        selector: action.selector.as_ref().map(selector_to_proto),
        text: action.text.clone(),
        direction: action.direction.map_or(0, |d| direction_to_proto(d) as i32),
        count: action.count,
        duration_ms: action.duration_ms,
    }
}

#[must_use]
pub fn validate_check_to_proto(check: &ValidateCheck) -> pb::ValidateCheck {
    pb::ValidateCheck {
        selector: Some(selector_to_proto(&check.selector)),
        kind: check_kind_to_proto(check.kind) as i32,
        expected_text: check.expected_text.clone(),
        expected_count: check.expected_count,
    }
}

#[must_use]
pub fn key_press_to_proto(press: &KeyPress) -> pb::KeyPress {
    pb::KeyPress {
        key: remote_key_to_proto(press.key) as i32,
        repeat: press.repeat,
        interval_ms: press.interval_ms,
    }
}

#[must_use]
pub fn probe_kind_to_proto(kind: ProbeKind) -> pb::ProbeKind {
    match kind {
        ProbeKind::Showing => pb::ProbeKind::Showing,
        ProbeKind::Player => pb::ProbeKind::Player,
        ProbeKind::Drm => pb::ProbeKind::Drm,
    }
}

pub fn probe_kind_from_proto(value: i32) -> Result<ProbeKind, WireError> {
    match pb::ProbeKind::try_from(value) {
        Ok(pb::ProbeKind::Showing) => Ok(ProbeKind::Showing),
        Ok(pb::ProbeKind::Player) => Ok(ProbeKind::Player),
        Ok(pb::ProbeKind::Drm) => Ok(ProbeKind::Drm),
        Ok(pb::ProbeKind::Unspecified) | Err(_) => Err(protocol_error(format!(
            "unknown probe kind enum value {value}"
        ))),
    }
}

#[must_use]
pub fn confidence_to_proto(confidence: Confidence) -> pb::ProbeConfidence {
    match confidence {
        Confidence::None => pb::ProbeConfidence::None,
        Confidence::Inferred => pb::ProbeConfidence::Inferred,
        Confidence::Corroborated => pb::ProbeConfidence::Corroborated,
        Confidence::Authoritative => pb::ProbeConfidence::Authoritative,
    }
}

pub fn confidence_from_proto(value: i32) -> Result<Confidence, WireError> {
    match pb::ProbeConfidence::try_from(value) {
        Ok(pb::ProbeConfidence::None) => Ok(Confidence::None),
        Ok(pb::ProbeConfidence::Inferred) => Ok(Confidence::Inferred),
        Ok(pb::ProbeConfidence::Corroborated) => Ok(Confidence::Corroborated),
        Ok(pb::ProbeConfidence::Authoritative) => Ok(Confidence::Authoritative),
        Err(_) => Err(protocol_error(format!(
            "unknown probe confidence enum value {value}"
        ))),
    }
}

#[must_use]
pub fn evidence_class_to_proto(class: EvidenceClass) -> pb::EvidenceClass {
    match class {
        EvidenceClass::Heuristic => pb::EvidenceClass::Heuristic,
        EvidenceClass::System => pb::EvidenceClass::System,
        EvidenceClass::Authoritative => pb::EvidenceClass::Authoritative,
    }
}

pub fn evidence_class_from_proto(value: i32) -> Result<EvidenceClass, WireError> {
    match pb::EvidenceClass::try_from(value) {
        Ok(pb::EvidenceClass::Heuristic) => Ok(EvidenceClass::Heuristic),
        Ok(pb::EvidenceClass::System) => Ok(EvidenceClass::System),
        Ok(pb::EvidenceClass::Authoritative) => Ok(EvidenceClass::Authoritative),
        Err(_) => Err(protocol_error(format!(
            "unknown evidence class enum value {value}"
        ))),
    }
}

#[must_use]
pub fn verdict_to_proto(verdict: Verdict) -> pb::VerdictState {
    match verdict {
        Verdict::Pass => pb::VerdictState::Pass,
        Verdict::Fail => pb::VerdictState::Fail,
        Verdict::Unverifiable => pb::VerdictState::Unverifiable,
    }
}

pub fn verdict_from_proto(value: i32) -> Result<Verdict, WireError> {
    match pb::VerdictState::try_from(value) {
        Ok(pb::VerdictState::Pass) => Ok(Verdict::Pass),
        Ok(pb::VerdictState::Fail) => Ok(Verdict::Fail),
        Ok(pb::VerdictState::Unverifiable) => Ok(Verdict::Unverifiable),
        Ok(pb::VerdictState::Unspecified) | Err(_) => Err(protocol_error(format!(
            "unknown verdict enum value {value}"
        ))),
    }
}

#[must_use]
pub fn probe_command_to_proto(command: &ProbeCommand) -> pb::ProbeCommand {
    pb::ProbeCommand {
        kind: probe_kind_to_proto(command.kind) as i32,
        selector: Some(selector_to_proto(&command.selector)),
        claims: command
            .claims
            .iter()
            .map(|claim| pb::ProbeClaim {
                claim: claim.id.clone(),
                negated: claim.negated,
            })
            .collect(),
        assertions: command
            .assertions
            .iter()
            .map(|assertion| pb::ProbeAssertion {
                property: assertion.property.clone(),
                operator: assertion
                    .operator
                    .map_or_else(String::new, |operator| operator.as_str().to_string()),
                value: assertion.value.clone(),
                negated: assertion.negated,
            })
            .collect(),
        min_confidence: confidence_to_proto(command.min_confidence) as i32,
        window_ms: command.window_ms,
        require_decoded: command.require_decoded,
        require_source: command.require_source,
        min_coverage: f64::from(command.min_coverage_pct) / 100.0,
    }
}

#[must_use]
pub fn observation_to_proto(observation: &Observation) -> (String, String) {
    match observation {
        Observation::Bool(value) => ("bool".to_string(), value.to_string()),
        Observation::Number(value) => ("number".to_string(), value.to_string()),
        Observation::Text(value) => ("text".to_string(), value.clone()),
        Observation::Enum(value) => ("enum".to_string(), value.clone()),
        Observation::Range { min, max } => ("range".to_string(), format!("{min}..{max}")),
    }
}

pub fn observation_from_proto(kind: &str, value: &str) -> Result<Observation, WireError> {
    match kind {
        "bool" => value
            .parse::<bool>()
            .map(Observation::Bool)
            .map_err(|_| protocol_error(format!("invalid boolean observation '{value}'"))),
        "number" => value
            .parse::<f64>()
            .map(Observation::Number)
            .map_err(|_| protocol_error(format!("invalid number observation '{value}'"))),
        "text" => Ok(Observation::Text(value.to_string())),
        "enum" => Ok(Observation::Enum(value.to_string())),
        "range" => {
            let (min, max) = value
                .split_once("..")
                .ok_or_else(|| protocol_error(format!("invalid range observation '{value}'")))?;
            Ok(Observation::Range {
                min: min
                    .parse::<f64>()
                    .map_err(|_| protocol_error(format!("invalid range minimum '{min}'")))?,
                max: max
                    .parse::<f64>()
                    .map_err(|_| protocol_error(format!("invalid range maximum '{max}'")))?,
            })
        }
        other => Err(protocol_error(format!(
            "unknown observation type '{other}'"
        ))),
    }
}

#[must_use]
pub fn evidence_to_proto(evidence: &Evidence) -> pb::ProbeEvidence {
    let (observation_type, observation) = observation_to_proto(&evidence.observation);
    pb::ProbeEvidence {
        source: evidence.source.as_str().to_string(),
        kind: evidence.kind.clone(),
        observation_type,
        observation,
        at_unix_ms: evidence.at_unix_ms,
        window_ms: evidence.window_ms,
        class: evidence_class_to_proto(evidence.class) as i32,
        supports: evidence.supports.clone(),
        contradicts: evidence.contradicts.clone(),
    }
}

pub fn evidence_from_proto(evidence: &pb::ProbeEvidence) -> Result<Evidence, WireError> {
    Ok(Evidence {
        source: EvidenceSource::parse(&evidence.source),
        kind: evidence.kind.clone(),
        observation: observation_from_proto(&evidence.observation_type, &evidence.observation)?,
        at_unix_ms: evidence.at_unix_ms,
        window_ms: evidence.window_ms,
        class: evidence_class_from_proto(evidence.class)?,
        supports: evidence.supports.clone(),
        contradicts: evidence.contradicts.clone(),
    })
}

#[must_use]
pub fn claim_verdict_to_proto(verdict: &ClaimVerdict) -> pb::ClaimVerdict {
    pb::ClaimVerdict {
        claim: verdict.claim.clone(),
        verdict: verdict_to_proto(verdict.verdict) as i32,
        confidence: confidence_to_proto(verdict.confidence) as i32,
        reason: verdict.reason.clone().unwrap_or_default(),
    }
}

pub fn claim_verdict_from_proto(verdict: &pb::ClaimVerdict) -> Result<ClaimVerdict, WireError> {
    Ok(ClaimVerdict {
        claim: verdict.claim.clone(),
        verdict: verdict_from_proto(verdict.verdict)?,
        confidence: confidence_from_proto(verdict.confidence)?,
        reason: (!verdict.reason.is_empty()).then(|| verdict.reason.clone()),
    })
}

#[must_use]
pub fn probe_result_to_proto(result: &ProbeResult) -> pb::ProbeResult {
    pb::ProbeResult {
        verdicts: result.verdicts.iter().map(claim_verdict_to_proto).collect(),
        evidence: result.evidence.iter().map(evidence_to_proto).collect(),
        confidence: confidence_to_proto(result.confidence) as i32,
        unverifiable_reason: result.unverifiable_reason.clone().unwrap_or_default(),
        observed: result.observed.clone().into_iter().collect(),
        window_ms: result.window_ms,
    }
}

pub fn probe_result_from_proto(result: &pb::ProbeResult) -> Result<ProbeResult, WireError> {
    Ok(ProbeResult {
        verdicts: result
            .verdicts
            .iter()
            .map(claim_verdict_from_proto)
            .collect::<Result<Vec<_>, _>>()?,
        evidence: result
            .evidence
            .iter()
            .map(evidence_from_proto)
            .collect::<Result<Vec<_>, _>>()?,
        confidence: confidence_from_proto(result.confidence)?,
        unverifiable_reason: (!result.unverifiable_reason.is_empty())
            .then(|| result.unverifiable_reason.clone()),
        observed: result
            .observed
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        window_ms: result.window_ms,
    })
}

/// Parse optional assertion operators and property names from the wire. The
/// collector is expected to ignore properties it cannot observe; the planner
/// already rejected unsupported names, so an unknown value is a bug and
/// reported as `PROTOCOL` by the agent, not silently accepted.
pub fn assertion_from_proto(assertion: &pb::ProbeAssertion) -> ProbeAssertion {
    ProbeAssertion {
        property: assertion.property.clone(),
        operator: if assertion.operator.is_empty() {
            None
        } else {
            Operator::parse(&assertion.operator)
        },
        value: assertion.value.clone(),
        negated: assertion.negated,
    }
}

/// Claim ids are opaque strings generated by the master; no validation here.
#[must_use]
pub fn claim_from_proto(claim: &pb::ProbeClaim) -> Claim {
    Claim {
        id: claim.claim.clone(),
        negated: claim.negated,
    }
}

/// The names of the probe properties accepted by this build (docs and
/// diagnostics use the same list).
#[must_use]
pub fn probe_property_names() -> Vec<&'static str> {
    ProbeProperty::ALL
        .iter()
        .map(|property| property.as_str())
        .collect()
}

/// The provider names accepted by this build.
#[must_use]
pub fn drm_provider_names() -> Vec<&'static str> {
    DrmProvider::ALL
        .iter()
        .map(|provider| provider.as_str())
        .collect()
}

#[must_use]
pub fn error_info_to_proto(error: &WireError) -> pb::ErrorInfo {
    pb::ErrorInfo {
        code: error_code_to_proto(error.code) as i32,
        message: error.message.clone(),
        details: error
            .details
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

pub fn error_info_from_proto(error: &pb::ErrorInfo) -> Result<WireError, WireError> {
    Ok(WireError {
        code: error_code_from_proto(error.code)?,
        message: error.message.clone(),
        details: error
            .details
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    })
}

/// Build a wire `Command` with its session-unique id and the executor wait
/// budget (spec 7.2.7; the master's grace period is added on the client side).
#[must_use]
pub fn command_to_proto(command: &Command, command_id: &str, timeout_ms: u32) -> pb::Command {
    let body = match command {
        Command::RunAction(action) => pb::command::Body::RunAction(run_action_to_proto(action)),
        Command::ValidateCheck(check) => {
            pb::command::Body::ValidateCheck(validate_check_to_proto(check))
        }
        Command::KeyPress(press) => pb::command::Body::KeyPress(key_press_to_proto(press)),
        Command::Screenshot { label } => pb::command::Body::Screenshot(pb::Screenshot {
            label: label.clone(),
        }),
        Command::AppControl { op, clear_data } => pb::command::Body::AppControl(pb::AppControl {
            op: app_op_to_proto(*op) as i32,
            clear_data: *clear_data,
        }),
        Command::Cancel { target_command_id } => pb::command::Body::Cancel(pb::Cancel {
            target_command_id: target_command_id.clone(),
        }),
        Command::Probe(probe) => pb::command::Body::Probe(probe_command_to_proto(probe)),
    };
    pb::Command {
        command_id: command_id.to_string(),
        timeout_ms,
        body: Some(body),
    }
}

/// `CommandResult`s with `ok=false` must carry `ErrorInfo` (spec 7.2.4); a
/// missing one is a protocol violation.
pub fn outcome_from_proto(result: &pb::CommandResult) -> Result<CommandOutcome, WireError> {
    let error = match &result.error {
        Some(info) => Some(error_info_from_proto(info)?),
        None => None,
    };
    if !result.ok && error.is_none() {
        return Err(protocol_error(format!(
            "command result '{}' reports failure without ErrorInfo",
            result.command_id
        )));
    }
    let probe = result
        .probe
        .as_ref()
        .map(probe_result_from_proto)
        .transpose()?;
    Ok(CommandOutcome {
        command_id: result.command_id.clone(),
        ok: result.ok,
        duration_ms: result.duration_ms,
        error,
        screenshot_png: result.screenshot_png.clone(),
        observed_text: result.observed_text.clone(),
        observed_count: result.observed_count,
        probe,
    })
}

/// `LogEntry` -> [`LogRecord`]; never fails (see [`log_level_from_proto`]).
#[must_use]
pub fn log_record_from_proto(entry: &pb::LogEntry) -> LogRecord {
    LogRecord {
        unix_ms: entry.unix_ms,
        level: log_level_from_proto(entry.level),
        source: entry.source.clone(),
        message: entry.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use impresario_core::selectors::Strategy;

    fn selector() -> Selector {
        Selector {
            key: Platform::Android,
            strategy: Strategy::AndroidId,
            value: "login_btn".to_string(),
            index: 1,
        }
    }

    #[test]
    fn platforms_round_trip() {
        for platform in Platform::ALL {
            let value = platform_to_proto(platform) as i32;
            assert_eq!(platform_from_proto(value), Ok(platform));
        }
        assert!(platform_from_proto(0).is_err());
        assert!(platform_from_proto(99).is_err());
    }

    #[test]
    fn probe_command_and_result_round_trip() {
        let mut command = ProbeCommand::new(
            ProbeKind::Player,
            Selector {
                key: Platform::Web,
                strategy: Strategy::Css,
                value: "#video".to_string(),
                index: 0,
            },
            vec![
                Claim::negated("state=Paused"),
                Claim::new("drm.enabled=false"),
            ],
        );
        command.assertions = vec![ProbeAssertion {
            property: "position".to_string(),
            operator: Some(Operator::Gt),
            value: "3".to_string(),
            negated: false,
        }];
        command.min_confidence = Confidence::Authoritative;
        command.window_ms = 1_500;
        command.require_decoded = true;
        command.require_source = true;
        command.min_coverage_pct = 40;

        let proto = probe_command_to_proto(&command);
        assert_eq!(proto.kind, pb::ProbeKind::Player as i32);
        assert_eq!(proto.claims.len(), 2);
        assert!(proto.claims[0].negated);
        assert_eq!(proto.assertions[0].property, "position");
        assert_eq!(
            proto.min_confidence,
            pb::ProbeConfidence::Authoritative as i32
        );
        assert_eq!(proto.window_ms, 1_500);
        assert!(proto.require_decoded);
        assert!(proto.require_source);
        assert!((proto.min_coverage - 0.4).abs() < f64::EPSILON);

        let evidence = vec![
            Evidence::supporting(
                EvidenceSource::MediaSession,
                "media_state",
                Observation::Enum("Playing".to_string()),
                EvidenceClass::Authoritative,
                "state=Paused",
            ),
            Evidence::contradicting(
                EvidenceSource::MediaDrm,
                "drm_enabled",
                Observation::Bool(false),
                EvidenceClass::Authoritative,
                "state=Paused",
            ),
        ];
        let result = ProbeResult {
            verdicts: vec![ClaimVerdict {
                claim: "state=Paused".to_string(),
                verdict: Verdict::Fail,
                confidence: Confidence::Authoritative,
                reason: Some("evidence contradicts the claim".to_string()),
            }],
            evidence,
            confidence: Confidence::Authoritative,
            unverifiable_reason: None,
            observed: [("state".to_string(), "Playing".to_string())]
                .into_iter()
                .collect(),
            window_ms: 1_500,
        };
        let round_tripped =
            probe_result_from_proto(&probe_result_to_proto(&result)).expect("result round trip");
        assert_eq!(round_tripped, result);
    }

    #[test]
    fn strategies_round_trip() {
        let all = [
            Strategy::Css,
            Strategy::XPath,
            Strategy::Text,
            Strategy::TextContains,
            Strategy::AndroidId,
            Strategy::AndroidDesc,
            Strategy::AndroidClass,
            Strategy::AccessibilityId,
            Strategy::Label,
            Strategy::LabelContains,
            Strategy::Predicate,
            Strategy::IosType,
        ];
        for strategy in all {
            let value = strategy_to_proto(strategy) as i32;
            assert_eq!(strategy_from_proto(value), Ok(strategy), "{strategy:?}");
        }
        assert!(strategy_from_proto(0).is_err());
    }

    #[test]
    fn error_codes_round_trip() {
        let all = [
            WireErrorCode::ElementNotFound,
            WireErrorCode::Timeout,
            WireErrorCode::NotInteractable,
            WireErrorCode::StaleElement,
            WireErrorCode::FocusUnreachable,
            WireErrorCode::UnsupportedAction,
            WireErrorCode::InvalidSelector,
            WireErrorCode::CheckFailed,
            WireErrorCode::AppCrashed,
            WireErrorCode::Cancelled,
            WireErrorCode::Transport,
            WireErrorCode::Protocol,
            WireErrorCode::Internal,
            WireErrorCode::AlreadyInUse,
            WireErrorCode::DeviceUnavailable,
            WireErrorCode::Unauthenticated,
        ];
        for code in all {
            let value = error_code_to_proto(code) as i32;
            assert_eq!(error_code_from_proto(value), Ok(code), "{code:?}");
        }
        assert!(error_code_from_proto(0).is_err());
        assert!(error_code_from_proto(77).is_err());
    }

    #[test]
    fn keys_map_all_variants() {
        for key in RemoteKey::ALL {
            let value = remote_key_to_proto(key) as i32;
            assert_ne!(value, 0, "{} maps to unspecified", key.as_str());
        }
        assert_eq!(
            remote_key_to_proto(RemoteKey::PlayPause),
            pb::RemoteKey::KeyPlaypause
        );
        assert_eq!(
            remote_key_to_proto(RemoteKey::FastForward),
            pb::RemoteKey::KeyFastforward
        );
    }

    #[test]
    fn click_command_round_trip() {
        let command = Command::RunAction(RunAction::click(selector()));
        let proto = command_to_proto(&command, "cmd-1", 2500);
        assert_eq!(proto.command_id, "cmd-1");
        assert_eq!(proto.timeout_ms, 2500);
        let Some(pb::command::Body::RunAction(action)) = &proto.body else {
            panic!("expected a run_action body");
        };
        assert_eq!(action.r#type, pb::ActionType::Click as i32);
        let sel = action.selector.as_ref().expect("selector");
        assert_eq!(sel.strategy, pb::SelectorStrategy::AndroidId as i32);
        assert_eq!(sel.value, "login_btn");
        assert_eq!(sel.index, 1);
    }

    #[test]
    fn screen_level_scroll_has_no_selector() {
        let command = Command::RunAction(RunAction::scroll(Direction::Down, 3, None));
        let proto = command_to_proto(&command, "c", 1000);
        let Some(pb::command::Body::RunAction(action)) = &proto.body else {
            panic!("expected a run_action body");
        };
        assert!(action.selector.is_none());
        assert_eq!(action.direction, pb::Direction::Down as i32);
        assert_eq!(action.count, 3);
    }

    #[test]
    fn back_command_is_bare() {
        let proto = command_to_proto(&Command::RunAction(RunAction::back()), "b", 0);
        let Some(pb::command::Body::RunAction(action)) = &proto.body else {
            panic!("expected a run_action body");
        };
        assert_eq!(action.r#type, pb::ActionType::Back as i32);
        assert!(action.selector.is_none());
        assert_eq!(action.direction, 0);
    }

    #[test]
    fn app_control_and_key_press_convert() {
        let proto = command_to_proto(
            &Command::AppControl {
                op: AppOp::Restart,
                clear_data: true,
            },
            "a",
            0,
        );
        let Some(pb::command::Body::AppControl(app)) = &proto.body else {
            panic!("expected an app_control body");
        };
        assert_eq!(app.op, pb::AppOp::Restart as i32);
        assert!(app.clear_data);

        let proto = command_to_proto(&Command::KeyPress(KeyPress::new(RemoteKey::Ok, 2)), "k", 0);
        let Some(pb::command::Body::KeyPress(press)) = &proto.body else {
            panic!("expected a key_press body");
        };
        assert_eq!(press.key, pb::RemoteKey::KeyOk as i32);
        assert_eq!(press.repeat, 2);
        assert_eq!(press.interval_ms, 100);
    }

    #[test]
    fn outcome_conversion_is_strict_about_failures() {
        let good = pb::CommandResult {
            command_id: "c1".to_string(),
            ok: true,
            duration_ms: 12,
            observed_text: "hello".to_string(),
            ..Default::default()
        };
        let outcome = outcome_from_proto(&good).expect("ok");
        assert!(outcome.ok);
        assert_eq!(outcome.observed_text, "hello");

        let malformed = pb::CommandResult {
            command_id: "c2".to_string(),
            ok: false,
            ..Default::default()
        };
        let err = outcome_from_proto(&malformed).expect_err("must fail");
        assert_eq!(err.code, WireErrorCode::Protocol);

        let with_error = pb::CommandResult {
            command_id: "c3".to_string(),
            ok: false,
            error: Some(pb::ErrorInfo {
                code: pb::ErrorCode::ElementNotFound as i32,
                message: "missing".to_string(),
                details: [("selector".to_string(), "#x".to_string())]
                    .into_iter()
                    .collect(),
            }),
            ..Default::default()
        };
        let outcome = outcome_from_proto(&with_error).expect("ok");
        let error = outcome.error.expect("error");
        assert_eq!(error.code, WireErrorCode::ElementNotFound);
        assert_eq!(
            error.details.get("selector").map(String::as_str),
            Some("#x")
        );
    }

    #[test]
    fn unknown_error_code_is_protocol() {
        let result = pb::CommandResult {
            command_id: "c".to_string(),
            ok: false,
            error: Some(pb::ErrorInfo {
                code: 12345,
                message: "???".to_string(),
                details: Default::default(),
            }),
            ..Default::default()
        };
        let err = outcome_from_proto(&result).expect_err("must fail");
        assert_eq!(err.code, WireErrorCode::Protocol);
    }

    #[test]
    fn log_levels_are_lenient() {
        assert_eq!(
            log_level_from_proto(pb::LogLevel::Warn as i32),
            LogLevel::Warn
        );
        assert_eq!(log_level_from_proto(0), LogLevel::Debug);
        assert_eq!(log_level_from_proto(99), LogLevel::Debug);
        assert_eq!(log_level_to_proto(LogLevel::Error), pb::LogLevel::Error);
    }
}
