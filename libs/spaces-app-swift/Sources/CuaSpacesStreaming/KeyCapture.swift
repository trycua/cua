// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit

/// Where a key event goes while an interactive stream view may own the
/// keyboard.
public enum KeyRoute: Equatable, Sendable {
    /// AppKit as usual: the responder chain, then the app's menus.
    case app
    /// To the Space, before the app's menus and shortcuts see it.
    case guest
    /// The release chord: the stream hands the keyboard back to the Mac.
    case release
}

/// The keyboard capture policy of an interactive stream view.
///
/// While the stream owns keyboard focus (it is its window's first responder,
/// which a click on it makes it), every Command chord goes to the Space
/// instead of the Cua Spaces menus: the Space sends Command to a macOS guest
/// and Super to a Linux one (the Windows key on Windows), so ⌘2 is Super+2
/// in Hyprland. This holds in the main window and in the floating viewer.
///
/// Pressing and releasing Control+Option, with no other key in between,
/// hands the keyboard back (Command shortcuts reach the Mac again) until the
/// stream is clicked again; so does clicking outside the stream. Control+
/// Option chords such as ⌃⌥T still reach the Space.
///
/// Shortcuts macOS reserves for itself (⌘Tab, ⌘Space, ⌘⇧3/4/5, ⌃ arrows,
/// Mission Control and other system shortcuts) never reach the app, so they
/// stay on the Mac: capturing them needs an event tap and the Accessibility
/// permission, which the viewer does not ask for.
public struct KeyCapture: Equatable, Sendable {
    /// Control+Option, pressed and released alone.
    public static let releaseChord: NSEvent.ModifierFlags = [.control, .option]

    /// The release chord is held with no other key typed since.
    private var releaseArmed = false

    public init() {}

    /// The route for one key event. `focused`: the event is for the window
    /// whose first responder is the interactive stream.
    public mutating func route(_ type: NSEvent.EventType, flags: NSEvent.ModifierFlags,
                               focused: Bool) -> KeyRoute {
        let held = flags.intersection([.command, .shift, .option, .control])
        guard focused else {
            releaseArmed = false
            return .app
        }
        switch type {
        case .keyDown:
            releaseArmed = false
            return held.contains(.command) ? .guest : .app
        case .flagsChanged:
            if held == Self.releaseChord {
                releaseArmed = true
                return .app
            }
            let released = releaseArmed && held.isEmpty
            if !held.isSubset(of: Self.releaseChord) || held.isEmpty { releaseArmed = false }
            return released ? .release : .app
        default:
            return .app
        }
    }
}
#endif
