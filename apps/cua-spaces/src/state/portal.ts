// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { FIXTURE_SPACES } from "../model/fixtures";
import { core } from "../core";
import type { Space } from "../model/types";
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
  /** Currently selected Space (the one the user is "in"). */
  selectedId: string;
  /** Keyboard focus inside the switcher, as an index into the MRU list; `-1` when on the "+ New" tile. */
  focusIndex: number;
  notice: Notice | null;
  /** Monotonic counter for notice ids. */
  noticeSeq: number;
}

export type PortalAction =
  | { type: "expand" }
  | { type: "collapse" }
  | { type: "open-create" }
  | { type: "cancel-create" }
  | { type: "focus"; index: number }
  | { type: "focus-move"; delta: 1 | -1 }
  | { type: "select"; id: string; now: number }
  | { type: "clear-notice"; id: number }
  /** Transient status line (e.g. "Creating a Space…" during a drag-to-New). */
  | { type: "notify"; text: string; kind?: "switch" | "create" }
  /** Replace the Space list with live registry data (keeps local MRU touches). */
  | { type: "sync-spaces"; spaces: Space[] };

export function initialState(spaces: readonly Space[] = FIXTURE_SPACES): PortalState {
  return core<PortalState>("roster.initial", { spaces });
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
 * machine (`cua-spaces-app-core::spaces::roster`).
 */
export function reduce(state: PortalState, action: PortalAction): PortalState {
  return core<PortalState>("roster.reduce", { state, action });
}
