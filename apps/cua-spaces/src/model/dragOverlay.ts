// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";

/**
 * Drag teleport state machine (macOS drag interaction, item A).
 *
 * A user drags another app's window (tracked globally by the CGEventTap in
 * `window_drag.rs`) toward the notch. This pure reducer drives the two
 * mutually-exclusive surfaces of the interaction and decides when to capture a
 * "ghost" preview of the dragged window and when to commit the teleport.
 *
 * The real foreign window is *never* moved, hidden, or repositioned: it stays
 * exactly where the user drags it. The ghost is a purely additive visual echo
 * rendered inside our own UI.
 *
 * The surfaces both live in the ambient portal itself — there is no separate
 * floating overlay window. The notch morphs in place:
 *   idle     → nothing showing (the ambient "N Spaces" tab).
 *   prompt   → the ambient "N Spaces" tab has collapsed into the notch and the
 *              notch has grown taller into a #000 "Teleport to Cua" box (the
 *              drag started but the window is not yet up at the notch). The big
 *              Space selector is hidden.
 *   selector → the window has been brought up to the notch box, so the box
 *              gives way and the full Space selector (switcher drop-mode)
 *              expands with the Space tiles as drop targets.
 *
 * Because the gesture is a *foreign* window drag, our surfaces never receive the
 * mouse; the portal hit-tests the global cursor against the notch zone and its
 * own tiles and feeds `enter-notch` / `leave-notch` / `over` / `out` / `drop`
 * here. The only side effects are the ghost capture and the final commit, kept
 * declarative so the whole idle → prompt → selector → commit/dismiss sequence is
 * unit-testable without React or Tauri.
 */

export type DragOverlayPhase = "idle" | "prompt" | "selector";

export interface DragOverlayState {
  phase: DragOverlayPhase;
  /** CoreGraphics window id of the dragged window (for the ghost capture). */
  windowId: number | null;
  /** Display name of the dragged app, for the captions. */
  appName: string | null;
  /** Space tile currently under the cursor while in `selector`; null otherwise. */
  targetSpaceId: string | null;
  /** Captured window image (data URL) once the ghost has been grabbed. */
  ghost: string | null;
}

export const initialDragOverlay: DragOverlayState = {
  phase: "idle",
  windowId: null,
  appName: null,
  targetSpaceId: null,
  ghost: null,
};

export type DragOverlayEvent =
  | { type: "start"; windowId: number | null; appName: string | null }
  /** Cursor entered the notch box hit area: expand into the Space selector. */
  | { type: "enter-notch" }
  /** Cursor left the selector region: collapse back to just the notch box. */
  | { type: "leave-notch" }
  /** Cursor moved onto a Space drop target (tile id); selector phase only. */
  | { type: "over"; spaceId: string }
  /** Cursor left every drop target; selector phase only. */
  | { type: "out" }
  /** The async window capture finished (null when it could not be grabbed). */
  | { type: "ghost-ready"; ghost: string | null }
  /** Drag released; `spaceId` is the tile under the cursor at release, if any. */
  | { type: "drop"; spaceId: string | null }
  /** Abort the whole interaction (e.g. permission lost). */
  | { type: "cancel" };

/**
 * A side effect the caller must perform after a transition. Kept declarative so
 * transitions stay pure and testable. The real window is never touched, so the
 * only effects are the additive ghost capture and the final commit.
 */
export type DragOverlayEffect =
  /** Capture the dragged window's image for the ghost (needs Screen Recording;
   * a null result just means the ghost falls back to a placeholder). */
  | { kind: "capture"; windowId: number | null }
  /** Commit the teleport to this Space (begins the transfer, item B). */
  | { kind: "commit"; spaceId: string };

export interface DragOverlayTransition {
  state: DragOverlayState;
  effects: DragOverlayEffect[];
}

/**
 * Advance the machine and report the side effects to run (the app core's
 * `teleport::drag`). Invalid events leave the state unchanged.
 */
export function applyDragOverlay(
  state: DragOverlayState,
  event: DragOverlayEvent,
): DragOverlayTransition {
  return core("drag.apply", { state, event });
}
