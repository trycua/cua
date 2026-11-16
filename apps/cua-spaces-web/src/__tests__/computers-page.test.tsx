// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Your computers (CUA-1673, CUA-3551) on the real routes and the wasm core:
// the list opens each computer's page, This Mac's switches editable and
// another computer's read-only, Space rows that lead to the Space's page,
// New Space here with the computer chosen, rename, Details, and Remove from
// account (asked first). No Space actions on a computer. The clouds you
// connected are listed under your computers, each with its own page.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
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
        return [
          ...(out as unknown[]),
          { id: "relay:studio", name: "Studio", provider: "relay", spacesdVersion: "0.6.0", features: [], reachable: false, error: LINE },
        ];
      }
      if (op === "machines.list") {
        return [
          ...(out as MachineRow[]),
          { id: "studio", name: "Studio", via: "relay", online: true, os: "macos", detail: LINE, limits: withNotSharing([], LINE) },
        ];
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
  const rows: MachineRow[] = view.machines.map((m) => ({
    id: m.id,
    name: m.name,
    via: m.current ? "local" : "relay",
    online: m.online,
    os: m.os,
    current: m.current || undefined,
    limits: [],
    ...(m.id.startsWith("dev_") ? { device: true } : {}),
  }));
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

/** The router of the app mounted last: a computer's page opens at its own address, as its row in the list opens it. */
let shown: Awaited<ReturnType<typeof mountApp>> | null = null;
async function mountAt(path: string, host: TestHost) {
  shown = await mountApp(path, host);
  return shown;
}
const select = (id: string) =>
  act(async () => {
    await shown!.router.navigate({ to: "/spaces/computer/$computerId", params: { computerId: id } });
  });
/** The element once it is drawn. */
function found<T extends Element = HTMLElement>(sel: string): Promise<T> {
  return waitFor(() => {
    const el = document.querySelector<T>(sel);
    if (!el) throw new Error(`nothing matches ${sel}`);
    return el;
  });
}
/** The list of your computers has drawn. */
const listed = () => waitFor(() => expect(document.querySelector("[data-spaces-tree]")).not.toBeNull());

describe.skipIf(!wasmBuilt)("your computers and each one's page (real routes, wasm core)", () => {
  it("draws the core's page for the rows listed here, a machine that stopped sharing included", async () => {
    const calls: [string, unknown][] = [];
    const { router, hooks } = await mountAt("/spaces", stopped(calls));
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.machines.some((m) => m.id === "studio")).toBe(true));
    await listed();

    const row = await waitFor(() => {
      document.querySelector<HTMLElement>('[data-tree-offline-toggle][aria-expanded="false"]')?.click();
      const el = document.querySelector<HTMLElement>('[data-machine-id="studio"]');
      if (!el) throw new Error("no Studio row");
      return el;
    });
    expect(row.querySelector("[data-tree-status]")?.textContent).toBe("Not sharing");
    // No subtitle of OS and connection, and no "Not set up for access".
    expect(document.body.textContent).not.toMatch(/Not set up for access|macOS · Cua relay/);
    await select("studio");
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
    const { router } = await mountAt("/spaces", hostDrawn(view, calls));
    await router.load();
    await listed();

    // The computers waiting to be added come first; the old ones fold into one offline row, with Remove old
    // computers beside it.
    expect(document.querySelector("[data-tree-pending]")).not.toBeNull();
    expect(document.querySelector('[data-machine-id="old1"]')).toBeNull();
    fireEvent.click(document.querySelector("[data-tree-offline-toggle]")!);
    await waitFor(() => expect(document.querySelector('[data-machine-id="old1"]')).not.toBeNull());
    expect(screen.getByText("Remove old computers")).toBeTruthy();

    // This Mac: its switches change here.
    await select("this-mac");
    let detail = await detailOf("Ada's MacBook Pro");
    const switches = () => [...detail.querySelectorAll<HTMLElement>("[data-machine-toggle] [role=switch]")];
    expect(switches().map((s) => s.getAttribute("aria-disabled") === "true" || s.hasAttribute("data-disabled"))).toEqual([false]);
    // One switch on this Mac's page: its desktop. "Let other computers create Spaces here" is a Settings row.
    const labels = () => [...detail.querySelectorAll("[data-machine-toggle]")].map((t) => t.textContent);
    expect(labels()).toEqual([expect.stringContaining("Let other computers control this desktop")]);
    expect(detail.textContent).not.toContain("devices");

    // Another Mac: read-only, and where to change it.
    await select("studio");
    detail = await detailOf("cua's Mac Studio");
    expect(switches().every((s) => s.hasAttribute("data-disabled"))).toBe(true);
    // Every other computer shows both switches, read-only, in the owner's words.
    expect(labels()).toEqual([
      expect.stringContaining("Let other computers create Spaces here"),
      expect.stringContaining("Let other computers control this desktop"),
    ]);
    expect(detail.querySelector("[data-machine-toggles-note]")?.textContent).toBe("Change this on that Mac.");
    // No Space actions on a machine.
    expect(within(detail).queryByRole("button", { name: /^(Open|Start|Stop|Teleport)/ })).toBeNull();

    // Details, collapsed until opened.
    expect(detail.querySelector("[data-machine-details]")).toBeNull();
    fireEvent.click(detail.querySelector("[data-machine-details-toggle]")!);
    await waitFor(() =>
      expect(document.querySelector("[data-machine-details]")?.textContent).toMatch(/OS.*macOS 26\.5\.2.*Connection.*via Cua relay.*Service.*Running/),
    );
    detail = await detailOf("cua's Mac Studio");

    // Rename saves on the relay machine.
    fireEvent.click(detail.querySelector("[data-machine-rename]")!);
    const input = await found<HTMLInputElement>("[data-machine-rename-input]");
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
    await select("studio");
    detail = await detailOf("cua's Mac Studio");
    fireEvent.click(detail.querySelector('[data-space-row="relay:studio/xm"]')!);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/spaces/${encodeURIComponent("relay:studio/xm")}`));
  });

  it("another machine's page says, read-only, how many Spaces can be created there", async () => {
    const view = await mockupView();
    const h = hostDrawn(view, []);
    const base = h.answer!;
    h.answer = (op, out) => {
      const rows = base(op, out);
      return op === "machines.list"
        ? (rows as MachineRow[]).map((m) => (m.id === "studio" ? { ...m, limits: [{ resource: "spaces", used: 2, limit: 6, reason: "" }] } : m))
        : rows;
    };
    const { router } = await mountAt("/spaces", h);
    await router.load();
    await listed();
    await select("studio");
    const detail = await detailOf("cua's Mac Studio");
    const help = detail.querySelector('[data-machine-toggle-help="spaces"]');
    expect(help?.textContent).toBe("Spaces can be created here (up to 6)");
    expect(detail.textContent).not.toContain("\u00b7");
    // Read-only: the switch cannot be flipped from here.
    expect(
      detail.querySelector('[data-machine-toggle="spaces"] [role="switch"]')?.hasAttribute("data-disabled") ||
        detail.querySelector('[data-machine-toggle="spaces"] [role="switch"]')?.getAttribute("aria-readonly") === "true",
    ).toBe(true);
  });

  it("asks before removing a machine from the account; Cancel removes nothing", async () => {
    const view = await mockupView();
    const calls: [string, unknown][] = [];
    const { router } = await mountAt("/spaces", hostDrawn(view, calls));
    await router.load();
    await listed();
    await select("byoc");
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
    const { router } = await mountAt("/spaces", hostDrawn(view, []));
    await router.load();
    await listed();
    await select("team");
    const detail = await detailOf("build-mini-01");
    expect(detail.querySelector("[data-machine-status]")?.textContent).toContain("Shared by your team");
    expect(detail.querySelector("[data-machine-toggles-note]")?.textContent).toBe("Its owner changes this on that Mac.");
    expect(detail.querySelector("[data-machine-remove]")).toBeNull();
    expect(detail.querySelector("[data-machine-rename]")).toBeNull();
  });

  it("shows a computer's issues with their fixes, and nothing for a healthy or offline computer", async () => {
    const view = await issuesView();
    const calls: [string, unknown][] = [];
    const { router } = await mountAt("/spaces", hostDrawn(view, calls));
    await router.load();
    await listed();

    // This Mac: Local Network (fail) and disk (warn), each with its fix. Its page asks for its health once it opens.
    await select("this-mac");
    let detail = await detailOf("Ada's MacBook Pro");
    await waitFor(() => expect(calls.some(([op]) => op === "machines.checkHealth")).toBe(true));
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
    await select("studio");
    detail = await detailOf("cua's Mac Studio");
    expect(issues().map((i) => i.textContent)).toEqual([
      "Running Spaces use 28 of 32 GB of memory on that Mac, so another may not start.",
      "It runs Cua 0.3.9, older than this Mac’s 0.4.1. Update Cua on it.",
    ]);
    expect(detail.querySelector("[data-machine-issue-action]")).toBeNull();

    // Healthy (the team machine) and offline (old checks not shown): no section.
    for (const [id, name] of [
      ["team", "build-mini-01"],
      ["byoc", "Cua BYOC tests"],
    ] as const) {
      await select(id);
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
    const { router } = await mountAt("/spaces/computer/this-mac", hostDrawn(view, calls));
    await router.load();
    const detail = await detailOf("Ada's MacBook Pro");
    fireEvent.click(within(detail).getByRole("button", { name: "Restart service" }));
    await waitFor(() => expect(calls.some(([op, a]) => op === "host.action" && JSON.stringify(a).includes("resume-sharing"))).toBe(true));
  });

  it("a peer on a newer Cua is a plain note when this app is the newest release: no warning, no button", async () => {
    const view = await issuesView("the Mac Studio runs a newer Cua and this app is the newest");
    const { router } = await mountAt("/spaces", hostDrawn(view, []));
    await router.load();
    await listed();
    await select("studio");
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
    const { router } = await mountAt("/spaces", h);
    await router.load();
    await listed();
    await select("studio");
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
    const { router } = await mountAt("/spaces", hostDrawn(view, []));
    await router.load();
    await listed();
    await select("studio");
    const detail = await detailOf("cua's Mac Studio");
    const rows = [...detail.querySelectorAll<HTMLElement>("[data-machine-spaces] [data-space-row]")];
    // The desktop first, then the Spaces: one row component, one look.
    expect(rows.map((r) => r.dataset.spaceRow)).toEqual([studio.desktop!.id, ...studio.spaces.map((x) => x.id)]);
    expect(rows[0]!.getAttribute("href")).toBe(`/spaces/${encodeURIComponent(studio.desktop!.id)}`);
    expect(rows[0]!.textContent).toContain("Desktop");
    expect(rows[1]!.textContent).not.toContain("Desktop");
  });

  it("a computer sharing only its desktop lists just that desktop", async () => {
    const view = await issuesView("a machine sharing only its desktop");
    const { router } = await mountAt("/spaces", hostDrawn(view, []));
    await router.load();
    await listed();
    await select("solo");
    const detail = await detailOf("solo");
    const rows = [...detail.querySelectorAll<HTMLElement>("[data-machine-spaces] [data-space-row]")];
    expect(rows.map((r) => r.dataset.spaceRow)).toEqual(["relay:solo"]);
  });

  it("draws the clouds: a group under your computers, each cloud's page, and Connect a cloud", async () => {
    const { router, hooks } = await mountAt("/spaces", { failsCreates: false, listsGhost: false, demo: { clouds: "connected" } });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.group.rows.map((r) => r.id)).toEqual(["aws", "hetzner"]));
    const group = document.querySelector("[data-cloud-group]")!;
    expect(
      [...group.querySelectorAll("[data-cloud-id]")].map((r) => [r.getAttribute("data-cloud-id"), r.querySelector("[data-cloud-line]")?.textContent]),
    ).toEqual([
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

    // Connect a cloud, under the clouds in the list: the picker, five rows, a read-only prompt.
    await act(() => router.navigate({ to: "/spaces" }));
    const connectRow = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-connect-a-cloud]");
      if (!el) throw new Error("no Connect a cloud");
      return el;
    });
    act(() => connectRow.click());
    const sheet = await waitFor(() => {
      const el = document.querySelector("[data-connect-cloud]");
      if (!el) throw new Error("no Connect a cloud sheet");
      return el;
    });
    expect([...sheet.querySelectorAll("[data-cloud-row-title]")].map((r) => r.textContent)).toEqual([
      "AWS",
      "Google Cloud",
      "Microsoft Azure",
      "Hetzner",
      "Modal",
    ]);
    expect([...sheet.querySelectorAll("[data-cloud-row-auth]")].map((r) => r.textContent)).toEqual([
      "OIDC",
      "Coming soon",
      "Coming soon",
      "API token",
      "API token",
    ]);
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
      const { router, hooks } = await mountAt("/spaces", { failsCreates: false, listsGhost: false, demo: { clouds } });
      await router.load();
      await waitFor(() => expect(hooks.machines.view?.clouds?.pages.some((p) => p.id === cloud)).toBe(true));
      await waitFor(() => expect(document.querySelector(`[data-cloud-id=${cloud}] [data-cloud-line]`)?.textContent).toBe(line));
      act(() => (document.querySelector(`[data-cloud-id=${cloud}]`) as HTMLElement).click());
      const page = await detailOf(cloud === "aws" ? "AWS" : "Hetzner");
      expect(page.querySelector("[data-cloud-status]")?.textContent).toBe(status);
      expect(page.querySelector(`[data-cloud-issue-action=${action}]`)).not.toBeNull();
      cleanup();
      resetNewSpaceSession();
    }
  });

  it("approves what an agent set up from the review sheet", async () => {
    const calls: [string, unknown][] = [];
    const { router, hooks } = await mountAt("/spaces", {
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
    const sheet = await found("[data-cloud-approval]");
    expect(sheet.textContent).toContain("Approve AWS for your agent?");
    act(() => (sheet.querySelector("[data-cloud-approve]") as HTMLElement).click());
    await waitFor(() => expect(calls).toEqual([["clouds.approve", { provider: "aws", approve: true }]]));
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.status).toBe("Connected"));
    // Let the dialog finish closing so nothing settles after the test ends.
    await waitFor(() => expect(document.querySelector("[data-cloud-approve]")).toBeNull());
  });

  it("a CLI sign-in AWS link offers Switch to OIDC, which opens AWS's sheet filled from the link", async () => {
    const { router, hooks } = await mountAt("/spaces", { failsCreates: false, listsGhost: false, demo: { clouds: "cli" } });
    await router.load();
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.switchAuth?.label).toBe("Switch to OIDC"));
    act(() => (document.querySelector("[data-cloud-id=aws]") as HTMLElement).click());
    const page = await detailOf("AWS");
    expect(page.querySelector("[data-cloud-status]")?.textContent).toBe("Connected");
    expect(page.querySelector("[data-cloud-issues]")).toBeNull();
    expect(page.querySelector("[data-cloud-setting=access]")?.textContent).toContain("The AWS CLI's sign-in on this machine");
    act(() => (page.querySelector("[data-cloud-switch=switch-oidc]") as HTMLElement).click());
    const sheet = await found("[data-connect-cloud]");
    await waitFor(() => expect(sheet.querySelector("[data-cloud-title]")?.textContent).toContain("AWS"));
    // Filled from the CLI link: its region, budget and largest machine.
    await waitFor(() => expect(sheet.textContent).toContain("8 vCPU, 32 GB"));
    expect(sheet.textContent).toContain("us-west-2");
    expect((sheet.querySelector("#cc-budget") as HTMLInputElement).value).toBe("150");
    // An OIDC link offers nothing to switch.
    cleanup();
    resetNewSpaceSession();
    const other = await mountAt("/spaces", { failsCreates: false, listsGhost: false, demo: { clouds: "connected" } });
    await other.router.load();
    await waitFor(() => expect(other.hooks.machines.view?.clouds?.pages[0]?.id).toBe("aws"));
    expect(other.hooks.machines.view?.clouds?.pages[0]?.switchAuth).toBeNull();
  });

  it("a link revoked elsewhere shows the host's sentence with Reconnect", async () => {
    const { router, hooks } = await mountAt("/spaces", { failsCreates: false, listsGhost: false, demo: { clouds: "revoked" } });
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
    const { router, hooks } = await mountAt("/spaces", {
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
    const sheet = await found("[data-cloud-approval]");
    expect(sheet.textContent).toContain("eval-runner asked to raise the limits on AWS");
    expect(sheet.textContent).toContain("$500");
    expect(sheet.textContent).toContain("8 vCPU, 32 GB");
    act(() => (sheet.querySelector("[data-cloud-deny]") as HTMLElement).click());
    await waitFor(() => expect(calls).toEqual([["clouds.approve", { provider: "aws", approve: false }]]));
    await waitFor(() => expect(hooks.machines.view?.clouds?.pages[0]?.status).toBe("Connected"));
    expect(hooks.machines.view?.clouds?.pages[0]?.settings.find((s) => s.id === "budget")?.value).toBe("$200");
    await waitFor(() => expect(document.querySelector("[data-cloud-deny]")).toBeNull());
  });
});

/**
 * A computer powered off. The host's list and view still count its relay connection (online, Remove disabled) while the
 * relay's own answer already says offline; the list and the computer's page trust the relay and re-read every 30 s.
 * A hand-made view, so these do not need the wasm core.
 */
describe("your computers when one powers off", () => {
  afterEach(() => vi.useRealTimers());

  const machine = (): MachinesView["machines"][number] =>
    ({
      id: "studio-pc",
      name: "Studio PC",
      line: "Windows · Cua relay",
      online: true,
      os: "windows",
      current: false,
      team: false,
      status: "Online",
      toggles: [],
      spacesTitle: "Spaces",
      spaces: [],
      newSpace: { label: "New Space here", on: "host:studio-pc", enabled: true },
      detailsTitle: "Details",
      details: [],
      remove: {
        label: "Remove from account",
        enabled: false,
        help: "It is online: remove it on that computer.",
        host: false,
        machine: "studio-pc",
        confirm: { title: "Remove Studio PC?", message: "", confirmLabel: "Remove", cancelLabel: "Cancel" },
      },
    }) as MachinesView["machines"][number];

  const view = (): MachinesView => ({
    summary: "1 machine, 1 online",
    groups: [{ id: "yours", title: "Your machines", collapsed: false, ids: ["studio-pc"] }],
    machines: [machine()],
    order: ["yours"],
  });

  const step = (space: string, name: string, online: boolean, role: string) => ({ space, name, online, role });

  async function mountPoweredOff(relayOnline: { value: boolean }) {
    return mountAt("/spaces", {
      failsCreates: false,
      listsGhost: false,
      // The demo's own Studio PC is replaced by this one, which the host still counts as online.
      answer: (op, out) =>
        op === "machines.list"
          ? [
              ...(out as MachineRow[]).filter((m) => m.id !== "studio-pc"),
              { id: "studio-pc", name: "Studio PC", via: "relay", online: true, os: "windows", limits: [] },
            ]
          : out,
      intercept: (op) =>
        op === "spaces.resolveDefault"
          ? {
              space: "relay:studio-pc",
              reason: "setting",
              defaultSpace: "relay:studio-pc",
              defaultOnline: relayOnline.value,
              order: [step("relay:studio-pc", "Studio PC", relayOnline.value, "default"), step("relay:this-mac", "This Mac", true, "here")],
            }
          : undefined,
      machinesView: view,
    });
  }

  const row = () => document.querySelector<HTMLElement>('[data-tree-kind="computer"][data-machine-id="studio-pc"]')!;

  it("flips the row to Offline within one refresh, with text a screen reader reads", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const relay = { value: true };
    const { router } = await mountPoweredOff(relay);
    await router.load();
    await waitFor(() => expect(row()).toBeTruthy());
    expect(row().getAttribute("data-online")).toBe("true");
    expect(within(row()).getByText(/^Online/)).toBeTruthy();

    relay.value = false;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    await waitFor(() => expect(row().getAttribute("data-online")).toBe("false"));
    expect(within(row()).getByText(/^Offline/)).toBeTruthy();
    expect(within(row()).queryByText(/^Online/)).toBeNull();
  });

  it("enables Remove from account once the relay calls the computer offline", async () => {
    const relay = { value: false };
    const { router } = await mountPoweredOff(relay);
    await router.load();
    await waitFor(() => expect(row().getAttribute("data-online")).toBe("false"));
    await select("studio-pc");
    const detail = await detailOf("Studio PC");
    const remove = detail.querySelector<HTMLButtonElement>('[data-machine-remove="studio-pc"]')!;
    expect(remove.disabled).toBe(false);
  });

  it("keeps Remove disabled while the relay still calls it online", async () => {
    const relay = { value: true };
    const { router } = await mountPoweredOff(relay);
    await router.load();
    await waitFor(() => expect(row()).toBeTruthy());
    await select("studio-pc");
    const detail = await detailOf("Studio PC");
    expect(detail.querySelector<HTMLButtonElement>('[data-machine-remove="studio-pc"]')!.disabled).toBe(true);
  });
});
