import Foundation

/// X11 keysym constants for VNC key events
/// See: https://www.cl.cam.ac.uk/~mgk25/ucs/keysymdef.h
enum X11Keysym: UInt32 {
    // Special keys
    case backSpace = 0xff08
    case tab = 0xff09
    case returnKey = 0xff0d  // Enter/Return
    case escape = 0xff1b
    case delete = 0xffff

    // Modifiers
    case shiftL = 0xffe1
    case shiftR = 0xffe2
    case controlL = 0xffe3
    case controlR = 0xffe4
    case metaL = 0xffe7     // Command on Mac
    case metaR = 0xffe8
    case altL = 0xffe9      // Option on Mac
    case altR = 0xffea
    case superL = 0xffeb    // Also used for Command
    case superR = 0xffec

    // Arrow keys
    case left = 0xff51
    case up = 0xff52
    case right = 0xff53
    case down = 0xff54

    // Navigation
    case home = 0xff50
    case end = 0xff57
    case pageUp = 0xff55
    case pageDown = 0xff56

    // Function keys
    case f1 = 0xffbe
    case f2 = 0xffbf
    case f3 = 0xffc0
    case f4 = 0xffc1
    case f5 = 0xffc2
    case f6 = 0xffc3
    case f7 = 0xffc4
    case f8 = 0xffc5
    case f9 = 0xffc6
    case f10 = 0xffc7
    case f11 = 0xffc8
    case f12 = 0xffc9

    // Space
    case space = 0x0020
}

/// Convert macOS key codes to X11 keysyms
func macKeyCodeToKeysym(_ keyCode: UInt16) -> UInt32 {
    let keyMap: [UInt16: UInt32] = [
        36: X11Keysym.returnKey.rawValue,   // Return/Enter
        48: X11Keysym.tab.rawValue,          // Tab
        51: X11Keysym.backSpace.rawValue,    // Backspace
        53: X11Keysym.escape.rawValue,       // Escape
        49: X11Keysym.space.rawValue,        // Space
        117: X11Keysym.delete.rawValue,      // Delete
        123: X11Keysym.left.rawValue,        // Left arrow
        124: X11Keysym.right.rawValue,       // Right arrow
        125: X11Keysym.down.rawValue,        // Down arrow
        126: X11Keysym.up.rawValue,          // Up arrow
        115: X11Keysym.home.rawValue,        // Home
        119: X11Keysym.end.rawValue,         // End
        116: X11Keysym.pageUp.rawValue,      // Page Up
        121: X11Keysym.pageDown.rawValue,    // Page Down

        // Function keys
        122: X11Keysym.f1.rawValue,
        120: X11Keysym.f2.rawValue,
        99: X11Keysym.f3.rawValue,
        118: X11Keysym.f4.rawValue,
        96: X11Keysym.f5.rawValue,
        97: X11Keysym.f6.rawValue,
        98: X11Keysym.f7.rawValue,
        100: X11Keysym.f8.rawValue,
        101: X11Keysym.f9.rawValue,
        109: X11Keysym.f10.rawValue,
        103: X11Keysym.f11.rawValue,
        111: X11Keysym.f12.rawValue,

        // Modifier keys
        56: X11Keysym.shiftL.rawValue,       // Left Shift
        60: X11Keysym.shiftR.rawValue,       // Right Shift
        59: X11Keysym.controlL.rawValue,     // Left Control
        62: X11Keysym.controlR.rawValue,     // Right Control
        58: X11Keysym.altL.rawValue,         // Left Option
        61: X11Keysym.altR.rawValue,         // Right Option
        55: X11Keysym.superL.rawValue,       // Left Command
        54: X11Keysym.superR.rawValue,       // Right Command
    ]

    return keyMap[keyCode] ?? UInt32(keyCode)
}

/// Convert a character to its X11 keysym
/// For ASCII characters, the keysym is the same as the ASCII code
func charToKeysym(_ char: Character) -> (keysym: UInt32, needsShift: Bool) {
    guard let scalar = char.unicodeScalars.first else {
        return (X11Keysym.space.rawValue, false)
    }

    let value = scalar.value

    // ASCII printable characters (0x20-0x7E)
    if value >= 0x20 && value <= 0x7E {
        // Check if character requires shift
        let needsShift = char.isUppercase ||
            "!@#$%^&*()_+{}|:\"<>?~".contains(char)

        // For uppercase letters, use lowercase keysym with shift
        if char.isUppercase {
            return (UInt32(char.lowercased().first?.asciiValue ?? 0), true)
        }

        // For shifted symbols, use the unshifted key's keysym
        let shiftedMap: [Character: UInt32] = [
            "!": 0x31, "@": 0x32, "#": 0x33, "$": 0x34, "%": 0x35,
            "^": 0x36, "&": 0x37, "*": 0x38, "(": 0x39, ")": 0x30,
            "_": 0x2d, "+": 0x3d, "{": 0x5b, "}": 0x5d, "|": 0x5c,
            ":": 0x3b, "\"": 0x27, "<": 0x2c, ">": 0x2e, "?": 0x2f,
            "~": 0x60
        ]

        if let unshifted = shiftedMap[char] {
            return (unshifted, true)
        }

        return (value, needsShift)
    }

    // Default to space
    return (X11Keysym.space.rawValue, false)
}
