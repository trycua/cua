// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "CuaDriver",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .executable(name: "cua-driver", targets: ["CuaDriverCLI"]),
        .library(name: "CuaDriverCore", targets: ["CuaDriverCore"]),
        .library(name: "CuaDriverServer", targets: ["CuaDriverServer"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.5.0"),
        .package(url: "https://github.com/modelcontextprotocol/swift-sdk.git", from: "0.9.0"),
    ],
    targets: [
        .target(
            name: "CuaDriverCore"
        ),
        .target(
            name: "CuaDriverServer",
            dependencies: [
                "CuaDriverCore",
                .product(name: "MCP", package: "swift-sdk"),
            ]
        ),
        .executableTarget(
            name: "CuaDriverCLI",
            dependencies: [
                "CuaDriverCore",
                "CuaDriverServer",
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
                .product(name: "MCP", package: "swift-sdk"),
            ]
        ),
        .testTarget(
            name: "ZoomMathTests",
            dependencies: ["CuaDriverCore"]
        ),
    ]
)
