// swift-tools-version: 6.0
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

// infinite-canvas: every window of every Space on one zoomable canvas.
//
// Three targets, split by what they may depend on:
// - CanvasModel: pure value types (camera, layout, level of detail, hotkey
//   state, presence styling). No AppKit, no SDK. Everything the tests pin.
// - CanvasStreaming: one tile's pipeline on the cua SDK: a SpaceStreamSession
//   per window, VideoToolbox hardware decode off the main thread, frames
//   enqueued straight into an AVSampleBufferDisplayLayer, and the per-window
//   stream preferences the level-of-detail policy asks for.
// - InfiniteCanvasApp: the AppKit overlay, SwiftUI chrome and the SDK wiring
//   (Spaces, presence, agents over ACP).
let package = Package(
    name: "InfiniteCanvas",
    platforms: [.macOS("26.0")],
    products: [
        .executable(name: "infinite-canvas", targets: ["InfiniteCanvasApp"]),
        .executable(name: "canvas-bench", targets: ["CanvasBench"]),
    ],
    dependencies: [
        .package(path: "../../libs/spaces-sdk-swift"),
        .package(name: "Cua", path: "../../libs/cua/swift"),
        // `CuaSpacesStreaming`, source-available (FSL-1.1-MIT) like the rest
        // of Cua Spaces.
        .package(name: "CuaSpacesApp", path: "../../libs/spaces-app-swift"),
    ],
    targets: [
        .target(name: "CanvasModel", path: "Sources/CanvasModel"),
        .target(
            name: "CanvasStreaming",
            dependencies: [
                "CanvasModel",
                .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp"),
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Sources/CanvasStreaming"
        ),
        .executableTarget(
            name: "InfiniteCanvasApp",
            dependencies: [
                "CanvasModel",
                "CanvasStreaming",
                .product(name: "CuaSpaces", package: "spaces-sdk-swift"),
                .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp"),
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Sources/InfiniteCanvasApp",
            resources: [.copy("Resources/cua.default.lottie")]
        ),
        .executableTarget(
            name: "CanvasBench",
            dependencies: ["CanvasModel", "CanvasStreaming"],
            path: "Sources/CanvasBench"
        ),
        .testTarget(
            name: "InfiniteCanvasTests",
            dependencies: ["CanvasModel", "CanvasStreaming", "InfiniteCanvasApp"],
            path: "Tests/InfiniteCanvasTests"
        ),
    ],
    swiftLanguageModes: [.v5]
)
