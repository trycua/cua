// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import CuaSpacesMacKit
import Foundation
import ServiceManagement

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

// `CuaSpacesMac --service-helper register|unregister|status`: the host
// service's login item (Contents/Library/LaunchAgents/
// com.trycua.spaces.helper.plist, "Cua Spaces Helper") through
// SMAppService, which only this bundle's own code may register. cua-host's
// app-helper runner runs it (from the app's SDK and from the `cua` CLI);
// prints `{"status": ...}` (and `"error"`) and exits before anything else.
if CommandLine.arguments.dropFirst().first == "--service-helper" {
    exit(HelperService.run(Array(CommandLine.arguments.dropFirst(2))))
}

// `CUA_ENV=staging`: this process and the daemon use the staging home.
TargetEnvironment.pin()

// Teleport, the Cua Volume, persistent agents and the Keyvault for every
// runtime the SDK builds in this process (the Cua Spaces extensions).
cuaSpacesRegister()
CuaSpacesMacApp.main()
