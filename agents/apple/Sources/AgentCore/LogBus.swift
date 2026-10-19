import Foundation

/// A live `StreamLogs` subscription. Cancel it when the stream ends so the bus
/// stops buffering lines for a gone client.
public final class LogSubscription: @unchecked Sendable {
    public let stream: AsyncStream<LogLine>

    private let lock = NSLock()
    private var cancelled = false
    private let onCancel: () -> Void

    init(stream: AsyncStream<LogLine>, onCancel: @escaping () -> Void) {
        self.stream = stream
        self.onCancel = onCancel
    }

    public func cancel() {
        lock.lock()
        if cancelled {
            lock.unlock()
            return
        }
        cancelled = true
        lock.unlock()
        onCancel()
    }
}

/// Fan-out for `StreamLogs` (spec 12.3): every subscriber receives every line
/// at or above its minimum level. Thread-safe: the gRPC event loop and the
/// command tasks log concurrently.
public final class LogBus: @unchecked Sendable {
    private let lock = NSLock()
    private var subscribers: [UUID: (minLevel: LogLevel, continuation: AsyncStream<LogLine>.Continuation)] = [:]
    private let now: @Sendable () -> Int64

    public init(now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }) {
        self.now = now
    }

    /// Emits one line to every interested subscriber.
    public func log(_ level: LogLevel, _ source: String, _ message: String) {
        let line = LogLine(unixMs: now(), level: level, source: source, message: message)
        lock.lock()
        let snapshot = subscribers
        lock.unlock()
        for (_, subscriber) in snapshot where level >= subscriber.minLevel {
            subscriber.continuation.yield(line)
        }
    }

    /// Opens a log stream at `minLevel`; cancel the subscription when the
    /// stream ends.
    public func subscribe(minLevel: LogLevel) -> LogSubscription {
        let id = UUID()
        let (stream, continuation) = AsyncStream<LogLine>.makeStream(
            bufferingPolicy: .bufferingNewest(1024)
        )
        lock.lock()
        subscribers[id] = (minLevel, continuation)
        lock.unlock()
        return LogSubscription(stream: stream) { [weak self] in
            guard let self else { return }
            self.lock.lock()
            let removed = self.subscribers.removeValue(forKey: id)
            self.lock.unlock()
            removed?.continuation.finish()
        }
    }

    /// Number of open subscriptions (tests and diagnostics).
    public var subscriberCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return subscribers.count
    }
}
