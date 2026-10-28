// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Testing
@testable import CuaSpacesStreaming

/// Command chords typed while the interactive stream has the keyboard go to
/// the Space (Super on a Linux guest), not to the app's menus (#4609).
@Suite @MainActor struct KeyCaptureTests {

    @Test func commandChordsGoToTheGuestWhileTheStreamHasFocus() {
        var c = KeyCapture()
        #expect(c.route(.keyDown, flags: .command, focused: true) == .guest, "⌘2 → Super+2")
        #expect(c.route(.keyDown, flags: [.command, .shift], focused: true) == .guest)
        #expect(c.route(.keyDown, flags: [.command, .control], focused: true) == .guest)
        // Plain keys and Control chords take the view's own keyDown.
        #expect(c.route(.keyDown, flags: [], focused: true) == .app)
        #expect(c.route(.keyDown, flags: .control, focused: true) == .app)
        // Without the keyboard the app keeps its shortcuts.
        #expect(c.route(.keyDown, flags: .command, focused: false) == .app)
    }

    /// Control+Option pressed and released alone hands the keyboard back;
    /// a Control+Option chord (⌃⌥T) does not.
    @Test func controlOptionAloneReleasesTheKeyboard() {
        var c = KeyCapture()
        #expect(c.route(.flagsChanged, flags: .control, focused: true) == .app)
        #expect(c.route(.flagsChanged, flags: [.control, .option], focused: true) == .app)
        #expect(c.route(.flagsChanged, flags: .option, focused: true) == .app)
        #expect(c.route(.flagsChanged, flags: [], focused: true) == .release)

        #expect(c.route(.flagsChanged, flags: [.control, .option], focused: true) == .app)
        #expect(c.route(.keyDown, flags: [.control, .option], focused: true) == .app, "⌃⌥T goes to the view")
        #expect(c.route(.flagsChanged, flags: [], focused: true) == .app, "a chord, not the release")

        #expect(c.route(.flagsChanged, flags: [.control, .option], focused: true) == .app)
        #expect(c.route(.flagsChanged, flags: [.control, .option, .shift], focused: true) == .app)
        #expect(c.route(.flagsChanged, flags: [], focused: true) == .app, "another modifier joined")
    }
}
#endif
