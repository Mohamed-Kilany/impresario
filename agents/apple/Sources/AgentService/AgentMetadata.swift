import GRPC
import NIOHPACK

/// Protocol metadata names (spec 7.2.1) and the client-side `CallOptions`
/// helper used by the loopback tests and by tools embedding this package.
public enum AgentMetadata {
    /// Names the session returned by an accepted handshake.
    public static let sessionIDHeader = "x-impresario-session-id"

    /// The shared token, when the agent is configured with one.
    public static let tokenHeader = "x-impresario-token"

    /// Request metadata for a call: the session id and, when configured, the
    /// token (spec 7.2.1).
    public static func callOptions(
        sessionID: String? = nil,
        token: String? = nil,
        timeout: TimeLimit = .none
    ) -> CallOptions {
        var options = CallOptions()
        options.timeLimit = timeout
        var headers = HPACKHeaders()
        if let sessionID, !sessionID.isEmpty {
            headers.add(name: sessionIDHeader, value: sessionID)
        }
        if let token, !token.isEmpty {
            headers.add(name: tokenHeader, value: token)
        }
        options.customMetadata = headers
        return options
    }
}
