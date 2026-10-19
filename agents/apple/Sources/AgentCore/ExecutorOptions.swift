import Foundation

/// Options of one agent session (from the handshake `options` map, spec 7.1).
public struct ExecutorOptions: Sendable, Equatable {
    public var platform: PlatformKind
    public var appId: String
    public var pollIntervalMs: Int
    public var scrollMaxAttempts: Int
    public var focusMaxSteps: Int
    public var animationsDisabled: Bool
    public var clearAppDataOnRestart: Bool

    public init(
        platform: PlatformKind,
        appId: String = "",
        pollIntervalMs: Int = 250,
        scrollMaxAttempts: Int = 10,
        focusMaxSteps: Int = 15,
        animationsDisabled: Bool = false,
        clearAppDataOnRestart: Bool = false
    ) {
        self.platform = platform
        self.appId = appId
        self.pollIntervalMs = max(1, pollIntervalMs)
        self.scrollMaxAttempts = max(1, scrollMaxAttempts)
        self.focusMaxSteps = max(1, focusMaxSteps)
        self.animationsDisabled = animationsDisabled
        self.clearAppDataOnRestart = clearAppDataOnRestart
    }

    /// Parses the master's `options` map; unknown keys are ignored (the map
    /// also carries device options this agent does not act on).
    public static func from(
        options: [String: String],
        platform: PlatformKind,
        appId: String
    ) -> ExecutorOptions {
        var result = ExecutorOptions(platform: platform, appId: appId)
        if let value = options["poll_interval_ms"], let parsed = Int(value), parsed > 0 {
            result.pollIntervalMs = parsed
        }
        if let value = options["scroll_max_attempts"], let parsed = Int(value), parsed > 0 {
            result.scrollMaxAttempts = parsed
        }
        if let value = options["focus_max_steps"], let parsed = Int(value), parsed > 0 {
            result.focusMaxSteps = parsed
        }
        if let value = options["animations_disabled"] {
            result.animationsDisabled = parseBool(value)
        }
        if let value = options["clear_app_data"] {
            result.clearAppDataOnRestart = parseBool(value)
        }
        return result
    }

    private static func parseBool(_ value: String) -> Bool {
        switch value.trimmingCharacters(in: .whitespaces).lowercased() {
        case "1", "true", "yes", "on": return true
        default: return false
        }
    }
}
