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
async function issuesView(name = "the mockup with issues"): Promise<MachinesView> {
  const fixture = JSON.parse(readFileSync(resolve(process.cwd(), "../../libs/cua/crates/cua-spaces-app-core/parity/machines.json"), "utf8"));
  const v = (fixture.views as { name: string; input: object }[]).find((x) => x.name.startsWith(name))!;
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
    expect(row.querySelector("[data-machine-line]")?.textContent).toBe("Not sharing");
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
    expect(switches().map((s) => s.getAttribute("aria-disabled") === "true" || s.hasAttribute("data-disabled"))).toEqual([false]);

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

  it("a peer on a newer Cua is a plain note when this app is the newest release: no warning, no button", async () => {
    const view = await issuesView("the Mac Studio runs a newer Cua and this app is the newest");
    const { router } = await mountApp("/machines", hostDrawn(view, []));
    await router.load();
    await screen.findByText(view.summary);
    expect(document.querySelector('[data-machine-id="studio"] [data-machine-health]')?.getAttribute("data-machine-health")).toBe("warn");
    select("studio");
    const detail = await detailOf("cua's Mac Studio");
    const note = detail.querySelector<HTMLElement>('[data-machine-issue="version"]')!;
    expect(note.dataset.level).toBe("info");
    expect(note.textContent).toContain("newer than this Mac’s 0.4.1");
    expect(note.textContent).not.toContain("Update Cua here");
    expect(note.querySelector("[data-machine-issue-action]")).toBeNull();
  });

  it("with the Cua Spaces update downloaded, the newer-Cua row offers Restart to update", async () => {
    const view = await issuesView("the Mac Studio runs a newer Cua and this app has the update");
    const calls: [string, unknown][] = [];
    const h = hostDrawn(view, calls);
    const base = h.intercept;
    h.intercept = (op, args) => {
      if (op === "update.install") {
        calls.push([op, args]);
        return null;
      }
      // No agent is working: a restart cuts nothing short.
      if (op === "agents.runs") return [];
      return base?.(op, args);
    };
    const { router } = await mountApp("/machines", h);
    await router.load();
    await screen.findByText(view.summary);
    select("studio");
    const detail = await detailOf("cua's Mac Studio");
    const row = detail.querySelector<HTMLElement>('[data-machine-issue="version"]')!;
    expect(row.dataset.level).toBe("warn");
    expect(row.textContent).toContain("Cua Spaces 0.12.2 is ready to install");
    fireEvent.click(within(row).getByRole("button", { name: "Restart to update" }));
    await waitFor(() => expect(calls.some(([op]) => op === "update.install")).toBe(true));
  });

  it("lists a shared desktop first among a machine's Spaces, drawn as any Space row", async () => {
    const view = await issuesView();
    const studio = view.machines.find((m) => m.id === "studio")!;
    expect(studio.desktop?.label).toBe("Desktop");
    const { router } = await mountApp("/machines", hostDrawn(view, []));
    await router.load();
    await screen.findByText(view.summary);
    select("studio");
    const detail = await detailOf("cua's Mac Studio");
    const rows = [...detail.querySelectorAll<HTMLElement>("[data-machine-spaces] [data-space-row]")];
    // The desktop first, then the Spaces: one row component, one look.
    expect(rows.map((r) => r.dataset.spaceRow)).toEqual([studio.desktop!.id, ...studio.spaces.map((x) => x.id)]);
    expect(rows[0]!.getAttribute("href")).toBe(`/spaces/${encodeURIComponent(studio.desktop!.id)}`);
    expect(rows[0]!.textContent).toContain("Desktop");
    expect(rows[1]!.textContent).not.toContain("Desktop");
  });

  it("reads a shared desktop in the row: desktop only, desktop with Spaces, Spaces only", async () => {
    const view = await issuesView("a machine sharing only its desktop");
    const { router } = await mountApp("/machines", hostDrawn(view, []));
    await router.load();
    await screen.findByText(view.summary);
    const line = (id: string) => document.querySelector(`[data-machine-id="${id}"] [data-machine-line]`)?.textContent;
    expect([line("solo"), line("both"), line("spaces")]).toEqual(["Desktop shared", "Desktop \u00b7 2 Spaces", "1 Space running"]);
    select("solo");
    const detail = await detailOf("solo");
    const rows = [...detail.querySelectorAll<HTMLElement>("[data-machine-spaces] [data-space-row]")];
    expect(rows.map((r) => r.dataset.spaceRow)).toEqual(["relay:solo"]);
  });

  it("draws the clouds: a group under the machines, each cloud's page, and Connect a cloud", async () => {
    const { router, hooks } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "connected" } });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.group.rows.map((r) => r.id)).toEqual(["aws", "hetzner"]));
    const group = document.querySelector("[data-cloud-group]")!;
    expect([...group.querySelectorAll("[data-cloud-id]")].map((r) => [r.getAttribute("data-cloud-id"), r.querySelector("[data-cloud-line]")?.textContent])).toEqual([
      ["aws", "3 running"],
      ["hetzner", "2 running"],
    ]);
    // The logo is the vendor's own file, AWS's in both its inks.
    expect(group.querySelectorAll("[data-cloud-logo=aws] img")).toHaveLength(2);
    expect(group.querySelector("[data-cloud-logo=hetzner] img")?.getAttribute("src")).toBe("/cloud-logos/hetzner-apple-icon-180x180.png");

    act(() => (group.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
    const aws = await detailOf("AWS");
    expect(aws.querySelector("[data-cloud-status]")?.textContent).toBe("Connected");
    expect(aws.querySelector("[data-cloud-spend-amount]")?.textContent).toBe("$42.18");
    expect([...aws.querySelectorAll("[data-cloud-machine]")].map((m) => m.textContent)).toEqual([
      "rl-env-0412agent eval-runner$0.08",
      "rl-env-0413agent eval-runner$0.08",
      "build-cacheada on Mac mini$0.05",
    ]);
    expect([...aws.querySelectorAll("[data-cloud-setting]")].map((s) => s.getAttribute("data-cloud-setting"))).toEqual(["region", "budget", "size", "access"]);
    expect(aws.querySelector("[data-cloud-issues]")).toBeNull();

    // Connect a cloud: the picker, five rows, a read-only prompt.
    act(() => (document.querySelector("[data-connect-a-cloud]") as HTMLElement).click());
    const sheet = await waitFor(() => document.querySelector("[data-connect-cloud]")!);
    expect(sheet).not.toBeNull();
    expect([...sheet.querySelectorAll("[data-cloud-row-title]")].map((r) => r.textContent)).toEqual(["AWS", "Google Cloud", "Microsoft Azure", "Hetzner", "Modal"]);
    expect([...sheet.querySelectorAll("[data-cloud-row-auth]")].map((r) => r.textContent)).toEqual(["OIDC", "Coming soon", "Coming soon", "API token", "API token"]);
    const prompt = sheet.querySelector<HTMLTextAreaElement>("[data-cloud-prompt]")!;
    expect(prompt.readOnly).toBe(true);
    expect(prompt.value).toContain("`cua cloud status` says connected.");
    // The subtitle ends with the quiet link to the cloud comparison (CUA-2200).
    const compare = sheet.querySelector<HTMLButtonElement>("[data-cloud-intro] [data-compare-clouds]")!;
    expect(compare.textContent).toBe("Compare clouds ↗");
  });

  it("each way a cloud goes wrong is one line in the list and one fix on its page", async () => {
    for (const [clouds, cloud, status, line, action] of [
      ["budget", "aws", "Budget reached", "Budget reached", "raise-budget"],
      ["untrusted", "aws", "Not trusted", "Not trusted", "set-up-again"],
      ["token", "hetzner", "Token refused", "Token refused", "new-token"],
      ["approval", "aws", "Waiting for approval", "Waiting for approval", "review"],
    ] as const) {
      const { router, hooks } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds } });
      await router.load();
      await waitFor(() => expect(hooks.machines.view?.clouds?.pages.some((p) => p.id === cloud)).toBe(true));
      act(() => (document.querySelector(`[data-cloud-id=${cloud}]`) as HTMLElement).click());
      const page = await detailOf(cloud === "aws" ? "AWS" : "Hetzner");
      expect(page.querySelector("[data-cloud-status]")?.textContent).toBe(status);
      expect(document.querySelector(`[data-cloud-id=${cloud}] [data-cloud-line]`)?.textContent).toBe(line);
      expect(page.querySelector(`[data-cloud-issue-action=${action}]`)).not.toBeNull();
      cleanup();
      resetNewSpaceSession();
    }
  });

  it("approves what an agent set up from the review sheet", async () => {
    const calls: [string, unknown][] = [];
    const { router, hooks } = await mountApp("/machines", {
      failsCreates: false,
      listsGhost: false,
      demo: { clouds: "approval" },
      intercept: (op, args) => {
        if (op === "clouds.approve") calls.push([op, args]);
        return undefined;
      },
    });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.approval?.title).toBe("Approve AWS for your agent?"));
    act(() => (document.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
    const page = await detailOf("AWS");
    act(() => (page.querySelector("[data-cloud-issue-action=review]") as HTMLElement).click());
    const sheet = await waitFor(() => document.querySelector("[data-cloud-approval]")!);
    expect(sheet.textContent).toContain("Approve AWS for your agent?");
    act(() => (sheet.querySelector("[data-cloud-approve]") as HTMLElement).click());
    await waitFor(() => expect(calls).toEqual([["clouds.approve", { provider: "aws", approve: true }]]));
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.status).toBe("Connected"));
  });

  it("lists the clouds above your machines, in the order the core gives", async () => {
    const { router, hooks } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "connected" } });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.group.rows.length).toBe(2));
    expect(hooks.machines.view!.order).toEqual(["clouds", "yours"]);
    const sections = [...document.querySelectorAll("[data-cloud-group], [data-machine-group]")].map((s) => s.getAttribute("data-cloud-group") ?? s.getAttribute("data-machine-group"));
    expect(sections).toEqual(["clouds", "yours"]);
    // Even with no cloud connected, "Connect a cloud" is the first row.
    cleanup();
    resetNewSpaceSession();
    await mountApp("/machines", { failsCreates: false, listsGhost: false }).then(async ({ router: r }) => r.load());
    await waitFor(() => expect(document.querySelector("[data-connect-a-cloud]")).not.toBeNull());
    const first = document.querySelector("[data-cloud-group], [data-machine-group]");
    expect(first?.getAttribute("data-cloud-group")).toBe("clouds");
  });

  it("scrolls the list and the detail pane on their own, under a header that stays put", async () => {
    const { router } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "connected", manyMachines: true } });
    await router.load();
    await waitFor(() => expect(document.querySelectorAll("[data-machine-id]").length).toBeGreaterThan(15));
    const list = document.querySelector<HTMLElement>("[data-machines-list-scroll]")!;
    const detail = document.querySelector<HTMLElement>("[data-machines-detail-scroll]")!;
    const split = document.querySelector<HTMLElement>("[data-machines-split]")!;
    for (const pane of [list, detail]) {
      expect(pane.className).toContain("overflow-y-auto");
      expect(pane.className).toContain("min-h-0");
    }
    expect(list.contains(detail) || detail.contains(list)).toBe(false);
    // Bounded: the split fills what the page leaves, and the page as a whole does not scroll.
    expect(split.className).toContain("flex-1");
    expect(split.className).toContain("min-h-0");
    const page = document.querySelector<HTMLElement>("[data-page-contained]")!;
    expect(page.className).toContain("overflow-hidden");
    expect(page.className).not.toContain("overflow-y-auto");
    // The header is outside both panes.
    const header = document.querySelector("h1")!;
    expect(list.contains(header) || detail.contains(header)).toBe(false);
  });

  it("keeps the selected row in view in the list", async () => {
    const calls: string[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element, arg?: boolean | ScrollIntoViewOptions) {
      calls.push(`${this.getAttribute("data-machine-id") ?? "cloud:" + this.getAttribute("data-cloud-id")} ${JSON.stringify(arg)}`);
    };
    try {
      const { router } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "connected", manyMachines: true } });
      await router.load();
      await waitFor(() => expect(document.querySelectorAll("[data-machine-id]").length).toBeGreaterThan(15));
      select("build-13");
      await waitFor(() => expect(calls).toContain('build-13 {"block":"nearest"}'));
      act(() => (document.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
      await waitFor(() => expect(calls).toContain('cloud:aws {"block":"nearest"}'));
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });

  it("a CLI sign-in AWS link offers Switch to OIDC, which opens AWS's sheet filled from the link", async () => {
    const { router, hooks } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "cli" } });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.switchAuth?.label).toBe("Switch to OIDC"));
    act(() => (document.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
    const page = await detailOf("AWS");
    expect(page.querySelector("[data-cloud-status]")?.textContent).toBe("Connected");
    expect(page.querySelector("[data-cloud-issues]")).toBeNull();
    expect(page.querySelector("[data-cloud-setting=access]")?.textContent).toContain("The AWS CLI's sign-in on this computer");
    act(() => (page.querySelector("[data-cloud-switch=switch-oidc]") as HTMLElement).click());
    const sheet = await waitFor(() => document.querySelector("[data-connect-cloud]")!);
    await waitFor(() => expect(sheet.querySelector("[data-cloud-title]")?.textContent).toContain("AWS"));
    // Filled from the CLI link: its region, budget and largest machine.
    await waitFor(() => expect(sheet.textContent).toContain("8 vCPU, 32 GB"));
    expect(sheet.textContent).toContain("us-west-2");
    expect((sheet.querySelector("#cc-budget") as HTMLInputElement).value).toBe("150");
    // An OIDC link offers nothing to switch.
    cleanup();
    resetNewSpaceSession();
    const other = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "connected" } });
    await other.router.load();
    await waitFor(() => expect(other.hooks.machines.view?.clouds?.pages[0]?.id).toBe("aws"));
    expect(other.hooks.machines.view?.clouds?.pages[0]?.switchAuth).toBeNull();
  });

  it("a link revoked elsewhere shows the host's sentence with Reconnect", async () => {
    const { router, hooks } = await mountApp("/machines", { failsCreates: false, listsGhost: false, demo: { clouds: "revoked" } });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.issues[0]?.id).toBe("link"));
    act(() => (document.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
    const page = await detailOf("AWS");
    expect(page.querySelector("[data-cloud-issue=link]")?.textContent).toContain("disconnected on another device");
    expect(page.querySelector("[data-cloud-issue=trust]")).toBeNull();
    act(() => (page.querySelector("[data-cloud-issue-action=reconnect]") as HTMLElement).click());
    await waitFor(() => expect(document.querySelector("[data-connect-cloud]")).not.toBeNull());
  });

  it("an agent's raise of the limits shows who asked and what, and Deny keeps the old limits", async () => {
    const calls: [string, unknown][] = [];
    const { router, hooks } = await mountApp("/machines", {
      failsCreates: false,
      listsGhost: false,
      demo: { clouds: "limits" },
      intercept: (op, args) => {
        if (op === "clouds.approve") calls.push([op, args]);
        return undefined;
      },
    });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.approval?.title).toBe("Approve new AWS limits?"));
    act(() => (document.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
    const page = await detailOf("AWS");
    act(() => (page.querySelector("[data-cloud-issue-action=review]") as HTMLElement).click());
    const sheet = await waitFor(() => document.querySelector("[data-cloud-approval]")!);
    expect(sheet.textContent).toContain("eval-runner asked to raise the limits on AWS");
    expect(sheet.textContent).toContain("$500");
    expect(sheet.textContent).toContain("8 vCPU, 32 GB");
    act(() => (sheet.querySelector("[data-cloud-deny]") as HTMLElement).click());
    await waitFor(() => expect(calls).toEqual([["clouds.approve", { provider: "aws", approve: false }]]));
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.status).toBe("Connected"));
    expect(hooks.machines.view?.clouds?.pages[0]?.settings.find((s) => s.id === "budget")?.value).toBe("$200");
  });
});
