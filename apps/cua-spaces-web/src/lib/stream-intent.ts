// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Opens a Space's stream before its page does.
 *
 * Connecting to a Space's desktop takes time the person feels as
 * "Connecting…". Most of it can run before the click: when the pointer rests
 * on a link to a Space (its tile, its row, a sidebar entry) or presses one,
 * the page tells the host (`prewarm`, stream-surface.ts) and the host opens
 * that Space's viewer stream. The page that follows binds to a stream that is
 * already connected, or further along than it would have been. The host lets
 * an unclaimed stream go after a few seconds, and keeps at most a few.
 *
 * Intent only: a hover shorter than `HOVER_MS` (the pointer passing over a
 * list) opens nothing.
 */

import { nativeVideoHandler, type VideoHandler } from "./stream-surface";

/** How long the pointer rests on a link before it counts as intent. */
export const HOVER_MS = 120;
/** A Space is not announced again within this time. */
export const REPEAT_MS = 10_000;

const SPACE_PATH = /^\/spaces\/([^/?#]+)\/?$/;

/** The Space a link points to (`/spaces/<id>`), or null for any other target. */
export function intentSpaceId(target: EventTarget | null): string | null {
  const el = target as Element | null;
  if (!el || typeof el.closest !== "function") return null;
  const a = el.closest("a[href]");
  if (!a) return null;
  let path: string;
  try {
    path = new URL(a.getAttribute("href") ?? "", "http://app.invalid").pathname;
  } catch {
    return null;
  }
  const m = SPACE_PATH.exec(path);
  if (!m) return null;
  try {
    return decodeURIComponent(m[1]!);
  } catch {
    return null;
  }
}

export interface IntentOptions {
  /** The host's video handler (null: nothing to tell, no listeners). */
  handler?: VideoHandler | null;
  now?: () => number;
  setTimer?: (fn: () => void, ms: number) => ReturnType<typeof setTimeout>;
  clearTimer?: (t: ReturnType<typeof setTimeout>) => void;
  /** Asked at the moment of intent: false while the person's settings say
   * not to connect by themselves. */
  allowed?: () => boolean;
}

/** Starts watching `doc` for intent; returns the function that stops. */
export function watchStreamIntent(doc: Document, options: IntentOptions = {}): () => void {
  const handler = options.handler === undefined ? nativeVideoHandler(doc.defaultView) : options.handler;
  if (!handler) return () => {};
  const now = options.now ?? Date.now;
  const setTimer = options.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearTimer = options.clearTimer ?? ((t) => clearTimeout(t));
  const sent = new Map<string, number>();
  let pending: { id: string; timer: ReturnType<typeof setTimeout> } | null = null;

  const send = (id: string): void => {
    if (options.allowed && !options.allowed()) return;
    const last = sent.get(id);
    if (last !== undefined && now() - last < REPEAT_MS) return;
    sent.set(id, now());
    try {
      const r: unknown = handler.postMessage({ type: "prewarm", spaceId: id });
      if (r && typeof (r as PromiseLike<unknown>).then === "function") (r as PromiseLike<unknown>).then(undefined, () => {});
    } catch {
      // The host went away: nothing to warm.
    }
  };
  const cancel = (): void => {
    if (pending) clearTimer(pending.timer);
    pending = null;
  };
  const over = (e: Event): void => {
    const id = intentSpaceId(e.target);
    if (!id) return cancel();
    if (pending?.id === id) return;
    cancel();
    pending = {
      id,
      timer: setTimer(() => {
        pending = null;
        send(id);
      }, HOVER_MS),
    };
  };
  const press = (e: Event): void => {
    const id = intentSpaceId(e.target);
    if (!id) return;
    cancel();
    send(id);
  };

  doc.addEventListener("pointerover", over, { passive: true });
  doc.addEventListener("pointerdown", press, { passive: true, capture: true });
  doc.addEventListener("focusin", over, { passive: true });
  return () => {
    cancel();
    doc.removeEventListener("pointerover", over);
    doc.removeEventListener("pointerdown", press, { capture: true });
    doc.removeEventListener("focusin", over);
  };
}
