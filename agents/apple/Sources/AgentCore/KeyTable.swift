import Foundation

/// Where a `key` command goes on the running platform (spec 6.2).
public enum KeyAction: Equatable {
    case remote(RemoteButton)
    case home
    case unsupported(String)
}

/// The remote/hardware key table (spec 6.2). One source file per executor.
public enum KeyTable {
    /// Resolves a wire key to a driver action, or an explanatory
    /// `UNSUPPORTED_ACTION` message when the platform has no such key.
    public static func resolve(_ key: RemoteKey, platform: PlatformKind) -> KeyAction {
        switch platform {
        case .tvos:
            return resolveTv(key)
        case .ios:
            if key == .home {
                return .home
            }
            return .unsupported("key '\(key.rawValue)' is not supported on ios (only 'home' is; spec 6.2)")
        }
    }

    private static func resolveTv(_ key: RemoteKey) -> KeyAction {
        switch key {
        case .ok:
            return .remote(.select)
        case .back:
            return .remote(.menu)
        case .up:
            return .remote(.up)
        case .down:
            return .remote(.down)
        case .left:
            return .remote(.left)
        case .right:
            return .remote(.right)
        case .home:
            return .home
        case .playPause:
            return .remote(.playPause)
        case .enter:
            return unsupported(key, "the tvOS remote has no enter key (use ok)")
        case .menu:
            return unsupported(key, "the tvOS remote has no menu key (use back)")
        case .play, .pause:
            return unsupported(key, "the tvOS remote has no separate play/pause keys (use playpause)")
        case .rewind, .fastForward:
            return unsupported(key, "the XCUITest remote has no rewind/fast-forward buttons")
        case .red, .green, .yellow, .blue:
            return unsupported(key, "the Apple TV remote has no colour keys")
        case .backspace:
            return unsupported(key, "the tvOS remote has no backspace key")
        }
    }

    /// `UNSUPPORTED_ACTION` text for tvOS: names the wire key (reports show it
    /// verbatim, spec 7.2 rule 6) and why the platform has no equivalent.
    private static func unsupported(_ key: RemoteKey, _ why: String) -> KeyAction {
        .unsupported("key '\(key.rawValue)' is not supported on tvos: \(why); spec 6.2")
    }
}
