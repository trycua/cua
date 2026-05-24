import AppKit
import Foundation
import os

/// An opaque handle returned by ``SystemFocusStealPreventer/beginSuppression``.
/// Pass the same handle to ``SystemFocusStealPreventer/endSuppression`` to
/// stop suppressing for that particular target; other concurrent suppressions
/// stay active until their own handles are ended.
///
/// **Prefer ``SystemFocusStealPreventer/withSuppression(targetPid:restoreTo:origin:body:)``
/// over manual `begin`/`end` whenever the suppression's lifetime fits inside
/// a single async function** — the closure form is leak-proof by construction.
/// When the lifetime must span function boundaries (e.g. a snapshot taken
/// before an action and released after side-effect detection), prefer
/// ``SuppressionLease`` over raw handles — the lease releases the entry in
/// `deinit`, so ARC catches leaks that scope-bound defers cannot.
public struct SuppressionHandle: Sendable, Hashable {
    fileprivate let id: UUID

    fileprivate init() {
        self.id = UUID()
    }
}

/// Reference-typed lease for a focus suppression entry. Releases the entry
/// in `deinit`, which is ARC's strongest available guarantee that no exit
/// path — including thrown errors, task cancellation, or future call-site
/// regressions — can leak the underlying registration.
///
/// Construct via ``SystemFocusStealPreventer/leaseSuppression(targetPid:restoreTo:origin:)``.
/// Call ``release()`` explicitly when you want to await pending reactivation
/// tasks; otherwise just drop the lease and ARC will fire a fire-and-forget
/// cleanup. `release()` is idempotent.
///
/// This is the recommended API for the snapshot/detect pattern where the
/// suppression's lifetime must span function boundaries — the lease can be
/// stored in a struct and the cleanup is guaranteed by the language, not
/// by call-site discipline.
public final class SuppressionLease: @unchecked Sendable {
    private let preventer: SystemFocusStealPreventer
    private let handle: SuppressionHandle
    /// `OSAllocatedUnfairLock<Bool>` rather than `NSLock`+`var` because Swift 6
    /// bans `NSLock.lock()` from async contexts (the kernel-level priority-
    /// inversion guarantees of `os_unfair_lock` mean the runtime can prove
    /// the critical section is bounded). This is the platform-idiomatic
    /// async-safe replacement for "lock + bool flag" patterns. macOS 13+,
    /// and we target macOS 14, so it's freely available.
    private let releasedFlag = OSAllocatedUnfairLock(initialState: false)

    /// The handle for the underlying entry. Useful for callers that want to
    /// pass through the legacy ``SystemFocusStealPreventer/endSuppression(_:)``
    /// API; new code should prefer ``release()``.
    public var rawHandle: SuppressionHandle { handle }

    fileprivate init(preventer: SystemFocusStealPreventer, handle: SuppressionHandle) {
        self.preventer = preventer
        self.handle = handle
    }

    /// Release the lease and await any in-flight reactivation tasks.
    /// Idempotent: calling more than once is a no-op. Concurrent calls are
    /// race-safe — exactly one will perform the dispatcher remove, the
    /// rest return early.
    public func release() async {
        // Atomic test-and-set. Returns the prior value; we proceed only
        // when we were the first caller to flip false→true.
        let alreadyReleased = releasedFlag.withLock { released in
            let prior = released
            released = true
            return prior
        }
        if alreadyReleased { return }
        await preventer.endSuppression(handle)
    }

    deinit {
        // ARC safety net: the holder dropped us without calling release().
        // Same atomic test-and-set as release(), but we can't await from a
        // deinit so we hand the cleanup to a detached Task. Pending
        // reactivation tasks scheduled by the observer are orphaned —
        // they're harmless idempotent `activate(options: [])` calls. The
        // deadline eviction in the dispatcher (layer 3) catches the same
        // case in bounded time even if this Task is never scheduled, so
        // we lose nothing by fire-and-forgetting here.
        let alreadyReleased = releasedFlag.withLock { released in
            let prior = released
            released = true
            return prior
        }
        if alreadyReleased { return }
        let p = preventer
        let h = handle
        Task.detached { await p.endSuppression(h) }
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
/// ## Lifetime safety
///
/// The shared dispatcher applies four overlapping guarantees so that no
/// single bug can resurrect the v0.1.9 focus-trap regression where a
/// leaked wildcard entry hijacked every app activation in the OS for the
/// rest of the process's life:
///
/// 1. **Closure scope (preferred)** — ``withSuppression(targetPid:restoreTo:origin:body:)``
///    pairs begin/end with `defer`. No handle escapes the closure.
/// 2. **ARC scope** — ``leaseSuppression(targetPid:restoreTo:origin:)`` returns
///    a ``SuppressionLease`` that ends the entry in `deinit`. Catches any
///    control flow scope-defer cannot — thrown errors between begin and end,
///    task cancellation, future call-site regressions.
/// 3. **Wall-clock deadline** — every entry carries a ``maxLifetimeNs``
///    expiry (default 5 s). The observer evicts expired entries on every
///    fire; a janitor task evicts during idle. **Worst-case leak duration is
///    bounded by ``maxLifetimeNs``, independent of every other layer.**
/// 4. **Observability** — every entry carries an ``origin`` tag and the
///    dispatcher logs a warning when active count crosses
///    ``warnActiveThreshold`` or when the deadline reaper fires. Future
///    leaks surface in `log show --process cua-driver` instead of silently
///    stealing focus.
///
/// Multiple concurrent suppressions are supported — each registration adds
/// an entry to the internal map. The shared `NSWorkspace` observer is
/// installed on the first suppression and removed when the last entry is
/// gone (whether removed manually, by lease deinit, or by deadline).
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
    /// Calculator-with-no-window has been verified to be a separate
    /// issue (`activates = false` swallows the initial window event)
    /// and tuning this delay does not rescue it.
    public static let suppressionDelayNs: UInt64 = 0

    /// Wall-clock upper bound on a suppression entry's lifetime. The
    /// dispatcher evicts entries older than this whenever the observer
    /// fires or the janitor runs. Set well above the longest legitimate
    /// click + detect window (≈1.3 s) so the safety net never trips
    /// during normal operation, but tight enough that a runaway leak
    /// recovers in seconds rather than the entire process lifetime.
    ///
    /// This bound is the layer-3 safety net that makes ``SuppressionLease``
    /// `deinit` and ``withSuppression`` `defer` mistakes recoverable.
    public static let maxLifetimeNs: UInt64 = 5_000_000_000  // 5 s

    /// How often the janitor task wakes up during idle to evict expired
    /// entries when no NSWorkspace activation events arrive. Cheap —
    /// just a lock + dictionary scan. Keeps the worst-case eviction
    /// latency at `maxLifetimeNs + janitorIntervalNs`.
    public static let janitorIntervalNs: UInt64 = 1_000_000_000  // 1 s

    /// Active-entry count above which the dispatcher logs a warning to the
    /// unified log. Legitimate workloads have at most ~2 concurrent
    /// suppressions (one from `WindowChangeDetector.snapshot()`, one from
    /// `LaunchAppTool`'s placeholder→pid swap). Anything above 2 is
    /// suspicious; above this threshold it's almost certainly a leak.
    public static let warnActiveThreshold: Int = 4

    /// Default origin tag used when a caller doesn't supply one. Surfaces
    /// in leak warnings as a fallback so we can still grep for the file.
    fileprivate static let unknownOrigin = "<unknown>"

    private let dispatcher: Dispatcher
    private let janitorIntervalNs: UInt64
    private var janitorTask: Task<Void, Never>?

    /// Designated initializer. Production callers use the default values
    /// for `maxLifetimeNs` / `janitorIntervalNs` / `warnActiveThreshold`
    /// — those are the safety-net knobs and there's no good reason to
    /// vary them in production. Tests pass tight values to verify the
    /// layer-3 reaper deterministically.
    ///
    /// Actors don't support `convenience` inits (they have a flat init
    /// model), so we expose one initializer with sensible defaults.
    public init(
        suppressionDelayNs: UInt64 = SystemFocusStealPreventer.suppressionDelayNs,
        maxLifetimeNs: UInt64 = SystemFocusStealPreventer.maxLifetimeNs,
        janitorIntervalNs: UInt64 = SystemFocusStealPreventer.janitorIntervalNs,
        warnActiveThreshold: Int = SystemFocusStealPreventer.warnActiveThreshold
    ) {
        self.dispatcher = Dispatcher(
            suppressionDelayNs: suppressionDelayNs,
            maxLifetimeNs: maxLifetimeNs,
            warnActiveThreshold: warnActiveThreshold
        )
        self.janitorIntervalNs = janitorIntervalNs
    }

    // MARK: - Closure-scoped (preferred)

    /// Run `body` while a suppression entry is active. The entry is
    /// guaranteed to be released on every exit path — return, throw, task
    /// cancellation. No handle escapes the closure, so callers cannot
    /// forget to release.
    ///
    /// This is the strongest available API: the language enforces the
    /// lifetime. Use it whenever the suppression fits inside a single
    /// async function.
    @discardableResult
    public func withSuppression<T: Sendable>(
        targetPid: pid_t,
        restoreTo: NSRunningApplication,
        origin: StaticString = #function,
        body: @Sendable () async throws -> T
    ) async rethrows -> T {
        let handle = dispatcher.add(
            targetPid: targetPid, restoreTo: restoreTo, origin: "\(origin)"
        )
        startJanitorIfNeeded()
        do {
            let result = try await body()
            await endSuppression(handle)
            return result
        } catch {
            await endSuppression(handle)
            throw error
        }
    }

    // MARK: - ARC-scoped

    /// Register a suppression and return a ``SuppressionLease`` that ends
    /// it in `deinit`. Use this when the lifetime must span function
    /// boundaries (e.g. snapshot/detect pattern) and a closure scope won't
    /// work. ARC catches leaks that scope-defers cannot.
    ///
    /// The caller can call ``SuppressionLease/release()`` to await pending
    /// reactivation tasks; if the caller simply drops the lease, ARC fires
    /// a fire-and-forget cleanup. Either way the entry is released.
    public func leaseSuppression(
        targetPid: pid_t,
        restoreTo: NSRunningApplication,
        origin: StaticString = #function
    ) -> SuppressionLease {
        let handle = dispatcher.add(
            targetPid: targetPid, restoreTo: restoreTo, origin: "\(origin)"
        )
        startJanitorIfNeeded()
        return SuppressionLease(preventer: self, handle: handle)
    }

    // MARK: - Manual (deprecated; kept for migration)

    /// Begin suppressing. Manual lifetime — caller is responsible for
    /// matching ``endSuppression(_:)``. **Prefer ``withSuppression`` or
    /// ``leaseSuppression`` over this manual API.** Direct begin/end pairs
    /// are vulnerable to leaks across error and async boundaries; the
    /// scoped APIs above make those leaks impossible.
    ///
    /// Returns a handle that must be passed to ``endSuppression(_:)`` to
    /// stop the suppression. Overlapping calls for different targets are
    /// independent — each registers its own `(pid, restoreTo)` entry. The
    /// underlying entry is also subject to the dispatcher's
    /// ``maxLifetimeNs`` deadline, so a forgotten end will self-recover
    /// in bounded time.
    @available(*, deprecated, message: "Prefer withSuppression { … } (closure-scoped) or leaseSuppression() (ARC-scoped). Manual begin/end pairs are leak-prone across error and async boundaries.")
    @discardableResult
    public func beginSuppression(
        targetPid: pid_t,
        restoreTo: NSRunningApplication,
        origin: StaticString = #function
    ) async -> SuppressionHandle {
        let handle = dispatcher.add(
            targetPid: targetPid, restoreTo: restoreTo, origin: "\(origin)"
        )
        startJanitorIfNeeded()
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

    // MARK: - Diagnostics

    /// Number of currently-active suppression entries. Test/diagnostic-only.
    public var activeCount: Int {
        dispatcher.activeCount
    }

    // MARK: - Janitor

    private func startJanitorIfNeeded() {
        if janitorTask != nil { return }
        let dispatcher = self.dispatcher
        let interval = self.janitorIntervalNs
        janitorTask = Task.detached(priority: .background) { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: interval)
                let evicted = dispatcher.reapExpired()
                for task in evicted { _ = await task.value }
                // Idle shutdown: when the dispatcher has no entries and
                // observer is torn down, stop the janitor.
                if await self?.shouldStopJanitor() ?? true { break }
            }
            await self?.clearJanitor()
        }
    }

    /// Test-only: force a reap pass without waiting for the janitor or
    /// an `NSWorkspace` activation. Production code should never call
    /// this — eviction is automatic. Exposed for unit tests so the
    /// layer-3 deadline contract can be verified deterministically.
    public func _forceReapForTesting() async {
        let pending = dispatcher.reapExpired()
        for task in pending { _ = await task.value }
    }

    private func shouldStopJanitor() -> Bool {
        dispatcher.activeCount == 0
    }

    private func clearJanitor() {
        janitorTask = nil
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
        let origin: String
        /// Wall-clock deadline (mach_absolute_time-style monotonic ns).
        /// Layer-3 safety net: when the observer fires or the janitor
        /// runs, any entry with `now > deadline` is force-evicted.
        let deadline: UInt64
    }

    private let suppressionDelayNs: UInt64
    private let maxLifetimeNs: UInt64
    private let warnActiveThreshold: Int

    private let lock = NSLock()
    private var entries: [UUID: Entry] = [:]
    private var pendingRestoreTasks: [Task<Void, Never>] = []
    private var observer: NSObjectProtocol?

    /// Unified-log subsystem. Routed through `os.Logger` so the messages
    /// appear in `log show --process cua-driver` and `log stream`. We
    /// don't take a swift-log dependency — `os.Logger` is free, builds
    /// into Console.app, and is the right tool for "operator wants to
    /// see what the driver did last Tuesday" diagnostics.
    private let logger = Logger(
        subsystem: "io.trycua.cua-driver", category: "FocusStealPreventer"
    )

    init(suppressionDelayNs: UInt64, maxLifetimeNs: UInt64, warnActiveThreshold: Int) {
        self.suppressionDelayNs = suppressionDelayNs
        self.maxLifetimeNs = maxLifetimeNs
        self.warnActiveThreshold = warnActiveThreshold
    }

    var activeCount: Int {
        lock.lock(); defer { lock.unlock() }
        return entries.count
    }

    /// Register a new entry and return its handle. Installs the shared
    /// `NSWorkspace` observer if this is the first entry. Logs a warning
    /// if the active count crosses the leak-suspicion threshold so future
    /// regressions surface in the unified log instead of silently
    /// stealing focus.
    func add(
        targetPid: pid_t, restoreTo: NSRunningApplication, origin: String
    ) -> SuppressionHandle {
        let handle = SuppressionHandle()
        let deadline = monotonicNow() &+ maxLifetimeNs

        lock.lock()
        entries[handle.id] = Entry(
            targetPid: targetPid,
            restoreTo: restoreTo,
            origin: origin,
            deadline: deadline
        )
        let count = entries.count
        let needsObserver = (observer == nil)
        // Snapshot a description list while holding the lock so we can
        // log without re-acquiring it.
        let leakSuspect = count > warnActiveThreshold
        let originList = leakSuspect ? entries.values.map(\.origin).sorted() : []
        lock.unlock()

        if needsObserver {
            installObserver()
        }

        if leakSuspect {
            // Surface, don't crash. A leak is a bug we want to fix; an
            // assert in production breaks the user's automation. Log it
            // loudly in the unified log instead — operators can grep for
            // "FocusStealPreventer leak" and the origin list pinpoints
            // the call sites holding the entries.
            logger.warning(
                """
                FocusStealPreventer leak suspect: \(count, privacy: .public) active \
                entries (threshold \(self.warnActiveThreshold, privacy: .public)). \
                Origins: \(originList.joined(separator: ", "), privacy: .public)
                """
            )
        }

        return handle
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

    /// Layer-3 safety net: scan for entries past their deadline and force-
    /// evict them. Returns any pending reactivation tasks that the caller
    /// can drain.
    ///
    /// Called from two places: (1) the janitor task on a timer, (2) the
    /// activation observer on every fire. The observer-side reap is what
    /// makes a leaked wildcard entry stop hijacking activations *before*
    /// the next user app-switch — even if the janitor is starved.
    @discardableResult
    func reapExpired() -> [Task<Void, Never>] {
        let now = monotonicNow()

        lock.lock()
        var evicted: [(UUID, Entry)] = []
        for (id, entry) in entries where now > entry.deadline {
            evicted.append((id, entry))
            entries.removeValue(forKey: id)
        }
        let shouldRemoveObserver = entries.isEmpty && !evicted.isEmpty
        let token = observer
        if shouldRemoveObserver {
            observer = nil
        }
        let pending = shouldRemoveObserver ? pendingRestoreTasks : []
        if shouldRemoveObserver {
            pendingRestoreTasks = []
        }
        lock.unlock()

        if shouldRemoveObserver, let token {
            NSWorkspace.shared.notificationCenter.removeObserver(token)
        }

        for (_, entry) in evicted {
            // Errors, not warnings: deadline reap means a higher-layer
            // guarantee (closure defer / lease deinit) failed. Surface
            // loudly so the next operator pass can find it.
            logger.error(
                """
                FocusStealPreventer deadline reap: evicted entry origin=\
                \(entry.origin, privacy: .public) targetPid=\
                \(entry.targetPid, privacy: .public). This indicates a \
                missing release path; investigate the named origin.
                """
            )
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

        // Reap on every fire. Cheap (one dictionary scan) and bounds the
        // worst-case leak duration to `maxLifetimeNs` — the leaked entry
        // stops hijacking activations *before* this very fire schedules a
        // restore task.
        reapExpired()

        lock.lock()
        // Match entries where:
        //   - targetPid == activatedPid  (specific target suppression), OR
        //   - targetPid == 0             (wildcard: suppress any activation that
        //                                 isn't restoreTo — used by the side-effect
        //                                 guard in WindowChangeDetector so that a
        //                                 background click opening a new app, e.g.
        //                                 UTM Gallery → Safari, is suppressed even
        //                                 though we didn't know Safari's pid ahead
        //                                 of time.)
        let restoreCandidates = entries.values
            .filter {
                $0.targetPid == activatedPid ||
                ($0.targetPid == 0 && activatedPid != $0.restoreTo.processIdentifier)
            }
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

// MARK: - Time

/// Monotonic nanosecond clock for entry deadlines. Uses
/// `clock_gettime(CLOCK_MONOTONIC_RAW)` so jumps in wall time (sleep,
/// NTP slew) cannot accidentally expire entries early or extend leaks.
@inline(__always)
private func monotonicNow() -> UInt64 {
    var ts = timespec()
    clock_gettime(CLOCK_MONOTONIC_RAW, &ts)
    return UInt64(ts.tv_sec) &* 1_000_000_000 &+ UInt64(ts.tv_nsec)
}
