import AgentCore
import Dispatch
import Foundation
import GRPC
import ImpresarioWire

/// Serves the additive `Inspector` gRPC service (TASK-INSPECTOR 4.2/6.3).
///
/// The inspector attaches to the live `Agent` handshake session: every RPC
/// carries the same token and `x-impresario-session-id` metadata, and the
/// driver is the one the session prepared. Capture work runs on the driver's
/// own serial queue (the XCUITest driver owns it), so this provider only
/// converts wire values and owns the executor-side revision.
public final class InspectorProvider: Impresario_V1_InspectorAsyncProvider, @unchecked Sendable {
    private let state: AgentState
    private let expectedToken: String
    private let lock = NSLock()
    private var revision: UInt64 = 0

    public init(state: AgentState, token: String = "") {
        self.state = state
        self.expectedToken = token
    }

    // ------------------------------------------------------------------
    // Inspector.Snapshot
    // ------------------------------------------------------------------

    public func snapshot(
        request: Impresario_V1_InspectorSnapshotRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Impresario_V1_InspectorSnapshotResponse {
        try authorize(context)
        let appId = try requireSession(context)
        guard let driver = state.uiDriver as? InspectorDriver else {
            throw GRPCStatus(
                code: .unimplemented,
                message: "this executor has no inspector driver"
            )
        }

        let info = state.uiDriver.deviceInfo()
        let logical = logicalSize(info)
        let started = DispatchTime.now().uptimeNanoseconds
        let rev = nextRevision()

        if request.screenshotOnly {
            let scale = request.screenshotScale == 0
                ? 100
                : Int(request.screenshotScale.clamped(to: 1...100))
            do {
                let png = try driver.screenshot(scalePercent: scale)
                var response = baseResponse(info: info, logical: logical, rev: rev)
                response.settled = true
                response.acquisitionMs = elapsedMs(since: started)
                attachScreenshot(png, to: &response)
                return response
            } catch let error as AgentError {
                var response = baseResponse(info: info, logical: logical, rev: rev)
                response.error = ProtoConversion.errorInfo(error.report)
                response.acquisitionMs = elapsedMs(since: started)
                return response
            } catch {
                var response = baseResponse(info: info, logical: logical, rev: rev)
                response.error = ProtoConversion.errorInfo(
                    AgentError.internalError(String(describing: error)).report
                )
                response.acquisitionMs = elapsedMs(since: started)
                return response
            }
        }

        let options = InspectorCaptureOptions(
            includeSystemUi: request.includeSystemUi,
            minVisibleFraction: Double(request.minVisibleFraction),
            maxNodes: Int(request.maxNodes),
            maxDepth: Int(request.maxDepth),
            includeRaw: request.includeRaw,
            includeScreenshot: request.includeScreenshot,
            screenshotScale: Int(request.screenshotScale),
            appId: appId,
            pixelRatio: info.pixelRatio,
            screenWidthPx: info.screenWidthPx,
            screenHeightPx: info.screenHeightPx
        )

        do {
            let capture = try driver.capture(options)
            var response = baseResponse(info: info, logical: logical, rev: rev)
            response.settled = true
            response.acquisitionMs = elapsedMs(since: started)
            response.nodes = capture.nodes.map(ProtoConversion.inspectorNode)
            response.degraded = capture.degraded
            response.focusedID = capture.focusedId
            response.truncated = capture.truncated
            response.timestampUnixMs = nowMs()
            attachScreenshot(capture.screenshotPng, to: &response)
            return response
        } catch let error as AgentError {
            var response = baseResponse(info: info, logical: logical, rev: rev)
            response.error = ProtoConversion.errorInfo(error.report)
            response.acquisitionMs = elapsedMs(since: started)
            response.timestampUnixMs = nowMs()
            return response
        } catch {
            var response = baseResponse(info: info, logical: logical, rev: rev)
            response.error = ProtoConversion.errorInfo(
                AgentError.internalError(String(describing: error)).report
            )
            response.acquisitionMs = elapsedMs(since: started)
            response.timestampUnixMs = nowMs()
            return response
        }
    }

    // ------------------------------------------------------------------
    // Inspector.Input
    // ------------------------------------------------------------------

    public func input(
        request: Impresario_V1_InspectorInputRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Impresario_V1_InspectorInputResponse {
        try authorize(context)
        _ = try requireSession(context)
        guard let driver = state.uiDriver as? InspectorDriver else {
            throw GRPCStatus(
                code: .unimplemented,
                message: "this executor has no inspector driver"
            )
        }

        do {
            let action = try ProtoConversion.inspectorInput(request)
            try driver.inspectorInput(action)
            var response = Impresario_V1_InspectorInputResponse()
            response.ok = true
            response.rev = currentRevision
            return response
        } catch let error as AgentError {
            var response = Impresario_V1_InspectorInputResponse()
            response.ok = false
            response.error = ProtoConversion.errorInfo(error.report)
            response.rev = currentRevision
            return response
        } catch {
            var response = Impresario_V1_InspectorInputResponse()
            response.ok = false
            response.error = ProtoConversion.errorInfo(
                AgentError.internalError(String(describing: error)).report
            )
            response.rev = currentRevision
            return response
        }
    }

    // ------------------------------------------------------------------
    // internals
    // ------------------------------------------------------------------

    private func baseResponse(
        info: AgentDeviceInfo,
        logical: (width: Double, height: Double),
        rev: UInt64
    ) -> Impresario_V1_InspectorSnapshotResponse {
        var response = Impresario_V1_InspectorSnapshotResponse()
        response.rev = rev
        response.deviceName = info.model
        response.platform = ProtoConversion.platform(state.platform)
        response.logicalWidth = Float(logical.width)
        response.logicalHeight = Float(logical.height)
        response.viewportWidth = Float(logical.width)
        response.viewportHeight = Float(logical.height)
        response.pixelRatio = Float(info.pixelRatio)
        response.timestampUnixMs = nowMs()
        return response
    }

    private func attachScreenshot(
        _ png: Data?,
        to response: inout Impresario_V1_InspectorSnapshotResponse
    ) {
        guard let png, !png.isEmpty else { return }
        response.screenshotPng = png
        if let size = Png.dimensions(png) {
            response.screenshotWidth = UInt32(clamping: max(0, size.width))
            response.screenshotHeight = UInt32(clamping: max(0, size.height))
        }
    }

    private func logicalSize(_ info: AgentDeviceInfo) -> (width: Double, height: Double) {
        let ratio = info.pixelRatio > 0 ? info.pixelRatio : 1
        return (Double(info.screenWidthPx) / ratio, Double(info.screenHeightPx) / ratio)
    }

    private func nextRevision() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        revision += 1
        return revision
    }

    private var currentRevision: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return revision
    }

    private func elapsedMs(since started: UInt64) -> UInt32 {
        let nanos = DispatchTime.now().uptimeNanoseconds - started
        return UInt32(clamping: Int(nanos / 1_000_000))
    }

    private func nowMs() -> UInt64 {
        UInt64(clamping: Int64(Date().timeIntervalSince1970 * 1000))
    }

    /// A configured token must be presented by every RPC (spec 7.2.1).
    private func authorize(_ context: GRPCAsyncServerCallContext) throws {
        guard !expectedToken.isEmpty else { return }
        let presented = context.request.headers.first(name: AgentMetadata.tokenHeader)
        guard presented == expectedToken else {
            throw GRPCStatus(
                code: .unauthenticated,
                message: "missing or invalid \(AgentMetadata.tokenHeader)"
            )
        }
    }

    /// Every inspector RPC names the live session that owns the driver.
    private func requireSession(
        _ context: GRPCAsyncServerCallContext
    ) throws -> String {
        guard
            let sessionID = context.request.headers.first(name: AgentMetadata.sessionIDHeader),
            !sessionID.isEmpty
        else {
            throw GRPCStatus(
                code: .invalidArgument,
                message: "missing \(AgentMetadata.sessionIDHeader) metadata"
            )
        }
        guard let appId = state.inspectorSession(sessionID) else {
            throw GRPCStatus(
                code: .notFound,
                message: "unknown session '\(sessionID)'"
            )
        }
        return appId
    }
}

private extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
    }
}
