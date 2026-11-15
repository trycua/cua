// swift-tools-version: 5.9
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import PackageDescription

let package = Package(
    name: "OpenKoalaBotExample",
    platforms: [.macOS(.v14)],
    dependencies: [
        // The Spaces SDK this sample was the design input for. Everything that
        // used to live in `Sources/OpenKoalaBotExample/Spaces/MCPSpacesClient.swift`
        // is now in there, reshaped by `FRICTION.md`; what used to live in
        // `Sources/OpenKoalaBotExample/Streaming/` is `CuaSpacesStreaming`, below.
        .package(path: "../../libs/spaces-sdk-swift"),
        // The generated SDK itself, for the calls the overlay does not wrap
        // (Space creation with a runtime, CPUs and memory).
        .package(name: "Cua", path: "../../libs/cua/swift"),
        // The stream views (`CuaSpacesStreaming`), source-available
        // (FSL-1.1-MIT) like the rest of Cua Spaces.
        .package(name: "CuaSpacesApp", path: "../../libs/spaces-app-swift"),
    ],
    targets: [
        .executableTarget(
            name: "OpenKoalaBotExample",
            dependencies: [
                .product(name: "CuaSpaces", package: "spaces-sdk-swift"),
                .product(name: "CuaSpacesStreaming", package: "CuaSpacesApp"),
                .product(name: "Cua", package: "Cua"),
            ],
            path: "Sources/OpenKoalaBotExample"
        ),
        .testTarget(
            name: "OpenKoalaBotExampleTests",
            dependencies: ["OpenKoalaBotExample",
                           .product(name: "CuaSpaces", package: "spaces-sdk-swift"),
                           .product(name: "Cua", package: "Cua")],
            path: "Tests/OpenKoalaBotExampleTests"
        ),
    ]
)
