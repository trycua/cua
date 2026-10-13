// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Picture in picture (`stream.pip`, `teleport.streamWindow`): a floating
// window per open panel, the SwiftUI app's `StreamPiPController` panel in
// Electron. It stays on top of every window and on every desktop (full-screen
// apps too), has no frame (the page draws a thin bar with the title, Open
// Space and close while the pointer is over it), keeps the stream's shape,
// and remembers where the last one was left. Each panel shows the web UI's
// picture-in-picture view (`apps/cua-spaces-web/src/pip`), which opens a
// stream of its own (the desktop, or one window) and stops it when the
// window closes.
//
// The page says the stream's size with `window.resizeTo(width, height)`
// once its first frame decodes; here that only sets the shape (the width
// stays the person's), as the Swift panel follows `surfaceSize`.
import { BrowserWindow, screen } from "electron";
import type { PipPresenter, PipSpec } from "./model/streams";
import { APP_ORIGIN } from "./protocol";
import { aspectOf, fitToStream, initialPipBounds, pipPath, PIP_DEFAULT_ASPECT, PIP_MIN_WIDTH, type Rect } from "./pip-layout";
import { readSettings, writeSettings } from "./settings";
import { appWindow } from "./window";

export interface PipWindows extends PipPresenter {
  /** Closes every panel (the app quits). */
  closeAll(): void;
}

export function createPipWindows(): PipWindows {
  const open = new Map<string, BrowserWindow>();
  const id = (spaceId: string, key: string) => `${spaceId}|${key}`;

  const place = (win: BrowserWindow) => {
    if (win.isDestroyed()) return;
    const b = win.getBounds();
    writeSettings({ pipBounds: { x: b.x, y: b.y, width: b.width, height: b.height } });
  };

  return {
    open(spec: PipSpec, closed: () => void) {
      const k = id(spec.spaceId, spec.key);
      const held = open.get(k);
      if (held && !held.isDestroyed()) {
        held.show();
        held.focus();
        return;
      }
      const work = screen.getPrimaryDisplay().workArea;
      const bounds = initialPipBounds({
        saved: readSettings().pipBounds ?? null,
        work,
        areas: screen.getAllDisplays().map((d) => d.workArea),
        aspect: PIP_DEFAULT_ASPECT,
        open: open.size,
      });
      const win = appWindow({
        ...bounds,
        title: spec.title,
        frame: false,
        titleBarStyle: "default",
        titleBarOverlay: false,
        // An NSPanel on macOS: it floats without taking the app forward.
        ...(process.platform === "darwin" ? { type: "panel" } : {}),
        minWidth: PIP_MIN_WIDTH,
        minHeight: Math.round(PIP_MIN_WIDTH / 3),
        alwaysOnTop: true,
        skipTaskbar: true,
        minimizable: false,
        maximizable: false,
        fullscreenable: false,
        backgroundColor: "#000000",
      });
      // Above other apps' windows, and present on top of full-screen apps
      // too: a PiP that disappears behind what you watch is not one.
      win.setAlwaysOnTop(true, "floating");
      win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
      win.setAspectRatio(aspectOf(bounds.width, bounds.height));
      open.set(k, win);

      win.webContents.on("content-bounds-updated", (event, requested: Rect) => {
        event.preventDefault();
        if (win.isDestroyed() || requested.width <= 0 || requested.height <= 0) return;
        win.setAspectRatio(aspectOf(requested.width, requested.height));
        win.setBounds(fitToStream(win.getBounds(), requested.width, requested.height));
      });
      let timer: NodeJS.Timeout | undefined;
      const remember = () => {
        clearTimeout(timer);
        timer = setTimeout(() => place(win), 400);
      };
      win.on("move", remember);
      win.on("resize", remember);
      win.on("close", () => {
        clearTimeout(timer);
        place(win);
      });
      win.on("closed", () => {
        if (open.get(k) === win) open.delete(k);
        closed();
      });
      win.once("ready-to-show", () => win.show());
      void win.loadURL(`${APP_ORIGIN}${pipPath(spec)}`);
    },
    close(spaceId: string, key: string) {
      const win = open.get(id(spaceId, key));
      if (win && !win.isDestroyed()) win.close();
    },
    closeAll() {
      for (const win of [...open.values()]) if (!win.isDestroyed()) win.close();
    },
  };
}
