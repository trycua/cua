// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Your computers as a tree and a default Space (CUA-3413, CUA-3551) on the
// real routes and the wasm core: the only view, with no switch; the sidebar
// keeps one Computers item, and its page lists each computer with the Spaces
// made on it; rename by double-click; Remove names each Space and
// deletes them only with the box ticked; Settings has the default Space.

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { clearRetiredFlagOverrides } from "@/bridge/flags";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import type { MachineRow } from "@/bridge/contracts/host";
import type { DefaultSpace } from "@/bridge/ops/spaces-settings";
import { DOUBLE_CLICK_MS } from "@/components/spaces/spaces-tree-page";
import { resetCollapsed } from "@/components/spaces/tree-parts";
import { resetAskTasks } from "@/lib/ask-first";
import { resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetPrepare();
  resetCollapsed();
});
afterEach(() => {
  cleanup();
  resetPrepare();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
/** Waits until `s` is in the page: waitFor retries only while its callback throws. */
const found = (s: string) =>
  waitFor(() => {
    const el = q(s);
    if (!el) throw new Error(`not yet in the page: ${s}`);
    return el;
  });
type Call = [string, Record<string, unknown>];

/** The demo host with the Linux box sharing its desktop; `offline` machines are offline; `extra` machines join the
 * account; every change recorded. */
function host(calls: Call[], opts: { offline?: string[]; resolved?: DefaultSpace; extra?: MachineRow[] } = {}): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    demo: { onboarded: true, desktops: true },
    answer: (op, out) =>
      op === "machines.list" ? [...(out as MachineRow[]), ...(opts.extra ?? [])].map((m) => (opts.offline?.includes(m.id) ? { ...m, online: false } : m)) : out,
    intercept: (op, args) => {
      if (
        op === "spaces.rename" ||
        op === "machines.rename" ||
        op === "machines.remove" ||
        op === "spaces.delete" ||
        op === "spaces.setSettings" ||
        op === "spaces.create"
      ) {
        calls.push([op, args as Record<string, unknown>]);
        if (op === "spaces.setSettings") return { ...(args as object), updatedAt: 1 };
        return op === "spaces.delete" ? "" : null;
      }
      if (op === "spaces.resolveDefault" && opts.resolved) return opts.resolved;
      return undefined;
    },
  };
}

/** A double-click as a person makes it: two clicks (detail 1, then 2), then the dblclick. */
const doubleClick = (el: HTMLElement) => {
  fireEvent.click(el, { detail: 1 });
  fireEvent.click(el, { detail: 2 });
  fireEvent.doubleClick(el, { detail: 2 });
};

describe.skipIf(!wasmBuilt)("your computers as a tree", () => {
  it(
    "is the only view: no switch and no Machines in the nav, Cua Driver as a line under Computers, and an old Spaces as a tree override is gone",
    { timeout: 30000 },
    async () => {
      // A device that turned the Labs switch off before it was deleted.
      localStorage.setItem("cua.flag.spaces_tree", "0");
      clearRetiredFlagOverrides();
      expect(localStorage.getItem("cua.flag.spaces_tree")).toBeNull();
      const { router } = await mountApp("/spaces", host([]));
      await router.load();
      await waitFor(() => expect(q("[data-spaces-tree]")).not.toBeNull());
      const sidebar = q("aside[aria-label=Sidebar]")!;
      const nav = [...sidebar.querySelectorAll("nav a:not([data-sidebar-driver])")].map((a) => a.textContent ?? "");
      expect(nav[0]).toMatch(/^Home/);
      expect(nav[1]).toMatch(/^Computers/);
      expect(nav.some((t) => /Machines|Spaces|Cua Driver/.test(t))).toBe(false);
      // Cua Driver is no nav item of its own, but This Mac's setting, one line under Computers (CUA-3551).
      await waitFor(() => expect(sidebar.querySelector("nav a:nth-of-type(2) + [data-sidebar-driver]")?.textContent).toBe("Cua Driver"));
      expect(document.querySelector("h1")?.textContent).toBe("Computers");
    },
  );

  it("lists each computer with its Spaces, This Mac first, under one Computers item in the sidebar", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/spaces", host([]));
    await router.load();
    await waitFor(() => expect(q("[data-spaces-tree]")).not.toBeNull());
    expect(screen.queryByRole("link", { name: /^Machines/ })).toBeNull();
    await waitFor(() => expect(qa("[data-tree-kind=computer]").length).toBeGreaterThan(1));
    const names = qa("[data-tree-kind=computer] [data-tree-name]").map((e) => e.textContent);
    expect(names[0]).toBe("This Mac");
    expect(names).toContain("Linux box");
    // The Linux box's Spaces sit under it on the page.
    const rows = qa("[data-tree-row]").map((r) => r.getAttribute("data-tree-row"));
    expect(rows.indexOf("relay:linux-box/ubuntu-build")).toBeGreaterThan(rows.indexOf("relay:linux-box"));
    // The sidebar has no tree: one Computers item, counting the computers and Spaces the page lists.
    const sidebar = q("aside[aria-label=Sidebar]")!;
    expect(sidebar.querySelector("[data-sidebar-tree], [data-tree-row], [data-tree-default]")).toBeNull();
    expect(sidebar.textContent).not.toContain("Linux box");
    await waitFor(() => expect(within(sidebar).getByRole("link", { name: /^Computers/ }).textContent).toContain(String(rows.length)));
    // The first computer that isn't this Mac is the default until one is set.
    expect(q('[data-tree-status="relay:mac-mini"]')?.textContent).toMatch(/, Default$/);
    // The fold caret sits inside the row, in a slot of its own: no negative margin pushes it out of the row's padding.
    for (const caret of qa("[data-tree-fold]")) {
      expect(caret.className).not.toMatch(/(^|\s)-m[lrxtb]?-\d/);
      expect(caret.parentElement?.className).toContain("size-5");
      expect(caret.closest('[role="cell"]')?.contains(caret)).toBe(true);
    }
    // Folding a computer hides its Spaces.
    fireEvent.click(q('[data-tree-fold="relay:linux-box"]')!);
    expect(q('[data-tree-row="relay:linux-box/ubuntu-build"]')).toBeNull();
    // A computer that shares its desktop opens that desktop's Space page.
    fireEvent.click(q('[data-tree-name="relay:linux-box"]')!, { detail: 0 });
    await waitFor(() => expect(router.state.location.pathname).toBe("/spaces/relay%3Alinux-box"));
  });

  it("opens the computer's own page for a computer that cannot be opened as a desktop (offline)", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/spaces", host([], { offline: ["linux-box"] }));
    await router.load();
    await waitFor(() => expect(q('[data-tree-name="relay:linux-box"]')).not.toBeNull());
    fireEvent.click(q('[data-tree-name="relay:linux-box"]')!, { detail: 0 });
    await waitFor(() => expect(router.state.location.pathname).toBe("/spaces/computer/linux-box"));
  });

  it("greys out the Spaces of an offline computer: each reads Offline, with no sentence about it", { timeout: 30000 }, async () => {
    await mountApp("/spaces", host([], { offline: ["linux-box"] }));
    await waitFor(() => expect(q('[data-tree-status="relay:linux-box/ubuntu-build"]')?.textContent).toBe("Offline"));
    expect(q('[data-tree-row="relay:linux-box/ubuntu-build"]')?.hasAttribute("data-dim")).toBe(true);
    expect(document.body.textContent).not.toMatch(/Waits for|so the \d+ Spaces? on it/);
    // The tasks cell of a row with none is empty.
    expect(q('[data-tree-tasks="relay:linux-box/ubuntu-build"]')?.textContent).toBe("");
    expect(document.body.textContent).not.toMatch(/\bNone\b/);
  });

  it('folds offline computers with no Spaces on them into one "N offline" row that opens', { timeout: 30000 }, async () => {
    const old = (n: number): MachineRow => ({
      id: `old-${n}`,
      name: `Old box ${n}`,
      via: "relay",
      online: false,
      os: "linux",
      limits: [],
      lastSeen: Math.floor(Date.now() / 1000) - 40 * 24 * 3600,
    });
    await mountApp("/spaces", host([], { extra: [old(1), old(2), old(3)] }));
    const fold = await found("[data-tree-offline-fold]");
    expect(fold.textContent).toMatch(/^4 offline computers hidden\s*Show$/);
    expect(fold.textContent).not.toContain("Old box 1");
    expect(fold.querySelector("[data-tree-fold], svg")).toBeNull();
    // Hidden, there is nothing to remove from here.
    expect(q("[data-machines-remove-old]")).toBeNull();
    expect(qa("[data-tree-name]").map((e) => e.textContent)).not.toContain("Old box 1");
    // The computers with Spaces stay in the list above it.
    expect(q('[data-tree-row="relay:linux-box"]')).not.toBeNull();
    fireEvent.click(q("[data-tree-offline-toggle]")!);
    await waitFor(() => expect(qa("[data-tree-name]").map((e) => e.textContent)).toContain("Old box 1"));
    expect(q("[data-tree-offline-toggle]")?.textContent).toBe("Hide");
    expect(q("[data-tree-offline-fold]")?.textContent).toMatch(/^4 offline computers\s*Hide/);
    // Shown, "Remove old computers" sits with them; hidden again, it goes.
    await waitFor(() => expect(q("[data-tree-offline-fold] [data-machines-remove-old]")?.textContent).toBe("Remove old computers"));
    fireEvent.click(q("[data-tree-offline-toggle]")!);
    await waitFor(() => expect(q("[data-machines-remove-old]")).toBeNull());
    // Each one keeps its row menu once shown.
    expect(qa("[data-tree-menu]").length).toBe(qa("[data-tree-row]").length);
  });

  it("renames a Space by double-clicking its name: Enter saves, Esc cancels", { timeout: 30000 }, async () => {
    const calls: Call[] = [];
    const { router } = await mountApp("/spaces", host(calls));
    const name = await found('[data-tree-name="relay:linux-box/ubuntu-build"]');
    doubleClick(name);
    const input = (await found("[data-tree-rename-input]")) as HTMLInputElement;
    expect(document.body.textContent).not.toContain("Renaming changes the name only.");
    fireEvent.change(input, { target: { value: "nightly-builds" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(q("[data-tree-rename-input]")).toBeNull();
    expect(calls).toEqual([]);
    doubleClick(q('[data-tree-name="relay:linux-box/ubuntu-build"]')!);
    const again = (await found("[data-tree-rename-input]")) as HTMLInputElement;
    fireEvent.change(again, { target: { value: "nightly-builds" } });
    fireEvent.keyDown(again, { key: "Enter" });
    await waitFor(() => expect(calls).toEqual([["spaces.rename", { spaceId: "relay:linux-box/ubuntu-build", name: "nightly-builds" }]]));
    // The clicks that started each double-click never opened the Space.
    await new Promise((r) => setTimeout(r, DOUBLE_CLICK_MS + 100));
    expect(router.state.location.pathname).toBe("/spaces");
  });

  it("a single click on a name opens it once the double-click time has passed", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/spaces", host([]));
    fireEvent.click(await found('[data-tree-name="relay:linux-box/ubuntu-build"]'), { detail: 1 });
    expect(router.state.location.pathname).toBe("/spaces");
    await waitFor(() => expect(decodeURIComponent(router.state.location.pathname)).toBe("/spaces/relay:linux-box/ubuntu-build"));
  });

  it("a computer the relay calls offline is offline in the tree, and can be removed, while the machines list lags", { timeout: 30000 }, async () => {
    const resolved = {
      space: "relay:this-mac",
      reason: "fallback",
      defaultSpace: "relay:linux-box",
      defaultOnline: false,
      order: [
        { space: "relay:linux-box", name: "Linux box", online: false, role: "default" },
        { space: "relay:this-mac", name: "This Mac", online: true, role: "here" },
      ],
    };
    await mountApp("/spaces", host([], { resolved }));
    await waitFor(() => expect(q('[data-tree-status="relay:linux-box/ubuntu-build"]')?.textContent).toBe("Offline"));
    fireEvent.click(await found('[data-tree-menu="relay:linux-box"]'));
    const remove = await screen.findByRole("menuitem", { name: "Remove Linux box" });
    expect(remove.getAttribute("aria-disabled")).not.toBe("true");
    expect(remove.hasAttribute("data-disabled")).toBe(false);
  });

  it("removes a computer: names each Space, deletes nothing unless the box is ticked", { timeout: 40000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/spaces", host(calls, { offline: ["linux-box"] }));
    const open = async () => {
      fireEvent.click(await found('[data-tree-menu="relay:linux-box"]'));
      fireEvent.click(await screen.findByRole("menuitem", { name: "Remove Linux box" }));
      return found('[data-tree-remove="relay:linux-box"]');
    };
    let dialog = await open();
    expect(within(dialog).getByText("Remove Linux box?")).toBeTruthy();
    expect(q("[data-tree-remove-children]")?.textContent).toContain("ubuntu-build");
    expect(q("[data-tree-remove-children]")?.textContent).toContain("windows-legacy");
    expect(dialog.textContent).toContain("Nothing is deleted from Linux box. Add it again and all 2 come back.");
    const box = q("[data-tree-also-delete]")!;
    expect(box.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(q("[data-tree-remove-confirm]")!);
    await waitFor(() => expect(calls.filter(([op]) => op === "machines.remove")).toHaveLength(1));
    expect(calls.some(([op]) => op === "spaces.delete")).toBe(false);
    expect(calls.find(([op]) => op === "machines.remove")![1]).not.toHaveProperty("deleteSpaces", true);

    // Ticked: the computer goes with its Spaces in one call; the relay keeps them for the computer to delete when it's back.
    calls.length = 0;
    dialog = await open();
    fireEvent.click(q("[data-tree-also-delete]")!);
    expect(dialog.textContent).toContain(
      "The 2 Spaces leave your account now and won't come back. Linux box deletes them, with their files, the next time it comes online.",
    );
    expect(q("[data-tree-remove-confirm]")?.textContent).toBe("Remove and delete");
    fireEvent.click(q("[data-tree-remove-confirm]")!);
    await waitFor(() => expect(calls.map(([op]) => op)).toEqual(["machines.remove"]));
    expect(calls[0]![1]).toMatchObject({ machine: "linux-box", deleteSpaces: true });
  });
});

describe.skipIf(!wasmBuilt)("Labs and Settings with your computers as a tree", () => {
  it("Labs has no Spaces as a tree, new Home or new setup switch any more; Home's own setting stays", { timeout: 30000 }, async () => {
    await mountApp("/settings/advanced", host([]));
    await waitFor(() => expect(q('[data-setting-row="ask-first:this-mac"]')).not.toBeNull());
    for (const flag of ["spaces_tree", "spaces_ask_first_home", "spaces_new_onboarding"]) expect(q(`[data-setting-row="flag:${flag}"]`)).toBeNull();
    for (const text of ["Spaces as a tree", "Try the new Home", "Try the new setup"]) expect(document.body.textContent).not.toContain(text);
  });

  it("General has the default Space, If it's offline and the order to try", { timeout: 30000 }, async () => {
    await mountApp("/settings/general", host([]));
    const group = await found("[data-default-space-group]");
    expect(group.textContent).toContain("Default Space");
    expect(group.textContent).toContain("If it's offline");
    expect(group.textContent).toContain("Order to try");
    expect(q('[data-order-item="here"]')?.textContent).toContain("Last resort");
  });
});
