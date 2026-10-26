import AgentCore
import AgentService
import Foundation
import XCTest

/// The agent UI test (spec 12.1): its `testServe` method starts Impresario
/// Agent with the real `XCUIDriver` and **blocks until a master sends
/// `Shutdown`** (D-18), so one Appium-style automation session per run is
/// served without restarting the test bundle.
///
/// The same source is compiled into `ImpresarioAgentTests-iOS` and
/// `ImpresarioAgentTests-tvOS`; the platform is selected at compile time.
/// `continueAfterFailure = false` keeps a failed start from retrying the bind.
final class ImpresarioAgentTests: XCTestCase {
    func testServe() async throws {
        continueAfterFailure = false

        let environment = ProcessInfo.processInfo.environment
        let platform = AgentEnvironment.platform()
        let token = AgentEnvironment.token(environment)
        let driver = XCUIDriver()
        let config = AgentServiceConfig(
            platform: platform,
            port: AgentServiceConfig.portFromEnvironment(environment),
            token: token,
            advertiseBonjour: AgentEnvironment.advertisesBonjour(environment),
            bonjourServiceName: AgentEnvironment.bonjourName(
                deviceName: AgentEnvironment.deviceName(),
                platform: platform
            )
        )
        let service = AgentService(config: config, driver: driver)

        // Alerts must be handled before they block the first connection.
        //
        // `addUIInterruptionMonitor` is main-thread-only; this async test starts
        // on a Swift-concurrency thread, so XCTest aborts with "No current
        // XCTContext" on tvOS. tvOS also has no touch input, so a monitor could
        // never tap Allow. The one-time local-network prompt is granted with
        // the remote instead (see SystemAlerts.tapAllow).
        #if !os(tvOS)
        SystemAlerts.register(on: self)
        #endif

        let handle = try service.start()
        print(
            AgentEnvironment.summary(
                platform: platform,
                port: handle.port,
                bonjour: config.advertiseBonjour,
                token: !token.isEmpty
            )
        )

        // The local-network prompt appears right after the first bind; tap it
        // so a WLAN run needs no human (I-88). tvOS cannot tap, and polling
        // SpringBoard for 20 s would only delay the agent, so skip it there.
        #if !os(tvOS)
        SystemAlerts.allowLocalNetworkIfPrompted()
        #endif
        print("impresario agent: serving; waiting for a master (Shutdown ends the run)")

        await handle.serveUntilShutdown()
        print("impresario agent: Shutdown received, agent exiting")
    }
}
