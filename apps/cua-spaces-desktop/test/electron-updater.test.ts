// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The real electron-updater, imported the way the CommonJS main bundle
// does: Node's own `import()` of the external package, in a plain Node
// process (vitest's import adds the named exports Node does not). Its
// `autoUpdater` getter is never read: constructing the updater needs Electron.
import { execFileSync } from "node:child_process";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { electronUpdater } from "../src/electron-updater";

const loader = path.join(__dirname, "../src/electron-updater.ts");

const inNode = (script: string) =>
  JSON.parse(execFileSync(process.execPath, ["--experimental-strip-types", "--no-warnings", "--input-type=module", "-e", script], { cwd: path.dirname(loader), encoding: "utf8" }));

describe("electronUpdater", () => {
  it("finds autoUpdater where Node's import() of the package puts it", () => {
    const seen = inNode(`
      const raw = await import("electron-updater");
      const { electronUpdater } = await import(${JSON.stringify(loader)});
      const mod = await electronUpdater();
      console.log(JSON.stringify({
        named: Object.keys(raw).includes("autoUpdater"),
        getter: typeof Object.getOwnPropertyDescriptor(mod, "autoUpdater")?.get,
      }));
    `);
    // The trap: no named `autoUpdater` (Windows and Linux Check Now failed on it).
    expect(seen).toEqual({ named: false, getter: "function" });
  });

  it("takes a module that names autoUpdater as it is", async () => {
    const autoUpdater = { channel: "latest" };
    expect((await electronUpdater(async () => ({ autoUpdater }))).autoUpdater).toBe(autoUpdater);
    expect((await electronUpdater(async () => ({ default: { autoUpdater } }))).autoUpdater).toBe(autoUpdater);
    await expect(electronUpdater(async () => ({}))).rejects.toThrow("no autoUpdater");
  });
});
