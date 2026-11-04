// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The viewer's keyboard policy on a Mac, as the SwiftUI app's `KeyCapture`
 * (libs/spaces-app-swift/Sources/CuaSpacesStreaming/KeyCapture.swift).
 *
 * While the viewer has the keyboard (its canvas has focus, which a click on
 * it gives), every Command chord goes to the Space instead of the app's
 * menus and the page's shortcuts: the Space sends Command to a macOS guest
 * and Super to a Linux one, so ⌘2 is Super+2 in Hyprland.
 *
 * Pressing and releasing Control+Option, with no other key in between,
 * hands the keyboard back until the viewer is clicked again; so does a
 * click outside it. Control+Option chords such as ⌃⌥T still reach the Space.
 *
 * Shortcuts macOS reserves for itself (⌘Tab, ⌘Space, ⌘⇧3/4/5, ⌃ arrows,
 * Mission Control) never reach the app, so they stay on the Mac.
 */

/** Where a key event goes: the app as usual, the Space (a Command chord),
 * or `release` (the release chord: the keyboard goes back to the page). */
export type KeyRoute = "app" | "guest" | "release";

export type KeyCaptureEvent = Pick<KeyboardEvent, "type" | "key" | "metaKey" | "shiftKey" | "altKey" | "ctrlKey">;

/** Keys that only change the modifiers (AppKit's `flagsChanged`). */
const MODIFIER_KEYS = new Set(["Meta", "OS", "Shift", "Alt", "Control"]);

export class KeyCapture {
  /** The release chord is held with no other key typed since. */
  private releaseArmed = false;

  /** The route for one `keydown` or `keyup`. `focused`: the viewer has the keyboard. */
  route(e: KeyCaptureEvent, focused: boolean): KeyRoute {
    if (!focused) {
      this.releaseArmed = false;
      return "app";
    }
    if (MODIFIER_KEYS.has(e.key)) {
      // The modifiers held after this press or release, like `flagsChanged`.
      const any = e.metaKey || e.shiftKey || e.altKey || e.ctrlKey;
      if (e.ctrlKey && e.altKey && !e.metaKey && !e.shiftKey) {
        this.releaseArmed = true;
        return "app";
      }
      const released = this.releaseArmed && !any;
      // Another modifier joined (not a subset of Control+Option), or all are up.
      if (e.metaKey || e.shiftKey || !any) this.releaseArmed = false;
      return released ? "release" : "app";
    }
    if (e.type !== "keydown") return "app";
    this.releaseArmed = false;
    return e.metaKey ? "guest" : "app";
  }
}
