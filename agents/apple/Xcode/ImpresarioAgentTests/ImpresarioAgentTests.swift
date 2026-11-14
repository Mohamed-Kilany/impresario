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

        // Never touch XCUIApplication from this test. `testServe` is async and
        // owns no XCTest main context, so *any* XCUI query that records a test
        // activity aborts the run with `Current context must not be nil`
        // (I-194). That rules out `addUIInterruptionMonitor` and direct
        // SpringBoard polling. The one-time local-network prompt is granted by
        // a human; the device wizard and `impresario doctor` detect and explain
        // that state (see docs/DEVICES.md).

        let handle = try service.start()
        print(
            AgentEnvironment.summary(
                platform: platform,
                port: handle.port,
                bonjour: config.advertiseBonjour,
                token: !token.isEmpty
            )
        )
        print("impresario agent: serving; waiting for a master (Shutdown ends the run)")

        await handle.serveUntilShutdown()
        print("impresario agent: Shutdown received, agent exiting")
    }
}
