//! Wire-neutral command and result model mirroring `impresario.v1` (spec
//! section 7.1).
//!
//! The planner compiles steps into these values; `impresario-clients`
//! converts them to protobuf (D-16: core never depends on the proto crate).
//! Field defaults match the contract: `count` 1, `duration_ms` 1000 for
//! long-press, key `repeat` 1 with a 100 ms interval.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ast::{AppOp, Direction};
use crate::keys::RemoteKey;
use crate::selectors::Selector;

/// `ActionType` of the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    Click,
    Type,
    Append,
    Clear,
    LongPress,
    Focus,
    Select,
    Scroll,
    ScrollTo,
    Swipe,
    Back,
}

impl ActionType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ActionType::Click => "CLICK",
            ActionType::Type => "TYPE",
            ActionType::Append => "APPEND",
            ActionType::Clear => "CLEAR",
            ActionType::LongPress => "LONG_PRESS",
            ActionType::Focus => "FOCUS",
            ActionType::Select => "SELECT",
            ActionType::Scroll => "SCROLL",
            ActionType::ScrollTo => "SCROLL_TO",
            ActionType::Swipe => "SWIPE",
            ActionType::Back => "BACK",
        }
    }
}

/// `RunAction` of the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAction {
    pub action: ActionType,
    /// Absent only for `BACK` and screen-level `SCROLL`/`SWIPE`.
    pub selector: Option<Selector>,
    /// `TYPE`, `APPEND`, `SELECT` (option text).
    pub text: String,
    pub direction: Option<Direction>,
    pub count: u32,
    pub duration_ms: u32,
}

impl RunAction {
    fn bare(action: ActionType) -> Self {
        Self {
            action,
            selector: None,
            text: String::new(),
            direction: None,
            count: 1,
            duration_ms: 0,
        }
    }

    #[must_use]
    pub fn click(selector: Selector) -> Self {
        Self {
            selector: Some(selector),
            ..Self::bare(ActionType::Click)
        }
    }

    #[must_use]
    pub fn type_text(selector: Selector, text: impl Into<String>) -> Self {
        Self {
            selector: Some(selector),
            text: text.into(),
            ..Self::bare(ActionType::Type)
        }
    }

    #[must_use]
    pub fn append(selector: Selector, text: impl Into<String>) -> Self {
        Self {
            selector: Some(selector),
            text: text.into(),
            ..Self::bare(ActionType::Append)
        }
    }

    #[must_use]
    pub fn clear(selector: Selector) -> Self {
        Self {
            selector: Some(selector),
            ..Self::bare(ActionType::Clear)
        }
    }

    #[must_use]
    pub fn long_press(selector: Selector, duration_ms: u32) -> Self {
        Self {
            selector: Some(selector),
            duration_ms,
            ..Self::bare(ActionType::LongPress)
        }
    }

    #[must_use]
    pub fn focus(selector: Selector) -> Self {
        Self {
            selector: Some(selector),
            ..Self::bare(ActionType::Focus)
        }
    }

    #[must_use]
    pub fn select(selector: Selector, option: impl Into<String>) -> Self {
        Self {
            selector: Some(selector),
            text: option.into(),
            ..Self::bare(ActionType::Select)
        }
    }

    #[must_use]
    pub fn scroll(direction: Direction, count: u32, selector: Option<Selector>) -> Self {
        Self {
            selector,
            direction: Some(direction),
            count,
            ..Self::bare(ActionType::Scroll)
        }
    }

    #[must_use]
    pub fn scroll_to(selector: Selector) -> Self {
        Self {
            selector: Some(selector),
            ..Self::bare(ActionType::ScrollTo)
        }
    }

    #[must_use]
    pub fn swipe(direction: Direction, selector: Option<Selector>) -> Self {
        Self {
            selector,
            direction: Some(direction),
            ..Self::bare(ActionType::Swipe)
        }
    }

    #[must_use]
    pub fn back() -> Self {
        Self::bare(ActionType::Back)
    }
}

/// `CheckKind` of the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckKind {
    Visible,
    Hidden,
    Enabled,
    Disabled,
    Focused,
    TextEquals,
    TextContains,
    Count,
}

impl CheckKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CheckKind::Visible => "VISIBLE",
            CheckKind::Hidden => "HIDDEN",
            CheckKind::Enabled => "ENABLED",
            CheckKind::Disabled => "DISABLED",
            CheckKind::Focused => "FOCUSED",
            CheckKind::TextEquals => "TEXT_EQUALS",
            CheckKind::TextContains => "TEXT_CONTAINS_CHECK",
            CheckKind::Count => "COUNT",
        }
    }
}

/// `ValidateCheck` of the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateCheck {
    pub selector: Selector,
    pub kind: CheckKind,
    pub expected_text: String,
    pub expected_count: u32,
}

impl ValidateCheck {
    #[must_use]
    pub fn visible(selector: Selector) -> Self {
        Self {
            selector,
            kind: CheckKind::Visible,
            expected_text: String::new(),
            expected_count: 0,
        }
    }
}

/// `KeyPress` of the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPress {
    pub key: RemoteKey,
    pub repeat: u32,
    pub interval_ms: u32,
}

impl KeyPress {
    #[must_use]
    pub fn new(key: RemoteKey, repeat: u32) -> Self {
        Self {
            key,
            repeat: repeat.max(1),
            interval_ms: 100,
        }
    }
}

/// A command to send on a session, mirroring the `Command` oneof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    RunAction(RunAction),
    ValidateCheck(ValidateCheck),
    KeyPress(KeyPress),
    Screenshot { label: String },
    AppControl { op: AppOp, clear_data: bool },
    Cancel { target_command_id: String },
}

impl Command {
    /// The oneof arm name (reporting and tests).
    #[must_use]
    pub fn kind_str(&self) -> &'static str {
        match self {
            Command::RunAction(_) => "run_action",
            Command::ValidateCheck(_) => "validate_check",
            Command::KeyPress(_) => "key_press",
            Command::Screenshot { .. } => "screenshot",
            Command::AppControl { .. } => "app_control",
            Command::Cancel { .. } => "cancel",
        }
    }
}

/// `ErrorCode` of the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireErrorCode {
    ElementNotFound,
    Timeout,
    NotInteractable,
    StaleElement,
    FocusUnreachable,
    UnsupportedAction,
    InvalidSelector,
    CheckFailed,
    AppCrashed,
    Cancelled,
    Transport,
    Protocol,
    Internal,
    AlreadyInUse,
    DeviceUnavailable,
    Unauthenticated,
}

impl WireErrorCode {
    /// Every code, in wire-declaration order.
    pub const ALL: [WireErrorCode; 16] = [
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

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WireErrorCode::ElementNotFound => "ELEMENT_NOT_FOUND",
            WireErrorCode::Timeout => "TIMEOUT",
            WireErrorCode::NotInteractable => "NOT_INTERACTABLE",
            WireErrorCode::StaleElement => "STALE_ELEMENT",
            WireErrorCode::FocusUnreachable => "FOCUS_UNREACHABLE",
            WireErrorCode::UnsupportedAction => "UNSUPPORTED_ACTION",
            WireErrorCode::InvalidSelector => "INVALID_SELECTOR",
            WireErrorCode::CheckFailed => "CHECK_FAILED",
            WireErrorCode::AppCrashed => "APP_CRASHED",
            WireErrorCode::Cancelled => "CANCELLED",
            WireErrorCode::Transport => "TRANSPORT",
            WireErrorCode::Protocol => "PROTOCOL",
            WireErrorCode::Internal => "INTERNAL",
            WireErrorCode::AlreadyInUse => "ALREADY_IN_USE",
            WireErrorCode::DeviceUnavailable => "DEVICE_UNAVAILABLE",
            WireErrorCode::Unauthenticated => "UNAUTHENTICATED",
        }
    }

    /// Infrastructure errors (exit code 3 when nothing else ran).
    #[must_use]
    pub fn is_infrastructure(self) -> bool {
        matches!(
            self,
            WireErrorCode::Transport
                | WireErrorCode::DeviceUnavailable
                | WireErrorCode::Unauthenticated
                | WireErrorCode::AlreadyInUse
                | WireErrorCode::Protocol
                | WireErrorCode::Internal
        )
    }
}

/// `ErrorInfo` of the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    pub code: WireErrorCode,
    pub message: String,
    pub details: BTreeMap<String, String>,
}

impl WireError {
    #[must_use]
    pub fn new(code: WireErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: BTreeMap::new(),
        }
    }
}

/// `CommandResult` of the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutcome {
    pub command_id: String,
    pub ok: bool,
    pub duration_ms: u32,
    pub error: Option<WireError>,
    pub screenshot_png: Vec<u8>,
    pub observed_text: String,
    pub observed_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::Platform;
    use crate::selectors::Strategy;

    fn selector() -> Selector {
        Selector {
            key: Platform::Android,
            strategy: Strategy::AndroidId,
            value: "login_btn".to_string(),
            index: 0,
        }
    }

    #[test]
    fn defaults_match_the_contract() {
        let action = RunAction::click(selector());
        assert_eq!(action.count, 1);
        assert_eq!(action.duration_ms, 0);
        assert!(action.direction.is_none());

        let key = KeyPress::new(RemoteKey::Ok, 0);
        assert_eq!(key.repeat, 1, "repeat zero means one press");
        assert_eq!(key.interval_ms, 100);

        let long = RunAction::long_press(selector(), 1500);
        assert_eq!(long.action, ActionType::LongPress);
        assert_eq!(long.duration_ms, 1500);
    }

    #[test]
    fn command_kinds() {
        assert_eq!(
            Command::RunAction(RunAction::back()).kind_str(),
            "run_action"
        );
        assert_eq!(
            Command::Screenshot {
                label: "x".to_string()
            }
            .kind_str(),
            "screenshot"
        );
    }

    #[test]
    fn error_codes_render_stable_names() {
        assert_eq!(WireErrorCode::ElementNotFound.as_str(), "ELEMENT_NOT_FOUND");
        assert_eq!(WireErrorCode::Unauthenticated.as_str(), "UNAUTHENTICATED");
        assert!(WireErrorCode::Transport.is_infrastructure());
        assert!(!WireErrorCode::CheckFailed.is_infrastructure());
        assert_eq!(WireErrorCode::ALL.len(), 16);
        for code in WireErrorCode::ALL {
            assert!(!code.as_str().is_empty());
        }
    }
}
