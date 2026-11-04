// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Presence over a stream surface: the shared `PresenceLayer` (from the
 * cua-spacesd HTML5 core) fed by this page's presence session. Drop it next
 * to a `MediaCanvas`; it finds the canvas in its parent and draws over it.
 */

import { useEffect, useRef } from "react";

import { PresenceLayer } from "@cua/spacesd-html5/core/presenceLayer";

import { createPresenceBridge, type PresenceBridge } from "../../native/presence";
import { acquirePresence } from "./sharedPresence";

export interface PresenceOverlayProps {
  spaceId: string;
  /** The remote window this surface streams; unset for the desktop. */
  windowId?: string;
  /** Publish and draw your own cursor (false for the view-only PiP). */
  interactive: boolean;
  /** Whether the stream beneath is live (default true): a lost stream draws
   * no cursor of yours, so the system cursor shows. */
  live?: boolean;
  /** Tests inject a fake; defaults to the Tauri bridge (none outside Tauri). */
  bridge?: PresenceBridge | null;
  /** Clock (tests). */
  now?: () => number;
  /** Receives the layer once it exists (tests). */
  onLayer?: (layer: PresenceLayer | null) => void;
}

const defaultBridge = createPresenceBridge();

export function PresenceOverlay({ spaceId, windowId, interactive, live = true, bridge, now, onLayer }: PresenceOverlayProps) {
  const anchor = useRef<HTMLSpanElement | null>(null);
  const latestLive = useRef(live);
  latestLive.current = live;
  const latestOnLayer = useRef(onLayer);
  latestOnLayer.current = onLayer;

  useEffect(() => {
    const host = anchor.current?.parentElement;
    const surface = host?.querySelector("canvas");
    if (!host || !surface) return;
    const clock = now ?? Date.now;
    const { session, release } = acquirePresence(bridge === undefined ? defaultBridge : bridge, spaceId, clock);
    let layer: PresenceLayer | null = null;
    let cancelled = false;
    void session.then((s) => {
      if (cancelled || !s) return;
      layer = new PresenceLayer({
        surface,
        host,
        view: () => s.view,
        me: () => ({ id: s.me, color: s.myColor }),
        ...(windowId ? { windowId } : {}),
        interactive,
        live: () => latestLive.current,
        onPointer: (p) => s.publish(p),
        now: clock,
      });
      latestOnLayer.current?.(layer);
    });
    return () => {
      cancelled = true;
      layer?.destroy();
      latestOnLayer.current?.(null);
      release();
    };
  }, [spaceId, windowId, interactive, bridge, now]);

  return <span ref={anchor} className="presence-anchor" hidden />;
}
