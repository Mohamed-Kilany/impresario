import XCTest
@testable import AgentCore

final class CommandExecutorTests: XCTestCase {
    private var driver: FakeUIDriver!
    private var clock: TestClock!
    private var log: TestLog!

    override func setUp() {
        super.setUp()
        driver = FakeUIDriver()
        clock = TestClock()
        log = TestLog()
    }

    private func execute(
        _ body: CommandBody,
        platform: PlatformKind = .ios,
        timeoutMs: Int = 1_000,
        appId: String = "com.example.app",
        scrollMaxAttempts: Int = 6,
        focusMaxSteps: Int = 8
    ) async -> CommandResultData {
        let executor = makeExecutor(
            driver: driver,
            platform: platform,
            appId: appId,
            scrollMaxAttempts: scrollMaxAttempts,
            focusMaxSteps: focusMaxSteps,
            clock: clock,
            log: log
        )
        return await executor.execute(command(body, timeoutMs: timeoutMs))
    }

    @discardableResult
    private func addButton(
        _ id: String,
        label: String = "",
        text: String = "",
        visible: Bool = true,
        enabled: Bool = true,
        hittable: Bool = true,
        focused: Bool = false,
        frame: CGRect = CGRect(x: 0, y: 0, width: 100, height: 40)
    ) -> FakeElement {
        let element = FakeElement(
            id: id,
            label: label,
            text: text,
            visible: visible,
            enabled: enabled,
            hittable: hittable,
            focused: focused,
            frame: frame
        )
        driver.elements.append(element)
        return element
    }

    // MARK: - click

    func testClickTapsTheElement() async {
        addButton("login", label: "Log in")
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.commandId, "c1")
        XCTAssertEqual(driver.taps, ["login"])
        XCTAssertNil(result.error)
    }

    func testClickWaitsForAnElementThatAppearsLater() async {
        let element = addButton("login", visible: false)
        driver.findHandler = { _, call in
            if call >= 3 { element.visible = true }
            return nil
        }
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.taps, ["login"])
        XCTAssertGreaterThanOrEqual(driver.findCalls, 3)
    }

    func testClickReportsElementNotFound() async {
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "missing")))
        )
        assertError(result, .elementNotFound)
        XCTAssertTrue(result.error?.message.contains("missing") == true)
        XCTAssertEqual(result.error?.details["selector"], "accessibilityId:missing#0")
    }

    func testClickReportsNotInteractableForADisabledElement() async {
        addButton("login", enabled: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
        )
        assertError(result, .notInteractable)
        XCTAssertEqual(result.error?.details["reason"], "disabled")
        XCTAssertTrue(driver.taps.isEmpty)
    }

    func testClickReportsNotInteractableForAHiddenElement() async {
        addButton("login", visible: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
        )
        assertError(result, .notInteractable)
        XCTAssertEqual(result.error?.details["reason"], "not visible")
    }

    func testClickReportsNotInteractableForANonHittableElementOnIos() async {
        addButton("login", hittable: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
        )
        assertError(result, .notInteractable)
        XCTAssertEqual(result.error?.details["reason"], "not hittable")
    }

    func testClickUsesTheRequestedIndex() async {
        addButton("row", label: "first")
        addButton("row", label: "second")
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "row", index: 1)))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.taps, ["row"])
    }

    func testOutOfRangeIndexIsElementNotFound() async {
        addButton("row")
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "row", index: 4)))
        )
        assertError(result, .elementNotFound)
    }

    func testClickRequiresASelector() async {
        let result = await execute(.runAction(RunActionSpec(kind: .click)))
        assertError(result, .protocolViolation)
    }

    func testTvClickFocusesThenPressesSelect() async {
        addButton("play", focused: true)
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "play"))),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.map(\.button), [.select])
    }

    func testTvClickFocusTraversalThenSelect() async {
        addButton("play", frame: CGRect(x: 0, y: 0, width: 100, height: 100))
        let target = addButton(
            "target",
            frame: CGRect(x: 200, y: 0, width: 100, height: 100)
        )
        target.focused = false
        driver.focusedFrameProvider = { self.driver.elements.first(where: { $0.focused })?.frame }
        driver.remoteHandler = { button in
            guard button == .right || button == .select else { return }
            let focused = self.driver.elements.first(where: { $0.focused })
            if button == .right, let focused {
                focused.focused = false
                let next = self.driver.elements
                    .filter { $0.frame.midX > focused.frame.midX }
                    .min { $0.frame.midX < $1.frame.midX }
                next?.focused = true
            }
        }
        driver.elements.first?.focused = true

        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "target"))),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.map(\.button), [.right, .select])
    }

    func testTvClickFallsBackToTapWhenFocusIsUnreachableButHittable() async {
        addButton("play", hittable: true)
        // No focused element and no frame: the navigator reports `blocked`.
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "play"))),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.taps, ["play"])
    }

    func testTvClickKeepsFocusErrorWhenNotHittable() async {
        addButton("play", hittable: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "play"))),
            platform: .tvos
        )
        assertError(result, .focusUnreachable)
        XCTAssertTrue(result.error?.message.contains("D-pad") == true)
    }

    // MARK: - text entry

    func testTypeReplacesTheText() async {
        let field = addButton("email", text: "old")
        let result = await execute(
            .runAction(RunActionSpec(kind: .type, selector: select(.accessibilityId, "email"), text: "new"))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(field.textValue, "new")
        XCTAssertEqual(result.observedText, "new")
        XCTAssertEqual(driver.typed.map(\.text), ["new"])
    }

    func testAppendPreservesTheText() async {
        let field = addButton("input", text: "old")
        let result = await execute(
            .runAction(RunActionSpec(kind: .append, selector: select(.accessibilityId, "input"), text: "x"))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(field.textValue, "oldx")
        XCTAssertEqual(result.observedText, "oldx")
    }

    func testClearEmptiesTheText() async {
        let field = addButton("input", text: "hello")
        let result = await execute(
            .runAction(RunActionSpec(kind: .clear, selector: select(.accessibilityId, "input")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(field.textValue, "")
        XCTAssertNil(result.observedText)
    }

    func testTextEntryOnAnEmptyField() async {
        let field = addButton("input", text: "")
        let result = await execute(
            .runAction(RunActionSpec(kind: .append, selector: select(.accessibilityId, "input"), text: "x"))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(field.textValue, "x")
    }

    func testTextEntryVerifiesTheReadBack() async {
        addButton("input", text: "old")
        driver.ignoreTypeText = true
        let result = await execute(
            .runAction(RunActionSpec(kind: .type, selector: select(.accessibilityId, "input"), text: "new"))
        )
        assertError(result, .notInteractable)
        XCTAssertEqual(result.error?.details["expected"], "new")
        XCTAssertEqual(result.error?.details["observed"], "")
    }

    func testTvTextEntryFocusesAndOpensTheKeyboard() async {
        let field = addButton("input", text: "", focused: true)
        let result = await execute(
            .runAction(RunActionSpec(kind: .type, selector: select(.accessibilityId, "input"), text: "abc")),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(field.textValue, "abc")
        XCTAssertEqual(driver.remotes.map(\.button), [.select])
    }

    // MARK: - longpress

    func testLongPressUsesTheDuration() async {
        addButton("item")
        let result = await execute(
            .runAction(
                RunActionSpec(
                    kind: .longPress,
                    selector: select(.accessibilityId, "item"),
                    durationMs: 1500
                )
            )
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.presses.first?.duration, 1.5)
    }

    func testLongPressDefaultsToOneSecond() async {
        addButton("item")
        let result = await execute(
            .runAction(RunActionSpec(kind: .longPress, selector: select(.accessibilityId, "item")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.presses.first?.duration, 1.0)
    }

    func testTvLongPressUsesTheRemoteSelectHold() async {
        addButton("item", focused: true)
        driver.longPressSelectResult = true
        let result = await execute(
            .runAction(
                RunActionSpec(
                    kind: .longPress,
                    selector: select(.accessibilityId, "item"),
                    durationMs: 2000
                )
            ),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.longPressDurations, [2.0])
        XCTAssertTrue(driver.presses.isEmpty)
    }

    func testTvLongPressWithoutDriverSupportIsUnsupported() async {
        addButton("item", focused: true)
        driver.longPressSelectResult = false
        let result = await execute(
            .runAction(RunActionSpec(kind: .longPress, selector: select(.accessibilityId, "item"))),
            platform: .tvos
        )
        assertError(result, .unsupportedAction)
    }

    // MARK: - focus / select

    func testFocusMovesTheFocusOnTv() async {
        let play = addButton("play", frame: CGRect(x: 200, y: 0, width: 100, height: 100))
        let first = addButton("home", frame: CGRect(x: 0, y: 0, width: 100, height: 100))
        first.focused = true
        driver.remoteHandler = { button in
            guard button == .right else { return }
            first.focused = false
            play.focused = true
        }

        let result = await execute(
            .runAction(RunActionSpec(kind: .focus, selector: select(.accessibilityId, "play"))),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.map(\.button), [.right])
    }

    func testFocusIsUnsupportedOnIos() async {
        addButton("play")
        let result = await execute(
            .runAction(RunActionSpec(kind: .focus, selector: select(.accessibilityId, "play")))
        )
        assertError(result, .unsupportedAction)
        XCTAssertTrue(result.error?.message.contains("ios") == true)
    }

    func testFocusReportsFocusUnreachableWithTrail() async {
        let target = addButton("play", frame: CGRect(x: 200, y: 0, width: 100, height: 100))
        let first = addButton("home", frame: CGRect(x: 0, y: 0, width: 100, height: 100))
        first.focused = true
        driver.remoteHandler = { _ in
            // Every press lands back on `home`: the dominant axis is blocked,
            // the alternative axis is blocked too (spec 6.3 step 2c).
            first.focused = true
            target.focused = false
        }

        let result = await execute(
            .runAction(RunActionSpec(kind: .focus, selector: select(.accessibilityId, "play"))),
            platform: .tvos
        )
        assertError(result, .focusUnreachable)
        XCTAssertEqual(result.error?.details["reason"], "blocked")
        XCTAssertEqual(result.error?.details["steps"], "2")
        let trail = result.error?.details["focus_trail"] ?? ""
        XCTAssertTrue(trail.hasPrefix("["), trail)
        XCTAssertTrue(trail.contains("\"direction\":\"right\""), trail)
        XCTAssertTrue(trail.contains("\"direction\":\"blocked\""), trail)
    }

    func testSelectAdjustsAPickerWheel() async {
        addButton("size", label: "Size")
        driver.selectValueHandler = { _, value in value == "Large" }
        let result = await execute(
            .runAction(RunActionSpec(kind: .select, selector: select(.accessibilityId, "size"), text: "Large"))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.observedText, "Large")
        XCTAssertEqual(driver.selectValues.map(\.value), ["Large"])
        XCTAssertTrue(driver.taps.isEmpty)
    }

    func testSelectOpensTheTriggerAndTapsTheOption() async {
        addButton("menu", label: "Language")
        addButton("option", label: "German")
        let result = await execute(
            .runAction(RunActionSpec(kind: .select, selector: select(.accessibilityId, "menu"), text: "German"))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.taps, ["menu", "option"])
        XCTAssertEqual(result.observedText, "German")
    }

    func testSelectReportsAMissingOption() async {
        addButton("menu", label: "Language")
        let result = await execute(
            .runAction(RunActionSpec(kind: .select, selector: select(.accessibilityId, "menu"), text: "Klingon"))
        )
        assertError(result, .elementNotFound)
        XCTAssertEqual(result.error?.details["option"], "Klingon")
    }

    func testSelectIsUnsupportedOnTv() async {
        addButton("menu")
        let result = await execute(
            .runAction(RunActionSpec(kind: .select, selector: select(.accessibilityId, "menu"), text: "x")),
            platform: .tvos
        )
        assertError(result, .unsupportedAction)
    }

    // MARK: - scroll

    func testScrollUsesTheDriverOnIos() async {
        addButton("list")
        let result = await execute(
            .runAction(
                RunActionSpec(
                    kind: .scroll,
                    selector: select(.accessibilityId, "list"),
                    direction: .down,
                    count: 3
                )
            )
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.scrolls.count, 1)
        XCTAssertEqual(driver.scrolls.first?.id, "list")
        XCTAssertEqual(driver.scrolls.first?.direction, .down)
        XCTAssertEqual(driver.scrolls.first?.steps, 3)
    }

    func testScrollWithoutAContainer() async {
        let result = await execute(
            .runAction(RunActionSpec(kind: .scroll, direction: .up, count: 2))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertNil(driver.scrolls.first?.id)
        XCTAssertEqual(driver.scrolls.first?.steps, 2)
    }

    func testTvScrollPressesTheDpad() async {
        addButton("list", focused: true)
        let result = await execute(
            .runAction(
                RunActionSpec(
                    kind: .scroll,
                    selector: select(.accessibilityId, "list"),
                    direction: .down,
                    count: 3
                )
            ),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.map(\.button), [.down, .down, .down])
        XCTAssertTrue(driver.scrolls.isEmpty)
    }

    func testScrollRequiresADirection() async {
        let result = await execute(.runAction(RunActionSpec(kind: .scroll, count: 1)))
        assertError(result, .protocolViolation)
    }

    // MARK: - scrollto

    func testScrollToReturnsWhenTheElementIsAlreadyVisible() async {
        addButton("target")
        let result = await execute(
            .runAction(RunActionSpec(kind: .scrollTo, selector: select(.accessibilityId, "target")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertTrue(driver.scrolls.isEmpty)
    }

    func testScrollToScrollsDownThenFindsTheElement() async {
        let target = addButton("target", visible: false)
        var scrolls = 0
        driver.scrollHandler = { _, direction, _ in
            scrolls += 1
            if direction == .down && scrolls == 2 {
                target.visible = true
            }
        }
        let result = await execute(
            .runAction(RunActionSpec(kind: .scrollTo, selector: select(.accessibilityId, "target"))),
            scrollMaxAttempts: 6
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.scrolls.map(\.direction), [.down, .down])
    }

    func testScrollToReversesAfterHalfTheBudget() async {
        addButton("target", visible: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .scrollTo, selector: select(.accessibilityId, "target"))),
            scrollMaxAttempts: 4
        )
        assertError(result, .elementNotFound)
        XCTAssertEqual(driver.scrolls.map(\.direction), [.down, .down, .up, .up])
        XCTAssertEqual(result.error?.details["scroll_attempts"], "4")
    }

    func testTvScrollToPressesDpadBothWays() async {
        addButton("target", visible: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .scrollTo, selector: select(.accessibilityId, "target"))),
            platform: .tvos,
            scrollMaxAttempts: 4
        )
        assertError(result, .elementNotFound)
        XCTAssertEqual(driver.remotes.map(\.button), [.down, .down, .up, .up])
    }

    func testScrollToStopsAtTheDeadline() async {
        addButton("target", visible: false)
        let result = await execute(
            .runAction(RunActionSpec(kind: .scrollTo, selector: select(.accessibilityId, "target"))),
            timeoutMs: 250,
            scrollMaxAttempts: 100
        )
        assertError(result, .elementNotFound)
        XCTAssertEqual(driver.scrolls.count, 3)
    }

    // MARK: - swipe / back

    func testSwipeUsesTheDriverOnIos() async {
        addButton("card")
        let result = await execute(
            .runAction(
                RunActionSpec(kind: .swipe, selector: select(.accessibilityId, "card"), direction: .left)
            )
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.swipes.first?.id, "card")
        XCTAssertEqual(driver.swipes.first?.direction, .left)
    }

    func testSwipeIsUnsupportedOnTv() async {
        let result = await execute(
            .runAction(RunActionSpec(kind: .swipe, direction: .left)),
            platform: .tvos
        )
        assertError(result, .unsupportedAction)
    }

    func testBackUsesTheDriverOnIos() async {
        driver.pressBackResult = true
        let result = await execute(.runAction(RunActionSpec(kind: .back)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.backPresses, 1)
    }

    func testBackWithoutDriverSupportIsUnsupported() async {
        driver.pressBackResult = false
        let result = await execute(.runAction(RunActionSpec(kind: .back)))
        assertError(result, .unsupportedAction)
    }

    func testBackPressesMenuOnTv() async {
        let result = await execute(.runAction(RunActionSpec(kind: .back)), platform: .tvos)
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.map(\.button), [.menu])
    }

    // MARK: - checks

    func testVisibleCheckPasses() async {
        addButton("spinner")
        let result = await execute(
            .validateCheck(ValidateCheckSpec(kind: .visible, selector: select(.accessibilityId, "spinner")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.observedText, "visible")
    }

    func testHiddenCheckPassesForAMissingElement() async {
        let result = await execute(
            .validateCheck(ValidateCheckSpec(kind: .hidden, selector: select(.accessibilityId, "spinner")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.observedText, "missing")
    }

    func testHiddenCheckPassesForAnInvisibleElement() async {
        addButton("spinner", visible: false)
        let result = await execute(
            .validateCheck(ValidateCheckSpec(kind: .hidden, selector: select(.accessibilityId, "spinner")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.observedText, "hidden")
    }

    func testVisibleCheckTimesOutWithObservedDetails() async {
        let result = await execute(
            .validateCheck(ValidateCheckSpec(kind: .visible, selector: select(.accessibilityId, "spinner"))),
            timeoutMs: 300
        )
        assertError(result, .checkFailed)
        XCTAssertEqual(result.error?.details["expected"], "visible")
        XCTAssertEqual(result.error?.details["observed"], "missing")
    }

    func testCheckPollsUntilTheConditionHolds() async {
        let element = addButton("spinner", visible: false)
        driver.findHandler = { _, call in
            if call >= 3 { element.visible = true }
            return nil
        }
        let result = await execute(
            .validateCheck(ValidateCheckSpec(kind: .visible, selector: select(.accessibilityId, "spinner")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertGreaterThanOrEqual(driver.findCalls, 3)
    }

    func testEnabledAndDisabledChecks() async {
        addButton("next", enabled: true)
        addButton("off", enabled: false)
        let enabled = await execute(
            .validateCheck(ValidateCheckSpec(kind: .enabled, selector: select(.accessibilityId, "next")))
        )
        XCTAssertTrue(enabled.ok, enabled.error?.message ?? "")
        let disabled = await execute(
            .validateCheck(ValidateCheckSpec(kind: .disabled, selector: select(.accessibilityId, "off")))
        )
        XCTAssertTrue(disabled.ok, disabled.error?.message ?? "")
        let wrong = await execute(
            .validateCheck(ValidateCheckSpec(kind: .enabled, selector: select(.accessibilityId, "off")))
        )
        assertError(wrong, .checkFailed)
        XCTAssertEqual(wrong.error?.details["observed"], "disabled")
    }

    func testFocusedCheck() async {
        addButton("tile", focused: true)
        let focused = await execute(
            .validateCheck(ValidateCheckSpec(kind: .focused, selector: select(.accessibilityId, "tile"))),
            platform: .tvos
        )
        XCTAssertTrue(focused.ok, focused.error?.message ?? "")
        XCTAssertEqual(focused.observedText, "focused")
    }

    func testTextChecks() async {
        addButton("label", text: "Hello world")
        let equals = await execute(
            .validateCheck(
                ValidateCheckSpec(
                    kind: .textEquals,
                    selector: select(.accessibilityId, "label"),
                    expectedText: "Hello world"
                )
            )
        )
        XCTAssertTrue(equals.ok, equals.error?.message ?? "")
        let contains = await execute(
            .validateCheck(
                ValidateCheckSpec(
                    kind: .textContains,
                    selector: select(.accessibilityId, "label"),
                    expectedText: "world"
                )
            )
        )
        XCTAssertTrue(contains.ok, contains.error?.message ?? "")
        let mismatch = await execute(
            .validateCheck(
                ValidateCheckSpec(
                    kind: .textEquals,
                    selector: select(.accessibilityId, "label"),
                    expectedText: "nope"
                )
            ),
            timeoutMs: 200
        )
        assertError(mismatch, .checkFailed)
        XCTAssertEqual(mismatch.error?.details["observed"], "Hello world")
    }

    func testCountChecks() async {
        addButton("row")
        addButton("row")
        let ok = await execute(
            .validateCheck(
                ValidateCheckSpec(kind: .count, selector: select(.accessibilityId, "row"), expectedCount: 2)
            )
        )
        XCTAssertTrue(ok.ok, ok.error?.message ?? "")
        XCTAssertEqual(ok.observedCount, 2)
        let wrong = await execute(
            .validateCheck(
                ValidateCheckSpec(kind: .count, selector: select(.accessibilityId, "row"), expectedCount: 3)
            ),
            timeoutMs: 200
        )
        assertError(wrong, .checkFailed)
        XCTAssertEqual(wrong.error?.details["observed"], "2")
    }

    func testCheckRequiresASelector() async {
        let result = await execute(.validateCheck(ValidateCheckSpec(kind: .visible)))
        assertError(result, .protocolViolation)
    }

    // MARK: - keys

    func testKeyHomePressesHome() async {
        let result = await execute(.keyPress(KeyPressSpec(key: .home, repeatCount: 1, intervalMs: 0)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.homePresses, 1)
    }

    func testTvKeyOkPressesSelectWithRepeat() async {
        let result = await execute(
            .keyPress(KeyPressSpec(key: .ok, repeatCount: 3, intervalMs: 250)),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.first?.button, .select)
        XCTAssertEqual(driver.remotes.first?.count, 3)
        XCTAssertEqual(driver.remotes.first?.interval ?? 0, 0.25, accuracy: 0.0001)
    }

    func testKeyDefaultsToA100msInterval() async {
        let result = await execute(
            .keyPress(KeyPressSpec(key: .down, repeatCount: 1, intervalMs: 0)),
            platform: .tvos
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.remotes.first?.interval ?? 0, 0.1, accuracy: 0.0001)
    }

    func testUnsupportedKeyIsAnUnsupportedAction() async {
        let result = await execute(
            .keyPress(KeyPressSpec(key: .enter, repeatCount: 1, intervalMs: 0)),
            platform: .tvos
        )
        assertError(result, .unsupportedAction)
        XCTAssertTrue(driver.remotes.isEmpty)
    }

    // MARK: - screenshot

    func testScreenshotReturnsThePng() async {
        let result = await execute(.screenshot)
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.screenshotPng, FakePNG.valid)
        XCTAssertEqual(Png.dimensions(result.screenshotPng ?? Data())?.width, 1)
    }

    func testEmptyScreenshotIsAnInternalError() async {
        driver.screenshotData = Data()
        let result = await execute(.screenshot)
        assertError(result, .internalError)
    }

    func testScreenshotDriverErrorIsReported() async {
        driver.screenshotError = AgentError.notInteractable("the screen is locked")
        let result = await execute(.screenshot)
        assertError(result, .notInteractable)
        XCTAssertEqual(result.error?.message, "the screen is locked")
    }

    // MARK: - app control

    func testLaunch() async {
        let result = await execute(.appControl(AppControlSpec(op: .launch, clearData: false)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.launches, ["com.example.app"])
        XCTAssertTrue(driver.terminations.isEmpty)
    }

    func testLaunchWithClearDataWarnsAndColdStarts() async {
        let result = await execute(.appControl(AppControlSpec(op: .launch, clearData: true)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.terminations, ["com.example.app"])
        XCTAssertEqual(driver.launches, ["com.example.app"])
        let warnings = log.at(.warn)
        XCTAssertEqual(warnings.count, 1)
        XCTAssertTrue(warnings.first?.message.contains("app_path") == true)
    }

    func testClearDataWarnsOnlyOnce() async {
        // One executor serves one session (the dispatcher owns it): the WARN
        // must be emitted once, not once per command (docs/DECISIONS.md I-84).
        let executor = makeExecutor(
            driver: driver,
            platform: .ios,
            appId: "com.example.app",
            clock: clock,
            log: log
        )
        _ = await executor.execute(command(.appControl(AppControlSpec(op: .launch, clearData: true))))
        _ = await executor.execute(command(.appControl(AppControlSpec(op: .restart, clearData: true))))
        XCTAssertEqual(log.at(.warn).count, 1)
    }

    func testRestartTerminatesThenLaunches() async {
        let result = await execute(.appControl(AppControlSpec(op: .restart, clearData: false)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.terminations, ["com.example.app"])
        XCTAssertEqual(driver.launches, ["com.example.app"])
    }

    func testTerminate() async {
        let result = await execute(.appControl(AppControlSpec(op: .terminate, clearData: false)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.terminations, ["com.example.app"])
    }

    func testBackgroundPressesHome() async {
        let result = await execute(.appControl(AppControlSpec(op: .background, clearData: false)))
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(driver.homePresses, 1)
    }

    func testCrashAfterLaunchIsReported() async {
        driver.crashAfterLaunch = true
        let result = await execute(.appControl(AppControlSpec(op: .launch, clearData: false)))
        assertError(result, .appCrashed)
    }

    // MARK: - protocol and failures

    func testCancelCommandIsAProtocolViolationAtTheExecutor() async {
        let result = await execute(.cancel(target: "c1"))
        assertError(result, .protocolViolation)
    }

    func testUnspecifiedBodyIsUnsupported() async {
        let result = await execute(.unspecified)
        assertError(result, .unsupportedAction)
    }

    func testDriverThrownAgentErrorIsPreserved() async {
        driver.findError = AgentError.invalidSelector("bad predicate")
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.predicate, "!!!")))
        )
        assertError(result, .invalidSelector)
        XCTAssertEqual(result.error?.message, "bad predicate")
    }

    func testUnexpectedDriverErrorIsAnInternalError() async {
        driver.findError = NSError(domain: "test", code: 7, userInfo: nil)
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "x")))
        )
        assertError(result, .internalError)
    }

    func testDurationIsMeasured() async {
        // The element only appears on the second find call, so the executor
        // spends one poll interval (the virtual clock's 100 ms) waiting for it
        // and reports that elapsed time.
        driver.findHandler = { _, call in call >= 2 ? [FakeElement(id: "login")] : [] }
        let result = await execute(
            .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
        )
        XCTAssertTrue(result.ok, result.error?.message ?? "")
        XCTAssertEqual(result.durationMs, 100)
    }

    func testCancellationProducesACancelledResult() async {
        addButton("login", visible: false)
        let executor = CommandExecutor(
            driver: driver,
            options: ExecutorOptions(platform: .ios, appId: "com.example.app", pollIntervalMs: 5),
            now: { 0 },
            sleep: { _ in try await Task.sleep(nanoseconds: 2_000_000) }
        )
        let task = Task {
            await executor.execute(
                AgentCommand(
                    id: "c1",
                    timeoutMs: 60_000,
                    body: .runAction(RunActionSpec(kind: .click, selector: select(.accessibilityId, "login")))
                )
            )
        }
        try? await Task.sleep(nanoseconds: 30_000_000)
        task.cancel()
        let result = await task.value
        assertError(result, .cancelled)
        XCTAssertEqual(result.commandId, "c1")
    }
}
