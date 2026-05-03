import AppKit
import ApplicationServices
import Foundation

/// Wires together ``AXEnablementAssertion`` and
/// ``SyntheticAppFocusEnforcer`` into a single ``withFocusSuppressed``
/// entry point that call sites wrap around AX actions on backgrounded apps.
///
/// Two-layer stack:
///
/// 1. **Enablement** — set `AXManualAccessibility` /
///    `AXEnhancedUserInterface` on the application root. Needed before
///    Chromium/Electron targets will even respond to attribute-level
///    action dispatch. No-op on native Cocoa apps.
/// 2. **Synthetic focus** — write `AXFocused` / `AXMain` on the target's
///    enclosing window and `AXFocused` on the element itself before the
///    action, restore on the way out. Makes AppKit's internal state
///    machine behave as if the action came from the focused process,
///    which prevents the common "target app flashes to the front because
///    AXPress tripped a reflex activation" failure mode.
///
/// A third layer (intercepting the resulting focus-return events on the
/// session event tap) is intentionally not implemented here — it needs
/// private kCPS SPIs we don't want to take a dependency on yet. The two
/// layers present are sufficient for the common case of driving
/// backgrounded AppKit apps.
public actor FocusGuard {
    private let enablement: AXEnablementAssertion
    private let enforcer: SyntheticAppFocusEnforcer
    private let systemPreventer: SystemFocusStealPreventer?

    public init(
        enablement: AXEnablementAssertion,
        enforcer: SyntheticAppFocusEnforcer,
        systemPreventer: SystemFocusStealPreventer? = nil
    ) {
        self.enablement = enablement
        self.enforcer = enforcer
        self.systemPreventer = systemPreventer
    }

    /// Run `body` with focus-suppression active for `pid`. `element`, when
    /// provided, is the AX node the action targets; the guard resolves its
    /// enclosing window via `kAXWindowAttribute` and writes synthetic focus
    /// on both. When `element` is `nil` (e.g. an application-root action)
    /// only the enablement layer applies.
    ///
    /// The enforcer state is always restored, even if `body` throws.
    public func withFocusSuppressed<T: Sendable>(
        pid: pid_t,
        element: AXUIElement?,
        body: @Sendable () async throws -> T
    ) async throws -> T {
        // Layer 1 — enablement. Best-effort; on native-AX apps this is a
        // cached no-op after the first call.
        let root = AXUIElementCreateApplication(pid)
        _ = await enablement.assert(pid: pid, root: root)

        // Layer 2 — synthetic focus. Walk up to the enclosing window so we
        // can mark it focused+main alongside the element itself. The window
        // lookup is best-effort: some AX trees omit kAXWindow on deeply
        // nested elements, and some app-root actions don't have a window
        // at all. In those cases the enforcer still tries whatever it has.
        //
        // SKIP when the window is minimized — writing AXFocused=true /
        // AXMain=true on a minimized window triggers Chrome (and likely
        // other apps) to deminiaturize. Without synthetic focus, the bare
        // AX action still works on the minimized AX tree and Chrome
        // stays in the Dock.
        let window = element.flatMap { enclosingWindow(of: $0) }
        let windowIsMinimized = window.flatMap { readBool($0, "AXMinimized") } ?? false
        let focusState: FocusState?
        if windowIsMinimized {
            focusState = nil
        } else {
            focusState = await enforcer.preventActivation(
                pid: pid, window: window, element: element
            )
        }

        // Layer 3 — reactive: arm the SystemFocusStealPreventer around
        // the AX action. If the target (e.g. Safari/WebKit) self-activates
        // in response to AXPress despite layers 1+2, this catches the
        // activation notification and immediately re-activates the prior
        // frontmost app. Only armed when the target isn't already
        // frontmost (no point suppressing self → self).
        var suppressionHandle: SuppressionHandle?
        if let preventer = systemPreventer {
            let targetApp = NSRunningApplication(processIdentifier: pid)
            let isTargetFrontmost = targetApp?.isActive ?? false
            if !isTargetFrontmost,
               let frontmost = NSWorkspace.shared.frontmostApplication
            {
                suppressionHandle = await preventer.beginSuppression(
                    targetPid: pid, restoreTo: frontmost
                )
            }
        }

        do {
            let result = try await body()
            if let focusState { await enforcer.reenableActivation(focusState) }
            if let handle = suppressionHandle {
                try? await Task.sleep(nanoseconds: 50_000_000) // 50ms
                await systemPreventer?.endSuppression(handle)
            }
            return result
        } catch {
            if let focusState { await enforcer.reenableActivation(focusState) }
            if let handle = suppressionHandle {
                await systemPreventer?.endSuppression(handle)
            }
            throw error
        }
    }

    // MARK: - Helpers

}

public enum FocusGuardError: Error, CustomStringConvertible, Sendable {
    case windowMinimized(pid: pid_t)

    public var description: String {
        switch self {
        case .windowMinimized(let pid):
            return "Target window for pid \(pid) is minimized. AX actions on minimized "
                + "windows (especially Chrome) cause unavoidable deminiaturization. "
                + "Use type_text_chars/press_key for keyboard input (these don't "
                + "deminiaturize), or accept the deminiaturize by unminimizing first."
        }
    }
}

private extension FocusGuard {
    func readBool(_ element: AXUIElement, _ attribute: String) -> Bool? {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            element, attribute as CFString, &value
        )
        guard result == .success, let v = value else { return nil }
        if CFGetTypeID(v) == CFBooleanGetTypeID() {
            return CFBooleanGetValue((v as! CFBoolean))
        }
        return nil
    }

    private func enclosingWindow(of element: AXUIElement) -> AXUIElement? {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            element, "AXWindow" as CFString, &value
        )
        guard result == .success, let raw = value else { return nil }
        guard CFGetTypeID(raw) == AXUIElementGetTypeID() else { return nil }
        return unsafeBitCast(raw, to: AXUIElement.self)
    }
}
