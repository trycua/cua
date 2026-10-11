// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What an upgrade or an uninstall leaves: the daemon stopped before files
// are replaced, launch at login and the app's own CLI copy removed with the
// app, the user's data kept; and an AppImage's daemon runs from a copy, so
// it does not keep the AppImage mounted.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("electron", () => ({ app: { isPackaged: true }, shell: {} }));

const { appImageCua, appImageDaemonRoot } = await import("../src/model/appimage-cua");
const { WINDOWS_APP_KEY, cliRecordArgs, installCliSilently } = await import("../src/model/environment");
const { WINDOWS_RUN_NAME } = await import("../src/login-item");

const require = createRequire(import.meta.url);
const config = require("../electron-builder.config.cjs");
const root = path.join(import.meta.dirname, "..");
const nsh = readFileSync(path.join(root, "packaging/installer.nsh"), "utf8");

/** The body of an NSIS macro in the include. */
const macro = (name: string) => {
  const match = new RegExp(`!macro ${name}\\n([\\s\\S]*?)!macroend`).exec(nsh);
  if (!match) throw new Error(`no macro ${name}`);
  return match[1]!;
};

describe("the Windows installer and uninstaller", () => {
  it("include packaging/installer.nsh", () => {
    expect(config.nsis.include).toBe("packaging/installer.nsh");
  });

  it("stop the daemon after the app is closed and before any file is written or removed", () => {
    // CHECK_APP_RUNNING runs in the installer's section before the old
    // version's uninstaller and the new files, and in the uninstaller's init.
    expect(macro("customCheckAppRunning").trim().split("\n").map((l) => l.trim())).toEqual([
      "!insertmacro IS_POWERSHELL_AVAILABLE",
      "!insertmacro _CHECK_APP_RUNNING",
      "!insertmacro stopCuaDaemon",
    ]);
    const stop = macro("stopCuaDaemon");
    expect(stop).toContain(`"$INSTDIR\\resources\\native\\cua.exe" daemon stop`);
    expect(stop).toContain("StartsWith('$INSTDIR\\'");
  });

  it("rely on electron-builder's own check, as the pinned templates define it", () => {
    const appBuilder = path.dirname(require.resolve("app-builder-lib/package.json", { paths: [require.resolve("electron-builder")] }));
    const templates = path.join(appBuilder, "templates", "nsis");
    const check = readFileSync(path.join(templates, "include", "allowOnlyOneInstallerInstance.nsh"), "utf8");
    for (const m of ["!macro _CHECK_APP_RUNNING", "!macro IS_POWERSHELL_AVAILABLE", "!insertmacro customCheckAppRunning"]) expect(check).toContain(m);
    // With a customCheckAppRunning these are left to the include.
    expect(check).toMatch(/!ifmacrondef customCheckAppRunning\s+!include "getProcessInfo.nsh"\s+Var pid\s+!endif/);
    expect(nsh).toContain('!include "getProcessInfo.nsh"\nVar pid');
    const installSection = readFileSync(path.join(templates, "installSection.nsh"), "utf8");
    const order = ["CHECK_APP_RUNNING", "uninstallOldVersion", "installApplicationFiles"].map((m) => installSection.indexOf(`!insertmacro ${m}`));
    expect(order.every((i) => i >= 0)).toBe(true);
    expect([...order].sort((a, b) => a - b)).toEqual(order);
    expect(readFileSync(path.join(templates, "uninstaller.nsh"), "utf8")).toContain("call un.checkAppRunning");
  });

  it("remove launch at login and the app's own CLI on uninstall, not on upgrade, and keep the user's data", () => {
    const un = macro("customUnInstall");
    expect(un.trim().startsWith("${ifNot} ${isUpdated}")).toBe(true);
    expect(un).toContain(`"Software\\Microsoft\\Windows\\CurrentVersion\\Run" "\${CUA_SPACES_RUN_NAME}"`);
    expect(un).toContain(`StartupApproved\\Run" "\${CUA_SPACES_RUN_NAME}"`);
    expect(nsh).toContain(`!define CUA_SPACES_RUN_NAME "${WINDOWS_RUN_NAME}"`);
    expect(`HKCU\\${/!define CUA_SPACES_KEY "([^"]+)"/.exec(nsh)![1]}`).toBe(WINDOWS_APP_KEY);
    expect(macro("removeAppCli")).toContain('ReadRegStr $0 HKCU "${CUA_SPACES_KEY}" "CliInstalled"');
    const code = nsh.split("\n").filter((l) => !l.trim().startsWith(";")).join("\n");
    expect(code).not.toMatch(/[\\/]\.cua\b|\$APPDATA|\$PROFILE|RMDir \/r/);
  });
});

describe("the CLI the app puts on PATH", () => {
  const plan = { upToDate: false, source: "C:\\app\\resources\\native\\cua.exe", onPath: false };
  const fakeNative = (installed: { target: string }) =>
    ({
      AppCliInstaller: { forExecutable: () => ({ plan: async () => plan, install: async () => installed }) },
    }) as never;

  it("is recorded on Windows, with the PATH entry only when the app added it", async () => {
    const target = "C:\\Users\\me\\AppData\\Local\\Programs\\cua\\bin\\cua.exe";
    const calls: string[][] = [];
    await installCliSilently(fakeNative({ target }), "C:\\app\\resources\\native\\cua.exe", "win32", async (args) => void calls.push(args));
    expect(calls).toEqual([
      ["add", WINDOWS_APP_KEY, "/v", "CliInstalled", "/t", "REG_SZ", "/d", target, "/f"],
      ["add", WINDOWS_APP_KEY, "/v", "CliPathAdded", "/t", "REG_SZ", "/d", "C:\\Users\\me\\AppData\\Local\\Programs\\cua\\bin", "/f"],
    ]);
    expect(cliRecordArgs(target, false)).toHaveLength(1);
  });

  it("is not recorded elsewhere (the .deb's is a link into the app)", async () => {
    const calls: string[][] = [];
    await installCliSilently(fakeNative({ target: "/home/me/.local/bin/cua" }), "/opt/Cua Spaces/resources/native/cua", "linux", async (args) => void calls.push(args));
    expect(calls).toEqual([]);
  });
});

describe("an AppImage's daemon", () => {
  const dirs: string[] = [];
  afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));
  const scratch = () => {
    const d = mkdtempSync(path.join(tmpdir(), "cua-appimage-"));
    dirs.push(d);
    return d;
  };

  it("lives in the user's data folder", () => {
    expect(appImageDaemonRoot({ XDG_DATA_HOME: "/x/data" })).toBe(path.join("/x/data", "cua-spaces", "daemon"));
  });

  it("runs from a copy out of the mount, one per build, the others removed", () => {
    const dir = scratch();
    const mount = path.join(dir, "mount");
    mkdirSync(mount);
    const bundled = path.join(mount, "cua");
    writeFileSync(bundled, "build one");
    const daemons = path.join(dir, "daemon");
    mkdirSync(path.join(daemons, "0.7.1-1-1"), { recursive: true });

    const copy = appImageCua(bundled, daemons, "0.7.2");
    expect(path.dirname(path.dirname(copy))).toBe(daemons);
    expect(readFileSync(copy, "utf8")).toBe("build one");
    expect(statSync(copy).mode & 0o777).toBe(0o755);
    expect(existsSync(path.join(daemons, "0.7.1-1-1"))).toBe(false);
    // The same build again uses the copy it made.
    expect(appImageCua(bundled, daemons, "0.7.2")).toBe(copy);

    writeFileSync(bundled, "build two, longer");
    const next = appImageCua(bundled, daemons, "0.7.2");
    expect(next).not.toBe(copy);
    expect(readFileSync(next, "utf8")).toBe("build two, longer");
    expect(existsSync(path.dirname(copy))).toBe(false);
  });

  it("runs from the mount when no copy can be made", () => {
    const dir = scratch();
    const bundled = path.join(dir, "cua");
    writeFileSync(bundled, "x");
    const blocked = path.join(dir, "file");
    writeFileSync(blocked, "");
    vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(appImageCua(bundled, path.join(blocked, "daemon"), "0.7.2")).toBe(bundled);
  });
});
