// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Right-click menus. A surface lists its actions and spreads
 * `useContextMenu(items)` onto its element:
 *
 *     const menu = useContextMenu(() => [
 *       { id: "space.open", label: "Open", run: open },
 *       SEPARATOR,
 *       canDelete && { id: "space.delete", label: "Delete…", destructive: true, confirm: {...}, run: remove },
 *     ]);
 *     <div {...menu}>…</div>
 *
 * The Mac app and Electron draw the menu natively (`window.showContextMenu`:
 * NSMenu, `Menu.popup`), the browser demo draws it in the page. A right
 * click, the context-menu key and Shift+F10 open it. Each item has a stable
 * action id; choosing one runs the item, after its `confirm` when it has
 * one. `<ContextMenuHost />` (mounted once by the root layout) draws the
 * page's menu and the confirmations, and turns off the web view's own menu
 * everywhere but in text fields and selected text; a link gets Open Link
 * and Copy Link.
 */

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { useEffect, type KeyboardEvent, type MouseEvent } from "react";
import { create } from "zustand";

import { HostError, useBridge, type DataAdapter, type WkMenuItem } from "@/bridge";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { cn } from "@/lib/utils";

/** What a destructive item asks first. */
export interface ContextMenuConfirm {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  /** Red confirm button (default: the item's `destructive`). */
  destructive?: boolean;
  /** Why the confirm button can't be pressed now. */
  disabledReason?: string | null;
  /** A second way through ("Remove from List"). */
  alternative?: { label: string; run: () => void | Promise<unknown> } | null;
}

/** One action in a context menu. */
export interface ContextMenuAction {
  /** Stable action id, `<surface>.<action>` (`space.stop`, `machine.copyName`). */
  id: string;
  label: string;
  /** What choosing it does. Optional with `copy`. */
  run?: () => void | Promise<unknown>;
  /** Shown, but not choosable now. */
  disabled?: boolean;
  /** Loses work or data: drawn red where the menu can, and should `confirm`. */
  destructive?: boolean;
  /** Ask this first; `run` only once confirmed. */
  confirm?: ContextMenuConfirm;
  /** Puts this text on the clipboard when chosen (natively in the hosts). */
  copy?: string;
}

/** A line between groups. */
export const SEPARATOR = { separator: true } as const;
export type ContextMenuSeparator = typeof SEPARATOR;

/** What a menu lists. Falsy entries are skipped, so conditions read inline. */
export type ContextMenuEntry = ContextMenuAction | ContextMenuSeparator | null | undefined | false | "" | 0;

export type ContextMenuItems = readonly ContextMenuEntry[] | (() => readonly ContextMenuEntry[]);

const isSeparator = (e: ContextMenuAction | ContextMenuSeparator): e is ContextMenuSeparator => "separator" in e;

/** The entries as drawn: no falsy ones, and no separator first, last or twice. */
export function normalizeMenu(entries: readonly ContextMenuEntry[]): (ContextMenuAction | ContextMenuSeparator)[] {
  const out: (ContextMenuAction | ContextMenuSeparator)[] = [];
  for (const e of entries) {
    if (!e) continue;
    if (isSeparator(e) && (out.length === 0 || isSeparator(out[out.length - 1]!))) continue;
    out.push(e);
  }
  while (out.length && isSeparator(out[out.length - 1]!)) out.pop();
  return out;
}

/** The items as the host draws them (`window.showContextMenu`). */
export function toWireItems(items: readonly (ContextMenuAction | ContextMenuSeparator)[]): WkMenuItem[] {
  return items.map((e) =>
    isSeparator(e)
      ? { id: "", label: "", separator: true }
      : {
          id: e.id,
          label: e.label,
          ...(e.disabled ? { enabled: false } : {}),
          ...(e.destructive ? { destructive: true } : {}),
          ...(e.copy !== undefined ? { copy: e.copy } : {}),
        },
  );
}

/* ---- State: the page's own menu and the confirmation ---------------------------- */

interface OpenMenu {
  items: (ContextMenuAction | ContextMenuSeparator)[];
  x: number;
  y: number;
  /** Opened from the keyboard: the first item is highlighted. */
  keyboard: boolean;
  /** Where focus returns when the menu closes. */
  returnFocus: HTMLElement | null;
}

interface MenuState {
  open: OpenMenu | null;
  confirming: ContextMenuAction | null;
}

export const useContextMenuStore = create<MenuState>()(() => ({ open: null, confirming: null }));

/** Runs a chosen item: asks first when it has `confirm`. `copied`: the host already copied its text. */
export function chooseContextMenuItem(item: ContextMenuAction, copied = false): void {
  if (item.disabled) return;
  if (item.confirm) {
    useContextMenuStore.setState({ confirming: item });
    return;
  }
  void runItem(item, copied);
}

async function runItem(item: ContextMenuAction, copied: boolean): Promise<void> {
  try {
    if (item.copy !== undefined && !copied) await navigator.clipboard?.writeText(item.copy);
    await item.run?.();
  } catch (e) {
    console.error(`context menu: ${item.id} failed`, e);
  }
}

/**
 * Shows `entries` at (`x`, `y`) (CSS pixels in the viewport): natively when
 * the host draws menus, else in the page. Resolves once the menu closed;
 * a chosen item then runs (or asks first).
 */
export async function showContextMenu(
  adapter: DataAdapter | null | undefined,
  entries: readonly ContextMenuEntry[],
  at: { x: number; y: number; keyboard?: boolean; returnFocus?: HTMLElement | null },
): Promise<void> {
  const items = normalizeMenu(entries);
  if (!items.some((e) => !isSeparator(e))) return;
  if (adapter?.showContextMenu) {
    try {
      const id = await adapter.showContextMenu({ items: toWireItems(items), x: Math.round(at.x), y: Math.round(at.y) });
      const chosen = items.find((e): e is ContextMenuAction => !isSeparator(e) && e.id === id);
      if (chosen) chooseContextMenuItem(chosen, true);
      return;
    } catch (e) {
      // An app from before native menus: the page draws it.
      if (!(e instanceof HostError && e.code === "unimplemented")) {
        console.error("context menu: the host could not show it", e);
        return;
      }
    }
  }
  useContextMenuStore.setState({ open: { items, x: at.x, y: at.y, keyboard: Boolean(at.keyboard), returnFocus: at.returnFocus ?? null } });
}

/* ---- The hook a surface spreads onto its element --------------------------------- */

/** The key presses that open a context menu: the menu key and Shift+F10. */
export const isContextMenuKey = (e: Pick<KeyboardEvent, "key" | "shiftKey">) => e.key === "ContextMenu" || (e.shiftKey && e.key === "F10");

/** Where a keyboard-opened menu goes: inside the element's top left. */
function anchorOf(el: Element): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  return { x: r.left + Math.min(16, r.width / 2), y: r.top + Math.min(24, r.height) };
}

export interface ContextMenuProps {
  onContextMenu: (e: MouseEvent<HTMLElement>) => void;
  onKeyDown: (e: KeyboardEvent<HTMLElement>) => void;
  "data-context-menu": "";
}

/**
 * Props that give an element its context menu. `items` is read when the menu
 * opens, so a function keeps the work (the core's calls) to that moment.
 * An element already handling `onKeyDown` should call `props.onKeyDown` too.
 */
export function useContextMenu(items: ContextMenuItems): ContextMenuProps {
  const { data: adapter } = useBridge();
  const read = () => (typeof items === "function" ? items() : items);
  return {
    "data-context-menu": "",
    onContextMenu: (e) => {
      // Text the person selected in here keeps the system's menu (Copy, Look Up).
      if (hasSelectionAt(e.target as Node) || isEditable(e.target as Element | null)) return;
      e.preventDefault();
      e.stopPropagation();
      // A keyboard-made contextmenu event has no pointer position.
      const keyboard = e.button !== 2 && e.clientX === 0 && e.clientY === 0;
      const at = keyboard ? anchorOf(e.currentTarget) : { x: e.clientX, y: e.clientY };
      void showContextMenu(adapter, read(), { ...at, keyboard, returnFocus: e.currentTarget });
    },
    onKeyDown: (e) => {
      if (!isContextMenuKey(e) || isEditable(e.target as Element | null)) return;
      e.preventDefault();
      e.stopPropagation();
      void showContextMenu(adapter, read(), { ...anchorOf(e.currentTarget), keyboard: true, returnFocus: e.currentTarget });
    },
  };
}

/* ---- The web view's own menu ------------------------------------------------------ */

/** A text field, or inside one: it keeps Cut, Copy and Paste. */
export function isEditable(el: Element | null | undefined): boolean {
  const field = el?.closest?.("input, textarea, select, [contenteditable]:not([contenteditable='false'])");
  if (!field) return false;
  if (field instanceof HTMLInputElement) {
    return !["button", "checkbox", "color", "file", "hidden", "image", "radio", "range", "reset", "submit"].includes(field.type);
  }
  return true;
}

/** Text is selected, and the right click is on it. */
export function hasSelectionAt(el: Node | null | undefined): boolean {
  const sel = globalThis.getSelection?.();
  if (!el || !sel || sel.isCollapsed || sel.toString().trim() === "") return false;
  for (let i = 0; i < sel.rangeCount; i++) if (sel.getRangeAt(i).intersectsNode(el)) return true;
  return false;
}

/** A link that leaves the app (http, https, mailto): its address. */
export function externalLink(el: Element | null | undefined): string | null {
  const a = el?.closest?.("a[href]") as HTMLAnchorElement | null | undefined;
  if (!a) return null;
  const href = a.href;
  try {
    const url = new URL(href);
    if (url.protocol === "mailto:") return href;
    if ((url.protocol === "http:" || url.protocol === "https:") && url.origin !== globalThis.location?.origin) return href;
  } catch {
    return null;
  }
  return null;
}

/** What the page's menu is for a right click no surface handled. */
export function defaultMenuFor(target: Element | null): "system" | { link: string } | "none" {
  if (isEditable(target) || hasSelectionAt(target)) return "system";
  const link = externalLink(target);
  return link ? { link } : "none";
}

/** Open Link and Copy Link, for a link no surface gave a menu of its own. */
export function linkMenu(href: string): ContextMenuAction[] {
  return [
    { id: "link.open", label: "Open Link", run: () => void globalThis.open?.(href, "_blank", "noopener") },
    { id: "link.copy", label: "Copy Link", copy: href },
  ];
}

/* ---- The host component ----------------------------------------------------------- */

/**
 * Mounted once by the root layout: draws the page's menu (hosts without
 * native menus) and the confirmations, and stands in for the web view's
 * own menu where no surface gave one.
 */
export function ContextMenuHost() {
  const { data: adapter } = useBridge();
  const open = useContextMenuStore((s) => s.open);
  const confirming = useContextMenuStore((s) => s.confirming);

  useEffect(() => {
    const onContextMenu = (e: globalThis.MouseEvent) => {
      if (e.defaultPrevented) return;
      const what = defaultMenuFor(e.target as Element | null);
      if (what === "system") return;
      e.preventDefault();
      if (what !== "none") void showContextMenu(adapter, linkMenu(what.link), { x: e.clientX, y: e.clientY });
    };
    document.addEventListener("contextmenu", onContextMenu);
    return () => document.removeEventListener("contextmenu", onContextMenu);
  }, [adapter]);

  const close = () => {
    const back = useContextMenuStore.getState().open?.returnFocus;
    useContextMenuStore.setState({ open: null });
    if (back?.isConnected) back.focus({ preventScroll: true });
  };
  const anchor = open
    ? { getBoundingClientRect: () => DOMRect.fromRect({ x: open.x, y: open.y, width: 0, height: 0 }) }
    : null;
  const c = confirming?.confirm;

  return (
    <>
      <MenuPrimitive.Root open={open !== null} onOpenChange={(o) => !o && close()} modal={false}>
        <MenuPrimitive.Portal>
          <MenuPrimitive.Positioner anchor={anchor} side="bottom" align="start" sideOffset={2} className="z-50 outline-none">
            <MenuPrimitive.Popup
              data-page-context-menu=""
              className="glass min-w-44 origin-(--transform-origin) rounded-lg border p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0"
            >
              {open?.items.map((e, i) =>
                isSeparator(e) ? (
                  <MenuPrimitive.Separator key={`sep-${i}`} className="mx-2 my-1 h-px bg-border" />
                ) : (
                  <MenuPrimitive.Item
                    key={e.id}
                    data-action-id={e.id}
                    disabled={e.disabled}
                    onClick={() => {
                      close();
                      chooseContextMenuItem(e);
                    }}
                    className={cn(
                      "flex cursor-default items-center rounded-md px-2 py-1 text-[13px] outline-none data-disabled:opacity-50 data-highlighted:bg-brand data-highlighted:text-white",
                      e.destructive && "text-destructive",
                    )}
                  >
                    {e.label}
                  </MenuPrimitive.Item>
                ),
              )}
            </MenuPrimitive.Popup>
          </MenuPrimitive.Positioner>
        </MenuPrimitive.Portal>
      </MenuPrimitive.Root>
      <ConfirmDialog
        open={confirming !== null}
        onOpenChange={(o) => !o && useContextMenuStore.setState({ confirming: null })}
        title={c?.title ?? ""}
        description={c?.message ?? ""}
        confirmLabel={c?.confirmLabel ?? ""}
        cancelLabel={c?.cancelLabel}
        destructive={c?.destructive ?? confirming?.destructive}
        disabledReason={c?.disabledReason ?? null}
        onConfirm={() => confirming && void runItem(confirming, false)}
        alternative={c?.alternative ? { label: c.alternative.label, onClick: () => void c.alternative!.run() } : null}
      />
    </>
  );
}
