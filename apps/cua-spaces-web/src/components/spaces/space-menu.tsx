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

import { spaceMenu, useBridge, useSession, useShareSheet, useTeleport, type SpaceMenuInput, type SpaceMenuItem } from "@/bridge";
import { cn } from "@/lib/utils";

import type { SpaceItem } from "./model";
import { MENU_ITEM, MENU_POPUP } from "./spaces-toolbar";

/** What the core's menu reads for this Space. */
export function menuInput(item: SpaceItem): SpaceMenuInput {
  const { access, state, space } = item;
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
  };
}

export interface MenuActions {
  /** Turn it on (true) or off. */
  power: (item: SpaceItem, on: boolean) => void;
  /** Ask to delete or leave it (the bulk confirm, with one Space). */
  remove: (item: SpaceItem) => void;
}

/** The menu's items, each with what it does. */
export function useSpaceMenu(item: SpaceItem, actions: MenuActions): (SpaceMenuItem & { run: () => void })[] {
  const { core } = useBridge();
  const navigate = useNavigate();
  const share = useShareSheet();
  const teleport = useTeleport();
  const { data: session } = useSession();
  if (core.status !== "ready") return [];
  const items = spaceMenu(core, menuInput(item));
  const run = (id: SpaceMenuItem["id"]) => {
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
      case "rename":
        return undefined;
    }
  };
  return items.map((i) => ({ ...i, run: () => run(i.id) }));
}

function Items({ items, Item, Separator }: { items: ReturnType<typeof useSpaceMenu>; Item: typeof MenuPrimitive.Item; Separator: typeof MenuPrimitive.Separator }) {
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

/** The "⋯" button, shown on hover (and while its menu is open). */
export function SpaceMoreMenu({ item, actions, className }: { item: SpaceItem; actions: MenuActions; className?: string }) {
  const items = useSpaceMenu(item, actions);
  if (!items.length) return null;
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        aria-label={`More for ${item.space.name}`}
        data-space-more={item.space.id}
        onClick={(e) => e.stopPropagation()}
        className={cn(
          "flex size-7 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground opacity-0 outline-none group-hover/row:opacity-100 group-hover/tile:opacity-100 hover:bg-tile hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-tile data-popup-open:text-foreground data-popup-open:opacity-100",
          className,
        )}
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
