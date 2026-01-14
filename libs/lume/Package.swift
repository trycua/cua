// swift-tools-version: 6.0
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "lume",
    platforms: [
        .macOS(.v14)
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser", from: "1.3.1"),
        .package(url: "https://github.com/apple/swift-format.git", branch: ("release/5.10")),
        .package(url: "https://github.com/apple/swift-atomics.git", .upToNextMajor(from: "1.2.0")),
        .package(url: "https://github.com/mhdhejazi/Dynamic", branch: "master"),
        .package(url: "https://github.com/jpsim/Yams.git", from: "5.0.0")
    ],
    targets: [
        // Targets are the basic building blocks of a package, defining a module or a test suite.
        // Targets can depend on other targets in this package and products from dependencies.
        .executableTarget(
            name: "lume",
            dependencies: [
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
                .product(name: "Atomics", package: "swift-atomics"),
                .product(name: "Dynamic", package: "Dynamic"),
                .product(name: "Yams", package: "Yams")
            ],
            path: "src",
            resources: [
                .copy("Resources/unattended-presets")
            ]),
        .testTarget(
            name: "lumeTests",
            dependencies: [
                "lume"
            ],
            path: "tests")
    ]
)
