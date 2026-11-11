// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Presence cursors over the video the page draws (the Electron shell's
 * WebCodecs slots): the other participants' cursors where the SDK's
 * `PresenceView` puts them, and yours at the local pointer, published to
 * them. The drawing is the shared `PresenceLayer` (the cua-spacesd HTML5
 * viewer's and the Tauri app's): the SDK's cursor art, presence colors and
 * name pills, placed over the letterboxed picture as the SwiftUI app's
 * `PresenceOverlay` does.
 *
 * One session per Space per page, shared by every slot showing it (the Space
 * page's inline desktop and nothing else, or one PiP window), so one person
 * is one participant however many slots they have open. Reference-counted:
 * the last release leaves. The host joins through the cua SDK
 * (`presence.join`, `bridge/ops/presence.ts`) and pushes the session's events
 * to this window only.
 */

import { HEARTBEAT_INTERVAL_MS, PresenceView, type PresenceEvent } from "@trycua/cua/spaces/presence";

import { PresenceLayer } from "../../../../../libs/cua/crates/cua-spacesd-html5/web/src/core/presenceLayer";
import type { CuaDesktopBridge } from "@/bridge/detect";
import { ELECTRON_BRIDGE_CHANNEL, ELECTRON_EVENT_CHANNEL } from "@/bridge/electron-channels";
import { PRESENCE_EVENT, type PresenceJoinInfo, type PresencePointerArgs } from "@/bridge/ops/presence";
import { isWebkitResponse } from "@/bridge/webkit-protocol";

/** How often staleness is checked (a timer, not rAF, so a hidden page still expires cursors). */
export const EXPIRE_EVERY_MS = 1_000;

let requestSeq = 0;

async function call<T>(desktop: CuaDesktopBridge, method: string, args: Record<string, unknown>): Promise<T> {
  const reply = await desktop.invoke(ELECTRON_BRIDGE_CHANNEL, { id: `presence-${++requestSeq}`, method, args });
  if (!isWebkitResponse(reply)) throw new Error("the shell answered without an envelope");
  if (!reply.ok) throw new Error(reply.error?.message ?? `${method} failed`);
  return reply.result as T;
}

/** One joined session: the SDK's model, fed by the host's events. */
export class SharedPresence {
  readonly view: PresenceView;
  private readonly stops: (() => void)[] = [];

  constructor(
    private readonly desktop: CuaDesktopBridge,
    readonly info: PresenceJoinInfo,
    private readonly now: () => number,
  ) {
    this.view = PresenceView.from(info.me, info.members, info.delayMs, now());
    this.view.setHeartbeatIntervalMs(HEARTBEAT_INTERVAL_MS);
  }

  start(): void {
    const off = this.desktop.on?.(ELECTRON_EVENT_CHANNEL, (raw) => {
      const e = raw as { event?: unknown; payload?: { handle?: unknown; event?: PresenceEvent } } | null;
      if (e?.event !== PRESENCE_EVENT || e.payload?.handle !== this.info.handle || !e.payload.event) return;
      this.view.apply(e.payload.event, this.now());
    });
    if (off) this.stops.push(off);
    const timer = setInterval(() => this.view.expire(this.now()), EXPIRE_EVERY_MS);
    this.stops.push(() => clearInterval(timer));
  }

  get me(): string {
    return this.info.me.participantId;
  }

  /** Your color, as the Space assigned it. */
  get myColor(): string {
    return this.view.participant(this.me)?.color || this.info.me.color;
  }

  publish(pointer: Omit<PresencePointerArgs, "handle">): void {
    void call(this.desktop, "presence.publish", { handle: this.info.handle, ...pointer }).catch(() => {});
  }

  stop(): void {
    for (const s of this.stops.splice(0)) s();
    void call(this.desktop, "presence.leave", { handle: this.info.handle }).catch(() => {});
  }
}

interface Entry {
  session: Promise<SharedPresence | null>;
  refs: number;
}

const sessions = new Map<string, Entry>();

/**
 * Joins (or reuses) the page's session for `spaceId`. Resolves to null when
 * the Space has no presence (or the host can't join): the video still
 * plays, with no cursors. Call the returned release exactly once.
 */
export function acquirePresence(
  desktop: CuaDesktopBridge,
  spaceId: string,
  now: () => number = Date.now,
): { session: Promise<SharedPresence | null>; release: () => void } {
  let entry = sessions.get(spaceId);
  if (!entry) {
    const session = call<PresenceJoinInfo>(desktop, "presence.join", { spaceId })
      .then((info) => {
        const s = new SharedPresence(desktop, info, now);
        s.start();
        return s;
      })
      .catch(() => null);
    entry = { session, refs: 0 };
    sessions.set(spaceId, entry);
  }
  entry.refs += 1;
  const held = entry;
  let released = false;
  return {
    session: held.session,
    release: () => {
      if (released) return;
      released = true;
      held.refs -= 1;
      if (held.refs > 0) return;
      if (sessions.get(spaceId) === held) sessions.delete(spaceId);
      void held.session.then((s) => s?.stop());
    },
  };
}

/** Tests: forget every session. */
export function resetPresenceSessions(): void {
  sessions.clear();
}

export interface PresenceAttachment {
  /** The stream under it is live (cursors show) or not (nothing shows, the system cursor stays). */
  setLive(live: boolean): void;
  /** Takes the cursors down and releases the session. */
  detach(): void;
}

/**
 * Draws presence over one slot's canvas (its parent hosts the layer) for the
 * slot's Space and target (`windowId`: one window; unset: the desktop).
 * Nothing shows until `setLive(true)`: no cursor floats over a cover that
 * says "Connecting…" or "Failed".
 */
export function attachPresence(
  desktop: CuaDesktopBridge,
  canvas: HTMLCanvasElement,
  host: HTMLElement,
  options: { spaceId: string; windowId?: string; interactive: boolean },
  now: () => number = Date.now,
): PresenceAttachment {
  const { session, release } = acquirePresence(desktop, options.spaceId, now);
  let live = false;
  let layer: PresenceLayer | null = null;
  let detached = false;
  const show = () => {
    if (layer) layer.root.style.display = live ? "" : "none";
  };
  void session.then((s) => {
    if (detached || !s) return;
    layer = new PresenceLayer({
      surface: canvas,
      host,
      view: () => s.view,
      me: () => ({ id: s.me, color: s.myColor }),
      ...(options.windowId ? { windowId: options.windowId } : {}),
      interactive: options.interactive,
      live: () => live,
      onPointer: (p) => s.publish(p),
      now,
    });
    show();
  });
  return {
    setLive(next) {
      live = next;
      show();
    },
    detach() {
      if (detached) return;
      detached = true;
      layer?.destroy();
      layer = null;
      release();
    },
  };
}
