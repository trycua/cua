// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import CuaSpacesMacKit
import Foundation

// `CuaSpacesMac --check-launch-only`: dyld has loaded every library the app
// links (libcua_sdk.dylib, Sparkle.framework) and their signatures passed
// library validation, or this line would never run; print the version and
// exit before any window, the daemon or the updater. The release scripts
// and CI run it to prove a bundle launches.
if CommandLine.arguments.dropFirst().first == "--check-launch-only" {
    let info = Bundle.main.infoDictionary ?? [:]
    let version = info["CuaVersion"] as? String ?? info["CFBundleShortVersionString"] as? String ?? "?"
    print("Cua Spaces \(version) (\(info["CFBundleVersion"] as? String ?? "?")) loads")
    exit(0)
}

// Teleport, the Cua Volume, persistent agents and the Keyvault for every
// runtime the SDK builds in this process (the Cua Spaces extensions).
cuaSpacesRegister()
CuaSpacesMacApp.main()
