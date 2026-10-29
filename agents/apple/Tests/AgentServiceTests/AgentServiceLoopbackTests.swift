import AgentCore
import AgentService
import Foundation
import GRPC
import ImpresarioWire
import XCTest

/// End-to-end tests: a real `AgentService` on an ephemeral loopback port with
/// the generated gRPC client on the other side (spec 12.1/12.3). The fake
/// driver makes the agent autonomous, so the tests are deterministic.
final class AgentServiceLoopbackTests: XCTestCase {
    // MARK: - helpers

    /// Asserts that `operation` fails with `code` (protocol rules of 7.2.1).
    private func expectStatus(
        _ code: GRPCStatus.Code,
        file: StaticString = #filePath,
        line: UInt = #line,
        _ operation: () async throws -> Void
    ) async {
        do {
            try await operation()
            XCTFail("expected the call to fail with \(code)", file: file, line: line)
        } catch let status as GRPCStatus {
            XCTAssertEqual(status.code, code, status.message ?? "", file: file, line: line)
        } catch {
            XCTFail("expected GRPCStatus \(code), got \(error)", file: file, line: line)
        }
    }

    /// Opens a session stream, runs `commands`, and returns the first results.
    private func openSession(
        _ harness: AgentHarness,
        sessionID: String
    ) -> (
        commands: AsyncStream<Impresario_V1_Command>.Continuation,
        events: EventReader
    ) {
        let (commands, continuation) = AsyncStream<Impresario_V1_Command>.makeStream()
        let events = EventReader(
            harness.client.session(
                commands,
                callOptions: AgentMetadata.callOptions(sessionID: sessionID)
            )
        )
        return (continuation, events)
    }

    // MARK: - handshake

    func testHandshakeAdvertisesPlatformCapabilities() async throws {
        let harness = try AgentHarness.start(platform: .tvos)
        defer { harness.stop() }

        let response = try await harness.handshake()
        XCTAssertTrue(response.accepted, response.error.message)
        XCTAssertFalse(response.sessionID.isEmpty)
        XCTAssertEqual(response.protocolVersion, "1.0.0")
        XCTAssertEqual(response.agentVersion, "1.0.0")
        XCTAssertEqual(
            Set(response.capabilities),
            [
                .logs, .screenshot, .dpadFocus, .keyEvents, .appControl,
                .probeShowing, .probePlayer, .probeDrm,
            ]
        )
        XCTAssertTrue(
            response.probeSources.contains { $0.source == "app_hook" && $0.available }
        )
        XCTAssertEqual(response.device.model, "iPhone Fake")
        XCTAssertEqual(response.device.pixelRatio, 3.0)
        XCTAssertFalse(response.hasError)
    }

    func testHandshakeRejectsAnotherPlatform() async throws {
        let harness = try AgentHarness.start(platform: .ios)
        defer { harness.stop() }

        let response = try await harness.handshake(platform: .tvos)
        XCTAssertFalse(response.accepted)
        XCTAssertEqual(response.error.code, .protocol)
        XCTAssertTrue(response.error.message.contains("tvos"), response.error.message)
    }

    func testHandshakeTokenComesFromTheBodyOrTheMetadata() async throws {
        let harness = try AgentHarness.start(token: "s3cret")
        defer { harness.stop() }

        // A wrong token is rejected, whether it arrives in the body or the
        // metadata (spec 7.2.1).
        let wrongBody = try await harness.handshake(token: "nope")
        XCTAssertFalse(wrongBody.accepted)
        XCTAssertEqual(wrongBody.error.code, .unauthenticated)
        let wrongMetadata = try await harness.handshake(metadataToken: "nope")
        XCTAssertFalse(wrongMetadata.accepted)
        XCTAssertEqual(wrongMetadata.error.code, .unauthenticated)

        // The metadata token alone is accepted; the session is live afterwards.
        let accepted = try await harness.handshake(metadataToken: "s3cret")
        XCTAssertTrue(accepted.accepted, accepted.error.message)
        XCTAssertFalse(accepted.sessionID.isEmpty)
    }

    func testEveryRpcRequiresTheToken() async throws {
        let harness = try AgentHarness.start(token: "s3cret")
        defer { harness.stop() }

        let handshake = try await harness.handshake(token: "s3cret")
        XCTAssertTrue(handshake.accepted, handshake.error.message)
        let sessionID = handshake.sessionID

        // The handshake RPC is exempt: it carries the token itself.
        let noSessionToken = AgentMetadata.callOptions(sessionID: sessionID)
        await expectStatus(.unauthenticated) {
            _ = try await harness.client.captureScreenshot(
                Impresario_V1_ScreenshotRequest(),
                callOptions: noSessionToken
            )
        }

        // With both the token and the session it succeeds.
        let response = try await harness.client.captureScreenshot(
            Impresario_V1_ScreenshotRequest(),
            callOptions: AgentMetadata.callOptions(sessionID: sessionID, token: "s3cret")
        )
        XCTAssertEqual(response.png, TestPNG.valid)
    }

    // MARK: - session stream

    func testSessionRunsCommandsInOrderAndClosesOnClientClose() async throws {
        let harness = try AgentHarness.start(platform: .ios, heartbeatIntervalMs: 100)
        defer { harness.stop() }

        let handshake = try await harness.handshake()
        XCTAssertTrue(handshake.accepted, handshake.error.message)
        let session = openSession(harness, sessionID: handshake.sessionID)

        session.commands.yield(clickCommand(id: "c-1"))
        session.commands.yield(screenshotCommand(id: "c-2"))

        let first = try await session.events.until(isResult)
        guard case .result(let clickResult) = first.body else {
            return XCTFail("expected a result, got \(first)")
        }
        XCTAssertEqual(clickResult.commandID, "c-1")
        XCTAssertTrue(clickResult.ok, clickResult.error.message)
        XCTAssertEqual(harness.driver.taps, ["login"])

        let second = try await session.events.until(isResult)
        guard case .result(let shotResult) = second.body else {
            return XCTFail("expected a result, got \(second)")
        }
        XCTAssertEqual(shotResult.commandID, "c-2")
        XCTAssertTrue(shotResult.ok, shotResult.error.message)
        XCTAssertEqual(shotResult.screenshotPng, TestPNG.valid)

        // Heartbeats keep flowing while the session is open (spec 7.2 rule 5).
        let heartbeat = try await session.events.until(isHeartbeat)
        guard case .heartbeat(let payload) = heartbeat.body else {
            return XCTFail("expected a heartbeat, got \(heartbeat)")
        }
        XCTAssertGreaterThan(payload.seq, 0)
        XCTAssertEqual(payload.currentCommandID, "")

        session.commands.finish()
        let closed = try await session.events.until(isClosed)
        guard case .closed(let reason) = closed.body else {
            return XCTFail("expected a closed event, got \(closed)")
        }
        XCTAssertEqual(reason.reason, "client_closed")
    }

    func testInvalidCommandGetsOneResultWithTheSpecificError() async throws {
        let harness = try AgentHarness.start()
        defer { harness.stop() }

        let handshake = try await harness.handshake()
        let session = openSession(harness, sessionID: handshake.sessionID)

        // A CSS selector is not supported on Apple platforms (docs/PARITY.md).
        session.commands.yield(cssClickCommand(id: "c-bad"))
        let first = try await session.events.until(isResult)
        guard case .result(let rejected) = first.body else {
            return XCTFail("expected a result, got \(first)")
        }
        XCTAssertEqual(rejected.commandID, "c-bad")
        XCTAssertFalse(rejected.ok)
        XCTAssertEqual(rejected.error.code, .invalidSelector)
        XCTAssertTrue(rejected.error.message.contains("PARITY.md"), rejected.error.message)

        // The session stays usable: the next command runs normally.
        session.commands.yield(clickCommand(id: "c-1"))
        let second = try await session.events.until(isResult)
        guard case .result(let accepted) = second.body else {
            return XCTFail("expected a result, got \(second)")
        }
        XCTAssertTrue(accepted.ok, accepted.error.message)
        XCTAssertEqual(harness.driver.taps, ["login"])

        // An empty command id is a protocol violation (spec 7.2 rule 4).
        session.commands.yield(clickCommand(id: ""))
        let third = try await session.events.until(isResult)
        guard case .result(let emptyId) = third.body else {
            return XCTFail("expected a result, got \(third)")
        }
        XCTAssertEqual(emptyId.error.code, .protocol)
    }

    func testSessionMetadataIsValidated() async throws {
        let harness = try AgentHarness.start()
        defer { harness.stop() }

        let handshake = try await harness.handshake()

        // Missing metadata: INVALID_ARGUMENT.
        let (missing, _) = AsyncStream<Impresario_V1_Command>.makeStream()
        let missingEvents = EventReader(harness.client.session(missing))
        await expectStatus(.invalidArgument) {
            let events = try await withTimeout { try await missingEvents.drain() }
            XCTFail("unexpected \(events.count) events")
        }

        // Unknown session: NOT_FOUND.
        let unknown = openSession(harness, sessionID: "not-a-session")
        await expectStatus(.notFound) {
            let events = try await withTimeout { try await unknown.events.drain() }
            XCTFail("unexpected \(events.count) events")
        }

        // A second stream for the same session: FAILED_PRECONDITION.
        let first = openSession(harness, sessionID: handshake.sessionID)
        first.commands.yield(clickCommand(id: "c-1"))
        _ = try await first.events.until(isResult)
        let second = openSession(harness, sessionID: handshake.sessionID)
        await expectStatus(.failedPrecondition) {
            let events = try await withTimeout { try await second.events.drain() }
            XCTFail("unexpected \(events.count) events")
        }
    }

    func testClientClosedReleasesTheSessionForTheNextHandshake() async throws {
        let harness = try AgentHarness.start()
        defer { harness.stop() }

        let first = try await harness.handshake()
        let session = openSession(harness, sessionID: first.sessionID)
        session.commands.yield(clickCommand())
        _ = try await session.events.until(isResult)
        session.commands.finish()
        _ = try await session.events.until(isClosed)

        // The agent serves the next Act without restarting (spec 9.2, D-18);
        // the server releases the session right after the stream ended.
        var second = try await harness.handshake()
        for _ in 0..<50 where !second.accepted {
            try await Task.sleep(nanoseconds: 20_000_000)
            second = try await harness.handshake()
        }
        XCTAssertTrue(second.accepted, second.error.message)
        XCTAssertNotEqual(second.sessionID, first.sessionID)
    }

    // MARK: - out-of-band RPCs

    func testScreenshotRpcReturnsTheDriverImage() async throws {
        let harness = try AgentHarness.start()
        defer { harness.stop() }

        let handshake = try await harness.handshake()
        let options = AgentMetadata.callOptions(sessionID: handshake.sessionID)
        let response = try await harness.client.captureScreenshot(
            Impresario_V1_ScreenshotRequest(),
            callOptions: options
        )
        XCTAssertEqual(response.png, TestPNG.valid)
        XCTAssertEqual(response.width, 1)
        XCTAssertEqual(response.height, 1)
        XCTAssertFalse(response.hasError)

        // A driver failure becomes a response error, not a transport failure.
        harness.driver.screenshotError = AgentError.notInteractable("nothing to see")
        let failed = try await harness.client.captureScreenshot(
            Impresario_V1_ScreenshotRequest(),
            callOptions: options
        )
        XCTAssertTrue(failed.hasError)
        XCTAssertEqual(failed.error.code, .notInteractable)
        XCTAssertEqual(failed.error.message, "nothing to see")
    }

    func testLogStreamDeliversAgentLogs() async throws {
        let harness = try AgentHarness.start()
        defer { harness.stop() }

        let handshake = try await harness.handshake()
        var request = Impresario_V1_LogRequest()
        request.minLevel = .debug
        let logs = harness.client.streamLogs(request)

        // Run a session to its client-close: the provider logs that at INFO.
        let session = openSession(harness, sessionID: handshake.sessionID)
        session.commands.yield(clickCommand())
        _ = try await session.events.until(isResult)
        session.commands.finish()
        _ = try await session.events.until(isClosed)

        let entry = try await withTimeout {
            for try await line in logs where line.message.contains("stream closed") {
                return line
            }
            throw TestTimeout()
        }
        XCTAssertEqual(entry.level, .info)
        XCTAssertEqual(entry.source, "agent")
    }

    func testShutdownClosesTheSessionAndSignalsTheAgent() async throws {
        let harness = try AgentHarness.start()
        defer { harness.stop() }

        let handshake = try await harness.handshake()
        let session = openSession(harness, sessionID: handshake.sessionID)
        session.commands.yield(screenshotCommand())
        _ = try await session.events.until(isResult)

        var request = Impresario_V1_ShutdownRequest()
        request.terminateApp = true
        _ = try await harness.client.shutdown(
            request,
            callOptions: AgentMetadata.callOptions(sessionID: handshake.sessionID)
        )

        let closed = try await session.events.until(isClosed)
        guard case .closed(let reason) = closed.body else {
            return XCTFail("expected a closed event, got \(closed)")
        }
        XCTAssertEqual(reason.reason, "shutdown")

        // The UI test's `testServe()` returns and the app can exit (spec 12.1).
        try await withTimeout {
            await harness.handle.waitForShutdown()
        }
    }
}
