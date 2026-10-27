// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Spaces page's "⋯" and right-click menu, picking several Spaces
// (checkbox, ⌘-click, ⇧-click, ⌘A, Esc), the selection bar, and deleting or
// leaving them together with the core's confirm, count and toast (CUA-2204).

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { bulkConfirm, spaceMenu, type BulkResult, type Space, type SpaceAccess } from "@/bridge";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { BulkConfirmDialog, bulkItem } from "@/components/spaces/bulk-remove";
import type { SpaceItem } from "@/components/spaces/model";
import { menuInput } from "@/components/spaces/space-menu";
import { SelectionBar, selectable } from "@/components/spaces/selection";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const HOST = { failsCreates: false, listsGhost: false };
const GB = 1_000_000_000;
const RUNNING = "local:design-review";
const STOPPED = "local:agent-sandbox";

const access = (kind: SpaceAccess["kind"], extra: Partial<SpaceAccess> = {}): SpaceAccess => ({
  kind,
  role: kind === "mine" ? "owner" : "editor",
  ownerName: kind === "mine" ? null : "Sam",
  teamName: kind === "team" ? "QA team" : null,
  place: kind === "mine" ? "This Mac" : kind === "team" ? "QA team" : "Sam's Mac mini",
  people: [],
  canPower: true,
  canTeleport: kind !== "withMe",
  canShare: kind === "mine",
  canRename: kind === "mine",
  canDelete: kind === "mine",
  canLeave: kind !== "mine",
  ...extra,
});
const item = (name: string, kind: SpaceAccess["kind"], running: boolean, extra: Partial<SpaceAccess> = {}, bytes: number | null = null): SpaceItem => {
  const space = {
    id: kind === "mine" ? `local:${name}` : `relay:sam/${name}`,
    name,
    os: "linux",
    status: "running",
    detail: "",
    lastUsedAt: 0,
    scene: "terminal",
    provider: kind === "mine" ? "local" : "relay",
    power: { control: "stop", off: !running },
  } as unknown as Space;
  return { space, state: running ? "running" : "stopped", access: access(kind, extra), mine: kind === "mine", bytes, machine: "This Mac" };
};

const tile = (id: string) => document.querySelector<HTMLElement>(`[data-space-id="${id}"]`)!;
const picked = () => Array.from(document.querySelectorAll<HTMLElement>("[data-space-id][data-selected]")).map((e) => e.getAttribute("data-space-id"));
const bar = () => document.querySelector<HTMLElement>("[data-selection-bar]");
const menuLabels = (kind: string) =>
  Array.from(document.querySelectorAll<HTMLElement>(`[data-space-menu="${kind}"] [role="menuitem"]`)).map((e) => `${e.textContent}${e.className.includes("text-destructive") ? " (red)" : ""}`);

describe.skipIf(!wasmBuilt)("a Space's menu, per whose it is", () => {
  it("yours, shared with you, a team's", async () => {
    const core = await testCore();
    const labels = (i: SpaceItem) => spaceMenu(core, menuInput(i)).map((m) => `${m.separatorBefore ? "--- " : ""}${m.label}${m.enabled ? "" : " (disabled)"}`);
    // Rename stays out: no host renames a Space yet.
    expect(labels(item("cua-builder", "mine", true))).toEqual(["Open", "Turn off", "Share…", "Teleport an app…", "--- Delete Space"]);
    const shared = item("design-review", "mine", true, { people: [{ who: "sam@example.com", name: "Sam", role: "editor" }] });
    expect(labels(shared)).toContain("Manage sharing…");
    expect(labels(item("qa-shared", "withMe", false))).toEqual(["Open", "Turn on", "--- Leave"]);
    expect(labels(item("nightly-e2e", "team", false))).toEqual(["Open", "Turn on", "Teleport an app… (disabled)", "--- Leave"]);
    expect(labels(item("label-studio", "withMe", false, { canPower: false }))).toEqual(["Open", "--- Leave"]);
  });
});

describe("the selection bar", () => {
  const bar = (items: SpaceItem[], progress: string | null = null) =>
    render(<SelectionBar picked={items} progress={progress} onClear={() => {}} onPower={() => {}} onRemove={() => {}} />).container;
  it("counts, offers Turn off while one runs, Turn on otherwise, and Delete for yours", () => {
    let c = bar([item("a", "mine", true), item("b", "mine", false), item("c", "mine", false)]);
    expect(c.querySelector("[data-selected-count]")?.textContent).toBe("3 selected");
    expect(c.querySelector("[data-bulk-power]")?.textContent).toBe(" Turn off");
    expect(c.querySelector("[data-bulk-remove]")?.textContent).toBe("Delete");
    cleanup();
    c = bar([item("b", "mine", false)]);
    expect(c.querySelector("[data-bulk-power]")?.textContent).toBe(" Turn on");
  });
  it("Leave when only others' are picked; no power when one may not be turned on", () => {
    const c = bar([item("x", "withMe", false), item("y", "withMe", false, { canPower: false })]);
    expect(c.querySelector("[data-bulk-remove]")?.textContent).toBe("Leave");
    expect(c.querySelector("[data-bulk-remove]")?.className).not.toContain("text-destructive");
    expect(c.querySelector("[data-bulk-power]")).toBeNull();
  });
  it("mixed keeps Delete", () => {
    const c = bar([item("a", "mine", false), item("x", "withMe", false)]);
    expect(c.querySelector("[data-bulk-remove]")?.textContent).toBe("Delete");
  });
  it("counts through while it runs", () => {
    const c = bar([item("a", "mine", false)], "Deleting 1 of 3…");
    expect(c.querySelector("[data-bulk-progress]")?.textContent).toBe("Deleting 1 of 3…");
    expect(c.querySelector("[data-bulk-remove]")).toBeNull();
  });
  it("others' Spaces can be picked to leave; one being created can't", () => {
    expect(selectable(item("x", "withMe", false))).toBe(true);
    expect(selectable(item("x", "withMe", false, { canLeave: false }))).toBe(false);
    expect(selectable({ ...item("a", "mine", false), state: "creating" })).toBe(false);
  });
});

describe.skipIf(!wasmBuilt)("the bulk confirm", () => {
  it("draws the core's view: groups, sizes, who loses access, Frees, the red button", async () => {
    const core = await testCore();
    const items = [
      item("cua-builder", "mine", true, { people: [{ who: "sam@example.com", name: "Sam", role: "editor" }] }, 14 * GB),
      item("onb-new", "mine", false, {}, 9 * GB),
      item("qa-shared", "withMe", false),
    ];
    const view = bulkConfirm(core, items.map(bulkItem))!;
    const { getByText } = render(<BulkConfirmDialog view={view} items={items} onCancel={() => {}} onConfirm={() => {}} />);
    expect(getByText("Delete 2 Spaces and leave 1?")).toBeTruthy();
    const dialog = document.querySelector<HTMLElement>("[data-bulk-confirm]")!;
    expect(dialog.querySelector("[data-bulk-body]")?.textContent).toContain("Sam loses access");
    expect(Array.from(dialog.querySelectorAll("[data-bulk-group]")).map((g) => g.getAttribute("data-bulk-group"))).toEqual(["Delete", "Leave"]);
    expect(dialog.querySelector('[data-bulk-row="local:cua-builder"]')?.textContent).toContain("14 GB");
    expect(dialog.querySelector('[data-bulk-row="relay:sam/qa-shared"]')?.textContent).toContain("Sam's Mac mini");
    // Someone else's row shows its owner's avatar; yours, the OS.
    expect(dialog.querySelector('[data-bulk-row="relay:sam/qa-shared"] [data-avatar="withMe"]')?.textContent).toBe("S");
    expect(dialog.querySelector('[data-bulk-row="local:cua-builder"] [data-avatar]')).toBeNull();
    expect(dialog.querySelector("[data-frees]")?.textContent).toBe("Frees 23 GB");
    expect(dialog.querySelector("[data-confirm-bulk]")?.className).toContain("bg-destructive");
  });
  it("Leave alone is a plain button", async () => {
    const core = await testCore();
    const view = bulkConfirm(core, [item("qa-shared", "withMe", false), item("label-studio", "withMe", false)].map(bulkItem))!;
    render(<BulkConfirmDialog view={view} onCancel={() => {}} onConfirm={() => {}} />);
    const button = document.querySelector<HTMLElement>("[data-confirm-bulk]")!;
    expect(button.textContent).toBe("Leave");
    expect(button.className).not.toContain("bg-destructive");
    expect(document.querySelector("[data-frees]")).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("on the page", () => {
  it("the ⋯ menu and right-click menu, and Delete Space asks with one Space", async () => {
    await mountApp("/spaces", HOST);
    await waitFor(() => expect(tile(RUNNING)).not.toBeNull());
    fireEvent.click(document.querySelector(`[data-space-more="${RUNNING}"]`)!);
    // Shared with five people: Manage sharing…; Rename stays out (no host renames a Space yet).
    await waitFor(() => expect(menuLabels("more")).toEqual(["Open", "Turn off", "Manage sharing…", "Teleport an app…", "Delete Space (red)"]));
    fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="delete"]')!);
    const dialog = await waitFor(() => document.querySelector<HTMLElement>("[data-bulk-confirm]") ?? Promise.reject(new Error("no confirm")));
    expect(dialog.textContent).toContain("Delete design-review?");
    expect(dialog.textContent).toContain("will be turned off first");
    expect(dialog.textContent).toMatch(/lose access/);
    fireEvent.click(Array.from(dialog.querySelectorAll("button")).find((b) => b.textContent === "Cancel")!);
    await waitFor(() => expect(document.querySelector("[data-bulk-confirm]")).toBeNull());

    fireEvent.contextMenu(tile(STOPPED));
    await waitFor(() => expect(menuLabels("context")).toEqual(["Open", "Turn on", "Share…", "Teleport an app…", "Delete Space (red)"]));
  });

  it("checkbox, ⌘-click and ⇧-click pick without opening; ⌘A picks every visible one; Esc clears", async () => {
    const { router } = await mountApp("/spaces?view=list&sort=name", HOST);
    await waitFor(() => expect(document.querySelectorAll("[data-space-row]").length).toBeGreaterThan(3));
    const order = Array.from(document.querySelectorAll("[data-space-row]")).map((r) => r.getAttribute("data-space-row")!);
    fireEvent.click(document.querySelector(`[data-select="${order[0]}"]`)!);
    await waitFor(() => expect(bar()?.textContent).toContain("1 selected"));
    // ⇧: from the last one picked to this one.
    fireEvent.click(document.querySelector(`[data-space-row="${order[2]}"]`)!, { shiftKey: true });
    await waitFor(() => expect(bar()?.textContent).toContain("3 selected"));
    expect(router.state.location.pathname).toBe("/spaces");
    // ⌘ toggles one off.
    fireEvent.click(document.querySelector(`[data-space-row="${order[1]}"]`)!, { metaKey: true });
    await waitFor(() => expect(bar()?.textContent).toContain("2 selected"));
    expect(Array.from(document.querySelectorAll("[data-space-row][data-selected]")).map((r) => r.getAttribute("data-space-row"))).toEqual([order[0], order[2]]);
    fireEvent.keyDown(window, { key: "a", metaKey: true });
    await waitFor(() => expect(bar()?.textContent).toContain(`${order.length} selected`));
    expect(document.querySelector("[data-select-all]")?.getAttribute("aria-checked")).toBe("true");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(bar()).toBeNull());
    expect(document.querySelector("[data-spaces-toolbar]")).not.toBeNull();
  });

  it("⌘-click on a tile picks it instead of opening it", async () => {
    const { router } = await mountApp("/spaces", HOST);
    await waitFor(() => expect(tile(STOPPED)).not.toBeNull());
    fireEvent.click(tile(STOPPED), { metaKey: true });
    await waitFor(() => expect(picked()).toEqual([STOPPED]));
    expect(router.state.location.pathname).toBe("/spaces");
    expect(bar()?.querySelector("[data-bulk-power]")?.textContent).toBe(" Turn on");
    fireEvent.click(bar()!.querySelector("[data-clear-selection]")!);
    await waitFor(() => expect(picked()).toEqual([]));
  });

  it("confirm deletes the picked ones with one call, counts through, then says what was freed", async () => {
    let release!: (r: { results: BulkResult[] }) => void;
    const calls: unknown[] = [];
    await mountApp("/spaces", {
      ...HOST,
      intercept: (op, args) => {
        if (op !== "spaces.deleteMany") return undefined;
        calls.push(args);
        return new Promise((r) => (release = r));
      },
    });
    await waitFor(() => expect(tile(STOPPED)).not.toBeNull());
    fireEvent.click(tile(STOPPED), { metaKey: true });
    fireEvent.click(tile(RUNNING), { metaKey: true });
    fireEvent.click(await waitFor(() => bar()!.querySelector<HTMLElement>("[data-bulk-remove]")!));
    const confirm = await waitFor(() => document.querySelector<HTMLElement>("[data-confirm-bulk]") ?? Promise.reject(new Error("no confirm")));
    expect(document.querySelector("[data-bulk-confirm]")?.textContent).toContain("Delete 2 Spaces?");
    fireEvent.click(confirm);
    await waitFor(() => expect(calls).toEqual([{ ids: [RUNNING, STOPPED] }]));
    await waitFor(() => expect(bar()?.querySelector("[data-bulk-progress]")?.textContent).toBe("Deleting 1 of 2…"));
    expect(tile(STOPPED).parentElement!.querySelector("[data-tile-busy]")).not.toBeNull();
    await act(async () => release({ results: [{ id: RUNNING, ok: true }, { id: STOPPED, ok: true }] }));
    await waitFor(() => expect(document.body.textContent).toMatch(/Deleted 2 Spaces\. \d+ GB freed\./));
    await waitFor(() => expect(bar()).toBeNull());
  });

  it("a partial failure says which one and why", async () => {
    await mountApp("/spaces", {
      ...HOST,
      intercept: (op, args) => (op === "spaces.deleteMany" ? { results: (args as { ids: string[] }).ids.map((id, i) => ({ id, ok: i === 0, error: i === 0 ? undefined : "the machine did not answer" })) } : undefined),
    });
    await waitFor(() => expect(tile(STOPPED)).not.toBeNull());
    fireEvent.click(tile(STOPPED), { metaKey: true });
    fireEvent.click(tile(RUNNING), { metaKey: true });
    fireEvent.click(await waitFor(() => bar()!.querySelector<HTMLElement>("[data-bulk-remove]")!));
    fireEvent.click(await waitFor(() => document.querySelector<HTMLElement>("[data-confirm-bulk]") ?? Promise.reject(new Error("no confirm"))));
    await waitFor(() => expect(document.body.textContent).toContain("the machine did not answer"));
    expect(document.body.textContent).toMatch(/Deleted 1 of 2|Couldn't delete/);
  });

  it("cleanup: the disk alert's list for This Mac, biggest first, Select stopped picks only the stopped ones", async () => {
    const { router } = await mountApp("/spaces?view=list&on=this-mac&sort=size", HOST);
    const stoppedButton = await waitFor(() => document.querySelector<HTMLElement>("[data-select-stopped]") ?? Promise.reject(new Error("no Select stopped")));
    fireEvent.click(stoppedButton);
    await waitFor(() => expect(bar()?.textContent).toContain("1 selected"));
    expect(Array.from(document.querySelectorAll("[data-space-row][data-selected]")).map((r) => r.getAttribute("data-space-row"))).toEqual([STOPPED]);
    expect(router.state.location.search).toEqual({ view: "list", on: "this-mac", sort: "size" });
  });

  it("?select=stopped picks them when the page opens, then leaves the URL", async () => {
    const { router } = await mountApp("/spaces?view=list&on=this-mac&sort=size&select=stopped", HOST);
    await waitFor(() => expect(bar()?.textContent).toContain("1 selected"));
    await waitFor(() => expect(router.state.location.search).toEqual({ view: "list", on: "this-mac", sort: "size" }));
  });

  it("Select stopped shows only in the list", async () => {
    await mountApp("/spaces", HOST);
    await waitFor(() => expect(tile(STOPPED)).not.toBeNull());
    expect(document.querySelector("[data-select-stopped]")).toBeNull();
  });

  it("someone else's Space: only what they allow, then Leave; leaving calls the host's leave", async () => {
    const calls: unknown[] = [];
    await mountApp("/spaces", {
      ...HOST,
      intercept: (op, args) => {
        if (op !== "spaces.leave") return undefined;
        calls.push(args);
        return { results: (args as { ids: string[] }).ids.map((id) => ({ id, ok: true })) };
      },
    });
    const SAM = "relay:space-sam-qa-shared";
    await waitFor(() => expect(tile(SAM)).not.toBeNull());
    fireEvent.click(document.querySelector(`[data-space-more="${SAM}"]`)!);
    await waitFor(() => expect(menuLabels("more").at(-1)).toBe("Leave"));
    expect(menuLabels("more")).not.toContain("Delete Space (red)");
    expect(menuLabels("more")).not.toContain("Share…");
    fireEvent.click(document.querySelector('[data-space-menu="more"] [data-menu-item="leave"]')!);
    const dialog = await waitFor(() => document.querySelector<HTMLElement>("[data-bulk-confirm]") ?? Promise.reject(new Error("no confirm")));
    expect(dialog.querySelector("[data-confirm-bulk]")?.className).not.toContain("bg-destructive");
    fireEvent.click(dialog.querySelector("[data-confirm-bulk]")!);
    await waitFor(() => expect(calls).toEqual([{ ids: [SAM] }]));
  });

  it("yours and someone else's together: one confirm that deletes and leaves", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/spaces", {
      ...HOST,
      intercept: (op, args) => {
        if (op !== "spaces.leave" && op !== "spaces.deleteMany") return undefined;
        calls.push([op, args]);
        return { results: (args as { ids: string[] }).ids.map((id) => ({ id, ok: true })) };
      },
    });
    const SAM = "relay:space-sam-qa-shared";
    await waitFor(() => expect(tile(SAM)).not.toBeNull());
    fireEvent.click(tile(STOPPED), { metaKey: true });
    fireEvent.click(tile(SAM), { metaKey: true });
    // Mixed keeps Delete in the bar; the confirm splits it.
    fireEvent.click(await waitFor(() => bar()!.querySelector<HTMLElement>('[data-bulk-remove="delete"]')!));
    const dialog = await waitFor(() => document.querySelector<HTMLElement>("[data-bulk-confirm]") ?? Promise.reject(new Error("no confirm")));
    expect(dialog.textContent).toContain("Delete 1 Space and leave 1?");
    expect(Array.from(dialog.querySelectorAll("[data-bulk-group]")).map((g) => g.getAttribute("data-bulk-group"))).toEqual(["Delete", "Leave"]);
    fireEvent.click(dialog.querySelector("[data-confirm-bulk]")!);
    await waitFor(() => expect(calls.map(([op]) => op).sort()).toEqual(["spaces.deleteMany", "spaces.leave"]));
    expect(calls).toContainEqual(["spaces.deleteMany", { ids: [STOPPED] }]);
    expect(calls).toContainEqual(["spaces.leave", { ids: [SAM] }]);
  });
});
