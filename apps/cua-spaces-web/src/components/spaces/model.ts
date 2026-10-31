// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Space, SpaceAccess } from "@/bridge";
import { isMachineDesktop, isThisMacDesktop, isUnsharedDesktop, type SpaceState } from "@/lib/spaces";

/** One Space as the Spaces page draws it (a tile or a list row). */
export interface SpaceItem {
  space: Space;
  state: SpaceState;
  access: SpaceAccess;
  /** Yours (shared or not). */
  mine: boolean;
  /** Its size on its computer's disk, when known (yours only). */
  bytes: number | null;
  /** The computer it runs on, by name (filter box and fallbacks). */
  machine: string;
  /** A machine's own desktop, listed beside the Spaces: `shared` while it is shared, `off` ("Not sharing") on This Mac while its switch is off. */
  desktop?: "shared" | "off";
}

/** How a list row is drawn as a desktop (`undefined`: a Space). */
export function desktopOf(space: Space): SpaceItem["desktop"] {
  if (!isMachineDesktop(space)) return undefined;
  return isUnsharedDesktop(space) ? "off" : "shared";
}

/** What a desktop row says in place of the core's state word: "Not sharing", or "Sharing" for This Mac's own (another machine's says "Running"). */
export function desktopWord(item: Pick<SpaceItem, "desktop" | "space">): string | undefined {
  if (item.desktop === "off") return "Not sharing";
  return item.desktop === "shared" && item.space.id === "this-mac" ? "Sharing" : undefined;
}

/** Whether the row has its "⋯" menu: a Space, and another machine's shared desktop. Not This Mac's desktop, and not a desktop that is not shared. */
export const hasMenu = (item: Pick<SpaceItem, "desktop" | "space">): boolean => !item.desktop || (item.desktop === "shared" && !isThisMacDesktop(item.space));

/** Whether the row has the hover Hide button: This Mac's desktop (another machine's desktop hides from its menu). */
export const hasHideButton = (item: Pick<SpaceItem, "desktop" | "space">): boolean => Boolean(item.desktop) && isThisMacDesktop(item.space);

/** Whether the notch can keep it: a Space that exists, and another machine's desktop. Not This Mac, and not one still being created. */
export const canPinToNotch = (item: Pick<SpaceItem, "desktop" | "space" | "state">): boolean =>
  item.state !== "creating" && !item.space.id.startsWith("pending:") && (!item.desktop || (item.desktop === "shared" && !isThisMacDesktop(item.space)));
