// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Typed wrapper over the shell's AX window-drag commands and the `window-drag`
 * / `window-drag-permission` events. Outside Tauri every call is a no-op so the
 * browser/test build never touches native globals.
 *
 * Screen coordinates in `WindowDragEvent` are top-left-origin logical points
 * (matching `CGEvent` location and Tauri's window positions ÷ scale), so the
 * renderer can map them straight into a window's client space.
 */
import type { DragDisplay } from "../model/dragTrigger";
import { hasTauri } from "./bridge";
import type { LogicalRect } from "./types";

export type WindowDragPhase = "start" | "move" | "end";

export interface WindowDragEvent {
  phase: WindowDragPhase;
  x: number;
  y: number;
  /** CoreGraphics window id of the dragged window (start/end only), for the
   * drag-overlay ghost capture + hide/restore. */
  windowId?: number | null;
  /** The dragged app's catalog id (start/end only). */
  appId?: string | null;
  /** The dragged window's app display name. */
  appName?: string | null;
  /** The dragged window's title (empty without Screen Recording permission). */
  windowTitle?: string | null;
  /** Whether teleport can bring the dragged app up in a Space (full or
   * install only), from the SDK catalog. */
  supported?: boolean | null;
  /** The SDK's capability level for the dragged app. */
  capability?: "full" | "install_only" | "unsupported" | null;
  /** The dragged app's catalog entry (the SDK's core JSON). */
  entry?: Record<string, unknown> | null;
  /** The window's frame at mouse down (start only). */
  startFrame?: LogicalRect | null;
  /** The window's frame now (start, end and some moves): with
   * `startFrame`, tells a move from a resize. */
  frame?: LogicalRect | null;
}

export interface WindowDragBridge {
  readonly isNative: boolean;
  /** Whether the app holds the Accessibility permission. */
  axTrusted(): Promise<boolean>;
  /** Prompt for Accessibility (opens System Settings); returns trust after. */
  requestAxTrust(): Promise<boolean>;
  /** Install the global mouse monitor if permitted; returns whether active. */
  startWindowDrag(): Promise<boolean>;
  /**
   * Hide (`true`) or restore (`false`) a foreign window during the drag
   * teleport so its ghost can stand in for it. Resolves to whether the window's
   * visibility actually changed (false when Accessibility is withheld — the
   * caller then keeps the ghost only). No-op/false outside the native shell.
   */
  setForeignWindowHidden(windowId: number, hidden: boolean): Promise<boolean>;
  /** The drag trigger's geometry for every display (from each screen's safe
   * area); empty outside the native shell. */
  dragDisplays(): Promise<DragDisplay[]>;
  /** Subscribe to drag phases. Resolves to an unsubscribe function. */
  onWindowDrag(handler: (event: WindowDragEvent) => void): Promise<() => void>;
  /** Subscribe to permission changes (`true` once the monitor is active). */
  onPermission(handler: (granted: boolean) => void): Promise<() => void>;
}

const noopUnsub = async () => () => {};

export function createFallbackWindowDragBridge(): WindowDragBridge {
  return {
    isNative: false,
    axTrusted: async () => false,
    requestAxTrust: async () => false,
    startWindowDrag: async () => false,
    setForeignWindowHidden: async () => false,
    dragDisplays: async () => [],
    onWindowDrag: noopUnsub,
    onPermission: noopUnsub,
  };
}

export function createTauriWindowDragBridge(): WindowDragBridge {
  const core = import("@tauri-apps/api/core");
  const event = import("@tauri-apps/api/event");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    axTrusted: () => invoke<boolean>("ax_trusted"),
    requestAxTrust: () => invoke<boolean>("request_ax_trust"),
    startWindowDrag: () => invoke<boolean>("start_window_drag"),
    setForeignWindowHidden: (windowId, hidden) =>
      invoke<boolean>("set_foreign_window_hidden", { windowId, hidden }),
    dragDisplays: () => invoke<DragDisplay[]>("drag_trigger_displays"),
    onWindowDrag: async (handler) => {
      const { listen } = await event;
      return listen<WindowDragEvent>("window-drag", (e) => handler(e.payload));
    },
    onPermission: async (handler) => {
      const { listen } = await event;
      return listen<boolean>("window-drag-permission", (e) => handler(e.payload));
    },
  };
}

export function createWindowDragBridge(): WindowDragBridge {
  return hasTauri() ? createTauriWindowDragBridge() : createFallbackWindowDragBridge();
}

/**
 * Map a drag's screen point (top-left logical points) into the client-space CSS
 * pixel coordinates of a window whose top-left sits at `windowOrigin` (also
 * top-left logical points). Pure so the hit-test can be unit tested.
 */
export function screenToClient(
  point: { x: number; y: number },
  windowOrigin: { x: number; y: number },
): { x: number; y: number } {
  return { x: point.x - windowOrigin.x, y: point.y - windowOrigin.y };
}
