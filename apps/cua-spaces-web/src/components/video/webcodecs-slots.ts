// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Live video drawn by the page (WebCodecs), for the Electron shell on
 * Windows and Linux, where no host draws native video over the page.
 *
 * Each slot (a Space tile, the viewer) gets a `<canvas>` inside its element
 * and one production `MediaSession`
 * (libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession.ts) on a
 * ticket from `spaces.openStream`: the shell mints it from the cua daemon,
 * and the socket goes straight from the page to the daemon's loopback
 * listener, so no video passes through IPC. It answers the same phases as
 * the native slots (`lib/video-slots.ts`), so `<StreamSurface>` and the
 * grid's thumbnail keep one fallback:
 *
 * - `connecting` until the first frame is drawn (the canvas stays hidden);
 * - `live` from the first frame;
 * - `failed` when no ticket comes (`opening`; `reason` is `unsupported`
 *   when the shell can't stream at all: no cua here, or sample data) or the
 *   session ends. Only a new registration (Try again) asks again.
 *
 * Tiles are view only. The viewer takes pointer, scroll and keys: a click
 * gives it the keyboard, and scrolls go to the page until it has it. While
 * it has the keyboard every key goes to the Space, Command chords too (the
 * Swift app's `KeyCapture`: the shell's menu shortcuts stand aside and the
 * page's shortcuts don't see them); on a Mac, Control+Option pressed and
 * released alone gives it back, Ctrl+Shift+F12 on Windows and Linux. On a
 * Mac, ⌘ goes to a Linux or Windows Space as Control (the Swift app's
 * `commandAsControl`), and a ⌘ chord goes whole (Chromium drops its key-up). Every session closes while the page is hidden (the
 * window minimized or on another desktop) and reopens with a fresh ticket
 * when it shows again.
 *
 * Decoding asks for the hardware decoder (VideoToolbox on macOS, Media
 * Foundation on Windows, VA-API on Linux), at low latency, and falls back
 * to software when it can't take the stream. A slot can show one window
 * instead of the desktop (`windowId`, picture in picture): its input goes
 * in the background, and the session opens again with activation when the
 * Space says that window needs it (`would_require_activation`).
 */

import { MediaSession, type MediaSessionOptions, type MediaTicket } from "../../../../../libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession";
import type { CuaDesktopBridge } from "@/bridge/detect";
import { ELECTRON_BRIDGE_CHANNEL } from "@/bridge/electron-channels";
import { isWebkitResponse } from "@/bridge/webkit-protocol";
import type { StreamTarget, StreamTicket } from "@/bridge/ops/stream";
import type { SlotOptions, SlotStatus } from "@/lib/video-slots";

import { KeyCapture } from "./key-capture";
import { VideoStats } from "./video-stats";
import { NO_VIDEO_HERE, webCodecsHost } from "./webcodecs-host";

export interface SessionLike {
  start(): void;
  stop(): void;
  /** Counters (the production session's), for the video bench. */
  readonly stats?: { framesReceived: number; framesDecoded: number };
}

/**
 * How a slot's session decodes and takes input: hardware decode at low
 * latency for every slot; for the viewer, ⌘ as Control on a Mac viewing a
 * Linux or Windows Space, ⌘ chords sent whole on a Mac, and scrolls to the
 * Space only while it has the keyboard.
 */
export function sessionConfig(options: Pick<SlotOptions, "interactive" | "os">, hostPlatform: string | undefined) {
  const mac = hostPlatform === "darwin";
  return {
    hardwareAcceleration: "prefer-hardware" as const,
    metaAsControl: options.interactive && mac && options.os !== undefined && options.os !== "macos",
    wholeCommandChords: options.interactive && mac,
    scrollNeedsFocus: options.interactive,
  };
}

/** What a slot asks the shell for: its Space's desktop or one window, at its tier. */
export function streamTarget(options: Pick<SlotOptions, "spaceId" | "tier" | "windowId" | "epoch">, activate = false): StreamTarget {
  const t: StreamTarget = { spaceId: options.spaceId, tier: options.tier };
  if (options.windowId) {
    t.windowId = options.windowId;
    if (options.epoch !== undefined) t.epoch = options.epoch;
    if (activate) t.activate = true;
  }
  return t;
}

/** An input acknowledgement that says the window takes input only when activated. */
export function needsActivation(ack: Record<string, unknown>): boolean {
  const error = ack.error as { code?: unknown } | undefined;
  return ack.delivered === false && error?.code === "would_require_activation";
}
export type SessionFactory = (options: MediaSessionOptions) => SessionLike;

interface Slot {
  id: string;
  el: HTMLElement;
  options: SlotOptions;
  listener: (status: SlotStatus) => void;
  canvas: HTMLCanvasElement;
  session: SessionLike | null;
  status: SlotStatus;
  /** Bumped on every open and close, so a late ticket for a closed session is dropped. */
  generation: number;
  /** A window's session that opens with activation (the Space asked for it). */
  activate: boolean;
  detach: (() => void)[];
}

/** Tells one ticket request from the next (the bridge envelope's id). */
let ticketSeq = 0;

class TicketError extends Error {
  constructor(
    message: string,
    readonly code?: string,
  ) {
    super(message);
  }
}

export class WebCodecsSlots {
  private readonly slots = new Map<string, Slot>();
  private next = 0;

  /** The video bench's counters, while the shell asks for them. */
  readonly stats: VideoStats | null;

  constructor(
    private readonly desktop: CuaDesktopBridge,
    private readonly doc: Document = document,
    private readonly createSession: SessionFactory = (o) => new MediaSession(o),
    stats?: VideoStats | null,
  ) {
    this.stats = stats !== undefined ? stats : desktop.videoStats ? new VideoStats() : null;
  }

  /** Starts `el`'s video; the returned function stops it and removes the canvas. */
  register(el: HTMLElement, options: SlotOptions, listener: (status: SlotStatus) => void): () => void {
    const id = `${options.spaceId}#${options.tier}#${++this.next}`;
    const canvas = this.doc.createElement("canvas");
    canvas.dataset.webcodecs = options.tier;
    Object.assign(canvas.style, {
      position: "absolute",
      inset: `${options.inset ?? 0}px`,
      width: `calc(100% - ${2 * (options.inset ?? 0)}px)`,
      height: `calc(100% - ${2 * (options.inset ?? 0)}px)`,
      objectFit: "contain",
      borderRadius: `${options.radius}px`,
      opacity: "0",
      outline: "none",
    });
    el.append(canvas);
    const slot: Slot = { id, el, options, listener, canvas, session: null, status: { phase: "connecting", focused: false }, generation: 0, activate: false, detach: [] };
    this.stats?.open(id, options.spaceId, options.tier);
    this.slots.set(id, slot);
    if (this.slots.size === 1) this.doc.addEventListener("visibilitychange", this.onVisibility);
    if (options.interactive) this.wireFocus(slot);
    this.open(slot);
    return () => {
      if (!this.slots.delete(id)) return;
      this.close(slot);
      for (const off of slot.detach.splice(0)) off();
      canvas.remove();
      this.stats?.close(id);
      if (this.slots.size === 0) this.doc.removeEventListener("visibilitychange", this.onVisibility);
    };
  }

  /** Gives keys to a slot's Space (null: back to the page). */
  focus(id: string | null): void {
    for (const slot of this.slots.values()) {
      if (slot.id === id && slot.options.interactive) slot.canvas.focus();
      else if (this.doc.activeElement === slot.canvas) slot.canvas.blur();
    }
  }

  idOf(el: HTMLElement): string | null {
    for (const slot of this.slots.values()) if (slot.el === el) return slot.id;
    return null;
  }

  private set(slot: Slot, status: Partial<SlotStatus>): void {
    const next = { ...slot.status, ...status };
    if (next.phase !== "failed") {
      delete next.reason;
      delete next.opening;
    }
    if (next.phase === slot.status.phase && next.focused === slot.status.focused && next.reason === slot.status.reason && next.opening === slot.status.opening) return;
    slot.status = next;
    slot.listener(next);
  }

  private async ticket(target: StreamTarget): Promise<MediaTicket> {
    const request = { id: `stream-${++ticketSeq}`, method: "spaces.openStream", args: target };
    const reply = await this.desktop.invoke(ELECTRON_BRIDGE_CHANNEL, request);
    if (!isWebkitResponse(reply)) throw new TicketError("the shell answered without an envelope");
    if (!reply.ok) throw new TicketError(reply.error?.message ?? "no stream", reply.error?.code);
    const t = reply.result as StreamTicket;
    return { wsUrl: t.wsUrl, ticketExpiresAt: t.expiresAt ?? undefined };
  }

  private open(slot: Slot): void {
    if (this.doc.visibilityState === "hidden") return;
    const gen = ++slot.generation;
    const current = () => gen === slot.generation && this.slots.get(slot.id) === slot;
    this.set(slot, { phase: "connecting" });
    const { interactive } = slot.options;
    const target = streamTarget(slot.options, slot.activate);
    this.ticket(target).then(
      (ticket) => {
        if (!current()) return;
        const stats = this.stats;
        const session: SessionLike = this.createSession({
          canvas: slot.canvas,
          ticket,
          reopen: () => this.ticket(target),
          interactive,
          audio: false,
          ...sessionConfig(slot.options, this.desktop.platform),
          onFrame: (width, height) => {
            if (stats && current()) stats.counts(slot.id, { received: session.stats?.framesReceived ?? 0, decoded: session.stats?.framesDecoded ?? 0, width, height });
            if (!current() || slot.status.phase === "live") return;
            slot.canvas.style.opacity = "1";
            this.set(slot, { phase: "live" });
          },
          onFrameTiming: stats ? (t) => current() && stats.drawn(slot.id, t) : undefined,
          onInputAck: (ack) => {
            // A window that takes input only when activated: open it again,
            // activating it, once (the Swift app's `reopenActivating`).
            if (!current() || !slot.options.windowId || slot.activate || !needsActivation(ack)) return;
            slot.activate = true;
            this.close(slot);
            this.open(slot);
          },
          onStatus: (status, detail) => {
            if (!current()) return;
            if (status === "failed" || status === "ended") this.fail(slot, detail ?? `the stream ${status}`);
          },
          onGone: (reason) => current() && this.fail(slot, reason),
        });
        slot.session = session;
        session.start();
      },
      (e: TicketError) => current() && this.fail(slot, e.code === "unsupported" ? NO_VIDEO_HERE : e.message, true),
    );
  }

  /** `opening`: no ticket came, so no stream ever existed. */
  private fail(slot: Slot, reason: string, opening = false): void {
    this.close(slot);
    this.stats?.failed(slot.id, reason);
    this.set(slot, { phase: "failed", reason, ...(opening ? { opening } : {}) });
  }

  private close(slot: Slot): void {
    slot.generation += 1;
    const session = slot.session;
    slot.session = null;
    session?.stop();
    slot.canvas.style.opacity = "0";
  }

  private onVisibility = (): void => {
    const hidden = this.doc.visibilityState === "hidden";
    for (const slot of this.slots.values()) {
      if (hidden) {
        if (slot.session || slot.status.phase === "connecting") {
          this.close(slot);
          this.set(slot, { phase: "connecting" });
        }
      } else if (!slot.session && slot.status.phase !== "failed") this.open(slot);
    }
  };

  /** The viewer: keys go to the Space while its canvas has focus. The release chord (Control+Option alone
   * on a Mac, Ctrl+Shift+F12 elsewhere: Ctrl+Esc opens the Start menu on Windows, and Ctrl+Alt
   * combinations belong to RDP clients and Linux desktops) hands them back. */
  private wireFocus(slot: Slot): void {
    const { canvas, el } = slot;
    canvas.tabIndex = 0;
    // The shell turns its menu shortcuts off while this has focus (apps/cua-spaces-desktop/src/keyboard.ts).
    canvas.setAttribute(KEYBOARD_ATTRIBUTE, "");
    const mac = this.desktop.platform === "darwin";
    const capture = new KeyCapture();
    const onFocus = () => this.set(slot, { focused: true });
    const onBlur = () => this.set(slot, { focused: false });
    // Capture on the slot's element runs before the session's own key listener on the canvas.
    const onKey = (e: KeyboardEvent) => {
      const focused = this.doc.activeElement === canvas;
      if (mac) {
        // The release passes on to the session: the Space gets that modifier's key-up.
        if (capture.route(e, focused) === "release") canvas.blur();
      } else if (focused && isReleaseKey(e)) {
        e.preventDefault();
        e.stopPropagation();
        canvas.blur();
      }
    };
    // Keys the Space took stop at the slot: the page's shortcuts (⌘K, ⌘B) are not run too.
    const contain = (e: KeyboardEvent) => {
      if (e.target === canvas && this.doc.activeElement === canvas) e.stopPropagation();
    };
    canvas.addEventListener("focus", onFocus);
    canvas.addEventListener("blur", onBlur);
    el.addEventListener("keydown", onKey, true);
    el.addEventListener("keyup", onKey, true);
    el.addEventListener("keydown", contain);
    el.addEventListener("keyup", contain);
    slot.detach.push(
      () => canvas.removeEventListener("focus", onFocus),
      () => canvas.removeEventListener("blur", onBlur),
      () => el.removeEventListener("keydown", onKey, true),
      () => el.removeEventListener("keyup", onKey, true),
      () => el.removeEventListener("keydown", contain),
      () => el.removeEventListener("keyup", contain),
    );
  }
}

/** The marker on a viewer's canvas that takes keys; the shell reads it. */
export const KEYBOARD_ATTRIBUTE = "data-space-keyboard";

/** The key that takes the keyboard back from a Space off the Mac: Ctrl+Shift+F12. */
export function isReleaseKey(e: Pick<KeyboardEvent, "type" | "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">): boolean {
  return e.type === "keydown" && e.key === "F12" && e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
}

let shared: WebCodecsSlots | null | undefined;

/** The page's WebCodecs slots, or null outside the Electron shell. */
export function webCodecsSlots(): WebCodecsSlots | null {
  if (shared === undefined) {
    const desktop = webCodecsHost();
    shared = desktop ? new WebCodecsSlots(desktop) : null;
    // The video bench reads the counters from the shell (`executeJavaScript`).
    const stats = shared?.stats;
    if (stats) (window as { __cuaVideoStats?: () => unknown }).__cuaVideoStats = () => stats.snapshot();
  }
  return shared;
}

/** Tests: forget the shared slots. */
export function resetWebCodecsSlots(): void {
  shared = undefined;
}
