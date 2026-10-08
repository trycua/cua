// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The application menu (menu.ts), as the SwiftUI app's: Settings… (⌘,) in
// the app menu, New Space (⌘N) in File, Enter Full Screen only in View (no
// reload or zoom outside development), "Cua Spaces Help" opening the docs,
// and no "Quit and Keep Windows"; Windows and Linux carry the same items.
import { describe, expect, it, vi } from "vitest";
import type { MenuItemConstructorOptions } from "electron";

const quit = vi.fn();
const openExternal = vi.fn();
const registerDefaults = vi.fn();
const setApplicationMenu = vi.fn();
vi.mock("electron", () => ({
  app: { quit, isPackaged: true },
  shell: { openExternal },
  systemPreferences: { registerDefaults },
  Menu: { setApplicationMenu, buildFromTemplate: (t: unknown) => t },
}));
const { HELP_URL, installMenu, menuTemplate } = await import("../src/menu");

type Item = MenuItemConstructorOptions;
const sub = (t: Item[], label: string) => (t.find((m) => m.label === label || m.role === label)?.submenu ?? []) as Item[];
const names = (items: Item[]) => items.map((i) => (i.type === "separator" ? "---" : (i.label ?? i.role)));

function actions() {
  const log: string[] = [];
  return { log, actions: { newSpace: () => log.push("new"), settings: () => log.push("settings") } };
}

describe("the macOS menu", () => {
  it("has the Swift app's app menu: About, Settings… (⌘,), Services, Hide, and its own Quit", () => {
    const { log, actions: a } = actions();
    const t = menuTemplate("darwin", a);
    const appMenu = sub(t, "Cua Spaces");
    expect(names(appMenu)).toEqual(["about", "---", "Settings…", "---", "services", "---", "hide", "hideOthers", "unhide", "---", "Quit Cua Spaces"]);
    const settings = appMenu.find((i) => i.label === "Settings…")!;
    expect(settings.accelerator).toBe("CmdOrCtrl+,");
    (settings.click as () => void)();
    const q = appMenu.find((i) => i.label === "Quit Cua Spaces")!;
    // Not the `quit` role, whose item macOS pairs with "Quit and Keep Windows".
    expect([q.role, q.accelerator]).toEqual([undefined, "Command+Q"]);
    (q.click as () => void)();
    expect(quit).toHaveBeenCalledOnce();
    expect(log).toEqual(["settings"]);
  });

  it("puts New Space (⌘N) and Close in File, full screen alone in View, and Cua Spaces Help in Help", () => {
    const { log, actions: a } = actions();
    const t = menuTemplate("darwin", a);
    expect(names(t)).toEqual(["Cua Spaces", "File", "editMenu", "View", "windowMenu", "help"]);
    const file = sub(t, "File");
    expect(names(file)).toEqual(["New Space", "---", "close"]);
    expect(file[0]!.accelerator).toBe("CmdOrCtrl+N");
    (file[0]!.click as () => void)();
    expect(log).toEqual(["new"]);
    expect(names(sub(t, "View"))).toEqual(["togglefullscreen"]);
    const help = sub(t, "help");
    expect(names(help)).toEqual(["Cua Spaces Help"]);
    (help[0]!.click as () => void)();
    expect(openExternal).toHaveBeenCalledWith(HELP_URL);
  });

  it("keeps Reload and the developer tools in a development build", () => {
    expect(names(sub(menuTemplate("darwin", actions().actions, { dev: true }), "View"))).toEqual(["reload", "forceReload", "toggleDevTools", "---", "togglefullscreen"]);
  });

  it("installs with ApplePersistenceIgnoreState, as the Swift app registers it", () => {
    const platform = Object.getOwnPropertyDescriptor(process, "platform")!;
    Object.defineProperty(process, "platform", { value: "darwin" });
    try {
      installMenu(actions().actions);
    } finally {
      Object.defineProperty(process, "platform", platform);
    }
    expect(registerDefaults).toHaveBeenCalledWith({ ApplePersistenceIgnoreState: true });
    expect(setApplicationMenu).toHaveBeenCalledOnce();
  });
});

describe("the Windows and Linux menu", () => {
  it("has New Space, Settings… and Exit in File and the same View and Help", () => {
    for (const platform of ["win32", "linux"] as const) {
      const { log, actions: a } = actions();
      const t = menuTemplate(platform, a);
      expect(names(t)).toEqual(["File", "editMenu", "View", "windowMenu", "help"]);
      const file = sub(t, "File");
      expect(names(file)).toEqual(["New Space", "---", "Settings…", "---", "Exit"]);
      expect(file.find((i) => i.label === "Settings…")!.accelerator).toBe("CmdOrCtrl+,");
      for (const i of file) (i.click as (() => void) | undefined)?.();
      expect(log).toEqual(["new", "settings"]);
      expect(names(sub(t, "View"))).toEqual(["togglefullscreen"]);
      expect(names(sub(t, "help"))).toEqual(["Cua Spaces Help"]);
    }
  });
});
