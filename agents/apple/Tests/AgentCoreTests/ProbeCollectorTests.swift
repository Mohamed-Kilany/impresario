import Foundation
import XCTest
@testable import AgentCore

final class ProbeCollectorTests: XCTestCase {
    private func collector(
        _ driver: FakeUIDriver,
        clock: TestClock
    ) -> ProbeCollector {
        ProbeCollector(
            driver: driver,
            extras: driver,
            now: { clock.now },
            sleep: { milliseconds in clock.advance(milliseconds) }
        )
    }

    private func video(_ driver: FakeUIDriver) {
        driver.elements = [
            FakeElement(id: "player", frame: CGRect(x: 0, y: 0, width: 1920, height: 1080))
        ]
    }

    private func selector() -> SelectorSpec {
        SelectorSpec(strategy: .accessibilityId, value: "player", index: 0)
    }

    func testShowingCollectsCoverageDecodedSourceAndVisibility() async throws {
        let driver = FakeUIDriver()
        video(driver)
        driver.regionCoverageValue = 0.91
        driver.showingDecodedValue = true
        driver.showingSourceValue = "https://example.test/sample.mp4"
        let clock = TestClock()

        let command = ProbeCommandSpec(
            kind: .showing,
            selector: selector(),
            claims: [
                ProbeClaimSpec(claim: "showing=present"),
                ProbeClaimSpec(claim: "showing.coverage>=80%"),
                ProbeClaimSpec(claim: "showing.decoded=true"),
                ProbeClaimSpec(claim: "showing.source~sample.mp4"),
            ],
            minConfidence: "authoritative"
        )
        let result = try await collector(driver, clock: clock).collect(command, timeoutMs: 1_000)

        XCTAssertEqual(result.verdicts.count, 4)
        for verdict in result.verdicts {
            XCTAssertEqual(verdict.verdict, "pass", "\(verdict.claim): \(verdict.reason ?? "")")
        }
        XCTAssertTrue(result.evidence.contains { $0.kind == "coverage" })
        XCTAssertTrue(result.evidence.contains { $0.source == .inAppHook })
        XCTAssertEqual(result.observed["showing.coverage>=80%"], "0.91")
    }

    func testMissingPieceMakesShowingAuthoritativelyFalse() async throws {
        let driver = FakeUIDriver()
        let clock = TestClock()
        let command = ProbeCommandSpec(
            kind: .showing,
            selector: selector(),
            claims: [ProbeClaimSpec(claim: "showing=present")]
        )
        let result = try await collector(driver, clock: clock).collect(command, timeoutMs: 300)
        XCTAssertEqual(result.verdicts.first?.verdict, "fail")
        XCTAssertEqual(result.verdicts.first?.confidence, "authoritative")
    }

    func testPlayerUsesSnapshotAndLeavesUnknownPropertiesUnverifiable() async throws {
        let driver = FakeUIDriver()
        video(driver)
        driver.mediaSnapshotValue = MediaSnapshot(
            state: "Playing",
            positionSeconds: 12,
            durationSeconds: 120,
            rate: 1,
            volume: 0.8,
            muted: false,
            widthPx: 1920,
            heightPx: 1080
        )
        driver.drmSnapshotValue = DrmSnapshot(enabled: false, provider: "fairplay")
        let clock = TestClock()
        let command = ProbeCommandSpec(
            kind: .player,
            selector: selector(),
            claims: [
                ProbeClaimSpec(claim: "state=Playing"),
                ProbeClaimSpec(claim: "drm.enabled=false"),
                ProbeClaimSpec(claim: "drm.provider=fairplay"),
            ],
            assertions: [
                ProbeAssertionSpec(property: "position", operator: ">", value: "5"),
                ProbeAssertionSpec(property: "resolution", operator: "=", value: "1080"),
                ProbeAssertionSpec(property: "muted", operator: "=", value: "true", negated: true),
                ProbeAssertionSpec(property: "error", operator: "=", value: ""),
            ],
            minConfidence: "authoritative"
        )
        let result = try await collector(driver, clock: clock).collect(command, timeoutMs: 1_000)
        let byClaim = Dictionary(uniqueKeysWithValues: result.verdicts.map { ($0.claim, $0) })
        XCTAssertEqual(byClaim["state=Playing"]?.verdict, "pass")
        XCTAssertEqual(byClaim["drm.enabled=false"]?.verdict, "pass")
        XCTAssertEqual(byClaim["drm.provider=fairplay"]?.verdict, "pass")
        XCTAssertEqual(byClaim["property.position"]?.verdict, "pass")
        XCTAssertEqual(byClaim["property.resolution"]?.verdict, "pass")
        XCTAssertEqual(byClaim["property.muted"]?.verdict, "pass")
        XCTAssertEqual(byClaim["property.error"]?.verdict, "unverifiable")
        XCTAssertTrue(result.evidence.contains { $0.kind == "media_state" && $0.source == .inAppHook })
        XCTAssertFalse(result.evidence.contains { $0.source == .mediaDrm || $0.source == .mediaSession })
    }

    func testPlayerWithoutSnapshotNeverInventsPlaying() async throws {
        let driver = FakeUIDriver()
        video(driver)
        let clock = TestClock()
        let command = ProbeCommandSpec(
            kind: .player,
            selector: selector(),
            claims: [ProbeClaimSpec(claim: "state=Playing")],
            minConfidence: "corroborated"
        )
        let result = try await collector(driver, clock: clock).collect(command, timeoutMs: 500)
        XCTAssertEqual(result.verdicts.first?.verdict, "unverifiable")
        XCTAssertEqual(result.verdicts.first?.confidence, "none")
    }

    func testMissingPlayerPieceContradictsStateClaims() async throws {
        let driver = FakeUIDriver()
        let clock = TestClock()
        let command = ProbeCommandSpec(
            kind: .player,
            selector: selector(),
            claims: [ProbeClaimSpec(claim: "state=Playing")]
        )
        let result = try await collector(driver, clock: clock).collect(command, timeoutMs: 300)
        XCTAssertEqual(result.verdicts.first?.verdict, "fail")
        XCTAssertEqual(result.verdicts.first?.confidence, "authoritative")
    }

    func testProbeCommandRoundTripsThroughExecutor() async throws {
        let driver = FakeUIDriver()
        video(driver)
        driver.mediaSnapshotValue = MediaSnapshot(state: "Paused", positionSeconds: 3)
        let clock = TestClock()
        let executor = makeExecutor(driver: driver, clock: clock)
        let command = AgentCommand(
            id: "probe-1",
            timeoutMs: 1_000,
            body: .probe(ProbeCommandSpec(
                kind: .player,
                selector: selector(),
                claims: [ProbeClaimSpec(claim: "state=Paused")],
                minConfidence: "authoritative"
            ))
        )
        let result = await executor.execute(command)
        XCTAssertTrue(result.ok)
        XCTAssertEqual(result.probe?.verdicts.first?.claim, "state=Paused")
        XCTAssertEqual(result.probe?.verdicts.first?.verdict, "pass")
    }
}
