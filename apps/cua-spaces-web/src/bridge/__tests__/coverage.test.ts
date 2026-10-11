// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { createElectronAdapter } from "../adapters/electron";
import { createTauriAdapter } from "../adapters/tauri";
import { createWebkitAdapter } from "../adapters/webkit";
import { HOST_COVERAGE, WEBKIT_HOST_ONLY } from "../coverage";
import { NEW_SPACE_ARGS } from "../ops/new-space";
import { AGENT_KEYS_ARGS } from "../ops/agent-keys";
import { KEYVAULT_MANAGE_ARGS } from "../ops/keyvault-manage";
import { KEYVAULT_SETUP_ARGS } from "../ops/keyvault-setup";
import { STREAM_ARGS } from "../ops/stream";
import type { HostWindow } from "../detect";
import { ELECTRON_BRIDGE_CHANNEL } from "../electron-channels";
import { OPERATIONS, type OpArgs, type OpName } from "../protocol";
import { ELECTRON_HOST_METHODS, WEBKIT_METHODS, type WebkitRequest } from "../webkit-protocol";

/** Arguments that take each operation down its usual path. */
const ARGS: { [K in OpName]: OpArgs<K> } = {
  "spaces.list": {},
  "spaces.create": { config: { image: "macos-tahoe", on: "local" } as never, pendingId: "pending:1" },
  "spaces.cancelCreate": { pendingId: "pending:1" },
  "spaces.open": { spaceId: "s1" },
  "spaces.setPower": { spaceId: "s1", on: true },
  "spaces.delete": { spaceId: "s1" },
  "machines.list": {},
  "host.status": {},
  "settings.get": {},
  "settings.set": { key: "telemetry", value: false },
  "settings.choose": { row: "auto-connect", option: "off" },
  "keyvault.overview": {},
  "keyvault.unlock": { passphrase: null },
  "keyvault.setUnattended": { itemIds: ["k1"], unattended: true },
  "keyvault.setDisabled": { disabled: true },
  "keyvault.approve": { requestId: "req-1", items: null },
  "keyvault.deny": { requestId: "req-1" },
  "keyvault.revokeGrant": { id: "grant-1" },
  "session.get": {},
  "session.signIn": {},
  "session.signOut": {},
  "session.completeOnboarding": { mode: "client" },
  "session.openExternal": { url: "https://cua.ai" },
  "agents.list": {},
  "agents.runs": { spaceId: "s1" },
  "agents.events": { spaceId: "s1", runId: "r1", cursor: 0 },
  "agents.pause": { name: "ada" },
  "agents.resume": { name: "ada" },
  "agents.setup": {},
  "agents.configure": { agents: null },
  ...NEW_SPACE_ARGS,
  ...AGENT_KEYS_ARGS,
  ...KEYVAULT_SETUP_ARGS,
  ...KEYVAULT_MANAGE_ARGS,
  "teleport.catalog": { spaceId: "s1" },
  "teleport.entryForPath": { path: "/Applications/Slack.app" },
  "teleport.windows": {},
  "teleport.remoteWindows": { spaceId: "s1" },
  "teleport.icon": { spaceId: "s1", icon: { kind: "host", path: "/Applications/Slack.app" } },
  "teleport.thumbnail": { spaceId: "s1", thumbnail: { kind: "host-window", windowId: 41 } },
  "teleport.plan": { spaceId: "s1", entry: { json: "{}" } as never, move: "app_only", files: [], sensitiveGroups: [] },
  "teleport.run": { spaceId: "s1", plan: { json: "{}" } as never, consent: { approved: true, acknowledgeSensitive: false }, runId: "run-1" },
  "teleport.sites": { providerId: "chrome" },
  "teleport.remembered": { providerId: "chrome", spaceId: "s1" },
  "teleport.streamWindow": { spaceId: "s1", spaceName: "Aurora", windowId: "w-1", appName: "Firefox", title: "Mozilla Firefox" },
  "sharing.list": { spaceId: "s1" },
  "sharing.share": { spaceId: "s1", who: "bob@example.com", role: "viewer" },
  "sharing.unshare": { spaceId: "s1", who: "bob@example.com" },
  "volume.overview": {},
  "volume.storage": {},
  "volume.storageSet": { update: { backend: "fs", s3: null, access_key_id: null, secret_access_key: null, dry_run: true } },
  "volume.mount": {},
  "volume.unmount": {},
  "volume.approve": { id: "q1" },
  "volume.deny": { id: "q1" },
  "volume.revoke": { id: "g1" },
  "volume.resolve": { path: "public/plan.md" },
  "volume.reveal": { path: "/Users/ada/Cua Volume/public" },
  "agents.setupDriver": { agents: ["codex"] },
  "about.get": {},
  "about.set": { channel: "beta" },
  "about.checkNow": {},
  "experiments.get": {},
  "experiments.set": { experiments: { cuaVolume: true } },
  "loginItem.get": {},
  "loginItem.set": { on: true },
  "loginItem.openSettings": {},
  "devices.get": {},
  "devices.enroll": {},
  "devices.checkEnrolled": {},
  "devices.approve": { code: "K7QX-M2RP", deviceId: null },
  "devices.rename": { id: "dev_p", name: "Work laptop" },
  "devices.revoke": { id: "dev_p" },
  "devices.confirmMachine": { id: "m1" },
  "storage.get": {},
  "storage.run": { request: { kind: "clear-cache" } },
  "notifications.list": {},
  "notifications.markAllRead": {},
  "telemetry.track": { signals: [{ type: "step", step: "app_launched", ok: true }] },
  "spaces.usage": { spaceId: "s1" },
  "spaces.windows": { spaceId: "s1" },
  "stream.pip": { spaceId: "s1", command: { type: "open", row: "desktop" } },
  "spaces.thumbnail": { spaceId: "s1" },
  "spaces.chooseFiles": {},
  "spaces.droppedFiles": { names: ["notes.txt"] },
  "spaces.sendFiles": { spaceId: "s1", paths: ["/tmp/notes.txt"] },
  "host.setUp": { request: { mode: "relay" } },
  "host.action": { action: "stop-sharing" },
  "host.openSettings": { url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" },
  "startup.get": {},
  "startup.act": { action: "allowAccess" },
  ...STREAM_ARGS,
};

const unsupportedOn = (op: OpName, host: "webkit" | "tauri") =>
  (HOST_COVERAGE[op] as { unsupported?: { webkit?: string; tauri?: string } }).unsupported?.[host];

const settingsPage = { page: { title: "Settings", sections: [] } };
const keyvault = { availability: "ready", overview: { items: [] }, busy: false, error: null };
const session = { identity: null, signedIn: false, cloudConfigured: false, signIn: "idle" };

/** What the SwiftUI host answers each method with (shapes only). */
function webkitResult(method: string): unknown {
  if (method.startsWith("settings.")) return settingsPage;
  if (method.startsWith("session.")) return session;
  switch (method) {
    case "spaces.list":
    case "spaces.setPower":
    case "spaces.delete":
      return { loaded: true, selectedId: null, spaces: [] };
    case "machines.list":
      return { devices: null, signedIn: false };
    case "spaces.add":
    case "spaces.create":
      return { id: "s2", name: "Studio", os: "linux", status: "running", detail: "", lastUsedAt: 1 };
    case "teleport.entryForPath":
      return { id: "app", name: "App", capability: "full", moves: ["appOnly"], sensitiveGroups: [] };
    case "teleport.plan":
      return { app: { id: "app", name: "App" }, spaceId: "s1", moves: "appOnly", steps: [], consent: [], json: "{}" };
    case "spaces.cancelCreate":
      return { id: "pending:1", state: "cancelled", message: "" };
    case "onboarding.get":
    case "onboarding.complete":
      return { completed: true, mode: "client" };
    case "host.status":
    case "host.setUp":
    case "host.action":
      return { configured: false, sharing: false, serviceInstalled: false, serviceRunning: false, serviceKind: "process", clients: [], permissions: [], shareDesktop: true, provideSpaces: false, maxSpaces: 0 };
    case "keyvault.approve":
      return { id: "grant-1", requestId: "req-1", items: [] };
    case "keyvault.revokeGrant":
      return 1;
    default:
      return method.startsWith("keyvault.") ? keyvault : null;
  }
}

function webkitWindow(sent: string[]): HostWindow {
  return {
    webkit: {
      messageHandlers: {
        cua: {
          postMessage: (m: unknown) => {
            const req = m as WebkitRequest;
            sent.push(req.method);
            return Promise.resolve({ id: req.id, ok: true, result: webkitResult(req.method) });
          },
        },
      },
    },
    addEventListener: () => {},
    removeEventListener: () => {},
    open: () => null,
  } as HostWindow;
}

/** How a call failed: its code, `unsupported` for an operation the adapter lacks. */
const failure = (e: { name?: string; code?: string }) => (e.name === "UnsupportedOperationError" ? "unsupported" : (e.code ?? "error"));

/** `static let methods = [...]` in WebUIBridge.swift. */
function swiftMethods(): string[] {
  // Tests run from apps/cua-spaces-web.
  const path = resolve(process.cwd(), "../cua-spaces-macos/Sources/CuaSpacesMacKit/WebHost/WebUIBridge.swift");
  const src = readFileSync(path, "utf8");
  const list = /static let methods = \[([\s\S]*?)\]/.exec(src)?.[1] ?? "";
  return [...list.matchAll(/"([^"]+)"/g)].map((m) => m[1]!);
}

describe("bridge contract coverage", () => {
  it("lists every operation, and only those", () => {
    expect(Object.keys(HOST_COVERAGE).sort()).toEqual([...OPERATIONS].sort());
  });

  it("webkit: each operation reaches the SwiftUI host, or says why not", () => {
    const missing = OPERATIONS.filter((op) => {
      const w = HOST_COVERAGE[op].webkit as { methods: readonly string[]; byDesign?: string };
      return w.methods.length === 0 && !w.byDesign && !unsupportedOn(op, "webkit");
    });
    expect(missing).toEqual([]);
  });

  it("webkit: the methods it names are the ones the Swift host routes", () => {
    const swift = swiftMethods();
    expect([...swift].sort()).toEqual([...WEBKIT_METHODS].sort());
    for (const op of OPERATIONS) {
      for (const m of HOST_COVERAGE[op].webkit.methods) expect(swift, `${op} -> ${m}`).toContain(m);
    }
  });

  it("webkit: every method the Swift host routes is called by an operation, or says why not", () => {
    const called = new Set(OPERATIONS.flatMap((op) => [...HOST_COVERAGE[op].webkit.methods]));
    const extra = swiftMethods().filter((m) => !called.has(m as never) && !WEBKIT_HOST_ONLY[m as keyof typeof WEBKIT_HOST_ONLY]);
    expect(extra, "routed by Swift, called by no operation: add it to WEBKIT_HOST_ONLY with a reason").toEqual([]);
    // Stale notes: called now, or no longer routed.
    const stale = Object.keys(WEBKIT_HOST_ONLY).filter((m) => called.has(m as never) || !swiftMethods().includes(m));
    expect(stale, "stale WEBKIT_HOST_ONLY entries").toEqual([]);
  });

  it("webkit: no unsupported or by-design note for an operation the Swift host routes under its own name", () => {
    const swift = swiftMethods();
    const stale = OPERATIONS.filter((op) => {
      const w = HOST_COVERAGE[op].webkit as { methods: readonly string[]; byDesign?: string };
      const noted = unsupportedOn(op, "webkit") || (w.methods.length === 0 && w.byDesign);
      return noted && swift.includes(op);
    });
    expect(stale, "the Swift host routes these now: list the method and drop the note").toEqual([]);
  });

  it("webkit: the adapter calls only the listed methods, and fails only by design", async () => {
    for (const op of OPERATIONS) {
      const sent: string[] = [];
      const cov = HOST_COVERAGE[op].webkit as { methods: readonly string[]; byDesign?: string };
      const outcome = await createWebkitAdapter(webkitWindow(sent))
        .call(op, ARGS[op] as never)
        .then(() => null, failure);
      expect(sent.filter((m) => !cov.methods.includes(m)), op).toEqual([]);
      if (unsupportedOn(op, "webkit")) {
        expect(outcome, op).toBe("unsupported");
        expect(sent, op).toEqual([]);
        continue;
      }
      if (cov.methods.length > 0) expect(sent.length, `${op} sent nothing`).toBeGreaterThan(0);
      if (outcome !== null) expect(cov.byDesign, `${op} failed with ${outcome}`).toBeTruthy();
      expect(outcome, op).not.toBe("unsupported");
    }
  });

  it("electron: the shell gets the SwiftUI host's methods on its one channel, plus its own", async () => {
    for (const op of OPERATIONS) {
      const cov = HOST_COVERAGE[op] as { webkit: { methods: readonly string[] }; electron?: { methods: readonly string[] } };
      const methods = cov.electron?.methods ?? cov.webkit.methods;
      for (const m of cov.electron?.methods ?? []) expect([...ELECTRON_HOST_METHODS, ...WEBKIT_METHODS], `${op} -> ${m}`).toContain(m);
      const channels = new Set<string>();
      const sent: string[] = [];
      const win = {
        cuaDesktop: {
          invoke: async (channel: string, m: unknown) => {
            const req = m as WebkitRequest;
            channels.add(channel);
            sent.push(req.method);
            return { id: req.id, ok: true, result: webkitResult(req.method) };
          },
        },
        open: () => null,
      } as unknown as HostWindow;
      const outcome = await createElectronAdapter(win)
        .call(op, ARGS[op] as never)
        .then(() => null, failure);
      expect([...channels], op).toEqual(sent.length ? [ELECTRON_BRIDGE_CHANNEL] : []);
      expect(sent.filter((m) => !methods.includes(m)), op).toEqual([]);
      if (!cov.electron && unsupportedOn(op, "webkit")) {
        expect(outcome, op).toBe("unsupported");
        continue;
      }
      expect(outcome, op).not.toBe("unsupported");
      if (methods.length > 0) expect(sent.length, `${op} sent nothing`).toBeGreaterThan(0);
    }
  });

  it("tauri: the adapter calls only the listed commands, and none is unsupported", async () => {
    for (const op of OPERATIONS) {
      const seen: string[] = [];
      const win = { __TAURI_INTERNALS__: { invoke: async (c: string) => (seen.push(c), null) }, __CUA_UI_STORAGE__: {} } as unknown as HostWindow;
      const outcome = await createTauriAdapter(win)
        .call(op, ARGS[op] as never)
        .then(() => null, failure);
      if (unsupportedOn(op, "tauri")) {
        expect(outcome, op).toBe("unsupported");
        expect(seen, op).toEqual([]);
        continue;
      }
      expect(outcome, op).not.toBe("unsupported");
      // An empty list: the page answers from the shell's UI storage, with no command.
      if (HOST_COVERAGE[op].tauri.length > 0) expect(seen.length, `${op} sent nothing`).toBeGreaterThan(0);
      expect(seen.filter((c) => !HOST_COVERAGE[op].tauri.includes(c as never)), op).toEqual([]);
    }
  });
});
