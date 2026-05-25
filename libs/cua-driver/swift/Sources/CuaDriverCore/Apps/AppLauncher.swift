import AppKit
import CoreGraphics
import Foundation

public enum AppLauncher {
    public enum LaunchError: Error, CustomStringConvertible, Sendable {
        case notFound(String)
        case launchFailed(String)
        case nothingSpecified

        public var description: String {
            switch self {
            case .notFound(let what):
                return "Could not locate app (\(what))."
            case .launchFailed(let msg):
                return "Launch failed: \(msg)"
            case .nothingSpecified:
                return "Provide either bundle_id or name."
            }
        }
    }

    /// Launch a macOS app in the background — window visible on screen,
    /// no focus steal.
    ///
    /// Uses `NSWorkspace.open(_:configuration:)` with
    /// `configuration.activates = false` so the target never becomes the
    /// frontmost app. After the launch callback, `NSRunningApplication.unhide()`
    /// is called so the target's windows appear on screen behind the current
    /// foreground app. The AX tree is fully populated and automation can
    /// start immediately via `click`, `hotkey`, `get_window_state`, etc.
    ///
    /// When `urls` is non-empty, dispatches through
    /// `NSWorkspace.open(_:withApplicationAt:configuration:)` instead so
    /// the app receives them through its `application(_:open:)` AppKit
    /// delegate. For Finder that's how you open a window rooted at a
    /// specific folder without activating the app. The same
    /// `activates = false` + `hides = true` background contract applies.
    ///
    /// Prefers `bundle_id` lookup via LaunchServices; falls back to
    /// scanning system paths (`/Applications`,
    /// `/System/Applications[/Utilities]`) and the current user's
    /// `~/Applications` (including the Chrome PWA `Chrome Apps.localized/`
    /// subfolder) when only a name is provided.
    public static func launch(
        bundleId: String? = nil,
        name: String? = nil,
        urls: [URL] = [],
        additionalArguments: [String] = [],
        additionalEnvironment: [String: String] = [:],
        createsNewApplicationInstance: Bool = false
    ) async throws -> AppInfo {
        let appURL = try locate(bundleId: bundleId, name: name)

        let config = NSWorkspace.OpenConfiguration()
        if !additionalArguments.isEmpty {
            config.arguments = additionalArguments
        }
        if !additionalEnvironment.isEmpty {
            var env = ProcessInfo.processInfo.environment
            env.merge(additionalEnvironment) { _, new in new }
            config.environment = env
        }
        config.createsNewApplicationInstance = createsNewApplicationInstance
        config.activates = false
        config.addsToRecentItems = false
        // NOTE: we used to set `config.hides = true` on the theory that
        // it forced the launch lifecycle to complete. Empirically, when
        // LaunchServices cold-launches Calculator without `hides`,
        // Calculator's window is created, the app's internal
        // SetFrontProcess attempt is denied by CPS ("0 windows, is
        // frontable"), and the window ends up visible-but-backgrounded
        // rather than invisible. Leaving hides unset matches that
        // behavior. The oapp AppleEvent descriptor attached below is
        // still what drives proper window creation on relaunch.
        // Attach an "Open Application" AppleEvent descriptor addressed at
        // the target's bundle id. Without it, LaunchServices treats a
        // hidden launch ambiguously and can silently skip window creation
        // for state-restored apps — the exact Calculator-windowless-on-
        // relaunch symptom we saw. With the AppleEvent attached, the OS
        // dispatches a proper `oapp` event to the target as part of the
        // launch, which reliably triggers `application(_:open:)` /
        // NSApplicationDelegate window creation regardless of
        // state-restoration weirdness.
        if let resolvedBundleId = Bundle(url: appURL)?.bundleIdentifier
            ?? bundleId
        {
            // Proper OpenApplication event (`aevt/oapp`) addressed to the
            // target's bundle id. The bare target descriptor alone isn't
            // enough; `setAppleEvent:` expects an actual event, and the
            // system uses it as the AppleEvent delivered to the target's
            // event handler during launch.
            let target = NSAppleEventDescriptor(
                bundleIdentifier: resolvedBundleId)
            let openEvent = NSAppleEventDescriptor(
                eventClass: AEEventClass(kCoreEventClass),
                eventID: AEEventID(kAEOpenApplication),
                targetDescriptor: target,
                returnID: AEReturnID(kAutoGenerateReturnID),
                transactionID: AETransactionID(kAnyTransactionID)
            )
            config.appleEvent = openEvent
        }

        let info: AppInfo
        if urls.isEmpty {
            // Use `open(_:configuration:)` (the URL-handling path) rather
            // than `openApplication(at:)`. The `appleEvent` descriptor
            // attached to OpenConfiguration only appears to be honored on
            // this code path. The .app-bundle URL resolves to the same app
            // launch either way, but the AppleEvent delivery differs.
            info = try await withCheckedThrowingContinuation { cont in
                NSWorkspace.shared.open(
                    appURL,
                    configuration: config
                ) { app, error in
                    Self.resumeWithResult(
                        cont: cont, app: app, error: error
                    )
                }
            }
        } else {
            info = try await withCheckedThrowingContinuation { cont in
                NSWorkspace.shared.open(
                    urls,
                    withApplicationAt: appURL,
                    configuration: config
                ) { app, error in
                    Self.resumeWithResult(cont: cont, app: app, error: error)
                }
            }
        }

        // Unhide the app so its windows are visible on screen in the
        // background. `activates = false` already prevents focus steal;
        // `unhide()` only makes existing windows drawable — it does not
        // activate the app or raise it above the current foreground.
        //
        // If the app self-activates after unhide (Chrome occasionally does
        // during its launch lifecycle), the focus-steal preventer in
        // LaunchAppTool re-activates the previous frontmost app. Calling
        // unhide() here is safe because by the time we reach this point
        // the launch-lifecycle self-activation window has closed.
        if let runningApp = NSRunningApplication(
            processIdentifier: info.pid)
        {
            runningApp.unhide()
        }

        // For Finder and Safari: if no on-screen windows appeared after
        // unhide (e.g. already running with windows on a different Space),
        // open the home directory via the URL-handoff path. This calls
        // `application(_:open:)` which creates a new window on the current
        // Space without activation — Finder opens ~/, Safari opens it as a
        // local file URL. Scoped to these two apps only to avoid unintended
        // side effects on other apps (document pickers, unexpected behavior).
        if urls.isEmpty {
            let resolvedBundleId =
                Bundle(url: appURL)?.bundleIdentifier ?? bundleId ?? ""
            let wantsFallback =
                resolvedBundleId == "com.apple.finder"
                || resolvedBundleId == "com.apple.Safari"
            if wantsFallback {
                let onScreen = WindowEnumerator.visibleWindows().filter { $0.pid == info.pid }
                if onScreen.isEmpty {
                    // Finder gets its home directory; Safari gets a blank page.
                    let fallbackURL: URL =
                        resolvedBundleId == "com.apple.Safari"
                        ? URL(string: "about:blank")!
                        : FileManager.default.homeDirectoryForCurrentUser
                    let fallbackConfig = NSWorkspace.OpenConfiguration()
                    fallbackConfig.activates = false
                    fallbackConfig.addsToRecentItems = false
                    _ = try? await withCheckedThrowingContinuation {
                        (cont: CheckedContinuation<AppInfo, Error>) in
                        NSWorkspace.shared.open(
                            [fallbackURL],
                            withApplicationAt: appURL,
                            configuration: fallbackConfig
                        ) { app, error in
                            Self.resumeWithResult(cont: cont, app: app, error: error)
                        }
                    }
                }
            }
        }

        return info
    }

    /// Shared completion-handler bridge for both code paths above
    /// (pure-launch vs. launch-with-urls). Swift's NSWorkspace completion
    /// handlers aren't `Sendable` in Swift 6 strict-concurrency mode;
    /// factoring the bridge out keeps the two `withCheckedThrowingContinuation`
    /// sites short enough that the compiler is happy without a bespoke
    /// `@preconcurrency` dance.
    private static func resumeWithResult(
        cont: CheckedContinuation<AppInfo, Error>,
        app: NSRunningApplication?,
        error: Error?
    ) {
        if let error {
            cont.resume(
                throwing: LaunchError.launchFailed(error.localizedDescription))
        } else if let app {
            let pid = app.processIdentifier
            cont.resume(
                returning: AppInfo(
                    pid: pid,
                    bundleId: app.bundleIdentifier,
                    name: app.localizedName ?? "",
                    running: true,
                    active: app.isActive
                ))
        } else {
            cont.resume(
                throwing: LaunchError.launchFailed("no app returned by LaunchServices"))
        }
    }

    private static func locate(bundleId: String?, name: String?) throws -> URL {
        if let bundleId, !bundleId.isEmpty {
            if let url = NSWorkspace.shared.urlForApplication(
                withBundleIdentifier: bundleId)
            {
                return url
            }
            throw LaunchError.notFound("bundle_id '\(bundleId)'")
        }
        if let name, !name.isEmpty {
            // Pass 1 — filesystem lookup by bundle filename (fastest; locale-independent
            // for English app names whose on-disk bundle name matches the display name).
            let appName = name.hasSuffix(".app") ? name : "\(name).app"
            // System roots first — they're canonical. User-local paths come
            // after so an app present in /Applications wins over a same-name
            // copy in ~/Applications. We do not recurse: only these specific
            // directories are searched. Chrome PWAs live under
            // ~/Applications/Chrome Apps.localized/ so we include it
            // explicitly rather than walking arbitrary subfolders.
            let home = FileManager.default.homeDirectoryForCurrentUser.path
            let roots = [
                "/Applications",
                "/System/Applications",
                "/System/Applications/Utilities",
                "/Applications/Utilities",
                "\(home)/Applications",
                "\(home)/Applications/Chrome Apps.localized",
            ]
            for root in roots {
                let path = "\(root)/\(appName)"
                if FileManager.default.fileExists(atPath: path) {
                    return URL(fileURLWithPath: path)
                }
            }

            // Pass 2 — LaunchServices bundle-ID lookup, in case the caller
            // passed a bundle identifier string as `name` rather than using
            // the `bundle_id` parameter (e.g. "com.apple.calculator").
            if let url = NSWorkspace.shared.urlForApplication(
                withBundleIdentifier: name)
            {
                return url
            }

            // Pass 3 — scan all candidate directories and match against each
            // bundle's metadata, in priority order:
            //   a) localizedName from NSRunningApplication (locale-aware; works
            //      on non-English systems, e.g. "計算機" on JP macOS)
            //   b) CFBundleDisplayName / CFBundleName (English; from Info.plist)
            //   c) bundle URL stem (filename minus .app)
            //
            // Matching is case-insensitive throughout so "calculator" and
            // "Calculator" both resolve.
            let needle = name.lowercased()

            // Check running apps first — NSRunningApplication.localizedName
            // gives the OS-locale display name without touching the disk.
            for app in NSWorkspace.shared.runningApplications {
                guard let url = app.bundleURL else { continue }
                if (app.localizedName?.lowercased() == needle) {
                    return url
                }
            }

            // Fall back to scanning installed bundles in the same roots.
            let fm = FileManager.default
            for root in roots {
                guard let children = try? fm.contentsOfDirectory(atPath: root)
                else { continue }
                for child in children where child.hasSuffix(".app") {
                    let path = "\(root)/\(child)"
                    guard let bundle = Bundle(path: path) else { continue }
                    // CFBundleDisplayName > CFBundleName > stem
                    let displayName =
                        (bundle.infoDictionary?["CFBundleDisplayName"] as? String)
                        ?? (bundle.infoDictionary?["CFBundleName"] as? String)
                        ?? URL(fileURLWithPath: path)
                            .deletingPathExtension().lastPathComponent
                    if displayName.lowercased() == needle {
                        return URL(fileURLWithPath: path)
                    }
                    // Also match against the raw stem ("Calculator" → "Calculator.app")
                    let stem = URL(fileURLWithPath: path)
                        .deletingPathExtension().lastPathComponent
                    if stem.lowercased() == needle {
                        return URL(fileURLWithPath: path)
                    }
                }
            }

            throw LaunchError.notFound("name '\(name)'")
        }
        throw LaunchError.nothingSpecified
    }

}
