// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * How a Space's page is split between the desktop and the thread, and which
 * side-panel sections are folded. Pure rules; the saved state is
 * `stores/space-layout.ts`.
 *
 * The desktop sits on top of the thread (`top`, the default) or docked to the
 * left or right of it. Dragging the divider resizes; dragging it to the left
 * or right edge docks the desktop there, and dragging a docked divider to the
 * top returns to the vertical split.
 */

export type Dock = "top" | "left" | "right";

/** The side panel's sections that fold. */
export type SectionId = "people" | "children" | "stream" | "details";
export const SECTIONS: readonly SectionId[] = ["people", "children", "stream", "details"];
export type FoldedSections = Record<SectionId, boolean>;

export interface SpaceLayoutPrefs {
  dock: Dock;
  /** The desktop's share of the column height when it is on top. */
  vSplit: number;
  /** The desktop's share of the column width when it is docked. */
  hSplit: number;
  /** Sections folded shut. */
  folded: FoldedSections;
}

export const DEFAULT_V_SPLIT = 0.52;
export const DEFAULT_H_SPLIT = 0.58;

/** Neither pane gets smaller than this (px). */
export const MIN_STAGE_H = 160;
export const MIN_BELOW_H = 200;
export const MIN_STAGE_W = 260;
export const MIN_BELOW_W = 340;

/** Below this column width (px) the desktop is always on top: side by side would leave both panes cramped. */
export const DOCK_MIN_WIDTH = MIN_STAGE_W + MIN_BELOW_W + 40;

export const DEFAULT_PREFS: SpaceLayoutPrefs = {
  dock: "top",
  vSplit: DEFAULT_V_SPLIT,
  hSplit: DEFAULT_H_SPLIT,
  folded: { people: false, children: false, stream: false, details: false },
};

/** `fraction` of `extent` for the first pane, kept inside both minimums. */
export function clampFraction(fraction: number, extent: number, minFirst: number, minSecond: number): number {
  if (!Number.isFinite(fraction)) return 0.5;
  if (extent <= minFirst + minSecond) return 0.5;
  return Math.min(1 - minSecond / extent, Math.max(minFirst / extent, fraction));
}

/** The dock that applies in a column `width` px wide: a narrow column never docks. */
export function effectiveDock(dock: Dock, width: number): Dock {
  return dock !== "top" && width > 0 && width < DOCK_MIN_WIDTH ? "top" : dock;
}

/** The clamp for the dock's own axis. */
export function clampSplit(dock: Dock, fraction: number, width: number, height: number): number {
  return dock === "top" ? clampFraction(fraction, height, MIN_STAGE_H, MIN_BELOW_H) : clampFraction(fraction, width, MIN_STAGE_W, MIN_BELOW_W);
}

/** The desktop's share of the axis `dock` splits, from the saved prefs. */
export function splitOf(prefs: Pick<SpaceLayoutPrefs, "vSplit" | "hSplit">, dock: Dock): number {
  return dock === "top" ? prefs.vSplit : prefs.hSplit;
}

/** A pointer this close to the left or right edge (share of the column width) docks the desktop there, and has to move this far to let go. */
export const EDGE_ENTER = 0.12;
export const EDGE_EXIT = 0.22;
/** A docked divider dragged this close to the top returns to the vertical split, and has to move this far down to stay docked. */
export const TOP_ENTER = 0.1;
export const TOP_EXIT = 0.2;

/**
 * Where a drag would leave the desktop if it ended now.
 *
 * `current` is the dock when the drag began, `previous` the answer for the
 * last pointer position, `x` and `y` the pointer as a share of the column
 * (0 to 1). Entering a zone takes less travel than leaving it (hysteresis), so
 * a pointer on the edge of a zone does not flicker the preview. Left and right
 * are reached through the vertical split, never directly.
 */
export function decideDock(current: Dock, previous: Dock, x: number, y: number): Dock {
  if (current === "top") {
    if (previous === "left") return x < EDGE_EXIT ? "left" : "top";
    if (previous === "right") return x > 1 - EDGE_EXIT ? "right" : "top";
    if (x < EDGE_ENTER) return "left";
    if (x > 1 - EDGE_ENTER) return "right";
    return "top";
  }
  if (previous === "top") return y < TOP_EXIT ? "top" : current;
  return y < TOP_ENTER ? "top" : current;
}

const STORAGE_KEY = "cua-spaces:space-layout";

const isDock = (v: unknown): v is Dock => v === "top" || v === "left" || v === "right";
const isFraction = (v: unknown): v is number => typeof v === "number" && v > 0 && v < 1;

/** Saved prefs from whatever was stored: anything unreadable falls back to its default. */
export function sanitizePrefs(raw: unknown): SpaceLayoutPrefs {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const f = (r.folded && typeof r.folded === "object" ? r.folded : {}) as Record<string, unknown>;
  return {
    dock: isDock(r.dock) ? r.dock : DEFAULT_PREFS.dock,
    vSplit: isFraction(r.vSplit) ? r.vSplit : DEFAULT_V_SPLIT,
    hSplit: isFraction(r.hSplit) ? r.hSplit : DEFAULT_H_SPLIT,
    folded: { people: f.people === true, children: f.children === true, stream: f.stream === true, details: f.details === true },
  };
}

/** Where the one saved split lived before the layout kept dock, sizes and folds together: the desktop's share of the column. */
export const LEGACY_SPLIT_KEY = "cua-spaces:call-split";

/**
 * Carries a split saved under [`LEGACY_SPLIT_KEY`] into the layout's own saved copy, so an upgrade keeps what the person
 * dragged. Only when no layout has been saved yet (a saved layout is newer and wins), and only a usable fraction. The old
 * key is left for the first-run reset to clear.
 */
export function migrateLegacySplit(storage: Pick<Storage, "getItem" | "setItem">): void {
  try {
    if (storage.getItem(STORAGE_KEY) !== null) return;
    const raw = storage.getItem(LEGACY_SPLIT_KEY);
    if (raw === null) return;
    const v = Number(raw);
    if (!isFraction(v)) return;
    storage.setItem(STORAGE_KEY, JSON.stringify({ state: { ...DEFAULT_PREFS, vSplit: v }, version: 0 }));
  } catch {
    // Blocked storage: nothing to carry over.
  }
}

export { STORAGE_KEY as SPACE_LAYOUT_STORAGE_KEY };
