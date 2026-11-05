// swift-tools-version:5.10
//
// Impresario Agent for Apple (iOS/tvOS), spec TASK.md section 12.
//
// Layout (decision D-13, D-14):
//   AgentCore       — pure Swift (Foundation only): wire model, selector parsing,
//                     key tables, FocusNavigator, the ordered dispatcher and the
//                     command executor. `swift test` runs it on any platform,
//                     including macOS and Linux.
//   ImpresarioWire  — the committed protobuf/gRPC code generated from
//                     proto/impresario/v1/impresario.proto (regenerate with
//                     scripts/gen-swift.sh; do not edit).
//   AgentService    — bridges ImpresarioWire <-> AgentCore and serves the gRPC
//                     `Agent` service on SwiftNIO (grpc-swift 1.x).
//
// The XCUITest bundle target lives in the Xcode project (project.yml): it
// implements `UIDriver` over XCUIApplication and calls `AgentService.run`.
import PackageDescription

let package = Package(
    name: "AgentCore",
    platforms: [
        .iOS(.v16),
        .tvOS(.v16),
        .macOS(.v13),
    ],
    products: [
        .library(name: "AgentCore", targets: ["AgentCore"]),
        .library(name: "ImpresarioWire", targets: ["ImpresarioWire"]),
        .library(name: "AgentService", targets: ["AgentService"]),
        .library(name: "ImpresarioHook", targets: ["ImpresarioHook"]),
    ],
    dependencies: [
        // Pinned per TASK.md D-14 (grpc-swift 1.x while the OS floor is below
        // the minimum of grpc-swift 2). See docs/DECISIONS.md I-83.
        .package(url: "https://github.com/apple/swift-protobuf.git", exact: "1.39.0"),
        .package(url: "https://github.com/grpc/grpc-swift.git", exact: "1.27.6"),
        // NIO is declared explicitly because the generated gRPC code and the
        // server glue name EventLoopFuture/EventLoopGroup; Package.resolved
        // pins the transitive graph.
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.65.0"),
        // NIOHPACK carries the gRPC metadata type (x-impresario-session-id /
        // -token); the version matches the pin grpc-swift resolves.
        .package(url: "https://github.com/apple/swift-nio-http2.git", from: "1.46.0"),
        // swift-log is declared explicitly because both gates need a manifest
        // they understand: 1.9.1 is the newest release that still uses
        // tools-version 6.1 and has no unsafe build flags (1.10+ adds
        // -require-explicit-sendable, which SwiftPM rejects in a dependency,
        // and 1.10.1+ also requires tools-version 6.2 that Xcode 16.4's Swift
        // 6.1.2 cannot load). 1.17.0 additionally uses `@nonexhaustive`, which
        // the Linux Swift 6.2.1 toolchain does not know.
        .package(url: "https://github.com/apple/swift-log.git", exact: "1.9.1"),
    ],
    targets: [
        .target(name: "AgentCore"),
        .target(name: "ImpresarioHook"),
        .target(
            name: "ImpresarioWire",
            dependencies: [
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
                .product(name: "GRPC", package: "grpc-swift"),
                .product(name: "NIO", package: "swift-nio"),
            ]
        ),
        .target(
            name: "AgentService",
            dependencies: [
                "AgentCore",
                "ImpresarioWire",
                .product(name: "GRPC", package: "grpc-swift"),
                .product(name: "NIO", package: "swift-nio"),
                .product(name: "NIOHPACK", package: "swift-nio-http2"),
            ]
        ),
        .testTarget(name: "AgentCoreTests", dependencies: ["AgentCore"]),
        .testTarget(
            name: "AgentServiceTests",
            dependencies: [
                "AgentCore",
                "AgentService",
                "ImpresarioWire",
                .product(name: "GRPC", package: "grpc-swift"),
                .product(name: "NIO", package: "swift-nio"),
            ]
        ),
    ]
)
