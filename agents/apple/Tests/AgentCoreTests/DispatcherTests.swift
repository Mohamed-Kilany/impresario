import XCTest
@testable import AgentCore

final class DispatcherTests: XCTestCase {
    private struct Harness {
        let driver: FakeUIDriver
        let clock: TestClock
        let dispatcher: Dispatcher
        let recorder: EventRecorder
        let gate: GateSleeper
    }

    private func makeHarness(
        platform: PlatformKind = .ios,
        pollIntervalMs: Int = 50,
        heartbeatIntervalMs: Int = 0,
        gated: Bool = true,
        findHandler: ((SelectorSpec, Int) -> [FakeElement]?)? = nil
    ) async -> Harness {
        let driver = FakeUIDriver()
        driver.findHandler = findHandler
        let clock = TestClock()
        let gate = GateSleeper()
        let sleeper: (Int) async throws -> Void
        if gated {
            sleeper = { milliseconds in
                clock.advance(milliseconds)
                await gate.wait()
            }
        } else {
            sleeper = { milliseconds in
                clock.advance(milliseconds)
                try await Task.sleep(nanoseconds: 1_000_000)
            }
        }
        let executor = CommandExecutor(
            driver: driver,
            options: ExecutorOptions(
                platform: platform,
                appId: "com.example.app",
                pollIntervalMs: pollIntervalMs
            ),
            now: { clock.now },
            sleep: sleeper
        )
        let dispatcher = Dispatcher(executor: executor, heartbeatIntervalMs: heartbeatIntervalMs)
        let recorder = EventRecorder()
        await dispatcher.start(emit: { event in recorder.record(event) })
        return Harness(driver: driver, clock: clock, dispatcher: dispatcher, recorder: recorder, gate: gate)
    }

    private func clickCommand(_ id: String, target: String = "a") -> AgentCommand {
        AgentCommand(
            id: id,
            timeoutMs: 5_000,
            body: .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, target)))
        )
    }

    private func screenshotCommand(_ id: String) -> AgentCommand {
        AgentCommand(id: id, timeoutMs: 5_000, body: .screenshot)
    }

    // MARK: - ordering and admission

    func testCommandsRunInOrderWithExactlyOneResultEach() async throws {
        let harness = await makeHarness(findHandler: { _, call in
            call >= 2 ? [FakeElement(id: "a")] : []
        })

        await harness.dispatcher.enqueue(clickCommand("a"))
        try await waitUntil { harness.driver.findCalls >= 1 && harness.gate.waiterCount >= 1 }

        await harness.dispatcher.enqueue(screenshotCommand("b"))
        let pending = await harness.dispatcher.pendingCount
        XCTAssertEqual(pending, 1)
        XCTAssertTrue(harness.recorder.results.isEmpty)

        harness.gate.open()
        try await waitUntil { harness.recorder.results.count == 2 }

        let results = harness.recorder.results
        XCTAssertEqual(results.map(\.commandId), ["a", "b"])
        XCTAssertEqual(results.map(\.ok), [true, true])
        XCTAssertEqual(results[1].screenshotPng, FakePNG.valid)
        let current = await harness.dispatcher.currentCommandId
        XCTAssertNil(current)
        XCTAssertEqual(harness.driver.taps, ["a"])
    }

    func testDuplicateCommandIdIsRejectedWithoutRunning() async throws {
        let harness = await makeHarness()

        await harness.dispatcher.enqueue(clickCommand("a"))
        try await waitUntil { harness.driver.findCalls >= 1 }

        await harness.dispatcher.enqueue(screenshotCommand("a"))
        try await waitUntil { harness.recorder.results.count == 1 }

        let duplicate = harness.recorder.results[0]
        XCTAssertEqual(duplicate.commandId, "a")
        XCTAssertFalse(duplicate.ok)
        XCTAssertEqual(duplicate.error?.code, .protocolViolation)
        XCTAssertTrue(duplicate.error?.message.contains("duplicate") == true)
        let current = await harness.dispatcher.currentCommandId
        XCTAssertEqual(current, "a")

        await harness.dispatcher.shutdown()
        harness.gate.open()
        try await waitUntil { harness.recorder.closedReasons == ["shutdown"] }
    }

    func testEmptyCommandIdIsRejected() async throws {
        let harness = await makeHarness()

        await harness.dispatcher.enqueue(screenshotCommand(""))
        try await waitUntil { harness.recorder.results.count == 1 }

        let result = harness.recorder.results[0]
        XCTAssertEqual(result.commandId, "")
        XCTAssertFalse(result.ok)
        XCTAssertEqual(result.error?.code, .protocolViolation)
        XCTAssertTrue(result.error?.message.contains("id is empty") == true)
    }

    func testRejectedCommandAnswersOneResultAndRemembersTheId() async throws {
        let harness = await makeHarness()

        // A command the bridge could not parse (spec 7.2 rule 4): the caller
        // gets exactly one result carrying the specific error.
        await harness.dispatcher.reject(id: "bad", error: .invalidSelector("css is not supported"))
        try await waitUntil { harness.recorder.results.count == 1 }

        let result = harness.recorder.results[0]
        XCTAssertEqual(result.commandId, "bad")
        XCTAssertFalse(result.ok)
        XCTAssertEqual(result.error?.code, .invalidSelector)
        XCTAssertEqual(result.error?.message, "css is not supported")
        XCTAssertEqual(result.durationMs, 0)

        // The id is now taken, so a duplicate is reported without running.
        await harness.dispatcher.reject(id: "bad", error: .invalidSelector("again"))
        try await waitUntil { harness.recorder.results.count == 2 }
        XCTAssertEqual(harness.recorder.results[1].commandId, "bad")
        XCTAssertEqual(harness.recorder.results[1].error?.code, .protocolViolation)
        XCTAssertTrue(harness.recorder.results[1].error?.message.contains("duplicate") == true)
    }

    // MARK: - cancellation

    func testCancelAcknowledgesThenCancelsTheCommand() async throws {
        let harness = await makeHarness(pollIntervalMs: 30_000, gated: false)

        await harness.dispatcher.enqueue(clickCommand("a"))
        try await waitUntil { await harness.dispatcher.currentCommandId == "a" }

        await harness.dispatcher.enqueue(
            AgentCommand(id: "c2", timeoutMs: 1_000, body: .cancel(target: "a"))
        )
        try await waitUntil { harness.recorder.results.count == 2 }

        let results = harness.recorder.results
        XCTAssertEqual(results[0].commandId, "c2")
        XCTAssertTrue(results[0].ok, results[0].error?.message ?? "")
        XCTAssertEqual(results[1].commandId, "a")
        XCTAssertFalse(results[1].ok)
        XCTAssertEqual(results[1].error?.code, .cancelled)
    }

    func testCancelOfAnUnknownCommandIsAProtocolViolation() async throws {
        let harness = await makeHarness()

        await harness.dispatcher.enqueue(
            AgentCommand(id: "c1", timeoutMs: 1_000, body: .cancel(target: "nope"))
        )
        try await waitUntil { harness.recorder.results.count == 1 }

        let result = harness.recorder.results[0]
        XCTAssertFalse(result.ok)
        XCTAssertEqual(result.error?.code, .protocolViolation)
        XCTAssertTrue(result.error?.message.contains("no in-flight command") == true)
    }

    func testCancelLetsTheQueuedCommandRun() async throws {
        let harness = await makeHarness(findHandler: { _, call in
            call >= 2 ? [FakeElement(id: "a")] : []
        })

        await harness.dispatcher.enqueue(clickCommand("a"))
        await harness.dispatcher.enqueue(screenshotCommand("b"))
        try await waitUntil { harness.driver.findCalls >= 1 && harness.gate.waiterCount >= 1 }

        await harness.dispatcher.enqueue(
            AgentCommand(id: "c2", timeoutMs: 1_000, body: .cancel(target: "a"))
        )
        harness.gate.open()
        try await waitUntil { harness.recorder.results.count == 3 }

        let results = harness.recorder.results
        XCTAssertEqual(results[0].commandId, "c2")
        XCTAssertTrue(results[0].ok)
        XCTAssertEqual(results[1].commandId, "a")
        XCTAssertEqual(results[1].error?.code, .cancelled)
        XCTAssertEqual(results[2].commandId, "b")
        XCTAssertTrue(results[2].ok)
    }

    // MARK: - closing

    func testClientClosedWaitsForInFlightWork() async throws {
        let harness = await makeHarness(findHandler: { _, call in
            call >= 2 ? [FakeElement(id: "a")] : []
        })

        await harness.dispatcher.enqueue(clickCommand("a"))
        await harness.dispatcher.enqueue(screenshotCommand("b"))
        try await waitUntil { harness.driver.findCalls >= 1 && harness.gate.waiterCount >= 1 }

        await harness.dispatcher.clientClosed()
        XCTAssertTrue(harness.recorder.closedReasons.isEmpty)

        harness.gate.open()
        try await waitUntil { harness.recorder.closedReasons == ["client_closed"] }

        XCTAssertEqual(harness.recorder.results.map(\.commandId), ["a", "b"])
        XCTAssertEqual(harness.recorder.results.map(\.ok), [true, true])
        let closedIndex = harness.recorder.all.firstIndex { event in
            if case .closed = event { return true }
            return false
        }
        XCTAssertEqual(closedIndex, harness.recorder.all.count - 1)
    }

    func testClientClosedOnAnIdleSessionClosesImmediately() async throws {
        let harness = await makeHarness()

        await harness.dispatcher.clientClosed()
        XCTAssertEqual(harness.recorder.closedReasons, ["client_closed"])

        await harness.dispatcher.enqueue(screenshotCommand("late"))
        try await Task.sleep(nanoseconds: 30_000_000)
        XCTAssertTrue(harness.recorder.results.isEmpty)
    }

    func testShutdownCancelsTheInFlightCommandAndCloses() async throws {
        let harness = await makeHarness(pollIntervalMs: 30_000, gated: false)

        await harness.dispatcher.enqueue(clickCommand("a"))
        try await waitUntil { await harness.dispatcher.currentCommandId == "a" }

        await harness.dispatcher.shutdown()
        try await waitUntil { harness.recorder.closedReasons == ["shutdown"] }

        XCTAssertEqual(harness.recorder.results.count, 1)
        XCTAssertEqual(harness.recorder.results[0].error?.code, .cancelled)

        await harness.dispatcher.shutdown()
        try await Task.sleep(nanoseconds: 20_000_000)
        XCTAssertEqual(harness.recorder.closedReasons, ["shutdown"])
    }

    // MARK: - heartbeats

    func testHeartbeatsFlowWhileIdleAndDuringWork() async throws {
        let harness = await makeHarness(
            pollIntervalMs: 30_000,
            heartbeatIntervalMs: 20,
            gated: true
        )
        defer {
            Task { await harness.dispatcher.shutdown() }
        }

        try await waitUntil { harness.recorder.heartbeats.count >= 2 }
        XCTAssertEqual(harness.recorder.heartbeats[0].seq, 1)
        XCTAssertEqual(harness.recorder.heartbeats[1].seq, 2)
        XCTAssertEqual(harness.recorder.heartbeats[0].currentCommandId, "")

        // The gated sleeper keeps the command in flight (it waits for an
        // element that never appears), so heartbeats must report it.
        await harness.dispatcher.enqueue(clickCommand("a"))
        try await waitUntil {
            harness.recorder.heartbeats.contains { $0.currentCommandId == "a" }
        }

        harness.gate.open()
        await harness.dispatcher.shutdown()
        try await waitUntil { harness.recorder.closedReasons == ["shutdown"] }
        let countAtClose = harness.recorder.heartbeats.count
        try await Task.sleep(nanoseconds: 60_000_000)
        XCTAssertEqual(harness.recorder.heartbeats.count, countAtClose)
    }
}
