// swift-tools-version: 6.1
// PermissionFlow 2.11.4 (MIT, github.com/jaywcjlove/PermissionFlow), vendored
// by Cua Spaces with one addition: `PermissionFlowController.guidance`, the
// panel's line and drag tile chosen by the host app for what the pane shows
// (not in the list, in the list but off, on). Only the two targets the app
// links are kept. See README.md beside this file.

import PackageDescription

let package = Package(
    name: "PermissionFlow",
    defaultLocalization: "en",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "SystemSettingsKit", targets: ["SystemSettingsKit"]),
        .library(name: "PermissionFlow", targets: ["PermissionFlow"]),
    ],
    targets: [
        .target(name: "SystemSettingsKit"),
        .target(
            name: "PermissionFlow",
            dependencies: ["SystemSettingsKit"],
            resources: [.process("Resources")]
        ),
    ]
)
