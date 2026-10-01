// swift-tools-version: 6.0
import PackageDescription

// The Swift streaming example (examples/streaming/SCENARIO.md).
//
// Depends on the local cua Swift package by path. Build it with the cua-sdk
// static library in an XCFramework (see README.md and scripts/build-ffi.sh):
//
//   CUA_SWIFT_XCFRAMEWORK=build/CuaSDKFFI.xcframework swift build -c release
let package = Package(
    name: "CuaStreamingExample",
    platforms: [.macOS(.v13)],
    dependencies: [
        .package(path: "deps/cua-swift"),
    ],
    targets: [
        .executableTarget(
            name: "cua-streaming",
            dependencies: [.product(name: "Cua", package: "cua-swift")],
            path: "Sources/CuaStreaming",
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("AVFoundation"),
                .linkedFramework("VideoToolbox"),
                .linkedFramework("CoreMedia"),
                .linkedFramework("CoreVideo"),
            ]
        ),
    ],
    swiftLanguageModes: [.v5]
)
