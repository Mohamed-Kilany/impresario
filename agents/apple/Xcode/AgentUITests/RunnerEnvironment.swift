import AgentCore
import Foundation

/// Everything the runner reads from its surroundings: the platform of the
/// bundle it was compiled into, the xctestrun environment and the device name
/// (spec 12.1). Kept apart from the test so the loopback tests and the
/// `xcodebuild` invocation agree on one definition.
enum RunnerEnvironment {
    /// The platform of this UI-test bundle. The bundle identifiers end in
    /// `-iOS` / `-tvOS` (`agents/apple/project.yml`), which is what
    /// `AppleTransport` launches; the compile-time check is the fallback.
    static func platform(for bundle: Bundle = .main) -> PlatformKind {
        if let identifier = bundle.bundleIdentifier, identifier.hasSuffix("-tvOS") {
            return .tvos
        }
        if let identifier = bundle.bundleIdentifier, identifier.hasSuffix("-iOS") {
            return .ios
        }
        #if os(tvOS)
        return .tvos
        #else
        return .ios
        #endif
    }

    /// `IMPRESARIO_AGENT_TOKEN`: the shared token the master must send
    /// (spec 7.2.1). Empty (the default) disables the check.
    static func token(
        _ environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> String {
        environment["IMPRESARIO_AGENT_TOKEN"] ?? ""
    }

    /// `IMPRESARIO_NO_BONJOUR=1` disables the `_impresario._tcp` advertisement —
    /// the escape hatch when a device's local-network permission cannot be
    /// granted and the master connects to a fixed `agent_address` instead
    /// (README "Troubleshooting").
    static func advertisesBonjour(
        _ environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Bool {
        let raw = environment["IMPRESARIO_NO_BONJOUR"]?.trimmingCharacters(in: .whitespaces).lowercased()
        return !(raw == "1" || raw == "true" || raw == "yes")
    }

    /// A Bonjour name that identifies the device on a network with several
    /// agents (`impresario-tvos-Wohnzimmer`), so `dns-sd -B _impresario._tcp`
    /// output is readable.
    static func bonjourName(deviceName: String, platform: PlatformKind) -> String {
        let host = deviceName.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleaned = host.isEmpty ? "device" : sanitize(host)
        return "impresario-\(platform.rawValue)-\(cleaned)"
    }

    /// `ProcessInfo`'s host name, which is the device's user-visible name on
    /// iOS/tvOS (`iPhone von Max`).
    static func deviceName() -> String {
        ProcessInfo.processInfo.hostName
    }

    /// Bonjour service names must not contain `.` or `:`.
    private static func sanitize(_ raw: String) -> String {
        let allowed = raw.map { character -> Character in
            if character == "." || character == ":" || character == "/" {
                return "-"
            }
            return character
        }
        let cleaned = String(allowed).trimmingCharacters(in: .whitespaces)
        return cleaned.isEmpty ? "device" : cleaned
    }

    /// The agent's HTTP-ish summary for the runner log.
    static func summary(platform: PlatformKind, port: Int, bonjour: Bool, token: Bool) -> String {
        let lines = [
            "impresario agent: platform=\(platform.rawValue) port=\(port) "
                + "bonjour=\(bonjour ? "on" : "off") token=\(token ? "required" : "off")",
            "impresario agent: reachable at http://<device-ip>:\(port) (grpc) — "
                + "pass agent_address=<device-ip>:\(port) to the master for WLAN runs",
            "impresario agent: addresses \(localAddresses().joined(separator: ", "))",
        ]
        return lines.joined(separator: "\n")
    }

    /// The device's IPv4 addresses (`en0`/`en1`), for the log line above.
    static func localAddresses() -> [String] {
        var addresses: [String] = []
        var head: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&head) == 0, let first = head else {
            return addresses
        }
        defer { freeifaddrs(head) }
        for pointer in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let interface = pointer.pointee
            guard let address = interface.ifa_addr, address.pointee.sa_family == UInt8(AF_INET) else {
                continue
            }
            let name = String(cString: interface.ifa_name)
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            let result = getnameinfo(
                address,
                socklen_t(address.pointee.sa_len),
                &host,
                socklen_t(host.count),
                nil,
                0,
                NI_NUMERICHOST
            )
            guard result == 0 else {
                continue
            }
            let text = host.withUnsafeBufferPointer { buffer -> String in
                guard let base = buffer.baseAddress else { return "" }
                return String(cString: base)
            }
            if !text.isEmpty {
                addresses.append("\(name)=\(text)")
            }
        }
        return addresses
    }
}
