// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Spaces as a tree and a default Space (CUA-3413, Labs `spaces_tree`) on the
// real routes and the wasm core: off, the app is as it was; on, Machines
// leaves the nav and the sidebar and the Spaces page list each computer with
// the Spaces made on it; rename by double-click; Remove names each Space and
// deletes them only with the box ticked; Home starts on the default Space,
// says why when it falls back, and never makes a computer by itself.

import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
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

/** The demo host with the Linux box sharing its desktop; `offline` machines are offline; every change recorded. */
function host(calls: Call[], opts: { offline?: string[]; resolved?: DefaultSpace } = {}): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    demo: { onboarded: true, desktops: true },
    answer: (op, out) => (op === "machines.list" ? (out as MachineRow[]).map((m) => (opts.offline?.includes(m.id) ? { ...m, online: false } : m)) : out),
    intercept: (op, args) => {
      if (op === "spaces.rename" || op === "machines.rename" || op === "machines.remove" || op === "spaces.delete" || op === "spaces.setSettings" || op === "spaces.create") {
        calls.push([op, args as Record<string, unknown>]);
        if (op === "spaces.setSettings") return { ...(args as object), updatedAt: 1 };
        return op === "spaces.delete" ? "" : null;
      }
      if (op === "spaces.resolveDefault" && opts.resolved) return opts.resolved;
      return undefined;
    },
  };
}

/** The switch on, with the "Ask first" Home it changes (Labs turns both on). */
/** A double-click as a person makes it: two clicks (detail 1, then 2), then the dblclick. */
const doubleClick = (el: HTMLElement) => {
  fireEvent.click(el, { detail: 1 });
  fireEvent.click(el, { detail: 2 });
  fireEvent.doubleClick(el, { detail: 2 });
};

const on = () => {
  setFlagOverride("spaces_tree", true);
  setFlagOverride("spaces_ask_first_home", true);
};

describe.skipIf(!wasmBuilt)("Spaces as a tree (Labs)", () => {
  it("off: the nav keeps Machines and the Spaces page is the grid", { timeout: 30000 }, async () => {
    setFlagOverride("spaces_ask_first_home", true);
    const { router } = await mountApp("/spaces", host([]));
    await router.load();
    await waitFor(() => expect(screen.getByRole("link", { name: /Machines/ })).toBeTruthy());
    expect(q("[data-sidebar-tree]")).toBeNull();
    expect(q("[data-spaces-tree]")).toBeNull();
  });

  it("on: Machines leaves the nav, and each computer lists the Spaces made on it, This Mac first", { timeout: 30000 }, async () => {
    on();
    const { router } = await mountApp("/spaces", host([]));
    await router.load();
    await waitFor(() => expect(q("[data-spaces-tree]")).not.toBeNull());
    expect(screen.queryByRole("link", { name: /^Machines/ })).toBeNull();
    await waitFor(() => expect(qa("[data-tree-kind=computer]").length).toBeGreaterThan(1));
    const names = qa("[data-tree-kind=computer] [data-tree-name]").map((e) => e.textContent);
    expect(names[0]).toBe("This Mac");
    expect(names).toContain("Linux box");
    // The Linux box's Spaces sit under it, in the sidebar too.
    const rows = qa("[data-tree-row]").map((r) => r.getAttribute("data-tree-row"));
    expect(rows.indexOf("relay:linux-box/ubuntu-build")).toBeGreaterThan(rows.indexOf("relay:linux-box"));
    expect(q('[data-sidebar-tree-row="relay:linux-box/ubuntu-build"]')).not.toBeNull();
    // The first computer that isn't this Mac is the default until one is set.
    expect(q('[data-tree-status="relay:mac-mini"]')?.textContent).toMatch(/, Default$/);
    expect(q("[data-sidebar-tree] [data-tree-default]")).not.toBeNull();
    // Folding a computer hides its Spaces in both lists.
    fireEvent.click(q('[data-tree-fold="relay:linux-box"]')!);
    expect(q('[data-tree-row="relay:linux-box/ubuntu-build"]')).toBeNull();
    expect(q('[data-sidebar-tree-row="relay:linux-box/ubuntu-build"]')).toBeNull();
    // Machines' page still opens.
    await act(() => router.navigate({ to: "/machines" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/machines"));
  });

  it("greys out the Spaces of an offline computer: they wait for it", { timeout: 30000 }, async () => {
    on();
    await mountApp("/spaces", host([], { offline: ["linux-box"] }));
    await waitFor(() => expect(q('[data-tree-status="relay:linux-box/ubuntu-build"]')?.textContent).toBe("Waits for Linux box"));
    expect(q('[data-tree-row="relay:linux-box/ubuntu-build"]')?.hasAttribute("data-dim")).toBe(true);
    expect(document.body.textContent).toContain("Linux box is offline, so the 2 Spaces on it are too. They come back when it does.");
  });

  it("renames a Space by double-clicking its name: Enter saves, Esc cancels", { timeout: 30000 }, async () => {
    on();
    const calls: Call[] = [];
    const { router } = await mountApp("/spaces", host(calls));
    const name = await found('[data-tree-name="relay:linux-box/ubuntu-build"]');
    doubleClick(name);
    const input = (await found("[data-tree-rename-input]")) as HTMLInputElement;
    expect(document.body.textContent).toContain("Renaming changes the name only.");
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
    on();
    const { router } = await mountApp("/spaces", host([]));
    fireEvent.click(await found('[data-tree-name="relay:linux-box/ubuntu-build"]'), { detail: 1 });
    expect(router.state.location.pathname).toBe("/spaces");
    await waitFor(() => expect(decodeURIComponent(router.state.location.pathname)).toBe("/spaces/relay:linux-box/ubuntu-build"));
  });

  it("a computer the relay calls offline is offline in the tree, and can be removed, while the machines list lags", { timeout: 30000 }, async () => {
    on();
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
    await waitFor(() => expect(q('[data-tree-status="relay:linux-box/ubuntu-build"]')?.textContent).toBe("Waits for Linux box"));
    fireEvent.click(await found('[data-tree-menu="relay:linux-box"]'));
    const remove = await screen.findByRole("menuitem", { name: "Remove Linux box" });
    expect(remove.getAttribute("aria-disabled")).not.toBe("true");
    expect(remove.hasAttribute("data-disabled")).toBe(false);
  });

  it("removes a computer: names each Space, deletes nothing unless the box is ticked", { timeout: 40000 }, async () => {
    on();
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
    expect(dialog.textContent).toContain("The 2 Spaces leave your account now and won't come back. Linux box deletes them, with their files, the next time it comes online.");
    expect(q("[data-tree-remove-confirm]")?.textContent).toBe("Remove and delete");
    fireEvent.click(q("[data-tree-remove-confirm]")!);
    await waitFor(() => expect(calls.map(([op]) => op)).toEqual(["machines.remove"]));
    expect(calls[0]![1]).toMatchObject({ machine: "linux-box", deleteSpaces: true });
  });
});

describe.skipIf(!wasmBuilt)("Home with Spaces as a tree (Labs)", () => {
  const step = (space: string, name: string, online: boolean, role: string) => ({ space, name, online, role });

  it("starts on the default Space, marked Default, with no line under the box", { timeout: 30000 }, async () => {
    on();
    const resolved = { space: "relay:linux-box", reason: "default", defaultSpace: "relay:linux-box", defaultOnline: true, order: [step("relay:linux-box", "Linux box", true, "default"), step("relay:this-mac", "This Mac", true, "here")] };
    await mountApp("/home", host([], { resolved }));
    await waitFor(() => expect(q("[data-ask-where]")?.textContent).toBe("Linux boxDefault"));
    expect(q("[data-tree-fallback]")).toBeNull();
  });

  it("the demo's computers share their desktops, so Home starts on the Mac mini when it is the default", { timeout: 30000 }, async () => {
    on();
    await mountApp("/home", host([]));
    await waitFor(() => expect(q("[data-ask-where]")?.textContent).toBe("Mac miniDefault"));
    expect(q("[data-tree-fallback]")).toBeNull();
  });

  it("with the default offline, the next online computer is the one used, never contradicted by the line", { timeout: 30000 }, async () => {
    on();
    const resolved = {
      space: "relay:mac-mini",
      reason: "fallback",
      defaultSpace: "relay:linux-box",
      defaultOnline: false,
      order: [step("relay:linux-box", "Linux box", false, "default"), step("relay:mac-mini", "Mac mini", true, "fallback"), step("relay:this-mac", "This Mac", true, "here")],
    };
    await mountApp("/home", host([], { offline: ["linux-box"], resolved }));
    await waitFor(() => expect(q("[data-ask-where]")?.textContent).toBe("Mac mini"));
    const line = await found("[data-tree-fallback]");
    expect(line.textContent).toContain("Linux box, your default, is offline. This runs on Mac mini, the next computer that can take it. If none can, it runs on This Mac.");
    expect(line.textContent).not.toContain("no other computer is online");
    expect(qa("[data-tree-fallback-step]").map((s) => s.textContent)).toEqual(["1Linux boxYour default. Offline now", "2Mac miniNext online computer. Used now", "3This MacLast resort"]);
  });

  it("falls back to the next online computer when the default is offline, and says why", { timeout: 30000 }, async () => {
    on();
    const resolved = {
      space: "relay:linux-box",
      reason: "fallback",
      defaultSpace: "relay:mac-mini",
      defaultOnline: false,
      order: [step("relay:mac-mini", "Mac mini", false, "default"), step("relay:linux-box", "Linux box", true, "fallback"), step("relay:this-mac", "This Mac", true, "here")],
    };
    await mountApp("/home", host([], { offline: ["mac-mini"], resolved }));
    await waitFor(() => expect(q("[data-ask-where]")?.textContent).toBe("Linux box"));
    const line = await found("[data-tree-fallback]");
    expect(line.textContent).toContain("Mac mini, your default, is offline. This runs on Linux box, the next computer that can take it. If none can, it runs on This Mac.");
    expect(qa("[data-tree-fallback-step]").map((s) => s.textContent)).toEqual(["1Mac miniYour default. Offline now", "2Linux boxNext online computer. Used now", "3This MacLast resort"]);
    expect(line.textContent).toContain("A new computer is never made unless you pick one.");
  });

  it("with nothing else online runs on This Mac, and never makes a computer by itself", { timeout: 30000 }, async () => {
    on();
    const calls: Call[] = [];
    const resolved = { space: null, reason: "here", defaultSpace: "relay:mac-mini", defaultOnline: false, order: [step("relay:mac-mini", "Mac mini", false, "default"), step("relay:this-mac", "This Mac", true, "here")] };
    await mountApp("/home", host(calls, { offline: ["mac-mini", "linux-box"], resolved }));
    await waitFor(() => expect(q("[data-ask-where]")?.textContent).toBe("This Mac"));
    expect(q("[data-tree-fallback]")?.textContent).toContain("Mac mini, your default, is offline, and no other computer is online. This runs on This Mac.");
    expect(q("[data-ask-where]")?.textContent).not.toMatch(/New Space/);
    expect(calls.some(([op]) => op === "spaces.create")).toBe(false);
  });

  it("the picker lists the default first, then the tree, then New Space on each computer; Make this my default saves it", { timeout: 30000 }, async () => {
    on();
    const calls: Call[] = [];
    await mountApp("/home", host(calls));
    fireEvent.click(await found('[data-picker="where"]'));
    await waitFor(() => expect(q("[data-tree-where]")).not.toBeNull());
    expect(q("[data-tree-where-current]")).not.toBeNull();
    expect(q('[data-tree-where-row="relay:linux-box/ubuntu-build"]')).not.toBeNull();
    expect(q("[data-tree-new-here]")?.textContent).toContain("New Space on This Mac");
    expect(q('[data-tree-new-on="linux-box"]')?.textContent).toContain("New Space on Linux box");
    fireEvent.click(q('[data-tree-make-default="relay:linux-box"]')!);
    await waitFor(() => expect(calls).toContainEqual(["spaces.setSettings", { defaultSpace: "relay:linux-box", fallback: [], fallbackOn: true }]));
  });
});

describe.skipIf(!wasmBuilt)("Labs and Settings with Spaces as a tree", () => {
  it("the Labs switch is off by default and turns on the new Home with it", { timeout: 30000 }, async () => {
    await mountApp("/settings/advanced", host([]));
    const sw = () => q('[data-setting-row="flag:spaces_tree"] [role=switch]')!;
    await waitFor(() => expect(sw()).toBeTruthy());
    expect(sw().getAttribute("aria-checked")).toBe("false");
    expect(q('[data-setting-row="flag:spaces_tree"]')?.textContent).toContain("Spaces as a tree");
    fireEvent.click(sw());
    await waitFor(() => expect(sw().getAttribute("aria-checked")).toBe("true"));
    expect(localStorage.getItem("cua.flag.spaces_tree")).toBe("1");
    expect(localStorage.getItem("cua.flag.spaces_ask_first_home")).toBe("1");
  });

  it("General has the default Space, If it's offline and the order to try, only while the switch is on", { timeout: 30000 }, async () => {
    const off = await mountApp("/settings/general", host([]));
    await off.router.load();
    await new Promise((r) => setTimeout(r, 300));
    expect(q("[data-default-space-group]")).toBeNull();
    cleanup();
    on();
    await mountApp("/settings/general", host([]));
    const group = await found("[data-default-space-group]");
    expect(group.textContent).toContain("Default Space");
    expect(group.textContent).toContain("If it's offline");
    expect(group.textContent).toContain("Order to try");
    expect(q('[data-order-item="here"]')?.textContent).toContain("Last resort");
  });
});
