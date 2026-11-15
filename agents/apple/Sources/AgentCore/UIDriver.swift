import Foundation

/// An opaque handle to a matched UI element.
///
/// The XCUITest driver boxes an `XCUIElement`; the tests box a fake. The box
/// is `@unchecked Sendable` because all real work happens on the main thread
/// inside the driver (spec 12.3 "Concurrency").
public struct ElementRef: @unchecked Sendable {
    public let handle: AnyObject

    public init(_ handle: AnyObject) {
        self.handle = handle
    }
}

/// One focusable element's identity, read in a single snapshot.
///
/// tvOS has no programmatic focus API: the executor moves the focus engine
/// with remote presses and asks which element holds focus. Reading that
/// element's identifier, label and frame in one snapshot lets the navigator
/// recognise the destination without a second `hasFocus` round trip per step
/// (I-201).
public struct FocusIdentity: Equatable {
    public var identifier: String
    public var label: String
    public var frame: CGRect

    public init(identifier: String, label: String, frame: CGRect) {
        self.identifier = identifier
        self.label = label
        self.frame = frame
    }

    /// Whether two snapshots describe the same focused element. The
    /// accessibility identifier is authoritative; a matching label inside an
    /// overlapping frame covers hosts that expose focus on a child node, and
    /// an exact frame is the fallback for elements without either.
    public func matches(_ other: FocusIdentity) -> Bool {
        if !identifier.isEmpty && !other.identifier.isEmpty {
            return identifier == other.identifier
        }
        if !label.isEmpty && label == other.label && frame.intersects(other.frame) {
            return true
        }
        return frame == other.frame
    }
}

/// One element's attributes as read in a single driver round trip.
///
/// XCUITest charges one accessibility IPC per attribute, and each call waits
/// for the app to settle, so the executor reads this batch once per poll
/// instead of calling `isVisible`, `isEnabled`, `frame` and `text` separately
/// (I-198). `hittable` is not part of the Apple snapshot and stays a separate
/// `UIDriver.isHittable` read on the click path.
public struct ElementAttributes {
    public var exists: Bool
    public var visible: Bool
    public var enabled: Bool
    public var focused: Bool
    public var frame: CGRect
    public var text: String

    public init(
        exists: Bool,
        visible: Bool,
        enabled: Bool,
        focused: Bool,
        frame: CGRect,
        text: String
    ) {
        self.exists = exists
        self.visible = visible
        self.enabled = enabled
        self.focused = focused
        self.frame = frame
        self.text = text
    }

    public static let missing = ElementAttributes(
        exists: false,
        visible: false,
        enabled: false,
        focused: false,
        frame: .zero,
        text: ""
    )
}

/// Buttons of `XCUIRemote` the driver can press (spec 6.2, tvos column).
public enum RemoteButton: String, Sendable, Equatable {
    case up
    case down
    case left
    case right
    case select
    case menu
    case home
    case playPause
    case pageUp
    case pageDown
}

/// The UI driver (spec 12.2, verbatim). The XCUITest bundle implements it over
/// `XCUIApplication`; `swift test` drives fakes through it.
public protocol UIDriver: AnyObject {
    func find(_ s: SelectorSpec, timeout: TimeInterval) throws -> [ElementRef]
    func count(_ s: SelectorSpec) throws -> Int
    func tap(_ e: ElementRef) throws
    func press(_ e: ElementRef, duration: TimeInterval) throws
    func text(_ e: ElementRef) -> String
    func typeText(_ e: ElementRef, _ t: String) throws
    /// Deletes the known content of a field. The caller passes the current
    /// text so XCUITest does not read the value again just to count deletes.
    func deleteAll(_ e: ElementRef, currentText: String) throws
    func isVisible(_ e: ElementRef) -> Bool
    func isEnabled(_ e: ElementRef) -> Bool
    func isHittable(_ e: ElementRef) -> Bool
    func hasFocus(_ e: ElementRef) -> Bool
    func frame(_ e: ElementRef) -> CGRect
    func focusedFrame() -> CGRect?
    func remote(_ b: RemoteButton, `repeat`: Int, interval: TimeInterval) throws
    func pressHome() throws
    func scroll(_ c: ElementRef?, _ d: Direction, steps: Int) throws
    func swipe(_ e: ElementRef?, _ d: Direction) throws
    func screenshotPNG() throws -> Data
    func launch(_ bundleId: String) throws
    func terminate(_ bundleId: String) throws
    func activate(_ bundleId: String) throws
    func deviceInfo() -> AgentDeviceInfo
}

/// Optional driver capabilities that the executor uses when present
/// (the Apple analogue of the Android agent's `DriverExtras`).
public protocol DriverExtras: AnyObject {
    /// Applies handshake-time options: the app under test and launch
    /// arguments (`animations_disabled`, spec 12.3).
    func prepare(appId: String, animationsDisabled: Bool)

    /// True when the app under test is no longer running after having been
    /// launched through this driver (`APP_CRASHED` detection).
    func targetCrashed(appId: String) -> Bool

    /// True while the on-screen keyboard is visible. tvOS keeps the keyboard
    /// open between text commands and gives it D-pad focus, so the executor
    /// must not traverse back to the field while this is true.
    func keyboardVisible() -> Bool

    /// Long-press Select on the remote (tvOS). Returns `false` when the driver
    /// cannot do it.
    func longPressSelect(duration: TimeInterval) throws -> Bool

    /// The `back` action on iOS: navigation-bar back button, else edge swipe.
    /// Returns `false` when the driver cannot perform either.
    func pressBack() throws -> Bool

    /// Adjusts a picker/menu element to `value` (iOS `select`). Returns
    /// `false` when the element is not an adjustable picker.
    func selectValue(_ e: ElementRef, _ value: String) throws -> Bool

    /// Current media/player evidence. The XCUITest bundle cannot see another
    /// app's AVPlayer, so its implementation returns `nil` unless an in-app
    /// hook (D-41) or a supported system channel supplies the snapshot.
    func mediaSnapshot() -> MediaSnapshot?

    /// Current DRM evidence, again only where a legitimate channel exists.
    func drmSnapshot() -> DrmSnapshot?

    /// Fraction (0...1) of non-background pixels inside `rect` of `png`.
    /// Implemented with CoreGraphics in the XCUITest bundle; fakes used by
    /// `swift test` provide deterministic values.
    func regionCoverage(png: Data, rect: CGRect) -> Double?

    /// Whether the selected element's media is decoded and presenting frames,
    /// when the platform hook can prove it.
    func showingDecoded(_ e: ElementRef) -> Bool?

    /// The media source URL/name of the selected element, when the platform
    /// hook can prove it.
    func showingSource(_ e: ElementRef) -> String?

    /// Batch attribute read (I-198). `nil` means the driver cannot batch and
    /// the executor should use the individual `UIDriver` calls.
    func attributes(_ e: ElementRef) -> ElementAttributes?

    /// The element that currently owns tvOS focus, in one snapshot (I-201).
    /// `nil` when the driver cannot inspect the focus engine.
    func focusedIdentity() -> FocusIdentity?

    /// The selected element's identity, for matching against the focus
    /// engine without re-reading the focused element (I-201).
    func elementIdentity(_ e: ElementRef) -> FocusIdentity?
}

public extension DriverExtras {
    func prepare(appId: String, animationsDisabled: Bool) {}
    func targetCrashed(appId: String) -> Bool { false }
    func keyboardVisible() -> Bool { false }
    func longPressSelect(duration: TimeInterval) throws -> Bool { false }
    func pressBack() throws -> Bool { false }
    func selectValue(_ e: ElementRef, _ value: String) throws -> Bool { false }
    func mediaSnapshot() -> MediaSnapshot? { nil }
    func drmSnapshot() -> DrmSnapshot? { nil }
    func regionCoverage(png: Data, rect: CGRect) -> Double? { nil }
    func showingDecoded(_ e: ElementRef) -> Bool? { nil }
    func showingSource(_ e: ElementRef) -> String? { nil }
    func attributes(_ e: ElementRef) -> ElementAttributes? { nil }
    func focusedIdentity() -> FocusIdentity? { nil }
    func elementIdentity(_ e: ElementRef) -> FocusIdentity? { nil }
}
