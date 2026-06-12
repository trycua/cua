import AppKit
import CoreGraphics
import Foundation

public enum AppEnumerator {
    /// Unified list of apps the agent can interact with or launch.
    ///
    /// Includes:
    /// - **Running apps** with `NSApplicationActivationPolicyRegular` (dock
    ///   apps; background / helper agents are filtered out) — `running =
    ///   true`, `active` reflects system-wide frontmost status.
    /// - **Installed-but-not-running apps** discovered by scanning the
    ///   standard app directories (`/Applications`, `/Applications/Utilities`,
    ///   `~/Applications`, `/System/Applications`) — `running = false`,
    ///   `active = false`, `pid = 0`.
    ///
    /// Use this list for both "what's currently running" and "what can I
    /// `launch_app` by name / bundle id" questions. The union is
    /// deduplicated by bundle id. For per-window visibility / Space
    /// residency, call `list_windows` — window-level state isn't
    /// mirrored on these records on purpose.
    public static func apps() -> [AppInfo] {
        var byBundleId: [String: AppInfo] = [:]
        var entries: [AppInfo] = []

        func record(_ info: AppInfo) {
            if let bid = info.bundleId, !bid.isEmpty {
                if byBundleId[bid] != nil { return }  // already listed
                byBundleId[bid] = info
            }
            entries.append(info)
        }

        // Primary: derive running pids from the current window list —
        // CGWindowListCopyWindowInfo is a fresh system query on every call.
        // NSWorkspace.shared.runningApplications alone is unreliable in
        // long-lived non-UI processes because its cache only updates when
        // the main run loop runs, so apps launched after our server
        // started wouldn't appear.
        var seenPids = Set<Int32>()
        if let windows =
            CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID)
            as? [[String: Any]]
        {
            for window in windows {
                guard let rawPid = window[kCGWindowOwnerPID as String] as? Int
                else { continue }
                guard let pid = Int32(exactly: rawPid) else { continue }
                guard !seenPids.contains(pid) else { continue }
                guard
                    let app = NSRunningApplication(processIdentifier: pid),
                    app.activationPolicy == .regular
                else { continue }
                seenPids.insert(pid)
                record(makeRunningInfo(app))
            }
        }

        // Secondary: regular-policy apps NSWorkspace knows about that
        // haven't produced a window yet (rare, but possible during launch).
        for app in NSWorkspace.shared.runningApplications
        where app.activationPolicy == .regular {
            let pid = app.processIdentifier
            guard !seenPids.contains(pid) else { continue }
            seenPids.insert(pid)
            record(makeRunningInfo(app))
        }

        // Tertiary: installed-but-not-running apps. Scan the standard app
        // directories and filter out anything whose bundle id is already
        // listed above. Keeps the list practical for an agent deciding
        // "can I launch X by name / bundle id" without relying on
        // brittle "is it running right now" heuristics.
        for installed in installedApps() {
            if let bid = installed.bundleId, byBundleId[bid] != nil {
                continue
            }
            record(installed)
        }

        return entries
    }

    /// Backward-compat alias — used to return running apps only.
    /// Now returns the same as `apps()` filtered to `running == true`.
    public static func runningApps() -> [AppInfo] {
        apps().filter { $0.running }
    }

    // MARK: - Internals

    private static func makeRunningInfo(
        _ app: NSRunningApplication
    ) -> AppInfo {
        AppInfo(
            pid: app.processIdentifier,
            bundleId: app.bundleIdentifier,
            name: app.localizedName ?? "",
            running: true,
            active: app.isActive
        )
    }

    /// Scan the common app-install directories for .app bundles. Cheap
    /// enough per call (~100-300 apps on a typical install) that we
    /// don't cache.
    private static func installedApps() -> [AppInfo] {
        var apps: [AppInfo] = []
        let fm = FileManager.default
        let home = fm.homeDirectoryForCurrentUser.path
        let roots = [
            "/Applications",
            "/Applications/Utilities",
            "\(home)/Applications",
            "/System/Applications",
            "/System/Applications/Utilities",
        ]
        for root in roots {
            guard let children = try? fm.contentsOfDirectory(atPath: root)
            else { continue }
            for child in children where child.hasSuffix(".app") {
                let path = "\(root)/\(child)"
                guard let info = infoFromBundle(at: path) else { continue }
                apps.append(info)
            }
        }
        return apps
    }

    private static func infoFromBundle(at path: String) -> AppInfo? {
        guard
            let bundle = Bundle(path: path),
            let bundleId = bundle.bundleIdentifier
        else { return nil }
        let name = (bundle.infoDictionary?["CFBundleDisplayName"] as? String)
            ?? (bundle.infoDictionary?["CFBundleName"] as? String)
            ?? bundle.bundleURL.deletingPathExtension().lastPathComponent
        return AppInfo(
            pid: 0,
            bundleId: bundleId,
            name: name,
            running: false,
            active: false
        )
    }
}
