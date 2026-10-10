// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import {
  nativeVideoHandler,
  type SurfacePhase,
  type SurfaceRect,
  type SurfaceState,
  type SurfaceTier,
  type VideoHandler,
} from "./stream-surface";

/** Whole CSS px, so sub-pixel layout jitter does not resend a rect. */
export function roundRect(r: SurfaceRect): SurfaceRect {
  return { x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) };
}

/** The overlap of two rects, or null when they don't overlap. */
export function intersect(a: SurfaceRect, b: SurfaceRect): SurfaceRect | null {
  const x = Math.max(a.x, b.x);
  const y = Math.max(a.y, b.y);
  const right = Math.min(a.x + a.width, b.x + b.width);
  const bottom = Math.min(a.y + a.height, b.y + b.height);
  return right > x && bottom > y ? { x, y, width: right - x, height: bottom - y } : null;
}

function sameRect(a: SurfaceRect | null, b: SurfaceRect | null): boolean {
  if (a === null || b === null) return a === b;
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/** Whether a surface report changes anything the host draws. */
export function sameSurface(a: SurfaceState | null, b: SurfaceState): boolean {
  return (
    a !== null &&
    a.spaceId === b.spaceId &&
    a.tier === b.tier &&
    a.interactive === b.interactive &&
    a.visible === b.visible &&
    a.occluded === b.occluded &&
    a.radius === b.radius &&
    sameRect(a.rect, b.rect) &&
    sameRect(a.clip, b.clip)
  );
}

/** Page UI that covers video when it overlaps a slot (portals: dialogs, menus, popovers, toasts). */
export const OCCLUDER_SELECTOR = [
  "[data-video-occluder]",
  '[role="dialog"]',
  '[role="alertdialog"]',
  '[role="menu"]',
  '[role="listbox"]',
  '[role="tooltip"]',
  "[data-toast]",
].join(",");

/**
 * Reports every mounted video slot to the host in one message per frame
 * (see `stream-surface.ts` for the protocol).
 *
 * One reporter per page: one set of observers, one animation frame, one
 * `surfaces` message carrying only what changed, so the host moves all its
 * layers in one Core Animation transaction.
 *
 * A slot is measured on anything that can move, clip or cover it: its own
 * or the page's resize, a scroll in any container (capture), a window
 * resize or zoom (`devicePixelRatio`), the page being hidden, and DOM
 * changes (dialogs, menus and toasts mount in portals). For a short while
 * after each of those it is measured every frame, so a CSS transition (a
 * dialog fading in, a sidebar sliding) is followed to its end; while any
 * slot is mounted a slow poll catches anything else.
 */

export interface SlotOptions {
  spaceId: string;
  tier: SurfaceTier;
  interactive: boolean;
  /** The slot's corner radius, CSS px. */
  radius: number;
  /** Its border width: the video goes inside it. */
  inset?: number;
  /** The Space's OS, where the page decodes the video itself (a Mac's ⌘
   * goes to a Linux or Windows Space as Control). */
  os?: string;
  /** One window of the Space instead of its desktop (picture in picture,
   * where the page decodes the video itself), with its epoch. */
  windowId?: string;
  epoch?: number;
}

export interface SlotStatus {
  phase: SurfacePhase;
  /** Why, when it failed. */
  reason?: string;
  /** It failed before a stream existed: the host could not open one (the
   * SwiftUI app's `streamProvider` threw; no ticket came). The viewer says
   * so once, as the SwiftUI detail's banner does; a stream that failed or
   * ended once open shows only its cover. */
  opening?: boolean;
  /** Keys go to this slot's Space. */
  focused: boolean;
}

interface Slot extends SlotOptions {
  id: string;
  el: HTMLElement;
  last: SurfaceState | null;
  status: SlotStatus;
  listener: (status: SlotStatus) => void;
}

/** Frames to keep measuring after something changed (follows transitions). */
const SETTLE_FRAMES = 20;
const POLL_MS = 400;

interface HostEventDetail {
  event?: string;
  payload?: { surfaceId?: string | null; state?: string; reason?: string; opening?: boolean } | null;
}

export class VideoSlotReporter {
  private readonly slots = new Map<string, Slot>();
  private removed: string[] = [];
  private frame = 0;
  private settle = 0;
  private poll: ReturnType<typeof setInterval> | null = null;
  private resize: ResizeObserver | null = null;
  private mutations: MutationObserver | null = null;
  private dpr: MediaQueryList | null = null;
  private focused: string | null = null;
  private next = 0;

  constructor(
    private readonly handler: VideoHandler,
    private readonly win: Window = window,
  ) {}

  /** Starts reporting `el`; the returned function stops (and removes the host's video). */
  register(el: HTMLElement, options: SlotOptions, listener: (status: SlotStatus) => void): () => void {
    const id = `${options.spaceId}#${options.tier}#${++this.next}`;
    const slot: Slot = { ...options, id, el, last: null, status: { phase: "connecting", focused: false }, listener };
    this.slots.set(id, slot);
    if (this.slots.size === 1) this.start();
    this.resize?.observe(el);
    this.flush();
    return () => {
      if (!this.slots.delete(id)) return;
      this.resize?.unobserve(el);
      if (slot.last) this.removed.push(id);
      if (this.focused === id) this.focused = null;
      this.flush();
      if (this.slots.size === 0) this.stop();
    };
  }

  /** Gives keys to a slot's Space (null: back to the page). */
  focus(id: string | null): void {
    this.handler.postMessage({ type: "focus", surfaceId: id });
  }

  /** The id of the slot `el` registered as, for `focus`. */
  idOf(el: HTMLElement): string | null {
    for (const slot of this.slots.values()) if (slot.el === el) return slot.id;
    return null;
  }

  /** Measure on the next frame, and for a few frames after. */
  schedule = (): void => {
    this.settle = SETTLE_FRAMES;
    if (!this.frame) this.frame = this.win.requestAnimationFrame(this.tick);
  };

  private tick = (): void => {
    this.frame = 0;
    this.flush();
    if (this.settle > 0) {
      this.settle -= 1;
      this.frame = this.win.requestAnimationFrame(this.tick);
    }
  };

  /** Measures every slot now and sends what changed. */
  flush(): void {
    const update: SurfaceState[] = [];
    for (const slot of this.slots.values()) {
      const next = this.measure(slot);
      if (sameSurface(slot.last, next)) continue;
      slot.last = next;
      update.push(next);
    }
    const remove = this.removed;
    this.removed = [];
    if (update.length || remove.length) this.handler.postMessage({ type: "surfaces", update, remove });
  }

  private measure(slot: Slot): SurfaceState {
    const inset = slot.inset ?? 0;
    const box = slot.el.getBoundingClientRect();
    const rect = roundRect({
      x: box.x + inset,
      y: box.y + inset,
      width: Math.max(0, box.width - 2 * inset),
      height: Math.max(0, box.height - 2 * inset),
    });
    const clipRaw = visiblePart(slot.el, rect, this.win);
    const clip = clipRaw ? roundRect(clipRaw) : null;
    const shown = this.win.document.visibilityState !== "hidden" && clip !== null && clip.width > 0 && clip.height > 0;
    return {
      surfaceId: slot.id,
      spaceId: slot.spaceId,
      tier: slot.tier,
      interactive: slot.interactive,
      rect,
      clip,
      radius: Math.max(0, slot.radius - inset),
      occluded: shown && clip ? isOccluded(slot.el, clip, this.win.document, slot.radius) : false,
      visible: shown,
    };
  }

  private onHostEvent = (e: Event): void => {
    const detail = (e as CustomEvent<HostEventDetail>).detail;
    if (!detail?.event?.startsWith("video.")) return;
    const p = detail.payload ?? {};
    if (detail.event === "video.surface" && p.surfaceId) {
      const slot = this.slots.get(p.surfaceId);
      const phase = p.state === "live" || p.state === "failed" ? p.state : "connecting";
      if (slot) this.update(slot, { ...slot.status, phase, reason: p.reason ?? undefined, opening: phase === "failed" && p.opening === true ? true : undefined });
    } else if (detail.event === "video.focus") {
      this.focused = p.surfaceId ?? null;
      for (const slot of this.slots.values()) {
        const focused = slot.id === this.focused;
        if (slot.status.focused !== focused) this.update(slot, { ...slot.status, focused });
      }
    }
  };

  private update(slot: Slot, status: SlotStatus): void {
    slot.status = status;
    slot.listener(status);
  }

  private watchDpr = (): void => {
    this.dpr?.removeEventListener("change", this.watchDpr);
    this.dpr = this.win.matchMedia?.(`(resolution: ${this.win.devicePixelRatio}dppx)`) ?? null;
    this.dpr?.addEventListener("change", this.watchDpr);
    this.schedule();
  };

  private start(): void {
    const w = this.win;
    if (typeof ResizeObserver !== "undefined") {
      this.resize = new ResizeObserver(this.schedule);
      this.resize.observe(w.document.documentElement);
    }
    if (typeof MutationObserver !== "undefined") {
      this.mutations = new MutationObserver(this.schedule);
      this.mutations.observe(w.document.body, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["data-open", "data-closed", "data-state", "open", "hidden", "style"],
      });
    }
    w.addEventListener("scroll", this.schedule, { capture: true, passive: true });
    w.addEventListener("resize", this.schedule);
    w.addEventListener("cua:event", this.onHostEvent);
    w.document.addEventListener("visibilitychange", this.schedule);
    this.watchDpr();
    this.poll = setInterval(() => this.flush(), POLL_MS);
  }

  private stop(): void {
    const w = this.win;
    if (this.frame) w.cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.resize?.disconnect();
    this.resize = null;
    this.mutations?.disconnect();
    this.mutations = null;
    w.removeEventListener("scroll", this.schedule, { capture: true });
    w.removeEventListener("resize", this.schedule);
    w.removeEventListener("cua:event", this.onHostEvent);
    w.document.removeEventListener("visibilitychange", this.schedule);
    this.dpr?.removeEventListener("change", this.watchDpr);
    this.dpr = null;
    if (this.poll) clearInterval(this.poll);
    this.poll = null;
  }
}

/**
 * The part of `rect` (the element's video rect) that the viewport and every
 * clipping ancestor (overflow other than visible) leave visible, or null.
 */
export function visiblePart(el: Element, rect: SurfaceRect, win: Window = window): SurfaceRect | null {
  let clip: SurfaceRect | null = intersect(rect, { x: 0, y: 0, width: win.innerWidth, height: win.innerHeight });
  for (let a = el.parentElement; a && clip; a = a.parentElement) {
    if (a === win.document.body || a === win.document.documentElement) break;
    const style = win.getComputedStyle(a);
    if (style.display === "none" || style.visibility === "hidden") return null;
    if (style.overflowX === "visible" && style.overflowY === "visible") continue;
    const b = a.getBoundingClientRect();
    // The padding box: inside the borders, without a scrollbar.
    clip = intersect(clip, { x: b.x + a.clientLeft, y: b.y + a.clientTop, width: a.clientWidth || b.width, height: a.clientHeight || b.height });
  }
  return clip;
}

/**
 * Whether page UI covers any of `clip`: an open dialog, menu, popover or
 * toast overlapping it, or anything else on top at a few sample points (a
 * dialog's backdrop, a sticky bar).
 */
export function isOccluded(el: Element, clip: SurfaceRect, doc: Document = document, radius = 0): boolean {
  for (const o of doc.querySelectorAll(OCCLUDER_SELECTOR)) {
    if (el.contains(o) || o.contains(el)) continue;
    const r = o.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && intersect(clip, { x: r.x, y: r.y, width: r.width, height: r.height })) return true;
  }
  if (typeof doc.elementFromPoint !== "function") return false;
  // Inside the rounded corners, which hit-test as the element's parent.
  const dx = Math.min(clip.width / 2, Math.ceil(radius * 0.3) + 2);
  const dy = Math.min(clip.height / 2, Math.ceil(radius * 0.3) + 2);
  const xs = [clip.x + dx, clip.x + clip.width / 2, clip.x + clip.width - dx];
  const ys = [clip.y + dy, clip.y + clip.height / 2, clip.y + clip.height - dy];
  for (const x of xs) {
    for (const y of ys) {
      const hit = doc.elementFromPoint(x, y);
      if (hit && hit !== el && !el.contains(hit)) return true;
    }
  }
  return false;
}

let shared: VideoSlotReporter | null | undefined;

/** The page's reporter, or null when the host draws no native video. */
export function videoSlots(): VideoSlotReporter | null {
  if (shared === undefined) {
    const handler = nativeVideoHandler();
    shared = handler ? new VideoSlotReporter(handler) : null;
  }
  return shared;
}

/** Tests: forget the shared reporter. */
export function resetVideoSlots(): void {
  shared = undefined;
}
