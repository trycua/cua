// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Testing
@testable import CuaSpacesStreaming

/// The viewer that had the keyboard when its stream dropped takes it back
/// once the stream is back (keys used to go nowhere until a click).
@MainActor
@Suite struct KeyboardReturnTests {
    @Test func returnsOnlyWhatItHadAndOnlyWhenFree() {
        let r = KeyboardReturn()
        // Nothing dropped: nothing to return.
        #expect(!r.back(keyboardIsFree: true))
        r.dropped(hadKeyboard: true)
        // Later calls of the same drop (keys already gone) don't count.
        r.dropped(hadKeyboard: false)
        #expect(r.waiting)
        #expect(r.back(keyboardIsFree: true))
        #expect(!r.back(keyboardIsFree: true))

        r.dropped(hadKeyboard: false)
        #expect(!r.back(keyboardIsFree: true))

        r.dropped(hadKeyboard: true)
        #expect(!r.back(keyboardIsFree: false))

        r.dropped(hadKeyboard: true)
        r.forget()
        #expect(!r.back(keyboardIsFree: true))
    }

    @Test func theViewTakesTheKeyboardBackUnlessSomethingElseHasIt() {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.titled],
                              backing: .buffered, defer: true)
        let root = NSView(frame: window.contentLayoutRect)
        window.contentView = root
        let viewer = LiveStreamInputView(frame: NSRect(x: 0, y: 0, width: 200, height: 150))
        let field = NSTextField(frame: NSRect(x: 210, y: 0, width: 100, height: 22))
        root.addSubview(viewer)
        root.addSubview(field)

        #expect(window.makeFirstResponder(viewer))
        viewer.streamDropped()
        // The cover shows, the viewer loses the keyboard to the window.
        window.makeFirstResponder(nil)
        #expect(viewer.streamBack())
        #expect(window.firstResponder === viewer)

        // The user clicked a field meanwhile: the keys stay there.
        viewer.streamDropped()
        window.makeFirstResponder(field)
        #expect(!viewer.streamBack())
        #expect(window.firstResponder !== viewer)

        // It didn't have the keys when the stream dropped.
        window.makeFirstResponder(nil)
        viewer.streamDropped()
        #expect(!viewer.streamBack())
        #expect(window.firstResponder !== viewer)
    }
}
#endif
