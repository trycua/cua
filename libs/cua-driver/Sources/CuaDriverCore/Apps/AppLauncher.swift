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

    /// Launch a macOS app in the background — no focus steal, no window
    /// visible on screen.
    ///
    /// Uses `NSWorkspace.openApplication(at:configuration:)` with both
    /// `configuration.activates = false` and `configuration.hides = true`.
    /// The `hides = true` flag is the key to a fully-populated AX tree on
    /// a pure background launch: LaunchServices lets the target finish
    /// `applicationDidFinishLaunching` and create its window, then hides
    /// the app back immediately. The target's AX tree is fully populated
    /// (Calculator on macOS 14+ goes from a 13-element menu-bar-only tree
    /// to a 41-element tree with the keypad) and there's no visible
    /// focus flash because the window never draws on screen.
    ///
    /// To bring a hidden-launched app onto the screen, the caller (or
    /// the user) must explicitly unhide it — either via
    /// `NSRunningApplication.unhide()` from their own code or through
    /// regular macOS UI (Dock click, Cmd-Tab, etc.).
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

        // We intentionally do NOT call `NSRunningApplication.unhide()` here.
        //
        // `unhide()` bumps the target's CPS activation count away from 0,
        // which lets a target like Chrome's subsequent self-activation
        // succeed and raise its window on top of the user's work —
        // exactly the focus-steal we're trying to avoid. Leaving the
        // activation count at 0 keeps self-activation attempts as the
        // softer "presents 0 windows, is frontable" rejection, which
        // resolves on its own once the target creates a window.

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
            throw LaunchError.notFound("name '\(name)'")
        }
        throw LaunchError.nothingSpecified
    }

}
