// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, describe, expect, it } from "vitest";
import { createDemoAdapter, demoOptionsFromSearch } from "../adapters/demo";
import { groupByApp } from "../derive";
import type { HostEvent } from "../protocol";

const adapters: ReturnType<typeof createDemoAdapter>[] = [];
const demo = (o: Parameters<typeof createDemoAdapter>[0] = {}) => {
  const a = createDemoAdapter({ latencyMs: 0, stepMs: 4, ...o });
  adapters.push(a);
  return a;
};
afterEach(() => adapters.splice(0).forEach((a) => a.dispose?.()));

describe("demo adapter", () => {
  it("has 6 Spaces across macOS, Windows and Linux on 4 machines, one offline", async () => {
    const a = demo();
    const rows = await a.call("spaces.list", {});
    const machines = await a.call("machines.list", {});
    expect(rows).toHaveLength(6);
    expect(new Set(rows.map((r) => r.os))).toEqual(new Set(["macos", "windows", "linux"]));
    expect(machines.map((m) => m.name)).toEqual(["This Mac", "Mac mini", "Linux box", "Studio PC"]);
    expect(machines.filter((m) => !m.online).map((m) => m.id)).toEqual(["studio-pc"]);
    expect(machines.find((m) => m.current)?.host?.configured).toBe(true);
    expect(machines.filter((m) => m.current)).toHaveLength(1);
    const hostOf = (r: (typeof rows)[number]) => r.host ?? "this-mac";
    const ids = new Set(machines.map((m) => m.id));
    expect(rows.map(hostOf).every((h) => ids.has(h))).toBe(true);
  });

  it("groups Keyvault items by app", async () => {
    const o = await demo().call("keyvault.overview", {});
    expect(o.availability).toBe("ready");
    const groups = groupByApp(o.items);
    expect(groups.map((g) => g.app)).toEqual(["Google Chrome", "Linear", "Notion", "Safari", "Slack"]);
    expect(groups.find((g) => g.key === "chrome")?.items).toHaveLength(5);
    expect(groups.find((g) => g.key === "chrome")?.lock).toBe("mixed");
    expect(groups.find((g) => g.key === "slack")?.lock).toBe("unlocked");
    expect(o.pending).toHaveLength(1);
  });

  it("steps a create through the SDK's phases, then lists it", async () => {
    const a = demo();
    const events: HostEvent[] = [];
    a.subscribe((e) => events.push(e));
    const row = await a.call("spaces.create", { config: { image: "ubuntu-xfce", name: "Scratch" }, pendingId: "pending:t" });
    const phases = events.flatMap((e) => (e.type === "spaces.createProgress" ? [e.progress.phase] : []));
    expect([...new Set(phases)]).toEqual(["preparing", "pulling", "creating", "booting", "waiting_for_services", "connecting"]);
    expect(events.at(-1)).toEqual({ type: "spaces.changed" });
    expect(row).toMatchObject({ id: "local:scratch", os: "linux", powerState: "running" });
    expect(await a.call("spaces.list", {})).toHaveLength(7);
  });

  it("creates on another machine with on: host:<id>", async () => {
    const row = await demo().call("spaces.create", { config: { image: "windows-11", on: "host:linux-box" }, pendingId: "p" });
    expect(row).toMatchObject({ provider: "relay", host: "linux-box", hostName: "Linux box", os: "windows" });
  });

  it("cancels a create", async () => {
    const a = demo({ stepMs: 20 });
    const create = a.call("spaces.create", { config: {}, pendingId: "pending:c" });
    await a.call("spaces.cancelCreate", { pendingId: "pending:c" });
    await expect(create).rejects.toThrow(/^cancelled/);
    expect(await a.call("spaces.list", {})).toHaveLength(6);
  });

  it("turns Spaces off and on", async () => {
    const a = demo();
    await a.call("spaces.setPower", { spaceId: "local:design-review", on: false });
    let row = (await a.call("spaces.list", {})).find((r) => r.id === "local:design-review");
    expect(row).toMatchObject({ powerState: "suspended", reachable: false });
    await a.call("spaces.setPower", { spaceId: "local:design-review", on: true });
    row = (await a.call("spaces.list", {})).find((r) => r.id === "local:design-review");
    expect(row).toMatchObject({ powerState: "running", reachable: true });
    await a.call("spaces.delete", { spaceId: "local:design-review" });
    expect(await a.call("spaces.list", {})).toHaveLength(5);
  });

  it("refuses unattended for identity providers", async () => {
    const a = demo();
    await expect(a.call("keyvault.setUnattended", { itemIds: ["kv-google"], unattended: true })).rejects.toThrow(/never runs unattended/);
    const items = await a.call("keyvault.setUnattended", { itemIds: ["kv-linear"], unattended: true });
    expect(items).toHaveLength(1);
    expect(items[0]?.policy.unattended).toBe(true);
  });

  it("starts locked and signed out on request", async () => {
    const a = demo(demoOptionsFromSearch("?demo=fresh,locked"));
    expect((await a.call("keyvault.overview", {})).availability).toBe("locked");
    const s = await a.call("session.get", {});
    expect(s.fleet.authMode).toBe("none");
    expect(s.onboarding.completed).toBe(false);
    await a.call("keyvault.unlock", {});
    expect((await a.call("keyvault.overview", {})).availability).toBe("ready");
  });

  it("starts with no Spaces on request", async () => {
    expect(await demo(demoOptionsFromSearch("?demo=empty")).call("spaces.list", {})).toEqual([]);
    expect((await demo(demoOptionsFromSearch("")).call("spaces.list", {})).length).toBeGreaterThan(0);
  });

  it("finishes a sign-in with an event", async () => {
    const a = demo({ signedIn: false });
    const events: HostEvent[] = [];
    a.subscribe((e) => events.push(e));
    expect((await a.call("session.signIn", {})).verificationUri).toMatch(/^https:/);
    await new Promise((r) => setTimeout(r, 30));
    expect(events).toContainEqual({ type: "session.signedIn", identity: "ada@example.com" });
    expect((await a.call("session.get", {})).fleet.identity).toBe("ada@example.com");
  });
});
