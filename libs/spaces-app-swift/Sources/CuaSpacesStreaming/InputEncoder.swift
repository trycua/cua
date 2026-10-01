// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
#endif
import CoreGraphics
import Foundation

/// Translates AppKit events into RCDP `interactive_input` events.
///
/// Two rules here are not style choices; both were paid for in debugging time.
///
/// **1. One scale.** Every coordinate comes from `StreamGeometry.normalized(for:)`
/// and nothing else. The encoder has no access to view bounds or to the surface
/// size independently, so it cannot invent a second mapping.
///
/// **2. No off-screen primer.** Some remote-input stacks send a throwaway
/// pointer event at a sentinel position — `(-1, -1)` is the usual one — to
/// "prime" the target before the real click. Do not. Blender's GHOST layer (and
/// it is not alone) resolves a click at *the pointer position it last saw*, not
/// at the coordinate the click event carries. A primer at `(-1, -1)` therefore
/// makes the next click land off-window, or on whatever the clamp produces. The
/// encoder never synthesizes a position the user did not point at, and
/// `makePointer` returns `nil` rather than substituting one; RCDP's own
/// validator would reject `(-1, -1)` anyway, so a sentinel cannot even be sent
/// — it would just silently drop the batch it rode in on.
///
/// Where a target genuinely needs the pointer to be *at* the click position
/// before the button goes down, the correct primer is a `move` **to the same
/// coordinate as the click**, which `pointerDown(at:)` emits. That is a real
/// position the user is pointing at, so it is safe for GHOST-style apps and a
/// no-op for everything else.
public struct InputEncoder {
    public var geometry: StreamGeometry

    public init(geometry: StreamGeometry) {
        self.geometry = geometry
    }

    // MARK: - Pointer

    /// A pointer move. `nil` when the point is outside the drawn frame.
    public func pointerMove(at viewPoint: CGPoint, modifiers: [InputModifier] = []) -> InteractiveInputEvent? {
        guard let normalized = geometry.normalized(for: viewPoint) else { return nil }
        return .pointer(phase: .move, button: nil, x: normalized.x, y: normalized.y, modifiers: modifiers)
    }

    /// Button press, preceded by a move **to the identical coordinate**.
    ///
    /// The paired move is what makes clicks land correctly in apps that resolve
    /// a click against the last pointer position they observed. It is emitted at
    /// the click's own position, never at a sentinel.
    public func pointerDown(at viewPoint: CGPoint, button: PointerButton,
                     modifiers: [InputModifier] = []) -> [InteractiveInputEvent] {
        guard let normalized = geometry.normalized(for: viewPoint) else { return [] }
        return [
            .pointer(phase: .move, button: nil, x: normalized.x, y: normalized.y, modifiers: modifiers),
            .pointer(phase: .down, button: button, x: normalized.x, y: normalized.y, modifiers: modifiers),
        ]
    }

    public func pointerUp(at viewPoint: CGPoint, button: PointerButton,
                   modifiers: [InputModifier] = []) -> InteractiveInputEvent? {
        guard let normalized = geometry.normalized(for: viewPoint) else { return nil }
        return .pointer(phase: .up, button: button, x: normalized.x, y: normalized.y, modifiers: modifiers)
    }

    /// A drag sample. Identical to a move on the wire; the button state is held
    /// by the preceding `down`, which is why a drag must never be interrupted by
    /// an out-of-frame sample silently turning into nothing — the caller clamps
    /// its *own* tracking, see `LiveStreamInputView`.
    public func drag(to viewPoint: CGPoint, modifiers: [InputModifier] = []) -> InteractiveInputEvent? {
        pointerMove(at: viewPoint, modifiers: modifiers)
    }

    public func scroll(at viewPoint: CGPoint, deltaX: Double, deltaY: Double,
                phase: GesturePhase, momentum: GesturePhase, precise: Bool,
                modifiers: [InputModifier] = []) -> InteractiveInputEvent? {
        guard let normalized = geometry.normalized(for: viewPoint) else { return nil }
        return .scroll(x: normalized.x, y: normalized.y, deltaX: deltaX, deltaY: deltaY,
                       phase: phase, momentum: momentum, precise: precise)
    }

    /// The overlay position for a pointer coordinate, obtained by round-tripping
    /// through the same mapping the wire coordinate used. Callers draw here.
    public func overlayPoint(for viewPoint: CGPoint) -> CGPoint? {
        guard let normalized = geometry.normalized(for: viewPoint) else { return nil }
        return geometry.point(forNormalized: normalized)
    }

    // MARK: - Keyboard

    #if canImport(AppKit)
    public static func modifiers(from flags: NSEvent.ModifierFlags) -> [InputModifier] {
        var result: [InputModifier] = []
        if flags.contains(.command) { result.append(.command) }
        if flags.contains(.shift) { result.append(.shift) }
        if flags.contains(.option) { result.append(.option) }
        if flags.contains(.control) { result.append(.control) }
        if flags.contains(.function) { result.append(.function) }
        return result
    }

    public static func button(for event: NSEvent) -> PointerButton {
        switch event.buttonNumber {
        case 1: return .right
        case 2: return .middle
        default: return .left
        }
    }

    /// The events for one key press or release: a key, or committed text.
    ///
    /// Printable input goes as `text_commit` **only** (on the press; the
    /// release sends nothing), so that dead keys, IME composition and
    /// option-modified characters arrive as the characters the user actually
    /// produced rather than as a guess reconstructed from a key code. Sending
    /// the key as well typed every character twice. Keys with no text
    /// (arrows, return, function keys) and anything held with Command or
    /// Control go as `key` events, because a chord is a shortcut, not text.
    /// The same split as the HTML5 viewer's `keyEventToInput`.
    public func keyEvents(for event: NSEvent, down: Bool) -> [InteractiveInputEvent] {
        Self.keyEvents(name: Self.keyName(for: event), characters: event.characters,
                       modifiers: Self.modifiers(from: event.modifierFlags),
                       down: down, isRepeat: event.isARepeat)
    }

    /// `keyEvents(for:down:)` over the event's parts, for tests.
    public static func keyEvents(name: String, characters: String?, modifiers: [InputModifier],
                                 down: Bool, isRepeat: Bool) -> [InteractiveInputEvent] {
        let chord = modifiers.contains(.command) || modifiers.contains(.control)
        if !chord, let characters, !characters.isEmpty, !isNonPrintable(characters) {
            return down ? [.textCommit(characters)] : []
        }
        return [.key(key: name, down: down, modifiers: modifiers, repeatKey: down && isRepeat)]
    }

    /// Key names follow the reference client's vocabulary so the host provider
    /// resolves them identically.
    public static func keyName(for event: NSEvent) -> String {
        switch event.keyCode {
        case 36, 76: return "enter"
        case 51: return "backspace"
        case 117: return "delete"
        case 48: return "tab"
        case 53: return "escape"
        case 126: return "up"
        case 125: return "down"
        case 123: return "left"
        case 124: return "right"
        case 115: return "home"
        case 119: return "end"
        case 116: return "pageup"
        case 121: return "pagedown"
        case 49: return "space"
        default:
            let characters = event.charactersIgnoringModifiers ?? ""
            return characters.isEmpty ? "unknown" : characters.lowercased()
        }
    }

    public static func isNonPrintable(_ characters: String) -> Bool {
        guard let scalar = characters.unicodeScalars.first else { return true }
        // Control characters and the private-use plane AppKit uses for function
        // keys and arrows.
        return scalar.value < 0x20 || scalar.value == 0x7F || (0xF700 ... 0xF8FF).contains(scalar.value)
    }
    #endif
}
