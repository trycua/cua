// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Teleport's run on the real native layer (`pnpm native`): the arguments
// the bridge's `teleport.run` passes (a plan, a Space, the consent and a
// progress listener written in TypeScript) lower into the library. The
// listener is optional, so it goes inside an optional's buffer; the UniFFI
// runtime used to lower a TypeScript object only as a bare argument and
// failed there with "Cannot lower this object to a pointer" before the run
// started (patches/@ubjs__core@0.31.0-3.patch). The native part is skipped
// when this machine's native directory was not built.
import { FfiConverterObjectWithCallbacks, FfiConverterOptional, type UniffiObjectFactory } from "@ubjs/core";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { teleportConsent } from "../src/model/teleport";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import type { TeleportPlan, TeleportRunEvent } from "../src/native/generated/index";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

describe("an object written in TypeScript, inside an optional", () => {
  it("lowers to a foreign handle, as a bare argument does", () => {
    const factory = { isConcreteType: () => false } as unknown as UniffiObjectFactory<{ onEvent(): void }>;
    const listener = { onEvent() {} };
    const converter = new FfiConverterObjectWithCallbacks(factory);
    const bytes = new FfiConverterOptional(converter).lower(listener, (n) => new Uint8Array(n));
    // Some(handle): the tag, then a u64 handle that lifts back to the listener.
    expect(bytes[0]).toBe(1);
    const handle = new DataView(bytes.buffer, bytes.byteOffset + 1, 8).getBigUint64(0);
    expect(converter.lift(handle)).toBe(listener);
  });
});

describe.skipIf(!built)("Teleport's run on the native layer", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  /** A direct Space with only a service URL: the SDK makes its Space without reaching it. */
  const SPACE = "direct:127.0.0.1:9";

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-teleport-native-"));
    const cuaHome = path.join(home, ".cua");
    mkdirSync(cuaHome, { recursive: true, mode: 0o700 });
    writeFileSync(path.join(cuaHome, "spaces.json"), JSON.stringify([{ id: SPACE, name: "Fixture" }]));
    writeFileSync(path.join(cuaHome, "spaces-credentials.json"), JSON.stringify({ [SPACE]: { service_urls: { mcp: "http://127.0.0.1:9/mcp" } } }), { mode: 0o600 });
    Object.assign(process.env, {
      HOME: home,
      USERPROFILE: home,
      CUA_HOME: cuaHome,
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

  it("lowers the bridge's arguments, listener included, and reaches the run", async () => {
    const cua = native.Cua.auto(native.CuaConfig.create({ fleetFromEnv: false, fleetFromSession: false, spacesHome: process.env.CUA_HOME }));
    expect(cua.mode()).toBe(native.CuaMode.Embedded);
    const space = await cua.spaces().space(SPACE);
    const teleport = native.teleport(cua);
    const plan: TeleportPlan = {
      app: {
        id: "fixture",
        name: "Fixture",
        capability: native.TeleportCapability.InstallOnly,
        moves: [native.TeleportMove.AppOnly],
        sensitiveGroups: [],
        json: "{}",
      },
      spaceId: SPACE,
      moves: native.TeleportMove.AppOnly,
      steps: [],
      consent: [],
      sensitive: false,
      totalBytes: 0n,
      warnings: [],
      relayUnsealed: false,
      // Not a plan the SDK made: the run refuses it, after every argument crossed.
      json: "{}",
    };
    const events: TeleportRunEvent[] = [];
    const consent = teleportConsent({ approved: true, cookieDomains: ["example.com"] });
    const run = teleport.run(plan, space, consent, { onEvent: (e) => void events.push(e) });
    await expect(run).rejects.toThrow(/not a plan from this SDK/);
    await expect(run).rejects.not.toThrow(/Cannot lower/);
    expect(events).toEqual([]);
  });
});
