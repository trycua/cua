import ApplicationServices
import Foundation

/// A best-effort snapshot of the boolean focus attributes that a
/// ``SyntheticAppFocusEnforcer`` may have overwritten. The enforcer keeps
/// this opaque; call sites just pass the returned value back to
/// ``SyntheticAppFocusEnforcer/reenableActivation(_:)`` to restore.
public struct FocusState: Sendable {
    fileprivate let pid: pid_t
    fileprivate let window: AXUIElement?
    fileprivate let element: AXUIElement?
    fileprivate let priorWindowFocused: Bool?
    fileprivate let priorWindowMain: Bool?
    fileprivate let priorElementFocused: Bool?
}

/// Writes the AX boolean focus attributes (`AXFocused`, `AXMain`) on the
/// target window and element of a backgrounded app *just before* we
/// dispatch an AX action, then restores them after. The goal is to keep
/// the target's internal AppKit state machine believing "I have focus,
/// proceed normally" without calling `NSRunningApplication.activate(...)`
/// or `AXUIElementPerformAction(kAXRaiseAction)` — both of which would
/// flip the target to the system-wide frontmost app and steal focus from
/// whatever the user was doing.
///
/// This is deliberately best-effort: if a specific attribute write returns
/// anything other than `.success` we move on. The priority is that the
/// primary AX action lands; perfect focus fidelity on the target is a
/// second-order concern. No logging either — these writes routinely fail
/// on elements that don't support AXFocused (labels, static text, etc.)
/// and filling logs with noise helps nobody.
public actor SyntheticAppFocusEnforcer {
    public init() {}

    /// Read prior values of the relevant focus attributes and then write
    /// `true` to each. Returns a ``FocusState`` that
    /// ``reenableActivation(_:)`` uses to restore the originals.
    ///
    /// The `window` and `element` arguments are both optional: the caller
    /// may not always be able to resolve the enclosing window (especially
    /// for application-root elements) but can still synthesize focus on
    /// whatever it does have.
    public func preventActivation(
        pid: pid_t,
        window: AXUIElement?,
        element: AXUIElement?
    ) async -> FocusState {
        let priorWindowFocused = window.flatMap { readBool($0, "AXFocused") }
        let priorWindowMain = window.flatMap { readBool($0, "AXMain") }
        let priorElementFocused = element.flatMap { readBool($0, "AXFocused") }

        if let window {
            writeBool(window, "AXFocused", true)
            writeBool(window, "AXMain", true)
        }
        if let element {
            writeBool(element, "AXFocused", true)
        }

        return FocusState(
            pid: pid,
            window: window,
            element: element,
            priorWindowFocused: priorWindowFocused,
            priorWindowMain: priorWindowMain,
            priorElementFocused: priorElementFocused
        )
    }

    /// Restore whatever prior state ``preventActivation(pid:window:element:)``
    /// captured. Attributes we couldn't read on the way in are left alone on
    /// the way out — writing a bogus "false" would be worse than leaving the
    /// "true" we installed.
    public func reenableActivation(_ state: FocusState) async {
        if let window = state.window {
            if let prior = state.priorWindowFocused {
                writeBool(window, "AXFocused", prior)
            }
            if let prior = state.priorWindowMain {
                writeBool(window, "AXMain", prior)
            }
        }
        if let element = state.element, let prior = state.priorElementFocused {
            writeBool(element, "AXFocused", prior)
        }
    }

    // MARK: - CF helpers

    private func readBool(_ element: AXUIElement, _ attribute: String) -> Bool? {
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

    private func writeBool(_ element: AXUIElement, _ attribute: String, _ value: Bool) {
        _ = AXUIElementSetAttributeValue(
            element,
            attribute as CFString,
            (value ? kCFBooleanTrue : kCFBooleanFalse) as CFTypeRef
        )
    }
}
