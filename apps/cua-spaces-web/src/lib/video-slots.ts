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

function sameRects(a: readonly SurfaceRect[], b: readonly SurfaceRect[]): boolean {
  return a.length === b.length && a.every((r, i) => sameRect(r, b[i] ?? null));
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
    sameRect(a.clip, b.clip) &&
    sameRects(a.holes ?? [], b.holes ?? [])
  );
}

/** Page UI that hides a Space tile's video entirely when it overlaps the
 * tile: a dialog takes the keyboard and dims the page, so the video under it
 * goes. The Space viewer never hides for one (`keepUnderDialogs`): a dialog
 * is a hole there, like a menu. A popover that marks itself a hole
 * (`data-video-occluder`, a profile card) is no occluder for either. */
export const OCCLUDER_SELECTOR = ['[role="dialog"]', '[role="alertdialog"]'].join(",");

/** The dimmed layer under a modal dialog (our Dialog and AlertDialog
 * backdrops): it paints over the whole page, so over a viewer that keeps its
 * video it is looked through, and the viewer takes no pointer or keys while
 * it is open (`modalOpen`). */
export const DIM_SELECTOR = "[data-window-dim]";

/** A dialog as a hole (a viewer that keeps its video under dialogs). */
const DIALOG_HOLE_SELECTOR = [OCCLUDER_SELECTOR, DIM_SELECTOR].join(",");

/** Whether a modal dialog is open: the page dims behind it and the Space
 * under it must take neither the pointer nor the keys. */
export function modalOpen(doc: Document = document): boolean {
  return doc.querySelector(`${DIM_SELECTOR}:not([data-closed]):not([data-ending-style])`) !== null;
}

/**
 * Small page UI that floats over a slot (menus, popovers, tooltips, toasts,
 * the viewer's own key bar): the host cuts the video out under each one
 * (`holes`) and the rest keeps playing. Hiding the whole video for a menu
 * turned the desktop black while the menu was open.
 */
export const HOLE_SELECTOR = [
  "[data-video-occluder]",
  "[data-video-hole]",
  '[role="menu"]',
  '[role="listbox"]',
  '[role="tooltip"]',
  "[data-toast]",
].join(",");

/** Where a slot's own controls live (a Space tile): a `[data-video-hole]`
 * inside one counts only for the slot in the same scope, and only while it
 * shows (a hover control sits at `opacity: 0` until then). */
export const HOLE_SCOPE_SELECTOR = "[data-video-hole-scope]";

/** Transparent layers that only catch outside clicks (Base UI's
 * `InternalBackdrop` under an open menu or popover): they paint nothing, so
 * they never cover video. */
export const INERT_SELECTOR = "[data-base-ui-inert]";

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

/** Events after which a tile's hover controls may have shown or hidden. */
const POINTER_FOCUS = ["pointerover", "pointerout", "focusin", "focusout"] as const;

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
    // The Space viewer stays drawn under every popup, card, menu, tooltip and dialog (CUA-2641); a tile hides under a dialog.
    const keep = slot.tier === "full";
    const clipRaw = visiblePart(slot.el, rect, this.win);
    const clip = clipRaw ? roundRect(clipRaw) : null;
    const shown = this.win.document.visibilityState !== "hidden" && clip !== null && clip.width > 0 && clip.height > 0;
    return {
      surfaceId: slot.id,
      spaceId: slot.spaceId,
      tier: slot.tier,
      // Under a modal dialog the viewer keeps playing but takes no pointer or keys.
      interactive: slot.interactive && !(keep && modalOpen(this.win.document)),
      rect,
      clip,
      radius: Math.max(0, slot.radius - inset),
      occluded: shown && clip ? isOccluded(slot.el, clip, this.win.document, slot.radius, keep) : false,
      holes: shown && clip ? videoHoles(slot.el, clip, this.win.document, keep).map(roundRect) : [],
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
        // `class`: a tile's controls (holes) show and hide by class (selection mode).
        attributeFilter: ["data-open", "data-closed", "data-state", "open", "hidden", "style", "class"],
      });
    }
    w.addEventListener("scroll", this.schedule, { capture: true, passive: true });
    // Hover and focus show a tile's controls by CSS alone (no DOM change): measure their holes.
    for (const type of POINTER_FOCUS) w.addEventListener(type, this.schedule, { passive: true });
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
    for (const type of POINTER_FOCUS) w.removeEventListener(type, this.schedule);
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
 * Whether page UI covers `clip` so the video must go: an open dialog
 * overlapping it, or anything else on top at a few sample points (a
 * dialog's backdrop, a sticky bar). Floating UI that gets a hole instead
 * (`HOLE_SELECTOR`), click-catching layers that paint nothing
 * (`INERT_SELECTOR`) and the slot's own ancestors (hit when an open modal
 * menu made the page `inert`) don't count.
 */
export function isOccluded(el: Element, clip: SurfaceRect, doc: Document = document, radius = 0, keepUnderDialogs = false): boolean {
  for (const o of doc.querySelectorAll(OCCLUDER_SELECTOR)) {
    if (el.contains(o) || o.contains(el)) continue;
    // A dialog-role popup that is a hole (a profile card) never hides the video, nor does any dialog over a viewer.
    if (keepUnderDialogs || o.closest(HOLE_SELECTOR)) continue;
    const r = o.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && intersect(clip, { x: r.x, y: r.y, width: r.width, height: r.height })) return true;
  }
  if (typeof doc.elementsFromPoint !== "function" && typeof doc.elementFromPoint !== "function") return false;
  // Inside the rounded corners, which hit-test as the element's parent.
  const dx = Math.min(clip.width / 2, Math.ceil(radius * 0.3) + 2);
  const dy = Math.min(clip.height / 2, Math.ceil(radius * 0.3) + 2);
  const xs = [clip.x + dx, clip.x + clip.width / 2, clip.x + clip.width - dx];
  const ys = [clip.y + dy, clip.y + clip.height / 2, clip.y + clip.height - dy];
  for (const x of xs) {
    for (const y of ys) {
      const hit = topmostPainted(doc, x, y);
      // An ancestor of the slot can't paint over it: it is what hit-testing
      // reaches when the slot itself takes no hits (a modal menu makes the
      // rest of the page `inert`).
      if (hit && !hit.contains(el) && !el.contains(hit) && !hit.closest(keepUnderDialogs ? `${HOLE_SELECTOR},${DIALOG_HOLE_SELECTOR}` : HOLE_SELECTOR)) return true;
    }
  }
  return false;
}

/** The topmost element at a point that paints: inert click catchers are looked through. */
function topmostPainted(doc: Document, x: number, y: number): Element | null {
  const all = typeof doc.elementsFromPoint === "function" ? doc.elementsFromPoint(x, y) : [doc.elementFromPoint(x, y)];
  for (const hit of all) {
    if (!hit) continue;
    if (hit.closest(INERT_SELECTOR)) continue;
    return hit;
  }
  return null;
}

/**
 * The floating UI over `clip` (menus, popovers, tooltips, toasts, the
 * viewer's key bar, a tile's checkbox and power button while they show),
 * each clipped to it: the host draws no video there.
 */
export function videoHoles(el: Element, clip: SurfaceRect, doc: Document = document, keepUnderDialogs = false): SurfaceRect[] {
  const holes: SurfaceRect[] = [];
  const win = doc.defaultView;
  // Over a viewer that keeps its video under dialogs, a dialog's popup is a hole (its dimmed backdrop is not: the video shows through it).
  const holeSelector = keepUnderDialogs ? `${HOLE_SELECTOR},${OCCLUDER_SELECTOR}` : HOLE_SELECTOR;
  for (const o of doc.querySelectorAll(holeSelector)) {
    // Floating UI inside the slot counts only when it asks (`data-video-hole`).
    if (o.contains(el) || (el.contains(o) && !o.hasAttribute("data-video-hole"))) continue;
    if (o.hasAttribute("data-video-hole")) {
      // Another tile's control, or one hidden until hover: no hole.
      const scope = o.closest(HOLE_SCOPE_SELECTOR);
      if (scope && !scope.contains(el)) continue;
      if (win && !showsNow(o, scope, win)) continue;
    }
    // One hole for a nested pair (a menu inside a marked popup).
    if (o.parentElement?.closest(holeSelector)) continue;
    const r = o.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    const hole = intersect(clip, { x: r.x, y: r.y, width: r.width, height: r.height });
    if (hole) holes.push(hole);
  }
  return holes;
}

/** Laid out, not hidden and not fully transparent (itself or an ancestor up to `scope`). */
function showsNow(o: Element, scope: Element | null, win: Window): boolean {
  if (win.getComputedStyle(o).visibility === "hidden") return false;
  const stop = scope?.parentElement ?? null;
  for (let a: Element | null = o; a && a !== stop; a = a.parentElement) {
    const style = win.getComputedStyle(a);
    if (style.display === "none" || Number.parseFloat(style.opacity) === 0) return false;
  }
  return true;
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
