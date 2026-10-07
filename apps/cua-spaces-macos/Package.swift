// swift-tools-version: 6.2
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

// Cua Spaces for macOS: a native SwiftUI shell over the same app core as the
// Tauri app (libs/cua/crates/cua-spaces-app-core), bound through the Cua
// Spaces app export (libs/spaces-app-swift, module CuaSpacesFFI) beside the
// cua SDK's (libs/cua/swift, module CuaSDK). Views only render; every
// decision is the core's.
//
//  * CuaSpacesMacKit: view models (thin holders of core state), views, the
//    notch panel and the menu bar extra.
//  * CuaSpacesMac: the app entry point.
//  * CuaSpacesMacTests: view-model, parity and snapshot tests (swift-testing).
//
// Sparkle (pinned; Package.resolved) is the macOS updater: a binary
// framework that scripts/build-app.sh embeds in Contents/Frameworks and
// scripts/build-release.sh signs from the inside out.
//
// UI tests (XCUITest) live in UITests/ and build through project.yml
// (XcodeGen) on a macOS runner with Xcode.
let package = Package(
    name: "CuaSpacesMac",
    platforms: [.macOS(.v26)],
    products: [
        .executable(name: "CuaSpacesMac", targets: ["CuaSpacesMac"]),
        .library(name: "CuaSpacesMacKit", targets: ["CuaSpacesMacKit"]),
    ],
    dependencies: [
        .package(name: "Cua", path: "../../libs/cua/swift"),
        .package(name: "CuaSpacesSDK", path: "../../libs/spaces-sdk-swift"),
        .package(name: "CuaSpacesApp", path: "../../libs/spaces-app-swift"),
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.0"),
    ],
    targets: [
        .target(
            name: "CuaSpacesMacKit",
            dependencies: [
                .product(name: "Cua", package: "Cua"),
                .product(name: "CuaSpaces", package: "CuaSpacesSDK"),
                .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp"),
                .product(name: "CuaSpacesFFI", package: "CuaSpacesApp"),
                .product(name: "CuaSpacesTeleport", package: "CuaSpacesApp"),
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            path: "Sources/CuaSpacesMacKit",
            resources: [.process("Resources")],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "CuaSpacesMac",
            dependencies: ["CuaSpacesMacKit", .product(name: "CuaSpacesFFI", package: "CuaSpacesApp")],
            path: "Sources/CuaSpacesMac",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "CuaSpacesMacTests",
            dependencies: ["CuaSpacesMacKit", .product(name: "Cua", package: "Cua"),
                           .product(name: "CuaSpacesFFI", package: "CuaSpacesApp"),
                           .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp")],
            path: "Tests/CuaSpacesMacTests",
            resources: [.copy("Snapshots")],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
