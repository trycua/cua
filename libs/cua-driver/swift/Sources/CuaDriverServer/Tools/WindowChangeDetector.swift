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
        /// CGWindowID of the window. Stable for the window's lifetime.
        public let windowId: Int
        /// Process id of the app that owns the window.
        public let pid: Int32
        /// Owning app's localized name (e.g. "Safari").
        public let appName: String
        /// Window title at the moment of detection. May be empty.
        public let title: String
    }

    /// State captured before the action.
    ///
    /// Holds an ARC-managed ``SuppressionLease`` rather than a raw handle.
    /// **This is the leak-proofing for the snapshot/detect pattern**: if a
    /// caller drops the `Snapshot` without ever calling
    /// ``WindowChangeDetector/detectChanges(snapshot:timeout:pollInterval:)``
    /// (e.g. an early-return error path between snapshot and detect), the
    /// lease's `deinit` releases the underlying entry. The cleanup happens
    /// by the language, not by call-site discipline.
    ///
    /// `Snapshot` is a struct, so a copy retains the lease reference. The
    /// final copy going out of scope is what fires deinit; explicit
    /// `detectChanges` releases it earlier and turns deinit into a no-op.
    public struct Snapshot: Sendable {
        /// CGWindowIDs of all visible layer-0 windows at snapshot time.
        public let windowIds: Set<Int>
        /// PID of the frontmost application at snapshot time, or nil.
        public let frontPid: Int32?
        /// Wildcard suppression lease armed at snapshot time.
        /// Active from `snapshot()` through `detectChanges()` so that any
        /// app that self-activates as a side-effect of the action (e.g.
        /// Safari opening from UTM Gallery) is blocked before the first
        /// compositor frame, not just after we notice it in the poll loop.
        ///
        /// ARC-managed: dropping the Snapshot without calling
        /// `detectChanges` is safe — `SuppressionLease.deinit` releases.
        internal let suppressionLease: SuppressionLease?
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

        /// Sentinel returned when the detection window elapses with no
        /// new windows and no foreground change. Reused so callers can
        /// `return .noChange` cheaply.
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
        // Lease form: ARC releases on Snapshot drop even if detectChanges()
        // is never called (early-return error path between snapshot and
        // detect). origin tag surfaces in the unified log if a leak warning
        // is ever triggered.
        var lease: SuppressionLease?
        if let pid = frontPid,
           let restoreTo = NSRunningApplication(processIdentifier: pid)
        {
            lease = await AppStateRegistry.systemFocusStealPreventer
                .leaseSuppression(
                    targetPid: 0,
                    restoreTo: restoreTo,
                    origin: "WindowChangeDetector.snapshot"
                )
        }

        return Snapshot(windowIds: ids, frontPid: frontPid, suppressionLease: lease)
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
        // Single-exit refactor so the lease can be torn down with a
        // direct `await` (not a detached Task) before returning. The
        // earlier defer-with-Task.detached form let `detectChanges`
        // return while the wildcard suppressor was still active —
        // a stale lease bleeding into the next action's snapshot
        // window. Awaiting in-line is the only way to make the
        // detection boundary the lease teardown boundary.
        //
        // The lease's `deinit` safety net still applies if this call
        // is somehow skipped (caller early-return between snapshot and
        // detect): ARC fires deinit when the Snapshot copy goes out of
        // scope and the entry is released. Belt + suspenders.
        var result: Changes = .noChange
        let deadline = Date().addingTimeInterval(timeout)
        pollLoop: while Date() < deadline {
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
                result = Changes(
                    newWindows: newEvents, foregroundChanged: foregroundChanged
                )
                break pollLoop
            }
        }

        // Tear down the wildcard suppressor BEFORE returning. Direct
        // `await` (not `Task { ... }`) so the dispatcher entry and any
        // in-flight delayed reactivation Tasks are fully drained before
        // the caller sees `result`. Without this, the next caller's
        // snapshot()  could observe the stale wildcard still firing on
        // the next NSWorkspace activation.
        if let lease = snapshot.suppressionLease {
            await lease.release()
        }
        return result
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
