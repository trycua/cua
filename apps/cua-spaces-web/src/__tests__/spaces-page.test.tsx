// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Spaces page (CUA-2204) on the real routes, the wasm core and the demo
// host: tiles with their corner badge, the one toolbar row (state chips,
// computer, sort, grid or list) kept in the URL, and the list view.

import { cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const HOST = { failsCreates: false, listsGhost: false };
const tiles = () => Array.from(document.querySelectorAll<HTMLElement>("[data-space-id]")).map((t) => t.getAttribute("data-space-id"));
const rows = () => Array.from(document.querySelectorAll<HTMLElement>("[data-space-row]")).map((t) => t.getAttribute("data-space-row"));
const tile = (id: string) => document.querySelector<HTMLElement>(`[data-space-id="${id}"]`)!.parentElement!;
const stateOf = (id: string) => tile(id).querySelector("[data-state-label]")?.textContent;
const nameRow = (id: string) => tile(id).querySelector("[data-space-name]")!.parentElement!;

async function pickMenu(menu: string, label: string) {
  fireEvent.click(document.querySelector(`[data-toolbar-menu="${menu}"]`)!);
  const item = await waitFor(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>(`[data-toolbar-popup="${menu}"] [role="menuitem"]`)).find((e) => e.textContent === label);
    if (!el) throw new Error(`no ${label}`);
    return el;
  });
  fireEvent.click(item);
}

describe.skipIf(!wasmBuilt)("the Spaces page", () => {
  it("tiles: the state as a dot and word on the name row, no corner badge, the computer as the meta line", async () => {
    await mountApp("/spaces", HOST);
    await waitFor(() => expect(stateOf("local:agent-sandbox")).toBe("Stopped"));
    expect(stateOf("local:design-review")).toBe("Running");
    // On the name row, not over the picture, and not in the meta line.
    expect(nameRow("local:design-review").querySelector("[data-state-label]")).not.toBeNull();
    expect(tile("local:design-review").querySelector("[data-badge]")).toBeNull();
    expect(tile("local:design-review").textContent).toContain("This Mac");
    expect(tile("relay:linux-box/ubuntu-build").textContent).toContain("Linux box");
    // The OS mark as the avatar; no outline around the tile.
    expect(tile("local:design-review").querySelector('[data-avatar="mine"] svg[aria-label="macOS"]')).not.toBeNull();
    expect(document.querySelector("[data-space-id]")!.className).not.toMatch(/\bborder\b/);
  });

  it("the state chips, the computer menu and the sort live in the URL", async () => {
    const { router } = await mountApp("/spaces", HOST);
    await waitFor(() => expect(tiles()).toContain("local:agent-sandbox"));
    fireEvent.click(document.querySelector('[data-chip="stopped"]')!);
    await waitFor(() => expect(router.state.location.search).toEqual({ state: "stopped" }));
    await waitFor(() => expect(tiles().every((id) => tile(id!).querySelector("[data-space-id]")!.getAttribute("data-state") === "stopped")).toBe(true));
    expect(document.querySelector('[data-chip="stopped"]')!.getAttribute("aria-pressed")).toBe("true");

    await pickMenu("computer", "This Mac");
    await waitFor(() => expect(router.state.location.search).toEqual({ state: "stopped", on: "this-mac" }));
    await waitFor(() => expect(tiles()).toEqual(["local:agent-sandbox"]));

    fireEvent.click(document.querySelector('[data-chip="all"]')!);
    await pickMenu("sort", "Name");
    await waitFor(() => expect(router.state.location.search).toEqual({ on: "this-mac", sort: "name" }));
    await waitFor(() => expect(tiles()).toEqual(["local:agent-sandbox", "local:design-review"]));
    await pickMenu("computer", "All machines");
    await waitFor(() => expect(router.state.location.search).toEqual({ sort: "name" }));
  });

  it("the owner menu: Anyone, Mine, Shared with me; others' Spaces show their owner, whose computer and their state", async () => {
    const { router } = await mountApp("/spaces", HOST);
    await waitFor(() => expect(tiles()).toContain("relay:space-sam-qa-shared"));
    const all = tiles().length;
    await pickMenu("owner", "Mine");
    await waitFor(() => expect(router.state.location.search).toEqual({ owner: "mine" }));
    await waitFor(() => expect(tiles()).not.toContain("relay:space-sam-qa-shared"));
    const mine = tiles().length;
    await pickMenu("owner", "Shared with me");
    await waitFor(() => expect(router.state.location.search).toEqual({ owner: "shared" }));
    await waitFor(() => expect(tiles().sort()).toEqual(["relay:space-priya-checkout", "relay:space-qa-regression", "relay:space-sam-qa-shared"]));
    expect(mine + 3).toBe(all);
    // The owner's initial in place of the OS; the team's for a team's Space; whose computer as the meta line.
    expect(tile("relay:space-sam-qa-shared").querySelector('[data-avatar="withMe"]')?.textContent).toBe("S");
    expect(tile("relay:space-qa-regression").querySelector('[data-avatar="team"]')?.textContent).toBe("Q");
    expect(tile("relay:space-qa-regression").textContent).toContain("QA team");
    // Not on your disk: the state, never a size.
    expect(stateOf("relay:space-priya-checkout")).toBe("Stopped");
  });

  it("a Space you shared shows who has it: three, then +2", async () => {
    await mountApp("/spaces", HOST);
    const stack = await waitFor(() => tile("local:design-review").querySelector<HTMLElement>("[data-people]") ?? Promise.reject(new Error("no stack")));
    expect(stack.getAttribute("data-people")).toBe("5");
    expect(stack.textContent).toMatch(/\+2$/);
  });

  it("the list: status, computer, size, last used; Name, Size and Last used sort from their headers", async () => {
    const { router } = await mountApp("/spaces?view=list", HOST);
    await waitFor(() => expect(rows().length).toBeGreaterThan(3));
    const row = document.querySelector<HTMLElement>('[data-space-row="local:agent-sandbox"]')!;
    expect(row.textContent).toContain("Stopped");
    expect(row.textContent).toContain("This Mac");
    expect(row.querySelector("[data-size-cell]")?.textContent).toBe("6 GB");
    expect(row.querySelector("[data-last-used]")?.textContent).toMatch(/ago$|Just now|Yesterday/);

    fireEvent.click(document.querySelector('[data-sort-header="size"]')!);
    await waitFor(() => expect(router.state.location.search).toEqual({ view: "list", sort: "size" }));
    // Biggest first; the ones without a known size ("—") last.
    const gb = () => Array.from(document.querySelectorAll("[data-size-cell]")).map((c) => (c.textContent === "—" ? -1 : parseFloat(c.textContent!)));
    await waitFor(() => expect(gb()).toEqual([...gb()].sort((a, b) => b - a)));
    expect(gb()[0]).toBeGreaterThan(0);
    fireEvent.click(document.querySelector('[data-sort-header="name"]')!);
    const names = () => Array.from(document.querySelectorAll("[data-space-row] [data-space-id]")).map((a) => a.textContent ?? "");
    await waitFor(() => expect(names()).toEqual([...names()].sort((a, b) => a.localeCompare(b))));
    fireEvent.click(document.querySelector('[data-sort-header="used"]')!);
    await waitFor(() => expect(router.state.location.search).toEqual({ view: "list" }));

    // Back to the grid.
    fireEvent.click(document.querySelector('[data-view="grid"]')!);
    await waitFor(() => expect(router.state.location.search).toEqual({}));
    await waitFor(() => expect(document.querySelector("[data-space-list]")).toBeNull());
  });

  it("a row opens its Space", async () => {
    const { router } = await mountApp("/spaces?view=list", HOST);
    await waitFor(() => expect(rows()).toContain("local:agent-sandbox"));
    fireEvent.click(document.querySelector('[data-space-row="local:agent-sandbox"]')!);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/spaces/${encodeURIComponent("local:agent-sandbox")}`));
  });

  it("?sort=size alone puts the grid's biggest first", async () => {
    await mountApp("/spaces?sort=size", HOST);
    await waitFor(() => expect(tiles()[0]).toBe("relay:linux-box/windows-legacy"));
  });

  it("in order: the computers' warning, then the toolbar, then the grid; with no Spaces, the warning then the first-run guide and no toolbar", async () => {
    const before = (a: Element, b: Element) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    for (const noSpaces of [true, false]) {
      await mountApp("/spaces", { ...HOST, demo: { noSpaces, disk: "low" } });
      const warning = await waitFor(() => document.querySelector("[data-computer-warnings]") ?? Promise.reject(new Error("no warning")));
      const area = await waitFor(() => document.querySelector(noSpaces ? "[data-empty-home]" : "[data-space-id]") ?? Promise.reject(new Error("no grid area")));
      const toolbar = document.querySelector("[data-spaces-toolbar]");
      expect(before(warning, area)).toBe(true);
      if (noSpaces) expect(toolbar).toBeNull();
      else {
        expect(before(warning, toolbar!)).toBe(true);
        expect(before(toolbar!, area)).toBe(true);
      }
      cleanup();
    }
  });
});
