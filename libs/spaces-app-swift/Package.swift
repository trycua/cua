// swift-tools-version: 5.9
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

// The Swift face of the Cua Spaces app export (source-available,
// FSL-1.1-MIT; `libs/cua/crates/cua-spaces-ffi`).
//
// Targets:
//  * `cua_spaces_ffiFFI`: the C header + modulemap UniFFI generates.
//  * `CuaSpacesFFI`: the generated binding (app core, Keyvault client,
//    teleport) plus `Cua.teleport()` and `Host.setupRequest(...)`, the same
//    spelling the SDK had. It uses the MIT SDK's types from `CuaSDK`
//    (`libs/cua/swift`).
//  * `CuaSpacesTeleport`: "Teleport an app…" and drag-and-drop onto a Space,
//    over `CuaSpacesFFI` and `CuaSpaces` (`libs/spaces-sdk-swift`).
//  * `CuaSpacesStreaming`: frames. SpaceStreamSession delivery, H.264
//    decode, interactive input, presence cursors and the view-shaped surface
//    that presents them, over `CuaSpaces` (`libs/spaces-sdk-swift`).
//
// The one Rust library behind both bindings is the Cua Spaces app export,
// built as `libcua_spaces_ffi` and staged where `libs/cua/swift` links its
// library (`scripts/stage-library.sh`), so the SDK and the app export share
// one runtime.
let package = Package(
    name: "CuaSpacesApp",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CuaSpacesFFI", targets: ["CuaSpacesFFI"]),
        .library(name: "CuaSpacesTeleport", targets: ["CuaSpacesTeleport"]),
        .library(name: "CuaSpacesStreaming", targets: ["CuaSpacesStreaming"]),
    ],
    dependencies: [
        .package(name: "Cua", path: "../cua/swift"),
        .package(name: "CuaSpacesSDK", path: "../spaces-sdk-swift"),
    ],
    targets: [
        .systemLibrary(name: "cua_spaces_ffiFFI", path: "Sources/cua_spaces_ffiFFI"),
        .target(
            name: "CuaSpacesFFI",
            dependencies: ["cua_spaces_ffiFFI", .product(name: "Cua", package: "Cua")],
            path: "Sources/CuaSpacesFFI"
        ),
        .target(
            name: "CuaSpacesTeleport",
            dependencies: [
                "CuaSpacesFFI",
                .product(name: "CuaSpaces", package: "CuaSpacesSDK"),
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Sources/CuaSpacesTeleport"
        ),
        .target(
            name: "CuaSpacesStreaming",
            dependencies: [
                .product(name: "CuaSpaces", package: "CuaSpacesSDK"),
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Sources/CuaSpacesStreaming",
            exclude: ["FRICTION-rcdp.md"],
            swiftSettings: [.enableExperimentalFeature("StrictConcurrency")]
        ),
        .testTarget(
            name: "CuaSpacesTeleportTests",
            dependencies: [
                "CuaSpacesTeleport",
                "CuaSpacesFFI",
                .product(name: "CuaSpaces", package: "CuaSpacesSDK"),
                "CuaSpacesStreaming",
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Tests/CuaSpacesTeleportTests",
            exclude: ["Snapshots"]
        ),
        .testTarget(
            name: "CuaSpacesStreamingTests",
            dependencies: [
                "CuaSpacesStreaming",
                .product(name: "CuaSpaces", package: "CuaSpacesSDK"),
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Tests/CuaSpacesStreamingTests"
        ),
    ]
)
