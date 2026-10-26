// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Sentry
import Testing

/// Crash reporting: consent, release and environment, and the privacy rules
/// every event passes before it leaves the machine.
@Suite("Crash reporting")
struct CrashReportingTests {
    @Test func consentNeedsTheSwitchAndTheNotice() {
        #expect(CrashReporting.isAllowed(enabled: true, noticeShown: true))
        #expect(!CrashReporting.isAllowed(enabled: true, noticeShown: false))
        #expect(!CrashReporting.isAllowed(enabled: false, noticeShown: true))
        #expect(!CrashReporting.isAllowed(enabled: false, noticeShown: false))
    }

    @Test func environmentFollowsChannelAndVersion() {
        #expect(CrashReporting.environment(channel: "local", version: "0.7.2") == "local")
        #expect(CrashReporting.environment(channel: "Local", version: "0.7.2-beta.1") == "local")
        #expect(CrashReporting.environment(channel: nil, version: "0.7.2-beta.1") == "beta")
        #expect(CrashReporting.environment(channel: "stable", version: "0.7.2") == "stable")
        #expect(CrashReporting.release(version: "0.7.2") == "cua-spaces-desktop@0.7.2")
    }

    @Test func noDsnMeansOff() {
        #expect(CrashReporting.settings(info: [:]) == nil)
        #expect(CrashReporting.settings(info: ["CuaCrashDSN": "  "]) == nil)
    }

    @Test func settingsReadTheBundle() throws {
        let s = try #require(CrashReporting.settings(info: [
            "CuaCrashDSN": "https://k@o1.ingest.sentry.io/2", "CuaVersion": "0.7.2-beta.1",
            "CFBundleShortVersionString": "0.7.2",
        ]))
        #expect(s.release == "cua-spaces-desktop@0.7.2-beta.1")
        #expect(s.environment == "beta")
        let env = CrashReporting.childEnvironment(s)
        #expect(env["CUA_CRASH_SHELL"] == "swift")
        #expect(env["CUA_CRASH_DSN"] == "https://k@o1.ingest.sentry.io/2")
        #expect(env["CUA_CRASH_RELEASE"] == s.release)
        #expect(env["CUA_CRASH_ENVIRONMENT"] == "beta")
    }

    @Test func forcedCrashOnlyInLocalBuilds() {
        #expect(CrashReporting.wantsForcedCrash(env: ["CUA_CRASH_TEST": "swift"], args: [], environment: "local"))
        #expect(CrashReporting.wantsForcedCrash(env: [:], args: ["--crash-test=swift"], environment: "local"))
        #expect(!CrashReporting.wantsForcedCrash(env: ["CUA_CRASH_TEST": "swift"], args: [], environment: "stable"))
        #expect(!CrashReporting.wantsForcedCrash(env: [:], args: [], environment: "local"))
    }

    @Test func scrubsHomesAndEmails() {
        let home = "/Volumes/Data/ada"
        #expect(CrashReporting.scrub("/Users/ada/Library/x.swift", home: home) == "~/Library/x.swift")
        #expect(CrashReporting.scrub("/home/bob/a", home: home) == "~/a")
        #expect(CrashReporting.scrub("C:\\Users\\Cy\\a", home: home) == "~\\a")
        #expect(CrashReporting.scrub("/Volumes/Data/ada/f", home: home) == "~/f")
        #expect(CrashReporting.scrub("mail ada@example.com now", home: home) == "mail [email] now")
        #expect(CrashReporting.scrubURL("https://x.test/a?token=1#frag", home: home) == "https://x.test/a")
    }

    private func event() -> Event {
        let e = Event(level: .fatal)
        let frame = Frame()
        frame.fileName = "/Users/ada/src/App.swift"
        frame.function = "crash()"
        let trace = SentryStacktrace(frames: [frame], registers: [:])
        let ex = Exception(value: "Space 'ada@example.com' in /Users/ada/x failed", type: "NSInvalidArgumentException")
        ex.stacktrace = trace
        e.exceptions = [ex]
        e.message = SentryMessage(formatted: "boom ada@example.com")
        e.user = User(userId: "42")
        e.user?.ipAddress = "1.2.3.4"
        e.serverName = "adas-macbook.local"
        e.tags = ["path": "/Users/ada/y", "shell": "swift"]
        e.extra = ["note": "ping ada@example.com", "n": 3]
        e.breadcrumbs = [Breadcrumb(level: .info, category: "ui.click")]
        let r = SentryRequest()
        r.url = "https://x.test/p?email=ada@example.com#f"
        r.queryString = "email=ada@example.com"
        r.fragment = "f"
        e.request = r
        e.context = ["device": ["name": "Ada's Mac", "model": "Mac15,3"]]
        return e
    }

    @Test func dropsEventsWithoutConsent() {
        #expect(CrashReporting.sanitize(event(), allowed: false, home: "/Users/ada") == nil)
    }

    @Test func clearsValuesAndPersonalData() throws {
        let e = try #require(CrashReporting.sanitize(event(), allowed: true, home: "/Users/ada"))
        let ex = try #require(e.exceptions?.first)
        #expect(ex.value == "")
        #expect(ex.type == "NSInvalidArgumentException")
        #expect(ex.stacktrace?.frames.first?.fileName == "~/src/App.swift")
        #expect(ex.stacktrace?.frames.first?.function == "crash()")
        #expect(e.message == nil)
        #expect(e.user == nil)
        #expect(e.serverName == nil)
        #expect(e.breadcrumbs == nil)
        #expect(e.tags?["path"] == "~/y")
        #expect(e.tags?["shell"] == "swift")
        #expect(e.extra?["note"] as? String == "ping [email]")
        #expect(e.extra?["n"] as? Int == 3)
        #expect(e.request?.url == "https://x.test/p")
        #expect(e.request?.queryString == nil)
        #expect(e.request?.fragment == nil)
        let device = e.context?["device"]
        #expect(device?["name"] == nil)
        #expect(device?["model"] as? String == "Mac15,3")
    }
}

@Suite("Crash reporting bridge")
struct CrashReportingBridgeTests {
    @Test func configCarriesConsentAndBuild() {
        let info: [String: Any] = ["CuaCrashDSN": "https://k@o.test/1", "CuaVersion": "0.7.2"]
        let c = CrashReporting.bridgeConfig(info: info, allowed: true, env: ["CUA_CRASH_TEST": "ui"])
        #expect(c["allowed"] as? Bool == true)
        #expect(c["shell"] as? String == "swift")
        #expect(c["environment"] as? String == "stable")
        #expect(c["release"] as? String == "cua-spaces-desktop@0.7.2")
        #expect(c["crashTest"] == nil)
        let l = CrashReporting.bridgeConfig(info: info.merging(["CuaChannel": "local"]) { $1 }, allowed: false,
                                            env: ["CUA_CRASH_TEST": "ui"])
        #expect(l["crashTest"] as? String == "ui")
        #expect(l["allowed"] as? Bool == false)
    }
}
