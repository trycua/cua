// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The viewer's keyboard (the SwiftUI app's `KeyCapture`): while a Space's
// live desktop has the keyboard, the app's menu shortcuts stand aside, so
// ⌘C, ⌘Q, ⌘W and every other Command chord (Ctrl chords off the Mac) reach
// the Space, not the menus. The web UI marks the viewer's canvas
// (apps/cua-spaces-web/src/components/video/webcodecs-slots.ts); the preload
// says when that canvas gains or loses focus, and main tells the window's
// web contents to ignore menu shortcuts meanwhile. The release chord
// (Control+Option on a Mac) is the page's: it blurs the canvas.

/** The marker the web UI puts on a viewer's canvas (it takes keys). */
export const KEYBOARD_ATTRIBUTE = "data-space-keyboard";

/** Preload -> main: `true` while a viewer has the keyboard. */
export const KEYBOARD_CHANNEL = "cua-desktop:space-keyboard";

/** Whether the focused element is a viewer that takes keys. */
export function viewerHasKeyboard(active: Pick<Element, "hasAttribute"> | null | undefined): boolean {
  return active?.hasAttribute(KEYBOARD_ATTRIBUTE) === true;
}

/** What a window's web contents is told: menu shortcuts off while a viewer has the keyboard. */
export interface MenuShortcuts {
  setIgnoreMenuShortcuts(ignore: boolean): void;
}

/** Applies the preload's report (anything but `true` gives the shortcuts back). */
export function applyKeyboard(contents: MenuShortcuts, has: unknown): void {
  contents.setIgnoreMenuShortcuts(has === true);
}
