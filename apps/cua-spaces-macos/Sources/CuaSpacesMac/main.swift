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

// The host service, run by this same executable so every permission is the
// app's ("Cua Spaces"; ServiceLauncher.swift), before anything else:
//  * `--service-helper run`: the service itself (the login item launchd
//    starts);
//  * `check-permissions`: cua-spacesd's permission check, answered for the
//    app;
//  * `--service-helper register|unregister|status`: the login item
//    (Contents/Library/LaunchAgents/com.trycua.spaces.helper.plist) through
//    SMAppService, which only this bundle's own code may register. cua-host's
//    app-helper runner runs it (from the app's SDK and from the `cua` CLI);
//    prints `{"status": ...}` (and `"error"`).
let launchArgs = Array(CommandLine.arguments.dropFirst()).filter { !$0.hasPrefix("-psn_") }
if launchArgs.first == "--service-helper" {
    if launchArgs.dropFirst().first == "run" { ServiceLauncher.serve() }
    exit(HelperService.run(Array(launchArgs.dropFirst())))
}
if launchArgs.first == "check-permissions" {
    ServiceLauncher.passthrough(launchArgs)
}

// `CUA_ENV=staging`: this process and the daemon use the staging home.
TargetEnvironment.pin()

// Teleport, the Cua Volume, persistent agents and the Keyvault for every
// runtime the SDK builds in this process (the Cua Spaces extensions).
cuaSpacesRegister()
// `CUA_SPACES_OFFSCREEN=1` (debug): an accessory app, for captures nobody sees.
OffscreenRun.prepare()
CuaSpacesMacApp.main()
