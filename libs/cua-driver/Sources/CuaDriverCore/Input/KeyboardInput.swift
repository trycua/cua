import CoreGraphics
import Foundation

public enum KeyboardError: Error, CustomStringConvertible, Sendable {
    case unknownKey(String)
    case noKeyInCombo
    case eventCreationFailed(String)

    public var description: String {
        switch self {
        case .unknownKey(let k): return "Unknown key name: \(k)"
        case .noKeyInCombo: return "Hotkey combo has no non-modifier key."
        case .eventCreationFailed(let k): return "Failed to create key event for \(k)."
        }
    }
}

/// Keyboard synthesis via CGEvent.
///
/// Architectural note: the driver is AX-only for mouse (click, scroll) —
/// avoiding CGEvent mouse injection sidesteps focus-steal and makes clicks
/// hit the element the AX tree says is there. For keyboard, AX has no
/// general way to send arrow keys, tab, or modifier combinations, so
/// keyboard synthesis uses CGEvent. This isn't a compromise — it's the
/// same path every macOS app uses for programmatic keyboard input, and
/// the focus-steal concerns don't apply.
public enum KeyboardInput {
    /// Press and release one key, optionally with modifier keys held down.
    /// Default delivery is the system HID tap — the event routes to whatever
    /// application is frontmost. Pass `toPid` to deliver directly to a
    /// specific process's event queue via `CGEvent.postToPid`, bypassing
    /// frontmost-app routing. That variant is what browser-aware tools use
    /// to send keys to backgrounded Chromium/Safari without focus steal.
    public static func press(
        _ key: String,
        modifiers: [String] = [],
        toPid pid: pid_t? = nil
    ) throws {
        guard let code = virtualKeyCode(for: key) else {
            throw KeyboardError.unknownKey(key)
        }
        let flags = modifierMask(for: modifiers)
        try sendKey(code: code, down: true, flags: flags, toPid: pid)
        try sendKey(code: code, down: false, flags: flags, toPid: pid)
    }

    /// Press a combination of keys simultaneously.
    /// Any of cmd/command/shift/option/alt/ctrl/control/fn are treated as
    /// modifiers; the remaining keys are pressed with those modifiers held.
    /// If multiple non-modifier keys are supplied, only the last is pressed.
    /// Pass `toPid` to route to a specific process's event queue via
    /// `CGEvent.postToPid`, bypassing frontmost-app routing.
    public static func hotkey(_ keys: [String], toPid pid: pid_t? = nil) throws {
        var modifiers: [String] = []
        var finalKey: String?
        for raw in keys {
            if modifierNames.contains(raw.lowercased()) {
                modifiers.append(raw)
            } else {
                finalKey = raw
            }
        }
        guard let final = finalKey else {
            throw KeyboardError.noKeyInCombo
        }
        try press(final, modifiers: modifiers, toPid: pid)
    }

    /// Type each character in ``text`` as a synthetic key-down + key-up
    /// pair whose Unicode payload is set via
    /// ``CGEventKeyboardSetUnicodeString``. Bypasses virtual-key mapping
    /// entirely, so accents, symbols, and emoji all go through.
    ///
    /// Use this when the AX-based ``type_text`` path can't reach the
    /// focused field — most commonly Chromium/web views embedded in
    /// Electron apps, which do not expose ``kAXSelectedText`` on their
    /// text inputs.
    ///
    /// ``delayMilliseconds`` spaces successive characters so apps with
    /// per-keystroke handlers (autocomplete, IME composition) keep up.
    /// Default is small but non-zero for that reason.
    public static func typeCharacters(
        _ text: String,
        delayMilliseconds: Int = 30,
        toPid pid: pid_t? = nil
    ) throws {
        let clampedDelay = max(0, min(200, delayMilliseconds))
        for character in text {
            try sendUnicodeCharacter(character, toPid: pid)
            if clampedDelay > 0 {
                // usleep() takes microseconds; delayMilliseconds is ms —
                // convert with ×1_000, not ×1_000_000. Old code slept 1000x
                // too long (e.g. an 11-char string took 110s instead of 110ms).
                let microseconds = UInt32(clampedDelay) * 1_000
                usleep(microseconds)
            }
        }
    }

    // MARK: - Internals

    private static func sendKey(
        code: Int,
        down: Bool,
        flags: CGEventFlags,
        toPid pid: pid_t? = nil
    ) throws {
        guard
            let event = CGEvent(
                keyboardEventSource: nil,
                virtualKey: CGKeyCode(code),
                keyDown: down
            )
        else {
            throw KeyboardError.eventCreationFailed("code=\(code) down=\(down)")
        }
        event.flags = flags
        if let pid {
            // Prefer SkyLight's SLEventPostToPid — it routes through
            // CGSTickleActivityMonitor which Chromium omniboxes need to
            // promote the synthetic event into their keyboard pipeline.
            // Prefer SkyLight's SLEventPostToPid — it routes through
            // CGSTickleActivityMonitor which some apps need to promote
            // the synthetic event. Falls back to the public
            // CGEventPostToPid when the SPI is unavailable (older macOS
            // / stripped framework).
            if !SkyLightEventPost.postToPid(pid, event: event) {
                event.postToPid(pid)
            }
        } else {
            event.post(tap: .cghidEventTap)
        }
    }

    /// Post key-down + key-up events whose unicode payload is a single
    /// Swift ``Character`` (which may be one or more UTF-16 code units
    /// for combined characters and most emoji). The virtual-key code
    /// is left at 0 — macOS uses the attached unicode string instead.
    /// Pass `toPid` to deliver the events directly to a process's queue
    /// rather than the system HID tap.
    ///
    /// **Modifier-state isolation**: when posting via `postToPid` some
    /// apps (notably Chrome) inspect the CGEvent's `flags` field to infer
    /// whether Shift or other modifiers were held to produce the unicode
    /// character. If `flags` is non-zero the app may "remember" the
    /// modifier and apply it to the next character. We explicitly clear
    /// `event.flags` on every event so the receiver always sees a clean
    /// modifier state regardless of what was typed before.
    private static func sendUnicodeCharacter(
        _ character: Character, toPid pid: pid_t? = nil
    ) throws {
        let utf16 = Array(String(character).utf16)
        for keyDown in [true, false] {
            guard
                let event = CGEvent(
                    keyboardEventSource: nil,
                    virtualKey: 0,
                    keyDown: keyDown
                )
            else {
                throw KeyboardError.eventCreationFailed(
                    "unicode character \"\(character)\" down=\(keyDown)"
                )
            }
            // Clear all modifier flags so the receiver doesn't accumulate
            // Shift/Control/Option state between characters. Without this,
            // Chrome interprets uppercase unicode events as "Shift held" and
            // applies Shift to the next character (e.g. 'E' + '2' → '@').
            event.flags = []
            utf16.withUnsafeBufferPointer { buffer in
                if let base = buffer.baseAddress {
                    event.keyboardSetUnicodeString(
                        stringLength: buffer.count,
                        unicodeString: base
                    )
                }
            }
            if let pid {
                if !SkyLightEventPost.postToPid(pid, event: event) {
                    event.postToPid(pid)
                }
            } else {
                event.post(tap: .cghidEventTap)
            }
        }
    }

    private static let modifierNames: Set<String> = [
        "cmd", "command", "shift", "option", "alt", "ctrl", "control", "fn",
    ]

    private static func modifierMask(for modifiers: [String]) -> CGEventFlags {
        var mask: CGEventFlags = []
        for raw in modifiers {
            switch raw.lowercased() {
            case "cmd", "command": mask.insert(.maskCommand)
            case "shift": mask.insert(.maskShift)
            case "option", "alt": mask.insert(.maskAlternate)
            case "ctrl", "control": mask.insert(.maskControl)
            case "fn": mask.insert(.maskSecondaryFn)
            default: break
            }
        }
        return mask
    }

    private static func virtualKeyCode(for name: String) -> Int? {
        let lower = name.lowercased()
        if let named = namedKeys[lower] {
            return named
        }
        guard lower.count == 1, let first = lower.first else {
            return nil
        }
        if let code = letterKeys[first] { return code }
        if let code = digitKeys[first] { return code }
        return nil
    }

    // Virtual key codes from Carbon/HIToolbox/Events.h (kVK_* constants).
    // Hand-duplicated as plain ints so we don't need the Carbon import or
    // wrestle with Swift 6's Sendable rules on C-imported vars.
    private static let namedKeys: [String: Int] = [
        "return": 0x24, "enter": 0x24,
        "tab": 0x30,
        "space": 0x31,
        "delete": 0x33, "backspace": 0x33,
        "forwarddelete": 0x75, "del": 0x75,
        "escape": 0x35, "esc": 0x35,
        "left": 0x7B, "leftarrow": 0x7B,
        "right": 0x7C, "rightarrow": 0x7C,
        "down": 0x7D, "downarrow": 0x7D,
        "up": 0x7E, "uparrow": 0x7E,
        "home": 0x73, "end": 0x77,
        "pageup": 0x74, "pagedown": 0x79,
        "f1": 0x7A, "f2": 0x78, "f3": 0x63, "f4": 0x76,
        "f5": 0x60, "f6": 0x61, "f7": 0x62, "f8": 0x64,
        "f9": 0x65, "f10": 0x6D, "f11": 0x67, "f12": 0x6F,
    ]

    private static let letterKeys: [Character: Int] = [
        "a": 0x00, "b": 0x0B, "c": 0x08, "d": 0x02, "e": 0x0E, "f": 0x03,
        "g": 0x05, "h": 0x04, "i": 0x22, "j": 0x26, "k": 0x28, "l": 0x25,
        "m": 0x2E, "n": 0x2D, "o": 0x1F, "p": 0x23, "q": 0x0C, "r": 0x0F,
        "s": 0x01, "t": 0x11, "u": 0x20, "v": 0x09, "w": 0x0D, "x": 0x07,
        "y": 0x10, "z": 0x06,
    ]

    private static let digitKeys: [Character: Int] = [
        "0": 0x1D, "1": 0x12, "2": 0x13, "3": 0x14, "4": 0x15,
        "5": 0x17, "6": 0x16, "7": 0x1A, "8": 0x1C, "9": 0x19,
    ]
}
