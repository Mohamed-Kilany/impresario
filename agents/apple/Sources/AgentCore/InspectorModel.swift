import Foundation

// ---------------------------------------------------------------------------
// Element inspector model (TASK-INSPECTOR, additive `Inspector` service)
//
// Like the rest of AgentCore this file is protobuf-free so `swift test` can
// exercise the provider and the fake driver on Linux. The gRPC bridge in
// AgentService converts these values to `Impresario_V1_Inspector*`.
// ---------------------------------------------------------------------------

/// Defaults match the connector and Android agent (TASK-INSPECTOR 6.3).
public let inspectorDefaultMaxNodes = 2_000
public let inspectorDefaultMaxDepth = 64

/// A rectangle in logical (screen point) coordinates.
public struct InspectorRectData: Sendable, Equatable {
    public var x: Double
    public var y: Double
    public var w: Double
    public var h: Double

    public init(x: Double, y: Double, w: Double, h: Double) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }
}

/// One normalized accessibility node; mirrors `InspectorNode` field-for-field.
public struct InspectorNodeData: Sendable, Equatable {
    public var id: UInt64
    public var parentId: UInt64
    public var role: String
    public var name: String
    public var text: String
    public var testId: String
    public var rawId: String
    public var bounds: InspectorRectData
    public var viewportBounds: InspectorRectData
    public var focused: Bool
    public var enabled: Bool
    public var selected: Bool
    public var checked: Bool?
    public var expanded: Bool?
    public var pressed: Bool?
    public var readonly: Bool
    public var required: Bool
    public var hidden: Bool
    public var secure: Bool
    public var systemUi: Bool
    public var scrollable: Bool
    public var canScrollBackward: Bool
    public var canScrollForward: Bool
    public var scrollAxis: String
    public var childrenHiddenCount: UInt32
    public var raw: String
    public var value: String
    public var label: String
    public var description: String
    public var placeholder: String
    public var alt: String
    public var title: String
    public var cssSelector: String
    public var xpath: String
    public var visibleFraction: Double
    public var escapesClip: Bool
    public var depth: UInt32
    public var textTruncated: Bool

    public init(
        id: UInt64,
        parentId: UInt64 = 0,
        role: String = "generic",
        name: String = "",
        text: String = "",
        testId: String = "",
        rawId: String = "",
        bounds: InspectorRectData = InspectorRectData(x: 0, y: 0, w: 0, h: 0),
        viewportBounds: InspectorRectData = InspectorRectData(x: 0, y: 0, w: 0, h: 0),
        focused: Bool = false,
        enabled: Bool = true,
        selected: Bool = false,
        checked: Bool? = nil,
        expanded: Bool? = nil,
        pressed: Bool? = nil,
        readonly: Bool = false,
        required: Bool = false,
        hidden: Bool = false,
        secure: Bool = false,
        systemUi: Bool = false,
        scrollable: Bool = false,
        canScrollBackward: Bool = false,
        canScrollForward: Bool = false,
        scrollAxis: String = "",
        childrenHiddenCount: UInt32 = 0,
        raw: String = "",
        value: String = "",
        label: String = "",
        description: String = "",
        placeholder: String = "",
        alt: String = "",
        title: String = "",
        cssSelector: String = "",
        xpath: String = "",
        visibleFraction: Double = 1,
        escapesClip: Bool = false,
        depth: UInt32 = 0,
        textTruncated: Bool = false
    ) {
        self.id = id
        self.parentId = parentId
        self.role = role
        self.name = name
        self.text = text
        self.testId = testId
        self.rawId = rawId
        self.bounds = bounds
        self.viewportBounds = viewportBounds
        self.focused = focused
        self.enabled = enabled
        self.selected = selected
        self.checked = checked
        self.expanded = expanded
        self.pressed = pressed
        self.readonly = readonly
        self.required = required
        self.hidden = hidden
        self.secure = secure
        self.systemUi = systemUi
        self.scrollable = scrollable
        self.canScrollBackward = canScrollBackward
        self.canScrollForward = canScrollForward
        self.scrollAxis = scrollAxis
        self.childrenHiddenCount = childrenHiddenCount
        self.raw = raw
        self.value = value
        self.label = label
        self.description = description
        self.placeholder = placeholder
        self.alt = alt
        self.title = title
        self.cssSelector = cssSelector
        self.xpath = xpath
        self.visibleFraction = visibleFraction
        self.escapesClip = escapesClip
        self.depth = depth
        self.textTruncated = textTruncated
    }
}

/// A capture request, already resolved against the active session.
public struct InspectorCaptureOptions: Sendable, Equatable {
    public var includeSystemUi: Bool
    public var minVisibleFraction: Double
    public var maxNodes: Int
    public var maxDepth: Int
    public var includeRaw: Bool
    public var includeScreenshot: Bool
    public var screenshotScale: Int
    public var appId: String
    public var pixelRatio: Double
    public var screenWidthPx: Int
    public var screenHeightPx: Int

    public init(
        includeSystemUi: Bool = false,
        minVisibleFraction: Double = 0,
        maxNodes: Int = 0,
        maxDepth: Int = 0,
        includeRaw: Bool = false,
        includeScreenshot: Bool = false,
        screenshotScale: Int = 100,
        appId: String = "",
        pixelRatio: Double = 1,
        screenWidthPx: Int = 0,
        screenHeightPx: Int = 0
    ) {
        self.includeSystemUi = includeSystemUi
        self.minVisibleFraction = minVisibleFraction
        self.maxNodes = maxNodes > 0 ? maxNodes : inspectorDefaultMaxNodes
        self.maxDepth = maxDepth > 0 ? maxDepth : inspectorDefaultMaxDepth
        self.includeRaw = includeRaw
        self.includeScreenshot = includeScreenshot
        self.screenshotScale = screenshotScale == 0 ? 100 : min(max(1, screenshotScale), 100)
        self.appId = appId
        self.pixelRatio = pixelRatio > 0 ? pixelRatio : 1
        self.screenWidthPx = screenWidthPx
        self.screenHeightPx = screenHeightPx
    }

    /// The logical viewport; falls back to the driver's device info.
    public func viewportSize(fallback: AgentDeviceInfo) -> (width: Double, height: Double) {
        let ratio = pixelRatio > 0 ? pixelRatio : max(1, fallback.pixelRatio)
        let widthPx = screenWidthPx > 0 ? screenWidthPx : fallback.screenWidthPx
        let heightPx = screenHeightPx > 0 ? screenHeightPx : fallback.screenHeightPx
        return (Double(widthPx) / ratio, Double(heightPx) / ratio)
    }
}

/// A viewport-pruned accessibility tree plus its focused node.
public struct InspectorCapture: Sendable, Equatable {
    public var nodes: [InspectorNodeData]
    public var focusedId: UInt64
    public var truncated: Bool
    public var degraded: [String]
    public var screenshotPng: Data?

    public init(
        nodes: [InspectorNodeData],
        focusedId: UInt64 = 0,
        truncated: Bool = false,
        degraded: [String] = [],
        screenshotPng: Data? = nil
    ) {
        self.nodes = nodes
        self.focusedId = focusedId
        self.truncated = truncated
        self.degraded = degraded
        self.screenshotPng = screenshotPng
    }
}

/// One inspector input action, converted from `InspectorInputRequest`.
public enum InspectorInputAction: Sendable, Equatable {
    case tap(nodeId: UInt64, x: Double, y: Double)
    case scroll(direction: Direction, count: Int, nodeId: UInt64, x: Double, y: Double)
    case key(RemoteKey)
    case type(nodeId: UInt64, text: String)
    case swipe(x1: Double, y1: Double, x2: Double, y2: Double, durationMs: Int)
}

/// The inspector side of a UI driver. The XCUITest driver implements it; the
/// fake drivers used by `swift test` may omit it, in which case the provider
/// returns `UNIMPLEMENTED` (the handshake capability still describes the real
/// device driver).
public protocol InspectorDriver: AnyObject {
    func capture(_ options: InspectorCaptureOptions) throws -> InspectorCapture

    /// Capture only the current frame at `scalePercent` (1-100).
    func screenshot(scalePercent: Int) throws -> Data?

    func inspectorInput(_ action: InspectorInputAction) throws
}

public extension InspectorDriver {
    func screenshot(scalePercent: Int) throws -> Data? { nil }
}
