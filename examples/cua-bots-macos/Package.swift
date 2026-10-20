// swift-tools-version: 6.0
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

// Cua Bots for macOS: persistent bots, each with an avatar and its own Space.
//
// Targets:
//  * `CuaBotsCore`: models, the koala avatar, rules, the Volume layout and the
//    store. Plain Swift and SwiftUI, shared with the iOS app.
//  * `CuaBotsUI`: SwiftUI pieces both apps draw (avatar picker, chat bubbles,
//    approval and sign-in cards, the rule editor).
//  * `CuaBotsCua`: the engine on the Cua SDK: a Space per bot, `cua agents`
//    runs, the routine clock, host access and the live stream, plus saved
//    sign-ins through the Cua Keyvault client (`CuaSpacesFFI`, the Cua Spaces
//    app export in `libs/spaces-app-swift`).
//  * `CuaBots`: the Mac app.
let package = Package(
    name: "CuaBotsMac",
    platforms: [.macOS(.v14), .iOS(.v18)],
    products: [
        .library(name: "CuaBotsCore", targets: ["CuaBotsCore"]),
        .library(name: "CuaBotsUI", targets: ["CuaBotsUI"]),
        .executable(name: "CuaBots", targets: ["CuaBots"]),
    ],
    dependencies: [
        .package(path: "../../libs/spaces-sdk-swift"),
        .package(name: "Cua", path: "../../libs/cua/swift"),
        .package(name: "CuaSpacesApp", path: "../../libs/spaces-app-swift"),
    ],
    targets: [
        .target(name: "CuaBotsCore", path: "Sources/CuaBotsCore"),
        .target(name: "CuaBotsUI", dependencies: ["CuaBotsCore"], path: "Sources/CuaBotsUI"),
        .target(
            name: "CuaBotsCua",
            dependencies: [
                "CuaBotsCore",
                .product(name: "Cua", package: "Cua"),
                .product(name: "CuaSpaces", package: "spaces-sdk-swift"),
                .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp"),
                .product(name: "CuaSpacesFFI", package: "CuaSpacesApp"),
            ],
            path: "Sources/CuaBotsCua"
        ),
        .executableTarget(
            name: "CuaBots",
            dependencies: ["CuaBotsCore", "CuaBotsUI", "CuaBotsCua",
                           .product(name: "CuaSpaces", package: "spaces-sdk-swift"),
                           .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp"),
                           .product(name: "CuaSpacesFFI", package: "CuaSpacesApp"),
                           .product(name: "Cua", package: "Cua")],
            path: "Sources/CuaBotsMac"
        ),
        .testTarget(name: "CuaBotsCoreTests", dependencies: ["CuaBotsCore", "CuaBotsUI"],
                    path: "Tests/CuaBotsCoreTests"),
    ],
    swiftLanguageModes: [.v5]
)
