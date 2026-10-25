import AgentCore
import Foundation
import Network

/// Receives the optional in-app hook's JSON snapshots over loopback (D-41).
///
/// The hook is debug-only and opt-in: the app posts to
/// `http://127.0.0.1:47831/hook`; the runner keeps the newest snapshot. A
/// missing hook is not an error - probes simply stay UNVERIFIABLE.
final class HookReceiver: @unchecked Sendable {
    static let shared = HookReceiver()
    static let port: UInt16 = 47_831

    struct Showing: Decodable {
        let decoded: Bool?
        let source: String?
        let piece: String?
    }

    struct Payload: Decodable {
        let media: MediaSnapshot?
        let drm: DrmSnapshot?
        let showing: Showing?
    }

    private let lock = NSLock()
    private var latest: Payload?
    private var listener: NWListener?
    private var started = false

    private init() {}

    func start() {
        lock.lock()
        defer { lock.unlock() }
        guard !started else { return }
        started = true
        guard let port = NWEndpoint.Port(rawValue: Self.port) else { return }
        do {
            let listener = try NWListener(using: .tcp, on: port)
            listener.newConnectionHandler = { [weak self] connection in
                self?.accept(connection)
            }
            listener.start(queue: DispatchQueue(label: "com.impresario.hook.listener"))
            self.listener = listener
        } catch {
            // Port busy or sandbox denied: the hook stays unavailable and the
            // probe reports UNVERIFIABLE rather than failing the run.
            started = false
        }
    }

    func stop() {
        lock.lock()
        listener?.cancel()
        listener = nil
        started = false
        lock.unlock()
    }

    func snapshot() -> Payload? {
        lock.lock()
        defer { lock.unlock() }
        return latest
    }

    private func accept(_ connection: NWConnection) {
        connection.start(queue: DispatchQueue(label: "com.impresario.hook.connection"))
        receive(connection, buffer: Data())
    }

    private func receive(_ connection: NWConnection, buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) {
            [weak self] data, _, complete, error in
            guard let self else {
                connection.cancel()
                return
            }
            var buffer = buffer
            if let data {
                buffer.append(data)
            }
            if let body = Self.httpBody(buffer) {
                if let payload = try? JSONDecoder().decode(Payload.self, from: body) {
                    self.lock.lock()
                    self.latest = payload
                    self.lock.unlock()
                }
                let response = Data("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".utf8)
                connection.send(content: response, completion: .contentProcessed { _ in
                    connection.cancel()
                })
                return
            }
            if complete || error != nil || buffer.count > 1_048_576 {
                connection.cancel()
                return
            }
            self.receive(connection, buffer: buffer)
        }
    }

    /// Returns the body once the request is complete, else nil.
    private static func httpBody(_ data: Data) -> Data? {
        let marker = Data("\r\n\r\n".utf8)
        guard let headerEnd = data.range(of: marker) else { return nil }
        let header = data.subdata(in: 0..<headerEnd.lowerBound)
        guard let text = String(data: header, encoding: .utf8) else { return nil }
        var length = 0
        for line in text.split(separator: "\r\n") {
            let parts = line.split(separator: ":", maxSplits: 1)
            if parts.count == 2, parts[0].lowercased() == "content-length" {
                length = Int(parts[1].trimmingCharacters(in: .whitespaces)) ?? 0
            }
        }
        let bodyStart = headerEnd.upperBound
        guard data.count >= bodyStart + length else { return nil }
        return data.subdata(in: bodyStart..<(bodyStart + length))
    }
}
