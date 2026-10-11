// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The application menu, as the SwiftUI app's (its scenes' commands plus
// AppKit's standard menus): the app menu with About, Settings… (⌘,),
// Services, Hide and Quit; File with New Space (⌘N) and Close; Edit; View
// with Enter Full Screen only (no reload or zoom: the Swift app has none;
// a development build keeps Reload and the developer tools); Window; and
// Help with "Cua Spaces Help", which opens the docs. Windows and Linux get
// the same items in their own places: New Space, Settings… and Exit in
// File, the help item in Help.
//
// Quit is this app's own item rather than the `quit` role, and the app
// registers `ApplePersistenceIgnoreState` as the Swift app does, so macOS
// adds no "Quit and Keep Windows" beside it (this app restores no windows).
import { app, Menu, shell, systemPreferences, type MenuItemConstructorOptions } from "electron";

/** The docs "Cua Spaces Help" opens. */
export const HELP_URL = "https://cua.ai/docs";

export interface MenuActions {
  /** The New Space wizard in the main window (the tray's New Space…). */
  newSpace(): void;
  /** Settings in the main window. */
  settings(): void;
}

/** The menu for `platform`; `dev` (an unpackaged build) adds Reload and the developer tools. */
export function menuTemplate(platform: NodeJS.Platform, actions: MenuActions, { dev = false, appName = "Cua Spaces" } = {}): MenuItemConstructorOptions[] {
  const mac = platform === "darwin";
  const newSpace: MenuItemConstructorOptions = { label: "New Space", accelerator: "CmdOrCtrl+N", click: () => actions.newSpace() };
  const settings: MenuItemConstructorOptions = { label: "Settings…", accelerator: "CmdOrCtrl+,", click: () => actions.settings() };
  const quit: MenuItemConstructorOptions = mac
    ? { label: `Quit ${appName}`, accelerator: "Command+Q", click: () => app.quit() }
    : { label: "Exit", click: () => app.quit() };
  const devItems: MenuItemConstructorOptions[] = dev ? [{ role: "reload" }, { role: "forceReload" }, { role: "toggleDevTools" }, { type: "separator" }] : [];
  return [
    ...(mac
      ? [
          {
            label: appName,
            submenu: [
              { role: "about" },
              { type: "separator" },
              settings,
              { type: "separator" },
              { role: "services" },
              { type: "separator" },
              { role: "hide" },
              { role: "hideOthers" },
              { role: "unhide" },
              { type: "separator" },
              quit,
            ],
          } satisfies MenuItemConstructorOptions,
        ]
      : []),
    {
      label: "File",
      submenu: mac ? [newSpace, { type: "separator" }, { role: "close" }] : [newSpace, { type: "separator" }, settings, { type: "separator" }, quit],
    },
    { role: "editMenu" },
    { label: "View", submenu: [...devItems, { role: "togglefullscreen" }] },
    { role: "windowMenu" },
    {
      role: "help",
      submenu: [{ label: `${appName} Help`, click: () => void shell.openExternal(HELP_URL) }],
    },
  ];
}

export function installMenu(actions: MenuActions): void {
  // AppKit offers "Quit and Keep Windows" to apps that restore their windows; this one doesn't.
  if (process.platform === "darwin") systemPreferences.registerDefaults({ ApplePersistenceIgnoreState: true });
  Menu.setApplicationMenu(Menu.buildFromTemplate(menuTemplate(process.platform, actions, { dev: !app.isPackaged })));
}
