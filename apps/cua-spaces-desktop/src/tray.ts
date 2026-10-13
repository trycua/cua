// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The menu bar item on macOS and the tray icon on Windows and Linux (the
// SwiftUI app's MenuBarExtra): the core's menu (`appMenu`: how many Spaces,
// the Cua Volume's sync line and conflicts, Open Cua Spaces, New Space…,
// Settings… and Quit), and on Windows and Linux, in place of the notch,
// each Space, which opens it. On macOS it is there whether the notch shows
// or not, as in the Swift app. On Windows, closing the window hides it here
// instead of quitting; use Quit in the menu (or File > Exit) to stop the
// app. Linux keeps quit-on-close, because many desktops (GNOME without the
// AppIndicator extension) show no tray at all.
import { app, Menu, nativeImage, Tray, type MenuItemConstructorOptions } from "electron";
import { existsSync } from "node:fs";
import * as path from "node:path";

let tray: Tray | null = null;
let quitting = false;

export const isQuitting = () => quitting;
export const closesToTray = () => tray !== null && process.platform === "win32";

// Packaged: Resources/tray/. Dev: the Cua Spaces icons, used in place.
function iconPath(): string | null {
  const name = process.platform === "darwin" ? "tray-template.png" : process.platform === "win32" ? "icon.ico" : "32x32.png";
  const dirs = app.isPackaged ? [path.join(process.resourcesPath, "tray")] : [path.join(__dirname, "../../cua-spaces/src-tauri/icons")];
  for (const dir of dirs) {
    const file = path.join(dir, name);
    if (existsSync(file)) return file;
  }
  return null;
}

/** What the tray lists of a Space. */
export interface TraySpace {
  id: string;
  name: string;
}

/** One of the core's menu items (`AppMenuItem`). */
export interface TrayMenuItem {
  id: string;
  label: string;
  shortcut?: string;
  enabled: boolean;
}

export interface TrayActions {
  /** The main window, in front. */
  open(): void;
  /** The main window on a route of the app. */
  route(route: string): void;
  /** The page's New Space. */
  newSpace(): void;
}

/** "⌘," / "⌘Q" as an accelerator (macOS only; the tray menus elsewhere show none). */
export function accelerator(shortcut: string | undefined, platform: NodeJS.Platform = process.platform): string | undefined {
  if (platform !== "darwin" || !shortcut?.startsWith("⌘") || shortcut.length !== 2) return undefined;
  return `Command+${shortcut.slice(1).toUpperCase()}`;
}

/** The menu: the core's items, and on Windows and Linux the Spaces after the count. */
export function trayTemplate(
  items: TrayMenuItem[],
  spaces: TraySpace[],
  actions: TrayActions,
  platform: NodeJS.Platform = process.platform,
): MenuItemConstructorOptions[] {
  const run = (id: string) => {
    switch (id) {
      case "open":
        return actions.open();
      case "newSpace":
        return actions.newSpace();
      case "settings":
        return actions.route("/settings");
      case "volumeConflicts":
        return actions.route("/volume");
      case "quit":
        return app.quit();
    }
  };
  const out: MenuItemConstructorOptions[] = [];
  for (const item of items) {
    if (item.id === "separator") {
      out.push({ type: "separator" });
      continue;
    }
    if (item.id === "status") {
      out.push({ label: item.label, enabled: false });
      // Windows and Linux have no notch: each Space is here.
      if (platform !== "darwin" && spaces.length) {
        out.push(...spaces.slice(0, 12).map((s) => ({ label: s.name, click: () => actions.route(`/spaces/${encodeURIComponent(s.id)}`) })));
      }
      continue;
    }
    out.push({ label: item.label, enabled: item.enabled, accelerator: accelerator(item.shortcut, platform), click: () => run(item.id) });
  }
  return out;
}

export function installTray(o: {
  actions: TrayActions;
  /** The core's menu (null until the native layer is in). */
  items: () => TrayMenuItem[] | null;
  spaces: () => TraySpace[];
  /** Calls back when the menu's input changed; returns the unsubscribe. */
  onChange?: (listener: () => void) => () => void;
}): void {
  if (tray) return;
  const icon = iconPath();
  if (!icon) return;

  const image = nativeImage.createFromPath(icon);
  if (process.platform === "darwin") image.setTemplateImage(true);
  tray = new Tray(image);
  tray.setToolTip("Cua Spaces");

  const fallback: TrayMenuItem[] = [
    { id: "open", label: "Open Cua Spaces", enabled: true },
    { id: "separator", label: "", enabled: false },
    { id: "quit", label: "Quit Cua Spaces", enabled: true },
  ];
  const rebuild = () => tray?.setContextMenu(Menu.buildFromTemplate(trayTemplate(o.items() ?? fallback, o.spaces(), o.actions)));

  // Windows opens the window on a left click; macOS and Linux show the menu.
  if (process.platform === "win32") tray.on("click", () => o.actions.open());
  // Read again each time the menu could open, and when what it shows changed.
  tray.on("right-click", rebuild);
  o.onChange?.(rebuild);
  rebuild();
  setInterval(rebuild, 60_000).unref();
}

app.on("before-quit", () => {
  quitting = true;
});
