import XCTest
@testable import AgentCore

final class SelectorSpecTests: XCTestCase {
    func testDescribeAndLookup() {
        let spec = SelectorSpec(strategy: .accessibilityId, value: "login", index: 2)
        XCTAssertEqual(spec.describe(), "accessibilityId:login#2")
        XCTAssertEqual(spec.lookup().index, 0)
        XCTAssertEqual(spec.lookup().value, "login")
    }

    func testKnownIosTypesAreAccepted() throws {
        for type in AgentElementType.allCases {
            let spec = try SelectorSpec.make(strategy: .iosType, value: type.rawValue, index: 0)
            XCTAssertEqual(spec.value, type.rawValue)
        }
    }

    func testUnknownIosTypeIsAnInvalidSelector() {
        XCTAssertThrowsError(try SelectorSpec.make(strategy: .iosType, value: "buton", index: 0)) { error in
            guard let agentError = error as? AgentError else {
                return XCTFail("expected an AgentError, got \(error)")
            }
            XCTAssertEqual(agentError.code, .invalidSelector)
            XCTAssertTrue(agentError.message.contains("buton"))
        }
    }

    func testOtherStrategiesAreNotTypeChecked() throws {
        XCTAssertNoThrow(try SelectorSpec.make(strategy: .label, value: "notAType", index: 0))
        XCTAssertNoThrow(try SelectorSpec.make(strategy: .predicate, value: "label BEGINSWITH 'x'", index: 0))
    }

    func testElementTypeCoversSpecNames() {
        // Names from spec 5.3 that the parser must accept.
        let required = [
            "button", "staticText", "textField", "secureTextField", "switch",
            "scrollView", "collectionView", "table", "cell", "image", "icon",
            "navigationBar", "tabBar", "alert", "sheet", "slider", "pickerWheel",
            "segmentedControl", "webView", "other", "any",
        ]
        for name in required {
            XCTAssertNotNil(AgentElementType(rawValue: name), "missing element type \(name)")
        }
    }
}
