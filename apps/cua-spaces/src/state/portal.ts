// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFleetLocally } from "../model/fleet";
import { FIXTURE_SPACES } from "../model/fixtures";
import { core } from "../core";
import { sortByMru } from "../model/mru";
import type { Fleet, FleetDraft, Space } from "../model/types";
import type { WindowMode } from "../native/types";

/**
 * Transient confirmation shown in the shelf. `kind` drives the icon/colour.
 */
export interface Notice {
  id: number;
  kind: "switch" | "create";
  text: string;
}

export interface PortalState {
  mode: WindowMode;
  spaces: Space[];
  fleets: Fleet[];
  /** Currently selected Space (the one the user is "in"). */
  selectedId: string;
  /** Keyboard focus inside the switcher, as an index into the MRU list; `-1` when on the "+ New" tile. */
  focusIndex: number;
  notice: Notice | null;
  /** Monotonic counter for notice ids. */
  noticeSeq: number;
}

export const DEFAULT_DRAFT: FleetDraft = {
  templateId: "qa-matrix",
  region: "us-central",
  size: "standard",
  autoSuspend: 15,
  count: 3,
  customOs: "linux",
};

export type PortalAction =
  | { type: "expand" }
  | { type: "collapse" }
  | { type: "open-create" }
  | { type: "cancel-create" }
  | { type: "focus"; index: number }
  | { type: "focus-move"; delta: 1 | -1 }
  | { type: "select"; id: string; now: number }
  | { type: "create-fleet"; draft: FleetDraft; now: number }
  | { type: "clear-notice"; id: number }
  /** Transient status line (e.g. "Creating a Space…" during a drag-to-New). */
  | { type: "notify"; text: string; kind?: "switch" | "create" }
  /** Replace the Space list with live registry data (keeps local MRU touches). */
  | { type: "sync-spaces"; spaces: Space[] };

export function initialState(spaces: readonly Space[] = FIXTURE_SPACES): PortalState {
  return { ...core<Omit<PortalState, "fleets">>("roster.initial", { spaces }), fleets: [] };
}

/** Index of the "+ New" tile in keyboard order (right after the last Space). */
export const NEW_TILE_INDEX = -1;

/**
 * Sentinel drop-target id for the "+ New" tile. Dragging an app onto it (as
 * opposed to an existing Space tile) creates a fresh Space and teleports the
 * dragged app into it. Not a real Space id, so it never resolves in `spaces`.
 */
export const NEW_SPACE_DROP_ID = "__new_space__";

/**
 * Advances the portal. Every action is the app core's Space list state
 * machine (`cua-spaces-app-core::spaces::roster`), except `create-fleet`:
 * the browser preview's synthetic fleets (`model/fleet.ts`), not app logic.
 */
export function reduce(state: PortalState, action: PortalAction): PortalState {
  if (action.type === "create-fleet") {
    const { fleet, spaces } = createFleetLocally(action.draft, state.spaces, action.now);
    const merged = sortByMru([...state.spaces, ...spaces]);
    const seq = state.noticeSeq + 1;
    return {
      ...state,
      mode: "switcher",
      spaces: merged,
      fleets: [...state.fleets, fleet],
      focusIndex: 0,
      noticeSeq: seq,
      notice: {
        id: seq,
        kind: "create",
        text: `${fleet.name} · starting ${spaces.length} ${spaces.length === 1 ? "Space" : "Spaces"}`,
      },
    };
  }
  const { fleets, ...roster } = state;
  const next = core<Omit<PortalState, "fleets">>("roster.reduce", { state: roster, action });
  return { ...next, fleets };
}
