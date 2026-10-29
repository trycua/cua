// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Space, SpaceAccess } from "@/bridge";
import { isMachineDesktop, isUnsharedDesktop, type SpaceState } from "@/lib/spaces";

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
