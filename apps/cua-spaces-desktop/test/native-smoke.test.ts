// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The real native layer (`pnpm native`), loaded in Node as the main process
// loads it, with a throwaway HOME and CUA_HOME: the app core answers, and
// the SDK lists no Spaces in an empty cua home without a daemon. Skipped
// when this machine's native directory was not built.
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

describe.skipIf(!built)("the native layer in Node", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-native-smoke-"));
    Object.assign(process.env, {
      HOME: home,
      USERPROFILE: home,
      CUA_HOME: path.join(home, ".cua"),
      CUA_TELEMETRY: "0",
      DO_NOT_TRACK: "1",
      CUA_KEYCHAIN_NONINTERACTIVE: "1",
    });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  it("answers the app core", () => {
    expect(native.appStatusLine(0)).toEqual(expect.any(String));
    expect(["arm64", "amd64"]).toContain(native.appHostArch());
    expect(native.appCreatesIsPending("pending:1")).toBe(true);
    expect(native.appRowsToSpaces([], BigInt(Date.now()))).toEqual([]);
  });

  it("gives flat enums their Swift case names", () => {
    expect(native.AppSpaceOs.Macos).toBe("macos");
    expect(native.CuaMode.Daemon).toBe("daemon");
  });

  it("lists no Spaces in an empty cua home, with no daemon", async () => {
    const cua = native.Cua.auto(native.CuaConfig.create({ fleetFromEnv: false, fleetFromSession: false, spacesHome: path.join(home, ".cua") }));
    expect(cua.mode()).toBe(native.CuaMode.Embedded);
    expect(await cua.spaces().list()).toEqual([]);
  });

  it("is the notch's core: plain { tag, inner } events go through its reducer", async () => {
    const { wireView } = await import("../src/notch/core");
    const core = native as unknown as import("../src/notch/core").NotchCore;
    const start = core.appNotchInitial();
    const shown = core.appNotchReduce(start, { tag: "Visibility", inner: { shown: true } });
    const hovered = core.appNotchReduce(shown.state, { tag: "HoverEnter" });
    expect(hovered.effects.map((e) => e.tag)).toContain("StartDwell");
    const opened = core.appNotchReduce(hovered.state, { tag: "Click" });
    expect(opened.state.open).toBe(true);
    expect(wireView(core.appNotchView(opened.state, native.appRowsToSpaces([], 0n))).phase).toBe("tiles");
  });

  it("refuses a second directory once loaded", async () => {
    await expect(loadNative(path.join(home, "elsewhere"))).rejects.toThrow(/already loaded/);
  });
});
