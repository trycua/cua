import AppKit
import CuaDriverCore
import Foundation

/// Detects windows and foreground-app changes that happen as a side-effect
/// of a click or other action, then re-raises the original foreground
/// application so background agent actions never permanently displace the
/// user's active context.
///
/// Usage (inside an `async` tool handler):
///
///     let snap = await WindowChangeDetector.snapshot()
///     // … perform action …
///     let changes = await WindowChangeDetector.detectChanges(snapshot: snap)
///     if let pid = snap.frontPid, changes.needsRestore {
///         await WindowChangeDetector.reRaiseForeground(pid: pid)
///     }
///     let suffix = changes.resultSuffix
///
public enum WindowChangeDetector {

    // MARK: - Types

    /// A lightweight record for a newly-appeared window.
    public struct WindowEvent: Sendable {
        public let windowId: Int
        public let pid: Int32
        public let appName: String
        public let title: String
    }

    /// State captured before the action.
    public struct Snapshot: Sendable {
        /// CGWindowIDs of all visible layer-0 windows at snapshot time.
        public let windowIds: Set<Int>
        /// PID of the frontmost application at snapshot time, or nil.
        public let frontPid: Int32?
        /// Wildcard suppression handle armed at snapshot time.
        /// Active from `snapshot()` through `detectChanges()` so that any
        /// app that self-activates as a side-effect of the action (e.g.
        /// Safari opening from UTM Gallery) is blocked before the first
        /// compositor frame, not just after we notice it in the poll loop.
        internal let suppressionHandle: SuppressionHandle?
    }

    /// What changed after the action.
    public struct Changes: Sendable {
        /// Windows that appeared after the action (new window IDs).
        public let newWindows: [WindowEvent]
        /// True when the frontmost app changed to a pid other than the
        /// original frontmost pid.
        public let foregroundChanged: Bool
        /// True when we found evidence that the action triggered a cross-app
        /// side-effect that required (or would have required) a foreground
        /// restore. True when:
        ///   - The OS-level frontmost app changed (detected before the
        ///     wildcard suppressor could fire), OR
        ///   - New windows appeared — even if foregroundChanged is false
        ///     because the wildcard suppressor in snapshot() already blocked
        ///     the steal before the poll loop could observe it.
        public var needsRestore: Bool { foregroundChanged || !newWindows.isEmpty }

        /// One-liner summary to append to a tool result, or empty string when
        /// nothing interesting happened.
        public var resultSuffix: String {
            guard needsRestore else { return "" }

            if !newWindows.isEmpty {
                // Deduplicate by app name for a compact message.
                let byApp = Dictionary(grouping: newWindows, by: { $0.appName })
                let appSummaries = byApp.map { (app, wins) -> String in
                    let titles = wins.compactMap {
                        $0.title.isEmpty ? nil : "\"\($0.title)\""
                    }
                    return titles.isEmpty ? app : "\(app) (\(titles.joined(separator: ", ")))"
                }.sorted()
                return "\n\n🪟 Action opened new window(s): \(appSummaries.joined(separator: "; "))."
            } else {
                return "\n\n🔀 Action caused a different app to become frontmost."
            }
        }

        public static let noChange = Changes(newWindows: [], foregroundChanged: false)
    }

    // MARK: - Public API

    /// Capture the current window set and frontmost pid, and arm the wildcard
    /// focus-steal suppressor. Call this immediately before performing an action.
    ///
    /// The suppressor (targetPid = 0) blocks any app from stealing focus from
    /// the current frontmost between `snapshot()` and `detectChanges()` — this
    /// covers the window during which the AX action fires and any side-effect
    /// app (e.g. Safari opening from UTM Gallery) would otherwise briefly
    /// appear at the front before we notice it in the poll loop.
    ///
    /// Safe to call from any thread — CGWindowList and NSWorkspace are
    /// both accessible off the main thread in a non-UI process.
    public static func snapshot() async -> Snapshot {
        let ids = currentWindowIds()
        let frontPid = await MainActor.run {
            NSWorkspace.shared.frontmostApplication?.processIdentifier
        }

        // Arm the wildcard suppressor immediately — before the action fires.
        var suppressionHandle: SuppressionHandle?
        if let pid = frontPid,
           let restoreTo = NSRunningApplication(processIdentifier: pid)
        {
            suppressionHandle = await AppStateRegistry.systemFocusStealPreventer
                .beginSuppression(targetPid: 0, restoreTo: restoreTo)
        }

        return Snapshot(windowIds: ids, frontPid: frontPid, suppressionHandle: suppressionHandle)
    }

    /// Poll for up to `timeout` seconds for new windows or a foreground-app
    /// change. Returns as soon as a change is detected or the timeout elapses.
    ///
    /// The wildcard suppressor that was armed in `snapshot()` stays active
    /// for the entire detection window and is ended on return. This ensures
    /// side-effect apps that activate after the poll loop starts are also
    /// suppressed.
    ///
    /// - Parameters:
    ///   - snapshot: The `Snapshot` captured before the action.
    ///   - timeout: How long to wait for a side-effect. Default 0.3s — new
    ///     windows triggered by a click appear within ~200ms on macOS; the
    ///     extra 100ms gives the suppressor time to fire and settle.
    ///   - pollInterval: Check interval in milliseconds. Default 50ms.
    public static func detectChanges(
        snapshot: Snapshot,
        timeout: TimeInterval = 1.0,
        pollInterval: Int = 50
    ) async -> Changes {
        // End the wildcard suppressor that was armed in snapshot() once
        // we return — covers the full action + detection window.
        defer {
            if let handle = snapshot.suppressionHandle {
                Task { await AppStateRegistry.systemFocusStealPreventer.endSuppression(handle) }
            }
        }

        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            try? await Task.sleep(for: .milliseconds(pollInterval))

            // --- New windows ---
            let current = currentWindowIds()
            let newIds = current.subtracting(snapshot.windowIds)
            var newEvents: [WindowEvent] = []
            if !newIds.isEmpty {
                // Resolve app names / titles from the live window list.
                let allVisible = WindowEnumerator.visibleWindows().filter { $0.layer == 0 }
                newEvents = allVisible
                    .filter { newIds.contains($0.id) }
                    .map { WindowEvent(
                        windowId: $0.id,
                        pid: $0.pid,
                        appName: $0.owner,
                        title: $0.name
                    )}
            }

            // --- Foreground change ---
            // With the wildcard suppressor armed we expect the frontmost app
            // to remain stable. Track changes anyway so the result suffix can
            // accurately describe what happened (the suppressor fires async
            // and a very brief transient change may still be observed here).
            let currentFront = await MainActor.run {
                NSWorkspace.shared.frontmostApplication?.processIdentifier
            }
            let foregroundChanged: Bool
            if let orig = snapshot.frontPid, let cur = currentFront {
                foregroundChanged = cur != orig
            } else {
                foregroundChanged = false
            }

            if !newEvents.isEmpty || foregroundChanged {
                return Changes(newWindows: newEvents, foregroundChanged: foregroundChanged)
            }
        }
        return .noChange
    }

    /// Re-activate the previously-frontmost application, sending its window
    /// back to the front on the current Space without moving or resizing it.
    ///
    /// Uses `activate(options: [])` — the non-deprecated form on macOS 14+.
    ///
    /// Call this AFTER `detectChanges` as a belt-and-suspenders restore in
    /// case the wildcard suppressor's async Task hasn't settled yet.
    @MainActor
    public static func reRaiseForeground(pid: Int32) {
        NSRunningApplication(processIdentifier: pid)?
            .activate(options: [])
    }

    // MARK: - Internal

    /// CGWindowIDs of all currently-visible layer-0 windows.
    /// Thread-safe — CGWindowListCopyWindowInfo is documented as callable
    /// from any thread.
    static func currentWindowIds() -> Set<Int> {
        Set(
            WindowEnumerator.visibleWindows()
                .filter { $0.layer == 0 }
                .map { $0.id }
        )
    }
}
