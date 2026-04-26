// swift-tools-version: 6.0
import PackageDescription

// Root shim: re-exports CuaDriverCore and CuaDriverServer so Swift packages
// can consume them directly from the trycua/cua monorepo without knowing the
// internal layout. Sources live in libs/cua-driver/Sources/; this file uses
// path: to forward there.
//
// Add to your Package.swift:
//
//   .package(url: "https://github.com/trycua/cua.git", from: "cua-driver-v0.1.0")
//
// Then in your target's dependencies:
//
//   .product(name: "CuaDriverCore", package: "cua")   // AX, input, capture, recording
//   .product(name: "CuaDriverServer", package: "cua") // MCP tool handlers + daemon layer

let package = Package(
    name: "cua",
    platforms: [.macOS(.v14)],
    products: [
        // Accessibility, input, capture, app-launch, recording primitives.
        // No external dependencies — system frameworks only.
        .library(name: "CuaDriverCore", targets: ["CuaDriverCore"]),

        // MCP tool handlers and daemon server built on top of CuaDriverCore.
        // Depends on modelcontextprotocol/swift-sdk for the MCP protocol types.
        .library(name: "CuaDriverServer", targets: ["CuaDriverServer"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/modelcontextprotocol/swift-sdk.git",
            from: "0.9.0"
        ),
    ],
    targets: [
        .target(
            name: "CuaDriverCore",
            path: "libs/cua-driver/Sources/CuaDriverCore"
        ),
        .target(
            name: "CuaDriverServer",
            dependencies: [
                "CuaDriverCore",
                .product(name: "MCP", package: "swift-sdk"),
            ],
            path: "libs/cua-driver/Sources/CuaDriverServer"
        ),
    ]
)
