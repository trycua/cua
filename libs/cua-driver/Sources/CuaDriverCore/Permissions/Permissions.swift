import ApplicationServices
import CoreGraphics
import ScreenCaptureKit

public struct PermissionsStatus: Sendable, Codable, Hashable {
    public let accessibility: Bool
    public let screenRecording: Bool

    public init(accessibility: Bool, screenRecording: Bool) {
        self.accessibility = accessibility
        self.screenRecording = screenRecording
    }

    private enum CodingKeys: String, CodingKey {
        case accessibility
        case screenRecording = "screen_recording"
    }
}

public enum Permissions {
    /// Accurate TCC status for both grants.
    ///
    /// Accessibility uses `AXIsProcessTrusted()` — reliable.
    ///
    /// Screen Recording does a real probe via `SCShareableContent.current`
    /// rather than `CGPreflightScreenCaptureAccess()` — the latter returns
    /// false negatives for subprocess-launched apps, even when the grant is
    /// active. The probe costs ~100-300ms but returns the truth.
    public static func currentStatus() async -> PermissionsStatus {
        async let screen = probeScreenRecording()
        return await PermissionsStatus(
            accessibility: AXIsProcessTrusted(),
            screenRecording: screen
        )
    }

    /// Ask macOS to raise the Accessibility TCC prompt if not yet granted.
    /// Returns the post-prompt trust state (may still be false if the user
    /// dismissed the prompt).
    @discardableResult
    public static func requestAccessibility() -> Bool {
        // Swift 6 treats the exported CFString `kAXTrustedCheckOptionPrompt`
        // as non-Sendable (it's declared as a C `var`). The string literal
        // "AXTrustedCheckOptionPrompt" is the documented, stable key.
        let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    /// Ask macOS to raise the Screen Recording TCC prompt if not yet granted.
    @discardableResult
    public static func requestScreenRecording() -> Bool {
        CGRequestScreenCaptureAccess()
    }

    private static func probeScreenRecording() async -> Bool {
        do {
            // excludingDesktopWindows is much faster than .current when the
            // window server has many off-screen entries (e.g. a crashed
            // CursorUIViewService with thousands of ghost windows).
            _ = try await SCShareableContent.excludingDesktopWindows(
                false, onScreenWindowsOnly: true)
            return true
        } catch {
            return false
        }
    }
}
