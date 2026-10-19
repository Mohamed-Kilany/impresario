import Foundation

/// An executor-side failure that maps onto the wire `ErrorInfo` (spec 11.1).
///
/// Instances only ever surface as gRPC not-ok states for transport-level
/// problems (authentication, wrong session); everything that happens inside a
/// session is answered with `ok=false` in the `CommandResult` (spec 7.2 rule 6).
public struct AgentError: Error, Sendable, Equatable {
    public let report: AgentErrorReport

    public init(code: AgentErrorCode, message: String, details: [String: String] = [:]) {
        self.report = AgentErrorReport(code: code, message: message, details: details)
    }

    public var code: AgentErrorCode { report.code }
    public var message: String { report.message }
    public var details: [String: String] { report.details }

    // ------------------------------------------------------------------
    // Factories (mirror agents/android .../Errors.kt)
    // ------------------------------------------------------------------

    public static func elementNotFound(_ message: String, _ selector: String? = nil) -> AgentError {
        AgentError(
            code: .elementNotFound,
            message: message,
            details: selector.map { ["selector": $0] } ?? [:]
        )
    }

    public static func notInteractable(_ message: String, _ details: [String: String] = [:]) -> AgentError {
        AgentError(code: .notInteractable, message: message, details: details)
    }

    public static func focusUnreachable(_ message: String, _ details: [String: String]) -> AgentError {
        AgentError(code: .focusUnreachable, message: message, details: details)
    }

    public static func unsupportedAction(_ message: String) -> AgentError {
        AgentError(code: .unsupportedAction, message: message)
    }

    public static func invalidSelector(_ message: String) -> AgentError {
        AgentError(code: .invalidSelector, message: message)
    }

    public static func checkFailed(
        _ message: String,
        expected: String,
        observed: String,
        matchedCount: Int,
        selector: String
    ) -> AgentError {
        AgentError(
            code: .checkFailed,
            message: message,
            details: [
                "selector": selector,
                "expected": expected,
                "observed": observed,
                "matched_count": String(matchedCount),
            ]
        )
    }

    public static func protocolError(_ message: String) -> AgentError {
        AgentError(code: .protocolViolation, message: message)
    }

    public static func internalError(_ message: String) -> AgentError {
        AgentError(code: .internalError, message: message)
    }

    public static func appCrashed(_ message: String) -> AgentError {
        AgentError(code: .appCrashed, message: message)
    }

    public static func cancelled() -> AgentError {
        AgentError(code: .cancelled, message: "the command was cancelled")
    }
}
