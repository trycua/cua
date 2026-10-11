// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This machine, Devices, Settings, About, launch at login, Storage, Cua
// Volume, Notifications and the first run on the real app core (`pnpm
// native`), over a fixture host, relay, daemon tools and system (the SwiftUI
// app's WebUIHostTests, BridgeContractTests, HostSetupAuthTests,
// HostSignInSharingTests, DevicesTests, LoginItemTests and UpdatesTests,
// ported): every answer in the shapes the page reads (bridge-shapes.json),
// the page's adapter reading them, and what each button does. Skipped when
// this machine's native directory was not built.
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
import { hostParts } from "../src/bridge/host-parts";
import { isMachineDesktop, menuItems, realSpaces, startHost } from "../src/bridge/host-start";
import { AppModel } from "../src/model/app-model";
import { CloudModel } from "../src/model/cloud";
import { DevicesModel } from "../src/model/devices";
import { HostModel } from "../src/model/host";
import { StartupModel } from "../src/model/startup";
import { devDirName, nativeFiles } from "../src/native/location";
import { loadNative, type Native } from "../src/native/load";
import { FixtureAccount, FixtureSpacesBackend, FixtureTelemetry } from "./fixtures";
import { FixtureAgentSetup } from "./agents-fixtures";
import { FakeAccountTokens, FixtureDevices, FixtureHost, FixtureSystem, HOST_METHODS, HOST_OPS, sdkError } from "./host-fixtures";
import { NOT_YET } from "./not-yet";

const dir = path.resolve(__dirname, "../native", devDirName(process.platform, process.arch));
const built = existsSync(nativeFiles(dir, process.platform).library);

const SHAPES = JSON.parse(readFileSync(path.resolve(__dirname, "../../cua-spaces-web/src/bridge/contracts/bridge-shapes.json"), "utf8")) as {
  webkit: Record<string, Schema>;
  $defs: Record<string, Schema>;
};
const json = (v: unknown) => JSON.parse(JSON.stringify({ v })).v as unknown;
const shapeProblems = (method: string, answer: unknown) => validate(SHAPES.webkit[method]!, json(answer), SHAPES.$defs).map((e) => `${method} ${e}`);
const ELECTRON_ADAPTER = path.resolve(__dirname, "../../cua-spaces-web/src/bridge/adapters/electron.ts");
const pageAdapter = async (win: unknown) =>
  ((await import(/* @vite-ignore */ ELECTRON_ADAPTER)) as { createElectronAdapter(win: unknown): { call(op: string, args: unknown): Promise<unknown>; dispose?(): void } }).createElectronAdapter(win);
const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

/** The daemon's tools as the fixture daemon answers them. */
class ToolBackend extends FixtureSpacesBackend {
  readonly tools: [string, Record<string, unknown>][] = [];
  notifications = [{ id: "n1", at_ms: 1000, agent: "ada", kind: "answer", title: "Done", body: "The report is ready", read: false }];
  agents: unknown[] = [];
  override async tool(tool: string, args: Record<string, unknown> = {}): Promise<unknown> {
    this.tools.push([tool, args]);
    switch (tool) {
      case "volume_storage":
        return { backend: "fs", fs_path: "~/.cua/volume", s3: null, has_keys: false };
      case "volume_mount_status":
      case "volume_mount":
      case "volume_unmount":
        return { enabled: true, state: "unmounted", method: "nfs", path: "~/Cua Volume", volume_name: "Cua Volume", detail: null, settings_url: null, volume_errors: [] };
      case "volume_cache_stats":
        return { size_bytes: 10, capacity_bytes: 1024 };
      case "volume_requests":
        return { requests: [{ id: "q1", principal: "agent:ada", prefix: "public/", mode: "r", reason: "read" }] };
      case "volume_grants":
        return { grants: [] };
      case "volume_sync_status":
        return null;
      case "volume_storage_set":
        return { ok: true, reachable: true, authorized: true, versioning: true, detail: null, applied: !args.dry_run };
      case "notifications_list":
        return { notifications: this.notifications };
      case "notifications_ack":
        this.notifications = this.notifications.map((n) => ({ ...n, read: true }));
        return {};
      case "persistent_agent_list":
        return { agents: this.agents };
      case "volume_approve":
      case "volume_deny":
      case "volume_revoke":
      case "volume_sync_resolve":
      case "volume_cache_set":
      case "volume_cache_clear":
        return {};
      default:
        return super.tool(tool);
    }
  }
}

describe.skipIf(!built)("This machine, Devices and Settings on the app core", () => {
  let home: string;
  let native: Native;
  const saved = { ...process.env };
  let backend: ToolBackend;
  let host: FixtureHost;
  let relay: FixtureDevices;
  let system: FixtureSystem;
  let model: AppModel;
  let bridge: Bridge;
  let events: BridgeEvent[];
  let agentSetup: FixtureAgentSetup;

  beforeAll(async () => {
    home = mkdtempSync(path.join(tmpdir(), "cua-host-bridge-"));
    Object.assign(process.env, { HOME: home, USERPROFILE: home, CUA_HOME: path.join(home, ".cua"), CUA_TELEMETRY: "0", DO_NOT_TRACK: "1", CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    native = await loadNative(dir);
  });

  afterAll(() => {
    process.env = saved;
    rmSync(home, { recursive: true, force: true });
  });

  const make = async (o: { system?: FixtureSystem | null; platform?: NodeJS.Platform; servicesIn?: boolean; account?: null; build?: string } = {}) => {
    backend = new ToolBackend(native);
    host = new FixtureHost();
    relay = new FixtureDevices();
    system = new FixtureSystem(home);
    const settingsPath = path.join(mkdtempSync(path.join(home, "app-")), "app-settings.json");
    const servicesIn = o.servicesIn ?? true;
    model = new AppModel({
      native,
      backend,
      startup: new StartupModel({ kind: "ready" }),
      settingsPath,
      host: new HostModel(native, host.like, o.platform ?? "darwin"),
      devices: new DevicesModel(native, () => relay.like),
      cloud: new CloudModel(native, () => backend),
      account: o.account === null ? null : new FixtureAccount("ada@example.com"),
      telemetry: new FixtureTelemetry(),
      servicesIn: () => servicesIn,
      cpus: 8,
      agentSetup: (agentSetup = new FixtureAgentSetup()),
    });
    bridge = createBridge({
      model,
      supervisor: null,
      version: "1.2.3",
      build: o.build,
      platform: o.platform ?? "darwin",
      env: { HOME: home },
      ui: { openSpace: () => {}, setBackground: () => {}, activate: () => {} },
      system: o.system === null ? undefined : (o.system ?? system),
    });
    events = [];
    bridge.events.subscribe((e) => events.push(e));
    await model.devices.refresh();
  };

  beforeEach(() => make());

  const call = (method: string, args: Record<string, unknown> = {}) => bridge.registry.handle(method, args);
  const reply = (method: string, args: Record<string, unknown> = {}) => bridge.registry.dispatch({ id: "x", method, args });
  const code = async (method: string, args: Record<string, unknown> = {}) => {
    const r = await reply(method, args);
    return r.ok ? null : r.error.code;
  };

  it("answers in the SwiftUI host's shapes", async () => {
    const cases: [string, Record<string, unknown>][] = [
      ["volume.overview", {}],
      ["volume.storage", {}],
      ["volume.storageSet", { update: { backend: "fs", dry_run: true } }],
      ["volume.mount", {}],
      ["volume.unmount", {}],
      ["volume.approve", { id: "q1" }],
      ["volume.deny", { id: "q1" }],
      ["volume.revoke", { id: "g1" }],
      ["volume.resolve", { path: "public/plan.md" }],
      ["volume.reveal", { path: "~/Cua Volume/public" }],
      ["about.get", {}],
      ["about.set", { channel: "beta" }],
      ["about.checkNow", {}],
      ["loginItem.get", {}],
      ["loginItem.set", { on: true }],
      ["loginItem.openSettings", {}],
      ["devices.get", {}],
      ["devices.enroll", {}],
      ["devices.checkEnrolled", {}],
      ["devices.approve", { deviceId: "dev_work" }],
      ["devices.rename", { id: "dev_old", name: "Old box" }],
      ["devices.revoke", { id: "dev_old" }],
      ["devices.confirmMachine", { id: "m2" }],
      ["storage.get", {}],
      ["storage.run", { request: { kind: "clear-cache" } }],
      ["notifications.list", {}],
      ["notifications.markAllRead", {}],
      ["host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3211" } }],
      ["host.action", { action: "provide-spaces" }],
      ["host.openSettings", { url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" }],
      ["settings.get", {}],
      ["settings.choose", { row: "telemetry", option: "off" }],
    ];
    const problems: string[] = [];
    for (const [method, args] of cases) problems.push(...shapeProblems(method, await call(method, args)));
    expect(problems).toEqual([]);
    // Every method of this area has its case (the Electron-only ones are checked below).
    const electronOnly = new Set(["onboarding.get", "onboarding.complete"]);
    expect(HOST_METHODS.filter((m) => !electronOnly.has(m) && !cases.some(([c]) => c === m))).toEqual([]);
    expect(HOST_METHODS.filter((m) => NOT_YET.includes(m))).toEqual([]);
  });

  it("reads into the page's contract through the Electron adapter", async () => {
    const win = { cuaDesktop: { invoke: (_c: string, request: unknown) => bridge.registry.dispatch(request), platform: "darwin" }, open: () => null };
    const adapter = await pageAdapter(win);
    const args: Partial<{ [K in OpName]: OpArgs<K> }> = {
      "volume.storageSet": { update: { backend: "fs", s3: null, access_key_id: null, secret_access_key: null, dry_run: true } } as never,
      "volume.approve": { id: "q1" },
      "volume.deny": { id: "q1" },
      "volume.revoke": { id: "g1" },
      "volume.resolve": { path: "public/plan.md" },
      "volume.reveal": { path: `${home}/Cua Volume/public` },
      "about.set": { autoCheck: true },
      "loginItem.set": { on: true },
      "devices.approve": { code: null, deviceId: "dev_work" },
      "devices.rename": { id: "dev_old", name: "Old box" },
      "devices.revoke": { id: "dev_old" },
      "devices.confirmMachine": { id: "m2" },
      "storage.run": { request: { kind: "clear-cache" } } as never,
      "host.setUp": { request: { mode: "direct", direct: "0.0.0.0:3211" } } as never,
      "host.action": { action: "provide-spaces" } as never,
      "host.openSettings": { url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" },
      "settings.set": { key: "telemetry", value: false },
      "settings.choose": { row: "auto-connect", option: "off" },
      "experiments.set": { experiments: { cuaVolume: true } } as never,
      "session.completeOnboarding": { mode: "client" },
    };
    // Every operation over this area's methods is checked here.
    const mine = OPERATIONS.filter((op) => {
      const cov = HOST_COVERAGE[op] as { webkit: { methods: readonly string[] }; electron?: { methods: readonly string[] } };
      return (cov.electron?.methods ?? cov.webkit.methods).some((m) => HOST_METHODS.includes(m) && m !== "onboarding.get");
    });
    expect(mine.filter((op) => !HOST_OPS.includes(op))).toEqual([]);
    const problems: string[] = [];
    for (const op of HOST_OPS as OpName[]) {
      try {
        const result = await adapter.call(op, (args[op] ?? {}) as never);
        problems.push(...validate(OP_SHAPES[op], result, SHAPE_DEFS).map((e) => `${op} ${e}`));
      } catch (e) {
        problems.push(`${op} failed: ${(e as Error).message}`);
      }
    }
    adapter.dispose?.();
    expect(problems).toEqual([]);
  });

  // MARK: This machine

  it("sets this machine up and runs its buttons through the host model", async () => {
    const s = (await call("host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3211" } })) as Record<string, unknown>;
    expect(s).toMatchObject({ configured: true, mode: "direct", progress: null, machineId: "0123abcd4567" });
    const p = (await call("host.action", { action: "provide-spaces" })) as Record<string, unknown>;
    expect(p.provideSpaces).toBe(true);
    expect(host.calls.at(-1)).toBe("configure:-:true");
    expect(await code("host.action", { action: "set-up" })).toBe("bad_args");
    expect(await code("host.setUp", { request: {} })).toBe("bad_args");
  });

  it("carries the whole state, the owner and the progress", async () => {
    await call("host.setUp", { request: { mode: "relay", name: "Studio" } });
    const s = (await call("host.status")) as Record<string, unknown>;
    expect(s.maxMacosVms).toBe(2);
    expect(Array.isArray(s.recentAccess) && Array.isArray(s.providedSpaces) && Array.isArray(s.spacesAudit)).toBe(true);
    expect(s).toMatchObject({ pausedSignedOut: false, owner: "user-1", ownerEmail: "ada@example.com", progress: null });
    expect("account" in s).toBe(true);
  });

  it("words a failed setup as the native form does", async () => {
    host.script = [sdkError("Http", "error sending request for url (https://relay.cua.ai): connection refused")];
    const r = await reply("host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3211" } });
    expect(r.ok).toBe(false);
    if (r.ok) return;
    expect(r.error).toMatchObject({
      code: "failed",
      title: "Couldn’t connect",
      message: "Cua Spaces couldn’t reach the internet. Check your connection and try again.",
      actionLabel: "Retry",
    });
    expect(r.error.details).toContain("connection refused");
  });

  it("words a failed button with the raw error as details, and Retry runs it again", async () => {
    await call("host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3211" } });
    host.failConfigure = "local runtime: service: launchctl bootstrap failed: 5";
    const r = await reply("host.action", { action: "provide-spaces" });
    expect(r.ok ? null : r.error).toMatchObject({ code: "failed", title: "Couldn’t start the Cua host service", actionLabel: "Retry" });
    expect(r.ok ? "" : r.error.details).toContain("launchctl bootstrap failed");
    expect(model.host.actionFailure?.kind).toBe("service");
    await model.host.retryFailedAction();
    expect(model.host.actionFailure).toBeNull();
    expect(host.calls.filter((c) => c.startsWith("configure:"))).toHaveLength(2);
    // Errors without words of their own keep the plain envelope.
    const plain = await reply("host.action", { action: "nope" });
    expect(plain.ok ? null : plain.error.title).toBeUndefined();
  });

  it("opens only System Settings panes, and only on macOS", async () => {
    await call("host.openSettings", { url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" });
    expect(system.opened).toEqual(["x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"]);
    expect(await code("host.openSettings", { url: "https://example.com" })).toBe("bad_args");
    await make({ platform: "win32" });
    expect(await code("host.openSettings", { url: "x-apple.systempreferences:x" })).toBe("unsupported");
  });

  it("signs in inline when relay setup has no account, with the progress line", async () => {
    const account = new FakeAccountTokens();
    account.signInGives = "tok-new";
    account.wire(model.host);
    await call("host.setUp", { request: { mode: "relay", name: "Studio" } });
    expect(account.signIns).toBe(1);
    expect(account.progressDuringSignIn).toBe(HostModel.signInProgress);
    expect(host.tokens).toEqual(["tok-new"]);
    expect(model.host.progress).toBeNull();
  });

  it("a cancelled sign-in is a sign-in failure with Sign In as its button", async () => {
    const account = new FakeAccountTokens();
    account.wire(model.host);
    const r = await reply("host.setUp", { request: { mode: "relay", name: "Studio" } });
    expect(host.tokens).toEqual([]);
    expect(r.ok ? null : r.error).toMatchObject({ title: "Sign in to Cua", actionLabel: "Sign In" });
  });

  it("refreshes a token the relay refuses once, then signs in again", async () => {
    const account = new FakeAccountTokens();
    account.token = "tok-stale";
    account.refreshed = "tok-fresh";
    account.wire(model.host);
    host.script = [sdkError("Unauthenticated", "relay: invalid account token: ExpiredSignature")];
    await call("host.setUp", { request: { mode: "relay" } });
    expect(account.reads).toEqual([false, true]);
    expect(host.tokens).toEqual(["tok-stale", "tok-fresh"]);

    await make();
    const again = new FakeAccountTokens();
    again.token = "tok-1";
    again.refreshed = "tok-2";
    again.wire(model.host);
    host.script = [sdkError("Unauthenticated", "relay: invalid account token"), sdkError("Unauthenticated", "relay: invalid account token")];
    const r = await reply("host.setUp", { request: { mode: "relay" } });
    expect(host.tokens).toEqual(["tok-1", "tok-2"]);
    expect(r.ok ? null : r.error.actionLabel).toBe("Sign In");
  });

  it("retries a network error reading the token, and offline is not signed out", async () => {
    const account = new FakeAccountTokens();
    account.token = "tok-1";
    account.failures = [sdkError("Transport", "offline"), sdkError("Timeout", "timed out")];
    account.wire(model.host);
    await call("host.setUp", { request: { mode: "relay" } });
    expect(account.reads).toHaveLength(3);
    expect(account.signIns).toBe(0);

    await make();
    const offline = new FakeAccountTokens();
    offline.failures = [0, 1, 2].map(() => sdkError("Transport", "error sending request: Network is unreachable"));
    offline.wire(model.host);
    const r = await reply("host.setUp", { request: { mode: "relay" } });
    expect(offline.signIns).toBe(0);
    expect(r.ok ? null : r.error).toMatchObject({ title: "Couldn’t connect", actionLabel: "Retry" });
  });

  it("pauses relay sharing while signed out, and Sign In resumes it", async () => {
    const account = new FakeAccountTokens();
    account.token = "tok";
    account.wire(model.host);
    model.host.currentAccount = () => ({ id: "user-1", email: "ada@example.com", display: "Ada" });
    await call("host.setUp", { request: { mode: "relay" } });
    account.token = null;
    await model.host.reconcileAccount();
    let s = (await call("host.status")) as Record<string, unknown>;
    expect(s.pausedSignedOut).toBe(true);
    expect(host.calls).toContain("pause");
    account.signInGives = "tok-again";
    s = (await call("host.action", { action: "sign-in" })) as Record<string, unknown>;
    expect(account.signIns).toBe(1);
    expect(host.calls.at(-1)).toBe("resume:user-1");
    expect(s.pausedSignedOut).toBe(false);
  });

  it("without an account wired, Sign In leaves the status as it is", async () => {
    await make({ account: null });
    await call("host.status");
    const s = (await call("host.action", { action: "sign-in" })) as Record<string, unknown>;
    expect(s.configured).toBe(false);
  });

  it("asks for Local Network access once this machine provides Spaces", async () => {
    let asked = 0;
    model.host.localNetwork = { request: () => asked++ };
    await call("host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3211" } });
    expect(asked).toBe(0);
    await call("host.action", { action: "provide-spaces" });
    await call("host.action", { action: "stop-providing-spaces" });
    await call("host.action", { action: "provide-spaces" });
    expect(asked).toBe(1);
  });

  // MARK: Devices

  it("shows enrollment when the relay refuses this device, then enrolls", async () => {
    relay.failSnapshot = "relay: this device is not enrolled for your cua.ai account";
    const input = (await call("devices.get")) as Record<string, unknown>;
    expect(String(input.readError)).toContain("not enrolled");
    expect(typeof input.deviceName).toBe("string");
    const r = (await call("devices.enroll")) as Record<string, unknown>;
    expect(r).toEqual({ enrolled: false, code: "K7QX-M2RP" });
    expect(model.devices.pendingCode).toBe("K7QX-M2RP");
    relay.failSnapshot = null;
    expect(((await call("devices.get")) as Record<string, unknown>).readError).toBeNull();
    relay.enrolledNow = true;
    expect(await call("devices.checkEnrolled")).toBe(true);
    expect(model.devices.pendingCode).toBeNull();
  });

  it("asks the person here before approving, naming the device", async () => {
    await call("devices.approve", { deviceId: "dev_work" });
    expect(system.asked).toEqual(["approve “Work laptop” for your Cua account"]);
    expect(relay.calls).toContain("approve:dev_work");
    system.allow = false;
    expect(await code("devices.approve", { deviceId: "dev_old" })).toBe("failed");
    expect(relay.calls.filter((c) => c.startsWith("approve:"))).toHaveLength(1);
    expect(await code("devices.approve", {})).toBe("bad_args");
  });

  it("never says approved unless the relay enrolled the device", async () => {
    relay.approveState = "pending";
    const r = await reply("devices.approve", { code: "K7QX-M2RP" });
    expect(r.ok ? "" : r.error.message).toContain("did not enroll");
  });

  it("renames with the core's clean name, revokes and confirms machines", async () => {
    await call("devices.rename", { id: "dev_old", name: "  Old box  " });
    expect(relay.calls.at(-1)).toBe("rename:dev_old:Old box");
    expect(await code("devices.rename", { id: "dev_old", name: "   " })).toBe("bad_args");
    await call("devices.revoke", { id: "dev_old" });
    await call("devices.confirmMachine", { id: "m2" });
    expect(relay.calls.slice(-2)).toEqual(["revoke:dev_old", "confirm-machine:m2"]);
  });

  it("announces a device asking to join once per launch", async () => {
    const started = startHost(bridge.context, { onboarded: () => true });
    await model.devices.refresh();
    await model.devices.refresh();
    const notes = system.notes.filter((n) => n.id.startsWith("device-"));
    expect(notes.map((n) => n.id)).toEqual(["device-dev_work", "device-dev_old"]);
    expect(notes[0]).toMatchObject({ route: "/settings/devices" });
    expect(notes[0]!.title.length).toBeGreaterThan(0);
    started.stop();
  });

  it("needs a signed-in account", async () => {
    model.devices.devices = () => null;
    expect(await code("devices.get")).toBe("unsupported");
    expect(await code("devices.enroll")).toBe("unsupported");
  });

  // MARK: Settings

  it("lays out General with the Teams waitlist, Welcome and What is collected, and the runtimes", async () => {
    const s = (await call("settings.get")) as { page: { sections: { id: string; rows: { id: string }[] }[] }; updateChannel: unknown };
    const rows = s.page.sections.flatMap((x) => x.rows.map((r) => r.id));
    expect(rows).toEqual(expect.arrayContaining(["account", "notch", "auto-connect", "welcome", "teams", "telemetry", "telemetry-note", "launch-at-login"]));
    expect(rows).toEqual(expect.arrayContaining(["macos-runtime", "linux-runtime"]));
    expect(s.updateChannel).toBe("stable");
  });

  it("chooses each row as the Swift app does", async () => {
    let settingsEvents = 0;
    bridge.events.subscribe((e) => e.event === "settings.changed" && settingsEvents++);
    await call("settings.choose", { row: "notch", option: "hide" });
    expect(model.settings.menuBar).toBe(true);
    await call("settings.choose", { row: "auto-connect", option: "off" });
    expect(model.settings.autoConnect).toBe(false);
    await call("settings.choose", { row: "default-location", option: "cloud" });
    expect(model.settings.defaultLocation).toBe(native.AppLocation.Cloud);
    await call("settings.choose", { row: "telemetry", option: "off" });
    expect(model.telemetry?.status().enabled).toBe(false);
    await call("settings.choose", { row: "launch-at-login", option: "on" });
    expect(system.loginItem.calls).toEqual(["register"]);
    expect(model.settings.launchAtLogin).toBe(true);
    await call("settings.choose", { row: "experiment:cua_volume", option: "on" });
    expect(model.settings.experiments.cuaVolume).toBe(true);
    // Storage follows General while Cua Volume is on.
    const s = (await call("settings.get")) as { page: { sections: { id: string }[] } };
    expect(s.page.sections.map((x) => x.id)).toContain("storage");
    await tick(5);
    expect(settingsEvents).toBeGreaterThan(0);
  });

  it("offers every experiment but the New UI preview, which this app is", async () => {
    const s = (await call("settings.get")) as { experiments: { sections: { rows: { id: string }[] }[] } };
    const rows = s.experiments.sections.flatMap((x) => x.rows.map((r) => r.id));
    expect(rows).toEqual(expect.arrayContaining(["experiment:cua_volume", "experiment:your_cloud", "experiment:sharing"]));
    expect(rows).not.toContain("experiment:web_ui");
    expect(await code("settings.choose", { row: "experiment:web_ui", option: "off" })).toBe("unsupported");
  });

  it("New Space's \u201cUse built-in Lume\u201d and \u201cUse built-in runtime\u201d change the Runtimes rows", async () => {
    const set: string[] = [];
    Object.assign(backend, { setLumeSource: async (v: string) => void set.push(`lume=${v}`) });
    backend.lumeSource = async () => "builtin";
    Object.assign(backend, { setLinuxSource: async (v: string) => void set.push(`linux=${v}`) });
    backend.linuxSource = async () => "builtin";
    // The wizard's switch (`RUNTIME_SWITCH_ROW`): the same choice as Settings, Runtimes.
    const s = (await call("settings.choose", { row: "macos-runtime", option: "builtin" })) as { page: { sections: { rows: { id: string; options: { id: string; active: boolean }[] }[] }[] } };
    await call("settings.choose", { row: "linux-runtime", option: "builtin" });
    expect(set).toEqual(["lume=builtin", "linux=builtin"]);
    expect(model.lumeSource).toBe("builtin");
    expect(model.linuxSource).toBe("builtin");
    const row = s.page.sections.flatMap((x) => x.rows).find((r) => r.id === "macos-runtime")!;
    expect(row.options.find((o) => o.active)?.id).toBe("builtin");
  });

  it("lists the coding agents under AI agents, and a row's button sets one up", async () => {
    const s = (await call("settings.get")) as { page: { sections: { rows: { id: string }[] }[] } };
    expect(s.page.sections.flatMap((x) => x.rows.map((r) => r.id))).toEqual(expect.arrayContaining(["agent:claude-code", "agent:codex"]));
    await call("settings.choose", { row: "agent:claude-code", option: "press" });
    expect(agentSetup.calls).toContain("setup:claude-code");
    expect(model.agents.coding.rows?.find((r) => r.agent === "claude-code")?.configured).toBe(true);
  });

  it("asks for Local Network access when a Space is created on this machine", async () => {
    let asked = 0;
    model.host.localNetwork = { request: () => asked++ };
    await call("spaces.create", { config: { image: "ghcr.io/trycua/linux:24.04", kind: "container", on: "cloud" }, pendingId: "pending:c1", os: "linux" });
    expect(asked).toBe(0);
    await call("spaces.create", { config: { image: "ghcr.io/trycua/linux:24.04", kind: "container", on: "local" }, pendingId: "pending:l1", os: "linux" });
    expect(asked).toBe(1);
  });

  it("the notch follows the menu bar setting", async () => {
    // main.ts feeds `notch.setShown(!settings.menuBar)` on each settings change.
    const shown: boolean[] = [];
    model.subscribe((c) => c === "settings" && shown.push(!model.settings.menuBar));
    await call("settings.choose", { row: "notch", option: "hide" });
    await call("settings.choose", { row: "notch", option: "show" });
    expect(shown).toEqual([false, true]);
  });

  it("reads and chooses the update channel by row, unsupported without an updater", async () => {
    await call("settings.choose", { row: "update-channel", option: "beta" });
    expect(system.updater.channel).toBe("beta");
    expect(model.settings.updateChannel).toBe(native.AppUpdateChannel.Beta);
    expect(((await call("settings.get")) as { updateChannel: string }).updateChannel).toBe("beta");
    await make({ system: null });
    expect(((await call("settings.get")) as { updateChannel: unknown }).updateChannel).toBeNull();
    expect(await code("settings.choose", { row: "update-channel", option: "beta" })).toBe("unsupported");
  });

  it("Welcome's Show again makes the first run due", async () => {
    await call("onboarding.complete", { mode: "client" });
    expect(await call("onboarding.get")).toEqual({ completed: true, mode: "client" });
    events.length = 0;
    await call("settings.choose", { row: "welcome", option: "show" });
    expect(await call("onboarding.get")).toMatchObject({ completed: false });
    expect(events.some((e) => e.event === "session.changed")).toBe(true);
  });

  it("the Keyvault's switches show its state and run through the broker", async () => {
    const { FakeKeyvault, NOW, parityOverview } = await import("./vault-fixtures");
    const { KeyvaultModel } = await import("../src/model/keyvault");
    const fake = new FakeKeyvault(native, parityOverview(native));
    fake.current.status = { ...fake.current.status!, autoWipe: false, skipUnlockPrompt: false };
    (model as { keyvault: InstanceType<typeof KeyvaultModel> }).keyvault = new KeyvaultModel(native, fake, () => NOW);
    await model.keyvault.refresh();
    const s = (await call("settings.get")) as { page: { sections: { rows: { id: string }[] }[] } };
    expect(s.page.sections.flatMap((x) => x.rows.map((r) => r.id))).toEqual(expect.arrayContaining(["keyvault-auto-wipe", "keyvault-unlock-prompt"]));
    await call("settings.choose", { row: "keyvault-auto-wipe", option: "on" });
    await call("settings.choose", { row: "keyvault-unlock-prompt", option: "off" });
    expect(fake.commands.map((c) => (c as unknown as { tag: string }).tag)).toEqual(["SetAutoWipe", "SetSkipUnlockPrompt"]);
    fake.failWith = "CuaError.PermissionDenied: presence was not confirmed";
    expect(await code("settings.choose", { row: "keyvault-auto-wipe", option: "off" })).toBe("failed");
  });

  // MARK: About and launch at login

  it("About is the updater's input, and Check Now answers before the updater's window", async () => {
    const a = (await call("about.set", { autoCheck: true, channel: "beta" })) as Record<string, unknown>;
    expect(a).toMatchObject({ autoCheck: true, channel: "beta", updater: true, platform: "macos", version: "1.2.3" });
    expect(String(a.os)).toMatch(/\(/);
    const c = (await call("about.checkNow")) as Record<string, unknown>;
    expect(c.checking).toBe(true);
    expect(system.updater.checks).toBe(0);
    await tick(5);
    expect(system.updater.checks).toBe(1);
    expect(((await call("about.get")) as Record<string, unknown>).lastCheck).toEqual(expect.any(String));
    expect(a.build).toBe("");
    await make({ system: null });
    expect(((await call("about.get")) as Record<string, unknown>).updater).toBe(false);
    expect(await code("about.set", { autoCheck: true })).toBe("unsupported");
    expect(await code("about.checkNow")).toBe("unsupported");
  });

  it("About carries the bundle's build, for \"Version 0.7.2 (0.7.2.41)\" as the SwiftUI app shows it", async () => {
    await make({ build: "1.2.3.41" });
    expect(((await call("about.get")) as Record<string, unknown>).build).toBe("1.2.3.41");
  });

  it("records what each check found", async () => {
    const telemetry = model.telemetry as FixtureTelemetry;
    const before = telemetry.recorded.length;
    await call("about.checkNow");
    await tick(5);
    const tags = telemetry.recorded.slice(before).map((s) => (s as unknown as { inner: { action: string; trigger: string } }).inner);
    expect(tags).toEqual([
      { action: "checked", channel: "stable", trigger: "user" },
      { action: "not_found", channel: "stable", trigger: "user" },
    ]);
  });

  it("launch at login is the system's, with what this machine serves", async () => {
    expect(await call("loginItem.get")).toEqual({ status: "notRegistered", providesSpaces: false, runsAgents: false });
    backend.agents = [{ name: "ada" }];
    expect(await call("loginItem.set", { on: true })).toEqual({ status: "enabled", providesSpaces: false, runsAgents: true });
    await call("loginItem.openSettings");
    expect(system.loginItem.calls).toEqual(["register", "open-settings"]);
    system.loginItem.failure = "not allowed";
    expect(await code("loginItem.set", { on: false })).toBe("failed");
    await make({ system: null });
    expect(await code("loginItem.get")).toBe("unsupported");
  });

  it("turns launch at login on for an install that never chose and serves Spaces; a choice stands", async () => {
    const parts = hostParts(bridge.context);
    await call("host.setUp", { request: { mode: "relay", profile: "spare" } });
    expect(model.host.state?.provideSpaces).toBe(true);
    await parts.loginItem.applyAtLaunch(true);
    expect(system.loginItem.calls).toEqual(["register"]);
    expect(model.settings.launchAtLogin).toBe(true);
    system.loginItem.current = "notRegistered";
    await parts.loginItem.applyAtLaunch(true);
    expect(system.loginItem.calls).toEqual(["register"]);
  });

  // MARK: Storage, Volume, Notifications

  it("Storage answers the three tools and runs the section's commands", async () => {
    const s = (await call("storage.get")) as Record<string, unknown>;
    expect(s).toMatchObject({ os: "macos", home });
    expect(s.storage).toMatchObject({ backend: "fs" });
    await call("storage.run", { request: { kind: "set-cache", capacity_bytes: 2048 } });
    expect(backend.tools.some(([t, a]) => t === "volume_cache_set" && a.capacity_bytes === 2048)).toBe(true);
    expect(await call("storage.run", { request: { kind: "test", update: { backend: "fs", dry_run: true } } })).toMatchObject({ ok: true, applied: false });
    await call("storage.run", { request: { kind: "reveal", path: "~/Cua Volume" } });
    expect(system.revealed).toEqual([path.join(home, "Cua Volume")]);
    expect(await code("storage.run", { request: { kind: "reveal", path: "/etc/hosts" } })).toBe("bad_args");
    expect(await code("storage.run", { request: { kind: "open-url", url: "https://x" } })).toBe("bad_args");
    expect(await code("storage.run", { request: { kind: "nope" } })).toBe("bad_args");
  });

  it("the Volume reads its tools and reveals only paths in the home folder", async () => {
    const o = (await call("volume.overview")) as Record<string, unknown>;
    expect(o).toMatchObject({ os: "macos", home, grants: [], sync: null });
    expect(o.requests).toHaveLength(1);
    await call("volume.approve", { id: "q1" });
    await call("volume.resolve", { path: "public/plan.md" });
    expect(backend.tools.filter(([t]) => t === "volume_approve" || t === "volume_sync_resolve").map(([t, a]) => [t, a])).toEqual([
      ["volume_approve", { request_id: "q1" }],
      ["volume_sync_resolve", { path: "public/plan.md" }],
    ]);
    await call("volume.reveal", { path: "~/Cua Volume/public" });
    expect(system.revealed.at(-1)).toBe(path.join(home, "Cua Volume", "public"));
    expect(await code("volume.reveal", { path: "/etc/hosts" })).toBe("bad_args");
    expect(await code("volume.reveal", { path: "~/../other" })).toBe("bad_args");
  });

  it("pages without the daemon say unsupported (the store reads as none)", async () => {
    await make({ servicesIn: false });
    expect(await code("volume.overview")).toBe("unsupported");
    expect(await code("notifications.list")).toBe("unsupported");
    expect(await code("storage.run", { request: { kind: "mount" } })).toBe("unsupported");
    expect(await call("volume.storage")).toBeNull();
  });

  it("lists the feed, posts what is new once, and marks it read", async () => {
    const parts = hostParts(bridge.context);
    const list = (await call("notifications.list")) as { id: string; read: boolean; atMs: number }[];
    expect(list).toMatchObject([{ id: "n1", read: false, atMs: 1000 }]);
    // The first read marks the feed seen without posting it (the core's first run).
    expect(system.notes).toEqual([]);
    expect(model.settings.notificationsSeenMs).toBe(1000n);
    backend.notifications = [...backend.notifications, { id: "n2", at_ms: 2000, agent: "ada", kind: "request", title: "Ada asks", body: "May I read public/?\nmore", read: false }];
    await parts.notifications.poll();
    await parts.notifications.poll();
    expect(system.notes).toEqual([{ id: "agent-n2", title: "Ada asks", body: "May I read public/?", route: "/notifications" }]);
    expect(model.settings.notificationsSeenMs).toBe(2000n);
    await call("notifications.markAllRead");
    expect(parts.notifications.feed[0]?.read).toBe(true);
  });

  // MARK: The first run

  it("is due until the page finishes it, which applies Launch at login and asks for Local Network", async () => {
    let asked = 0;
    model.host.localNetwork = { request: () => asked++ };
    expect(await call("onboarding.get")).toEqual({ completed: false, mode: "client" });
    events.length = 0;
    expect(await call("onboarding.complete", { mode: "host", launchAtLogin: true })).toEqual({ completed: true, mode: "host" });
    expect(asked).toBe(1);
    expect(system.loginItem.calls).toEqual(["register"]);
    expect(model.settings.launchAtLogin).toBe(true);
    expect(events.some((e) => e.event === "session.changed")).toBe(true);
    expect(await code("onboarding.complete", { mode: "server" })).toBe("bad_args");
  });

  it("the page reads the first run from the session (Electron), and finishes it", async () => {
    const win = { cuaDesktop: { invoke: (_c: string, request: unknown) => bridge.registry.dispatch(request), platform: "darwin" }, open: () => null };
    const adapter = await pageAdapter(win);
    expect(await adapter.call("session.get", {})).toMatchObject({ onboarding: { completed: false, mode: "client" } });
    await adapter.call("session.completeOnboarding", { mode: "client", launchAtLogin: false });
    expect(await adapter.call("session.get", {})).toMatchObject({ onboarding: { completed: true } });
    expect(system.loginItem.calls).toEqual(["unregister"]);
    adapter.dispose?.();
  });

  // MARK: The menu bar item and the tray

  it("counts the Spaces the page lists, not the machines' own desktops", async () => {
    await call("host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3211" } });
    backend.rowsNow = [
      ...backend.rowsNow,
      { ...backend.rowsNow[2]!, id: "relay:m1", name: "studio-mac", provider: "relay", host: undefined },
      { ...backend.rowsNow[2]!, id: "relay:m1/space-1", name: "on-studio", provider: "relay", host: "m1" },
    ];
    await model.refresh();
    const real = realSpaces(model.spaces);
    expect(real.map((s) => s.id)).not.toContain("relay:m1");
    expect(real.map((s) => s.id)).toContain("relay:m1/space-1");
    expect(model.spaces.some((s) => isMachineDesktop(s))).toBe(true);
    const items = menuItems(bridge.context);
    expect(items[0]!.label).toBe(native.appStatusLine(real.length));
    expect(items.map((i) => i.id)).toEqual(expect.arrayContaining(["open", "newSpace", "settings", "quit"]));
  });

  it("says the launch is still waiting first", async () => {
    model.startup.phase = { kind: "starting" };
    const items = menuItems(bridge.context);
    expect(items[0]).toMatchObject({ id: "status", label: "Starting Cua…", enabled: false });
    expect(items[1]!.id).toBe("separator");
  });
});
