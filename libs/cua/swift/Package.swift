// swift-tools-version: 5.9
import Foundation
import PackageDescription

// The Swift face of the cua SDK (`libs/cua/crates/cua-sdk`).
//
// Targets:
//  * `cua_sdkFFI`: the C header + modulemap UniFFI generates, backed by the
//    Rust library. Three ways to get the library, picked in this order:
//      1. `CUA_SWIFT_XCFRAMEWORK=build/CuaSDKFFI.xcframework` (a local
//         XCFramework from scripts/build-xcframework.sh, relative to this
//         package);
//      2. the release `.binaryTarget` (`releaseURL`/`releaseChecksum`,
//         written by the release workflow);
//      3. development: the system-library target plus the host dylib staged
//         in `lib/` by `node ../scripts/stage-uniffi-library.mjs --only=swift`.
//  * `CuaSDK`: the generated Swift binding (never edited by hand).
//  * `Cua`: the public module; re-exports `CuaSDK` and adds conveniences.
let releaseURL = ""
let releaseChecksum = ""

let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let env = ProcessInfo.processInfo.environment
let localXCFramework = env["CUA_SWIFT_XCFRAMEWORK"].flatMap { $0.isEmpty ? nil : $0 }
let binary = localXCFramework != nil || !releaseURL.isEmpty

var ffiTarget: Target
if let path = localXCFramework {
    ffiTarget = .binaryTarget(name: "cua_sdkFFI", path: path)
} else if !releaseURL.isEmpty {
    ffiTarget = .binaryTarget(name: "cua_sdkFFI", url: releaseURL, checksum: releaseChecksum)
} else {
    ffiTarget = .systemLibrary(name: "cua_sdkFFI", path: "Sources/cua_sdkFFI")
}

// The static library in the XCFramework needs every system framework and
// library the cdylib links (compare `otool -L libcua_sdk.dylib`): the Rust
// TLS/network stack, the VideoToolbox media codec, and on macOS the in-process
// driver (Accessibility, CoreGraphics, AppKit). The dev dylib carries its own
// load commands.
let sdkLinker: [LinkerSetting] = binary
    ? [
        .linkedFramework("Security"),
        .linkedFramework("SystemConfiguration"),
        .linkedFramework("CoreFoundation"),
        .linkedFramework("Foundation"),
        .linkedFramework("CoreGraphics"),
        .linkedFramework("CoreMedia"),
        .linkedFramework("CoreVideo"),
        .linkedFramework("VideoToolbox"),
        .linkedFramework("LocalAuthentication"),
        .linkedFramework("AppKit", .when(platforms: [.macOS])),
        .linkedFramework("ApplicationServices", .when(platforms: [.macOS])),
        .linkedLibrary("resolv"),
        .linkedLibrary("iconv"),
        .linkedLibrary("objc"),
        .linkedLibrary("c++"),
    ]
    : [
        .unsafeFlags([
            "-L", "\(packageDir)/lib",
            "-lcua_sdk",
            "-Xlinker", "-rpath", "-Xlinker", "\(packageDir)/lib",
        ]),
    ]

let package = Package(
    name: "Cua",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        .library(name: "Cua", targets: ["Cua"]),
    ],
    targets: [
        ffiTarget,
        .target(
            name: "CuaSDK",
            dependencies: ["cua_sdkFFI"],
            path: "Sources/CuaSDK",
            linkerSettings: sdkLinker
        ),
        .target(
            name: "Cua",
            dependencies: ["CuaSDK"],
            path: "Sources/Cua"
        ),
        .testTarget(
            name: "CuaTests",
            dependencies: ["Cua"],
            path: "Tests/CuaTests"
        ),
    ]
)
