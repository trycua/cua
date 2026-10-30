// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Machines page's rows on the real app core (`pnpm native`): they are
// the core's (`machines::page_rows`), devices of their own included, and
// the page gets the same rows. Skipped when this machine's native
// directory was not built.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { createBridge, type Bridge } from "../src/bridge";
import { AppModel } from "../src/model/app-model";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { StartupModel } from "../src/model/startup";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry, fixtureDevices, fixtureHost } from "./fixtures";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

describe.skipIf(!built)("the Machines rows on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let telemetry: FixtureTelemetry;
  let model: AppModel;
  let bridge: Bridge;
  let appDir: string;

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-machine-rows-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  const make = () => {
    const backend = new FixtureSpacesBackend(native);
    telemetry = new FixtureTelemetry();
    model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath: path.join(appDir, "app-settings.json"),
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => fixtureDevices()),
      cloud: new CloudModel(native, () => backend),
      account: new FixtureAccount("ada@example.com"),
      telemetry,
      servicesIn: () => true,
      cpus: 8,
    });
    bridge = createBridge({ model, supervisor: null, version: "1.2.3", platform: "darwin", env: {}, ui: { openSpace: () => {}, setBackground: () => {}, activate: () => {} } } as never);
  };

  beforeEach(() => {
    appDir = mkdtempSync(path.join(home, "app-"));
  });

  const call = (method: string, args: Record<string, unknown> = {}) => bridge.registry.handle(method, args, { window: "win-1" });

  it("lists the core's rows: this machine and its device as one, the devices of their own", async () => {
    make();
    await model.refresh();
    await model.devices.refresh();
    const rows = model.machinePageRows;
    expect(rows[0]).toMatchObject({ id: "this-mac", current: true, device: "dev_mac" });
    expect(rows.filter((r) => r.deviceOnly).map((r) => r.name)).toEqual(["Work laptop"]);
    // The page gets the same rows.
    const answer = (await call("machines.list")) as { rows: { id: string; status: string; spacesLabel: string }[] };
    expect(answer.rows.map((r) => r.id)).toEqual(rows.map((r) => r.id));
    // With the core's status word and Spaces count (CUA-1617), as the
    // SwiftUI list shows them.
    expect(answer.rows.map((r) => [r.status, r.spacesLabel])).toEqual(rows.map((r) => [r.status, r.spacesLabel]));
    expect(rows[0]!.status).toBe("Online");
    expect(rows.find((r) => r.deviceOnly)!.spacesLabel).toBe("");
  });
});
