// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space's "⋯" and right-click menu. The items are the core's
 * (`spaces.menu`): only what you may do with it, Delete Space in red set
 * apart for yours, Leave for someone else's. Each one runs the flow the
 * page already has.
 */

import { ContextMenu } from "@base-ui/react/context-menu";
import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { useNavigate } from "@tanstack/react-router";
import { MoreHorizontalIcon } from "lucide-react";
import { Fragment, type ReactNode } from "react";

import { spaceMenu, useBridge, useNotchPins, useSession, useShareSheet, useTeleport, type SpaceMenuInput, type SpaceMenuItem } from "@/bridge";
import { toastError } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

import { canPinToNotch, type SpaceItem } from "./model";
import { MENU_ITEM, MENU_POPUP } from "./spaces-toolbar";

/**
 * What the core's menu reads for this Space. `pinned`: the notch's pins, when the host can keep
 * them (Pin to notch / Unpin from notch); without it the menu has no pin item. One of your other
 * machines' desktops is not a Space to turn on, share or delete: its menu is Open and the pin.
 */
export function menuInput(item: SpaceItem, pinned?: ReadonlySet<string>): SpaceMenuInput {
  const { access, state, space } = item;
  const pin = pinned && canPinToNotch(item) ? { canPin: true, pinned: pinned.has(space.id) } : {};
  if (item.desktop) return { kind: "mine", role: "owner", power: "off", ...pin };
  const busy = space.power?.turningOn !== undefined && space.power?.turningOn !== null;
  return {
    kind: access.kind,
    role: access.role,
    // A team's Space you made: its maker deletes it.
    maker: access.kind === "team" && access.canDelete,
    power: busy || state === "creating" || state === "deleting" ? "busy" : state === "running" ? "running" : "off",
    shared: access.people.length > 0,
    canPower: access.canPower && Boolean(space.power),
    canTeleport: access.canTeleport,
    canShare: access.canShare,
    // No host renames a Space yet (`spaces.*` has no rename): the item stays out until one does.
    canRename: false,
    canDelete: access.canDelete && item.mine,
    canLeave: access.canLeave,
    ...pin,
  };
}

export interface MenuActions {
  /** Turn it on (true) or off. */
  power: (item: SpaceItem, on: boolean) => void;
  /** Ask to delete or leave it (the bulk confirm, with one Space). */
  remove: (item: SpaceItem) => void;
  /** Hide a machine's desktop row from the list (Hide on This Mac's hover, in the menu of another machine's). */
  hide?: (item: SpaceItem) => void;
}

/** One menu entry: the core's item, or Hide (the page's own, on another machine's desktop), with what it does. */
type MenuEntry = Omit<SpaceMenuItem, "id"> & { id: SpaceMenuItem["id"] | "hide"; run: () => void };

/** The menu's items, each with what it does. */
export function useSpaceMenu(item: SpaceItem, actions: MenuActions): MenuEntry[] {
  const { core } = useBridge();
  const navigate = useNavigate();
  const share = useShareSheet();
  const teleport = useTeleport();
  const { data: session } = useSession();
  const pins = useNotchPins();
  // This Mac's desktop and a desktop that is not shared have no menu: Hide is This Mac's hover button.
  if (core.status !== "ready" || item.desktop === "off" || item.space.id === "this-mac") return [];
  const items = spaceMenu(core, menuInput(item, pins.canPin ? pins.pinned : undefined));
  const run = (id: MenuEntry["id"]) => {
    switch (id) {
      case "open":
        return void navigate({ to: "/spaces/$spaceId", params: { spaceId: item.space.id } });
      case "power":
        return actions.power(item, item.state !== "running");
      case "share":
      case "manageSharing":
        return void share.open(item.space, session?.signedIn ?? false);
      case "teleport":
        return void teleport.open(item.space);
      case "delete":
      case "leave":
        return actions.remove(item);
      case "pin":
        return void pins.toggle(item.space.id).catch(toastError(`Couldn't change the notch`));
      case "hide":
        return actions.hide?.(item);
      case "rename":
        return undefined;
    }
  };
  const entries: MenuEntry[] = items.map((i) => ({ ...i, run: () => run(i.id) }));
  // Another machine's desktop leaves the list from here (This Mac's has the hover button).
  if (item.desktop && actions.hide) entries.push({ id: "hide", label: "Hide", enabled: true, destructive: false, separatorBefore: false, run: () => run("hide") });
  return entries;
}

function Items({ items, Item, Separator }: { items: MenuEntry[]; Item: typeof MenuPrimitive.Item; Separator: typeof MenuPrimitive.Separator }) {
  return items.map((i) => (
    <Fragment key={i.id}>
      {i.separatorBefore ? <Separator className="-mx-1 my-1 h-px bg-foreground/10" /> : null}
      <Item
        disabled={!i.enabled}
        data-menu-item={i.id}
        onClick={i.run}
        className={cn(MENU_ITEM, i.destructive && "text-destructive data-highlighted:bg-destructive data-highlighted:text-white")}
      >
        {i.label}
      </Item>
    </Fragment>
  ));
}

const MORE_BUTTON =
  "flex size-7 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground opacity-0 outline-none group-hover/row:opacity-100 group-hover/tile:opacity-100 group-focus-within/tile:opacity-100 hover:bg-tile hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-tile data-popup-open:text-foreground data-popup-open:opacity-100";

/**
 * The "⋯" button, shown on hover, while focus is in the tile and while its
 * menu is open. Without items (the app core still loading, or none here)
 * it still shows, disabled, saying why, so every tile has the same controls.
 */
export function SpaceMoreMenu({ item, actions, className }: { item: SpaceItem; actions: MenuActions; className?: string }) {
  const items = useSpaceMenu(item, actions);
  const { core } = useBridge();
  if (!items.length) {
    const why = core.status === "loading" ? "Still loading" : "This Space's actions aren't available here";
    return (
      <button
        type="button"
        aria-label={`More for ${item.space.name}`}
        aria-disabled
        aria-description={why}
        title={why}
        data-space-more={item.space.id}
        data-disabled=""
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
        }}
        className={cn(MORE_BUTTON, "text-muted-foreground/50 hover:bg-transparent hover:text-muted-foreground/50", className)}
      >
        <MoreHorizontalIcon className="size-4" />
      </button>
    );
  }
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        aria-label={`More for ${item.space.name}`}
        data-space-more={item.space.id}
        onClick={(e) => e.stopPropagation()}
        className={cn(MORE_BUTTON, className)}
      >
        <MoreHorizontalIcon className="size-4" />
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="end" className="z-50 outline-none">
          <MenuPrimitive.Popup data-space-menu="more" className={MENU_POPUP}>
            <Items items={items} Item={MenuPrimitive.Item} Separator={MenuPrimitive.Separator} />
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}

/** The same menu at the pointer, anywhere on a tile or a list row. */
export function SpaceContextMenu({ item, actions, children }: { item: SpaceItem; actions: MenuActions; children: ReactNode }) {
  const items = useSpaceMenu(item, actions);
  if (!items.length) return <>{children}</>;
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger render={<div />}>{children}</ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Positioner className="z-50 outline-none">
          <ContextMenu.Popup data-space-menu="context" className={MENU_POPUP}>
            <Items items={items} Item={ContextMenu.Item} Separator={ContextMenu.Separator} />
          </ContextMenu.Popup>
        </ContextMenu.Positioner>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
}

/**
 * The "⋯" on a row of a machine's page ("Spaces on this machine": its shared desktop, then the Spaces it
 * provides): Open and Pin to notch / Unpin from notch, the core's menu for what a row there can do. It
 * shows on hover over the row's chevron. Nothing where the app core can't pin.
 */
export function MachineRowMenu({ id, name }: { id: string; name: string }) {
  const { core } = useBridge();
  const navigate = useNavigate();
  const pins = useNotchPins();
  if (core.status !== "ready" || !pins.canPin) return null;
  const items = spaceMenu(core, { kind: "mine", role: "owner", power: "off", canPin: true, pinned: pins.pinned.has(id) });
  const run = (item: SpaceMenuItem) => {
    if (item.id === "open") void navigate({ to: "/spaces/$spaceId", params: { spaceId: id } });
    else if (item.id === "pin") void pins.toggle(id).catch(toastError("Couldn't change the notch"));
  };
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger aria-label={`More for ${name}`} data-space-more={id} className={cn(MORE_BUTTON, "absolute top-1/2 right-[-6px] -translate-y-1/2")}>
        <MoreHorizontalIcon className="size-4" />
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="end" className="z-50 outline-none">
          <MenuPrimitive.Popup data-space-menu="more" className={MENU_POPUP}>
            {items.map((i) => (
              <MenuPrimitive.Item key={i.id} disabled={!i.enabled} data-menu-item={i.id} onClick={() => run(i)} className={MENU_ITEM}>
                {i.label}
              </MenuPrimitive.Item>
            ))}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
