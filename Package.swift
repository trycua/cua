// swift-tools-version: 6.0
import PackageDescription

// Root shim: re-exports cua-driver Swift package products so Swift packages
// can consume them directly from the trycua/cua monorepo without knowing the
// internal layout. Sources live in libs/cua-driver/swift/Sources/; this file
// uses path: to forward there.
//
// CuaDriverCore and CuaDriverServer are the legacy Swift implementation.
// CuaDriverEmbedded is the Swift wrapper for the Rust embedded driver C ABI.
// SPM does not build Rust crates by itself, so apps using CuaDriverEmbedded
// must also link libcua_driver_embedded.a/.dylib or a packaged xcframework.
//
// IMPORTANT — SPM version resolution:
// SPM's `from:` / `upToNextMajor` only recognises semver tags ("0.1.0",
// "v0.1.0"). This repo uses "cua-driver-v*" tags for the CLI releases, which
// SPM cannot parse. Until plain semver tags are published, pin by revision:
//
//   .package(url: "https://github.com/trycua/cua.git", .revision("cua-driver-v0.2.18"))
//
// When the repo starts publishing semver tags alongside the CLI tags, use:
//
//   .package(url: "https://github.com/trycua/cua.git", from: "0.1.0")
//
// Then in your target's dependencies:
//
//   .product(name: "CuaDriverCore", package: "cua")   // AX, input, capture, recording
//   .product(name: "CuaDriverServer", package: "cua") // MCP tool handlers + daemon layer
//   .product(name: "CuaDriverEmbedded", package: "cua") // Rust embedded MCP wrapper

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

        // Thin Swift wrapper over the Rust embedded driver's C ABI.
        // The host app must link the Rust static library, dylib, or xcframework.
        .library(name: "CuaDriverEmbedded", targets: ["CuaDriverEmbedded"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/modelcontextprotocol/swift-sdk.git",
            from: "0.9.0"
        ),
    ],
    targets: [
        // NOTE: if libs/cua-driver/swift/Package.swift ever gains resources:,
        // swiftSettings:, linkerSettings:, or exclude: on these targets,
        // mirror those changes here to avoid a silent build mismatch.
        .target(
            name: "CuaDriverCore",
            path: "libs/cua-driver/swift/Sources/CuaDriverCore"
        ),
        .target(
            name: "CuaDriverServer",
            dependencies: [
                "CuaDriverCore",
                .product(name: "MCP", package: "swift-sdk"),
            ],
            path: "libs/cua-driver/swift/Sources/CuaDriverServer"
        ),
        .target(
            name: "CuaDriverEmbedded",
            path: "libs/cua-driver/swift/Sources/CuaDriverEmbedded"
        ),
    ]
)
