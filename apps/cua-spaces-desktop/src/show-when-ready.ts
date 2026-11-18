// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Windows start hidden and show once their page has painted (`ready-to-show`),
// so no blank frame flashes. Chromium on Wayland presents nothing for a window
// that was never mapped, so on some compositors (headless GNOME Shell, seen on
// Ubuntu 24.04) the first paint, and with it `ready-to-show`, never comes and
// the window never appears. A window is therefore shown at the latest after
// `READY_TIMEOUT_MS`; its background colour is already the theme's, so showing
// early only means the page paints a moment later. (CUA-1507, Wayland.)
import type { BrowserWindow } from "electron";

export const READY_TIMEOUT_MS = 2500;

/** Calls `show` once, when `win` is ready to show or `READY_TIMEOUT_MS` passed, whichever is first; never for a closed window. */
export function showWhenReady(win: Pick<BrowserWindow, "once" | "isDestroyed">, show: () => void, timeoutMs = READY_TIMEOUT_MS): void {
  let done = false;
  const go = () => {
    if (done || win.isDestroyed()) return;
    done = true;
    clearTimeout(timer);
    show();
  };
  const timer = setTimeout(go, timeoutMs);
  timer.unref?.();
  win.once("ready-to-show", go);
  win.once("closed", () => {
    done = true;
    clearTimeout(timer);
  });
}
