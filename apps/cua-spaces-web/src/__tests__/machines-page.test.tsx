// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Machines page (CUA-1673) on the real routes and the wasm core: the
// list in groups with one line each, This Mac's switches editable and
// another machine's read-only, Space rows that lead to the Space's page,
// New Space here with the machine chosen, rename, Details, and Remove from
// account (asked first). No Space actions on a machine.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { MachineRow, MachinesView } from "@/bridge/contracts/host";
import { readNewSpaceSession, resetNewSpaceSession } from "@/bridge/new-space";
import { withNotSharing } from "@/bridge/sharing";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
// New Space is one session for the whole page: a test that opens it must not leave it open for the next.
afterEach(resetNewSpaceSession);
stubBrowserApis();

const LINE = "Studio stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)";

/** The demo host with one more machine whose owner stopped sharing it. */
function stopped(calls: [string, unknown][]): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => {
      if (op === "spaces.list") {
        return [...(out as unknown[]), { id: "relay:studio", name: "Studio", provider: "relay", spacesdVersion: "0.6.0", features: [], reachable: false, error: LINE }];
      }
      if (op === "machines.list") {
        return [...(out as MachineRow[]), { id: "studio", name: "Studio", via: "relay", online: true, os: "macos", detail: LINE, limits: withNotSharing([], LINE) }];
      }
      return out;
    },
    intercept: (op, args) => {
      if (op === "machines.remove" || op === "machines.rename") {
        calls.push([op, args]);
        return null;
      }
      return undefined;
    },
  };
}

/** The approved mockup, drawn by the core as the Mac app's host draws it
 * (`parity/machines.json`, its first view). */
async function mockupView(): Promise<MachinesView> {
  const fixture = JSON.parse(readFileSync(resolve(process.cwd(), "../../libs/cua/crates/cua-spaces-app-core/parity/machines.json"), "utf8"));
  const core = await testCore();
  return core.call<MachinesView>("machines.view", { input: fixture.views[0].input });
}

/** The mockup with each machine's health as its daemon reported it
 * (`parity/machines.json`, the view "the mockup with issues"). */
async function issuesView(): Promise<MachinesView> {
  const fixture = JSON.parse(readFileSync(resolve(process.cwd(), "../../libs/cua/crates/cua-spaces-app-core/parity/machines.json"), "utf8"));
  const v = (fixture.views as { name: string; input: object }[]).find((x) => x.name.startsWith("the mockup with issues"))!;
  const core = await testCore();
  return core.call<MachinesView>("machines.view", { input: v.input });
}

function hostDrawn(view: MachinesView, calls: [string, unknown][]): TestHost {
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

const detailOf = (name: string) => screen.findByRole("region", { name: `${name} details` });
const select = (id: string) => act(() => (document.querySelector(`[data-machine-id="${id}"]`) as HTMLElement).click());

describe.skipIf(!wasmBuilt)("the Machines page (real routes, wasm core)", () => {
  it("draws the core's page for the rows listed here, a machine that stopped sharing included", async () => {
    const calls: [string, unknown][] = [];
    const { router, hooks } = await mountApp("/machines", stopped(calls));
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.machines.some((m) => m.id === "studio")).toBe(true));
    await screen.findByText(hooks.machines.view!.summary);

    const row = document.querySelector('[data-machine-id="studio"]') as HTMLElement;
    expect(row.querySelector("[data-machine-line]")?.textContent).toBe("Not sharing · No Spaces");
    // No subtitle of OS and connection, and no "Not set up for access".
    expect(document.body.textContent).not.toMatch(/Not set up for access|macOS · Cua relay/);
    select("studio");
    const detail = await detailOf("Studio");
    expect(detail.querySelector("[data-machine-status]")?.textContent).toContain("Not sharing");
    // Online, so the relay would only take it back: Remove says where.
    const remove = detail.querySelector<HTMLButtonElement>('[data-machine-remove="studio"]')!;
    expect(remove.disabled).toBe(true);
    expect(remove.parentElement?.getAttribute("title")).toMatch(/Remove it on that Mac/);
    expect(detail.querySelector<HTMLButtonElement>("[data-machine-new-space]")!.disabled).toBe(true);
  });

  it("matches the approved mockup: groups, switches, Space links, New Space here, rename, Details and Remove", async () => {
    const view = await mockupView();
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/machines", hostDrawn(view, calls));
    await router.load();
    await screen.findByText(view.summary);

    // Groups, with the old machines collapsed.
    const groups = [...document.querySelectorAll("[data-machine-group]")].map((g) => g.getAttribute("data-machine-group"));
    expect(groups).toEqual(["yours", "old", "team"]);
    const old = document.querySelector<HTMLElement>('[data-machine-id="old1"]')!;
    expect(old.hidden).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: /Offline for 30\+ days/ }));
    expect(old.hidden).toBe(false);
    expect(screen.getByText("Remove old machines")).toBeTruthy();
    expect(document.querySelector('[data-machine-id="this-mac"]')?.textContent).toContain("This Mac");

    // This Mac: its switches change here.
    let detail = await detailOf("Ada's MacBook Pro");
    const switches = () => [...detail.querySelectorAll<HTMLElement>("[data-machine-toggle] [role=switch]")];
    expect(switches().map((s) => s.getAttribute("aria-disabled") === "true" || s.hasAttribute("data-disabled"))).toEqual([false, false]);

    // Another Mac: read-only, and where to change it.
    select("studio");
    detail = await detailOf("cua's Mac Studio");
    expect(switches().every((s) => s.hasAttribute("data-disabled"))).toBe(true);
    expect(detail.querySelector("[data-machine-toggles-note]")?.textContent).toBe("Change this on that Mac.");
    // No Space actions on a machine.
    expect(within(detail).queryByRole("button", { name: /^(Open|Start|Stop|Teleport)/ })).toBeNull();

    // Details, collapsed until opened.
    expect(detail.querySelector("[data-machine-details]")).toBeNull();
    fireEvent.click(detail.querySelector("[data-machine-details-toggle]")!);
    expect(detail.querySelector("[data-machine-details]")?.textContent).toMatch(/OS.*macOS 26\.5\.2.*Connection.*via Cua relay.*Service.*Running/);

    // Rename saves on the relay machine.
    fireEvent.click(detail.querySelector("[data-machine-rename]")!);
    const input = await waitFor(() => document.querySelector<HTMLInputElement>("[data-machine-rename-input]")!);
    fireEvent.change(input, { target: { value: "Studio" } });
    fireEvent.submit(input.closest("form")!);
    await waitFor(() => expect(calls).toContainEqual(["machines.rename", { id: "studio", name: "Studio" }]));

    // New Space here: the wizard, with this machine chosen.
    fireEvent.click(detail.querySelector('[data-machine-new-space="host:studio"]')!);
    await waitFor(() => expect(readNewSpaceSession().open).toBe(true));
    // Chosen as soon as the wizard offers it, or waiting for it to be.
    await waitFor(() => {
      const s = readNewSpaceSession();
      expect(s.placeOn === "host:studio" || (s.state as { host?: string } | null)?.host === "studio").toBe(true);
    });
    act(() => router.history.back());

    // A Space row leads to the Space's own page.
    select("studio");
    detail = await detailOf("cua's Mac Studio");
    fireEvent.click(detail.querySelector('[data-space-row="relay:studio/xm"]')!);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/spaces/${encodeURIComponent("relay:studio/xm")}`));
  });

  it("asks before removing a machine from the account; Cancel removes nothing", async () => {
    const view = await mockupView();
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/machines", hostDrawn(view, calls));
    await router.load();
    await screen.findByText(view.summary);
    select("byoc");
    const detail = await detailOf("Cua BYOC tests");
    expect(detail.querySelector("[data-machine-notice]")?.textContent).toMatch(/offline/);
    const remove = detail.querySelector<HTMLButtonElement>('[data-machine-remove="byoc"]')!;
    expect(remove.textContent).toBe("Remove from account");
    fireEvent.click(remove);
    let dialog = await screen.findByRole("alertdialog");
    expect(dialog.textContent).toContain("Remove “Cua BYOC tests” from your account?");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const changes = () => calls.filter(([op]) => op !== "machines.checkHealth");
    expect(changes()).toEqual([]);

    fireEvent.click(remove);
    dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(changes()).toEqual([["machines.remove", { machine: "byoc", device: "dev_byoc" }]]));
  });

  it("a team machine has no Remove and says its owner changes it", async () => {
    const view = await mockupView();
    const { router } = await mountApp("/machines", hostDrawn(view, []));
    await router.load();
    await screen.findByText(view.summary);
    select("team");
    const detail = await detailOf("build-mini-01");
    expect(detail.querySelector("[data-machine-status]")?.textContent).toContain("Shared by your team");
    expect(detail.querySelector("[data-machine-toggles-note]")?.textContent).toBe("Its owner changes this on that Mac.");
    expect(detail.querySelector("[data-machine-remove]")).toBeNull();
    expect(detail.querySelector("[data-machine-rename]")).toBeNull();
  });

  it("shows a machine's issues with their fixes, tints its dot, and shows nothing for a healthy or offline machine", async () => {
    const view = await issuesView();
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/machines", hostDrawn(view, calls));
    await router.load();
    await screen.findByText(view.summary);
    // The page asks the machines for their health once it opens.
    await waitFor(() => expect(calls.some(([op]) => op === "machines.checkHealth")).toBe(true));

    // The dots: red for This Mac, amber for the Mac Studio, none on the rest.
    const tint = (id: string) => document.querySelector(`[data-machine-id="${id}"] [data-machine-health]`)?.getAttribute("data-machine-health") ?? null;
    expect([tint("this-mac"), tint("studio"), tint("byoc"), tint("old1")]).toEqual(["fail", "warn", null, null]);

    // This Mac: Local Network (fail) and disk (warn), each with its fix.
    let detail = await detailOf("Ada's MacBook Pro");
    const issues = () => [...detail.querySelectorAll<HTMLElement>("[data-machine-issue]")];
    expect(issues().map((i) => [i.dataset.machineIssue, i.dataset.level])).toEqual([
      ["machine.local_network", "fail"],
      ["machine.disk", "warn"],
    ]);
    expect(issues()[0]!.textContent).toContain("Local Network access is off for Cua Spaces.");
    fireEvent.click(within(issues()[0]!).getByRole("button", { name: "Open Local Network settings" }));
    await waitFor(() =>
      expect(calls).toContainEqual(["host.openSettings", { url: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork" }]),
    );
    expect(within(issues()[1]!).getByRole("button", { name: "Free disk space" })).toBeTruthy();

    // The Mac Studio: said there, with no buttons (its settings are on it).
    select("studio");
    detail = await detailOf("cua's Mac Studio");
    expect(issues().map((i) => i.textContent)).toEqual([
      "Running Spaces use 28 of 32 GB of memory on that Mac, so another may not start.",
      "It runs Cua 0.3.9, older than this Mac’s 0.4.1. Update Cua on it.",
    ]);
    expect(detail.querySelector("[data-machine-issue-action]")).toBeNull();

    // Healthy (the team machine) and offline (old checks not shown): no section.
    for (const [id, name] of [["team", "build-mini-01"], ["byoc", "Cua BYOC tests"]] as const) {
      select(id);
      detail = await detailOf(name);
      expect(detail.querySelector("[data-machine-issues]")).toBeNull();
      expect(detail.textContent).not.toContain("Issues");
    }
  });

  it("a Restart service fix runs the host action", async () => {
    const view = await issuesView();
    const mac = view.machines.find((m) => m.current)!;
    mac.issues = [{ id: "service", level: "fail", text: "The Cua service isn’t running.", action: { label: "Restart service", hostAction: "resume-sharing" } }];
    mac.health = "fail";
    const calls: [string, unknown][] = [];
    const { router } = await mountApp("/machines", hostDrawn(view, calls));
    await router.load();
    const detail = await detailOf("Ada's MacBook Pro");
    fireEvent.click(within(detail).getByRole("button", { name: "Restart service" }));
    await waitFor(() => expect(calls.some(([op, a]) => op === "host.action" && JSON.stringify(a).includes("resume-sharing"))).toBe(true));
  });
});
