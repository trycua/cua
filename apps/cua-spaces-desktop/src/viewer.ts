// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's desktop in a window of its own (`spaces.open`, "Open in
// window"), the SwiftUI app's `SpaceWindowView` in Electron: one window per
// Space (opening it again brings it forward), titled with the Space's name,
// with the system's title bar, showing only the web UI's viewer
// (`/viewer`: the stream under a toolbar with the source picker, the
// stream's size and Pop out). It opens where the last viewer was left, at
// that width, takes the stream's shape once the first frame says it (the
// page's `window.resizeTo`, read here as the shape only), and closes when
// its Space is deleted. The video bench places its viewer itself and
// remembers nothing (`remember: false`).
import { BrowserWindow, screen } from "electron";
import { APP_ORIGIN } from "./protocol";
import { fitViewerToStream, initialViewerBounds, viewerPath, VIEWER_MIN_HEIGHT, VIEWER_MIN_WIDTH } from "./viewer-layout";
import type { Rect } from "./pip-layout";
import { readSettings, writeSettings } from "./settings";
import { appWindow } from "./window";

export interface ViewerWindows {
  /** The Space's viewer, opened or brought to the front. */
  open(spaceId: string, name: string, os?: string): BrowserWindow;
  /** Closes the viewers of Spaces no longer listed (deleted). */
  prune(listed: ReadonlySet<string>): void;
  closeAll(): void;
}

/** E2e runs (`CUA_SPACES_E2E_HIDDEN=1`) never show a window. */
const hidden = () => process.env.CUA_SPACES_E2E_HIDDEN === "1";

export function createViewerWindows({ remember = true }: { remember?: boolean } = {}): ViewerWindows {
  const open = new Map<string, BrowserWindow>();

  const place = (win: BrowserWindow) => {
    if (!remember || win.isDestroyed() || win.isMinimized() || win.isFullScreen() || win.isMaximized()) return;
    const frame = win.getBounds();
    const [width] = win.getContentSize();
    writeSettings({ viewerBounds: { x: frame.x, y: frame.y, width: width ?? frame.width } });
  };

  return {
    open(spaceId, name, os) {
      const held = open.get(spaceId);
      if (held && !held.isDestroyed()) {
        if (held.isMinimized()) held.restore();
        if (!hidden()) held.show();
        held.focus();
        return held;
      }
      const work = screen.getPrimaryDisplay().workArea;
      const bounds = initialViewerBounds({
        saved: remember ? (readSettings().viewerBounds ?? null) : null,
        work,
        areas: screen.getAllDisplays().map((d) => d.workArea),
        open: open.size,
      });
      const win = appWindow({
        ...bounds,
        useContentSize: true,
        title: name,
        minWidth: VIEWER_MIN_WIDTH,
        minHeight: VIEWER_MIN_HEIGHT,
        // The system's title bar, with the Space's name, over the page's toolbar.
        titleBarStyle: "default",
        titleBarOverlay: false,
        trafficLightPosition: undefined,
        backgroundColor: "#000000",
      });
      open.set(spaceId, win);

      // The stream's size, once known: the window takes its shape (on the
      // display it is on), keeping its place.
      win.webContents.on("content-bounds-updated", (event, requested: Rect) => {
        event.preventDefault();
        if (win.isDestroyed() || win.isMaximized() || win.isFullScreen() || requested.width <= 0 || requested.height <= 0) return;
        const [width = 0, height = 0] = win.getContentSize();
        const area = screen.getDisplayMatching(win.getBounds()).workArea;
        const next = fitViewerToStream({ width, height }, requested, area);
        win.setContentSize(next.width, next.height);
      });
      let timer: NodeJS.Timeout | undefined;
      const remembered = () => {
        clearTimeout(timer);
        timer = setTimeout(() => place(win), 400);
      };
      win.on("move", remembered);
      win.on("resize", remembered);
      win.on("close", () => {
        clearTimeout(timer);
        place(win);
      });
      win.on("closed", () => {
        if (open.get(spaceId) === win) open.delete(spaceId);
      });
      win.once("ready-to-show", () => {
        if (!hidden()) win.show();
      });
      void win.loadURL(`${APP_ORIGIN}${viewerPath(spaceId, name, os)}`);
      return win;
    },
    prune(listed) {
      for (const [id, win] of [...open]) {
        if (!listed.has(id) && !win.isDestroyed()) win.close();
      }
    },
    closeAll() {
      for (const win of [...open.values()]) if (!win.isDestroyed()) win.close();
    },
  };
}
