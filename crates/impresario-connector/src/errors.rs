//! Connector failures mapped onto the wire `ErrorInfo` contract (spec 7.1).

use std::collections::BTreeMap;

use impresario_core::command::WireErrorCode;
use impresario_proto::v1 as pb;

/// One connector failure: a wire error code, a human message and optional
/// `details` (spec 5.6: `obscured_by`, `option`, `scroll_attempts`, ...).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ConnectorError {
    pub code: WireErrorCode,
    pub message: String,
    pub details: BTreeMap<String, String>,
}

impl ConnectorError {
    #[must_use]
    pub fn new(code: WireErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    #[must_use]
    pub fn element_not_found(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::ElementNotFound, message)
    }

    #[must_use]
    pub fn not_interactable(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::NotInteractable, message)
    }

    #[must_use]
    pub fn check_failed(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::CheckFailed, message)
    }

    #[must_use]
    pub fn invalid_selector(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::InvalidSelector, message)
    }

    #[must_use]
    pub fn unsupported_action(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::UnsupportedAction, message)
    }

    #[must_use]
    pub fn device_unavailable(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::DeviceUnavailable, message)
    }

    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::Protocol, message)
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::Internal, message)
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(WireErrorCode::Cancelled, "the command was cancelled")
    }

    #[must_use]
    pub fn focus_unreachable(message: impl Into<String>) -> Self {
        Self::new(WireErrorCode::FocusUnreachable, message)
    }

    /// The wire `ErrorInfo` for this failure.
    #[must_use]
    pub fn to_proto(&self) -> pb::ErrorInfo {
        pb::ErrorInfo {
            code: error_code_to_proto(self.code) as i32,
            message: self.message.clone(),
            details: self
                .details
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }
}

/// Mirror of the master-side mapping (clients::convert); the connector must
/// not depend on `impresario-clients` (spec 3.2).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_details_into_proto() {
        let error = ConnectorError::not_interactable("obscured")
            .with_detail("obscured_by", "div#overlay")
            .with_detail("selector", "#go");
        let proto = error.to_proto();
        assert_eq!(proto.code, pb::ErrorCode::NotInteractable as i32);
        assert_eq!(
            proto.details.get("obscured_by").map(String::as_str),
            Some("div#overlay")
        );
        assert_eq!(error.to_string(), "obscured");
    }

    #[test]
    fn every_error_code_has_a_proto_mapping() {
        for code in WireErrorCode::ALL {
            assert_ne!(error_code_to_proto(code) as i32, 0, "{code:?}");
        }
    }
}
