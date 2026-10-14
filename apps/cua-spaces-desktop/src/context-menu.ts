// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The page's context menus, drawn natively (`window.showContextMenu`): the
// page names its items by stable action id and where it was clicked (CSS
// pixels from its top left); the menu pops up there, and the chosen item's
// id goes back (null when it closed without one). An item with `copy`
// puts that text on the clipboard here, since the page's own clipboard
// write could fail after the round trip. The SwiftUI host does the same
// with an NSMenu (`WebUIWindowController.showContextMenu`).
//
// Text fields and selected text keep a menu too: the page leaves those
// right clicks alone, and `editingMenu` answers them with Cut, Copy and
// Paste (Electron has no menu of its own).
import type { MenuItemConstructorOptions } from "electron";

/** One item as the page sends it (`WkMenuItem` in the web bridge). */
export interface ContextMenuItem {
  id: string;
  label: string;
  enabled: boolean;
  destructive: boolean;
  separator: boolean;
  copy?: string;
}

export interface ContextMenuRequest {
  items: ContextMenuItem[];
  x: number;
  y: number;
}

const isObject = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

/** The page's request, checked: null when it is not one. */
export function contextMenuOf(args: Record<string, unknown>): ContextMenuRequest | null {
  const { items, x, y } = args;
  if (!Array.isArray(items) || !Number.isFinite(x) || !Number.isFinite(y)) return null;
  const out: ContextMenuItem[] = [];
  for (const it of items) {
    if (!isObject(it)) return null;
    const separator = it.separator === true;
    if (!separator && (typeof it.id !== "string" || !it.id || typeof it.label !== "string")) return null;
    if (it.copy !== undefined && typeof it.copy !== "string") return null;
    out.push({
      id: separator ? "" : (it.id as string),
      label: separator ? "" : (it.label as string),
      enabled: it.enabled !== false,
      destructive: it.destructive === true,
      separator,
      ...(typeof it.copy === "string" ? { copy: it.copy } : {}),
    });
  }
  return { items: out, x: x as number, y: y as number };
}

/** The menu's template: `choose` runs with the item a click picked. */
export function contextMenuTemplate(items: ContextMenuItem[], choose: (item: ContextMenuItem) => void): MenuItemConstructorOptions[] {
  return items.map((it) => (it.separator ? { type: "separator" } : { id: it.id, label: it.label, enabled: it.enabled, click: () => choose(it) }));
}

/** Where the menu pops up in the window's content (DIPs): the page's point at its zoom. */
export function contextMenuPoint(req: Pick<ContextMenuRequest, "x" | "y">, zoom: number): { x: number; y: number } {
  const z = Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
  return { x: Math.round(req.x * z), y: Math.round(req.y * z) };
}

/** What Electron says about a right click the page left alone (`context-menu`'s params, the parts read here). */
export interface EditingParams {
  isEditable: boolean;
  selectionText: string;
  editFlags: { canUndo: boolean; canRedo: boolean; canCut: boolean; canCopy: boolean; canPaste: boolean; canSelectAll: boolean };
}

/** Cut, Copy and Paste in a text field; Copy on selected text; nothing elsewhere. */
export function editingMenu(p: EditingParams): MenuItemConstructorOptions[] {
  const f = p.editFlags;
  if (p.isEditable) {
    return [
      { role: "undo", enabled: f.canUndo },
      { role: "redo", enabled: f.canRedo },
      { type: "separator" },
      { role: "cut", enabled: f.canCut },
      { role: "copy", enabled: f.canCopy },
      { role: "paste", enabled: f.canPaste },
      { type: "separator" },
      { role: "selectAll", enabled: f.canSelectAll },
    ];
  }
  return p.selectionText.trim() ? [{ role: "copy", enabled: f.canCopy }] : [];
}
