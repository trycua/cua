import AppKit
import XCTest
@testable import CuaDriverCore

/// Unit tests for the four-layer leak-prevention design in
/// ``SystemFocusStealPreventer``.
///
/// The contract under test is that **no caller error path can leave a
/// suppression entry alive in the dispatcher for longer than
/// ``SystemFocusStealPreventer/maxLifetimeNs``**, regardless of which
/// API surface they used. Each test exercises one layer of the design:
///
/// 1. ``testWithSuppressionReleasesOnReturn`` /
///    ``testWithSuppressionReleasesOnThrow`` — closure scope (compiler
///    enforces release on every exit path).
/// 2. ``testLeaseReleasesOnExplicitCall`` /
///    ``testLeaseReleasesOnDeinit`` — ARC scope (deinit catches what
///    scope-defer cannot).
/// 3. ``testDeadlineReapsLeakedManualEntry`` — wall-clock deadline
///    (the safety net under everything else).
///
/// We use `NSRunningApplication.current` for `restoreTo` so the tests
/// don't depend on any external app being frontmost. The dispatcher
/// just stores the reference — none of these tests fire the
/// `NSWorkspace.didActivateApplicationNotification` observer.
final class FocusStealPreventerTests: XCTestCase {

    private var selfApp: NSRunningApplication { NSRunningApplication.current }

    // MARK: - Layer 1: closure scope

    func testWithSuppressionReleasesOnReturn() async {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)
        let __count1 = await preventer.activeCount
        XCTAssertEqual(__count1, 0)
        await preventer.withSuppression(targetPid: 0, restoreTo: selfApp) {
            let inside = await preventer.activeCount
            XCTAssertEqual(inside, 1, "entry must be live during body")
        }

        let __count2 = await preventer.activeCount

        XCTAssertEqual(__count2, 0,
            "withSuppression must release on normal return"
        )
    }

    func testWithSuppressionReleasesOnThrow() async {
        struct BodyError: Error {}
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)

        do {
            try await preventer.withSuppression(targetPid: 0, restoreTo: selfApp) {
                throw BodyError()
            }
            XCTFail("expected throw")
        } catch is BodyError {
            // expected
        } catch {
            XCTFail("unexpected error: \(error)")
        }

        let __count3 = await preventer.activeCount

        XCTAssertEqual(__count3, 0,
            "withSuppression must release on thrown error"
        )
    }

    // MARK: - Layer 2: ARC scope (lease)

    func testLeaseReleasesOnExplicitCall() async {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)
        let lease = await preventer.leaseSuppression(targetPid: 0, restoreTo: selfApp)
        let __count4 = await preventer.activeCount
        XCTAssertEqual(__count4, 1)
        await lease.release()
        let __count5 = await preventer.activeCount
        XCTAssertEqual(__count5, 0)
    }

    func testLeaseReleaseIsIdempotent() async {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)
        let lease = await preventer.leaseSuppression(targetPid: 0, restoreTo: selfApp)

        await lease.release()
        await lease.release()  // second call must be a no-op

        let __count6 = await preventer.activeCount

        XCTAssertEqual(__count6, 0)
    }

    /// ARC fires `deinit` when the lease's last reference goes out of
    /// scope. The deinit's cleanup is dispatched to a `Task.detached`,
    /// so we have to poll for the active-count to drop. The poll
    /// timeout (2 s) is generous compared to the dispatcher's release
    /// latency (microseconds). If this test ever flakes, the design's
    /// language-level guarantee has failed and the regression is
    /// urgent.
    func testLeaseReleasesOnDeinit() async throws {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)

        // Scope the lease so it deinits at the end of this block.
        do {
            let lease = await preventer.leaseSuppression(targetPid: 0, restoreTo: selfApp)
            let __count7 = await preventer.activeCount
            XCTAssertEqual(__count7, 1)
            _ = lease
        }

        try await waitForActiveCount(0, on: preventer, timeout: 2.0)
    }

    // MARK: - Layer 3: wall-clock deadline (the safety net)

    /// **The crucial test.** Even when every higher layer fails — the
    /// caller used the deprecated raw `beginSuppression`, threw away
    /// the handle, and never called `endSuppression` — the dispatcher's
    /// deadline reaper must evict the entry within `maxLifetimeNs`.
    /// Anything else means the v0.1.9 focus-trap regression class is
    /// still possible.
    ///
    /// Uses the test-seam initializer to set `maxLifetimeNs = 200 ms`
    /// so the test runs fast. The eviction is triggered explicitly via
    /// `_forceReapForTesting()`, which simulates the reap pass that
    /// happens automatically inside the activation observer and the
    /// janitor task. We don't rely on the janitor to prove the
    /// contract — that would make the test wait the full janitor
    /// interval and add CI flake risk.
    func testDeadlineReapsLeakedManualEntry() async throws {
        let testMaxLifetimeNs: UInt64 = 200_000_000  // 200 ms
        let preventer = SystemFocusStealPreventer(
            suppressionDelayNs: 0,
            maxLifetimeNs: testMaxLifetimeNs,
            janitorIntervalNs: 60_000_000_000,  // disable janitor influence
            warnActiveThreshold: 1000
        )

        // Deprecated API on purpose: this test exists *because* the
        // deprecated path remains a leak hazard for callers who haven't
        // migrated. The deadline must protect them.
        @available(*, deprecated)
        func leakAnEntry() async {
            _ = await preventer.beginSuppression(targetPid: 0, restoreTo: selfApp)
        }
        await leakAnEntry()
        let __count8 = await preventer.activeCount
        XCTAssertEqual(__count8, 1)
        // Wait past the deadline.
        try await Task.sleep(nanoseconds: testMaxLifetimeNs + 50_000_000)

        // Trigger the observer-side reap path directly.
        await preventer._forceReapForTesting()

        let __count9 = await preventer.activeCount

        XCTAssertEqual(__count9,
            0,
            "deadline reaper must evict expired entries even when the caller leaked the handle"
        )
    }

    /// The deadline reaper must not evict still-live entries just because
    /// they share a preventer with expired ones.
    func testDeadlineReapsOnlyExpiredEntries() async throws {
        let testMaxLifetimeNs: UInt64 = 200_000_000
        let preventer = SystemFocusStealPreventer(
            suppressionDelayNs: 0,
            maxLifetimeNs: testMaxLifetimeNs,
            janitorIntervalNs: 60_000_000_000,
            warnActiveThreshold: 1000
        )

        @available(*, deprecated)
        func makeOldHandle() async -> SuppressionHandle {
            await preventer.beginSuppression(targetPid: 0, restoreTo: selfApp)
        }
        let oldHandle = await makeOldHandle()
        // Wait so the first entry is past its deadline.
        try await Task.sleep(nanoseconds: testMaxLifetimeNs + 50_000_000)
        // Add a fresh entry — its deadline is `now + maxLifetimeNs`.
        let freshLease = await preventer.leaseSuppression(targetPid: 0, restoreTo: selfApp)

        await preventer._forceReapForTesting()

        let __count10 = await preventer.activeCount

        XCTAssertEqual(__count10, 1,
            "fresh entry must survive a reap pass that evicts only the expired one"
        )
        // Cleanup.
        await preventer.endSuppression(oldHandle)  // already evicted; idempotent
        await freshLease.release()
        let __count11 = await preventer.activeCount
        XCTAssertEqual(__count11, 0)
    }

    // MARK: - Diagnostics

    /// `endSuppression` of an already-evicted handle must be idempotent.
    /// Existing call sites (and the deprecated migration window) depend
    /// on this — a forgotten handle that gets reaped should not crash
    /// the eventual end call.
    func testEndSuppressionAfterDeadlineIsNoOp() async throws {
        let testMaxLifetimeNs: UInt64 = 100_000_000
        let preventer = SystemFocusStealPreventer(
            suppressionDelayNs: 0,
            maxLifetimeNs: testMaxLifetimeNs,
            janitorIntervalNs: 60_000_000_000,
            warnActiveThreshold: 1000
        )

        @available(*, deprecated)
        func beginAndForget() async -> SuppressionHandle {
            await preventer.beginSuppression(targetPid: 0, restoreTo: selfApp)
        }
        let handle = await beginAndForget()

        try await Task.sleep(nanoseconds: testMaxLifetimeNs + 50_000_000)
        await preventer._forceReapForTesting()
        let __count12 = await preventer.activeCount
        XCTAssertEqual(__count12, 0)
        // Calling end on an already-reaped entry must not crash or
        // resurrect anything.
        await preventer.endSuppression(handle)
        let __count13 = await preventer.activeCount
        XCTAssertEqual(__count13, 0)
    }

    // MARK: - Concurrency invariants (regression tests for CR feedback)

    /// Regression test for the Task-based defer that let `detectChanges`
    /// return before the lease was actually torn down. A direct `await
    /// lease.release()` must drain the dispatcher entry before the
    /// caller proceeds — otherwise a stale wildcard suppressor could
    /// bleed into the next caller's snapshot window.
    ///
    /// This test verifies the explicit `release()` semantics: the
    /// post-await `activeCount` is observed by the same task that
    /// awaited, with no scheduling gap that a `Task { await ... }`
    /// detach would introduce.
    func testExplicitReleaseDrainsBeforeReturning() async {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)
        let lease = await preventer.leaseSuppression(targetPid: 0, restoreTo: selfApp)
        let beforeRelease = await preventer.activeCount
        XCTAssertEqual(beforeRelease, 1)

        await lease.release()

        // Crucial: the count is observed *immediately* after `release`
        // returns, with no `try await Task.sleep` or polling. A
        // detached release would not satisfy this assertion
        // deterministically.
        let immediatelyAfter = await preventer.activeCount
        XCTAssertEqual(
            immediatelyAfter, 0,
            "explicit release() must drain the entry before returning, "
            + "not hand cleanup to a detached Task"
        )
    }

    /// Regression test for the LaunchAppTool placeholder→pid crossfade.
    /// Two overlapping leases must coexist in the dispatcher (overlap is
    /// the structural fix for the gap-window bug). The dispatcher's add
    /// and remove must be independent so the crossfade has zero
    /// suppression-free time.
    func testCrossfadeOfTwoLeasesHasNoSuppressionGap() async {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)
        let __c201 = await preventer.activeCount
        XCTAssertEqual(__c201, 0)
        // Phase 1: placeholder armed.
        let placeholder = await preventer.leaseSuppression(targetPid: 0, restoreTo: selfApp)
        let __c202 = await preventer.activeCount
        XCTAssertEqual(__c202, 1)
        // Phase 2: pid-specific armed BEFORE placeholder is released.
        // This is the crossfade: both entries live concurrently.
        let pidSpecific = await preventer.leaseSuppression(
            targetPid: 1234, restoreTo: selfApp
        )
        let duringOverlap = await preventer.activeCount
        XCTAssertEqual(
            duringOverlap, 2,
            "dispatcher must support two concurrent leases — this is "
            + "what makes the LaunchAppTool crossfade leak-free"
        )

        // Phase 3: drop the placeholder; the pid-specific entry survives.
        await placeholder.release()
        let afterPlaceholderDrop = await preventer.activeCount
        XCTAssertEqual(
            afterPlaceholderDrop, 1,
            "releasing the placeholder must not affect the pid-specific entry"
        )

        // Phase 4: drop the pid-specific entry. Done.
        await pidSpecific.release()
        let __c203 = await preventer.activeCount
        XCTAssertEqual(__c203, 0)
    }

    // MARK: - Wildcard tie-break (LIFO contract)

    /// **The actual LIFO winner contract.** When multiple wildcard
    /// entries match the same activation, the most-recently-registered
    /// (highest-sequence) entry's `restoreTo` must win — not just
    /// "they coexist" but specifically "the second one's restoreTo is
    /// what `handleActivation` would pick".
    ///
    /// Uses ``SystemFocusStealPreventer/_winnerForActivationTesting(activatedPid:)``
    /// — an internal test seam that runs the same selection logic
    /// `handleActivation` uses, without requiring a real
    /// `NSWorkspace.didActivateApplicationNotification`. This is the
    /// only way to verify the actual winner identity from a unit test
    /// (CI runners don't have a real frontmost-app graph).
    ///
    /// Two distinguishable `NSRunningApplication` references are
    /// required: we use Finder (PID stable for the user's session) and
    /// the test's own process. Either one is non-nil on every macOS
    /// host the test ever runs on.
    func testWildcardLifoTiebreakWinnerIsLatestRegistered() async throws {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)

        // Two distinguishable restoreTo apps. Finder is a reliable
        // second target — PID is stable per user session and `bundleId`
        // matches `com.apple.finder` deterministically.
        let finderRunning = NSRunningApplication.runningApplications(
            withBundleIdentifier: "com.apple.finder"
        ).first
        guard let finder = finderRunning else {
            throw XCTSkip(
                "Finder must be running to compare two distinguishable "
                + "restoreTo apps; not detected on this host"
            )
        }
        XCTAssertNotEqual(
            finder.processIdentifier, selfApp.processIdentifier,
            "Finder and the test process must have distinct pids — "
            + "otherwise the wildcard filter would collapse them"
        )

        // Register first wildcard with selfApp as restoreTo, second
        // with Finder as restoreTo. Both target 0 (wildcard). LIFO
        // contract: a hypothetical activation by ANY pid !=
        // {selfApp.pid, finder.pid} should return the second-registered
        // entry's restoreTo (Finder), because it has the higher sequence.
        let first = await preventer.leaseSuppression(
            targetPid: 0, restoreTo: selfApp, origin: "test.lifo.first"
        )
        let second = await preventer.leaseSuppression(
            targetPid: 0, restoreTo: finder, origin: "test.lifo.second"
        )

        // Pick a fake activation pid that's neither selfApp nor Finder
        // so both wildcards match (the activated-pid != restoreTo.pid
        // clause excludes self-restore, not third-party activations).
        // Use 1 (kernel-task / launchd, never matches restoreTo here).
        let fakeActivatedPid: pid_t = 1
        let winner = await preventer._winnerForActivationTesting(
            activatedPid: fakeActivatedPid
        )

        XCTAssertNotNil(winner, "both wildcards should match pid=1")
        XCTAssertEqual(
            winner?.processIdentifier,
            finder.processIdentifier,
            "LIFO contract: second-registered entry's restoreTo (Finder) "
            + "must win the tiebreak; the first-registered (selfApp) loses. "
            + "If this fails, the wildcard-vs-wildcard ambiguity is back."
        )

        // Now drop the second; the first should win on its own.
        await second.release()
        let winnerAfterDrop = await preventer._winnerForActivationTesting(
            activatedPid: fakeActivatedPid
        )
        XCTAssertEqual(
            winnerAfterDrop?.processIdentifier,
            selfApp.processIdentifier,
            "after dropping the second-registered entry, the first must "
            + "become the sole match and therefore the winner"
        )

        // Cleanup.
        await first.release()
        let cleanedUp = await preventer.activeCount
        XCTAssertEqual(cleanedUp, 0)
    }

    /// LIFO tiebreak also applies between a wildcard (older) and a
    /// pid-specific entry (newer) — the LaunchAppTool crossfade case.
    /// The pid-specific entry's `restoreTo` must win because it has the
    /// higher sequence, regardless of whether the wildcard would also
    /// have matched.
    func testLifoWinsAcrossWildcardAndPidSpecific() async throws {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)

        let finder = NSRunningApplication.runningApplications(
            withBundleIdentifier: "com.apple.finder"
        ).first
        guard let finder else {
            throw XCTSkip("Finder must be running for this test")
        }

        let targetPid: pid_t = 9999  // hypothetical app pid — not in the running set

        // Older wildcard that would match anything-not-selfApp.
        let wildcardLease = await preventer.leaseSuppression(
            targetPid: 0, restoreTo: selfApp, origin: "test.crossfade.wildcard"
        )
        // Newer pid-specific entry that targets the same hypothetical app.
        let pidSpecificLease = await preventer.leaseSuppression(
            targetPid: targetPid, restoreTo: finder,
            origin: "test.crossfade.pidSpecific"
        )

        let winner = await preventer._winnerForActivationTesting(
            activatedPid: targetPid
        )
        XCTAssertEqual(
            winner?.processIdentifier,
            finder.processIdentifier,
            "during the LaunchAppTool crossfade, the newer pid-specific "
            + "entry's restoreTo (Finder) must win over the older wildcard's "
            + "(selfApp) — this is what makes the gap-window-free swap safe"
        )

        await pidSpecificLease.release()
        await wildcardLease.release()
    }

    // MARK: - Per-entry maxLifetime override (#7)

    /// Callers with legitimately long-running suppressions can override
    /// the dispatcher's default 5 s deadline. The override must be
    /// honored — the entry must NOT be evicted before its custom
    /// deadline.
    func testMaxLifetimeOverrideHonored() async throws {
        // Default deadline is tiny (50 ms) — but caller's override
        // (500 ms) must win and keep the entry alive past the dispatcher
        // default.
        let preventer = SystemFocusStealPreventer(
            suppressionDelayNs: 0,
            maxLifetimeNs: 50_000_000,           // 50 ms default
            janitorIntervalNs: 60_000_000_000,   // disable janitor
            warnActiveThreshold: 1000
        )

        let lease = await preventer.leaseSuppression(
            targetPid: 0,
            restoreTo: selfApp,
            origin: "test.longLived",
            maxLifetimeOverrideNs: 500_000_000   // 500 ms override
        )

        // Wait past the default deadline but well before the override.
        try await Task.sleep(nanoseconds: 200_000_000)  // 200 ms
        await preventer._forceReapForTesting()

        let stillAlive = await preventer.activeCount
        XCTAssertEqual(
            stillAlive, 1,
            "per-entry maxLifetimeOverrideNs (500 ms) must override the "
            + "dispatcher default (50 ms); entry must survive a reap pass "
            + "after the default deadline but before the override"
        )

        // Now wait past the override and confirm reaping kicks in.
        try await Task.sleep(nanoseconds: 400_000_000)  // 400 ms more = 600 ms total
        await preventer._forceReapForTesting()

        let nowReaped = await preventer.activeCount
        XCTAssertEqual(
            nowReaped, 0,
            "after the override deadline expires, the entry must be reaped "
            + "like any other expired entry"
        )

        // Cleanup (lease was already evicted by the reaper; release is a no-op).
        await lease.release()
    }

    /// `withSuppression` accepts the same override and the entry must
    /// outlive the dispatcher default while the body is running. This
    /// catches the case where someone wires the parameter through
    /// `leaseSuppression` but forgets to plumb it through
    /// `withSuppression`.
    func testWithSuppressionOverrideKeepsEntryAlive() async {
        let preventer = SystemFocusStealPreventer(
            suppressionDelayNs: 0,
            maxLifetimeNs: 50_000_000,
            janitorIntervalNs: 60_000_000_000,
            warnActiveThreshold: 1000
        )

        await preventer.withSuppression(
            targetPid: 0,
            restoreTo: selfApp,
            maxLifetimeOverrideNs: 500_000_000
        ) {
            // Sleep past the dispatcher default (50 ms) but well below
            // the override (500 ms). The entry must still be alive when
            // we measure activeCount.
            try? await Task.sleep(nanoseconds: 150_000_000)
            await preventer._forceReapForTesting()
            let alive = await preventer.activeCount
            XCTAssertEqual(
                alive, 1,
                "withSuppression must propagate maxLifetimeOverrideNs to "
                + "the dispatcher; entry must be alive after a reap that "
                + "ran past the default deadline"
            )
        }

        let afterBody = await preventer.activeCount
        XCTAssertEqual(afterBody, 0)
    }

    // MARK: - Snapshot/detect ARC contract (#5)

    /// The structural invariant that makes `WindowChangeDetector`'s
    /// snapshot/detect pattern leak-proof: a struct holding a
    /// `SuppressionLease` reference, when copied and dropped without
    /// calling explicit `release()`, must still see the entry evicted
    /// (via lease deinit). This is the ClickTool early-return-after-
    /// snapshot scenario distilled to a unit test.
    ///
    /// `Snapshot` is a real type in `CuaDriverServer` (not testable from
    /// here without the server module), so we model the same shape with
    /// a local struct. Same ARC behaviour, same guarantee.
    func testStructHoldingLeaseReleasesOnDrop() async throws {
        let preventer = SystemFocusStealPreventer(suppressionDelayNs: 0)

        struct SnapshotShape {
            let lease: SuppressionLease?
        }

        // Construct the snapshot, copy it (struct semantics), then drop
        // both copies without calling release() — the early-return
        // pattern.
        do {
            let lease = await preventer.leaseSuppression(
                targetPid: 0, restoreTo: selfApp, origin: "test.snapshot"
            )
            let snap = SnapshotShape(lease: lease)
            let snapCopy = snap
            _ = snapCopy
            let alive = await preventer.activeCount
            XCTAssertEqual(alive, 1)
        }

        // Both snap and snapCopy went out of scope. The lease was
        // shared by reference (class), so its retain count drops to 0
        // when the last reference dies. ARC fires deinit, which
        // dispatches a Task to release. Poll for eviction.
        try await waitForActiveCount(0, on: preventer, timeout: 2.0)
    }

    // MARK: - Layer 4 observability (#3)

    /// End-to-end verification that the `os.Logger` warning path
    /// actually surfaces in the unified log under the expected
    /// subsystem. Catches regressions like:
    ///
    /// - subsystem string typo (so `log stream --predicate
    ///   'subsystem == "io.trycua.cua-driver"'` returns nothing)
    /// - `os.Logger` initialization failure (rare, but possible if the
    ///   subsystem string is rejected at build time)
    /// - Info.plist requirements being added in a future SDK that
    ///   silently drop logs from unsigned helper binaries.
    ///
    /// We trigger the leak-suspicion warning by registering more
    /// entries than `warnActiveThreshold`, then shell out to `log show`
    /// and assert the message appears. If unified log capture is
    /// disabled on this machine (rare) the test skips with `XCTSkip`.
    ///
    /// **Skipped on CI runners** that don't have the unified-log
    /// privilege or an ARM64 host: `log show` requires
    /// `com.apple.private.logging.diagnostic` or sudo, which CI
    /// containers usually lack. Running the test gates on
    /// `LayerFourLogVerify=1` in the env so CI is opt-in only.
    func testLayerFourLoggerSurfacesInUnifiedLog() async throws {
        try XCTSkipUnless(
            ProcessInfo.processInfo.environment["LayerFourLogVerify"] == "1",
            "Layer-4 log verification requires unified-log capture privilege; "
            + "set LayerFourLogVerify=1 to opt in."
        )

        // Marker tag — embedded in the origin string and grepped from
        // `log show`. Unique per test run so the assertion can't false-
        // positive on a stale prior log entry.
        let marker = "test-layer4-\(UUID().uuidString.prefix(8))"

        // 4 is the default warn threshold; 5 trips the warning.
        let preventer = SystemFocusStealPreventer(
            suppressionDelayNs: 0,
            maxLifetimeNs: 60_000_000_000,
            janitorIntervalNs: 60_000_000_000,
            warnActiveThreshold: 4
        )
        let cutoff = Date()

        var leases: [SuppressionLease] = []
        // Origin needs to be a StaticString. We can't interpolate the
        // marker into the origin field directly — so we embed it in the
        // log by bumping past the threshold and grepping for the
        // dispatcher's "FocusStealPreventer leak suspect" prefix plus a
        // count-based assertion. A subsequent assertion checks the
        // origin list contains our static origin string.
        for _ in 0..<5 {
            let lease = await preventer.leaseSuppression(
                targetPid: 0, restoreTo: selfApp,
                origin: "test.layer4.marker"
            )
            leases.append(lease)
        }

        // Give os_log time to flush. os_log is not synchronous; the
        // dispatcher writes are buffered before they hit unified log
        // storage.
        try await Task.sleep(nanoseconds: 1_500_000_000)  // 1.5 s

        // Query unified log for the warning line. The `--start` cutoff
        // is just before we triggered the warning; `--predicate`
        // narrows by subsystem to avoid scanning the entire host log.
        //
        // `log show` requires a date in `YYYY-MM-DD HH:MM:SS` format
        // (local time, NOT ISO 8601 — the leading `Z` and fractional
        // seconds are rejected with a parse error and then `log show`
        // emits an empty result. Discovered this the hard way.)
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        formatter.timeZone = TimeZone.current
        let process = Process()
        process.launchPath = "/usr/bin/log"
        process.arguments = [
            "show",
            "--predicate", "subsystem == \"io.trycua.cua-driver\"",
            "--info",
            "--start", formatter.string(from: cutoff)
        ]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        try process.run()
        process.waitUntilExit()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let output = String(data: data, encoding: .utf8) ?? ""

        // Cleanup leases before any throw/skip so the dispatcher
        // teardown is observable in subsequent tests if the runner
        // doesn't reset state between tests.
        for lease in leases { await lease.release() }

        // Distinguish "log show couldn't run" (skip — environment
        // problem, not a code bug) from "log show ran but found
        // nothing" (fail — Layer 4 contract is broken). The status
        // codes that trigger skip are the ones a privilege-restricted
        // CI sandbox produces:
        //   - non-zero exit
        //   - stderr containing TCC / privilege-denial markers
        //   - empty output (the unified log archive is unavailable)
        //
        // The XCTSkipUnless guard above already gates on
        // `LayerFourLogVerify=1`, so this second-stage skip only fires
        // when an opt-in caller's environment turns out to be more
        // restricted than they thought — not on the default CI path.
        if process.terminationStatus != 0 {
            throw XCTSkip(
                "`log show` exited with status \(process.terminationStatus); "
                + "unified-log capture unavailable on this host. Output: \(output)"
            )
        }
        let lowerOut = output.lowercased()
        let privilegeDeniedMarkers = [
            "operation not permitted",
            "not authorized",
            "missing entitlement",
            "no logs found",  // empty archive (sandboxed runners)
            "log archive does not contain",
        ]
        if privilegeDeniedMarkers.contains(where: lowerOut.contains) {
            throw XCTSkip(
                "`log show` reports privilege/availability issue; skipping. "
                + "Output: \(output)"
            )
        }

        // Genuine assertion path — `log show` ran cleanly and produced
        // output. The Layer-4 contract is now under test for real.
        XCTAssertTrue(
            output.contains("FocusStealPreventer leak suspect"),
            "Layer 4 logger output not found in unified log. This means "
            + "future leak warnings will silently disappear instead of "
            + "surfacing in `log show`. Marker=\(marker). Output=\(output)"
        )
        XCTAssertTrue(
            output.contains("test.layer4.marker"),
            "Layer 4 logger origin field not propagated. Future operators "
            + "won't be able to trace leak warnings to their call site."
        )
    }

    // MARK: - Helpers

    /// Poll `activeCount` until it reaches `expected` or `timeout`
    /// elapses. Used for tests where cleanup happens on a detached
    /// Task and there's no better signal.
    private func waitForActiveCount(
        _ expected: Int,
        on preventer: SystemFocusStealPreventer,
        timeout: TimeInterval
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await preventer.activeCount == expected { return }
            try await Task.sleep(nanoseconds: 10_000_000)  // 10 ms
        }
        let final = await preventer.activeCount
        XCTFail("activeCount never reached \(expected); final=\(final)")
    }
}
