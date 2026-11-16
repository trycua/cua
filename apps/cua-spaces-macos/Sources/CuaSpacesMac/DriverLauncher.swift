// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Cua Driver's login item (CUA-3638):
//
//   CuaSpacesMac --driver-helper run   SMAppService.agent, Contents/Library/
//                                      LaunchAgents/<label>.plist (by
//                                      default com.trycua.spaces.driver.plist):
//                                      runs DriverSupervisor, which keeps
//                                      Contents/Helpers/cua-driver serve
//                                      --embedded up as its own child
//
// launchd starts this executable, so macOS holds the app ("Cua Spaces")
// responsible for the driver: its Accessibility and Screen Recording are the
// app's, as with the host service (ServiceLauncher.swift). It runs whether or
// not this Mac is set up, shared or signed in; only the person's "Let agents
// use apps on this Mac" switch (the pause marker) keeps the driver off.
//
// Stop (launchctl kill SIGTERM, logout) stops the driver and exits 0, which
// launchd leaves stopped until the next login or the app starts it again. An
// update in place exits non-zero once the driver is stopped, so launchd
// starts the new build. Never touches AppKit.

import CuaSpacesMacKit
import Darwin
import Foundation

private var driverStopping: sig_atomic_t = 0

enum DriverLauncher {
    static func serve() -> Never {
        guard let driver = EmbeddedDriver.current() else {
            NSLog("cua-spaces driver: this build of Cua Spaces carries no Cua Driver; nothing to run")
            exit(0)
        }
        for sig in [SIGTERM, SIGINT, SIGHUP] {
            signal(sig) { _ in driverStopping = 1 }
        }
        // The files it runs from: an update replaces them (restart onto the
        // new build), a removal deletes them (stop).
        let me = Bundle.main.executablePath.map { URL(fileURLWithPath: $0).resolvingSymlinksInPath().path }
            ?? CommandLine.arguments[0]
        let files = [me, driver.binary]
        let started = files.map(HelperService.identity)
        var misses = 0
        var lastLook = Date()
        var verdict = HelperService.WatchVerdict.keep
        let supervisor = DriverSupervisor(driver: driver)
        supervisor.run {
            if driverStopping != 0 { return true }
            if Date().timeIntervalSince(lastLook) >= 5 {
                lastLook = Date()
                verdict = HelperService.watch(started: started, now: files.map(HelperService.identity),
                                              specPresent: true, misses: &misses)
                switch verdict {
                case .keep: return false
                case .stop: NSLog("cua-spaces driver: the app's files are gone; stopping")
                case .restart: NSLog("cua-spaces driver: the app was updated; restarting onto the new build")
                }
                return true
            }
            return false
        }
        exit(verdict == .restart ? HelperService.restartExitCode : 0)
    }
}
