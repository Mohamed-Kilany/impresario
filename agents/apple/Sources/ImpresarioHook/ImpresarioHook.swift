import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Reference Swift SDK for Impresario's optional in-app test hook (D-41).
///
/// Debug-only and opt-in: the app owner adds this package, calls `start()`
/// once, and reports player/DRM/showing state whenever it changes. The agent
/// reads the latest snapshot from loopback and treats it as AUTHORITATIVE
/// evidence. Missing reports never fail a run; they leave a claim
/// UNVERIFIABLE.
///
/// JSON contract (docs/PROBES.md):
/// `{"media": {...}, "drm": {...}, "showing": {"decoded":true,"source":"…"}}`
public final class ImpresarioHook: @unchecked Sendable {
    public static let shared = ImpresarioHook()

    public struct Media: Codable, Sendable, Equatable {
        public var state: String?
        public var positionSeconds: Double?
        public var durationSeconds: Double?
        public var rate: Double?
        public var volume: Double?
        public var muted: Bool?
        public var widthPx: Int?
        public var heightPx: Int?
        public var audioTrack: String?
        public var subtitleTrack: String?
        public var error: String?
        public var decoded: Bool?
        public var source: String?

        public init(
            state: String? = nil,
            positionSeconds: Double? = nil,
            durationSeconds: Double? = nil,
            rate: Double? = nil,
            volume: Double? = nil,
            muted: Bool? = nil,
            widthPx: Int? = nil,
            heightPx: Int? = nil,
            audioTrack: String? = nil,
            subtitleTrack: String? = nil,
            error: String? = nil,
            decoded: Bool? = nil,
            source: String? = nil
        ) {
            self.state = state
            self.positionSeconds = positionSeconds
            self.durationSeconds = durationSeconds
            self.rate = rate
            self.volume = volume
            self.muted = muted
            self.widthPx = widthPx
            self.heightPx = heightPx
            self.audioTrack = audioTrack
            self.subtitleTrack = subtitleTrack
            self.error = error
            self.decoded = decoded
            self.source = source
        }
    }

    public struct Drm: Codable, Sendable, Equatable {
        public var enabled: Bool?
        public var provider: String?
        public var license: String?
        public var keyStatus: String?
        public var securityLevel: String?
        public var hdcp: String?

        public init(
            enabled: Bool? = nil,
            provider: String? = nil,
            license: String? = nil,
            keyStatus: String? = nil,
            securityLevel: String? = nil,
            hdcp: String? = nil
        ) {
            self.enabled = enabled
            self.provider = provider
            self.license = license
            self.keyStatus = keyStatus
            self.securityLevel = securityLevel
            self.hdcp = hdcp
        }
    }

    public struct Showing: Codable, Sendable, Equatable {
        public var decoded: Bool?
        public var source: String?
        public var piece: String?

        public init(decoded: Bool? = nil, source: String? = nil, piece: String? = nil) {
            self.decoded = decoded
            self.source = source
            self.piece = piece
        }
    }

    private struct Payload: Codable {
        let media: Media?
        let drm: Drm?
        let showing: Showing?
    }

    private let lock = NSLock()
    private var endpoint: URL?
    private var session: URLSession?
    private var inFlight = false

    private init() {}

    /// Starts reporting to the agent on `127.0.0.1:<port>`.
    public func start(port: UInt16 = 47_831) {
        guard let url = URL(string: "http://127.0.0.1:\(port)/hook") else { return }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 1
        configuration.timeoutIntervalForResource = 1
        lock.lock()
        endpoint = url
        session = URLSession(configuration: configuration)
        lock.unlock()
    }

    public func stop() {
        lock.lock()
        session?.invalidateAndCancel()
        session = nil
        endpoint = nil
        lock.unlock()
    }

    /// Reports a complete snapshot. Coalesces concurrent calls: only one POST
    /// is in flight, which keeps a 250 ms playback timer cheap.
    public func report(media: Media? = nil, drm: Drm? = nil, showing: Showing? = nil) {
        lock.lock()
        guard let endpoint, let session, !inFlight else {
            lock.unlock()
            return
        }
        guard let body = try? JSONEncoder().encode(Payload(media: media, drm: drm, showing: showing)) else {
            lock.unlock()
            return
        }
        inFlight = true
        lock.unlock()

        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let task = session.dataTask(with: request) { [weak self] _, _, _ in
            guard let self else { return }
            self.lock.lock()
            self.inFlight = false
            self.lock.unlock()
        }
        task.resume()
    }
}
