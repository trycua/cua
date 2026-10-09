// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The bridge's system services with Electron (`bridge/system.ts`): links and
// settings panes, Show in Finder / Explorer, notifications (a click brings
// the app to the page it is about), the owner check before approving a
// device, the login item and the updater.
//
// The owner check is the system's prompt through the app core
// (`model/presence.ts`: Touch ID, Windows Hello, polkit), with a dialog of
// the app's own that says so where Windows or Linux has none.
import { app, BrowserWindow, dialog, Notification, shell } from "electron";
import * as os from "node:os";
import type { SystemNote, SystemServices } from "./bridge/system";
import { confirmOwner } from "./model/presence";
import { makeLoginItem } from "./login-item";
import type { Native } from "./native/load";
import { ElectronUpdater } from "./updater";

const OPENABLE = /^(https?:|x-apple\.systempreferences:|ms-settings:)/;

export function makeSystem(o: { native: Native; showRoute: (route: string) => void; platform?: NodeJS.Platform }): SystemServices {
  const platform = o.platform ?? process.platform;
  // Shown notifications, by id (a newer one with the same id replaces it).
  const shown = new Map<string, Notification>();

  const notify = (note: SystemNote) => {
    if (!Notification.isSupported()) return;
    shown.get(note.id)?.close();
    const n = new Notification({ title: note.title, body: note.body });
    n.on("click", () => {
      app.focus({ steal: true });
      o.showRoute(note.route ?? "/spaces");
    });
    n.on("close", () => {
      if (shown.get(note.id) === n) shown.delete(note.id);
    });
    shown.set(note.id, n);
    n.show();
  };

  const confirmPresence = async (reason: string) => {
    // Windows Hello sits over the foreground window, and polkit's dialog over the session: be in front.
    if (platform !== "darwin") app.focus({ steal: true });
    await confirmOwner(platform, reason, (r) => o.native.appConfirmPresence(r), async (w) => {
      const win = BrowserWindow.getFocusedWindow() ?? BrowserWindow.getAllWindows()[0];
      const options: Electron.MessageBoxOptions = { type: "warning", ...w, buttons: ["Approve", "Cancel"], defaultId: 1, cancelId: 1 };
      const r = win ? await dialog.showMessageBox(win, options) : await dialog.showMessageBox(options);
      return r.response === 0;
    });
  };

  return {
    openExternal: async (url) => {
      if (!OPENABLE.test(url)) throw new Error(`not a page or settings pane: ${url}`);
      await shell.openExternal(url);
    },
    reveal: (p) => shell.showItemInFolder(p),
    notify,
    confirmPresence,
    loginItem: makeLoginItem(platform),
    updater: ElectronUpdater.start(),
    home: process.env.HOME || os.homedir(),
  };
}
