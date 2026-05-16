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
