// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";
import { HostError, UnsupportedOperationError, isUnsupported } from "../adapter";
import { createElectronAdapter } from "../adapters/electron";
import { TAURI_UNSUPPORTED, createTauriAdapter } from "../adapters/tauri";
import { createWebkitAdapter, toPending } from "../adapters/webkit";
import { hostState, mergeMachineRows, toMachines } from "../derive";
import { hostFailure } from "../this-machine";
import { readNewSpaceSession, resetNewSpaceSession } from "../new-space";
import type { HostWindow } from "../detect";
import { ELECTRON_BRIDGE_CHANNEL, ELECTRON_EVENT_CHANNEL } from "../electron-channels";
import { OPERATIONS, type HostEvent } from "../protocol";
import { WEBKIT_EVENT, WEBKIT_METHODS, WEBKIT_OPEN_SETTINGS_EVENT, type WebkitRequest } from "../webkit-protocol";
import { tick } from "./testCore";

/* ---- Tauri ---------------------------------------------------------------- */

function tauriHost(answers: Record<string, unknown> = {}, withEvents = true) {
  const calls: [string, Record<string, unknown> | undefined][] = [];
  const listeners = new Map<string, (e: { payload: unknown }) => void>();
  const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push([cmd, args]);
    if (cmd in answers) {
      const a = answers[cmd];
      if (a instanceof Error) throw a;
      return a;
    }
    return null;
  });
  const win: HostWindow = {
    __TAURI_INTERNALS__: { invoke },
    ...(withEvents
      ? {
          __TAURI__: {
            event: {
              listen: async (name, handler) => {
                listeners.set(name, handler);
                return () => listeners.delete(name);
              },
            },
          },
        }
      : {}),
    __CUA_UI_STORAGE__: { "cua.settings.appearance": "dark", "cua.settings.menuBar": "true" },
  };
  return { win, calls, listeners, invoke };
}

describe("tauri adapter", () => {
  it("maps operations onto the existing invoke commands", async () => {
    const { win, calls } = tauriHost({ list_spaces: [], keyvault_set_unattended: [], set_space_power: {} });
    const a = createTauriAdapter(win);
    await a.call("spaces.list", {});
    await a.call("spaces.create", { config: { image: "ubuntu" }, pendingId: "pending:1" });
    await a.call("spaces.setPower", { spaceId: "s1", on: false });
    await a.call("spaces.delete", { spaceId: "s1" });
    await a.call("spaces.cancelCreate", { pendingId: "pending:1" });
    await a.call("keyvault.overview", {});
    await a.call("keyvault.unlock", {});
    await a.call("keyvault.unlock", { passphrase: "hunter22" });
    await a.call("keyvault.setUnattended", { itemIds: ["a"], unattended: true });
    await a.call("keyvault.approve", { requestId: "r", items: null });
    await a.call("keyvault.deny", { requestId: "r" });
    await a.call("keyvault.revokeGrant", { id: "g" });
    await a.call("keyvault.setDisabled", { disabled: true });
    await a.call("session.signIn", {});
    await a.call("session.signOut", {});
    await a.call("session.completeOnboarding", { mode: "client" });
    expect(calls).toEqual([
      ["list_spaces", undefined],
      ["create_space", { config: { image: "ubuntu" }, pendingId: "pending:1" }],
      ["set_space_power", { spaceId: "s1", on: false }],
      ["delete_space", { spaceId: "s1" }],
      ["cancel_create", { pendingId: "pending:1" }],
      ["keyvault_overview", undefined],
      ["keyvault_unlock", undefined],
      ["keyvault_unlock_passphrase", { passphrase: "hunter22" }],
      ["keyvault_set_unattended", { args: { itemIds: ["a"], unattended: true } }],
      ["keyvault_approve", { requestId: "r", items: null }],
      ["keyvault_deny", { requestId: "r" }],
      ["keyvault_revoke_grant", { id: "g" }],
      ["keyvault_set_disabled", { disabled: true }],
      ["begin_sign_in", undefined],
      ["sign_out", undefined],
      ["complete_onboarding", { mode: "client" }],
    ]);
  });

  it("answers every operation", async () => {
    const app = { id: "slack", name: "Slack", capability: "full", moves: ["app_only"] };
    const { win } = tauriHost({
      fleet_status: { configured: false, authMode: "none" },
      list_hosts: [],
      teleport_entry_for_path: app,
      teleport_plan: { app, space_id: "s", moves: "app_only", steps: [], consent: [], sensitive: false, total_bytes: 0, warnings: [] },
      teleport_run: { app_id: "slack", installed: [], sent: [], imported: [], skipped: [], launched: true },
    });
    const a = createTauriAdapter(win);
    for (const op of OPERATIONS) {
      const teleport = { path: "/Applications/Slack.app", icon: { kind: "none" }, thumbnail: { kind: "none" }, entry: { json: "{}" }, plan: { json: "{}" }, consent: { approved: true }, files: [], sensitiveGroups: [] };
      const args = {
        config: {},
        pendingId: "p",
        key: "theme",
        value: "dark",
        name: "ada",
        spaceId: "s",
        agents: null,
        signals: [],
        // storage.run takes a StorageRequest; host.setUp the host form's request.
        request: op === "host.setUp" ? { mode: "relay" } : { kind: "clear-cache" },
        action: "stop-sharing",
        url: "x-apple.systempreferences:com.apple.preference.security",
        ...teleport,
      } as never;
      if (TAURI_UNSUPPORTED.includes(op)) await expect(a.call(op, args)).rejects.toBeInstanceOf(UnsupportedOperationError);
      else await expect(a.call(op, args)).resolves.not.toBeInstanceOf(Error);
    }
  });

  it("maps agents onto agents_tool, list_space_agents and the agent setup commands", async () => {
    const { win, calls } = tauriHost({
      agents_tool: {
        agents: [{ name: "ada", harness: "claude-code", space: "local:dev", paused: false, space_state: "running", run_id: "r1", saved_ms: 5, last_error: null }],
      },
      list_space_agents: [{ runId: "r1", agent: "claude-code", status: "idle", reason: "", summary: "fix", createdAt: 1, phase: "waiting", turn: 1 }],
      agent_setup_detect: [],
      agent_setup_configure: [],
    });
    const a = createTauriAdapter(win);
    expect(await a.call("agents.list", {})).toEqual([
      { name: "ada", harness: "claude-code", space: "local:dev", paused: false, spaceState: "running", runId: "r1", savedMs: 5, lastError: null },
    ]);
    expect(await a.call("agents.runs", { spaceId: "local:dev" })).toHaveLength(1);
    await a.call("agents.pause", { name: "ada" });
    await a.call("agents.resume", { name: "ada" });
    await a.call("agents.setup", {});
    await a.call("agents.configure", { agents: ["hermes"] });
    expect(calls).toEqual([
      ["agents_tool", { tool: "persistent_agent_list", args: {} }],
      ["list_space_agents", { spaceId: "local:dev" }],
      ["agents_tool", { tool: "agent_pause", args: { name: "ada" } }],
      ["agents_tool", { tool: "agent_resume", args: { name: "ada" } }],
      ["agent_setup_detect", undefined],
      ["agent_setup_configure", { agents: ["hermes"] }],
    ]);
    const err = await a.call("agents.events", { spaceId: "local:dev", runId: "r1", cursor: 0 }).catch((e: unknown) => e);
    expect(isUnsupported(err)).toBe(true);
  });

  it("puts this machine first, then list_hosts", async () => {
    const { win } = tauriHost({
      host_status: { machineId: "m1", name: "Studio" },
      get_environment: { platform: "macos" },
      list_hosts: [
        { id: "m1", name: "Studio", via: "relay", online: true, os: "macos", limits: [] },
        { id: "m2", name: "Linux box", via: "relay", online: false, os: "linux", limits: [] },
      ],
    });
    const rows = await createTauriAdapter(win).call("machines.list", {});
    expect(rows.map((r) => [r.id, r.name, r.via, Boolean(r.current)])).toEqual([
      ["m1", "Studio", "local", true],
      ["m2", "Linux box", "relay", false],
    ]);
    expect(rows[0]?.host).toMatchObject({ machineId: "m1" });
  });

  it("reads UI settings from the shell's storage and writes each key to its command", async () => {
    const { win, calls } = tauriHost({
      telemetry_status: { enabled: false, sourceKind: "config" },
      get_default_location: { value: "cloud", source: "config", path: "x" },
      login_item_status: "requiresApproval",
      login_item_set: "notRegistered",
    });
    const a = createTauriAdapter(win);
    const s = await a.call("settings.get", {});
    expect(s.values).toEqual({
      theme: "dark",
      menuBar: true,
      hotkey: "⌘⇧Space",
      telemetry: false,
      defaultLocation: "cloud",
      launchAtLogin: true,
      updateChannel: null,
    });
    await a.call("settings.set", { key: "theme", value: "light" });
    await a.call("settings.set", { key: "telemetry", value: true });
    await a.call("settings.set", { key: "defaultLocation", value: "local" });
    await a.call("settings.set", { key: "launchAtLogin", value: false });
    await expect(a.call("settings.set", { key: "updateChannel", value: "beta" })).rejects.toThrow(/not available/);
    expect(win.__CUA_UI_STORAGE__?.["cua.settings.appearance"]).toBe("light");
    expect(calls.filter(([c]) => !["telemetry_status", "get_default_location", "login_item_status"].includes(c))).toEqual([
      ["ui_storage_set", { key: "cua.settings.appearance", value: "light" }],
      ["telemetry_set_enabled", { enabled: true }],
      ["set_default_location", { on: "local" }],
      ["login_item_set", { on: false }],
    ]);
  });

  it("maps Tauri events to host events", async () => {
    const { win, listeners } = tauriHost();
    const a = createTauriAdapter(win);
    const got: HostEvent[] = [];
    a.subscribe((e) => got.push(e));
    await tick();
    listeners.get("spaces:changed")?.({ payload: null });
    listeners.get("spaces:create-progress")?.({ payload: { pendingId: "p", phase: "pulling", detail: "" } });
    listeners.get("auth:signed-in")?.({ payload: { identity: "ada@example.com" } });
    listeners.get("auth:sign-in-failed")?.({ payload: null });
    expect(got).toEqual([
      { type: "spaces.changed" },
      { type: "spaces.createProgress", progress: { pendingId: "p", phase: "pulling", detail: "" } },
      { type: "session.signedIn", identity: "ada@example.com" },
      { type: "session.signInFailed", reason: "Sign-in failed" },
    ]);
    expect(a.pollSpacesMs).toBeUndefined();
    a.dispose?.();
  });

  it("asks to be polled without the global event API", () => {
    expect(createTauriAdapter(tauriHost({}, false).win).pollSpacesMs).toBe(10_000);
  });

  it("passes errors through", async () => {
    const { win } = tauriHost({ list_spaces: new Error("daemon down") });
    await expect(createTauriAdapter(win).call("spaces.list", {})).rejects.toThrow("daemon down");
  });
});

/* ---- Electron --------------------------------------------------------------- */

function electronHost(handle: (channel: string, args: unknown) => unknown) {
  const subs = new Map<string, (p: unknown) => void>();
  const invoke = vi.fn(async (channel: string, args?: unknown) => handle(channel, args));
  const win: HostWindow = {
    cuaDesktop: {
      invoke,
      on: (channel, listener) => {
        subs.set(channel, listener);
        return () => subs.delete(channel);
      },
    },
  };
  return { win, invoke, subs };
}

describe("electron adapter", () => {
  /** Answers each bridge request with `answer(method)`, in the webkit envelope. */
  const bridged = (answer: (method: string, args: unknown) => unknown) => (channel: string, m: unknown) => {
    expect(channel).toBe(ELECTRON_BRIDGE_CHANNEL);
    const req = m as WebkitRequest;
    return { id: req.id, ...(answer(req.method, req.args) as object) };
  };

  it("sends the SwiftUI host's methods on the one bridge channel and maps the answers", async () => {
    const space = { id: "local:a", name: "A", os: "linux", status: "running", detail: "", lastUsedAt: 1 };
    const { win, invoke } = electronHost(bridged(() => ({ ok: true, result: { loaded: true, selectedId: null, spaces: [{ space, deleting: false }] } })));
    const rows = await createElectronAdapter(win).call("spaces.list", {});
    expect(rows).toMatchObject([{ id: "local:a", name: "A", os: "linux", provider: "cloud" }]);
    expect(invoke).toHaveBeenCalledWith(ELECTRON_BRIDGE_CHANNEL, expect.objectContaining({ method: "spaces.list", args: {} }));
    expect(createElectronAdapter(win).mode).toBe("electron");
  });

  it("asks the shell for a media ticket (only Electron draws video in the page)", async () => {
    const ticket = { wsUrl: "ws://127.0.0.1:5123/v1/bridge/media?ticket=t", expiresAt: null };
    const { win, invoke } = electronHost(bridged(() => ({ ok: true, result: ticket })));
    expect(await createElectronAdapter(win).call("spaces.openStream", { spaceId: "local:a", tier: "tile" })).toEqual(ticket);
    expect(invoke).toHaveBeenCalledWith(ELECTRON_BRIDGE_CHANNEL, expect.objectContaining({ method: "spaces.openStream", args: { spaceId: "local:a", tier: "tile" } }));
  });

  it("reads the first run from the shell and finishes it there (the SwiftUI app's is native)", async () => {
    const sent: [string, unknown][] = [];
    let completed = false;
    const { win } = electronHost(
      bridged((method, args) => {
        sent.push([method, args]);
        if (method === "onboarding.get" || method === "onboarding.complete") return { ok: true, result: { completed, mode: "host" } };
        if (method === "onboarding.complete") completed = true;
        return { ok: true, result: { identity: null, signedIn: false, cloudConfigured: false, signIn: "idle" } };
      }),
    );
    const a = createElectronAdapter(win);
    expect((await a.call("session.get", {})).onboarding).toEqual({ completed: false, mode: "host" });
    await a.call("session.completeOnboarding", { mode: "host", launchAtLogin: false });
    expect(sent.at(-1)).toEqual(["onboarding.complete", { mode: "host", launchAtLogin: false }]);
    await a.call("session.completeOnboarding", { mode: "client" });
    expect(sent.at(-1)).toEqual(["onboarding.complete", { mode: "client" }]);
  });

  it("names this machine for the shell's platform", async () => {
    const { win } = electronHost(bridged((method) => ({ ok: true, result: method === "spaces.list" ? { loaded: true, selectedId: null, spaces: [] } : { devices: null, signedIn: false } })));
    win.cuaDesktop!.platform = "win32";
    const rows = await createElectronAdapter(win).call("machines.list", {});
    expect(rows[0]).toMatchObject({ id: "this-mac", name: "This PC", os: "windows", current: true });
  });

  it("turns an error envelope into a HostError with its code", async () => {
    const { win } = electronHost(bridged(() => ({ ok: false, error: { message: "Touch ID was cancelled", code: "cancelled" } })));
    const err = await createElectronAdapter(win).call("keyvault.setUnattended", { itemIds: ["k"], unattended: true }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(HostError);
    expect(err).toMatchObject({ message: "Touch ID was cancelled", code: "cancelled" });
  });

  it("rejects a reply without an envelope", async () => {
    const { win } = electronHost(() => [1, 2]);
    await expect(createElectronAdapter(win).call("spaces.list", {})).rejects.toMatchObject({ code: "protocol" });
  });

  it("delivers the host's cua:event pushes", () => {
    const { win, subs } = electronHost(bridged(() => ({ ok: true, result: null })));
    const a = createElectronAdapter(win);
    const got: HostEvent[] = [];
    const off = a.subscribe((e) => got.push(e));
    subs.get(ELECTRON_EVENT_CHANNEL)?.({ event: "keyvault.changed" });
    subs.get(ELECTRON_EVENT_CHANNEL)?.({ event: "spaces.changed" });
    off();
    subs.get(ELECTRON_EVENT_CHANNEL)?.({ event: "settings.changed" });
    expect(got).toEqual([{ type: "keyvault.changed" }, { type: "spaces.changed" }, { type: "machines.changed" }]);
    a.dispose?.();
    expect(subs.size).toBe(0);
  });
});

describe("electron adapter: the menus", () => {
  it("turns the menu's Settings… into the window event the page's key bindings answer, as the SwiftUI host sends it", () => {
    const subs = new Map<string, (p: unknown) => void>();
    const target = new EventTarget();
    const win = Object.assign(target, {
      cuaDesktop: { invoke: vi.fn(), on: (channel: string, listener: (p: unknown) => void) => (subs.set(channel, listener), () => subs.delete(channel)) },
    }) as unknown as HostWindow;
    const seen: unknown[] = [];
    target.addEventListener(WEBKIT_EVENT, (e) => seen.push((e as CustomEvent).detail));
    const a = createElectronAdapter(win);
    subs.get(ELECTRON_EVENT_CHANNEL)?.({ event: WEBKIT_OPEN_SETTINGS_EVENT, payload: null });
    expect(seen).toEqual([{ event: WEBKIT_OPEN_SETTINGS_EVENT, payload: null }]);
    a.dispose?.();
  });
});

/* ---- WebKit (the SwiftUI host, WebUIBridge.swift) ------------------------------ */

type Answer = { ok: true; result: unknown } | { ok: false; error: { code: string; message: string } };

function webkitHost(answer: (req: WebkitRequest) => Answer | Promise<Answer> | undefined) {
  const sent: WebkitRequest[] = [];
  const target = new EventTarget();
  const win: HostWindow = {
    webkit: {
      messageHandlers: {
        cua: {
          postMessage: (m: unknown) => {
            const req = m as WebkitRequest;
            sent.push(req);
            const a = answer(req);
            return a === undefined ? new Promise(() => {}) : Promise.resolve(a).then((r) => ({ id: req.id, ...r }));
          },
        },
      },
    },
    addEventListener: (t, l) => target.addEventListener(t, l),
    removeEventListener: (t, l) => target.removeEventListener(t, l),
  };
  const emit = (event: string, payload: unknown = null) => target.dispatchEvent(new CustomEvent("cua:event", { detail: { event, payload } }));
  return { win, sent, emit };
}

const ok = (result: unknown): Answer => ({ ok: true, result });

const wkSpace = {
  id: "relay:mac-mini/qa",
  name: "QA",
  os: "windows",
  status: "suspended",
  detail: "Suspended",
  lastUsedAt: 1_790_000_000_000,
  provider: "relay",
  sdk: { features: ["desktop_stream"], spacesdVersion: "0.6.0", reachable: true, error: null },
  osPrettyName: "Windows 11",
  host: "mac-mini",
  hostName: "Mac mini",
  power: { control: "stop", off: true, turningOn: null, error: null },
};

const settingsPage = {
  page: {
    title: "Settings",
    sections: [
      {
        id: "general",
        rows: [
          { id: "notch", kind: "choice", label: "", enabled: true, options: [{ id: "show", label: "", active: false }, { id: "hide", label: "", active: true }] },
          { id: "default-location", kind: "choice", label: "", enabled: false, help: "Set by CUA_ON", options: [{ id: "local", label: "", active: false }, { id: "cloud", label: "", active: true }] },
        ],
      },
      { id: "privacy", rows: [{ id: "telemetry", kind: "choice", label: "", enabled: true, options: [{ id: "on", label: "", active: false }, { id: "off", label: "", active: true }] }] },
    ],
  },
};

const settingsWithLogin = {
  ...settingsPage,
  updateChannel: "beta",
  page: {
    ...settingsPage.page,
    sections: [
      { id: "general", rows: [{ id: "launch-at-login", kind: "toggle", label: "", enabled: true, options: [{ id: "on", label: "", active: true }, { id: "off", label: "", active: false }] }] },
      ...settingsPage.page.sections,
    ],
  },
};

describe("webkit adapter", () => {
  it("speaks the SwiftUI host's envelope and maps spaces back to registry rows", async () => {
    const { win, sent } = webkitHost(() => ok({ loaded: true, selectedId: null, spaces: [{ space: wkSpace, deleting: false }] }));
    const rows = await createWebkitAdapter(win).call("spaces.list", {});
    expect(sent[0]).toMatchObject({ method: "spaces.list", args: {} });
    expect(typeof sent[0]!.id).toBe("string");
    expect(rows).toEqual([
      expect.objectContaining({ id: "relay:mac-mini/qa", provider: "relay", os: "windows", host: "mac-mini", power: "stop", powerState: "stopped", reachable: true, features: ["desktop_stream"] }),
    ]);
  });

  it("says what the host said about its last list read (the SwiftUI app's rosterError)", async () => {
    let rosterError: string | null = "Could not refresh Spaces. Previously loaded rows may be out of date.";
    const { win } = webkitHost(() => ok({ loaded: true, selectedId: null, spaces: [{ space: wkSpace, deleting: false }], rosterError }));
    const a = createWebkitAdapter(win);
    expect(a.listNotice?.()).toBeNull();
    expect(await a.call("spaces.list", {})).toHaveLength(1);
    expect(a.listNotice?.()).toBe(rosterError);
    rosterError = null;
    await a.call("spaces.list", {});
    expect(a.listNotice?.()).toBeNull();
  });

  it("says the host's standing notice while it lasts (another app keeps its own daemon)", async () => {
    let daemonNotice: string | null = "Another Cua Spaces app (/Applications/Cua Spaces.app, cua 0.4.0) is running and keeps starting its own daemon, so this app uses that one.";
    const { win } = webkitHost(() => ok({ loaded: true, selectedId: null, spaces: [{ space: wkSpace, deleting: false }], rosterError: null, daemonNotice }));
    const a = createWebkitAdapter(win);
    expect(a.hostNotice?.()).toBeNull();
    await a.call("spaces.list", {});
    expect(a.hostNotice?.()).toBe(daemonNotice);
    expect(a.listNotice?.()).toBeNull();
    daemonNotice = null;
    await a.call("spaces.list", {});
    expect(a.hostNotice?.()).toBeNull();
  });

  it("keeps a failed create's reason from the host's own pending row", async () => {
    const error = "This Mac is already running two macOS VMs, the most Apple's macOS license allows at once.";
    const failed = { id: "pending:abc", name: "macOS", os: "macos", status: "suspended", detail: error, lastUsedAt: 1, provider: "local", progress: { phase: "booting", permille: 900, label: "Failed", error, cancellable: false, cancelling: false } };
    const { win } = webkitHost(() => ok({ loaded: true, selectedId: null, spaces: [{ space: failed, deleting: false }, { space: wkSpace, deleting: false }] }));
    const rows = await createWebkitAdapter(win).call("spaces.list", {});
    expect(rows[0]).toMatchObject({ id: "pending:abc", hostProgress: { label: "Failed", error } });
    // Only the host's creates carry it.
    expect(rows[1]!.hostProgress).toBeUndefined();
  });

  it("only calls methods the host routes", async () => {
    const { win, sent } = webkitHost((req) => (req.method === "settings.get" ? ok(settingsPage) : ok({ loaded: true, selectedId: null, spaces: [] })));
    const a = createWebkitAdapter(win);
    for (const op of OPERATIONS) await a.call(op, {} as never).catch(() => {});
    expect(sent.every((r) => (WEBKIT_METHODS as readonly string[]).includes(r.method))).toBe(true);
  });

  it("maps actions onto the host's methods and arguments", async () => {
    const { win, sent } = webkitHost((req) => (req.method === "settings.get" || req.method === "settings.choose" ? ok(settingsPage) : ok({ availability: "ready", overview: { items: [] }, busy: false, error: null })));
    const a = createWebkitAdapter(win);
    await a.call("spaces.setPower", { spaceId: "s1", on: true });
    await a.call("spaces.delete", { spaceId: "s1" });
    await a.call("spaces.delete", { spaceId: "s1", removeOnly: true });
    await a.call("keyvault.setUnattended", { itemIds: ["k1"], unattended: true });
    await a.call("keyvault.setUnattended", { itemIds: ["k1"], unattended: false });
    await a.call("keyvault.unlock", {});
    await a.call("settings.set", { key: "menuBar", value: false });
    await a.call("settings.set", { key: "telemetry", value: true });
    expect(sent.map((r) => [r.method, r.args])).toEqual([
      ["spaces.setPower", { id: "s1", on: true }],
      ["spaces.delete", { id: "s1" }],
      // "Remove from List": `AppModel.delete(_:removeOnly: true)`.
      ["spaces.delete", { id: "s1", removeOnly: true }],
      ["keyvault.unlock", { ids: ["k1"] }],
      ["keyvault.lock", { ids: ["k1"] }],
      ["keyvault.unlockVault", {}],
      ["settings.choose", { row: "notch", option: "show" }],
      ["settings.get", {}],
      ["settings.choose", { row: "telemetry", option: "on" }],
      ["settings.get", {}],
    ]);
  });

  it("reads settings from the core's page", async () => {
    const { win } = webkitHost(() => ok(settingsPage));
    const snap = await createWebkitAdapter(win).call("settings.get", {});
    expect(snap.values).toMatchObject({ menuBar: true, telemetry: false, defaultLocation: "cloud", launchAtLogin: null, updateChannel: null });
    expect(snap.defaultLocation).toMatchObject({ source: "env", env: "CUA_ON" });
  });

  it("reads launch at login and the update channel when the host sends them, and sets them by row", async () => {
    const { win, sent } = webkitHost(() => ok(settingsWithLogin));
    const a = createWebkitAdapter(win);
    expect((await a.call("settings.get", {})).values).toMatchObject({ launchAtLogin: true, updateChannel: "beta" });
    await a.call("settings.set", { key: "launchAtLogin", value: false });
    await a.call("settings.set", { key: "updateChannel", value: "stable" });
    expect(sent.filter((r) => r.method === "settings.choose").map((r) => r.args)).toEqual([
      { row: "launch-at-login", option: "off" },
      { row: "update-channel", option: "stable" },
    ]);
  });

  it("maps the session and the Keyvault overview", async () => {
    const { win } = webkitHost((req) =>
      req.method === "session.get"
        ? ok({ identity: "ada@example.com", signedIn: true, cloudConfigured: true, signIn: "idle" })
        : ok({
            availability: "ready",
            busy: false,
            error: null,
            overview: {
              serverVerified: true,
              namesVisible: true,
              itemsTotal: 1,
              status: { unlocked: true, osProtectorAvailable: true },
              items: [{ id: "k1", kind: "cookie", providerId: "chrome", appDisplay: "Google Chrome", domain: "github.com", key: "s", blob: "x", identityProvider: false, policy: { allowedTargets: [], ttlSecs: 0, unattended: true }, createdMs: 1, updatedMs: 2 }],
            },
          }),
    );
    const a = createWebkitAdapter(win);
    expect((await a.call("session.get", {})).fleet).toMatchObject({ identity: "ada@example.com", authMode: "user" });
    // The SwiftUI app's first run is its own window: the page never shows one there.
    expect((await a.call("session.get", {})).onboarding).toEqual({ completed: true, mode: "client" });
    const kv = await a.call("keyvault.overview", {});
    expect(kv.status).toMatchObject({ unlocked: true, os_protector_available: true });
    expect(kv.items[0]).toMatchObject({ provider_id: "chrome", app_display: "Google Chrome", policy: { unattended: true, ttl_secs: 0 }, updated_ms: 2 });
    expect(kv.items[0]).not.toHaveProperty("blob");
  });

  it("maps this Mac's own desktop (status local) to a local, running row", async () => {
    const here = { id: "local", name: "This machine", os: "macos", status: "local", detail: "", lastUsedAt: 1, provider: null, sdk: null, power: null };
    const { win } = webkitHost(() => ok({ loaded: true, selectedId: null, spaces: [{ space: here, deleting: false }] }));
    const [row] = await createWebkitAdapter(win).call("spaces.list", {});
    expect(row).toMatchObject({ provider: "local", reachable: true, powerState: "running" });
  });

  it("keeps this Mac local while it is not set up for access", async () => {
    const mac = { id: "this-mac", name: "This machine", os: "macos", status: "suspended", detail: "Set up for access", lastUsedAt: 1, provider: null, sdk: null, power: null };
    const { win } = webkitHost(() => ok({ loaded: true, selectedId: null, spaces: [{ space: mac, deleting: false }] }));
    const [row] = await createWebkitAdapter(win).call("spaces.list", {});
    expect(row).toMatchObject({ provider: "local", reachable: false, powerState: "suspended" });
  });

  it("lists this Mac and the machines that provide Spaces", async () => {
    const { win } = webkitHost((req) =>
      req.method === "spaces.list" ? ok({ loaded: true, selectedId: null, spaces: [{ space: wkSpace, deleting: false }] }) : ok({ devices: null, signedIn: false }),
    );
    const rows = await createWebkitAdapter(win).call("machines.list", {});
    expect(rows.map((r) => [r.id, r.via, Boolean(r.current)])).toEqual([
      ["this-mac", "local", true],
      ["mac-mini", "relay", false],
    ]);
  });

  it("lists machines that share their own desktop, with presence and device details", async () => {
    const desktop = { id: "relay:studio", name: "Studio", os: "linux", status: "suspended", detail: "Offline", lastUsedAt: 1, provider: "relay", sdk: null, power: null };
    const { win } = webkitHost((req) =>
      req.method === "spaces.list"
        ? ok({ loaded: true, selectedId: null, spaces: [{ space: wkSpace, deleting: false }, { space: desktop, deleting: false }] })
        : ok({
            thisMachine: { status: "local", statusText: "Local", detail: "Sharing · Relay" },
            devices: { rows: [{ id: "studio", name: "Studio", platform: "Linux", current: false, detail: "Linux · Enrolled", lastSeen: 1700000000 }] },
            signedIn: true,
          }),
    );
    const rows = mergeMachineRows(await createWebkitAdapter(win).call("machines.list", {}));
    expect(rows.map((r) => [r.id, r.online, r.detail ?? null])).toEqual([
      ["this-mac", true, "Sharing · Relay"],
      ["mac-mini", true, null],
      ["studio", false, "Offline"],
    ]);
    expect(rows[2]).toMatchObject({ os: "linux", lastSeen: 1700000000 });
  });

  it("rejects with the host's error and code; a passphrase is native_only", async () => {
    const { win } = webkitHost(() => ({ ok: false, error: { message: "nope", code: "not_found" } }));
    const a = createWebkitAdapter(win);
    await expect(a.call("spaces.delete", { spaceId: "x" })).rejects.toMatchObject({ message: "nope", code: "not_found" });
    await expect(a.call("keyvault.approve", { requestId: "r", items: null })).rejects.toMatchObject({ code: "not_found" });
    await expect(a.call("keyvault.unlock", { passphrase: "p" })).rejects.toMatchObject({ code: "native_only" });
  });

  it("routes cancel, host status, approve, deny and revoke with the contract's arguments", async () => {
    const { win, sent } = webkitHost((req) => {
      switch (req.method) {
        case "spaces.cancelCreate":
          return ok({ id: "pending:1", state: "cancelled", message: "" });
        case "host.status":
          return ok({
            configured: true, mode: "relay", relayUrl: "https://relay.cua.ai", machineId: "m1", name: "Studio", sharing: true,
            serviceInstalled: true, serviceRunning: true, serviceKind: "launchd", online: true, clients: [{ id: "a", email: null, name: "Ana", streams: 1 }],
            permissions: [{ id: "accessibility", title: "Accessibility", settingsUrl: null, instructions: "Turn on Cua Spaces", granted: false }],
            error: null, shareDesktop: true, provideSpaces: false, maxSpaces: 0,
          });
        case "keyvault.approve":
          return ok({ id: "g1", requestId: "req-1", callerFp: "fp", callerDisplay: "bot", items: ["i1"], targets: ["dev-1"], actions: [], createdMs: 1, notAfterMs: 2, usesLeft: null, revoked: false, agent: null });
        case "keyvault.revokeGrant":
          return ok(2);
        default:
          return ok(null);
      }
    });
    const a = createWebkitAdapter(win);
    expect(await a.call("spaces.cancelCreate", { pendingId: "pending:1" })).toEqual({ id: "pending:1", state: "cancelled", message: "" });
    const host = await a.call("host.status", {});
    expect(host).toMatchObject({ configured: true, mode: "relay", machineId: "m1", service: { installed: true, running: true, kind: "launchd" } });
    expect(host.permissions[0]).toMatchObject({ id: "accessibility", label: "Accessibility", granted: false });
    expect(await a.call("keyvault.approve", { requestId: "req-1", items: ["i1"] })).toMatchObject({ id: "g1", request_id: "req-1", not_after_ms: 2 });
    expect(await a.call("keyvault.deny", { requestId: "req-2" })).toBeNull();
    expect(await a.call("keyvault.revokeGrant", { id: "g1" })).toBe(2);
    expect(sent.map((r) => [r.method, r.args])).toEqual([
      ["spaces.cancelCreate", { pendingId: "pending:1" }],
      ["host.status", {}],
      ["keyvault.approve", { requestId: "req-1", items: ["i1"] }],
      ["keyvault.deny", { requestId: "req-2" }],
      ["keyvault.revokeGrant", { id: "g1" }],
    ]);
  });

  it("passes waiting Keyvault requests through, tagged as the broker tags them", () => {
    const p = toPending({
      id: "req-1",
      caller: { pid: 1, uid: 501, path: null, signing: { type: "adHoc", identifier: "bot", cdhash: "ab" }, firstParty: false, osVerified: true, launchedBy: null, verifiedName: null },
      callerFp: "fp",
      callerDisplay: "bot",
      request: { selectors: [{ type: "site", app: "chrome", site: "github.com" }, { type: "app", app: "slack" }], targets: ["dev-1"], actions: [], durationSecs: 600, uses: null, reason: "deploy", claimedName: null, agent: null },
      items: [],
      needsImport: [{ type: "login", site: "x.com" }],
      createdMs: 5,
    });
    expect(p.caller.signing).toEqual({ kind: "ad_hoc", identifier: "bot", cdhash: "ab" });
    expect(p.caller.os_verified).toBe(true);
    expect(p.request.selectors).toEqual([{ kind: "site", app: "chrome", site: "github.com" }, { kind: "app", app: "slack" }]);
    expect(p.request.duration_secs).toBe(600);
    expect(p.needs_import).toEqual([{ kind: "login", site: "x.com" }]);
    expect(p.caller_display).toBe("bot");
    expect(toPending({ caller: { signing: "unsigned" }, request: {} }).caller.signing).toEqual({ kind: "unsigned" });
  });

  it("routes the agents.* methods with the contract's arguments and records", async () => {
    const agent = { name: "ada", harness: "claude-code", space: "local:dev", paused: false, spaceState: "running", runId: "r1", savedMs: 0, lastError: null };
    const run = { runId: "r1", agent: "claude-code", status: "running", reason: "", summary: "Fix the tests", createdAt: 1, phase: "thinking", turn: 2 };
    const page = { run_id: "r1", status: "running", phase: "thinking", events: [], cursor: 4, caught_up: true };
    const row = { agent: "codex", name: "Codex", installed: true, configured: true, detail: "", skillsInstalled: 3, skillsTotal: 3, mcpConfig: null, skillsDir: null };
    const { win, sent } = webkitHost((req) => {
      switch (req.method) {
        case "agents.list":
          return ok([agent]);
        case "agents.runs":
          return ok([run]);
        case "agents.events":
          return ok(page);
        case "agents.setup":
        case "agents.configure":
          return ok([row]);
        default:
          return ok(null);
      }
    });
    const a = createWebkitAdapter(win);
    expect(await a.call("agents.list", {})).toEqual([agent]);
    expect(await a.call("agents.runs", { spaceId: "local:dev" })).toEqual([run]);
    expect(await a.call("agents.events", { spaceId: "local:dev", runId: "r1", cursor: 0 })).toEqual(page);
    expect(await a.call("agents.pause", { name: "ada" })).toBeNull();
    expect(await a.call("agents.resume", { name: "ada" })).toBeNull();
    expect(await a.call("agents.setup", {})).toEqual([row]);
    expect(await a.call("agents.configure", { agents: null })).toEqual([row]);
    expect(sent.map((r) => [r.method, r.args])).toEqual([
      ["agents.list", {}],
      ["agents.runs", { spaceId: "local:dev" }],
      ["agents.events", { spaceId: "local:dev", runId: "r1", cursor: 0 }],
      ["agents.pause", { name: "ada" }],
      ["agents.resume", { name: "ada" }],
      ["agents.setup", {}],
      ["agents.configure", { agents: null }],
    ]);
  });

  it("reads the host's unsupported answer (no daemon) as unsupported", async () => {
    const { win } = webkitHost(() => ({ ok: false, error: { code: "unsupported", message: "Agents need the cua daemon" } }));
    const err = await createWebkitAdapter(win).call("agents.list", {}).catch((e: unknown) => e);
    expect(isUnsupported(err)).toBe(true);
  });

  it("puts this Mac's host status on its machine row", async () => {
    const host = {
      configured: true, mode: "relay", relayUrl: null, directUrl: null, machineId: "m1", name: "Studio", sharing: true,
      serviceInstalled: true, serviceRunning: true, serviceKind: "launchd", online: true, clients: [], permissions: [],
      error: null, shareDesktop: true, provideSpaces: false, maxSpaces: 0,
    };
    const { win } = webkitHost((req) => (req.method === "spaces.list" ? ok({ loaded: true, selectedId: null, spaces: [] }) : ok({ devices: null, signedIn: false, host })));
    const [self] = await createWebkitAdapter(win).call("machines.list", {});
    expect(self!.host).toMatchObject({ configured: true, mode: "relay", machineId: "m1", service: { installed: true, running: true, kind: "launchd" } });
  });

  it("passes the core's whole host state through, the logs, the owner and the progress", async () => {
    const recentAccess = [{ atMs: 5, via: "relay", who: "bob@example.com", what: "desktop" }];
    const providedSpaces = [{ relayMachine: "m2", localSpace: "s1", name: "QA", image: "ubuntu", os: "linux", kind: "container", createdBy: "bob", createdAtMs: 4 }];
    const spacesAudit = [{ atMs: 3, action: "create", who: "bob", space: "QA", detail: "" }];
    const wk = {
      configured: true, mode: "relay", relayUrl: "https://relay.cua.ai", directUrl: null, machineId: "m1", name: "Studio", sharing: false,
      serviceInstalled: false, serviceRunning: false, serviceKind: "launchd", online: false, clients: [], permissions: [],
      error: null, shareDesktop: true, provideSpaces: true, maxSpaces: 4, maxMacosVms: 2, recentAccess, accessLogError: "edited",
      providedSpaces, spacesAudit, spacesAuditError: null, pausedSignedOut: true, owner: "user-1", ownerEmail: "ada@example.com",
      account: null, progress: "Sign in to Cua in your browser to continue. Setup finishes on its own after that.",
    };
    const { win } = webkitHost((req) => (req.method === "spaces.list" ? ok({ loaded: true, selectedId: null, spaces: [] }) : req.method === "host.status" ? ok(wk) : ok({ devices: null, signedIn: false, host: wk })));
    const a = createWebkitAdapter(win);
    const status = await a.call("host.status", {});
    expect(status).toMatchObject({
      maxMacosVms: 2, recentAccess, accessLogError: "edited", providedSpaces, spacesAudit, spacesAuditError: null,
      pausedSignedOut: true, owner: "user-1", ownerEmail: "ada@example.com", account: null, progress: wk.progress,
    });
    // The core's state for `host.panel` keeps them all (the progress is the page's own).
    const state = hostState(status);
    expect(state).toMatchObject({ maxMacosVms: 2, recentAccess, providedSpaces, spacesAudit, pausedSignedOut: true, owner: "user-1", ownerEmail: "ada@example.com", account: null });
    expect(state).not.toHaveProperty("progress");
    // The machine row carries it to the page, and the progress beside the panel.
    const [self] = await a.call("machines.list", {});
    expect(self!.host?.pausedSignedOut).toBe(true);
    expect(toMachines([self!], [], undefined)[0]!.hostProgress).toBe(wk.progress);
  });

  it("keeps the host's words for a failed setup and waits as long as the sign-in takes", async () => {
    vi.useFakeTimers();
    try {
      const presented = { code: "failed", message: "Your Cua account isn’t signed in on this Mac. Sign in, then try again.", title: "Sign in to Cua", details: "Not signed in to Cua: the sign-in did not finish.", actionLabel: "Sign In" };
      let answer!: (a: Answer) => void;
      const { win, sent } = webkitHost((req) => (req.method === "host.setUp" ? new Promise<Answer>((r) => (answer = r)) : ok(null)));
      const call = createWebkitAdapter(win, { timeoutMs: 1_000 }).call("host.setUp", { request: { mode: "relay" } });
      const settled = call.catch((e: unknown) => e);
      // Minutes in the browser: no timeout.
      await vi.advanceTimersByTimeAsync(60_000);
      expect(sent.map((r) => r.method)).toEqual(["host.setUp"]);
      answer({ ok: false, error: presented });
      const e = (await settled) as HostError;
      expect(e).toBeInstanceOf(HostError);
      expect(e.message).toBe(presented.message);
      expect(e.presented).toEqual({ title: presented.title, details: presented.details, actionLabel: "Sign In" });
      expect(hostFailure(e)).toEqual({ title: "Sign in to Cua", message: presented.message, details: presented.details, actionLabel: "Sign In" });
      // A plain error stays plain.
      expect(hostFailure(new HostError("nope", "failed"))).toEqual({ message: "nope" });
    } finally {
      vi.useRealTimers();
    }
  });

  it("reads the host's own settings rows and picks their options with settings.choose", async () => {
    const choice = (id: string, active: string) => ({ id, kind: "choice", label: "", enabled: true, options: ["auto", "builtin", "system"].map((o) => ({ id: o, label: o, active: o === active })) });
    const toggle = (id: string, on: boolean) => ({ id, kind: "toggle", label: "", enabled: true, options: [{ id: "on", label: "On", active: on }, { id: "off", label: "Off", active: !on }] });
    const page = {
      page: {
        title: "Settings",
        sections: [
          { id: "general", rows: [toggle("auto-connect", false)] },
          { id: "runtimes", rows: [choice("macos-runtime", "builtin"), choice("linux-runtime", "system")] },
          { id: "keyvault", rows: [toggle("keyvault-auto-wipe", true)] },
        ],
      },
    };
    const { win, sent } = webkitHost(() => ok(page));
    const a = createWebkitAdapter(win);
    expect((await a.call("settings.get", {})).hostSettings).toEqual({ lumeSource: "builtin", linuxSource: "system", autoConnect: false, keyvaultAutoWipe: true });
    await a.call("settings.choose", { row: "macos-runtime", option: "system" });
    expect(sent.slice(-2).map((r) => [r.method, r.args])).toEqual([
      ["settings.choose", { row: "macos-runtime", option: "system" }],
      ["settings.get", {}],
    ]);
    // A host without these rows has none of them.
    const { win: bare } = webkitHost(() => ok(settingsPage));
    expect((await createWebkitAdapter(bare).call("settings.get", {})).hostSettings).toEqual({ lumeSource: null, linuxSource: null, autoConnect: null, keyvaultAutoWipe: null });
  });

  it("maps Teleport's records onto the core's words, and keeps the host's keys for the plan and run", async () => {
    const entry = { id: "com.apple.Safari", name: "Safari", capability: "installOnly", moves: ["appOnly", "appWithState"], sensitiveGroups: ["signIns"], providerId: "safari", lastUsedMs: 5 };
    const { win, sent, emit } = webkitHost((req) => {
      switch (req.method) {
        case "teleport.catalog":
          return ok([entry]);
        case "teleport.plan":
          return ok({ app: entry, spaceId: "s1", moves: "appWithState", steps: [{ kind: "install", summary: "Install" }], consent: [{ kind: "secret", key: "k", label: "L", detail: "", bytes: 2, sensitive: true }], sensitive: true, totalBytes: 2, warnings: [], relayUnsealed: false, json: "plan-1" });
        case "teleport.run":
          return ok({ appId: "com.apple.Safari", installed: [], sent: [], imported: [], skipped: [], launched: true });
        case "teleport.sites":
          return ok({ providerId: "safari", appDisplay: "Safari", domains: [{ domain: "x.com", sessionCookies: 1 }], notes: [] });
        default:
          return ok(null);
      }
    });
    const a = createWebkitAdapter(win);
    const [e] = await a.call("teleport.catalog", { spaceId: "s1" });
    expect(e).toMatchObject({ capability: "install_only", moves: ["app_only", "app_with_state"], sensitiveGroups: ["sign_ins"], json: "com.apple.Safari", hostPath: null });
    const plan = await a.call("teleport.plan", { spaceId: "s1", entry: e!, move: "app_with_state", files: [], sensitiveGroups: ["sign_ins"] });
    expect(plan).toMatchObject({ moves: "app_with_state", json: "plan-1", consent: [{ kind: "secret", sensitive: true }] });
    const got: HostEvent[] = [];
    a.subscribe((ev) => got.push(ev));
    const run = a.call("teleport.run", { spaceId: "s1", plan, consent: { approved: true, acknowledgeSensitive: true }, runId: "r1" });
    emit("teleport.progress", { runId: "r1", event: { step: 1, steps: 2, kind: "install", phase: "started", detail: "", doneBytes: 0, totalBytes: 2 } });
    expect((await run).launched).toBe(true);
    expect(got).toEqual([{ type: "teleport.progress", runId: "r1", event: { step: 1, steps: 2, kind: "install", phase: "started", detail: "", doneBytes: 0, totalBytes: 2 } }]);
    expect(await a.call("teleport.sites", { providerId: "safari" })).toMatchObject({ provider_id: "safari", app_display: "Safari", domains: [{ domain: "x.com", session_cookies: 1 }] });
    expect(sent.map((r) => [r.method, r.args])).toEqual([
      ["teleport.catalog", { spaceId: "s1" }],
      ["teleport.plan", { spaceId: "s1", entry: { id: "com.apple.Safari" }, move: "app_with_state", files: [], sensitiveGroups: ["sign_ins"] }],
      ["teleport.run", { spaceId: "s1", plan: { json: "plan-1" }, consent: { approved: true, acknowledgeSensitive: true }, runId: "r1" }],
      ["teleport.sites", { providerId: "safari" }],
    ]);
  });

  it("sends only clean usage events, and opens only System Settings panes", async () => {
    const { win, sent } = webkitHost(() => ok(null));
    const a = createWebkitAdapter(win);
    await a.call("telemetry.track", { signals: [{ type: "feature", feature: "space_open" }, { type: "feature", feature: "/Users/ada" } as never] });
    await a.call("telemetry.track", { signals: [{ type: "feature", feature: "Ada Lovelace" } as never] });
    expect(sent.map((r) => [r.method, r.args])).toEqual([["telemetry.track", { signals: [{ type: "feature", feature: "space_open" }] }]]);
    await expect(a.call("host.openSettings", { url: "https://example.com" })).rejects.toMatchObject({ code: "bad_args" });
    expect(sent).toHaveLength(1);
  });

  it("runs New Space on the app's env and create, with its progress", async () => {
    vi.useFakeTimers();
    try {
      const env = { defaultLocation: "local", cloudAvailable: true, localAvailable: true, maxCpus: 8, hosts: [{ id: "m1", name: "gamma-4 Mac Studio", via: "relay", online: true, os: "macos", limits: [] }] };
      const created = { ...wkSpace, id: "relay:m1/sandbox-1", name: "Sandbox 1", status: "running" };
      let finish: (a: Answer) => void = () => {};
      const { win, sent, emit } = webkitHost((req) => {
        switch (req.method) {
          case "spaces.createOptions":
            return ok({ local: null, gpus: null, cloudPricing: null, experiments: {}, maxCpus: 8, env });
          case "spaces.create":
            return new Promise<Answer>((r) => (finish = r));
          case "spaces.list":
            return ok({ loaded: true, selectedId: null, spaces: [{ space: { ...wkSpace, id: "pending:1" }, deleting: false }, { space: wkSpace, deleting: false }] });
          default:
            return ok(null);
        }
      });
      const a = createWebkitAdapter(win);
      const got: HostEvent[] = [];
      a.subscribe((e) => got.push(e));
      expect((await a.call("spaces.createOptions", {})).env).toEqual(env);
      const config = { image: "macos-tahoe", on: "host:m1", kind: "vm" as const, runtime: "lume" as const, spacesd: true };
      const create = a.call("spaces.create", { config, pendingId: "pending:1", os: "macos" });
      // A create answers when the Space is ready: no timeout.
      await vi.advanceTimersByTimeAsync(60 * 60_000);
      emit("spaces.createProgress", { pendingId: "pending:1", phase: "pulling", fraction: 0.5, detail: "" });
      // The app's own pending row for it is the page's already.
      expect((await a.call("spaces.list", {})).map((r) => r.id)).toEqual([wkSpace.id]);
      finish(ok(created));
      expect((await create).id).toBe("relay:m1/sandbox-1");
      expect(sent.find((r) => r.method === "spaces.create")?.args).toEqual({ config, pendingId: "pending:1", os: "macos" });
      expect(got).toEqual([{ type: "spaces.createProgress", progress: { pendingId: "pending:1", phase: "pulling", fraction: 0.5, detail: "" } }]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("opens the page's New Space when the app asks", () => {
    resetNewSpaceSession();
    const { win, emit } = webkitHost(() => ok(null));
    createWebkitAdapter(win);
    emit("spaces.newRequested", { on: "host:m1" });
    expect(readNewSpaceSession().requested).toEqual({ on: "host:m1" });
    emit("spaces.newRequested", { on: null });
    expect(readNewSpaceSession().requested).toEqual({ on: null });
    resetNewSpaceSession();
  });

  it("lists a relay machine and its enrolled device once, by the hostname it reported", async () => {
    const desktop = { id: "relay:96fe", name: "gamma-4 Mac Studio", os: "macos", status: "running", detail: "", lastUsedAt: 1, provider: "relay", sdk: null, power: null };
    const { win } = webkitHost((req) =>
      req.method === "spaces.list"
        ? ok({ loaded: true, selectedId: null, spaces: [{ space: desktop, deleting: false }] })
        : ok({
            devices: {
              rows: [
                { id: "dev_gamma", name: "gamma-4.example.com", platform: "macOS", current: false, detail: "macOS · Enrolled", lastSeen: 1791044974 },
                { id: "dev_laptop", name: "ada-laptop.local", platform: "macOS", current: false, lastSeen: 1791000000 },
              ],
            },
            signedIn: true,
            hostnames: { "96fe": "Gamma-4.example.com." },
            presence: { "96fe": true },
            deviceStates: { dev_gamma: "enrolled", dev_laptop: "enrolled" },
          }),
    );
    const raw = await createWebkitAdapter(win).call("machines.list", {});
    // The adapter reports what the host said; the store lists each computer once.
    expect(raw.map((r) => [r.id, Boolean(r.device), r.hostname ?? null, r.presence ?? null])).toEqual([
      ["this-mac", false, null, null],
      ["96fe", false, "Gamma-4.example.com.", true],
      ["dev_gamma", true, null, null],
      ["dev_laptop", true, null, null],
    ]);
    const rows = mergeMachineRows(raw);
    expect(rows.map((r) => [r.id, r.name, r.os, Boolean(r.device)])).toEqual([
      ["this-mac", "This Mac", "macos", false],
      ["96fe", "gamma-4 Mac Studio", "macos", false],
      ["dev_laptop", "ada-laptop.local", "macos", true],
    ]);
    expect(rows[1]).toMatchObject({ lastSeen: 1791044974, detail: "macOS · Enrolled" });
  });

  it("lists a machine that refused the list's connect because its owner stopped sharing it as online and not sharing", async () => {
    const refused = (name: string, error: string) => ({
      id: `relay:${name}`,
      name,
      os: "macos",
      status: "suspended",
      detail: `Unreachable · ${error}`,
      lastUsedAt: 1,
      provider: "relay",
      sdk: { features: [], spacesdVersion: "0.4.1", reachable: false, error },
      power: null,
    });
    const { win } = webkitHost((req) =>
      req.method === "spaces.list"
        ? ok({
            loaded: true,
            selectedId: null,
            spaces: [
              // The SDK's own line, and the raw refusal an older SDK passes.
              { space: refused("gamma-4", "gamma-4 stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)"), deleting: false },
              { space: refused("studio", "cua-spacesd is not available: relay:studio has no cua-spacesd (gRPC: relay assertion refused: this machine stopped sharing)"), deleting: false },
              // Any other failure to connect is not a refusal: offline.
              { space: refused("lab", "timed out"), deleting: false },
            ],
          })
        : ok({ devices: null, signedIn: false }),
    );
    const rows = await createWebkitAdapter(win).call("machines.list", {});
    expect(rows.map((r) => [r.id, r.online, r.limits.map((l) => l.resource)])).toEqual([
      ["this-mac", true, []],
      ["gamma-4", true, ["sharing"]],
      ["studio", true, ["sharing"]],
      ["lab", false, []],
    ]);
    expect(rows[1]!.limits[0]!.reason).toBe("gamma-4 stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)");
  });

  it("rejects when the reply promise rejects, and times out", async () => {
    const failing = webkitHost(() => Promise.reject(new Error("handler threw")));
    await expect(createWebkitAdapter(failing.win).call("spaces.list", {})).rejects.toThrow("handler threw");
    const silent = webkitHost(() => undefined);
    await expect(createWebkitAdapter(silent.win, { timeoutMs: 10 }).call("spaces.list", {})).rejects.toMatchObject({ code: "timeout" });
  });

  it("turns cua:event window events into host events", () => {
    const { win, emit } = webkitHost(() => undefined);
    const a = createWebkitAdapter(win);
    const got: HostEvent[] = [];
    a.subscribe((e) => got.push(e));
    emit("spaces.changed");
    emit("session.changed");
    emit("agents.changed");
    emit("machines.changed");
    emit("something.else");
    expect(got).toEqual([
      { type: "spaces.changed" },
      { type: "machines.changed" },
      { type: "session.changed" },
      { type: "agents.changed" },
      { type: "machines.changed" },
    ]);
    a.dispose?.();
    emit("keyvault.changed");
    expect(got).toHaveLength(5);
  });

  it("rejects pending calls on dispose", async () => {
    const { win } = webkitHost(() => undefined);
    const a = createWebkitAdapter(win);
    const p = a.call("spaces.list", {});
    a.dispose?.();
    await expect(p).rejects.toMatchObject({ code: "closed" });
  });
});

/* ---- Keyvault management (ops/keyvault-manage.ts) --------------------------------- */

describe("keyvault management", () => {
  const wkKeyvault = {
    availability: "ready",
    overview: { items: [], pending: [], grants: [], deliveries: [], serverVerified: true },
    dismissed: ["imp-1"],
    busy: false,
    error: null,
  };

  it("maps onto the SwiftUI host's methods, and reads the copies hidden from the notch", async () => {
    const { win, sent } = webkitHost((req) => ok(req.method.startsWith("keyvault.") ? wkKeyvault : null));
    const a = createWebkitAdapter(win);
    expect(await a.call("keyvault.overview", {})).toMatchObject({ availability: "ready", dismissed: ["imp-1"] });
    expect(await a.call("keyvault.showItems", {})).toMatchObject({ availability: "ready", dismissed: ["imp-1"] });
    expect(await a.call("keyvault.delete", { itemIds: ["k1", "k2"] })).toBeNull();
    expect(await a.call("keyvault.run", { command: { type: "release", target: "local:aurora" } })).toBeNull();
    expect(await a.call("keyvault.dismiss", { imports: ["imp-2"] })).toBeNull();
    expect(sent.map((r) => [r.method, r.args])).toEqual([
      ["keyvault.get", {}],
      ["keyvault.showItems", {}],
      ["keyvault.delete", { ids: ["k1", "k2"] }],
      ["keyvault.run", { command: { type: "release", target: "local:aurora" } }],
      ["keyvault.dismiss", { imports: ["imp-2"] }],
    ]);
  });

  it("a declined delete rejects as cancelled", async () => {
    const { win } = webkitHost((req) => (req.method === "keyvault.delete" ? { ok: false, error: { code: "cancelled", message: "Delete cancelled" } } : ok(null)));
    await expect(createWebkitAdapter(win).call("keyvault.delete", { itemIds: ["k1"] })).rejects.toMatchObject({ code: "cancelled" });
  });

  it("asks the SwiftUI host for the sites sent last time, and takes none as a fresh start", async () => {
    const answers: unknown[] = [["github.com", "linear.app"], null];
    const { win, sent } = webkitHost((req) => (req.method === "teleport.remembered" ? ok(answers.shift()) : ok(null)));
    const a = createWebkitAdapter(win);
    expect(await a.call("teleport.remembered", { providerId: "chrome", spaceId: "s1" })).toEqual(["github.com", "linear.app"]);
    expect(await a.call("teleport.remembered", { providerId: "chrome", spaceId: "s2" })).toBeNull();
    expect(sent.map((r) => r.args)).toEqual([{ providerId: "chrome", spaceId: "s1" }, { providerId: "chrome", spaceId: "s2" }]);
  });

  it("Tauri has no commands for them", async () => {
    const { win, calls } = tauriHost();
    const a = createTauriAdapter(win);
    for (const [op, args] of [
      ["keyvault.showItems", {}],
      ["keyvault.delete", { itemIds: ["k1"] }],
      ["keyvault.run", { command: { type: "release", target: "t" } }],
      ["keyvault.dismiss", { imports: ["i"] }],
      ["teleport.remembered", { providerId: "chrome", spaceId: "s1" }],
    ] as const) {
      await expect(a.call(op, args as never)).rejects.toBeInstanceOf(UnsupportedOperationError);
    }
    expect(calls).toEqual([]);
  });
});

/* ---- Keyvault setup (ops/keyvault-setup.ts) ------------------------------------- */

describe("keyvault.setup", () => {
  it("runs the SwiftUI host's Touch ID setup and answers the recovery key; a passphrase stays native", async () => {
    const { win, sent } = webkitHost((req) => (req.method === "keyvault.setup" ? ok({ recoveryKey: "ABCD-EFGH" }) : ok(null)));
    const a = createWebkitAdapter(win);
    expect(await a.call("keyvault.setup", {})).toEqual({ recoveryKey: "ABCD-EFGH" });
    expect(sent.map((r) => [r.method, r.args])).toEqual([["keyvault.setup", {}]]);
    await expect(a.call("keyvault.setup", { passphrase: "a long passphrase" })).rejects.toMatchObject({ code: "native_only" });
    expect(sent).toHaveLength(1);
  });

  it("maps onto Tauri's keyvault_setup commands", async () => {
    const { win, calls } = tauriHost({ keyvault_setup: "KEY-1", keyvault_setup_passphrase: null });
    const a = createTauriAdapter(win);
    expect(await a.call("keyvault.setup", {})).toEqual({ recoveryKey: "KEY-1" });
    expect(await a.call("keyvault.setup", { passphrase: "a long passphrase" })).toEqual({ recoveryKey: null });
    expect(calls).toEqual([
      ["keyvault_setup", undefined],
      ["keyvault_setup_passphrase", { passphrase: "a long passphrase" }],
    ]);
  });
});
