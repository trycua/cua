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
/// 2. `GetProcessForPID(targetPid, &targetPSN)`.
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
            SkyLightEventPost.getProcessPSN(forPid: targetPid, into: raw.baseAddress!)
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
}
