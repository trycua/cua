// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Machines page (CUA-1673) on the real app core (`pnpm native`): the
// view the page draws (`machines_view::view`) is the core's, the same the
// SwiftUI page draws, from what this app has; `machines.list` carries it in
// the shape the web contract reads; and the page's own actions, rename and
// Remove from account, reach the relay and the Spaces. Skipped when this
// machine's native directory was not built.
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import { SHAPE_DEFS, WEBKIT_SHAPES } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { createBridge, type Bridge } from "../src/bridge";
import { encode } from "../src/bridge/value";
import { AppModel } from "../src/model/app-model";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { StartupModel } from "../src/model/startup";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry, fixtureHost } from "./fixtures";
import { FixtureDevices } from "./host-fixtures";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);
const PARITY = path.resolve(__dirname, "../../../libs/cua/crates/cua-spaces-app-core/parity");

describe.skipIf(!built)("the Machines page on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let model: AppModel;
  let bridge: Bridge;
  let backend: FixtureSpacesBackend;
  let devices: FixtureDevices;

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-machines-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  beforeEach(() => {
    const appDir = mkdtempSync(path.join(home, "app-"));
    backend = new FixtureSpacesBackend(native);
    devices = new FixtureDevices();
    model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath: path.join(appDir, "app-settings.json"),
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => devices.like),
      cloud: new CloudModel(native, () => backend),
      account: new FixtureAccount("ada@example.com"),
      telemetry: new FixtureTelemetry(),
      servicesIn: () => true,
      cpus: 8,
      checklistPath: null,
      about: () => ({ computerName: "Ada's MacBook Pro", appVersion: "0.9.1", osVersion: "macOS 26.5.2" }),
    });
    model.devices.signedIn = true;
    bridge = createBridge({ model, supervisor: null, version: "1.2.3", platform: "darwin", env: {}, ui: { openSpace: () => {}, setBackground: () => {}, activate: () => {} } } as never);
  });

  const call = (method: string, args: Record<string, unknown> = {}) => bridge.registry.handle(method, args, { window: "win-1" });

  it("draws the approved mockup as the core's golden has it", () => {
    const fixture = JSON.parse(readFileSync(path.join(PARITY, "machines.json"), "utf8")) as { views: { input: unknown }[] };
    const golden = JSON.parse(readFileSync(path.join(PARITY, "golden/machines.json"), "utf8")) as {
      frames: { view?: string; summary?: string; groups?: { group: string; rows: string[] }[] }[];
    };
    const expected = golden.frames.find((f) => f.view)!;
    const view = native.appMachinesView(native.appMachinesViewInputFromJson(JSON.stringify(fixture.views[0]!.input)));
    expect(view.summary).toBe(expected.summary);
    const byId = new Map(view.machines.map((m) => [m.id, m] as const));
    const rows = view.groups.map((g) =>
      g.ids.map((id) => {
        const m = byId.get(id)!;
        return `${m.online ? "●" : "○"} ${m.name}${m.badge ? ` [${m.badge}]` : ""} — ${m.line}`;
      }),
    );
    expect(rows).toEqual(expected.groups!.map((g) => g.rows));
  });

  it("draws the mockup with issues as the core's golden has it: tinted rows and each machine's issues", () => {
    const fixture = JSON.parse(readFileSync(path.join(PARITY, "machines.json"), "utf8")) as { views: { name: string; input: unknown }[] };
    const golden = JSON.parse(readFileSync(path.join(PARITY, "golden/machines.json"), "utf8")) as {
      frames: { view?: string; summary?: string; groups?: { group: string; rows: string[] }[]; pages?: string[][] }[];
    };
    const v = fixture.views.find((x) => x.name.startsWith("the mockup with issues"))!;
    const expected = golden.frames.find((f) => f.view === v.name)!;
    const view = native.appMachinesView(native.appMachinesViewInputFromJson(JSON.stringify(v.input)));
    const byId = new Map(view.machines.map((m) => [m.id, m] as const));
    const rows = view.groups.map((g) =>
      g.ids.map((id) => {
        const m = byId.get(id)!;
        return `${m.online ? "●" : "○"} ${m.name}${m.badge ? ` [${m.badge}]` : ""} — ${m.line}${m.health ? ` [${m.health}]` : ""}`;
      }),
    );
    expect(rows).toEqual(expected.groups!.map((g) => g.rows));
    // Each page's issue lines, as parity.rs's view_frame writes them.
    const issueLines = view.machines.map((m) =>
      m.issues.map((i) => `${m.issuesTitle.toLowerCase()} ${i.level}: ${i.text}${i.action ? ` [${i.action.label}]` : ""}`),
    );
    expect(issueLines).toEqual(expected.pages!.map((p) => p.filter((l) => l.startsWith("issues "))));
    expect(issueLines.flat().length).toBeGreaterThan(0);
  });

  it("checks the machines that can be checked, keeps their answers for the view, and drops what is no longer current", async () => {
    await model.refresh();
    await model.devices.refresh();
    const url = "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";
    backend.health.set("this", {
      cuaVersion: "0.4.1",
      checks: [{ id: "machine.local_network", status: "fail", message: "blocked", remediation: "", facts: new Map([["app", "Cua Spaces"], ["settings_url", url]]) }],
    });
    const targets = model.machinesView.machines.filter((m) => m.healthTarget !== undefined && m.healthTarget !== null);
    expect(targets.some((m) => m.current)).toBe(true);
    await expect(call("machines.checkHealth")).resolves.toBeNull();
    // This machine through its own daemon (no machine), the others by id; never an offline one.
    expect(backend.healthAsked).toContain(null);
    const offline = model.machinesView.machines.filter((m) => !m.current && !m.online).map((m) => m.id);
    for (const asked of backend.healthAsked) expect(offline).not.toContain(asked);
    expect(backend.healthAsked.length).toBe(targets.length);
    const me = model.machinesView.machines.find((m) => m.current)!;
    expect(me.health).toBe("fail");
    expect(me.issues.map((i) => [i.id, i.action?.label, i.action?.url])).toEqual([["machine.local_network", "Open Local Network settings", url]]);
    // The list carries them in the web contract's shape.
    const answer = (await call("machines.list")) as unknown;
    expect(validate(WEBKIT_SHAPES["machines.list"]!, answer, SHAPE_DEFS)).toEqual([]);

    // An answer for a machine that is no longer a target is dropped.
    model.machineHealth.set("gone", { id: "gone", cuaVersion: "0.4.1", checks: [], error: undefined });
    backend.health.set("this", "the daemon is not running");
    await call("machines.checkHealth");
    expect(model.machineHealth.has("gone")).toBe(false);
    const again = model.machinesView.machines.find((m) => m.current)!;
    expect(again.issues[0]?.id).toBe("unchecked");
    expect(again.issues[0]?.text).toContain("the daemon is not running");
  });

  it("lists the view from this app's rows, with whose each machine is, and sends it in the web contract's shape", async () => {
    await model.refresh();
    await model.devices.refresh();
    const view = model.machinesView;
    // The same machines as the rows, This machine first, named after this computer.
    expect(view.machines.map((m) => m.id).sort()).toEqual(model.machinePageRows.map((r) => r.id).sort());
    const me = view.machines.find((m) => m.current)!;
    expect(me.name).not.toBe("");
    expect(me.details.find((d) => d.label === "App")?.value).toBe("0.9.1");
    expect(me.newSpace.on).toBe("local");
    const answer = (await call("machines.list")) as { view: { machines: { id: string; toggles: { actions: string[] }[] }[] } };
    expect(answer.view).toEqual(encode(view));
    expect(validate(WEBKIT_SHAPES["machines.list"]!, answer, SHAPE_DEFS)).toEqual([]);
    // Enums as the SwiftUI host's BridgeValue names them (the case name;
    // the web adapter turns them into the bridge's words).
    const actions = answer.view.machines.flatMap((m) => m.toggles.flatMap((t) => t.actions));
    for (const a of actions) expect(a).toMatch(/^[a-z]+[A-Za-z]*$/);
  });

  it("renames a machine on the relay; this machine by its own relay id", async () => {
    await model.refresh();
    await expect(call("machines.rename", { id: "m2", name: "  Build box  " })).resolves.toBeNull();
    expect(devices.calls).toContain("rename-machine:m2:Build box");
    model.host.machineId = "self-id";
    await call("machines.rename", { id: "this-mac", name: "Ada's Mac" });
    expect(devices.calls).toContain("rename-machine:self-id:Ada's Mac");
    await expect(call("machines.rename", { id: "m2", name: "   " })).rejects.toMatchObject({ code: "bad_args" });
  });

  it("removes a machine from the account: off the relay, then its device revoked", async () => {
    await model.refresh();
    await expect(call("machines.remove", { machine: "m2", device: "dev_old" })).resolves.toBeNull();
    expect(backend.unregistered).toEqual(["m2"]);
    expect(devices.calls).toContain("revoke:dev_old");
    await expect(call("machines.remove", {})).rejects.toMatchObject({ code: "bad_args" });
    // Signed out: no relay to ask.
    model.devices.signedIn = false;
    const signedOut = new DevicesModel(native, () => null);
    Object.assign(model, { devices: signedOut });
    await expect(call("machines.remove", { machine: "m2" })).rejects.toMatchObject({ code: "unsupported" });
  });
});
