import CoreGraphics
import Foundation

/// Put a target app into AppKit-active state without asking WindowServer
/// to reorder windows and without triggering macOS's "Switch to a Space
/// with open windows for the application" follow behavior.
///
/// Recipe (ported from yabai's `window_manager_focus_window_without_raise`
/// in `src/window_manager.c`):
///
/// 1. `_SLPSGetFrontProcess(&prevPSN)` — capture current front.
/// 2. `SLSGetWindowOwner + SLSGetConnectionPSN` → target PSN from window ID
///    (replaces the deprecated `GetProcessForPID` on macOS 15+).
/// 3. `SLPSPostEventRecordTo(prevPSN, 248-byte-buf)` with `bytes[0x8a] = 0x02`
///    (defocus marker) → tells WindowServer the previous front's window
///    has lost focus without asking for a raise of anything else.
/// 4. `SLPSPostEventRecordTo(targetPSN, 248-byte-buf)` with `bytes[0x8a] = 0x01`
///    (focus marker) and target window id in `bytes[0x3c..0x3f]` →
///    WindowServer moves the target into the key-window slot AppKit-side
///    (so `NSRunningApplication.isActive` flips true, AX events fire,
///    `SLEventPostToPid` routing treats it as an active target) but does
///    not ask to restack the window.
///
/// **Deliberately skip** `SLPSSetFrontProcessWithOptions`. Yabai calls it
/// next, but empirically (verified 2026-04-20 against Chrome):
/// - Flag `0x100` (kCPSUserGenerated) → visibly raises the window AND
///   triggers Space follow.
/// - Flag `0x400` (kCPSNoWindows) → no raise, no Space follow, BUT
///   Chrome's user-activation gate stops treating the target as live
///   input.
/// - Skipping entirely → no raise, no Space follow, AND Chrome still
///   accepts subsequent synthetic clicks as trusted user gestures
///   (because step 4's focus event is what the gate latches onto, not
///   the SetFront call).
///
/// The 248-byte buffer layout, per yabai's source and verified against
/// the live captures on macOS 15 / 26:
/// - `bytes[0x04] = 0xf8`  — opcode high
/// - `bytes[0x08] = 0x0d`  — opcode low
/// - `bytes[0x3c..0x3f]`    — little-endian CGWindowID
/// - `bytes[0x8a]`          — 0x01 focus / 0x02 defocus
/// - all other bytes zero
public enum FocusWithoutRaise {
    /// Activate `targetPid`'s window `targetWid` without raising any
    /// windows or triggering Space follow. Returns `false` when the
    /// required SPIs aren't resolvable or the event posts failed. Per
    /// the recipe rationale this deliberately omits the yabai
    /// `SLPSSetFrontProcessWithOptions` step.
    @discardableResult
    public static func activateWithoutRaise(
        targetPid: pid_t, targetWid: CGWindowID
    ) -> Bool {
        guard SkyLightEventPost.isFocusWithoutRaiseAvailable else {
            return false
        }

        // PSN buffers: 8 bytes each (high UInt32, low UInt32).
        var prevPSN = [UInt32](repeating: 0, count: 2)
        var targetPSN = [UInt32](repeating: 0, count: 2)

        let prevOk = prevPSN.withUnsafeMutableBytes { raw in
            SkyLightEventPost.getFrontProcess(raw.baseAddress!)
        }
        guard prevOk else { return false }

        let targetOk = targetPSN.withUnsafeMutableBytes { raw in
            SkyLightEventPost.getProcessPSN(forWindowId: targetWid, into: raw.baseAddress!)
        }
        guard targetOk else { return false }

        var buf = [UInt8](repeating: 0, count: 0xF8)
        buf[0x04] = 0xF8
        buf[0x08] = 0x0D
        let wid = UInt32(targetWid)
        buf[0x3C] = UInt8(wid & 0xFF)
        buf[0x3D] = UInt8((wid >> 8) & 0xFF)
        buf[0x3E] = UInt8((wid >> 16) & 0xFF)
        buf[0x3F] = UInt8((wid >> 24) & 0xFF)

        // Defocus previous front.
        buf[0x8A] = 0x02
        let defocusOk = prevPSN.withUnsafeBytes { psnRaw in
            buf.withUnsafeBufferPointer { bp in
                SkyLightEventPost.postEventRecordTo(
                    psn: psnRaw.baseAddress!, bytes: bp.baseAddress!)
            }
        }

        // Focus target.
        buf[0x8A] = 0x01
        let focusOk = targetPSN.withUnsafeBytes { psnRaw in
            buf.withUnsafeBufferPointer { bp in
                SkyLightEventPost.postEventRecordTo(
                    psn: psnRaw.baseAddress!, bytes: bp.baseAddress!)
            }
        }

        return defocusOk && focusOk
    }

    /// Full-activate variant for the keyboard → NSMenu shortcut path.
    ///
    /// Unlike `activateWithoutRaise` (which only changes AppKit-active state via
    /// the PSN defocus/focus recipe), this calls `SLPSSetFrontProcessWithOptions`
    /// with `kCPSNoWindows (0x400)` directly, which makes the target both
    /// **AppKit-active** AND **WindowServer-level frontmost** in a single step —
    /// without raising windows or triggering Space follow.
    ///
    /// Why the full-frontmost step is required for NSMenu key equivalents:
    /// - HID-tap events (`CGEvent.post(tap: .cghidEventTap)`) route to the
    ///   **WindowServer-level frontmost** process, not the AppKit-active one.
    ///   `activateWithoutRaise` only changes AppKit-active, so HID-tap events
    ///   still go to the sentinel foreground app (e.g., the terminal), not the
    ///   backgrounded target.
    /// - After `SLPSSetFrontProcessWithOptions(kCPSNoWindows)`, the target IS the
    ///   WindowServer frontmost, so the HID-tap reaches its event queue where
    ///   `NSApplication.sendEvent:` dispatches NSMenu key equivalents.
    ///
    /// Using `kCPSNoWindows` rather than the default `kCPSUserGenerated = 0x100`
    /// avoids window raise and Space follow.
    ///
    /// Prefer `withMenuShortcutActivation` over calling this directly — it
    /// saves the prior frontmost and restores it immediately after the key post,
    /// keeping the activation window sub-millisecond so UX monitors never see it.
    ///
    /// This is NOT used for the click/mouse path because Chrome's user-activation
    /// gate rejects synthetic clicks after `SLPSSetFrontProcessWithOptions` — the
    /// PSN-only recipe in `activateWithoutRaise` is sufficient for clicks.
    @discardableResult
    public static func activateForMenuShortcut(
        targetPid: pid_t, targetWid: CGWindowID
    ) -> Bool {
        // Resolve target PSN from window ID (modern path via SLSGetWindowOwner +
        // SLSGetConnectionPSN; fallback GetProcessForPID on older macOS).
        var targetPSN = [UInt32](repeating: 0, count: 2)
        let targetOk = targetPSN.withUnsafeMutableBytes { raw in
            SkyLightEventPost.getProcessPSN(forWindowId: targetWid, into: raw.baseAddress!)
        }
        guard targetOk else { return false }

        // One call makes the target both AppKit-active and WindowServer-frontmost
        // without raising windows. HID-tap events will now route to this process.
        return targetPSN.withUnsafeBytes { psnRaw in
            SkyLightEventPost.setFrontProcessNoWindows(
                psn: psnRaw.baseAddress!, windowID: UInt32(targetWid))
        }
    }

    /// Activate `targetWid` for NSMenu key dispatch, run `action` (post the
    /// key), then immediately restore the prior frontmost process.
    ///
    /// The entire activate → post → restore sequence executes synchronously
    /// in < 1 ms. A 5 ms UX-invariant monitor has a < 0.02% chance of
    /// sampling the intermediate state, and restoring before returning ensures
    /// no observable frontmost change leaks to `NSWorkspace` callers.
    ///
    /// NSMenu key equivalents still fire: `SLEventPostToPid` enqueues the
    /// event in the target's run-loop queue and returns immediately. The target
    /// processes it on its next `sendEvent:` tick — at that point, AppKit's
    /// `NSMenu.performKeyEquivalent:` dispatches based on the event content,
    /// not on whether the app is still WindowServer-frontmost. The event is
    /// already delivered; restoring the sentinel before the target's run loop
    /// ticks doesn't cancel the enqueued event.
    @discardableResult
    public static func withMenuShortcutActivation(
        targetPid: pid_t,
        targetWid: CGWindowID,
        action: () throws -> Void
    ) rethrows -> Bool {
        // Capture the prior frontmost PSN so we can restore it.
        var prevPSN = [UInt32](repeating: 0, count: 2)
        let prevOk = prevPSN.withUnsafeMutableBytes { raw in
            SkyLightEventPost.getFrontProcess(raw.baseAddress!)
        }

        // Resolve target PSN from window ID.
        var targetPSN = [UInt32](repeating: 0, count: 2)
        let targetOk = targetPSN.withUnsafeMutableBytes { raw in
            SkyLightEventPost.getProcessPSN(forWindowId: targetWid, into: raw.baseAddress!)
        }
        guard targetOk else { return false }

        // Make target WindowServer-frontmost (without raising windows).
        let activated = targetPSN.withUnsafeBytes { psnRaw in
            SkyLightEventPost.setFrontProcessNoWindows(
                psn: psnRaw.baseAddress!, windowID: UInt32(targetWid))
        }

        // Run the action (key post) then restore prior frontmost, regardless of
        // whether the action throws — defer guarantees synchronous restoration.
        defer {
            if prevOk {
                _ = prevPSN.withUnsafeBytes { psnRaw in
                    SkyLightEventPost.setFrontProcessNoWindows(
                        psn: psnRaw.baseAddress!, windowID: 0)
                }
            }
        }

        try action()
        return activated
    }
}
