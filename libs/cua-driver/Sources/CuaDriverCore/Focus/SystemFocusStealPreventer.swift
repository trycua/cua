import AppKit
import Foundation

/// An opaque handle returned by ``SystemFocusStealPreventer/beginSuppression``.
/// Pass the same handle to ``SystemFocusStealPreventer/endSuppression`` to
/// stop suppressing for that particular target; other concurrent suppressions
/// stay active until their own handles are ended.
public struct SuppressionHandle: Sendable, Hashable {
    fileprivate let id: UUID

    fileprivate init() {
        self.id = UUID()
    }
}

/// Layer 3 of the focus-suppression stack. Reactively counters the
/// "target app called `NSApp.activate(ignoringOtherApps:)` in its own
/// `applicationDidFinishLaunching`" failure mode.
///
/// AppKit's `NSWorkspace.OpenConfiguration.activates = false` is honored by
/// LaunchServices, but does nothing to stop the launched app from activating
/// itself once its process is running. Calculator, many Electron shells, and
/// various AppKit apps do exactly that, which causes a brief front-app flash
/// even when the caller asked for a background launch.
///
/// Mechanism (pure public AppKit — no private SPIs, no CGEventTap, no
/// undocumented symbols):
///
/// 1. Subscribe to `NSWorkspace.didActivateApplicationNotification`.
/// 2. When the newly-active app's pid matches any active suppression's
///    `targetPid`, schedule `restoreTo.activate(options: [])` on the main
///    actor after a short delay (``suppressionDelayNs``). The observer
///    itself stays synchronous on the posting thread — we only hop off it
///    to sleep.
/// 3. The delay is a deliberate tradeoff between flash visibility (short
///    delay = less visible) and giving the target's post-activation
///    runloop work a chance to complete before we demote it back. See
///    the `suppressionDelayNs` constant for the empirical caveat around
///    Calculator on macOS 14+: the apparent "Calculator has no window
///    after layer-3 fires" bug is really "Calculator has no window when
///    launched with `activates = false`, regardless of layer-3" and
///    isn't rescuable by tuning this delay.
///
/// This is still a **reactive** suppression — the target briefly becomes
/// frontmost and the user sees a sub-``suppressionDelayNs`` flash.
/// Eliminating the flash entirely would require pre-emptively
/// intercepting activation events via private
/// `CGSRegisterConnectionNotifyProc` / kCPS notifications, which we
/// deliberately do not take a dependency on.
///
/// Multiple concurrent suppressions are supported — each `beginSuppression`
/// call returns a distinct handle and adds an entry to the internal map.
/// The shared `NSWorkspace` observer is installed on the first suppression
/// and removed when the last handle is ended.
public actor SystemFocusStealPreventer {
    /// Delay between observing the target's self-activation and firing
    /// the restoring `activate(options: [])`. Tradeoff:
    ///   - Too short: a target that does post-activation work on the
    ///     main runloop (creating a window, wiring up menus, etc.) can
    ///     be interrupted mid-sequence if we yank focus back
    ///     synchronously from the activation observer.
    ///   - Too long: the flash is visible to the user.
    ///
    /// Was 300 ms — visibly flashed Chrome on top of the user's work
    /// for ~18 frames before demotion. Dropped to 0 to demote
    /// synchronously within the same run-loop turn as the activation
    /// notification. A zero-delay synchronous demote reliably completes
    /// before the window server composites the next frame, so the
    /// target never visually reaches the front.
    ///
    /// Historical caveat that kept us at 300 ms: we worried that a
    /// target doing synchronous post-activation work on the main
    /// runloop would be interrupted. In practice, the target gets
    /// several frames' worth of runloop turns inside
    /// `applicationDidFinishLaunching` BEFORE our demote reaches
    /// WindowServer — the activation notification itself is async.
    /// Calculator still gets its window created (orthogonal path via
    /// the `hides=YES` + `unhide()` dance). Chrome still gets its
    /// URL handoff processed. Net: zero-delay demote is strictly
    /// better.
    private static let suppressionDelayNs: UInt64 = 0

    private let dispatcher: Dispatcher

    public init() {
        self.dispatcher = Dispatcher(suppressionDelayNs: Self.suppressionDelayNs)
    }

    /// Begin suppressing focus-steal events for `targetPid`. Any
    /// `NSWorkspace.didActivateApplicationNotification` that fires while the
    /// suppression is active and names `targetPid` as the newly-active app
    /// schedules a delayed `restoreTo.activate(options: [])` on the main
    /// actor to steal focus back onto whatever was frontmost before the
    /// launch.
    ///
    /// Returns a handle that must be passed to ``endSuppression(_:)`` to
    /// stop the suppression. Overlapping calls for different targets are
    /// independent — each registers its own `(pid, restoreTo)` entry.
    @discardableResult
    public func beginSuppression(
        targetPid: pid_t,
        restoreTo: NSRunningApplication
    ) async -> SuppressionHandle {
        let handle = SuppressionHandle()
        dispatcher.add(handle: handle, targetPid: targetPid, restoreTo: restoreTo)
        return handle
    }

    /// Stop suppressing. Removes the entry for `handle`, awaits any
    /// in-flight delayed reactivation Tasks that were scheduled by the
    /// observer, and — if it was the last active suppression — tears down
    /// the shared `NSWorkspace` observer. Awaiting the in-flight Tasks
    /// means a caller sequence of `Task.sleep(Xms) → endSuppression` can
    /// rely on seeing the final frontmost state on return, even if a
    /// reactivation fired late in the suppression window. Idempotent —
    /// ending an unknown handle is a no-op.
    public func endSuppression(_ handle: SuppressionHandle) async {
        let pending = dispatcher.remove(handle: handle)
        for task in pending {
            _ = await task.value
        }
    }
}

// MARK: - Dispatcher

/// Lock-protected observer state. Lives outside the actor so that the
/// `NSWorkspace` observer callback — which runs synchronously on whatever
/// thread posted the notification (typically main) — can read the
/// suppression map without hopping back into the actor. Any actor hop
/// inside the synchronous observer would push the delayed-reactivation
/// scheduling further out, not closer in.
private final class Dispatcher: @unchecked Sendable {
    private struct Entry {
        let targetPid: pid_t
        let restoreTo: NSRunningApplication
    }

    private let suppressionDelayNs: UInt64
    private let lock = NSLock()
    private var entries: [UUID: Entry] = [:]
    private var pendingRestoreTasks: [Task<Void, Never>] = []
    private var observer: NSObjectProtocol?

    init(suppressionDelayNs: UInt64) {
        self.suppressionDelayNs = suppressionDelayNs
    }

    func add(handle: SuppressionHandle, targetPid: pid_t, restoreTo: NSRunningApplication) {
        lock.lock()
        entries[handle.id] = Entry(targetPid: targetPid, restoreTo: restoreTo)
        let needsObserver = (observer == nil)
        lock.unlock()

        if needsObserver {
            installObserver()
        }
    }

    /// Removes the entry for `handle` and returns any in-flight
    /// reactivation Tasks so the caller can await them. When the last
    /// entry is removed, also tears down the shared observer. The
    /// returned Tasks are drained from the dispatcher's pending list;
    /// callers own the awaits.
    func remove(handle: SuppressionHandle) -> [Task<Void, Never>] {
        lock.lock()
        entries.removeValue(forKey: handle.id)
        let shouldRemoveObserver = entries.isEmpty
        let token = observer
        if shouldRemoveObserver {
            observer = nil
        }
        let pending = pendingRestoreTasks
        if shouldRemoveObserver {
            pendingRestoreTasks = []
        }
        lock.unlock()

        if shouldRemoveObserver, let token {
            NSWorkspace.shared.notificationCenter.removeObserver(token)
        }
        return pending
    }

    private func installObserver() {
        // queue: nil delivers the callback synchronously on the posting
        // thread. NSWorkspace posts on main, so the activation handler
        // runs on main with no extra hop — which matters because we want
        // the reactivation Task scheduled as close to the thief's
        // activation moment as possible, even if the actual
        // `activate(options:)` call is then deferred by the delay.
        let token = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification,
            object: nil,
            queue: nil
        ) { [weak self] note in
            self?.handleActivation(note: note)
        }

        lock.lock()
        // Another thread may have installed an observer in the window
        // between our read and this write. Keep the first one and drop
        // the duplicate; observer install is idempotent.
        if observer == nil {
            observer = token
            lock.unlock()
        } else {
            lock.unlock()
            NSWorkspace.shared.notificationCenter.removeObserver(token)
        }
    }

    private func handleActivation(note: Notification) {
        guard
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey]
                as? NSRunningApplication
        else { return }

        let activatedPid = app.processIdentifier

        lock.lock()
        let restoreCandidates = entries.values
            .filter { $0.targetPid == activatedPid }
            .map { $0.restoreTo }
        lock.unlock()

        guard let restoreTo = restoreCandidates.first else { return }

        // Schedule the reactivation after a short delay — see the
        // `suppressionDelayNs` comment on SystemFocusStealPreventer for
        // the rationale. The observer itself stays synchronous; only the
        // restore is deferred. Stash the Task so `endSuppression` can
        // await any still-pending reactivations before returning. The
        // list is only drained by `endSuppression`; suppression windows
        // are short enough that at most a handful of Tasks accumulate.
        let delayNs = suppressionDelayNs
        let task = Task.detached {
            try? await Task.sleep(nanoseconds: delayNs)
            await MainActor.run {
                _ = restoreTo.activate(options: [])
            }
        }
        lock.lock()
        pendingRestoreTasks.append(task)
        lock.unlock()
    }
}
