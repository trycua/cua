// swift-tools-version: 6.0
//
// cua-vz: the virtual machine monitor of a macOS microVM Space. One process
// per VM (like `firecracker` on Linux), started by cua's microVM runtime
// (libs/cua/crates/cua-vmm/src/microvm). It boots a Linux kernel and the
// guest init with Apple's Virtualization.framework, saves and restores the
// VM's memory and devices (checkpoints, forks, warm starts), and serves the
// guest init's vsock port and the Space's published ports to the host.
//
// No dependencies beyond the SDK: the binary is signed with
// cua-vz.entitlements (com.apple.security.virtualization) by build.sh.

import PackageDescription

let package = Package(
    name: "cua-vz",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "cua-vz", targets: ["cua-vz"])
    ],
    targets: [
        .target(name: "CuaVZCore"),
        .executableTarget(name: "cua-vz", dependencies: ["CuaVZCore"]),
        .testTarget(name: "CuaVZCoreTests", dependencies: ["CuaVZCore"]),
    ],
    // Plain threads and dispatch queues around Virtualization.framework's
    // own queue: Swift 5 language mode, no strict-concurrency checking.
    swiftLanguageModes: [.v5]
)
