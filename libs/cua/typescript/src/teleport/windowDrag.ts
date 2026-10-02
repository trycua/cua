/**
 * Dragging a real app window onto a Space: the drop-zone state every client
 * renders (the dragged window's preview, its app, the Space under the
 * cursor). Events come from the SDK's window-drag monitor
 * (`Teleport.startWindowDrag`, or a host relaying it); the preview is the
 * SDK's in-memory thumbnail of that one window.
 */

import type { CatalogEntry } from "./model.js"

export interface DraggedWindow {
  windowId: number
  appName: string
  title: string
  bundlePath?: string | null
}

/** One monitor event (global top-left display points). */
export interface WindowDragEvent {
  phase: "start" | "move" | "end"
  x: number
  y: number
  window?: DraggedWindow | null
  app?: CatalogEntry | null
}

export interface WindowDropState {
  active: boolean
  window: DraggedWindow | null
  app: CatalogEntry | null
  /** The dragged window's preview (`data:` URL), when captured. */
  thumbnail: string | null
  /** The drop target under the cursor. */
  overId: string | null
  x: number
  y: number
}

export type WindowDropEvent =
  | WindowDragEvent
  | { phase: "thumbnail"; url: string | null }
  | { phase: "over"; id: string | null }

export const idleWindowDrop: WindowDropState = {
  active: false,
  window: null,
  app: null,
  thumbnail: null,
  overId: null,
  x: 0,
  y: 0,
}

/** What a release commits: teleport `app` into `targetId`. */
export interface WindowDropCommit {
  targetId: string
  app: CatalogEntry
  window: DraggedWindow
}

/**
 * The reducer. Only apps teleport can bring up start a drop (an
 * unsupported app's window drag is ignored, as before). `end` returns the
 * commit when released over a target.
 */
export function reduceWindowDrop(
  s: WindowDropState,
  e: WindowDropEvent,
): { state: WindowDropState; commit: WindowDropCommit | null; capture: number | null } {
  switch (e.phase) {
    case "start": {
      if (!e.window || !e.app || e.app.capability === "unsupported") {
        return { state: idleWindowDrop, commit: null, capture: null }
      }
      return {
        state: { active: true, window: e.window, app: e.app, thumbnail: null, overId: null, x: e.x, y: e.y },
        commit: null,
        capture: e.window.windowId,
      }
    }
    case "move":
      return { state: s.active ? { ...s, x: e.x, y: e.y } : s, commit: null, capture: null }
    case "thumbnail":
      return { state: s.active ? { ...s, thumbnail: e.url } : s, commit: null, capture: null }
    case "over":
      return { state: s.active ? { ...s, overId: e.id } : s, commit: null, capture: null }
    case "end": {
      const commit =
        s.active && s.overId && s.app && s.window ? { targetId: s.overId, app: s.app, window: s.window } : null
      return { state: idleWindowDrop, commit, capture: null }
    }
  }
}

/** Screen point to client point, given the viewport's screen origin. */
export function screenToClient(x: number, y: number, origin: { x: number; y: number }): { x: number; y: number } {
  return { x: x - origin.x, y: y - origin.y }
}

/**
 * A browser viewport's screen origin (approximate: assumes the browser
 * chrome sits above the page, as in desktop browsers).
 */
export function browserViewportOrigin(w: {
  screenX: number
  screenY: number
  outerWidth: number
  innerWidth: number
  outerHeight: number
  innerHeight: number
}): { x: number; y: number } {
  const side = Math.max(0, (w.outerWidth - w.innerWidth) / 2)
  return { x: w.screenX + side, y: w.screenY + Math.max(0, w.outerHeight - w.innerHeight - side) }
}

/** The closest `[attr]` element's value at a client point, or null. */
export function targetAt(
  doc: { elementFromPoint(x: number, y: number): Element | null },
  x: number,
  y: number,
  attr = "data-space-id",
): string | null {
  const el = doc.elementFromPoint(x, y)
  const hit = el?.closest(`[${attr}]`)
  return hit?.getAttribute(attr) ?? null
}

/** PNG bytes (from the SDK) as a `data:` URL for an `<img>`. */
export function pngDataUrl(bytes: Uint8Array | ArrayBuffer | readonly number[]): string {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes as ArrayBuffer)
  let bin = ""
  for (let i = 0; i < u8.length; i += 0x8000) bin += String.fromCharCode(...u8.subarray(i, i + 0x8000))
  return `data:image/png;base64,${btoa(bin)}`
}
