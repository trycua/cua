// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { flipFuses, FuseV1Options, FuseVersion, getCurrentFuseWire } from "@electron/fuses";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
// @ts-expect-error: plain .mjs script, no types
import { checkWire, fuseFile, readWire } from "../scripts/check-fuses.mjs";

const require = createRequire(import.meta.url);
const { RELEASE_FUSES, electronFuses } = require("../packaging/fuses.cjs");
const SENTINEL = "dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX";

const dirs: string[] = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

/** A file with a fuse wire as Electron ships it (run-as-node, inspect and NODE_OPTIONS on). */
function fakeElectron(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "fuses-"));
  dirs.push(dir);
  const file = path.join(dir, "electron");
  writeFileSync(file, Buffer.concat([Buffer.from("\x7fELF junk"), Buffer.from(SENTINEL), Buffer.from([1, 8]), Buffer.from("10110001"), Buffer.from("tail")]));
  return file;
}

describe("Electron fuses", () => {
  it("are turned on in the release config, and off with CUA_SPACES_NO_FUSES=1", () => {
    const configPath = require.resolve("../electron-builder.config.cjs");
    const load = (env: Record<string, string | undefined>) => {
      const saved = { ...process.env };
      for (const [k, v] of Object.entries(env)) {
        if (v === undefined) delete process.env[k];
        else process.env[k] = v;
      }
      delete require.cache[configPath];
      try {
        return require(configPath);
      } finally {
        process.env = saved;
        delete require.cache[configPath];
      }
    };
    const fuses = load({ CUA_SPACES_NO_FUSES: undefined, CSC_LINK: undefined, CSC_NAME: undefined }).electronFuses;
    expect(fuses).toEqual({
      resetAdHocDarwinSignature: true,
      runAsNode: false,
      enableCookieEncryption: true,
      enableNodeOptionsEnvironmentVariable: false,
      enableNodeCliInspectArguments: false,
      enableEmbeddedAsarIntegrityValidation: true,
      onlyLoadAppFromAsar: true,
      grantFileProtocolExtraPrivileges: false,
    });
    // Signed macOS builds are signed after the flip; no ad hoc reset.
    expect(load({ CSC_NAME: "Developer ID Application: Test" }).electronFuses.resetAdHocDarwinSignature).toBe(false);
    expect(load({ CUA_SPACES_NO_FUSES: "1" }).electronFuses).toBeNull();
  });

  it("each fuse sits at @electron/fuses' index", () => {
    const byName: Record<string, number> = {
      runAsNode: FuseV1Options.RunAsNode,
      enableCookieEncryption: FuseV1Options.EnableCookieEncryption,
      enableNodeOptionsEnvironmentVariable: FuseV1Options.EnableNodeOptionsEnvironmentVariable,
      enableNodeCliInspectArguments: FuseV1Options.EnableNodeCliInspectArguments,
      enableEmbeddedAsarIntegrityValidation: FuseV1Options.EnableEmbeddedAsarIntegrityValidation,
      onlyLoadAppFromAsar: FuseV1Options.OnlyLoadAppFromAsar,
      grantFileProtocolExtraPrivileges: FuseV1Options.GrantFileProtocolExtraPrivileges,
    };
    for (const [name, { index }] of Object.entries(RELEASE_FUSES) as [string, { index: number }][]) expect(index, name).toBe(byName[name]);
  });

  it("check-fuses fails a stock binary and passes it once flipped as electron-builder does", async () => {
    const file = fakeElectron();
    const before = checkWire(readWire(readFileSync(file)));
    expect(before).toContain("runAsNode: on, expected off");
    expect(before).toContain("onlyLoadAppFromAsar: off, expected on");

    // What electron-builder's generateFuseConfig hands @electron/fuses.
    const { resetAdHocDarwinSignature: _, ...fuses } = electronFuses({ resetAdHocSignature: false });
    const config: Record<string, unknown> = { version: FuseVersion.V1 };
    for (const [name, on] of Object.entries(fuses)) config[RELEASE_FUSES[name].index] = on;
    await flipFuses(file, config as Parameters<typeof flipFuses>[1]);

    expect(checkWire(readWire(readFileSync(file)))).toEqual([]);
    const wire = await getCurrentFuseWire(file);
    expect(wire[FuseV1Options.RunAsNode]).toBe(48);
    expect(wire[FuseV1Options.OnlyLoadAppFromAsar]).toBe(49);
  });

  it("finds the wire in a real Electron and reads it like @electron/fuses", async () => {
    const stock = process.platform === "darwin" ? path.join(__dirname, "../node_modules/electron/dist/Electron.app") : path.join(__dirname, "../node_modules/electron/dist/electron");
    if (!existsSync(fuseFile(stock))) return;
    const dir = mkdtempSync(path.join(tmpdir(), "fuses-"));
    dirs.push(dir);
    const copy = path.join(dir, "Electron Framework");
    copyFileSync(fuseFile(stock), copy);
    const mine = readWire(readFileSync(copy));
    const theirs = await getCurrentFuseWire(copy);
    expect(mine.version).toBe(1);
    expect(mine.states[0]).toBe(theirs[FuseV1Options.RunAsNode] === 49 ? "on" : "off");
    // Stock Electron: run-as-node on, which is why release builds flip it.
    expect(checkWire(mine)).not.toEqual([]);
  });

  it("no wire is a failure", () => {
    expect(checkWire(readWire(Buffer.from("nothing here")))).toEqual(["no fuse wire found"]);
  });
});
