// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Stable and Local channels (src/channel.ts): which one a process is, and
// that Local shares no name with the released app.
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { LOCAL_EXECUTABLE, localCuaHome, resolveChannel } from "../src/channel";

const STABLE_EXE = "C:\\Users\\me\\AppData\\Local\\Programs\\cua-spaces\\Cua Spaces.exe";
const LOCAL_EXE = "C:\\Users\\me\\AppData\\Local\\Programs\\Cua Spaces Local\\Cua Spaces Local.exe";

describe("resolveChannel", () => {
  it("is Stable for the released executable and an empty environment", () => {
    expect(resolveChannel({}, STABLE_EXE).name).toBe("stable");
  });
  it("is Local for the Local executable, whatever the case", () => {
    expect(resolveChannel({}, LOCAL_EXE).isLocal).toBe(true);
    expect(resolveChannel({}, LOCAL_EXE.toUpperCase()).isLocal).toBe(true);
  });
  it("lets CUA_CHANNEL decide first", () => {
    expect(resolveChannel({ CUA_CHANNEL: "local" }, STABLE_EXE).isLocal).toBe(true);
    expect(resolveChannel({ CUA_CHANNEL: " Stable " }, LOCAL_EXE).isLocal).toBe(false);
  });
  it("does not guess at an unknown CUA_CHANNEL", () => {
    expect(() => resolveChannel({ CUA_CHANNEL: "loca1" }, LOCAL_EXE)).toThrow(/not a Cua channel/);
  });
  it("names the Local package's executable", () => {
    expect(LOCAL_EXECUTABLE).toBe("cua spaces local.exe");
  });
});

describe("Local shares nothing with Stable", () => {
  const stable = resolveChannel({}, STABLE_EXE);
  const local = resolveChannel({}, LOCAL_EXE);
  it("has its own names", () => {
    expect(local.appUserModelId).toBe(`${stable.appUserModelId}.local`);
    expect(local.deepLinkScheme).toBe("cua-local");
    for (const key of ["appName", "appUserModelId", "deepLinkScheme", "registryKey", "userDataDirName", "homeDirName"] as const) {
      expect(local[key]).not.toBe(stable[key]);
    }
  });
  it("keeps Stable's names as released", () => {
    expect(stable.registryKey).toBe("Software\\ai.cua.spaces.desktop");
    expect(local.registryKey).toBe("Software\\ai.cua.spaces.desktop.local");
    expect(stable).toMatchObject({ appName: "Cua Spaces", appUserModelId: "ai.cua.spaces.desktop", deepLinkScheme: "cua", homeDirName: ".cua" });
  });
  it("homes on staging in ~/.cua-local and on prod below it, as CuaChannel::Local does", () => {
    expect(localCuaHome("C:\\Users\\me", {})).toBe("C:\\Users\\me\\.cua-local");
    expect(localCuaHome("C:\\Users\\me", { CUA_ENV: "prod" })).toBe("C:\\Users\\me\\.cua-local\\envs\\prod");
    expect(localCuaHome("C:\\Users\\me", { CUA_ENV: "staging" })).toBe("C:\\Users\\me\\.cua-local");
  });
  it("matches the Rust names", () => {
    const rs = readFileSync(path.join(__dirname, "../../../libs/cua/crates/cua-home/src/channel.rs"), "utf8");
    expect(rs).toContain(`"${local.homeDirName}"`);
    expect(rs).toContain(`"${local.deepLinkScheme}"`);
  });
});
