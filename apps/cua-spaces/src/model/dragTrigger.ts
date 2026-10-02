// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * When a dragged window opens the switcher (the app core's
 * `notch::drag_trigger`, which the SwiftUI notch runs too): move or resize,
 * the line 5 pt above the notch's bottom edge, the 0.3 s rest in the
 * "Teleport to Cua" box, staying open inside the panel and collapsing when
 * the cursor leaves. The portal feeds it the shell's `window-drag` events
 * and the time, and runs the `DragOverlayEvent`s it returns.
 */
import { core } from "../core";
import type { LogicalRect } from "../native/types";
import type { DragOverlayEvent } from "./dragOverlay";

/** What the shell reads about a screen (AppKit coordinates). */
export interface ScreenFacts {
  frame: LogicalRect;
  visibleFrame: LogicalRect;
  safeAreaTop: number;
  auxLeftWidth: number | null;
  auxRightWidth: number | null;
}

export type DragKind = "pending" | "move" | "resize";
export type TriggerPhase = "hidden" | "prompt" | "expanded";

/** One display's trigger geometry, global top-left points. */
export interface DragDisplay {
  frame: LogicalRect;
  notch: LogicalRect;
  prompt: LogicalRect;
  expanded: LogicalRect;
}

export interface DragTriggerState {
  active: boolean;
  kind: DragKind;
  phase: TriggerPhase;
  windowId: number | null;
  appName: string | null;
  startFrame: LogicalRect | null;
  display: number | null;
  still: { x: number; y: number; sinceMs: number } | null;
}

export type DragTriggerEvent =
  | {
      type: "start";
      windowId: number | null;
      appName: string | null;
      x: number;
      y: number;
      tMs: number;
      startFrame: LogicalRect | null;
      frame: LogicalRect | null;
    }
  | { type: "cursor"; x: number; y: number; tMs: number }
  | { type: "frame"; frame: LogicalRect }
  | { type: "tick"; tMs: number }
  | { type: "end"; x: number; y: number; tMs: number }
  | { type: "cancel" };

export interface DragTriggerTransition {
  state: DragTriggerState;
  /** Run these on the drag overlay, in order. */
  overlay: DragOverlayEvent[];
  /** Send a `tick` at this time (replacing any earlier one); null cancels. */
  tickAtMs: number | null;
}

export function initialDragTrigger(): DragTriggerState {
  return core("dragTrigger.initial", {});
}

export function applyDragTrigger(
  state: DragTriggerState,
  event: DragTriggerEvent,
  displays: readonly DragDisplay[],
): DragTriggerTransition {
  return core("dragTrigger.apply", { state, event, displays });
}

/** A window's frame at mouse down against a later one. */
export function classifyDrag(start: LogicalRect, now: LogicalRect): DragKind {
  return core("dragTrigger.classify", { start, now });
}

/** Whether a `window-drag` start is a resize (its frames say so). */
export function isResize(e: { startFrame?: LogicalRect | null; frame?: LogicalRect | null }): boolean {
  return Boolean(e.startFrame && e.frame) && classifyDrag(e.startFrame!, e.frame!) === "resize";
}

/**
 * The display a monitor (global top-left points) is on, from the shell's
 * list: the portal opens on its own monitor only.
 */
export function displayFor(displays: readonly DragDisplay[], monitor: LogicalRect): DragDisplay[] {
  const cx = monitor.x + monitor.width / 2;
  const cy = monitor.y + monitor.height / 2;
  const d = displays.find(
    ({ frame: f }) => cx >= f.x && cx < f.x + f.width && cy >= f.y && cy < f.y + f.height,
  );
  return d ? [d] : [];
}

/** The trigger geometry for screens (the primary first), for the portal. */
export function portalDragDisplays(screens: readonly ScreenFacts[]): DragDisplay[] {
  return core("dragTrigger.portalDisplays", { screens });
}

/**
 * The portal's monitor (global top-left points) as its only display when
 * the shell cannot read the screens: a menu-bar-high notch stands in.
 */
export function fallbackDragDisplays(monitor: LogicalRect): DragDisplay[] {
  // As the primary display in AppKit coordinates, flipped back to `monitor.y`.
  const frame = { ...monitor, y: -monitor.y };
  return portalDragDisplays([
    { frame, visibleFrame: frame, safeAreaTop: 0, auxLeftWidth: null, auxRightWidth: null },
  ]);
}
