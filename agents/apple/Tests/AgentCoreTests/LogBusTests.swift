import XCTest
@testable import AgentCore

final class LogBusTests: XCTestCase {
    func testFiltersByMinimumLevel() async {
        let bus = LogBus(now: { 42 })
        let subscription = bus.subscribe(minLevel: .info)
        var iterator = subscription.stream.makeAsyncIterator()

        bus.log(.debug, "agent", "hidden")
        bus.log(.info, "agent", "shown")

        let line = await iterator.next()
        XCTAssertEqual(line?.message, "shown")
        XCTAssertEqual(line?.level, .info)
        XCTAssertEqual(line?.source, "agent")
        XCTAssertEqual(line?.unixMs, 42)
        subscription.cancel()
    }

    func testFansOutToEverySubscriber() async {
        let bus = LogBus(now: { 7 })
        let first = bus.subscribe(minLevel: .debug)
        let second = bus.subscribe(minLevel: .warn)
        var firstIterator = first.stream.makeAsyncIterator()
        var secondIterator = second.stream.makeAsyncIterator()

        bus.log(.info, "executor", "info")
        bus.log(.warn, "executor", "warn")

        let firstInfo = await firstIterator.next()
        let firstWarn = await firstIterator.next()
        XCTAssertEqual(firstInfo?.message, "info")
        XCTAssertEqual(firstWarn?.message, "warn")

        let secondWarn = await secondIterator.next()
        XCTAssertEqual(secondWarn?.message, "warn")

        first.cancel()
        second.cancel()
    }

    func testCancelUnsubscribes() {
        let bus = LogBus()
        let subscription = bus.subscribe(minLevel: .debug)
        XCTAssertEqual(bus.subscriberCount, 1)
        subscription.cancel()
        subscription.cancel()
        XCTAssertEqual(bus.subscriberCount, 0)
    }

    func testCancelledSubscriptionReceivesNothing() async {
        let bus = LogBus()
        let subscription = bus.subscribe(minLevel: .debug)
        var iterator = subscription.stream.makeAsyncIterator()
        subscription.cancel()
        bus.log(.error, "agent", "after cancel")
        let line = await iterator.next()
        XCTAssertNil(line)
    }
}
