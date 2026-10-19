import AgentCore
import Foundation

#if canImport(Darwin)
/// Advertises the agent as `_impresario._tcp` on the local network (spec 12.4),
/// so a master can find an iOS/tvOS device without a configured
/// `agent_address`.
///
/// The publisher owns the `NetService`, publishes on the main run loop (the
/// runner's run loop keeps spinning while the test waits) and re-publishes on
/// name collisions; `stop()` releases it. Darwin-only: a Linux `swift test`
/// uses the stub below and never touches the local network.
public final class BonjourPublisher: NSObject, NetServiceDelegate {
    private let name: String
    private let port: Int
    private let domain: String
    private let log: LogBus
    private let txt: [String: String]
    private var service: NetService?

    /// `txt` is advertised as the TXT record (spec 12.4): `platform`,
    /// `protocol` and `agent` let a master filter several agents on one network
    /// without connecting to each of them.
    public init(
        name: String,
        port: Int,
        log: LogBus,
        domain: String = "local.",
        txt: [String: String] = [:]
    ) {
        self.name = name
        self.port = port
        self.domain = domain
        self.log = log
        self.txt = txt
    }

    /// Starts advertising; safe to call again after a failed publish.
    public func publish() {
        stop()
        let service = NetService(
            domain: domain,
            type: BonjourPublisher.serviceType,
            name: name,
            port: Int32(port)
        )
        service.delegate = self
        service.schedule(in: RunLoop.main, forMode: .common)
        if !txt.isEmpty {
            let record = txt.mapValues { Data($0.utf8) }
            if !service.setTXTRecord(NetService.data(fromTXTRecord: record)) {
                log.log(.warn, "agent", "Bonjour refused the TXT record \(txt)")
            }
        }
        self.service = service
        service.publish()
        log.log(
            .info,
            "agent",
            "advertising '\(name)' as \(BonjourPublisher.serviceType) port \(port) on \(domain)"
        )
    }

    public func stop() {
        guard let service else { return }
        service.stop()
        service.delegate = nil
        self.service = nil
    }

    /// The Bonjour service type (also the `NSBonjourServices` entry the app
    /// must declare, spec 12.4).
    public static let serviceType = "_impresario._tcp."

    @objc public func netServiceDidPublish(_ sender: NetService) {
        log.log(.info, "agent", "Bonjour published '\(sender.name)' on port \(sender.port)")
    }

    @objc public func netService(_ sender: NetService, didNotPublish errorDict: [String: NSNumber]) {
        log.log(.warn, "agent", "Bonjour publish failed for '\(sender.name)': \(errorDict)")
    }
}
#else
/// Advertising stub for platforms without `NetService` (`swift test` on
/// Linux): it only logs, so the loopback tests never touch the local network.
public final class BonjourPublisher {
    public static let serviceType = "_impresario._tcp."

    public init(
        name: String,
        port: Int,
        log: LogBus,
        domain: String = "local.",
        txt: [String: String] = [:]
    ) {
        log.log(
            .warn,
            "agent",
            "Bonjour advertisement is unavailable on this platform; the master needs an explicit "
                + "agent_address (name '\(name)', port \(port), domain \(domain), txt \(txt))"
        )
    }

    public func publish() {}
    public func stop() {}
}
#endif
