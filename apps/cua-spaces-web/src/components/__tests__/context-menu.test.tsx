// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Right-click menus: a surface's items go to the host's native menu (the
// Mac app, Electron) with stable action ids and come back as a choice that
// runs the item, after a confirmation for a destructive one; the browser
// demo draws the same menu in the page; the keyboard opens it too; and the
// web view's own menu ("Copy Image") never shows except in text fields and
// on selected text. Then the Space tile's and the machine row's menus on
// the real pages (wasm core, demo host): only the actions their pages
// offer, by the same rules.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BridgeProvider, HostError, type DataAdapter, type WkContextMenuRequest } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import {
  ContextMenuHost,
  SEPARATOR,
  defaultMenuFor,
  normalizeMenu,
  toWireItems,
  useContextMenu,
  useContextMenuStore,
  type ContextMenuEntry,
} from "@/components/context-menu";
import { ToastProvider } from "@/components/ui/toast";
import { mountApp, stubBrowserApis } from "@/__tests__/app-harness";

afterEach(() => {
  cleanup();
  useContextMenuStore.setState({ open: null, confirming: null });
});
stubBrowserApis();

/** A demo host, optionally with native menus that answer `choose`. */
function host(choose?: (menu: WkContextMenuRequest) => Promise<string | null>): { adapter: DataAdapter; shown: WkContextMenuRequest[] } {
  const demo = createDemoAdapter({ latencyMs: 1 });
  const shown: WkContextMenuRequest[] = [];
  const adapter: DataAdapter = {
    mode: demo.mode,
    call: demo.call.bind(demo),
    subscribe: demo.subscribe.bind(demo),
    ...(choose
      ? {
          showContextMenu: (menu: WkContextMenuRequest) => {
            shown.push(menu);
            return choose(menu);
          },
        }
      : {}),
  };
  return { adapter, shown };
}

function Target({ items }: { items: ContextMenuEntry[] }) {
  const menu = useContextMenu(() => items);
  return (
    <div {...menu} tabIndex={0} data-testid="target">
      target
    </div>
  );
}

function mount(adapter: DataAdapter, ui: React.ReactNode) {
  return render(
    <BridgeProvider adapter={adapter} core={noCore}>
      <ToastProvider>
        {ui}
        <ContextMenuHost />
      </ToastProvider>
    </BridgeProvider>,
  );
}

describe("the items", () => {
  it("drop falsy entries and stray separators, and go to the host by id", () => {
    const run = () => {};
    const items = normalizeMenu([SEPARATOR, { id: "a.open", label: "Open", run }, false, SEPARATOR, SEPARATOR, null, { id: "a.copy", label: "Copy ID", copy: "x" }, { id: "a.del", label: "Delete…", destructive: true, disabled: true, run }, SEPARATOR]);
    expect(toWireItems(items)).toEqual([
      { id: "a.open", label: "Open" },
      { id: "", label: "", separator: true },
      { id: "a.copy", label: "Copy ID", copy: "x" },
      { id: "a.del", label: "Delete…", enabled: false, destructive: true },
    ]);
  });
});

describe("a native host", () => {
  it("shows the items at the click and runs the chosen one", async () => {
    const open = vi.fn();
    const { adapter, shown } = host(async () => "a.open");
    mount(adapter, <Target items={[{ id: "a.open", label: "Open", run: open }, SEPARATOR, { id: "a.other", label: "Other", run: vi.fn() }]} />);
    await screen.findByTestId("target");
    const e = fireEvent.contextMenu(screen.getByTestId("target"), { clientX: 40, clientY: 50, button: 2 });
    expect(e).toBe(false); // the web view's own menu does not show
    await waitFor(() => expect(open).toHaveBeenCalledOnce());
    expect(shown).toEqual([{ items: [{ id: "a.open", label: "Open" }, { id: "", label: "", separator: true }, { id: "a.other", label: "Other" }], x: 40, y: 50 }]);
    expect(document.querySelector("[data-page-context-menu]")).toBeNull();
  });

  it("asks first for a destructive item, and runs it only once confirmed", async () => {
    const remove = vi.fn();
    const { adapter } = host(async () => "a.delete");
    mount(adapter, <Target items={[{ id: "a.delete", label: "Delete…", destructive: true, confirm: { title: "Delete Aurora?", message: "Gone for good.", confirmLabel: "Delete Space" }, run: remove }]} />);
    fireEvent.contextMenu(await screen.findByTestId("target"), { clientX: 4, clientY: 4, button: 2 });
    const confirm = await screen.findByRole("button", { name: "Delete Space" });
    expect(screen.getByText("Delete Aurora?")).toBeTruthy();
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(confirm);
    await waitFor(() => expect(remove).toHaveBeenCalledOnce());
  });

  it("does nothing when the menu closed without a choice", async () => {
    const run = vi.fn();
    const { adapter, shown } = host(async () => null);
    mount(adapter, <Target items={[{ id: "a.open", label: "Open", run }]} />);
    fireEvent.contextMenu(await screen.findByTestId("target"), { clientX: 4, clientY: 4, button: 2 });
    await waitFor(() => expect(shown).toHaveLength(1));
    await act(() => Promise.resolve());
    expect(run).not.toHaveBeenCalled();
  });

  it("opens from the keyboard (Shift+F10, the menu key) at the element", async () => {
    const { adapter, shown } = host(async () => null);
    mount(adapter, <Target items={[{ id: "a.open", label: "Open", run: vi.fn() }]} />);
    const target = await screen.findByTestId("target");
    fireEvent.keyDown(target, { key: "F10", shiftKey: true });
    fireEvent.keyDown(target, { key: "ContextMenu" });
    fireEvent.keyDown(target, { key: "F10" });
    await waitFor(() => expect(shown).toHaveLength(2));
  });

  it("falls back to the page's menu on an app without native menus", async () => {
    const { adapter } = host(async () => {
      throw new HostError("window.showContextMenu is not available in this host", "unimplemented");
    });
    mount(adapter, <Target items={[{ id: "a.open", label: "Open", run: vi.fn() }]} />);
    fireEvent.contextMenu(await screen.findByTestId("target"), { clientX: 4, clientY: 4, button: 2 });
    await waitFor(() => expect(document.querySelector('[data-action-id="a.open"]')).not.toBeNull());
  });
});

describe("the page's own menu (browser demo)", () => {
  beforeEach(() => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn(() => Promise.resolve()) } });
  });

  it("draws the items, and a click runs one", async () => {
    const open = vi.fn();
    const { adapter } = host();
    mount(adapter, <Target items={[{ id: "a.open", label: "Open", run: open }, SEPARATOR, { id: "a.copyId", label: "Copy ID", copy: "local:aurora" }, { id: "a.off", label: "Off", disabled: true, run: vi.fn() }]} />);
    fireEvent.contextMenu(await screen.findByTestId("target"), { clientX: 30, clientY: 30, button: 2 });
    const item = await waitFor(() => document.querySelector<HTMLElement>('[data-action-id="a.copyId"]')!);
    expect([...document.querySelectorAll("[data-action-id]")].map((e) => e.getAttribute("data-action-id"))).toEqual(["a.open", "a.copyId", "a.off"]);
    expect(document.querySelector('[data-action-id="a.off"]')?.hasAttribute("data-disabled")).toBe(true);
    fireEvent.click(item);
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith("local:aurora"));
    expect(open).not.toHaveBeenCalled();
  });
});

describe("the web view's own menu", () => {
  it("is off everywhere but text fields and selected text; an outside link gets Open and Copy Link", async () => {
    const { adapter, shown } = host(async () => null);
    mount(
      adapter,
      <div>
        <img data-testid="img" alt="" />
        <input data-testid="field" />
        <input type="checkbox" data-testid="box" />
        <a href="https://cua.ai/docs" data-testid="link">
          docs
        </a>
        <p data-testid="text">some text</p>
      </div>,
    );
    expect(fireEvent.contextMenu(await screen.findByTestId("img"))).toBe(false);
    expect(fireEvent.contextMenu(screen.getByTestId("box"))).toBe(false);
    expect(fireEvent.contextMenu(screen.getByTestId("field"))).toBe(true);
    expect(defaultMenuFor(screen.getByTestId("field"))).toBe("system");
    expect(fireEvent.contextMenu(screen.getByTestId("link"))).toBe(false);
    await waitFor(() => expect(shown.at(-1)?.items).toEqual([{ id: "link.open", label: "Open Link" }, { id: "link.copy", label: "Copy Link", copy: "https://cua.ai/docs" }]));
    const text = screen.getByTestId("text");
    const range = document.createRange();
    range.selectNodeContents(text);
    getSelection()!.removeAllRanges();
    getSelection()!.addRange(range);
    expect(fireEvent.contextMenu(text)).toBe(true);
    getSelection()!.removeAllRanges();
    expect(fireEvent.contextMenu(text)).toBe(false);
  });
});

/* ---- The real pages ---------------------------------------------------------- */

const menuOf = async (shown: WkContextMenuRequest[], el: Element) => {
  const before = shown.length;
  fireEvent.contextMenu(el, { clientX: 10, clientY: 10, button: 2 });
  await waitFor(() => expect(shown.length).toBe(before + 1));
  return shown.at(-1)!.items.map((i) => (i.separator ? "-" : `${i.id}${i.enabled === false ? " (off)" : ""}`));
};

const find = (selector: string) =>
  waitFor(() => {
    const el = document.querySelector<HTMLElement>(selector);
    if (!el) throw new Error(`no ${selector}`);
    return el;
  });

const tileOf = (id: string) =>
  waitFor(() => {
    const link = document.querySelector<HTMLElement>(`[data-space-id="${id}"]`);
    if (!link) throw new Error(`no tile ${id}`);
    return link.parentElement!;
  });

describe.skipIf(!wasmBuilt)("a Space's menu on the Spaces page", () => {
  it("offers what its page offers, by its state", async () => {
    const shown: WkContextMenuRequest[] = [];
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, showContextMenu: async (m) => (shown.push(m), null) });
    // Running, turns off (stop): Turn off and Restart.
    const running = await tileOf("relay:mac-mini/qa-windows");
    await waitFor(() => expect(running.querySelector("[data-space-id]")?.getAttribute("data-state")).toBe("running"));
    expect(await menuOf(shown, running)).toEqual([
      "space.open",
      "-",
      "space.stop",
      "space.restart",
      "-",
      "space.teleport",
      "space.sendFiles",
      "space.share",
      "-",
      "space.copyName",
      "space.copyId",
      "-",
      "space.delete",
    ]);
    expect(shown.at(-1)!.items.find((i) => i.id === "space.copyId")?.copy).toBe("relay:mac-mini/qa-windows");
    // Running, suspends: Suspend, no Restart.
    const suspends = await tileOf("local:design-review");
    const items = await menuOf(shown, suspends);
    expect(items).toContain("space.stop");
    expect(items).not.toContain("space.restart");
    expect(shown.at(-1)!.items.find((i) => i.id === "space.stop")?.label).toBe("Suspend…");
    // Stopped: Turn on, and nothing that needs it running.
    const stopped = await tileOf("local:agent-sandbox");
    await waitFor(() => expect(stopped.querySelector("[data-space-id]")?.getAttribute("data-state")).toBe("stopped"));
    const off = await menuOf(shown, stopped);
    expect(off).toContain("space.start");
    expect(off).not.toContain("space.stop");
    expect(off).not.toContain("space.sendFiles");
    expect(off.filter((i) => i.startsWith("space.teleport") || i.startsWith("space.share"))).toEqual(["space.teleport (off)", "space.share (off)"]);
  });

  it("stops a Space only after the confirmation, through the page's own action", async () => {
    const calls: [string, unknown][] = [];
    await mountApp("/spaces", {
      failsCreates: false,
      listsGhost: false,
      showContextMenu: async () => "space.stop",
      intercept: (op, args) => void calls.push([op, args]),
    });
    const tile = await tileOf("relay:mac-mini/qa-windows");
    await waitFor(() => expect(tile.querySelector("[data-space-id]")?.getAttribute("data-state")).toBe("running"));
    fireEvent.contextMenu(tile, { clientX: 10, clientY: 10, button: 2 });
    const confirm = await screen.findByRole("button", { name: "Turn off" });
    expect(calls.some(([op]) => op === "spaces.setPower")).toBe(false);
    fireEvent.click(confirm);
    await waitFor(() => expect(calls.find(([op]) => op === "spaces.setPower")?.[1]).toMatchObject({ spaceId: "relay:mac-mini/qa-windows", on: false }));
  });
});

describe.skipIf(!wasmBuilt)("a machine's menu on Machines", () => {
  it("offers Open, New Space here and Copy Name; Open shows its detail", async () => {
    const shown: WkContextMenuRequest[] = [];
    let answer: string | null = null;
    await mountApp("/machines", { failsCreates: false, listsGhost: false, showContextMenu: async (m) => (shown.push(m), answer) });
    const here = await find('[data-machine-id="this-mac"]');
    expect(await menuOf(shown, here)).toEqual(["machine.open", "machine.newSpace", "-", "machine.copyName"]);
    expect(shown.at(-1)!.items.find((i) => i.id === "machine.copyName")?.copy).toBe("This Mac");
    answer = "machine.open";
    const other = await find('[data-machine-id="linux-box"]');
    fireEvent.contextMenu(other, { clientX: 10, clientY: 10, button: 2 });
    await waitFor(() => expect(other.getAttribute("aria-current")).toBe("true"));
  });
});
