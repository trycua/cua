// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The bridge's view models through the real app core (wasm), and the
 * TypeScript fallbacks against them on the demo data.
 */

import { describe, expect, it } from "vitest";
import { DEMO_MACHINES, demoMachineRows, demoKeyvaultItems, demoKeyvaultOverview, demoSpaceRows } from "../adapters/demo-data";
import {
  NO_CREATES,
  composeCreates,
  keyvaultViews,
  reduceCreates,
  rowsToSpaces,
  hostPanel,
  hostSetupGuide,
  settingsPage,
  toMachines,
  validateImageRef,
} from "../derive";
import { DEFAULT_SETTINGS } from "../protocol";
import { toHostStatus, toKeyvaultOverview } from "../adapters/webkit";
import { replayFlow } from "../parity";
import { wizardEnv } from "../new-space";
import { noCore, testCore, wasmBuilt } from "./testCore";

const NOW = Date.parse("2026-10-03T12:00:00Z");

describe("without the core", () => {
  it("reports unavailable and throws on call", () => {
    expect(noCore.status).toBe("unavailable");
    expect(noCore.tryCall("spaces.rowsToSpaces")).toBeUndefined();
    expect(() => noCore.call("spaces.rowsToSpaces")).toThrow(/not loaded/);
  });

  it("still maps the demo data, names as typed", () => {
    const spaces = rowsToSpaces(noCore, demoSpaceRows(NOW), NOW);
    expect(spaces.map((s) => [s.name, s.status])).toEqual([
      ["design-review", "running"],
      ["agent-sandbox", "suspended"],
      ["release-checks", "suspended"],
      ["qa-windows", "running"],
      ["ubuntu-build", "running"],
      ["windows-legacy", "suspended"],
    ]);
    expect(settingsPage(noCore, { values: DEFAULT_SETTINGS, defaultLocation: null, telemetry: null }, undefined, undefined)).toBeNull();
  });

  it("keeps an OS that was not reported unknown, never Linux", () => {
    const [space] = rowsToSpaces(noCore, [{ id: "direct:10.0.0.5:3211", name: "10.0.0.5:3211", provider: "direct", spacesdVersion: "", features: [], reachable: true }], NOW);
    expect(space).toMatchObject({ os: "unknown", scene: "blank" });
  });
});

describe.skipIf(!wasmBuilt)("an unknown OS through the core", () => {
  it("maps the same as the fallback", async () => {
    const core = await testCore();
    const row = { id: "relay:test-host", name: "Test host", provider: "relay" as const, spacesdVersion: "0.4.1", features: [], reachable: true };
    const [space] = rowsToSpaces(core, [row], NOW);
    expect(space).toMatchObject({ os: "unknown", scene: "blank" });
    expect(rowsToSpaces(core, [{ ...row, os: "unknown" }], NOW)[0]!.os).toBe("unknown");
  });
});

describe.skipIf(!wasmBuilt)("with the wasm core", () => {
  it("loads and lists its methods", async () => {
    const core = await testCore();
    expect(core.status).toBe("ready");
    expect(core.methods).toEqual(expect.arrayContaining(["spaces.rowsToSpaces", "creates.reduce", "settings.page", "keyvault.page"]));
  });

  it("maps the demo rows like the fallback does", async () => {
    const core = await testCore();
    const rows = demoSpaceRows(NOW);
    const fromCore = rowsToSpaces(core, rows, NOW);
    const fallback = rowsToSpaces(noCore, rows, NOW);
    expect(fromCore.map((s) => [s.id, s.os, s.status, s.host])).toEqual(fallback.map((s) => [s.id, s.os, s.status, s.host]));
    expect(fromCore.find((s) => s.id === "relay:mac-mini/qa-windows")?.hostName).toBe("Mac mini");
  });

  it("runs the create state machine", async () => {
    const core = await testCore();
    const registry = rowsToSpaces(core, demoSpaceRows(NOW), NOW);
    let state = reduceCreates(core, NO_CREATES, {
      type: "start",
      id: "pending:x",
      name: "scratch",
      os: "linux",
      provider: "local",
      now: NOW,
      image: "ubuntu-xfce",
    });
    state = reduceCreates(core, state, { type: "progress", id: "pending:x", phase: "pulling", fraction: 0.5, now: NOW + 1000 });
    const shown = composeCreates(core, registry, state);
    const pending = shown.find((s) => s.id === "pending:x");
    expect(pending?.status).toBe("provisioning");
    expect(pending?.progress?.phase).toBe("pulling");
    expect(pending?.progress?.permille).toBeGreaterThan(0);

    state = reduceCreates(core, state, { type: "power-start", id: "local:design-review", on: false, now: NOW });
    const powering = composeCreates(core, registry, state).find((s) => s.id === "local:design-review");
    expect(powering?.power?.turningOn).toBe(false);
  });

  it("checks image references", async () => {
    const core = await testCore();
    expect(validateImageRef(core, "ubuntu-xfce")).toBeNull();
    expect(validateImageRef(core, "not an image!")).toMatch(/image reference/i);
  });

  it("draws This machine from the SwiftUI host's whole state: the paused notice and the log preview", async () => {
    const core = await testCore();
    const base = {
      configured: true, mode: "relay", relayUrl: "https://relay.cua.ai", directUrl: null, machineId: "m1", name: "Studio",
      serviceKind: "launchd", clients: [], permissions: [], error: null, shareDesktop: true, provideSpaces: false, maxSpaces: 0,
      owner: "user-1", ownerEmail: "ada@example.com",
    };
    // Signed out: one line and Sign In, in place of the summary.
    const paused = hostPanel(core, toHostStatus({ ...base, sharing: false, serviceInstalled: false, serviceRunning: false, online: false, pausedSignedOut: true, account: null }));
    expect(paused.summary).toBe("Paused · signed out");
    expect(paused.notice).toMatch(/^Sign in to share this Mac through Cua/);
    expect(paused.noticeAction).toMatchObject({ id: "sign-in", label: "Sign In" });
    // Signed in as the owner: whose it is.
    const recentAccess = Array.from({ length: 7 }, (_, i) => ({ atMs: NOW - i * 60_000, via: "relay", who: `user${i}@example.com`, what: "connect" }));
    const mine = hostPanel(
      core,
      toHostStatus({ ...base, sharing: true, serviceInstalled: true, serviceRunning: true, online: true, account: { id: "user-1", email: "ada@example.com", display: "Ada" }, recentAccess }),
    );
    expect(mine.notice ?? null).toBeNull();
    expect(mine.facts.find((f) => f.label === "Shared with")?.value).toBe("Your account (Ada)");
    // The newest five, then Show All… for the rest.
    expect(mine.recent).toHaveLength(5);
    expect(mine.recentMore).toBe("Show All…");
    expect(mine.recentAll!.length).toBeGreaterThan(5);
  });

  it("lays out the Settings page", async () => {
    const core = await testCore();
    const page = settingsPage(
      core,
      { values: DEFAULT_SETTINGS, defaultLocation: null, telemetry: null },
      {
        fleet: { configured: true, authMode: "user", baseUrl: "", tokenUrl: "", identity: "ada@example.com" },
        onboarding: { completed: true },
        daemon: null,
      },
      { kind: "idle" },
    );
    expect(page?.title).toBe("Settings");
    const account = page?.sections.find((s) => s.id === "account");
    expect(account?.rows[0]).toMatchObject({ id: "account", label: "ada@example.com", button: "Sign out" });
  });

  it("builds the Keyvault views from the demo overview", async () => {
    const core = await testCore();
    const overview = demoKeyvaultOverview(NOW, demoKeyvaultItems(NOW), false);
    const views = keyvaultViews(core, overview, NOW);
    expect(views?.page.ready).toBe(true);
    expect(views?.page.pendingCount).toBe(1);
    expect(views?.sidebar.apps.map((a) => a.title)).toEqual(expect.arrayContaining(["Google Chrome", "Safari", "Slack"]));
    const vault = views!.vault;
    expect(vault.total).toBe(10);
    expect(new Set(vault.apps.map((a) => a.name))).toEqual(new Set(["Google Chrome", "Safari", "Slack", "Notion", "Linear"]));
    const chrome = vault.apps.find((a) => a.providerId === "chrome")!;
    expect(chrome.sites.map((s) => s.site)).toEqual(expect.arrayContaining(["github.com", "linear.app", "google.com"]));
    expect(chrome.lock).toBe("mixed");
    // Identity providers are never offered for unlocking.
    expect(chrome.unlockIds).not.toContain("kv-google");
    expect(keyvaultViews(core, demoKeyvaultOverview(NOW, [], true), NOW)?.page.canUnlock).toBe(true);
  });

  it("reads the SwiftUI host's camelCase overview once the WebKit adapter maps it", async () => {
    const core = await testCore();
    const camel = (v: unknown): unknown =>
      Array.isArray(v)
        ? v.map(camel)
        : v && typeof v === "object"
          ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()), camel(x)]))
          : v;
    // What BridgeValue.encode makes of KeyvaultOverview (Swift field names).
    const swift = camel(demoKeyvaultOverview(NOW, demoKeyvaultItems(NOW), false)) as Record<string, unknown>;
    const overview = toKeyvaultOverview({ availability: "ready", overview: swift, busy: false, error: null });
    const views = keyvaultViews(core, overview, NOW);
    expect(views?.page.ready).toBe(true);
    expect(views?.vault.total).toBe(10);
    expect(views?.vault.apps.find((a) => a.providerId === "chrome")?.lock).toBe("mixed");
  });

  it("replays every parity golden through the loaded core", async () => {
    const core = await testCore();
    const flows = core.parity!.flows();
    expect(flows.length).toBeGreaterThanOrEqual(30);
    for (const f of flows) {
      expect(core.parity!.run(f.name, f.flow, (m, a) => core.call(m, a)), f.name).toEqual(JSON.parse(f.golden));
    }
  });

  it("replays the Spaces and Keyvault flows through the bridge", async () => {
    const core = await testCore();
    for (const name of ["provisioning", "delete-space", "space-power", "keyvault-approve-deny", "keyvault-unlock"]) {
      const replay = replayFlow(core, name);
      expect(replay.bridged.length, name).toBeGreaterThan(0);
      expect(replay.checkpoints.length, name).toBeGreaterThan(0);
      expect(replay.transcript, name).toEqual(replay.golden);
    }
  });

  it("puts Spaces on their machines", async () => {
    const core = await testCore();
    const machines = toMachines(DEMO_MACHINES, rowsToSpaces(core, demoSpaceRows(NOW), NOW));
    expect(machines.map((m) => [m.name, m.spaceIds.length])).toEqual([
      ["This Mac", 2],
      ["Mac mini", 2],
      ["Linux box", 2],
      ["Studio PC", 0],
    ]);
  });

  it("lists a relay machine and the device on it once, online when the relay sees it, never a Run on choice when only a device", async () => {
    const core = await testCore();
    // The app's probe of gamma-4 timed out (no hostname, not
    // reached), the relay sees it connected, and its device is named after
    // its hostname.
    const rows = [
      { id: "this-mac", name: "This Mac", via: "local", online: true, os: "macos", current: true, limits: [] },
      { id: "96fedb7e", name: "gamma-4 Mac Studio", via: "relay", online: false, presence: true, os: "macos", limits: [] },
      { id: "dev_2278", name: "gamma-4.example.com", via: "relay", online: false, os: "macos", limits: [], device: true, lastSeen: 1791044974, detail: "macOS · Enrolled" },
      { id: "dev_lap", name: "Laptop", via: "relay", online: false, os: "windows", limits: [], device: true, lastSeen: 1791000000 },
    ];
    const machines = toMachines(rows, [], core, 1791045000);
    expect(machines.map((m) => [m.id, m.name, m.online, Boolean(m.device)])).toEqual([
      ["this-mac", "This Mac", true, false],
      ["96fedb7e", "gamma-4 Mac Studio", true, false],
      ["dev_lap", "Laptop", false, true],
    ]);
    expect(machines[1]).toMatchObject({ detail: "macOS · Enrolled", lastSeen: 1791044974 });
    const env = wizardEnv(core, { options: null, clouds: null, defaultLocation: "local", cloudAvailable: false, machines });
    expect(env.hosts?.map((h) => [h.id, h.online])).toEqual([["96fedb7e", true]]);
  });

  it("counts a machine's own desktop as on it", () => {
    const rows = [
      { id: "this-mac", name: "This Mac", via: "local", online: true, os: "macos", current: true, limits: [] },
      { id: "96fe", name: "gamma-4 Mac Studio", via: "relay", online: true, os: "macos", limits: [] },
    ];
    const desktop = { id: "relay:96fe", provider: "relay" } as never;
    const elsewhere = { id: "relay:gone", provider: "relay" } as never;
    expect(toMachines(rows, [desktop, elsewhere]).map((m) => m.spaceIds)).toEqual([[], ["relay:96fe"]]);
  });

  it("draws this machine's host page and the connection path with the core", async () => {
    const core = await testCore();
    const machines = toMachines(demoMachineRows(NOW), rowsToSpaces(core, demoSpaceRows(NOW), NOW), core);
    expect(machines.map((m) => [m.id, m.connection])).toEqual([
      ["this-mac", "relay"],
      ["mac-mini", "relay"],
      ["linux-box", "relay"],
      ["studio-pc", "direct"],
    ]);
    const here = machines[0]!;
    expect(here.panel?.configured).toBe(true);
    expect(here.detail).toBe("Sharing · 1 connected");
    expect(here.panel?.facts.map((f) => f.label)).toEqual(["Name", "Access", "Service"]);
    expect(here.panel?.recent?.[0]?.text).toBe("ada@example.com · Screen and input");
    expect(machines.slice(1).every((m) => m.panel === undefined)).toBe(true);
  });

  it("explains host setup in the core's words", async () => {
    const core = await testCore();
    const guide = hostSetupGuide(core);
    expect(guide.choices.map((c) => c.id)).toEqual(["desktop", "spare"]);
    expect(guide.intro).toMatch(/no port forwarding/);
    expect(guide.form?.fields.map((f) => [f.id, f.advanced])).toEqual([
      ["name", false],
      ["profile", false],
      ["allow", false],
      ["direct", true],
      ["relay", true],
    ]);
    expect(hostSetupGuide(noCore)).toMatchObject({ form: null, choices: guide.choices, intro: guide.intro });
  });
});
