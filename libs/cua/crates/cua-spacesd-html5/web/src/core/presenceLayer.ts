// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The presence cursors over a stream surface, framework-free, shared by the
 * cua-spacesd HTML5 viewer and the Cua Spaces app (which imports this core
 * through its `@cua/spacesd-html5/core` alias).
 *
 * The model is the SDK's (`@trycua/cua/spaces/presence`: `PresenceView`
 * interpolation, idle fade, heartbeat staleness) and the art is the SDK's
 * shared cursor set (`cursorArtSvg`). This file only places DOM:
 *
 * - your own cursor is drawn at the local pointer inside the pointer event
 *   handler, with no network in between, in your presence color and with the
 *   shape the server reports for you; the system cursor is hidden only while
 *   the pointer is over the video;
 * - everyone else is drawn where `PresenceView.drawables` says, every
 *   animation frame, with a name pill;
 * - the local pointer is handed to `onPointer` at most every 33 ms (newest
 *   wins, the last position of a movement always goes out) and hides go out
 *   at once.
 *
 * The layer never changes how input reaches the Space: its listeners are
 * passive and only read positions.
 */

import { cursorArt, cursorArtSvg, presenceTextColor, type PresenceView } from "@trycua/cua/spaces/presence";

/** Drawn cursor size in CSS pixels (the art canvas is 32). */
export const CURSOR_SIZE = 24;
/** Outbound pointer interval (ms). */
export const SEND_INTERVAL_MS = 33;

export interface PresencePointerUpdate {
  x: number;
  y: number;
  visible: boolean;
  windowId?: string;
}

export interface PresenceLayerOptions {
  /** The stream canvas the cursors sit on. */
  surface: HTMLCanvasElement;
  /** Where the layer's root element goes (usually the surface's parent). */
  host: HTMLElement;
  /** The model, or null while joining or when the Space has no presence. */
  view: () => PresenceView | null;
  /** The caller's participant id and color, once joined. */
  me: () => { id: string; color: string } | null;
  /** The window this surface streams; unset for a display. */
  windowId?: string;
  /** Publish and draw the local pointer (false for view-only mirrors). */
  interactive: boolean;
  /** Whether the stream under the surface is live (default: always). A
   * stream that failed or ended draws no cursor of yours. */
  live?: () => boolean;
  /** Receives the local pointer, throttled. */
  onPointer?: (update: PresencePointerUpdate) => void;
  /** Clock in Unix milliseconds (tests). */
  now?: () => number;
  /** Frame scheduler (tests pass a manual one). */
  requestFrame?: (cb: () => void) => number;
  cancelFrame?: (id: number) => void;
}

/**
 * Whether your own cursor is drawn over a stream, and so the system cursor
 * hidden there: presence joined, the stream live with a real size, and the
 * pointer over the picture. Anything else keeps the system cursor; the two
 * are never shown together.
 */
export function drawsOwnCursor(s: { joined: boolean; live: boolean; width: number; height: number; inside: boolean }): boolean {
  return s.joined && s.live && s.width > 0 && s.height > 0 && s.inside;
}

interface ContentRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The video's rect inside a canvas (letterboxed under `object-fit: contain`). */
export function surfaceContentRect(canvas: HTMLCanvasElement): ContentRect {
  const r = canvas.getBoundingClientRect();
  const iw = canvas.width;
  const ih = canvas.height;
  const fit = typeof getComputedStyle === "function" ? getComputedStyle(canvas).objectFit : "";
  if (fit !== "contain" || iw <= 0 || ih <= 0 || r.width <= 0 || r.height <= 0) {
    return { left: r.left, top: r.top, width: r.width, height: r.height };
  }
  const scale = Math.min(r.width / iw, r.height / ih);
  const width = iw * scale;
  const height = ih * scale;
  return { left: r.left + (r.width - width) / 2, top: r.top + (r.height - height) / 2, width, height };
}

/** One cursor element: the shared art in `color`, plus an optional name pill. */
export function renderCursor(el: HTMLElement, shape: string, color: string, name: string | null): void {
  const key = `${shape}|${color}|${name ?? ""}`;
  if (el.dataset.key === key) return;
  el.dataset.key = key;
  el.dataset.shape = shape;
  const art = cursorArt(shape);
  const scale = CURSOR_SIZE / 32;
  const svg = cursorArtSvg(shape, color, CURSOR_SIZE);
  const hx = art.hotspot[0] * scale;
  const hy = art.hotspot[1] * scale;
  let pill = "";
  if (name) {
    const text = presenceTextColor(color);
    const safe = name.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c] as string);
    pill =
      `<span class="presence-cursor-name" style="position:absolute;left:${Math.round(CURSOR_SIZE - hx)}px;top:${Math.round(CURSOR_SIZE - hy)}px;` +
      `padding:1px 6px;border-radius:7px;background:${color};color:${text};font:600 11px/15px -apple-system,BlinkMacSystemFont,system-ui,sans-serif;` +
      `white-space:nowrap">${safe}</span>`;
  }
  el.innerHTML =
    `<span class="presence-cursor-art" style="position:absolute;left:${-hx}px;top:${-hy}px;width:${CURSOR_SIZE}px;height:${CURSOR_SIZE}px;` +
    `filter:drop-shadow(0 1px 1px rgba(0,0,0,0.35))">${svg}</span>${pill}`;
}

function cursorElement(id: string, isMe: boolean): HTMLElement {
  const el = document.createElement("div");
  el.className = isMe ? "presence-cursor presence-cursor-me" : "presence-cursor";
  el.dataset.participant = id;
  if (isMe) el.dataset.me = "true";
  el.style.cssText = "position:absolute;left:0;top:0;width:0;height:0;pointer-events:none;will-change:transform";
  return el;
}

export class PresenceLayer {
  readonly root: HTMLElement;
  private readonly opts: PresenceLayerOptions;
  private readonly now: () => number;
  private readonly cursors = new Map<string, HTMLElement>();
  private own: HTMLElement | null = null;
  private frame: number | undefined;
  private pointer: { clientX: number; clientY: number } | null = null;
  private lastSent = Number.NEGATIVE_INFINITY;
  private lastVisible = false;
  private hidingSystemCursor = false;
  private pending: PresencePointerUpdate | null = null;
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private destroyed = false;
  private readonly onMove = (e: PointerEvent) => this.pointerAt(e.clientX, e.clientY);
  private readonly onLeave = () => this.pointerAt(null, null);
  private readonly onBlur = () => this.pointerAt(null, null);

  constructor(opts: PresenceLayerOptions) {
    this.opts = opts;
    this.now = opts.now ?? Date.now;
    this.root = document.createElement("div");
    this.root.className = "presence-layer";
    this.root.style.cssText = "position:absolute;left:0;top:0;width:0;height:0;overflow:visible;pointer-events:none;z-index:6";
    opts.host.appendChild(this.root);
    if (opts.interactive) {
      opts.surface.addEventListener("pointermove", this.onMove, { passive: true });
      opts.surface.addEventListener("pointerdown", this.onMove, { passive: true });
      opts.surface.addEventListener("pointerleave", this.onLeave, { passive: true });
      // A window without focus gets no pointer moves to follow.
      if (typeof window !== "undefined") window.addEventListener("blur", this.onBlur);
    }
    this.schedule();
  }

  /** The local pointer moved to a viewport point, or left (null). */
  pointerAt(clientX: number | null, clientY: number | null): void {
    if (this.destroyed || !this.opts.interactive) return;
    const rect = surfaceContentRect(this.opts.surface);
    let inside: { x: number; y: number } | null = null;
    if (clientX !== null && clientY !== null && rect.width > 0 && rect.height > 0) {
      const x = (clientX - rect.left) / rect.width;
      const y = (clientY - rect.top) / rect.height;
      if (x >= 0 && y >= 0 && x <= 1 && y <= 1) inside = { x, y };
    }
    this.pointer = clientX !== null && clientY !== null && inside ? { clientX, clientY } : null;
    this.syncOwn();
    if (inside) this.send({ x: inside.x, y: inside.y, visible: true, ...this.target() });
    else if (this.lastVisible) this.send({ x: 0, y: 0, visible: false, ...this.target() });
  }

  private target(): { windowId?: string } {
    return this.opts.windowId ? { windowId: this.opts.windowId } : {};
  }

  private send(update: PresencePointerUpdate): void {
    const now = this.now();
    const edge = update.visible !== this.lastVisible;
    if (edge || now - this.lastSent >= SEND_INTERVAL_MS) {
      this.pending = null;
      this.lastSent = now;
      this.lastVisible = update.visible;
      this.opts.onPointer?.(update);
      return;
    }
    this.pending = update;
    if (this.flushTimer === undefined) {
      this.flushTimer = setTimeout(() => {
        this.flushTimer = undefined;
        const p = this.pending;
        this.pending = null;
        if (!p || this.destroyed) return;
        this.lastSent = this.now();
        this.lastVisible = p.visible;
        this.opts.onPointer?.(p);
      }, Math.max(0, SEND_INTERVAL_MS - (now - this.lastSent)));
    }
  }

  /** Draws your cursor and hides the system one, or neither
   * ([`drawsOwnCursor`]); re-checked every frame, so a stream that is lost,
   * shrinks to nothing or loses presence restores the system cursor. */
  private syncOwn(): void {
    if (!this.opts.interactive) return;
    const rect = surfaceContentRect(this.opts.surface);
    const shown =
      this.pointer !== null &&
      drawsOwnCursor({
        joined: this.opts.me() !== null,
        live: this.opts.live?.() ?? true,
        width: rect.width,
        height: rect.height,
        inside: true,
      });
    if (shown) this.drawOwn();
    else if (this.own) this.own.style.display = "none";
    if (shown !== this.hidingSystemCursor) {
      this.hidingSystemCursor = shown;
      this.opts.surface.style.cursor = shown ? "none" : "";
    }
  }

  private drawOwn(): void {
    const me = this.opts.me();
    if (!me || !this.pointer) return;
    if (!this.own) {
      this.own = cursorElement(me.id, true);
      this.root.appendChild(this.own);
    }
    const shape = this.opts.view()?.shapeOf(me.id) ?? "arrow";
    renderCursor(this.own, shape, me.color, null);
    const origin = this.root.getBoundingClientRect();
    this.own.style.display = "";
    this.own.style.transform = `translate(${this.pointer.clientX - origin.left}px, ${this.pointer.clientY - origin.top}px)`;
  }

  /** Draws every cursor for `now` (called each animation frame). */
  render(now: number = this.now()): void {
    if (this.destroyed) return;
    const view = this.opts.view();
    const me = this.opts.me();
    this.syncOwn();
    const seen = new Set<string>();
    if (view) {
      const rect = surfaceContentRect(this.opts.surface);
      const origin = this.root.getBoundingClientRect();
      for (const d of view.drawables(now)) {
        if (d.isMe || d.participantId === me?.id) continue;
        const onThis = this.opts.windowId ? d.windowId === this.opts.windowId : !d.windowId;
        if (!onThis) continue;
        seen.add(d.participantId);
        let el = this.cursors.get(d.participantId);
        if (!el) {
          el = cursorElement(d.participantId, false);
          this.cursors.set(d.participantId, el);
          this.root.appendChild(el);
        }
        renderCursor(el, d.shape, d.color, d.displayName || null);
        const px = rect.left - origin.left + d.x * rect.width;
        const py = rect.top - origin.top + d.y * rect.height;
        el.style.transform = `translate(${px}px, ${py}px)`;
        el.style.opacity = String(d.alpha);
      }
    }
    for (const [id, el] of this.cursors) {
      if (!seen.has(id)) {
        el.remove();
        this.cursors.delete(id);
      }
    }
  }

  private schedule(): void {
    const raf = this.opts.requestFrame ?? (typeof requestAnimationFrame === "function" ? requestAnimationFrame : null);
    if (!raf) return;
    const tick = () => {
      if (this.destroyed) return;
      this.render();
      this.frame = raf(tick);
    };
    this.frame = raf(tick);
  }

  destroy(): void {
    if (this.destroyed) return;
    if (this.lastVisible) this.opts.onPointer?.({ x: 0, y: 0, visible: false, ...this.target() });
    this.destroyed = true;
    const cancel = this.opts.cancelFrame ?? (typeof cancelAnimationFrame === "function" ? cancelAnimationFrame : null);
    if (this.frame !== undefined && cancel) cancel(this.frame);
    if (this.flushTimer !== undefined) clearTimeout(this.flushTimer);
    this.opts.surface.removeEventListener("pointermove", this.onMove);
    this.opts.surface.removeEventListener("pointerdown", this.onMove);
    this.opts.surface.removeEventListener("pointerleave", this.onLeave);
    if (typeof window !== "undefined") window.removeEventListener("blur", this.onBlur);
    this.opts.surface.style.cursor = "";
    this.root.remove();
  }
}
