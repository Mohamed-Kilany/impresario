import AgentCore
import CoreGraphics
import Foundation
import ImageIO
import UIKit
import XCTest
// Xcode 26 splits the UI-automation classes into their own framework; XCTest
// re-exports them on older SDKs (docs/DECISIONS.md I-89).
#if canImport(XCUIAutomation)
import XCUIAutomation
#endif

/// The XCUITest implementation of `UIDriver` (spec 12.2/12.3) — the only part
/// of the Apple agent that touches the app under test.
///
/// `AgentCore` calls drivers from Swift-concurrency tasks, so every entry point
/// funnels through one serial dispatch queue: XCUITest queries must not run
/// concurrently, and the queue also keeps the out-of-band `CaptureScreenshot`
/// from interleaving with a running command. The queue is *not* the main queue:
/// `testServe()` is async, and blocking the main thread while waiting for the
/// app under test can deadlock XCTest's own machinery.
///
/// App lifecycle calls are the exception. XCUITest asserts that
/// `XCUIApplication.launch/terminate/activate` run on the main thread; they are
/// dispatched there, and the driver queue waits for the result.
final class XCUIDriver: NSObject, UIDriver, DriverExtras, InspectorDriver {
    /// Launch argument that asks the app under test to disable animations
    /// (`animations_disabled`, spec 12.3). Apple has no API that switches
    /// another app's animations off, so the app under test must honour this
    /// flag; the stub app does.
    static let animationsDisabledArgument = "-impresario-animations-disabled"

    private let queue = DispatchQueue(label: "com.impresario.agent.xcuidriver")
    private var appId = ""
    private var animationsDisabled = false
    private var hasLaunched = false
    private var app: XCUIApplication?
    /// Set after a successful type: tvOS owns D-pad focus while its keyboard
    /// session is up. `XCUIApplication.keyboards` is the preferred probe, but
    /// the full-screen tvOS keyboard is not guaranteed to appear in the app's
    /// accessibility tree, so remember that we opened a session.
    private var typedIntoField = false
    /// The inspector registry of the last capture: node id → live element and
    /// its logical frame. Guarded by the driver queue.
    private var inspectorElements: [UInt64: XCUIElement] = [:]
    private var inspectorRects: [UInt64: CGRect] = [:]

    // ------------------------------------------------------------------
    // plumbing
    // ------------------------------------------------------------------

    /// Serialises `body` on the driver queue (all public methods do this).
    private func onQueue<T>(_ body: () throws -> T) rethrows -> T {
        try queue.sync(execute: body)
    }

    /// Runs an XCUITest lifecycle call where XCTest requires it. `onQueue`
    /// holds the driver serialisation while this waits, so queries cannot
    /// interleave with a launch or termination.
    private func onMain<T>(_ body: () throws -> T) rethrows -> T {
        if Thread.isMainThread {
            return try body()
        }
        return try DispatchQueue.main.sync(execute: body)
    }

    private func message(_ error: NSString?) -> String {
        error.map { $0 as String } ?? "unknown XCTest failure"
    }

    private func xcElement(_ ref: ElementRef) throws -> XCUIElement {
        guard let element = ref.handle as? XCUIElement else {
            throw AgentError.internalError("the element handle does not belong to this driver")
        }
        return element
    }

    /// The proxy for the app under test (spec 7.1 `app_id`).
    private func application() throws -> XCUIApplication {
        if let app {
            return app
        }
        guard !appId.isEmpty else {
            throw AgentError.protocolError(
                "the play did not name an app under test (app_id); the Apple agent cannot guess it"
            )
        }
        let app = XCUIApplication(bundleIdentifier: appId)
        if animationsDisabled {
            app.launchArguments.append(Self.animationsDisabledArgument)
        }
        self.app = app
        return app
    }

    private func application(for bundleId: String) throws -> XCUIApplication {
        let wanted = bundleId.isEmpty ? appId : bundleId
        guard !wanted.isEmpty else {
            throw AgentError.protocolError("no app under test (app_id) was named")
        }
        if wanted == appId, let app {
            return app
        }
        let app = XCUIApplication(bundleIdentifier: wanted)
        if animationsDisabled {
            app.launchArguments.append(Self.animationsDisabledArgument)
        }
        if wanted == appId {
            self.app = app
        }
        return app
    }

    // ------------------------------------------------------------------
    // selectors
    // ------------------------------------------------------------------

    /// Every match for `spec`, in XCUITest document order (spec 5.3); index
    /// selection happens in `AgentCore`.
    private func matches(_ spec: SelectorSpec) throws -> [XCUIElement] {
        let app = try application()
        let query: XCUIElementQuery
        switch spec.strategy {
        case .accessibilityId:
            query = app.descendants(matching: .any).matching(identifier: spec.value)
        case .label:
            query = app.descendants(matching: .any)
                .matching(NSPredicate(format: "label == %@", spec.value))
        case .labelContains:
            query = app.descendants(matching: .any)
                .matching(NSPredicate(format: "label CONTAINS %@", spec.value))
        case .predicate:
            query = app.descendants(matching: .any).matching(try predicate(from: spec.value))
        case .iosType:
            query = app.descendants(matching: try elementType(spec.value))
        }
        return query.allElementsBoundByIndex
    }

    /// Builds a play-provided predicate without letting invalid syntax crash
    /// the test bundle (spec 5.3: a bad format is `INVALID_SELECTOR`).
    private func predicate(from format: String) throws -> NSPredicate {
        var error: NSString?
        guard let predicate = IMPXCUShim.predicate(format: format, error: &error) else {
            throw AgentError.invalidSelector("invalid predicate '\(format)': \(message(error))")
        }
        return predicate
    }

    private func elementType(_ raw: String) throws -> XCUIElement.ElementType {
        guard let type = AgentElementType(rawValue: raw) else {
            throw AgentError.invalidSelector("unknown iOS element type '\(raw)'")
        }
        return XCUIElement.ElementType(agentType: type)
    }

    // ------------------------------------------------------------------
    // UIDriver
    // ------------------------------------------------------------------

    func find(_ s: SelectorSpec, timeout: TimeInterval) throws -> [ElementRef] {
        try onQueue {
            let deadline = Date().addingTimeInterval(max(0, timeout))
            while true {
                let found = try matches(s)
                if !found.isEmpty || Date() >= deadline {
                    return found.map { ElementRef($0) }
                }
                Thread.sleep(forTimeInterval: 0.05)
            }
        }
    }

    func count(_ s: SelectorSpec) throws -> Int {
        try onQueue { try matches(s).count }
    }

    func tap(_ e: ElementRef) throws {
        try onQueue {
            let element = try xcElement(e)
            guard element.exists else {
                throw AgentError.elementNotFound("the element no longer exists")
            }
            guard element.isHittable else {
                throw AgentError.notInteractable("the element is not hittable")
            }
            var error: NSString?
            guard IMPXCUShim.tap(element, error: &error) else {
                throw AgentError.notInteractable("the tap failed: \(message(error))")
            }
        }
    }

    func press(_ e: ElementRef, duration: TimeInterval) throws {
        try onQueue {
            let element = try xcElement(e)
            guard element.exists else {
                throw AgentError.elementNotFound("the element no longer exists")
            }
            guard element.isHittable else {
                throw AgentError.notInteractable("the element is not hittable")
            }
            var error: NSString?
            guard IMPXCUShim.press(element, forDuration: duration, error: &error) else {
                throw AgentError.notInteractable("the press failed: \(message(error))")
            }
        }
    }

    func text(_ e: ElementRef) -> String {
        onQueue {
            guard let element = try? xcElement(e), element.exists else {
                return ""
            }
            // Spec 5.4: the value wins unless it is empty or the placeholder.
            let value = (element.value as? String) ?? ""
            let placeholder = element.placeholderValue ?? ""
            if !value.isEmpty && value != placeholder {
                return value
            }
            return element.label
        }
    }

    func typeText(_ e: ElementRef, _ t: String) throws {
        try onQueue {
            // XCUIElement.typeText asserts that it runs on the main thread.
            // Keep the driver queue held while dispatching there so a query
            // cannot interleave with the keyboard session.
            try onMain {
                let element = try xcElement(e)
                try ensureEditable(element)
                var error: NSString?
                guard IMPXCUShim.type(t, into: element, error: &error) else {
                    throw AgentError.notInteractable("typing failed: \(message(error))")
                }
                typedIntoField = true
            }
        }
    }

    func deleteAll(_ e: ElementRef) throws {
        try onQueue {
            try onMain {
                let element = try xcElement(e)
                try ensureEditable(element)
                let current = (element.value as? String) ?? ""
                guard !current.isEmpty else {
                    return
                }
                // XCUITest has no "clear" API: press delete once per character.
                // The executor rewrites the whole field (I-87), so this is the
                // deterministic path (a locale-dependent Clear/Select-All menu
                // would not be).
                let deletes = String(
                    repeating: XCUIKeyboardKey.delete.rawValue,
                    count: current.count
                )
                var error: NSString?
                guard IMPXCUShim.type(deletes, into: element, error: &error) else {
                    throw AgentError.notInteractable("clearing the field failed: \(message(error))")
                }
                typedIntoField = true
            }
        }
    }

    func isVisible(_ e: ElementRef) -> Bool {
        onQueue {
            // Spec 5.4 (ios, tvos): exists, non-empty frame, intersects the
            // app window.
            guard
                let element = try? xcElement(e),
                element.exists
            else {
                return false
            }
            let frame = element.frame
            guard !frame.isEmpty else {
                return false
            }
            guard
                let app = try? application(),
                let window = app.windows.allElementsBoundByIndex.first
            else {
                return false
            }
            return window.frame.intersects(frame)
        }
    }

    func isEnabled(_ e: ElementRef) -> Bool {
        onQueue {
            guard let element = try? xcElement(e), element.exists else {
                return false
            }
            return element.isEnabled
        }
    }

    func isHittable(_ e: ElementRef) -> Bool {
        onQueue {
            guard let element = try? xcElement(e), element.exists else {
                return false
            }
            return element.isHittable
        }
    }

    func hasFocus(_ e: ElementRef) -> Bool {
        onQueue {
            guard let element = try? xcElement(e), element.exists else {
                return false
            }
            return element.hasFocus
        }
    }

    func frame(_ e: ElementRef) -> CGRect {
        onQueue {
            guard let element = try? xcElement(e), element.exists else {
                return .zero
            }
            return element.frame
        }
    }

    func focusedFrame() -> CGRect? {
        onQueue {
            // The focus engine is tvOS's, but `hasFocus` reads keyboard focus on
            // iOS too, so one implementation serves both.
            guard let app = try? application() else {
                return nil
            }
            let focused = app.descendants(matching: .any)
                .matching(NSPredicate(format: "hasFocus == true"))
                .allElementsBoundByIndex
            for element in focused {
                let frame = element.frame
                if !frame.isEmpty {
                    return frame
                }
            }
            return nil
        }
    }

    func remote(_ b: RemoteButton, `repeat` count: Int, interval: TimeInterval) throws {
        #if os(tvOS)
        onQueue {
            let button: XCUIRemote.Button
            switch b {
            case .up: button = .up
            case .down: button = .down
            case .left: button = .left
            case .right: button = .right
            case .select: button = .select
            case .menu:
                button = .menu
                // Menu dismisses the tvOS keyboard before it reaches the app.
                typedIntoField = false
            case .home:
                button = .home
                typedIntoField = false
            case .playPause: button = .playPause
            case .pageUp: button = .pageUp
            case .pageDown: button = .pageDown
            }
            for _ in 0..<max(1, count) {
                XCUIRemote.shared.press(button)
                if interval > 0 {
                    Thread.sleep(forTimeInterval: interval)
                }
            }
        }
        #else
        throw AgentError.unsupportedAction(
            "remote buttons are tvos-only; use click/type on ios (docs/PARITY.md)"
        )
        #endif
    }

    func pressHome() throws {
        onQueue {
            XCUIDevice.shared.press(.home)
            typedIntoField = false
        }
    }

    func scroll(_ c: ElementRef?, _ d: Direction, steps: Int) throws {
        try onQueue {
            let target = try container(c)
            // `scroll <dir>` reveals content in `dir`, so the finger moves the
            // other way (spec 5.6).
            for _ in 0..<max(1, steps) {
                try drag(target, finger: d.opposite)
            }
        }
    }

    func swipe(_ e: ElementRef?, _ d: Direction) throws {
        try onQueue {
            // `swipe <dir>` moves the finger in `dir` (spec 5.6).
            try drag(try container(e), finger: d)
        }
    }

    func screenshotPNG() throws -> Data {
        try onQueue {
            // tvOS 26.1's `pngRepresentation` renders repeated text down the
            // frame. Encode the screenshot image through UIKit instead, after
            // giving focus/layout animations a settled frame.
            Thread.sleep(forTimeInterval: 0.5)
            let image = XCUIScreen.main.screenshot().image
            guard let png = image.pngData(), !png.isEmpty, Png.dimensions(png) != nil else {
                throw AgentError.internalError("the screenshot did not produce a PNG")
            }
            return png
        }
    }

    func launch(_ bundleId: String) throws {
        try onQueue {
            let app = try application(for: bundleId)
            let (ok, error): (Bool, NSString?) = onMain {
                var error: NSString?
                let ok = IMPXCUShim.launch(app, error: &error)
                return (ok, error)
            }
            guard ok else {
                throw AgentError.appCrashed(
                    "the app under test (\(bundleId)) did not launch: \(message(error))"
                )
            }
            hasLaunched = true
            typedIntoField = false
        }
    }

    func terminate(_ bundleId: String) throws {
        try onQueue {
            let app = try application(for: bundleId)
            let (ok, error): (Bool, NSString?) = onMain {
                var error: NSString?
                let ok = IMPXCUShim.terminate(app, error: &error)
                return (ok, error)
            }
            guard ok else {
                throw AgentError.internalError(
                    "terminating the app under test (\(bundleId)) failed: \(message(error))"
                )
            }
            hasLaunched = false
            typedIntoField = false
        }
    }

    func activate(_ bundleId: String) throws {
        try onQueue {
            let app = try application(for: bundleId)
            let (ok, error): (Bool, NSString?) = onMain {
                var error: NSString?
                let ok = IMPXCUShim.activate(app, error: &error)
                return (ok, error)
            }
            guard ok else {
                throw AgentError.internalError(
                    "activating the app under test (\(bundleId)) failed: \(message(error))"
                )
            }
            hasLaunched = true
            typedIntoField = false
        }
    }

    func deviceInfo() -> AgentDeviceInfo {
        onQueue {
            let image = XCUIScreen.main.screenshot().image
            let scale = Double(image.scale)
            let width = Int((Double(image.size.width) * scale).rounded())
            let height = Int((Double(image.size.height) * scale).rounded())
            return AgentDeviceInfo(
                model: Self.machineModel(),
                osName: Self.osName,
                osVersion: Self.osVersionText,
                screenWidthPx: max(0, width),
                screenHeightPx: max(0, height),
                pixelRatio: max(0, scale),
                runtime: Self.runtime
            )
        }
    }

    // ------------------------------------------------------------------
    // DriverExtras
    // ------------------------------------------------------------------

    func prepare(appId: String, animationsDisabled: Bool) {
        HookReceiver.shared.start()
        onQueue {
            if self.appId != appId {
                self.appId = appId
                self.app = nil
                self.hasLaunched = false
            }
            self.animationsDisabled = animationsDisabled
        }
    }

    func targetCrashed(appId: String) -> Bool {
        onQueue {
            guard hasLaunched else {
                return false
            }
            let wanted = appId.isEmpty ? self.appId : appId
            guard wanted == self.appId, let app else {
                return false
            }
            return app.state == .notRunning
        }
    }

    func keyboardVisible() -> Bool {
        onQueue {
            // A completed type proves the keyboard session exists even when
            // `app.keyboards` cannot see tvOS's full-screen keyboard.
            if typedIntoField {
                return true
            }
            guard let app = try? application() else {
                return false
            }
            let keyboard = app.keyboards.firstMatch
            return keyboard.exists && !keyboard.frame.isEmpty
        }
    }

    func longPressSelect(duration: TimeInterval) throws -> Bool {
        #if os(tvOS)
        onQueue {
            XCUIRemote.shared.press(.select, forDuration: duration)
            return true
        }
        #else
        return false
        #endif
    }

    func pressBack() throws -> Bool {
        #if os(tvOS)
        // tvOS `back` is the menu button (KeyTable), not a navigation bar.
        return false
        #else
        return try onQueue {
            let app = try application()
            // UIKit's default identifier for the navigation-bar back button
            // (spec 12.3 `back`), then the interactive-pop edge swipe.
            let back = app.navigationBars.buttons["BackButton"]
            if back.exists && back.isHittable {
                var error: NSString?
                guard IMPXCUShim.tap(back, error: &error) else {
                    throw AgentError.notInteractable("the back button did not tap: \(message(error))")
                }
                return true
            }
            guard IMPXCUShim.drag(app, from: CGVector(dx: 0.005, dy: 0.5), to: CGVector(dx: 0.7, dy: 0.5), error: nil) else {
                return false
            }
            return true
        }
        #endif
    }

    func selectValue(_ e: ElementRef, _ value: String) throws -> Bool {
        try onQueue {
            let element = try xcElement(e)
            guard element.exists, element.elementType == .pickerWheel else {
                return false
            }
            var error: NSString?
            guard IMPXCUShim.adjustPickerWheel(element, to: value, error: &error) else {
                throw AgentError.notInteractable(
                    "the picker wheel did not accept '\(value)': \(message(error))"
                )
            }
            return true
        }
    }

    // Player/DRM snapshots are supplied only by the optional in-app hook
    // (D-41). XCUITest has no public API that reads another app's AVPlayer or
    // FairPlay session, so returning nil is the honest answer here; the probe
    // then reports UNVERIFIABLE instead of faking Playing.
    func mediaSnapshot() -> MediaSnapshot? {
        HookReceiver.shared.snapshot()?.media
    }

    func drmSnapshot() -> DrmSnapshot? {
        HookReceiver.shared.snapshot()?.drm
    }

    func showingDecoded(_ e: ElementRef) -> Bool? {
        HookReceiver.shared.snapshot()?.showing?.decoded
    }

    func showingSource(_ e: ElementRef) -> String? {
        HookReceiver.shared.snapshot()?.showing?.source
    }

    /// Fraction of sampled pixels inside `rect` whose luminance differs from
    /// the region mean by more than a small threshold. This is the same
    /// system-class signal the Android driver uses; it is not an authoritative
    /// decoder counter. A uniformly painted frame therefore reports low
    /// coverage, and the probe stays honest rather than guessing.
    func regionCoverage(png: Data, rect: CGRect) -> Double? {
        onQueue {
            guard !png.isEmpty, rect.width > 1, rect.height > 1 else { return nil }
            guard let source = CGImageSourceCreateWithData(png as CFData, nil),
                  let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
            else { return nil }
            let width = image.width
            let height = image.height
            guard width > 0, height > 0 else { return nil }
            let screen = XCUIScreen.main.screenshot().image
            let pointWidth = max(1, screen.size.width)
            let scaleX = CGFloat(width) / pointWidth
            let pointHeight = max(1, screen.size.height)
            let scaleY = CGFloat(height) / pointHeight
            let left = max(0, min(width - 1, Int(rect.minX * scaleX)))
            let right = max(left + 1, min(width, Int(rect.maxX * scaleX)))
            // CoreGraphics bitmap rows start at the image's top row, while the
            // screenshot coordinate origin is top-left too; no y flip here.
            let top = max(0, min(height - 1, Int(rect.minY * scaleY)))
            let bottom = max(top + 1, min(height, Int(rect.maxY * scaleY)))

            let bytesPerRow = width * 4
            var pixels = [UInt8](repeating: 0, count: bytesPerRow * height)
            let space = CGColorSpaceCreateDeviceRGB()
            let info = CGBitmapInfo.byteOrder32Big.rawValue
                | CGImageAlphaInfo.premultipliedLast.rawValue
            guard let context = CGContext(
                data: &pixels,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: bytesPerRow,
                space: space,
                bitmapInfo: info
            ) else { return nil }
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))

            let step = max(1, min(right - left, bottom - top) / 200)
            var luminance: [Double] = []
            var y = top
            while y < bottom {
                var x = left
                while x < right {
                    let offset = y * bytesPerRow + x * 4
                    let alpha = Double(pixels[offset + 3]) / 255.0
                    if alpha >= 0.03 {
                        let red = Double(pixels[offset])
                        let green = Double(pixels[offset + 1])
                        let blue = Double(pixels[offset + 2])
                        luminance.append(0.2126 * red + 0.7152 * green + 0.0722 * blue)
                    }
                    x += step
                }
                y += step
            }
            guard !luminance.isEmpty else { return 0 }
            let mean = luminance.reduce(0, +) / Double(luminance.count)
            let painted = luminance.filter { abs($0 - mean) > 6.0 }.count
            return Double(painted) * 100.0 / Double(luminance.count)
        }
    }

    // ------------------------------------------------------------------
    // InspectorDriver (TASK-INSPECTOR)
    // ------------------------------------------------------------------

    private static let inspectorTraversalBudget = 20_000
    private static let inspectorNameLimit = 200
    private static let inspectorTextLimit = 1_000

    func capture(_ options: InspectorCaptureOptions) throws -> InspectorCapture {
        try onQueue {
            let app = try application()
            if app.state != .runningForeground, !appId.isEmpty {
                // The inspector attaches to an app, like the Android driver's
                // app window: bring it up when the transport only started the
                // test runner (and not the app under test).
                _ = onMain {
                    var error: NSString?
                    return IMPXCUShim.activate(app, error: &error)
                }
            }
            let appFrame = app.frame
            let viewport = CGRect(origin: .zero, size: appFrame.size)

            var nodes: [InspectorNodeData] = []
            var elements: [UInt64: XCUIElement] = [:]
            var frames: [UInt64: CGRect] = [:]
            var truncated = false

            func walk(
                _ element: XCUIElement,
                parentId: UInt64,
                depth: Int,
                systemUi: Bool
            ) {
                if nodes.count >= Self.inspectorTraversalBudget {
                    truncated = true
                    return
                }
                if depth > options.maxDepth { return }
                guard element.exists else { return }

                let type = element.elementType
                let isSystemUi = systemUi || type == .statusBar
                if isSystemUi && !options.includeSystemUi { return }

                let id = UInt64(nodes.count + 1)
                let frame = element.frame
                let role = Self.inspectorRole(type)
                let identifier = element.identifier
                let label = element.label
                let placeholder = element.placeholderValue ?? ""
                let rawValue = (element.value as? String) ?? ""
                let secure = type == .secureTextField
                let value = secure ? "" : rawValue
                let input = Self.inspectorInputType(type)
                let name = label.isEmpty
                    ? (element.title.isEmpty ? identifier : element.title)
                    : label
                let text = input ? "" : label
                let nameCapped = Self.capText(name, Self.inspectorNameLimit)
                let textCapped = Self.capText(text, Self.inspectorTextLimit)
                let valueCapped = Self.capText(value, Self.inspectorTextLimit)
                let scrollable = Self.inspectorScrollable(type)

                nodes.append(
                    InspectorNodeData(
                        id: id,
                        parentId: parentId,
                        role: role,
                        name: nameCapped.text,
                        text: textCapped.text,
                        testId: identifier,
                        rawId: identifier,
                        bounds: InspectorRectData(
                            x: Double(frame.minX),
                            y: Double(frame.minY),
                            w: Double(frame.width),
                            h: Double(frame.height)
                        ),
                        focused: element.hasFocus,
                        enabled: element.isEnabled,
                        selected: element.isSelected,
                        checked: Self.inspectorChecked(element),
                        readonly: input && !element.isEnabled,
                        hidden: false,
                        secure: secure,
                        systemUi: isSystemUi,
                        scrollable: scrollable,
                        scrollAxis: scrollable
                            ? (frame.height >= frame.width ? "vertical" : "horizontal")
                            : "",
                        raw: options.includeRaw ? Self.inspectorRaw(element) : "",
                        value: valueCapped.text,
                        label: label,
                        placeholder: placeholder,
                        title: element.title,
                        visibleFraction: Self.visibleFraction(frame, viewport),
                        depth: UInt32(clamping: depth),
                        textTruncated: nameCapped.truncated
                            || textCapped.truncated
                            || valueCapped.truncated
                    )
                )
                elements[id] = element
                frames[id] = frame

                for child in element.children(matching: .any).allElementsBoundByIndex {
                    walk(child, parentId: id, depth: depth + 1, systemUi: isSystemUi)
                }
            }

            walk(app, parentId: 0, depth: 0, systemUi: false)

            let pruned = Self.pruneInspectorNodes(
                nodes,
                options: options,
                viewport: viewport
            )
            let keptIds = Set(pruned.nodes.map(\.id))
            inspectorElements = elements.filter { keptIds.contains($0.key) }
            inspectorRects = frames.filter { keptIds.contains($0.key) }

            var degraded: [String] = []
            if pruned.nodes.isEmpty {
                degraded.append("UNAVAILABLE: the app tree has no visible accessibility nodes")
            }
            if truncated {
                degraded.append(
                    "PARTIAL: the accessibility tree hit the traversal budget; some nodes may be missing"
                )
            }
            if pruned.capped {
                degraded.append("PARTIAL: max_nodes truncated the snapshot")
            }

            let png = options.includeScreenshot ? try screenshotPNGOnQueue() : nil
            return InspectorCapture(
                nodes: pruned.nodes,
                focusedId: pruned.focusedId,
                truncated: truncated || pruned.capped,
                degraded: degraded,
                screenshotPng: png
            )
        }
    }

    func screenshot(scalePercent: Int) throws -> Data? {
        onQueue {
            let image = XCUIScreen.main.screenshot().image
            let percent = min(100, max(1, scalePercent))
            if percent == 100 {
                return image.pngData()
            }
            let size = CGSize(
                width: max(1, image.size.width * CGFloat(percent) / 100),
                height: max(1, image.size.height * CGFloat(percent) / 100)
            )
            let format = UIGraphicsImageRendererFormat()
            format.scale = image.scale
            let renderer = UIGraphicsImageRenderer(size: size, format: format)
            return renderer.image { _ in
                image.draw(in: CGRect(origin: .zero, size: size))
            }.pngData()
        }
    }

    func inspectorInput(_ action: InspectorInputAction) throws {
        try onQueue {
            switch action {
            case .tap(let nodeId, let x, let y):
                try inspectorTap(nodeId: nodeId, x: x, y: y)
            case .scroll(let direction, let count, let nodeId, _, _):
                try inspectorScroll(direction: direction, count: count, nodeId: nodeId)
            case .key(let key):
                try inspectorKey(key)
            case .type(let nodeId, let text):
                try inspectorType(nodeId: nodeId, text: text)
            case .swipe(let x1, let y1, let x2, let y2, _):
                try inspectorSwipe(x1: x1, y1: y1, x2: x2, y2: y2)
            }
        }
    }

    // MARK: inspector actions (queue-held)

    private func inspectorTap(nodeId: UInt64, x: Double, y: Double) throws {
        #if os(tvOS)
        throw AgentError.unsupportedAction(
            "the inspector cannot tap on tvos (no touch input); use the key action"
        )
        #else
        if let element = inspectorElements[nodeId], element.exists, element.isHittable {
            var error: NSString?
            if IMPXCUShim.tap(element, error: &error) {
                return
            }
        }
        let point = inspectorRects[nodeId].map { CGPoint(x: $0.midX, y: $0.midY) }
            ?? CGPoint(x: x, y: y)
        try tapScreen(point)
        #endif
    }

    private func inspectorScroll(direction: Direction, count: Int, nodeId: UInt64) throws {
        let steps = max(1, count)
        #if os(tvOS)
        let button: RemoteButton
        switch direction {
        case .up: button = .up
        case .down: button = .down
        case .left: button = .left
        case .right: button = .right
        }
        for _ in 0..<steps {
            try pressRemote(button)
        }
        #else
        let target: XCUIElement
        if let element = inspectorElements[nodeId],
           element.exists,
           element.elementType == .scrollView || element.elementType == .table
            || element.elementType == .collectionView || element.elementType == .webView
        {
            target = element
        } else {
            target = try container(nil)
        }
        // `scroll` names the content direction (DSL 5); the finger moves the
        // other way (mirrors the Android driver).
        let finger: Direction
        switch direction {
        case .up: finger = .down
        case .down: finger = .up
        case .left: finger = .right
        case .right: finger = .left
        }
        for _ in 0..<steps {
            try drag(target, finger: finger)
        }
        #endif
    }

    private func inspectorKey(_ key: RemoteKey) throws {
        let platform: PlatformKind
        #if os(tvOS)
        platform = .tvos
        #else
        platform = .ios
        #endif
        switch KeyTable.resolve(key, platform: platform) {
        case .remote(let button):
            try pressRemote(button)
        case .home:
            XCUIDevice.shared.press(.home)
            typedIntoField = false
        case .unsupported(let message):
            throw AgentError.unsupportedAction(message)
        }
    }

    private func inspectorType(nodeId: UInt64, text: String) throws {
        let app = try application()
        let element: XCUIElement
        if nodeId != 0 {
            guard let found = inspectorElements[nodeId], found.exists else {
                throw AgentError.elementNotFound("the inspector node no longer exists")
            }
            element = found
            try ensureEditable(element)
        } else {
            element = app.descendants(matching: .any)
                .matching(NSPredicate(format: "hasFocus == true"))
                .firstMatch
            guard element.exists else {
                throw AgentError.notInteractable("no inspector node is focused")
            }
        }
        guard !text.isEmpty else { return }
        try onMain {
            var error: NSString?
            guard IMPXCUShim.type(text, into: element, error: &error) else {
                throw AgentError.notInteractable(
                    "inspector typing failed: \(message(error))"
                )
            }
        }
        typedIntoField = true
    }

    private func inspectorSwipe(x1: Double, y1: Double, x2: Double, y2: Double) throws {
        #if os(tvOS)
        throw AgentError.unsupportedAction(
            "the inspector cannot swipe on tvos (no touch input); use the key action"
        )
        #else
        let app = try application()
        let frame = app.frame
        guard frame.width > 1, frame.height > 1 else {
            throw AgentError.notInteractable("the app has no visible extent")
        }
        let start = app.coordinate(
            withNormalizedOffset: CGVector(
                dx: (x1 - Double(frame.minX)) / Double(frame.width),
                dy: (y1 - Double(frame.minY)) / Double(frame.height)
            )
        )
        let end = app.coordinate(
            withNormalizedOffset: CGVector(
                dx: (x2 - Double(frame.minX)) / Double(frame.width),
                dy: (y2 - Double(frame.minY)) / Double(frame.height)
            )
        )
        try onMain {
            start.press(forDuration: 0.05, thenDragTo: end)
        }
        #endif
    }

    private func tapScreen(_ point: CGPoint) throws {
        #if os(tvOS)
        throw AgentError.unsupportedAction("touch input is unavailable on tvos")
        #else
        let app = try application()
        let frame = app.frame
        guard frame.width > 1, frame.height > 1 else {
            throw AgentError.notInteractable("the app has no visible extent")
        }
        let coordinate = app.coordinate(
            withNormalizedOffset: CGVector(
                dx: (point.x - frame.minX) / frame.width,
                dy: (point.y - frame.minY) / frame.height
            )
        )
        try onMain {
            coordinate.tap()
        }
        #endif
    }

    private func pressRemote(_ button: RemoteButton) throws {
        #if os(tvOS)
        let mapped: XCUIRemote.Button
        switch button {
        case .up: mapped = .up
        case .down: mapped = .down
        case .left: mapped = .left
        case .right: mapped = .right
        case .select: mapped = .select
        case .menu: mapped = .menu
        case .home: mapped = .home
        case .playPause: mapped = .playPause
        case .pageUp: mapped = .pageUp
        case .pageDown: mapped = .pageDown
        }
        XCUIRemote.shared.press(mapped)
        #else
        _ = button
        throw AgentError.unsupportedAction("remote input is unavailable on ios")
        #endif
    }

    // MARK: inspector normalization

    private func screenshotPNGOnQueue() throws -> Data {
        // tvOS 26.1's `pngRepresentation` renders repeated text down the
        // frame; encode the screenshot image through UIKit instead.
        Thread.sleep(forTimeInterval: 0.5)
        let image = XCUIScreen.main.screenshot().image
        guard let png = image.pngData(), !png.isEmpty, Png.dimensions(png) != nil else {
            throw AgentError.internalError("the screenshot did not produce a PNG")
        }
        return png
    }

    private static func inspectorRole(_ type: XCUIElement.ElementType) -> String {
        switch type {
        case .button, .toolbarButton, .menuButton, .popUpButton, .disclosureTriangle:
            return "button"
        case .link:
            return "link"
        case .textField, .secureTextField, .textView, .searchField, .comboBox:
            return "textbox"
        case .checkBox:
            return "checkbox"
        case .radioButton:
            return "radio"
        case .switch, .toggle:
            return "switch"
        case .slider, .valueIndicator:
            return "slider"
        case .picker, .pickerWheel:
            return "combobox"
        case .table, .collectionView, .scrollView, .outline, .browser:
            return "list"
        case .tableRow, .outlineRow:
            return "row"
        case .cell:
            return "cell"
        case .image, .icon:
            return "image"
        case .staticText:
            return "text"
        case .alert:
            return "alert"
        case .sheet, .dialog, .popover:
            return "dialog"
        case .progressIndicator, .activityIndicator:
            return "progressbar"
        case .tab:
            return "tab"
        case .tabBar, .tabGroup:
            return "tablist"
        case .navigationBar:
            return "navigation"
        case .toolbar:
            return "toolbar"
        case .menu, .menuBar:
            return "menu"
        case .menuItem, .menuBarItem:
            return "menuitem"
        default:
            return "generic"
        }
    }

    private static func inspectorInputType(_ type: XCUIElement.ElementType) -> Bool {
        switch type {
        case .textField, .secureTextField, .textView, .searchField, .comboBox:
            return true
        default:
            return false
        }
    }

    private static func inspectorScrollable(_ type: XCUIElement.ElementType) -> Bool {
        switch type {
        case .scrollView, .table, .collectionView, .textView, .webView, .outline,
             .browser, .pickerWheel:
            return true
        default:
            return false
        }
    }

    private static func inspectorChecked(_ element: XCUIElement) -> Bool? {
        switch element.elementType {
        case .switch, .toggle:
            let value = ((element.value as? String) ?? "").lowercased()
            if value == "1" || value == "on" { return true }
            if value == "0" || value == "off" { return false }
            return element.isSelected
        case .checkBox, .radioButton:
            return element.isSelected
        default:
            return nil
        }
    }

    private static func inspectorRaw(_ element: XCUIElement) -> String {
        let frame = element.frame
        let object: [String: Any] = [
            "type": String(describing: element.elementType),
            "identifier": element.identifier,
            "label": element.label,
            "value": (element.value as? String) ?? "",
            "placeholder": element.placeholderValue ?? "",
            "frame": [
                "x": Double(frame.minX),
                "y": Double(frame.minY),
                "w": Double(frame.width),
                "h": Double(frame.height),
            ],
        ]
        guard
            let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
            let text = String(data: data, encoding: .utf8)
        else {
            return ""
        }
        return text
    }

    private static func capText(_ value: String, _ limit: Int) -> (text: String, truncated: Bool) {
        if value.count > limit {
            return (String(value.prefix(limit)), true)
        }
        return (value, false)
    }

    private static func visibleFraction(_ rect: CGRect, _ viewport: CGRect) -> Double {
        let area = rect.width * rect.height
        guard area > 0 else { return 0 }
        let clipped = rect.intersection(viewport)
        guard !clipped.isNull, clipped.width > 0, clipped.height > 0 else { return 0 }
        return min(1, max(0, Double((clipped.width * clipped.height) / area)))
    }

    private static func clipInspectorRect(
        _ rect: InspectorRectData,
        _ viewport: CGRect
    ) -> InspectorRectData {
        let clipped = CGRect(
            x: rect.x,
            y: rect.y,
            width: rect.w,
            height: rect.h
        ).intersection(viewport)
        guard !clipped.isNull else {
            return InspectorRectData(x: rect.x, y: rect.y, w: 0, h: 0)
        }
        return InspectorRectData(
            x: Double(clipped.minX),
            y: Double(clipped.minY),
            w: Double(Swift.max(0, clipped.width)),
            h: Double(Swift.max(0, clipped.height))
        )
    }

    /// Viewport pruning mirrors the Android driver and the connector: keep the
    /// focused chain, drop off-screen/system nodes, then cap `max_nodes`.
    private static func pruneInspectorNodes(
        _ nodes: [InspectorNodeData],
        options: InspectorCaptureOptions,
        viewport: CGRect
    ) -> (nodes: [InspectorNodeData], focusedId: UInt64, capped: Bool) {
        if nodes.isEmpty { return ([], 0, false) }

        var byId: [UInt64: Int] = [:]
        for (index, node) in nodes.enumerated() { byId[node.id] = index }
        var keep = [Bool](repeating: false, count: nodes.count)

        for index in nodes.indices {
            let node = nodes[index]
            if node.systemUi && !options.includeSystemUi { continue }
            if Int(node.depth) > options.maxDepth { continue }
            if node.hidden && !node.focused { continue }
            let area = Double(node.bounds.w) * Double(node.bounds.h)
            if area <= 0 { continue }
            if node.focused {
                keep[index] = true
                continue
            }
            if node.visibleFraction > 0
                && (options.minVisibleFraction <= 0
                    || node.visibleFraction >= options.minVisibleFraction)
            {
                keep[index] = true
            }
        }

        for index in nodes.indices where keep[index] {
            var parent = nodes[index].parentId
            var guardCount = 0
            while parent != 0 && guardCount < 1_024 {
                guard let parentIndex = byId[parent] else { break }
                if keep[parentIndex] { break }
                keep[parentIndex] = true
                parent = nodes[parentIndex].parentId
                guardCount += 1
            }
        }

        var keptCount = keep.filter { $0 }.count
        var capped = false
        if keptCount > options.maxNodes {
            capped = true
            var priority = [Bool](repeating: false, count: nodes.count)
            for index in nodes.indices where nodes[index].focused {
                var current: Int? = index
                var guardCount = 0
                while let currentIndex = current, guardCount < 1_024, !priority[currentIndex] {
                    priority[currentIndex] = true
                    let parent = nodes[currentIndex].parentId
                    current = parent == 0 ? nil : byId[parent]
                    guardCount += 1
                }
            }
            for index in nodes.indices.reversed() where keptCount > options.maxNodes {
                if keep[index] && !priority[index] {
                    keep[index] = false
                    keptCount -= 1
                }
            }
            for index in nodes.indices.reversed() where keptCount > options.maxNodes {
                if keep[index] && !nodes[index].focused {
                    keep[index] = false
                    keptCount -= 1
                }
            }
        }

        var hiddenCounts: [UInt64: UInt32] = [:]
        for index in nodes.indices where !keep[index] {
            let parent = nodes[index].parentId
            if parent != 0, let parentIndex = byId[parent], keep[parentIndex] {
                hiddenCounts[parent, default: 0] += 1
            }
        }

        var kept: [InspectorNodeData] = []
        kept.reserveCapacity(keptCount)
        for index in nodes.indices where keep[index] {
            var node = nodes[index]
            node.childrenHiddenCount += hiddenCounts[node.id] ?? 0
            if node.parentId != 0 {
                let parentKept = byId[node.parentId].map { keep[$0] } ?? false
                if !parentKept {
                    node.parentId = 0
                }
            }
            node.viewportBounds = clipInspectorRect(node.bounds, viewport)
            kept.append(node)
        }
        let focusedId = kept.first(where: { $0.focused })?.id ?? 0
        return (kept, focusedId, capped)
    }

    // ------------------------------------------------------------------
    // helpers (queue-held)
    // ------------------------------------------------------------------

    private func ensureEditable(_ element: XCUIElement) throws {
        guard element.exists else {
            throw AgentError.elementNotFound("the element no longer exists")
        }
        guard Self.editableTypes.contains(element.elementType) else {
            throw AgentError.notInteractable(
                "the element is not editable (element type \(element.elementType.rawValue))"
            )
        }
        guard element.isEnabled else {
            throw AgentError.notInteractable("the element is not enabled")
        }
        #if !os(tvOS)
        // On tvOS the keyboard overlay covers the field, so hittability is not
        // a usable precondition there (spec 12.3 text entry).
        guard element.isHittable else {
            throw AgentError.notInteractable("the element is not hittable")
        }
        #endif
    }

    private static let editableTypes: Set<XCUIElement.ElementType> = [
        .textField, .secureTextField, .textView, .searchField, .comboBox,
    ]

    /// The scroll/swipe target: the given element, else the first scrollable of
    /// the app, else the app itself (spec 5.6 "the primary scrollable").
    private func container(_ ref: ElementRef?) throws -> XCUIElement {
        if let ref {
            return try xcElement(ref)
        }
        let app = try application()
        for view in app.scrollViews.allElementsBoundByIndex where view.exists && view.frame.height > 1 {
            return view
        }
        return app
    }

    /// One drag across 80% of the target's visible extent (spec 5.6) with the
    /// finger moving in `finger`.
    private func drag(_ target: XCUIElement, finger direction: Direction) throws {
        guard target.exists else {
            throw AgentError.elementNotFound("the scroll target no longer exists")
        }
        let frame = target.frame
        guard frame.width > 1, frame.height > 1 else {
            throw AgentError.notInteractable("the scroll target has no visible extent")
        }
        let from: CGVector
        let to: CGVector
        switch direction {
        case .up: from = CGVector(dx: 0.5, dy: 0.9); to = CGVector(dx: 0.5, dy: 0.1)
        case .down: from = CGVector(dx: 0.5, dy: 0.1); to = CGVector(dx: 0.5, dy: 0.9)
        case .left: from = CGVector(dx: 0.9, dy: 0.5); to = CGVector(dx: 0.1, dy: 0.5)
        case .right: from = CGVector(dx: 0.1, dy: 0.5); to = CGVector(dx: 0.9, dy: 0.5)
        }
        var error: NSString?
        guard IMPXCUShim.drag(target, from: from, to: to, error: &error) else {
            throw AgentError.notInteractable("the swipe failed: \(message(error))")
        }
    }

    // ------------------------------------------------------------------
    // device facts
    // ------------------------------------------------------------------

    private static var osName: String {
        #if os(tvOS)
        return "tvOS"
        #else
        return "iOS"
        #endif
    }

    private static var runtime: String {
        #if targetEnvironment(simulator)
        return "simulator"
        #else
        return "device"
        #endif
    }

    private static var osVersionText: String {
        let version = ProcessInfo.processInfo.operatingSystemVersion
        if version.patchVersion == 0 {
            return "\(version.majorVersion).\(version.minorVersion)"
        }
        return "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
    }

    /// The hardware identifier (`hw.machine`), e.g. `iPhone14,7` or `AppleTV6,2`.
    private static func machineModel() -> String {
        var size = 0
        guard sysctlbyname("hw.machine", nil, &size, nil, 0) == 0, size > 0 else {
            return fallbackModel
        }
        var buffer = [CChar](repeating: 0, count: size)
        guard sysctlbyname("hw.machine", &buffer, &size, nil, 0) == 0 else {
            return fallbackModel
        }
        let machine = buffer.withUnsafeBufferPointer { pointer -> String in
            guard let base = pointer.baseAddress else {
                return ""
            }
            return String(cString: base)
        }
        return machine.isEmpty ? fallbackModel : machine
    }

    private static var fallbackModel: String {
        #if os(iOS)
        return UIDevice.current.model
        #else
        return "Apple TV"
        #endif
    }
}

extension Direction {
    /// The opposite direction (`scroll <d>` is the finger direction inverted).
    var opposite: Direction {
        switch self {
        case .up: return .down
        case .down: return .up
        case .left: return .right
        case .right: return .left
        }
    }
}

extension XCUIElement.ElementType {
    /// Maps the wire `type:` names (spec 5.3, `AgentCore.AgentElementType`)
    /// onto XCUITest's element types.
    init(agentType: AgentElementType) {
        switch agentType {
        case .any: self = .any
        case .other: self = .other
        case .application: self = .application
        case .group: self = .group
        case .window: self = .window
        case .sheet: self = .sheet
        case .drawer: self = .drawer
        case .alert: self = .alert
        case .dialog: self = .dialog
        case .button: self = .button
        case .radioButton: self = .radioButton
        case .radioGroup: self = .radioGroup
        case .checkBox: self = .checkBox
        case .disclosureTriangle: self = .disclosureTriangle
        case .popUpButton: self = .popUpButton
        case .comboBox: self = .comboBox
        case .menuButton: self = .menuButton
        case .toolbarButton: self = .toolbarButton
        case .popover: self = .popover
        case .keyboard: self = .keyboard
        case .key: self = .key
        case .navigationBar: self = .navigationBar
        case .tabBar: self = .tabBar
        case .tabGroup: self = .tabGroup
        case .toolbar: self = .toolbar
        case .statusBar: self = .statusBar
        case .table: self = .table
        case .tableRow: self = .tableRow
        case .tableColumn: self = .tableColumn
        case .outline: self = .outline
        case .outlineRow: self = .outlineRow
        case .browser: self = .browser
        case .collectionView: self = .collectionView
        case .cell: self = .cell
        case .slider: self = .slider
        case .pageIndicator: self = .pageIndicator
        case .progressIndicator: self = .progressIndicator
        case .activityIndicator: self = .activityIndicator
        case .segmentedControl: self = .segmentedControl
        case .picker: self = .picker
        case .pickerWheel: self = .pickerWheel
        case .switch: self = .switch
        case .toggle: self = .toggle
        case .link: self = .link
        case .image: self = .image
        case .icon: self = .icon
        case .searchField: self = .searchField
        case .scrollView: self = .scrollView
        case .scrollBar: self = .scrollBar
        case .staticText: self = .staticText
        case .textField: self = .textField
        case .secureTextField: self = .secureTextField
        case .datePicker: self = .datePicker
        case .textView: self = .textView
        case .menu: self = .menu
        case .menuItem: self = .menuItem
        case .menuBar: self = .menuBar
        case .menuBarItem: self = .menuBarItem
        case .map: self = .map
        case .webView: self = .webView
        case .incrementArrow: self = .incrementArrow
        case .decrementArrow: self = .decrementArrow
        case .timeline: self = .timeline
        case .ratingIndicator: self = .ratingIndicator
        case .valueIndicator: self = .valueIndicator
        case .splitter: self = .splitter
        case .tab: self = .tab
        case .touchBar: self = .touchBar
        case .statusItem: self = .statusItem
        }
    }
}
