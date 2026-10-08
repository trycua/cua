// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The bridge on the real app core (`pnpm native`), over fixture Spaces, host,
// devices and account (the SwiftUI app's BridgeContractTests): every ported
// method answers in the shapes the page reads (bridge-shapes.json, the
// document the Swift tests check too), the create, cancel, power and delete
// paths run the core's state machines, sign-in moves through its phases,
// events go out when what the page shows changed, and the page's adapter
// reads every answer into its contract. Skipped when this machine's native
// directory was not built.
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { validate, type Schema } from "../../cua-spaces-web/src/bridge/contracts/schema";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { HOST_COVERAGE } from "../../cua-spaces-web/src/bridge/coverage";
import { OPERATIONS, type OpArgs, type OpName } from "../../cua-spaces-web/src/bridge/protocol";
import { createBridge, type Bridge } from "../src/bridge";
import type { BridgeEvent } from "../src/bridge/host";
import { createOptions } from "../src/bridge/spaces";
import { telemetrySignal } from "../src/bridge/telemetry";
import { AppModel } from "../src/model/app-model";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { StartupModel, startupState } from "../src/model/startup";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry, fixtureDevices, fixtureHost } from "./fixtures";
import { AGENT_METHODS } from "./agents-fixtures";
import { HOST_METHODS, HOST_OPS } from "./host-fixtures";
import { NOT_YET } from "./not-yet";
import { VAULT_METHODS, VAULT_OPS } from "./vault-fixtures";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

const SHAPES = JSON.parse(readFileSync(path.resolve(__dirname, "../../cua-spaces-web/src/bridge/contracts/bridge-shapes.json"), "utf8")) as {
  webkit: Record<string, Schema>;
  $defs: Record<string, Schema>;
};
const json = (v: unknown) => JSON.parse(JSON.stringify({ v })).v as unknown;
const shapeProblems = (method: string, answer: unknown) => validate(SHAPES.webkit[method]!, json(answer), SHAPES.$defs).map((e) => `${method} ${e}`);
/** The page's Electron adapter. Loaded by path: the web bridge's React modules
 * behind it do not typecheck in this project (no JSX or React types here). */
const ELECTRON_ADAPTER = path.resolve(__dirname, "../../cua-spaces-web/src/bridge/adapters/electron.ts");
interface PageAdapter {
  call(op: string, args: unknown): Promise<unknown>;
  dispose?(): void;
}
const pageAdapter = async (win: unknown) =>
  ((await import(/* @vite-ignore */ ELECTRON_ADAPTER)) as { createElectronAdapter(win: unknown): PageAdapter }).createElectronAdapter(win);

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

async function until(f: () => boolean, ms = 2000) {
  const deadline = Date.now() + ms;
  while (!f()) {
    if (Date.now() > deadline) throw new Error("timed out waiting");
    await tick(5);
  }
}

describe.skipIf(!built)("the bridge on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let backend: FixtureSpacesBackend;
  let account: FixtureAccount;
  let telemetry: FixtureTelemetry;
  let model: AppModel;
  let bridge: Bridge;
  let events: BridgeEvent[];
  let opened: [string, string][];
  let backgrounds: [unknown, string, string | null][];
  const pipsOpen = new Map<string, () => void>();

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-bridge-native-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  beforeEach(async () => {
    backend = new FixtureSpacesBackend(native);
    account = new FixtureAccount("ada@example.com");
    telemetry = new FixtureTelemetry();
    const settingsPath = path.join(mkdtempSync(path.join(home, "app-")), "app-settings.json");
    model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath,
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => fixtureDevices()),
      cloud: new CloudModel(native, () => backend),
      account,
      telemetry,
      servicesIn: () => true,
      cpus: 8,
    });
    model.createAcceptTimeout = 5;
    opened = [];
    backgrounds = [];
    bridge = createBridge({
      model,
      supervisor: null,
      version: "1.2.3",
      platform: "darwin",
      env: { CUA_HOME: path.join(home, ".cua") },
      ui: {
        openSpace: (id, name) => opened.push([id, name]),
        setBackground: (win, color, appearance) => backgrounds.push([win, color, appearance]),
        activate: () => {},
        chooseFiles: async () => ["/Users/ada/Desktop/picked.txt"],
        pip: { open: (spec, closed) => pipsOpen.set(`${spec.spaceId}|${spec.key}`, closed), close: (id, key) => pipsOpen.delete(`${id}|${key}`) },
      },
    });
    events = [];
    bridge.events.subscribe((e) => events.push(e));
    await model.refresh();
    await model.devices.refresh();
  });

  const call = (method: string, args: Record<string, unknown> = {}) => bridge.registry.handle(method, args, { window: "win-1" });
  const code = (method: string, args: Record<string, unknown> = {}) =>
    bridge.registry.dispatch({ id: "x", method, args }).then((r) => (r.ok ? null : r.error.code));

  it("answers every ported method in the SwiftUI host's shapes", async () => {
    const space = model.spaces.find((s) => s.id === "local:aurora")!.id;
    const cases: [string, Record<string, unknown>][] = [
      ["app.info", {}],
      ["session.get", {}],
      ["spaces.list", {}],
      ["spaces.open", { id: space }],
      ["spaces.createOptions", {}],
      ["spaces.cancelCreate", { pendingId: "pending:none" }],
      ["spaces.setPower", { id: space, on: true }],
      ["machines.list", {}],
      ["host.status", {}],
      ["telemetry.track", { signals: [{ type: "step", step: "app_launched", ok: true }] }],
      ["window.setBackgroundColor", { color: "#16181c" }],
      ["window.setDragRegions", { rects: [{ x: 78, y: 0, width: 400, height: 38 }] }],
      ["startup.get", {}],
      ["startup.act", { action: "tryAgain" }],
      ["spaces.usage", { spaceId: space }],
      ["spaces.windows", { spaceId: space }],
      ["stream.pip", { spaceId: space, command: { type: "open", row: "w-1" } }],
      ["spaces.thumbnail", { spaceId: space }],
      ["spaces.chooseFiles", {}],
      ["spaces.droppedFiles", { names: ["notes.txt"], paths: ["/Users/ada/notes.txt"] }],
      ["spaces.sendFiles", { spaceId: space, paths: ["/Users/ada/notes.txt"] }],
      ["sharing.list", { spaceId: space }],
      ["sharing.share", { spaceId: space, who: "bo@example.com", role: "viewer" }],
      ["sharing.unshare", { spaceId: space, who: "bo@example.com" }],
      ["spaces.add", { url: "http://studio.local:7400", token: "", name: "studio" }],
      ["clouds.status", {}],
      ["clouds.test", { target: { provider: "aws", region: "us-west-2" } }],
      ["clouds.connect", { target: { provider: "aws", region: "us-west-2" }, makeDefault: false }],
      ["session.signOut", {}],
    ];
    const problems: string[] = [];
    for (const [method, args] of cases) problems.push(...shapeProblems(method, await call(method, args)));
    expect(problems).toEqual([]);
    // Every ported method has its case (or is checked below).
    // (Teleport and the Keyvault: vault-native.test.ts; This machine, Devices, Settings and the rest: host-bridge.test.ts.)
    const elsewhere = new Set(["session.signIn", "spaces.create", "spaces.delete", "spaces.openStream", ...AGENT_METHODS, ...VAULT_METHODS, ...HOST_METHODS]);
    const ported = bridge.registry.methods.filter((m) => !NOT_YET.includes(m) && !elsewhere.has(m));
    expect(ported.filter((m) => !cases.some(([c]) => c === m))).toEqual([]);
  });

  it("lists this host and its methods in app.info", async () => {
    const info = (await call("app.info")) as { platform: string; host: string; version: string; methods: string[] };
    expect(info).toMatchObject({ platform: "macos", host: "electron", version: "1.2.3" });
    expect(info.methods[0]).toBe("app.info");
    expect(info.methods.at(-1)).toBe("spaces.openStream");
  });

  it("answers New Space's options while starting and once ready", () => {
    expect(shapeProblems("spaces.createOptions", createOptions(model.knownNewSpaceEnv(), null, true))).toEqual([]);
    expect(createOptions(model.knownNewSpaceEnv(), null, true)).toMatchObject({ pending: true, macosVmsRunning: null });
  });

  it("gives every launch state its shape", () => {
    for (const phase of [{ kind: "starting" }, { kind: "needsKeychain", locked: false }, { kind: "startFailed" }, { kind: "ready" }] as const) {
      expect(shapeProblems("startup.get", startupState(new StartupModel(phase)))).toEqual([]);
    }
  });

  it("lists This machine first, then the registry's Spaces, with their details", async () => {
    const list = (await call("spaces.list")) as { loaded: boolean; spaces: { space: { id: string; os: string; status: string }; detail: { title: string } }[] };
    expect(list.loaded).toBe(true);
    // In the core's order; This machine is there because this app manages a host.
    expect(list.spaces.map((s) => s.space.id).sort()).toEqual(["cloud:builder", "direct:10.0.0.5:3211", "local:aurora", "this-mac"]);
    const aurora = list.spaces.find((s) => s.space.id === "local:aurora")!;
    expect(aurora.space).toMatchObject({ os: "linux", status: "running" });
    expect(typeof aurora.detail.title).toBe("string");
  });

  it("creates through the core's create: progress, then the new Space", async () => {
    const config = { image: "ghcr.io/trycua/linux:24.04", kind: "container", on: "local", name: "dev" };
    const made = (await call("spaces.create", { pendingId: "pending:1", config, os: "linux" })) as { id: string; name: string };
    expect(made).toMatchObject({ id: "local:dev", name: "dev" });
    expect(backend.created[0]).toMatchObject({ image: config.image, on: "local", kind: "container", runtime: "auto", name: "dev", spacesd: true });
    const progress = events.filter((e) => e.event === "spaces.createProgress").map((e) => e.payload as { pendingId: string; phase: string; bytesTotal: number | null });
    expect(progress.map((p) => p.phase)).toEqual(["preparing", "pulling", "ready"]);
    expect(progress[1]).toMatchObject({ pendingId: "pending:1", bytesTotal: 10 });
    expect(model.spaces.some((s) => s.id === "local:dev")).toBe(true);
    expect(model.spaces.some((s) => s.id === "pending:1")).toBe(false);
  });

  it("refuses a create it cannot run", async () => {
    expect(await code("spaces.create", { pendingId: "nope", config: { image: "x", kind: "container" } })).toBe("bad_args");
    expect(await code("spaces.create", { pendingId: "pending:2", config: { image: "x", kind: "box" } })).toBe("bad_args");
    expect(await code("spaces.create", { pendingId: "pending:2", config: { kind: "vm" } })).toBe("bad_args");
    expect(await code("spaces.create", { pendingId: "pending:2", config: { image: "x", kind: "vm", runtime: "xen" } })).toBe("bad_args");
  });

  it("cancels a create still running: the row goes and the create answers cancelled", async () => {
    let release = () => {};
    backend.hold = new Promise<void>((r) => (release = r));
    const create = bridge.registry.dispatch({ id: "c", method: "spaces.create", args: { pendingId: "pending:3", config: { image: "linux", kind: "container", name: "gone" }, os: "linux" } });
    await until(() => model.spaces.some((s) => s.id === "pending:3"));
    expect(await call("spaces.cancelCreate", { pendingId: "pending:3" })).toEqual({ id: "pending:3", state: "cancelled", message: "" });
    await until(() => backend.cancelled.has("pending:3"));
    release();
    expect(await create).toMatchObject({ id: "c", ok: false, error: { code: "cancelled" } });
    await until(() => !model.spaces.some((s) => s.id === "pending:3"));
    expect(await call("spaces.cancelCreate", { pendingId: "pending:none" })).toMatchObject({ state: "not_creating" });
    expect(await call("spaces.cancelCreate", { pendingId: "local:aurora" })).toMatchObject({ state: "already_created" });
  });

  it("keeps a failed create on its row, in the SDK's words", async () => {
    backend.createError = new Error("CuaError.Runtime: no room on disk");
    Object.assign(backend.createError, { [Symbol.for("typeName")]: "CuaError", tag: "Runtime" });
    const reply = await bridge.registry.dispatch({ id: "f", method: "spaces.create", args: { pendingId: "pending:4", config: { image: "linux", kind: "container" }, os: "linux" } });
    expect(reply).toMatchObject({ ok: false, error: { code: "failed", message: "no room on disk" } });
    expect(model.creates.pending.find((p) => p.id === "pending:4")?.error).toBe("no room on disk");
  });

  it("turns a Space off and deletes one, answering the list at once", async () => {
    await call("spaces.setPower", { id: "local:aurora", on: false });
    expect(backend.power).toEqual([{ id: "local:aurora", on: false }]);
    await until(() => model.spaces.find((s) => s.id === "local:aurora")?.status === native.AppSpaceStatus.Suspended);
    const answer = (await call("spaces.delete", { id: "direct:10.0.0.5:3211", removeOnly: true })) as { spaces: { space: { id: string }; deleting: boolean }[] };
    expect(answer.spaces.find((s) => s.space.id === "direct:10.0.0.5:3211")?.deleting).toBe(true);
    await until(() => backend.removed.length === 1);
    expect(backend.removed[0]).toMatchObject({ id: "direct:10.0.0.5:3211", removeOnly: true });
    await until(() => !model.spaces.some((s) => s.id === "direct:10.0.0.5:3211"));
    expect(await code("spaces.delete", { id: "local:nope" })).toBe("not_found");
    expect(await code("spaces.setPower", { id: "local:aurora" })).toBe("bad_args");
  });

  it("opens a Space's window by id, and only a listed one", async () => {
    expect(await call("spaces.open", { id: "local:aurora" })).toBeNull();
    expect(opened).toEqual([["local:aurora", model.spaces.find((s) => s.id === "local:aurora")!.name]]);
    expect(await code("spaces.open", { id: "local:nope" })).toBe("not_found");
    expect(await code("spaces.open", {})).toBe("bad_args");
  });

  it("signs in with a device code, then answers the identity", async () => {
    const sessionEvents = () => events.filter((e) => e.event === "session.changed").length;
    await call("session.signOut");
    expect(await call("session.get")).toMatchObject({ identity: null, signedIn: false, signIn: "idle" });
    await tick(5);
    events.length = 0;
    expect(await call("session.signIn")).toMatchObject({ signIn: "starting" });
    await until(() => model.signIn.tag === "Waiting");
    expect(await call("session.get")).toMatchObject({ signIn: { type: "waiting", userCode: "ABCD-1234" } });
    await tick(5);
    expect(sessionEvents()).toBe(1);
    account.completeSignIn("ada@example.com");
    await until(() => model.identity === "ada@example.com");
    expect(await call("session.get")).toMatchObject({ identity: "ada@example.com", signedIn: true, signIn: "idle" });
    await tick(5);
    expect(sessionEvents()).toBe(2);
  });

  it("answers the Machines page from the host and the relay's devices", async () => {
    const m = (await call("machines.list")) as Record<string, unknown>;
    expect(m.signedIn).toBe(true);
    expect(m.presence).toEqual({ m1: true });
    expect(m.deviceStates).toEqual({ dev_mac: "enrolled", dev_work: "pending" });
    expect((m.devices as { rows: unknown[] }).rows.length).toBe(2);
    expect(m.host).toMatchObject({ configured: false, machineId: null, progress: null });
    expect(m.thisMachine).toMatchObject({ id: "this-mac" });
  });

  it("records the page's usage events through the switch, word by word", async () => {
    await call("telemetry.track", {
      signals: [
        { type: "feature", feature: "space_open" },
        { type: "feature", feature: "Not A Word" },
        { type: "space-create", location: "local", guestOs: "linux", kind: "container", outcome: "ready", failedPhase: "none", stalled: false, elapsedMs: 1200, gpu: false },
      ],
    });
    const after = telemetry.recorded.slice(-2);
    expect(after.map((s) => (s as unknown as { tag: string }).tag)).toEqual(["Feature", "SpaceCreate"]);
    telemetry.current = { ...telemetry.current, enabled: false };
    const before = telemetry.recorded.length;
    await call("telemetry.track", { signals: [{ type: "feature", feature: "space_open" }] });
    expect(telemetry.recorded.length).toBe(before);
    expect(await code("telemetry.track", {})).toBe("bad_args");
    expect(telemetrySignal(native, { type: "experiments-on", experiments: ["cua_volume", "BAD"] })).toBeNull();
    expect(telemetrySignal(native, { type: "launched", onboardingEligible: null })).not.toBeNull();
  });

  it("sets the calling window's background, and checks the drag regions", async () => {
    await call("window.setBackgroundColor", { color: "16181C", appearance: "dark" });
    expect(backgrounds).toEqual([["win-1", "#16181c", "dark"]]);
    expect(await code("window.setBackgroundColor", { color: "dark" })).toBe("bad_args");
    expect(await code("window.setDragRegions", { rects: [{ x: 1 }] })).toBe("bad_args");
  });

  it("tells the page when the list changed, once, and not when nothing did", async () => {
    await tick(5);
    events.length = 0;
    await model.refresh();
    await tick(5);
    expect(events.filter((e) => e.event === "spaces.changed")).toEqual([]);
    backend.rowsNow = backend.rowsNow.slice(0, 2);
    await model.refresh();
    await tick(5);
    expect(events.filter((e) => e.event === "spaces.changed")).toEqual([{ event: "spaces.changed" }]);
  });

  it("tells the page when this device is approved, once, so an open detail connects", async () => {
    // The SwiftUI app's DetailApprovalTests: the relay approves this device between two reads.
    const relay = fixtureDevices();
    const snapshot = await relay.snapshot(50);
    snapshot.devices[0]!.state = "pending";
    model.devices.devices = () => relay;
    await model.devices.refresh();
    await tick(5);
    expect(((await call("machines.list")) as { accessNotice?: unknown }).accessNotice).toBeTruthy();
    events.length = 0;

    snapshot.devices[0]!.state = "enrolled";
    // The minute's read, as `startHost` runs it: nothing else tells the page.
    await model.devices.refresh();
    await tick(5);
    expect(events.filter((e) => e.event === "machines.changed")).toEqual([{ event: "machines.changed" }]);
    expect(((await call("machines.list")) as { accessNotice?: unknown }).accessNotice).toBeFalsy();

    // Repeated snapshots say nothing new.
    await model.devices.refresh();
    await model.devices.refresh();
    await tick(5);
    expect(events.filter((e) => e.event === "machines.changed")).toHaveLength(1);
  });

  it("says when a list read failed, in its own words only, keeps the rows, and clears it on the next good read", async () => {
    // The SwiftUI app's DiscoveryTests.
    await tick(5);
    const before = ((await call("spaces.list")) as { spaces: unknown[] }).spaces.length;
    expect(((await call("spaces.list")) as { rosterError: unknown }).rosterError).toBeNull();
    events.length = 0;
    const rows = backend.rows.bind(backend);
    backend.rows = async () => {
      throw new Error("relay said 502: https://relay.example/v1?token=secret");
    };
    await model.refresh();
    await tick(5);
    const failed = (await call("spaces.list")) as { spaces: unknown[]; rosterError: string };
    expect(failed.rosterError).toBe("Could not refresh Spaces. Previously loaded rows may be out of date.");
    expect(failed.spaces).toHaveLength(before);
    expect(JSON.stringify(failed)).not.toContain("secret");
    expect(model.banner).toBeNull();
    expect(events.filter((e) => e.event === "spaces.changed")).toEqual([{ event: "spaces.changed" }]);

    backend.rows = rows;
    await model.refresh();
    await tick(5);
    expect(((await call("spaces.list")) as { rosterError: unknown }).rosterError).toBeNull();
    expect(events.filter((e) => e.event === "spaces.changed")).toHaveLength(2);
  });

  it("says the Spaces could not be loaded when the first read fails", async () => {
    const fresh = new AppModel({
      native,
      backend: Object.assign(new FixtureSpacesBackend(native), {
        rows: async () => {
          throw new Error("no daemon");
        },
      }),
      startup: new StartupModel({ kind: "ready" }),
      settingsPath: path.join(mkdtempSync(path.join(home, "app-")), "app-settings.json"),
      host: new HostModel(native, fixtureHost()),
      devices: new DevicesModel(native, () => fixtureDevices()),
      cloud: new CloudModel(native, () => backend),
      account,
      telemetry,
      servicesIn: () => true,
      cpus: 8,
    });
    await fresh.refresh();
    expect(fresh.loaded).toBe(false);
    expect(fresh.rosterError).toBe("Could not load Spaces. Try refreshing again.");
  });

  it("pushes the launch's states as startup.changed", async () => {
    const startup = model.startup;
    startup.start = async () => {};
    events.length = 0;
    startup.begin();
    await until(() => startup.isReady);
    const states = events.filter((e) => e.event === "startup.changed").map((e) => (e.payload as { phase: string }).phase);
    expect(states[0]).toBe("starting");
    expect(states.at(-1)).toBe("ready");
  });

  it("reads into the page's contract through the Electron adapter", async () => {
    const win = {
      cuaDesktop: { invoke: (_channel: string, request: unknown) => bridge.registry.dispatch(request), platform: "darwin" },
      open: () => null,
    };
    const adapter = await pageAdapter(win);
    const args: Partial<{ [K in OpName]: OpArgs<K> }> = {
      "spaces.setPower": { spaceId: "local:aurora", on: true },
      "spaces.open": { spaceId: "local:aurora" },
      "spaces.cancelCreate": { pendingId: "pending:none" },
      "telemetry.track": { signals: [{ type: "feature", feature: "space_open" }] },
      "startup.act": { action: "tryAgain" },
      "spaces.usage": { spaceId: "local:aurora" },
      "spaces.windows": { spaceId: "local:aurora" },
      "stream.pip": { spaceId: "local:aurora", command: { type: "open", row: "w-1" } },
      "spaces.thumbnail": { spaceId: "local:aurora" },
      "spaces.droppedFiles": { names: [] },
      "spaces.sendFiles": { spaceId: "local:aurora", paths: ["/Users/ada/Desktop/picked.txt"] },
      "sharing.list": { spaceId: "local:aurora" },
      "sharing.share": { spaceId: "local:aurora", who: "bo@example.com", role: "viewer" },
      "sharing.unshare": { spaceId: "local:aurora", who: "bo@example.com" },
      "spaces.add": { url: "http://studio.local:7400", token: null, name: "studio" },
      "clouds.test": { target: { provider: "aws", region: "us-west-2" } },
      "clouds.connect": { target: { provider: "aws", region: "us-west-2" }, makeDefault: false },
    };
    // This machine, Devices, Settings and the rest: host-bridge.test.ts reads them with their arguments.
    const skip = new Set<OpName>(["spaces.create", "spaces.delete", "session.signIn", "session.signOut", "session.openExternal", "spaces.openStream", ...VAULT_OPS, ...(HOST_OPS as OpName[])]);
    const problems: string[] = [];
    const checked: string[] = [];
    for (const op of OPERATIONS) {
      const cov = HOST_COVERAGE[op] as { webkit: { methods: readonly string[] }; electron?: { methods: readonly string[] } };
      const methods = cov.electron?.methods ?? cov.webkit.methods;
      if (skip.has(op) || methods.length === 0 || methods.some((m) => AGENT_METHODS.includes(m)) || methods.some((m) => NOT_YET.includes(m))) continue;
      try {
        const result = await adapter.call(op, (args[op] ?? {}) as never);
        problems.push(...validate(OP_SHAPES[op], result, SHAPE_DEFS).map((e) => `${op} ${e}`));
        checked.push(op);
      } catch (e) {
        problems.push(`${op} failed: ${(e as Error).message}`);
      }
    }
    adapter.dispose?.();
    expect(problems).toEqual([]);
    expect(checked).toEqual(expect.arrayContaining(["spaces.list", "machines.list", "host.status", "session.get", "spaces.createOptions", "startup.get"]));
  });
});
