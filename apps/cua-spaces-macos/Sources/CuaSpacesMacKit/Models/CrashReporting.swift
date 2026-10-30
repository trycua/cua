// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import Foundation
import Sentry

/// Crash reporting for the app (CUA-2343), through sentry-cocoa.
///
/// Off unless the bundle carries a DSN (Info.plist `CuaCrashDSN`, written by
/// `scripts/build-app.sh` from `apps/cua-spaces-desktop/config/spaces-crash-reporting.json`; empty =
/// off) and the person allows it: the "Share usage data" switch is on and the
/// first-run notice was shown, the same rule usage events follow. It is
/// checked at start and again for every event. Turning the switch off while
/// the app runs closes the SDK; turning it on takes effect at the next launch.
///
/// What leaves the machine: the crash or error's type, module and stack
/// frames, the release, the environment, the OS and device model, and the
/// session (crash-free rate). Never messages, usernames, emails, the host
/// name, screenshots, the view hierarchy, breadcrumbs, traces or an IP.
public enum CrashReporting {
    /// What the app tells the Rust core and the daemon (they inherit it).
    public struct Settings: Equatable {
        public var dsn: String
        public var release: String
        public var environment: String
    }

    // MARK: Pure logic (unit-tested)

    /// Reports are allowed only when usage data is shared and the notice was
    /// shown (`Status.enabled && Status.notice_shown`).
    static func isAllowed(enabled: Bool, noticeShown: Bool) -> Bool { enabled && noticeShown }

    /// `local` for a Local build, `beta` for a version with a `-` suffix,
    /// otherwise `stable`.
    static func environment(channel: String?, version: String) -> String {
        if (channel ?? "").trimmingCharacters(in: .whitespaces).lowercased() == "local" { return "local" }
        return version.contains("-") ? "beta" : "stable"
    }

    static func release(version: String) -> String { "cua-spaces-desktop@\(version)" }

    /// The app's version: `CuaVersion` (a release build stamps the full
    /// string, suffix included), else `CFBundleShortVersionString`.
    static func version(info: [String: Any]) -> String {
        for key in ["CuaVersion", "CFBundleShortVersionString"] {
            if let v = (info[key] as? String)?.trimmingCharacters(in: .whitespaces), !v.isEmpty { return v }
        }
        return "0"
    }

    /// The settings for a bundle, or nil when crash reporting is off (no DSN).
    static func settings(info: [String: Any]) -> Settings? {
        let dsn = ((info["CuaCrashDSN"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !dsn.isEmpty else { return nil }
        let version = version(info: info)
        return Settings(dsn: dsn, release: release(version: version),
                        environment: environment(channel: info["CuaChannel"] as? String, version: version))
    }

    /// The variables the Rust core and daemon read (`cua-crash`).
    static func childEnvironment(_ s: Settings) -> [String: String] {
        ["CUA_CRASH_DSN": s.dsn, "CUA_CRASH_RELEASE": s.release,
         "CUA_CRASH_ENVIRONMENT": s.environment, "CUA_CRASH_SHELL": "swift"]
    }

    /// The forced crash for testing the pipeline: `CUA_CRASH_TEST=swift` (or
    /// the launch argument `--crash-test=swift`), honoured only in a Local build.
    static func wantsForcedCrash(env: [String: String], args: [String], environment: String) -> Bool {
        guard environment == "local" else { return false }
        return env["CUA_CRASH_TEST"] == "swift" || args.contains("--crash-test=swift")
    }

    /// The page's `crashReporting.config` answer (`CrashReportingConfig`).
    /// `allowed` is the consent rule; the DSN, release and environment are the
    /// build's. `crashTest` is set only in a Local build.
    static func bridgeConfig(info: [String: Any], allowed: Bool, env: [String: String]) -> [String: Any] {
        let version = version(info: info)
        let environment = environment(channel: info["CuaChannel"] as? String, version: version)
        var out: [String: Any] = [
            "allowed": allowed,
            "dsn": ((info["CuaCrashDSN"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines),
            "release": release(version: version), "environment": environment, "shell": "swift",
        ]
        if environment == "local", env["CUA_CRASH_TEST"] == "ui" { out["crashTest"] = "ui" }
        return out
    }

    private static let emailPattern = try! NSRegularExpression(
        pattern: "[A-Za-z0-9._%+\\-]+@[A-Za-z0-9\\-]+(?:\\.[A-Za-z0-9\\-]+)+")
    private static let homePatterns: [NSRegularExpression] = [
        try! NSRegularExpression(pattern: "/Users/[^/\\s\"']+"),
        try! NSRegularExpression(pattern: "/home/[^/\\s\"']+"),
        try! NSRegularExpression(pattern: "[A-Za-z]:\\\\Users\\\\[^\\\\\\s\"']+"),
    ]

    /// Home folders become `~`, emails `[email]`.
    static func scrub(_ text: String, home: String = NSHomeDirectory()) -> String {
        var out = text
        let trimmedHome = home.hasSuffix("/") ? String(home.dropLast()) : home
        if trimmedHome.count > 1 { out = out.replacingOccurrences(of: trimmedHome, with: "~") }
        for re in homePatterns {
            out = re.stringByReplacingMatches(in: out, range: NSRange(out.startIndex..., in: out), withTemplate: "~")
        }
        return emailPattern.stringByReplacingMatches(
            in: out, range: NSRange(out.startIndex..., in: out), withTemplate: "[email]")
    }

    /// A request URL without its query string and fragment, scrubbed.
    static func scrubURL(_ url: String, home: String = NSHomeDirectory()) -> String {
        var s = url
        if let i = s.firstIndex(where: { $0 == "?" || $0 == "#" }) { s = String(s[..<i]) }
        return scrub(s, home: home)
    }

    /// Scrubs any value that can carry text (strings, and containers of them).
    static func scrubValue(_ value: Any, home: String) -> Any {
        switch value {
        case let s as String: return scrub(s, home: home)
        case let d as [String: Any]: return d.mapValues { scrubValue($0, home: home) }
        case let a as [Any]: return a.map { scrubValue($0, home: home) }
        default: return value
        }
    }

    private static func scrubFrames(_ stacktrace: SentryStacktrace?, home: String) {
        for frame in stacktrace?.frames ?? [] {
            if let f = frame.fileName { frame.fileName = scrub(f, home: home) }
            if let p = frame.package { frame.package = scrub(p, home: home) }
            if let m = frame.module { frame.module = scrub(m, home: home) }
        }
    }

    /// The contract's privacy rules for one event; nil drops it (not allowed).
    static func sanitize(_ event: Event, allowed: Bool, home: String = NSHomeDirectory()) -> Event? {
        guard allowed else { return nil }
        // Messages can carry Space names, file names and emails: type, module
        // and frames stay, values go.
        for exception in event.exceptions ?? [] {
            exception.value = ""
            scrubFrames(exception.stacktrace, home: home)
        }
        for thread in event.threads ?? [] { scrubFrames(thread.stacktrace, home: home) }
        event.message = nil
        event.user = nil
        event.serverName = nil
        event.breadcrumbs = nil
        event.tags = event.tags?.mapValues { scrub($0, home: home) }
        event.extra = event.extra?.mapValues { scrubValue($0, home: home) }
        if let request = event.request {
            if let url = request.url { request.url = scrubURL(url, home: home) }
            request.queryString = nil
            request.fragment = nil
            request.cookies = nil
            request.headers = nil
            request.bodySize = 0
        }
        if var context = event.context {
            // The device name is the Mac's host name.
            if var device = context["device"] {
                device.removeValue(forKey: "name")
                context["device"] = device
            }
            event.context = context.mapValues { $0.mapValues { scrubValue($0, home: home) } }
        }
        return event
    }

    // MARK: Start and stop

    /// The switch as the app core stores it (`telemetryStatus()`).
    private static func currentlyAllowed() -> Bool {
        let s = telemetryStatus()
        return isAllowed(enabled: s.enabled, noticeShown: s.noticeShown)
    }

    private nonisolated(unsafe) static var started = false

    /// Call early in `main`, after `TargetEnvironment.pinChannel()` (the
    /// switch lives in the channel's home) and before anything starts the
    /// Rust core or the daemon. A no-op without a DSN or without consent.
    public static func start(bundle: Bundle = .main,
                             environment: [String: String] = ProcessInfo.processInfo.environment,
                             arguments: [String] = CommandLine.arguments) {
        guard environment["CUA_SPACES_FIXTURES"] != "1",
              let settings = settings(info: bundle.infoDictionary ?? [:]),
              currentlyAllowed() else { return }
        for (key, value) in childEnvironment(settings) { setenv(key, value, 1) }
        SentrySDK.start { options in
            options.dsn = settings.dsn
            options.releaseName = settings.release
            options.environment = settings.environment
            options.sendDefaultPii = false
            options.enableCrashHandler = true
            options.enableAutoSessionTracking = true
            options.attachStacktrace = true
            options.enableAutoBreadcrumbTracking = false
            options.enableNetworkBreadcrumbs = false
            options.enableNetworkTracking = false
            options.enableCaptureFailedRequests = false
            options.enableAutoPerformanceTracing = false
            options.enableSwizzling = false
            options.enableAppHangTracking = false
            options.enableMetricKit = false
            options.maxBreadcrumbs = 0
            options.tracesSampleRate = 0
            options.beforeSend = { event in sanitize(event, allowed: currentlyAllowed()) }
        }
        SentrySDK.configureScope { scope in
            scope.setTag(value: "swift", key: "shell")
            scope.setTag(value: "shell", key: "component")
            scope.setUser(nil)
        }
        started = true
        if wantsForcedCrash(env: environment, args: arguments, environment: settings.environment) {
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { forcedCrash() }
        }
    }

    /// The host service (`CuaSpacesMac --service-helper run`): only the
    /// variables its cua-spacesd child reports with. The Swift SDK is not
    /// started there: starting it checks the windowless helper in with
    /// LaunchServices as "Cua Spaces" itself, and opening the app then fails
    /// (error -600) while the helper runs. Returns whether the variables were
    /// set (a DSN and consent).
    @discardableResult
    public static func prepareService(bundle: Bundle = .main,
                                      environment: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
        guard environment["CUA_SPACES_FIXTURES"] != "1",
              let settings = settings(info: bundle.infoDictionary ?? [:]),
              currentlyAllowed() else { return false }
        for (key, value) in childEnvironment(settings) { setenv(key, value, 1) }
        return true
    }

    /// Whether the Swift SDK was started in this process.
    public static var isStarted: Bool { started }

    /// The Settings switch changed: closes the SDK when reports are no
    /// longer allowed (it is not restarted until the next launch).
    public static func switchChanged(enabled: Bool, noticeShown: Bool) {
        guard started, !isAllowed(enabled: enabled, noticeShown: noticeShown) else { return }
        SentrySDK.close()
        started = false
    }

    @inline(never)
    private static func forcedCrash() {
        fatalError("CUA_CRASH_TEST=swift")
    }
}
