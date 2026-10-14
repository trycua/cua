// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The page's context menus (`window.showContextMenu`): the request is
// checked, the window's native menu gets the items in order, the chosen
// item's id goes back, and a host without native menus says so (the page
// then draws its own). Right clicks the page leaves alone keep Cut, Copy
// and Paste in text fields.
import { describe, expect, it } from "vitest";
import { WEBKIT_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { BridgeEvents, BridgeRegistry } from "../src/bridge/host";
import { windowMethods } from "../src/bridge/window";
import { contextMenuOf, contextMenuPoint, contextMenuTemplate, editingMenu, type ContextMenuRequest } from "../src/context-menu";

const request = {
  items: [
    { id: "space.open", label: "Open" },
    { id: "", label: "", separator: true },
    { id: "space.stop", label: "Turn off…", destructive: true, enabled: false },
    { id: "space.copyId", label: "Copy ID", copy: "local:aurora" },
  ],
  x: 120.5,
  y: 64,
};

function registry(showContextMenu?: (win: unknown, menu: ContextMenuRequest) => Promise<string | null>) {
  const shown: [unknown, ContextMenuRequest][] = [];
  const ctx = {
    ui: showContextMenu
      ? {
          showContextMenu: (win: unknown, menu: ContextMenuRequest) => {
            shown.push([win, menu]);
            return showContextMenu(win, menu);
          },
        }
      : {},
    events: new BridgeEvents(),
  } as unknown as BridgeContext;
  const r = new BridgeRegistry();
  r.register(windowMethods(ctx));
  return { r, shown };
}

describe("window.showContextMenu", () => {
  it("shows the items over the calling window and answers the chosen id, in the contract's shape", async () => {
    const { r, shown } = registry(async () => "space.copyId");
    const answer = await r.handle("window.showContextMenu", request, { window: "win-1" });
    expect(answer).toEqual({ id: "space.copyId" });
    expect(validate(WEBKIT_SHAPES["window.showContextMenu"]!, answer, SHAPE_DEFS)).toEqual([]);
    expect(shown).toHaveLength(1);
    expect(shown[0]![0]).toBe("win-1");
    expect(shown[0]![1].items.map((i) => (i.separator ? "-" : `${i.id}${i.enabled ? "" : " (off)"}`))).toEqual([
      "space.open",
      "-",
      "space.stop (off)",
      "space.copyId",
    ]);
  });

  it("answers null when the menu closed without a choice", async () => {
    const { r } = registry(async () => null);
    const answer = await r.handle("window.showContextMenu", request, { window: "win-1" });
    expect(answer).toEqual({ id: null });
    expect(validate(WEBKIT_SHAPES["window.showContextMenu"]!, answer, SHAPE_DEFS)).toEqual([]);
  });

  it("refuses a malformed request, and says unimplemented without native menus", async () => {
    const { r } = registry(async () => null);
    const code = (args: Record<string, unknown>) => r.dispatch({ id: "x", method: "window.showContextMenu", args }).then((a) => (a.ok ? null : a.error.code));
    expect(await code({ items: [{ id: "a" }], x: 1, y: 1 })).toBe("bad_args");
    expect(await code({ items: [{ id: "a", label: "A" }], x: "1", y: 1 })).toBe("bad_args");
    expect(await code({ items: [{ id: "a", label: "A", copy: 4 }], x: 1, y: 1 })).toBe("bad_args");
    expect(await code({ x: 1, y: 1 })).toBe("bad_args");
    const bare = registry();
    expect(await bare.r.dispatch({ id: "x", method: "window.showContextMenu", args: request }).then((a) => (a.ok ? null : a.error.code))).toBe("unimplemented");
  });
});

describe("the native menu", () => {
  it("keeps the items' order, labels and enabled state, and a click picks its item", () => {
    const menu = contextMenuOf(request)!;
    const picked: string[] = [];
    const template = contextMenuTemplate(menu.items, (it) => picked.push(it.id));
    expect(template.map((t) => (t.type === "separator" ? "-" : `${t.label}${t.enabled ? "" : " (off)"}`))).toEqual(["Open", "-", "Turn off… (off)", "Copy ID"]);
    (template[3]!.click as () => void)();
    expect(picked).toEqual(["space.copyId"]);
    expect(menu.items[3]!.copy).toBe("local:aurora");
  });

  it("pops up at the page's point at its zoom", () => {
    expect(contextMenuPoint({ x: 120.5, y: 64 }, 1)).toEqual({ x: 121, y: 64 });
    expect(contextMenuPoint({ x: 100, y: 40 }, 1.25)).toEqual({ x: 125, y: 50 });
    expect(contextMenuPoint({ x: 100, y: 40 }, 0)).toEqual({ x: 100, y: 40 });
  });
});

describe("right clicks the page leaves alone", () => {
  const flags = { canUndo: false, canRedo: false, canCut: true, canCopy: true, canPaste: true, canSelectAll: true };
  it("a text field gets Cut, Copy and Paste", () => {
    const roles = editingMenu({ isEditable: true, selectionText: "", editFlags: flags }).map((t) => t.role ?? t.type);
    expect(roles).toEqual(["undo", "redo", "separator", "cut", "copy", "paste", "separator", "selectAll"]);
  });
  it("selected text gets Copy, and anything else nothing", () => {
    expect(editingMenu({ isEditable: false, selectionText: "hello", editFlags: flags }).map((t) => t.role)).toEqual(["copy"]);
    expect(editingMenu({ isEditable: false, selectionText: "  ", editFlags: flags })).toEqual([]);
  });
});
