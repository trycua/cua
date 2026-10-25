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

// A Local build ("Cua Spaces Local", Info.plist CuaChannel=local) settles
// CUA_CHANNEL, CUA_ENV and CUA_HOME first, before anything else starts, so the
// helper below, cua-spacesd and every other child inherit them. A Stable
// build changes nothing here.
TargetEnvironment.pinChannel()

// The host service, run by this same executable so every permission is the
// app's ("Cua Spaces"; ServiceLauncher.swift), before anything else:
//  * `--service-helper run`: the service itself (the login item launchd
//    starts);
//  * `check-permissions`: cua-spacesd's permission check, answered for the
//    app;
//  * `--service-helper register|unregister|status`: the login item
//    (Contents/Library/LaunchAgents/<CuaHelperLabel>.plist, by default
//    com.trycua.spaces.helper.plist) through
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
// A new process's Screen Recording and Accessibility answers, for
// PermissionGuide to close its panel once access is granted.
if launchArgs.first == "preflight-permissions" {
    print(PermissionGuide.preflightReport())
    exit(0)
}

// `CUA_ENV=staging`: this process and the daemon use the staging home (a
// Local build already settled its three variables above).
TargetEnvironment.pin()

// Teleport, the Cua Volume, persistent agents and the Keyvault for every
// runtime the SDK builds in this process (the Cua Spaces extensions).
cuaSpacesRegister()
// `CUA_SPACES_OFFSCREEN=1` (debug): an accessory app, for captures nobody sees.
OffscreenRun.prepare()
// The app runs with its window here: it asks for Screen Recording and
// Accessibility itself (its first run and Settings, PermissionGuide), so the
// host service no longer shows macOS's own prompts (ServiceLauncher).
if let spec = HelperService.specPath(info: Bundle.main.infoDictionary, home: NSHomeDirectory()) {
    PermissionGuide.noteAppAsks(specPath: spec)
}
CuaSpacesMacApp.main()
