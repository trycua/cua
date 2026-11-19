// swift-tools-version: 6.0
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

// Cua Bots for iPhone: a remote client for the bots your Mac runs. It
// reaches each bot's own Space over the relay (or directly), streams its
// computer, chats, and answers approvals and notifications.
//
//  * `CuaBotsRemote`: the connection, on the Cua Swift SDK (`Relay`,
//    `SpacesdClient` files and media).
//  * `CuaBotsPhone`: the SwiftUI screens.
//  * `CuaBotsPhonePreview`: a macOS host that shows the phone screens in a
//    phone-sized window against a real bot, for development and screenshots
//    on a Mac without Xcode. The iPhone app itself is `App/` (see README).
let package = Package(
    name: "CuaBotsiOS",
    platforms: [.iOS(.v18), .macOS(.v14)],
    products: [
        .library(name: "CuaBotsRemote", targets: ["CuaBotsRemote"]),
        .library(name: "CuaBotsPhone", targets: ["CuaBotsPhone"]),
    ],
    dependencies: [
        .package(path: "../cua-bots-macos"),
        .package(name: "Cua", path: "../../libs/cua/swift"),
        // Decoded frames of a bot's screen (`openMediaDecoded`): the decoder
        // ships with the Cua Spaces app export.
        .package(name: "CuaSpacesApp", path: "../../libs/spaces-app-swift"),
    ],
    targets: [
        .target(
            name: "CuaBotsRemote",
            dependencies: [.product(name: "CuaBotsCore", package: "cua-bots-macos"),
                           .product(name: "Cua", package: "Cua"),
                           .product(name: "CuaSpacesFFI", package: "CuaSpacesApp")],
            path: "Sources/CuaBotsRemote"),
        .target(
            name: "CuaBotsPhone",
            dependencies: ["CuaBotsRemote",
                           .product(name: "CuaBotsCore", package: "cua-bots-macos"),
                           .product(name: "CuaBotsUI", package: "cua-bots-macos")],
            path: "Sources/CuaBotsPhone"),
        .executableTarget(
            name: "CuaBotsPhonePreview",
            dependencies: ["CuaBotsPhone", "CuaBotsRemote",
                           .product(name: "CuaBotsCore", package: "cua-bots-macos"),
                           .product(name: "Cua", package: "Cua")],
            path: "Sources/CuaBotsPhonePreview"),
        .testTarget(
            name: "CuaBotsRemoteTests",
            dependencies: ["CuaBotsRemote", "CuaBotsPhone",
                           .product(name: "CuaBotsCore", package: "cua-bots-macos")],
            path: "Tests/CuaBotsRemoteTests"),
    ],
    swiftLanguageModes: [.v5]
)
