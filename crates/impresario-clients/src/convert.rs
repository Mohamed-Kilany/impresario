//! Conversions between the wire-neutral `impresario-core` model and the
//! generated protobuf types (D-16: core never depends on the proto crate).
//!
//! All failures are malformed-wire errors with code `PROTOCOL` (spec 7.2.6:
//! unspecified or unknown enum values are protocol violations). The
//! exception is log severity, which is read leniently: log reading must never
//! fail a session.

use impresario_core::ast::{AppOp, Direction};
use impresario_core::command::{
    ActionType, CheckKind, Command, CommandOutcome, KeyPress, RunAction, ValidateCheck, WireError,
    WireErrorCode,
};
use impresario_core::keys::RemoteKey;
use impresario_core::platforms::Platform;
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
    Ok(CommandOutcome {
        command_id: result.command_id.clone(),
        ok: result.ok,
        duration_ms: result.duration_ms,
        error,
        screenshot_png: result.screenshot_png.clone(),
        observed_text: result.observed_text.clone(),
        observed_count: result.observed_count,
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
