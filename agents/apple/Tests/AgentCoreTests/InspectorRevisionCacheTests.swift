import XCTest
@testable import AgentCore

/// `NODE_REF` addressing (INS-R4): the master resolves a canonical `LOCATOR`
/// against one Inspector snapshot and then sends revision + node id. The
/// executor must resolve the current and previous capture, treat node id `0` as
/// an explicit miss, and answer `STALE_ELEMENT` for anything it no longer has.
final class InspectorRevisionCacheTests: XCTestCase {
    func testCurrentAndPreviousRevisionsResolve() throws {
        var cache = InspectorRevisionCache<Int>()
        cache.store(revision: 1, handles: [10: 100])
        cache.store(revision: 2, handles: [10: 200, 11: 201])

        XCTAssertEqual(try cache.resolve(revision: 2, nodeID: 10), 200)
        XCTAssertEqual(try cache.resolve(revision: 1, nodeID: 10), 100)
    }

    func testThirdCaptureEvictsTheOldestRevision() {
        var cache = InspectorRevisionCache<Int>()
        cache.store(revision: 1, handles: [10: 100])
        cache.store(revision: 2, handles: [10: 200])
        cache.store(revision: 3, handles: [10: 300])

        XCTAssertNil(cache.handles(revision: 1))
        XCTAssertThrowsError(try cache.resolve(revision: 1, nodeID: 10)) { error in
            XCTAssertEqual((error as? AgentError)?.code, .staleElement)
        }
        XCTAssertEqual(try? cache.resolve(revision: 2, nodeID: 10), 200)
        XCTAssertEqual(try? cache.resolve(revision: 3, nodeID: 10), 300)
    }

    func testUnknownNodeInAKnownRevisionIsStale() {
        var cache = InspectorRevisionCache<Int>()
        cache.store(revision: 5, handles: [1: 11])

        XCTAssertThrowsError(try cache.resolve(revision: 5, nodeID: 2)) { error in
            XCTAssertEqual((error as? AgentError)?.code, .staleElement)
        }
    }

    func testNodeZeroIsAnExplicitMiss() throws {
        var cache = InspectorRevisionCache<Int>()
        cache.store(revision: 5, handles: [1: 11])

        XCTAssertNil(try cache.resolve(revision: 5, nodeID: 0))
        // A miss never needs a live revision: the master uses node 0 to tell
        // the executor a probe matched nothing.
        XCTAssertNil(try cache.resolve(revision: 99, nodeID: 0))
    }

    func testZeroRevisionIsNotStored() {
        var cache = InspectorRevisionCache<Int>()
        cache.store(revision: 0, handles: [1: 11])

        XCTAssertNil(cache.handles(revision: 0))
    }

    func testCaptureOptionsCarryTheRevision() {
        XCTAssertEqual(InspectorCaptureOptions().revision, 0)
        let options = InspectorCaptureOptions(
            appId: "com.example.app",
            revision: 42
        )
        XCTAssertEqual(options.revision, 42)
    }
}
