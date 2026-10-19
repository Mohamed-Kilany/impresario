import XCTest
@testable import AgentCore

/// Spec 6.2, Apple columns: the key table is the single source of truth and is
/// verified key by key.
final class KeyTableTests: XCTestCase {
    func testTvSupportedKeys() {
        let expected: [RemoteKey: RemoteButton] = [
            .ok: .select,
            .back: .menu,
            .up: .up,
            .down: .down,
            .left: .left,
            .right: .right,
            .playPause: .playPause,
        ]
        for (key, button) in expected {
            XCTAssertEqual(KeyTable.resolve(key, platform: .tvos), .remote(button), "key \(key.rawValue)")
        }
        XCTAssertEqual(KeyTable.resolve(.home, platform: .tvos), .home)
    }

    func testTvUnsupportedKeys() {
        let unsupported: [RemoteKey] = [
            .enter, .menu, .play, .pause, .rewind, .fastForward,
            .red, .green, .yellow, .blue, .backspace,
        ]
        for key in unsupported {
            guard case .unsupported(let message) = KeyTable.resolve(key, platform: .tvos) else {
                XCTFail("expected \(key.rawValue) to be unsupported on tvos")
                continue
            }
            XCTAssertTrue(message.contains(key.rawValue), message)
        }
    }

    func testIosOnlyHomeIsSupported() {
        XCTAssertEqual(KeyTable.resolve(.home, platform: .ios), .home)
        for key in RemoteKey.allCases where key != .home {
            guard case .unsupported(let message) = KeyTable.resolve(key, platform: .ios) else {
                XCTFail("expected \(key.rawValue) to be unsupported on ios")
                continue
            }
            XCTAssertTrue(message.contains(key.rawValue), message)
        }
    }

    func testEveryWireKeyHasADecision() {
        for key in RemoteKey.allCases {
            for platform in PlatformKind.allCases {
                switch KeyTable.resolve(key, platform: platform) {
                case .home, .remote:
                    continue
                case .unsupported(let message):
                    XCTAssertFalse(message.isEmpty)
                }
            }
        }
    }
}
