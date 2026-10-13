// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The menu bar item and the tray (the SwiftUI app's MenuBarExtra): the
// core's items in order with their actions, ⌘ shortcuts on macOS, and on
// Windows and Linux each Space after the count.
import { describe, expect, it, vi } from "vitest";

const quit = vi.fn();
vi.mock("electron", () => ({ app: { quit, on: () => {}, isPackaged: false }, Menu: {}, nativeImage: {}, Tray: class {} }));
const { accelerator, trayTemplate } = await import("../src/tray");

const items = [
  { id: "status", label: "2 Spaces", enabled: false },
  { id: "separator", label: "", enabled: false },
  { id: "open", label: "Open Cua Spaces", enabled: true },
  { id: "newSpace", label: "New Space…", enabled: true },
  { id: "settings", label: "Settings…", shortcut: "⌘,", enabled: true },
  { id: "volumeConflicts", label: "1 conflict", enabled: true },
  { id: "separator", label: "", enabled: false },
  { id: "quit", label: "Quit Cua Spaces", shortcut: "⌘Q", enabled: true },
];
const spaces = [
  { id: "local:a", name: "Aurora" },
  { id: "cloud:b", name: "Builder" },
];

function actions() {
  const log: string[] = [];
  return { log, actions: { open: () => log.push("open"), route: (r: string) => log.push(`route ${r}`), newSpace: () => log.push("new") } };
}

describe("the menu bar item and the tray", () => {
  it("is the core's menu on macOS, with its shortcuts and no Space list", () => {
    const { log, actions: a } = actions();
    const t = trayTemplate(items, spaces, a, "darwin");
    expect(t.map((i) => (i.type === "separator" ? "---" : i.label))).toEqual(["2 Spaces", "---", "Open Cua Spaces", "New Space…", "Settings…", "1 conflict", "---", "Quit Cua Spaces"]);
    expect(t[0]!.enabled).toBe(false);
    expect(t.find((i) => i.label === "Settings…")!.accelerator).toBe("Command+,");
    expect(t.find((i) => i.label === "Quit Cua Spaces")!.accelerator).toBe("Command+Q");
    for (const i of t) (i.click as (() => void) | undefined)?.();
    expect(log).toEqual(["open", "new", "route /settings", "route /volume"]);
    expect(quit).toHaveBeenCalledOnce();
  });

  it("lists each Space after the count on Windows and Linux, each opening it", () => {
    for (const platform of ["win32", "linux"] as const) {
      const { log, actions: a } = actions();
      const t = trayTemplate(items, spaces, a, platform);
      expect(t.slice(0, 3).map((i) => i.label)).toEqual(["2 Spaces", "Aurora", "Builder"]);
      (t[1]!.click as () => void)();
      expect(log).toEqual(["route /spaces/local%3Aa"]);
      expect(t.every((i) => i.accelerator === undefined)).toBe(true);
    }
  });

  it("reads only ⌘ shortcuts, on macOS", () => {
    expect(accelerator("⌘,", "darwin")).toBe("Command+,");
    expect(accelerator("⌘Q", "win32")).toBeUndefined();
    expect(accelerator(undefined, "darwin")).toBeUndefined();
  });
});
