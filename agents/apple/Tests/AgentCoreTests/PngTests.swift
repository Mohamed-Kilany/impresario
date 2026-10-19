import XCTest
@testable import AgentCore

final class PngTests: XCTestCase {
    func testReadsIhdrDimensions() {
        let dimensions = Png.dimensions(FakePNG.valid)
        XCTAssertEqual(dimensions?.width, 1)
        XCTAssertEqual(dimensions?.height, 1)
    }

    func testRejectsGarbage() {
        XCTAssertNil(Png.dimensions(Data()))
        XCTAssertNil(Png.dimensions(Data([0x00, 0x01, 0x02])))
        XCTAssertNil(Png.dimensions(Data(repeating: 0x41, count: 64)))
    }

    func testRejectsTruncatedPng() {
        XCTAssertNil(Png.dimensions(FakePNG.valid.prefix(16)))
    }

    func testRejectsZeroDimensions() {
        var bytes = [UInt8](FakePNG.valid.prefix(24))
        bytes[16] = 0
        bytes[17] = 0
        bytes[18] = 0
        bytes[19] = 0
        XCTAssertNil(Png.dimensions(Data(bytes)))
    }

    func testReadsLargeDimensionsBigEndian() {
        var bytes = [UInt8](FakePNG.valid.prefix(24))
        bytes[16] = 0x00
        bytes[17] = 0x00
        bytes[18] = 0x07
        bytes[19] = 0x80 // 1920
        bytes[20] = 0x00
        bytes[21] = 0x00
        bytes[22] = 0x04
        bytes[23] = 0x38 // 1080
        let dimensions = Png.dimensions(Data(bytes))
        XCTAssertEqual(dimensions?.width, 1920)
        XCTAssertEqual(dimensions?.height, 1080)
    }
}
