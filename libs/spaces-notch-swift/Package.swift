// swift-tools-version: 6.2
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

// The Cua Spaces notch: the overlay at the top of a Mac's screen with the
// Space tiles. One set of views, two hosts:
//
//  * `CuaSpacesNotchUI`: the views, the panel and the plain data they draw
//    (`NotchData`). The SwiftUI app (apps/cua-spaces-macos) feeds them from
//    the app core directly; nothing here links the core.
//  * `CuaSpacesNotchHelper`: the helper's side of the notch protocol
//    (newline-delimited JSON on stdin and stdout) and the surface it drives.
//  * `CuaSpacesNotch`: "Cua Spaces Notch.app", the agent app the Electron app
//    (apps/cua-spaces-desktop) bundles and launches on macOS. Electron runs
//    the app core's notch model and sends the view; the helper draws it and
//    sends input and clicks back.
//
// Build the helper bundle with apps/cua-spaces-desktop `pnpm notch`.
let package = Package(
    name: "CuaSpacesNotch",
    platforms: [.macOS(.v26)],
    products: [
        .library(name: "CuaSpacesNotchUI", targets: ["CuaSpacesNotchUI"]),
        .library(name: "CuaSpacesNotchHelper", targets: ["CuaSpacesNotchHelper"]),
        .executable(name: "CuaSpacesNotch", targets: ["CuaSpacesNotch"]),
    ],
    targets: [
        .target(
            name: "CuaSpacesNotchUI",
            path: "Sources/CuaSpacesNotchUI",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "CuaSpacesNotchHelper",
            dependencies: ["CuaSpacesNotchUI"],
            path: "Sources/CuaSpacesNotchHelper",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "CuaSpacesNotch",
            dependencies: ["CuaSpacesNotchHelper"],
            path: "Sources/CuaSpacesNotch",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "CuaSpacesNotchTests",
            dependencies: ["CuaSpacesNotchUI", "CuaSpacesNotchHelper"],
            path: "Tests/CuaSpacesNotchTests",
            // Read by path (the Electron side's tests read the same files).
            exclude: ["Fixtures"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
