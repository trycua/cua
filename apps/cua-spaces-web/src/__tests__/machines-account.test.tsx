// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// One list of machines (CUA-2621): the machines waiting to be added are the top group with
// "Add machine" and "Not me" and a count on the sidebar, a machine that is only signed in is a row
// in the state "Signed in, not sharing" that Revoke removes, each page has Rename, Revoke, recent
// access and (this machine) its trust, and the old Settings tab lands on Machines.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { DevicesInput, MachineRow, MachinesView } from "@/bridge";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

type Calls = [string, unknown][];

/** The demo host, noting the calls it is asked for; `extra` rewrites what the relay says of the account. */
function host(calls: Calls, extra?: (d: DevicesInput) => DevicesInput): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => (op === "devices.get" && extra ? extra(out as DevicesInput) : out),
    intercept: (op, args) => {
      calls.push([op, args]);
      return undefined;
    },
  };
}

/** The approved mockup, drawn by the core with a signed-in machine that shares nothing (Ana's laptop). */
async function mockupHost(calls: Calls): Promise<TestHost> {
  const fixture = JSON.parse(readFileSync(resolve(process.cwd(), "../../libs/cua/crates/cua-spaces-app-core/parity/machines.json"), "utf8"));
  const view = (await testCore()).call<MachinesView>("machines.view", { input: fixture.views[0].input });
  const rows: MachineRow[] = view.machines.map((m) => ({ id: m.id, name: m.name, via: m.current ? "local" : "relay", online: m.online, os: m.os, current: m.current || undefined, limits: [] }));
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => (op === "machines.list" ? rows : out),
    intercept: (op, args) => {
      if (op === "machines.remove" || op === "machines.rename" || op === "machines.checkHealth" || op === "host.openSettings" || op === "host.action") {
        calls.push([op, args]);
        return null;
      }
      return undefined;
    },
    machinesView: () => view,
  };
}

const q = (sel: string) => document.querySelector<HTMLElement>(sel);
const names = () => [...document.querySelectorAll("[data-pending-machine]")].map((e) => e.querySelector("span.truncate")?.textContent);
const sentFor = (calls: Calls, op: string) => calls.filter(([o]) => o === op).map(([, a]) => a);

describe.skipIf(!wasmBuilt)("one list of machines (real routes, wasm core)", () => {
  it("puts the machines waiting to be added first, each with Add machine and Not me, and counts them in the sidebar", async () => {
    await mountApp("/machines", host([]));
    await waitFor(() => expect(q('[data-machine-group="pending"]')).not.toBeNull());
    expect(names()).toEqual(["Work laptop", "Old laptop"]);
    // First in the list, above the clouds and the machines.
    expect(document.querySelector("[data-machine-group], [data-cloud-group]")?.getAttribute("data-machine-group")).toBe("pending");
    for (const row of document.querySelectorAll("[data-pending-machine]")) {
      expect(within(row as HTMLElement).getByRole("button", { name: "Add machine" })).toBeTruthy();
      expect(within(row as HTMLElement).getByRole("button", { name: "Not me" })).toBeTruthy();
    }
    expect(q("[data-pending-badge]")?.textContent).toBe("2");
    // The count is on every page, not only on Machines.
    cleanup();
    await mountApp("/spaces", host([]));
    await waitFor(() => expect(q("[data-pending-badge]")?.textContent).toBe("2"));
  });

  it("Add machine adds the waiting machine by its id, with no code to type, and it leaves the waiting group", async () => {
    const calls: Calls = [];
    await mountApp("/machines", host(calls));
    await waitFor(() => expect(names()).toContain("Work laptop"));
    fireEvent.click(within(q('[data-pending-machine="dev_work"]')!).getByRole("button", { name: "Add machine" }));
    await waitFor(() => expect(sentFor(calls, "devices.approve")).toEqual([{ code: null, deviceId: "dev_work" }]));
    await waitFor(() => expect(names()).toEqual(["Old laptop"]));
    expect(q("[data-pending-badge]")?.textContent).toBe("1");
  });

  it("Not me refuses the waiting machine, and it leaves the list", async () => {
    const calls: Calls = [];
    await mountApp("/machines", host(calls));
    await waitFor(() => expect(names()).toContain("Work laptop"));
    fireEvent.click(within(q('[data-pending-machine="dev_work"]')!).getByRole("button", { name: "Not me" }));
    await waitFor(() => expect(sentFor(calls, "devices.revoke")).toEqual([{ id: "dev_work" }]));
    await waitFor(() => expect(names()).toEqual(["Old laptop"]));
    expect(q('[data-machine-id="dev_work"]')).toBeNull();
  });

  const withUnknown = (d: DevicesInput): DevicesInput => ({ ...d, machines: [...(d.machines ?? []), { id: "m3", name: "unknown-box", confirmed: false }] });

  it("a machine that registered without proof is confirmed with Add machine", async () => {
    const calls: Calls = [];
    await mountApp("/machines", host(calls, withUnknown));
    await waitFor(() => expect(names()).toContain("unknown-box"));
    fireEvent.click(within(q('[data-pending-machine="m3"]')!).getByRole("button", { name: "Add machine" }));
    await waitFor(() => expect(sentFor(calls, "devices.confirmMachine")).toEqual([{ id: "m3" }]));
  });

  it("Not me on a machine that registered without proof asks first, then removes it from the account", async () => {
    const calls: Calls = [];
    await mountApp("/machines", host(calls, withUnknown));
    await waitFor(() => expect(names()).toContain("unknown-box"));
    fireEvent.click(within(q('[data-pending-machine="m3"]')!).getByRole("button", { name: "Not me" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(sentFor(calls, "machines.remove")).toEqual([]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(sentFor(calls, "machines.remove")).toEqual([{ machine: "m3", device: null }]));
  });

  it("the waiting machine's page asks 'Add this machine?' with its name and system and two buttons", async () => {
    await mountApp("/machines", host([]));
    await waitFor(() => expect(names()).toContain("Work laptop"));
    act(() => q('[data-pending-machine="dev_work"] button')!.click());
    const page = await waitFor(() => q('[data-pending-detail="dev_work"]')!);
    expect(within(page).getByRole("heading", { name: "Add this machine?" })).toBeTruthy();
    expect(page.textContent).toContain("Work laptop");
    expect(page.textContent).toContain("Windows");
    expect(within(page).getByRole("button", { name: "Add machine" })).toBeTruthy();
    expect(within(page).getByRole("button", { name: "Not me" })).toBeTruthy();
    expect(page.textContent).not.toMatch(/device|computer|enroll/i);
  });

  it("a machine that is only signed in is a row 'Signed in, not sharing', with no Spaces switches, and Revoke asks first", async () => {
    const calls: Calls = [];
    await mountApp("/machines", await mockupHost(calls));
    const row = await waitFor(() => {
      const r = q('[data-machine-id="dev_lap"]');
      if (!r) throw new Error("no row");
      return r;
    });
    expect(row.querySelector("[data-machine-line]")?.textContent).toBe("Signed in, not sharing");
    expect(q('[data-machine-group="yours"]')?.contains(row)).toBe(true);
    expect(document.body.textContent).not.toMatch(/Your devices|Signed-in device/);
    act(() => row.click());
    const page = await screen.findByRole("region", { name: "Ana's laptop details" });
    expect(page.querySelector("[data-machine-status]")?.textContent).toContain("Signed in, not sharing");
    expect(page.querySelector("[data-machine-toggle]")).toBeNull();
    // Rename and Revoke are on its page.
    expect(page.querySelector("[data-machine-rename]")).not.toBeNull();
    const revoke = page.querySelector<HTMLElement>('[data-machine-remove="dev_lap"]')!;
    expect(revoke.textContent).toBe("Revoke");
    fireEvent.click(revoke);
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toMatch(/Revoke/);
    expect(dialog.textContent).not.toMatch(/device/i);
    fireEvent.click(within(dialog).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(sentFor(calls, "machines.remove")).toEqual([{ machine: null, device: "dev_lap" }]));
  });

  it("each machine's page has its recent access, and this machine's page says how far it is trusted", async () => {
    await mountApp("/machines", host([]));
    const trust = await waitFor(() => {
      const t = q("[data-machine-trust]");
      if (!t) throw new Error("no trust section");
      return t;
    });
    expect(trust.getAttribute("data-machine-trust")).toBe("enrolled");
    expect(trust.querySelector("[data-machine-trust-status]")?.textContent).toMatch(/^Trusted until/);
    // The demo's relay says studio-mac and mac-mini were reached.
    act(() => q('[data-machine-id="mac-mini"]')?.click());
    const recent = await waitFor(() => {
      const r = q("[data-machine-recent]");
      if (!r) throw new Error("no recent access");
      return r;
    });
    expect(recent.textContent).not.toMatch(/device|computer|enrolled/i);
  });

  it("the old Trusted machines address lands on Machines", async () => {
    const { router } = await mountApp("/settings/devices", host([]));
    await waitFor(() => expect(router.state.location.pathname).toBe("/machines"));
  });
});
