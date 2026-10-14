// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The getting-started checklist's one copy and the Machines page's rows on
// the real app core (`pnpm native`), as the SwiftUI app's
// ChecklistPaletteTests check them: the app ticks steps off from what it
// shows, the page reads and changes the same copy, progress the page kept
// before is merged once, and the Machines rows are the core's
// (`machines::page_rows`), devices of their own included. Skipped when
// this machine's native directory was not built.
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

describe.skipIf(!built)("the checklist and the Machines rows on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let telemetry: FixtureTelemetry;
  let model: AppModel;
  let bridge: Bridge;
  let appDir: string;

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-checklist-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  const make = (seed: string | null = null) => {
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
      checklistPath: path.join(appDir, "checklist.json"),
      checklistSeedPath: seed,
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
    const answer = (await call("machines.list")) as { rows: { id: string }[] };
    expect(answer.rows.map((r) => r.id)).toEqual(rows.map((r) => r.id));
  });

  it("ticks steps off from what the app shows, and the page changes the same copy", async () => {
    make();
    await model.refresh();
    await model.devices.refresh();
    model.changed("machines");
    const done = () => ((call("checklist.get") as unknown as Promise<{ state: { done: string[] } }>).then((r) => r.state.done));
    expect(await done()).toEqual(expect.arrayContaining(["add_machine", "create_space"]));
    await call("checklist.send", { action: { type: "done", step: "teleport" } });
    expect(await done()).toContain("teleport");
    // Kept between launches.
    expect(JSON.parse(readFileSync(path.join(appDir, "checklist.json"), "utf8")).done).toContain("teleport");
    // The page never sends what the app shows.
    await expect(call("checklist.send", { action: { type: "observe", otherMachine: true } })).rejects.toMatchObject({ code: "bad_args" });
    // A run in a Space ticks "Run an agent".
    model.sawAgentRuns(1);
    expect(await done()).toContain("run_agent");
  });

  it("merges what the page kept before without its events again", async () => {
    make();
    const before = telemetry.recorded.length;
    const r = (await call("checklist.send", { action: { type: "merge", state: { done: ["share", "nope"], dismissed: true, shown: true } } })) as {
      state: { done: string[]; dismissed: boolean };
    };
    expect(r.state.done).toEqual(["share"]);
    expect(r.state.dismissed).toBe(true);
    expect(telemetry.recorded.length).toBe(before);
  });

  it("starts from the SwiftUI app's checklist when it has none of its own", () => {
    const swift = path.join(home, "swift");
    mkdirSync(swift, { recursive: true });
    writeFileSync(path.join(swift, "checklist.json"), JSON.stringify({ done: ["add_machine", "teleport"], dismissed: false, shown: true }));
    make(path.join(swift, "checklist.json"));
    expect(model.checklist.stored).toEqual({ done: ["add_machine", "teleport"], dismissed: false, shown: true });
  });
});
