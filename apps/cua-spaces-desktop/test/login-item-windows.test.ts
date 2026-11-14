// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Launch at login on Windows, against a stand-in for Electron's Run entry
// handling (shell/browser/browser_win.cc): `setLoginItemSettings` writes
// HKCU\...\Run\<name> as a quoted command line; `getLoginItemSettings` reads
// `openAtLogin` under the AppUserModelID and finds the app's entries by the
// program its `path` parses to, with StartupApproved\Run saying whether one
// is turned off.
import { beforeEach, describe, expect, it, vi } from "vitest";

const AUMID = "ai.cua.spaces.desktop";
const EXE = "C:\\Users\\me\\AppData\\Local\\Programs\\cua-spaces\\Cua Spaces.exe";

const registry = { run: new Map<string, string>(), approved: new Map<string, "on" | "off">() };

/** Chromium's base::CommandLine::FromString(...).GetProgram(). */
const program = (commandLine: string) => (commandLine.startsWith('"') ? commandLine.slice(1, commandLine.indexOf('"', 1)) : (commandLine.split(/\s/)[0] ?? ""));
const quote = (arg: string) => (/[\s"]/.test(arg) ? `"${arg}"` : arg);
/** Electron's FormatCommandLineString. */
const format = (path: string | undefined, args: string[] = []) => {
  const exe = (path || EXE).replace(/^"(.*)"$/, "$1");
  return [quote(exe), ...args.map(quote)].join(" ");
};

type Options = { name?: string; path?: string; args?: string[]; openAtLogin?: boolean };

vi.mock("electron", () => ({
  app: {
    isPackaged: true,
    setLoginItemSettings: (o: Options) => {
      const name = o.name || AUMID;
      if (o.openAtLogin) {
        registry.run.set(name, format(o.path, o.args));
        registry.approved.delete(name);
      } else {
        registry.run.delete(name);
        registry.approved.delete(name);
      }
    },
    getLoginItemSettings: (o: Options) => {
      const lookup = program(o.path || EXE).toLowerCase();
      const items = [...registry.run].filter(([, value]) => program(value).toLowerCase() === lookup);
      return {
        openAtLogin: registry.run.get(AUMID) === format(o.path, o.args),
        executableWillLaunchAtLogin: items.some(([name]) => registry.approved.get(name) !== "off"),
      };
    },
  },
  shell: { openExternal: vi.fn(), openPath: vi.fn() },
}));

const { WindowsLoginItem, windowsLoginOptions, WINDOWS_RUN_NAME, HIDDEN_ARG } = await import("../src/login-item");

describe("launch at login on Windows", () => {
  beforeEach(() => {
    registry.run.clear();
    registry.approved.clear();
  });

  it("reads the entry it wrote as on, not as waiting for approval", () => {
    const item = new WindowsLoginItem(windowsLoginOptions(EXE));
    expect(item.status()).toBe("notRegistered");
    item.register();
    expect(registry.run.get(AUMID)).toBe(`"${EXE}" ${HIDDEN_ARG}`);
    expect(item.status()).toBe("enabled");
    item.unregister();
    expect(registry.run.size).toBe(0);
    expect(item.status()).toBe("notRegistered");
  });

  it("names the entry after the AppUserModelID, the name Electron reads it under", () => {
    expect(WINDOWS_RUN_NAME).toBe(AUMID);
    expect(windowsLoginOptions(EXE)).toEqual({ name: AUMID, path: `"${EXE}"`, args: [HIDDEN_ARG] });
  });

  it("reads an entry turned off under Startup apps as waiting there", () => {
    const item = new WindowsLoginItem(windowsLoginOptions(EXE));
    item.register();
    registry.approved.set(AUMID, "off");
    expect(item.status()).toBe("requiresApproval");
  });

  it("would read a working entry as waiting for approval with an unquoted path (the bug)", () => {
    // The path with a space parses to `...\cua-spaces\Cua`, which no entry runs.
    const item = new WindowsLoginItem({ name: AUMID, path: EXE, args: [HIDDEN_ARG] });
    item.register();
    expect(registry.run.get(AUMID)).toBe(`"${EXE}" ${HIDDEN_ARG}`);
    expect(item.status()).toBe("requiresApproval");
  });
});
