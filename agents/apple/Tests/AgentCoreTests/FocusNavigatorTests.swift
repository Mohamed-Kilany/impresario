import XCTest
@testable import AgentCore

/// Fake `FocusProbe` over a cell grid: pressing a direction moves the focus by
/// one cell (unless the test overrides the movement).
final class FakeFocusProbe: FocusProbe {
    var focusFrame: CGRect?
    var target: CGRect?
    var label = "focused"
    var focused = false
    var pressLog: [Direction] = []
    var pressFails = false
    var pressHook: ((Direction) -> Void)?
    /// One cell in `direction`: the default movement model.
    static let step: (Direction, CGRect) -> CGRect = { direction, frame in
        switch direction {
        case .up: return frame.offsetBy(dx: 0, dy: -frame.height)
        case .down: return frame.offsetBy(dx: 0, dy: frame.height)
        case .left: return frame.offsetBy(dx: -frame.width, dy: 0)
        case .right: return frame.offsetBy(dx: frame.width, dy: 0)
        }
    }
    var move: (Direction, CGRect) -> CGRect = FakeFocusProbe.step

    func currentFocusLabel() -> String? { label }
    func currentFocusFrame() -> CGRect? { focusFrame }
    func targetFrame() -> CGRect? { target }

    func press(_ direction: Direction) -> Bool {
        pressLog.append(direction)
        pressHook?(direction)
        if pressFails { return false }
        if let frame = focusFrame {
            let next = move(direction, frame)
            focusFrame = next
            if let target, next.midX == target.midX, next.midY == target.midY {
                focused = true
            }
        }
        return true
    }

    func isTargetFocused() -> Bool { focused }
}

final class FocusNavigatorTests: XCTestCase {
    private func cell(_ column: Int, _ row: Int) -> CGRect {
        CGRect(x: CGFloat(column) * 100, y: CGFloat(row) * 100, width: 100, height: 100)
    }

    func testAlreadyFocusedNeedsNoPress() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(0, 0)
        probe.focused = true

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        XCTAssertEqual(result, .focused)
        XCTAssertTrue(probe.pressLog.isEmpty)
    }

    func testMovesRightTowardsTarget() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(2, 0)

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        XCTAssertEqual(result, .focused)
        XCTAssertEqual(probe.pressLog, [.right, .right])
        XCTAssertEqual(probe.focusFrame, cell(2, 0))
    }

    func testDominantAxisWins() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(1, 1)

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        XCTAssertEqual(result, .focused)
        XCTAssertEqual(probe.pressLog, [.down, .right])
    }

    func testMovesLeftAlongTheDominantAxis() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(2, 2)
        probe.target = cell(0, 1)

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        // The axis is recomputed after every step (spec 6.3 step 2b): from
        // (2,2) dx = -200 beats dy = -100, so it goes left; at (1,2) the
        // deltas tie and the tie rule prefers vertical (up); at (1,1) only dx
        // is left, so it goes left onto the target.
        XCTAssertEqual(result, .focused)
        XCTAssertEqual(probe.pressLog, [.left, .up, .left])
    }

    func testReportsCycle() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(9, 0)
        // Toggle between two cells: the (frame, right) key repeats.
        probe.move = { _, frame in
            frame.midX < 100 ? self.cell(1, 0) : self.cell(0, 0)
        }

        let result = FocusNavigator(probe: probe, maxSteps: 10).ensureTargetFocused()

        guard case .unreachable(let reason, let trail, let steps) = result else {
            return XCTFail("expected unreachable, got \(result)")
        }
        XCTAssertEqual(reason, "cycle")
        XCTAssertEqual(steps, 2)
        XCTAssertEqual(trail.count, 2)
        XCTAssertEqual(trail.map(\.direction), ["right", "right"])
    }

    func testReportsBudget() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = CGRect(x: 1000, y: 0, width: 100, height: 100)
        probe.move = { _, frame in frame.offsetBy(dx: 10, dy: 0) }

        let result = FocusNavigator(probe: probe, maxSteps: 3).ensureTargetFocused()

        guard case .unreachable(let reason, let trail, let steps) = result else {
            return XCTFail("expected unreachable, got \(result)")
        }
        XCTAssertEqual(reason, "budget")
        XCTAssertEqual(steps, 3)
        XCTAssertEqual(trail.count, 3)
    }

    func testReportsBlockedWhenPressFails() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(2, 0)
        probe.pressFails = true

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        guard case .unreachable(let reason, _, let steps) = result else {
            return XCTFail("expected unreachable, got \(result)")
        }
        XCTAssertEqual(reason, "blocked")
        XCTAssertEqual(steps, 1)
    }

    func testTriesAlternativeAxisWhenDominantIsBlocked() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(1, 1)
        // The grid's first cell is a dead drop target: `down` does nothing
        // there, everywhere else it moves one row.
        probe.move = { direction, frame in
            if direction == .down && frame.midX < 100 && frame.midY < 100 { return frame }
            return FakeFocusProbe.step(direction, frame)
        }

        let result = FocusNavigator(probe: probe, maxSteps: 6).ensureTargetFocused()

        XCTAssertEqual(result, .focused)
        XCTAssertEqual(probe.pressLog, [.down, .right, .down])
    }

    func testBlockedOnBothAxesIsBlocked() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = cell(1, 1)
        probe.move = { _, frame in frame }

        let result = FocusNavigator(probe: probe, maxSteps: 6).ensureTargetFocused()

        guard case .unreachable(let reason, let trail, _) = result else {
            return XCTFail("expected unreachable, got \(result)")
        }
        XCTAssertEqual(reason, "blocked")
        XCTAssertEqual(trail.last?.direction, "blocked")
    }

    func testUnreadableFocusPressesDownOnce() {
        let probe = FakeFocusProbe()
        probe.focusFrame = nil
        probe.target = cell(1, 0)
        var revealing = true
        // The first press only reveals the focus: the probe reports the frame
        // it moved onto, and that press itself moves nothing (spec 6.3 step
        // 2a: "press down once, then read the frame again").
        probe.move = { direction, frame in
            if revealing {
                revealing = false
                return frame
            }
            return FakeFocusProbe.step(direction, frame)
        }
        probe.pressHook = { _ in
            if revealing { probe.focusFrame = self.cell(0, 0) }
        }

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        XCTAssertEqual(result, .focused)
        XCTAssertEqual(probe.pressLog, [.down, .right])
    }

    func testUnreadableFocusTwiceIsACycle() {
        let probe = FakeFocusProbe()
        probe.focusFrame = nil
        probe.target = cell(1, 0)

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        guard case .unreachable(let reason, _, let steps) = result else {
            return XCTFail("expected unreachable, got \(result)")
        }
        XCTAssertEqual(reason, "cycle")
        XCTAssertEqual(steps, 2)
    }

    func testMissingTargetFrameIsBlocked() {
        let probe = FakeFocusProbe()
        probe.focusFrame = cell(0, 0)
        probe.target = nil

        let result = FocusNavigator(probe: probe, maxSteps: 5).ensureTargetFocused()

        guard case .unreachable(let reason, _, _) = result else {
            return XCTFail("expected unreachable, got \(result)")
        }
        XCTAssertEqual(reason, "blocked")
    }

    func testFrameStringShape() {
        XCTAssertEqual(
            FocusNavigator.frameString(CGRect(x: 10.4, y: 20.6, width: 100, height: 40)),
            "[10,20][110,60]"
        )
    }

    func testTrailJson() {
        let trail = [
            FocusTrailEntry(step: 1, direction: "right", focusedLabel: "Play", frame: "[0,0][100,40]"),
            FocusTrailEntry(step: 2, direction: "down", focusedLabel: "", frame: "[100,0][200,40]"),
        ]
        let json = FocusNavigator.trailJson(trail)
        XCTAssertEqual(
            json,
            "[{\"step\":1,\"direction\":\"right\",\"focused_label\":\"Play\",\"frame\":\"[0,0][100,40]\"}," +
                "{\"step\":2,\"direction\":\"down\",\"focused_label\":\"\",\"frame\":\"[100,0][200,40]\"}]"
        )
    }

    func testTrailJsonEscapesSpecialCharacters() {
        let trail = [
            FocusTrailEntry(step: 1, direction: "right", focusedLabel: "a\"b\\c\nd", frame: ""),
        ]
        let json = FocusNavigator.trailJson(trail)
        XCTAssertTrue(json.contains("\"focused_label\":\"a\\\"b\\\\c\\nd\""), json)
    }
}
