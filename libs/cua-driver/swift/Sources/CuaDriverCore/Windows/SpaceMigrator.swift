import CoreGraphics
import Darwin
import Foundation

/// Detect whether a pid's primary window is on the user's current Space.
///
/// **Why this matters.** macOS's AX subsystem silently strips the UI
/// hierarchy of windows that live on a Space other than the one the
/// user is currently viewing — a SwiftUI System Settings window on
/// another Space snapshots as menu-bar-only (13 elements) even though
/// `SCShareableContent` still surfaces the backing store. Without this
/// signal, callers have no idea why their `get_window_state` came back
/// empty and blame our driver for the gap.
///
/// **Why this is a detector, not a migrator.** We originally tried to
/// pull off-Space windows back with `CGSMoveWindowsToManagedSpace` /
/// `SLSSpaceAddWindowsAndRemoveFromSpaces` etc. On macOS 14+ (verified
/// against Tahoe 26.4 in 2026-04) every variant of the SPI returns
/// status 0 but is a silent no-op for non-WindowServer clients —
/// WindowServer gates cross-Space window moves behind an entitlement
/// none of our callers hold. yabai hits the same wall and requires
/// SIP-off to work. We surface the detection instead and let the
/// caller recover (user switches Space, target activation, etc.)
/// rather than pretending we can fix it.
public enum SpaceMigrator {
    private typealias MainConnectionFn = @convention(c) () -> UInt32
    private typealias GetActiveSpaceFn = @convention(c) (Int32) -> UInt64
    private typealias CopySpacesForWindowsFn = @convention(c) (
        Int32, Int32, CFArray
    ) -> CFArray?

    private struct Resolved {
        let main: MainConnectionFn
        let getActiveSpace: GetActiveSpaceFn
        let copySpacesForWindows: CopySpacesForWindowsFn
    }

    private static let resolved: Resolved? = {
        _ = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_LAZY)
        let rtldDefault = UnsafeMutableRawPointer(bitPattern: -2)
        guard
            let mainP = dlsym(rtldDefault, "SLSMainConnectionID"),
            let activeP = dlsym(rtldDefault, "SLSGetActiveSpace"),
            let copyP = dlsym(rtldDefault, "SLSCopySpacesForWindows")
        else { return nil }
        return Resolved(
            main: unsafeBitCast(mainP, to: MainConnectionFn.self),
            getActiveSpace: unsafeBitCast(activeP, to: GetActiveSpaceFn.self),
            copySpacesForWindows: unsafeBitCast(
                copyP, to: CopySpacesForWindowsFn.self)
        )
    }()

    /// Off-Space detection report. `.unknown` when the SkyLight SPIs
    /// didn't resolve (old OS, sandboxed environment) or the pid has
    /// no layer-0 window at all (menubar-only helpers, just-launched).
    public enum SpaceStatus: Sendable, Equatable {
        case onCurrentSpace
        case onAnotherSpace(currentSpaceID: UInt64, windowSpaceIDs: [UInt64])
        case unknown
    }

    /// User's current active Space id on the primary display, or nil
    /// when the SkyLight SPIs didn't resolve. Cheap (~one SPI hop) —
    /// callers that need per-window comparisons should cache this once
    /// per batch rather than re-calling it N times.
    public static func currentSpaceID() -> UInt64? {
        guard let r = resolved else { return nil }
        return r.getActiveSpace(Int32(bitPattern: r.main()))
    }

    /// Every managed Space id the window is a member of, or nil when
    /// the SkyLight SPIs didn't resolve. A window bound to the user's
    /// current Space has `currentSpaceID()` in the result; one on
    /// another Space has only the other Space's id.
    ///
    /// Per-window attribution — `SLSCopySpacesForWindows` returns the
    /// UNION of Spaces across its input array, so a batch call can't
    /// tell you which Space any individual window is on. Call this
    /// per window and let the caller aggregate.
    public static func spaceIDs(forWindowID windowID: UInt32) -> [UInt64]? {
        guard let r = resolved else { return nil }
        let cid = Int32(bitPattern: r.main())
        let widArray = [NSNumber(value: windowID)] as CFArray
        // Mask 7 (user+current+others) is the most permissive — returns
        // every managed Space the window is a member of.
        guard
            let raw = r.copySpacesForWindows(cid, 7, widArray) as? [NSNumber]
        else { return nil }
        return raw.map { $0.uint64Value }
    }

    /// Inspect the pid's largest layer-0 top-level window and report
    /// whether it lives on the user's current Space. Matches the
    /// window selector `WindowCapture.captureFrontmostWindow` uses —
    /// the "main" window by max-area so the signal matches what our
    /// screenshot captured and the caller reasons about.
    public static func status(forPid pid: pid_t) -> SpaceStatus {
        guard let active = currentSpaceID() else { return .unknown }

        guard
            let all = CGWindowListCopyWindowInfo(
                [.optionAll, .excludeDesktopElements], kCGNullWindowID
            ) as? [[String: Any]]
        else { return .unknown }

        let candidates = all.compactMap {
            entry -> (id: UInt32, area: Double)? in
            guard
                let ownerPid = entry[kCGWindowOwnerPID as String] as? Int,
                Int32(ownerPid) == pid,
                let layer = entry[kCGWindowLayer as String] as? Int,
                layer == 0,
                let number = entry[kCGWindowNumber as String] as? Int,
                let bounds = entry[kCGWindowBounds as String]
                    as? [String: Double],
                let width = bounds["Width"], let height = bounds["Height"],
                width > 1, height > 1
            else { return nil }
            return (UInt32(number), width * height)
        }
        guard let primary = candidates.max(by: { $0.area < $1.area })
        else { return .unknown }

        guard let spaces = spaceIDs(forWindowID: primary.id)
        else { return .unknown }
        if spaces.contains(active) { return .onCurrentSpace }
        return .onAnotherSpace(
            currentSpaceID: active, windowSpaceIDs: spaces)
    }
}
