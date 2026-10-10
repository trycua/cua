// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { AppWindowIcon, XIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { CuaDesktopBridge } from "@/bridge/detect";
import { ELECTRON_BRIDGE_CHANNEL } from "@/bridge/electron-channels";
import { useVideoSlot } from "@/components/video/use-video-slot";
import { cn } from "@/lib/utils";

/**
 * A picture-in-picture panel's contents (`/pip`), in the Electron shell's
 * floating window (apps/cua-spaces-desktop/src/pip.ts), as the SwiftUI
 * app's `PiPStreamContent`: the stream filling the panel on black, a small
 * status chip while it is not live, and nothing else until the pointer is
 * over it, when a thin bar shows the title, Open Space and close. The bar
 * drags the panel. The stream is its own session (the desktop, or one
 * window), at the viewer's tier with input: a click gives it the keyboard,
 * Control+Option pressed and released alone (Ctrl+Shift+F12 off the Mac)
 * gives it back. Once the stream's size is known the panel takes its shape
 * (`window.resizeTo`, which the shell reads as the shape only).
 */
export interface PipParams {
  spaceId: string;
  spaceName: string;
  title: string;
  os?: string;
  windowId?: string;
  epoch?: number;
}

export function pipParams(search: string): PipParams {
  const q = new URLSearchParams(search);
  const epoch = Number(q.get("epoch"));
  return {
    spaceId: q.get("space") ?? "",
    spaceName: q.get("name") ?? "",
    title: q.get("title") ?? "",
    os: q.get("os") || undefined,
    windowId: q.get("window") || undefined,
    epoch: q.get("epoch") !== null && Number.isFinite(epoch) ? epoch : undefined,
  };
}

/** The status chip's words and colour (`StreamStatusBadge`); null while live. `phase` null: no video here at all. */
export function pipBadge(phase: "connecting" | "live" | "failed" | null, reason?: string): { text: string; tone: "yellow" | "red" } | null {
  if (phase === "live") return null;
  if (phase === null) return { text: "Live video isn't available here", tone: "red" };
  if (phase === "failed") return { text: reason ? `Failed: ${reason}` : "Failed", tone: "red" };
  return { text: "Connecting…", tone: "yellow" };
}

const desktop = () => (window as { cuaDesktop?: CuaDesktopBridge }).cuaDesktop;

export function PipView({ params }: { params: PipParams }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [attempt, setAttempt] = useState(0);
  const slot = useVideoSlot(ref, {
    active: params.spaceId !== "",
    attempt,
    spaceId: params.spaceId,
    tier: "full",
    interactive: true,
    radius: 0,
    os: params.os,
    windowId: params.windowId,
    epoch: params.epoch,
  });

  useEffect(() => {
    document.title = params.title || params.spaceName || "Cua Spaces";
  }, [params.title, params.spaceName]);

  // The panel takes the stream's shape once its size is known, and again when it changes.
  const live = slot?.phase === "live";
  useEffect(() => {
    const canvas = ref.current?.querySelector("canvas");
    if (!live || !canvas) return;
    const shape = () => {
      if (canvas.width > 0 && canvas.height > 0) window.resizeTo(canvas.width, canvas.height);
    };
    shape();
    const watch = new MutationObserver(shape);
    watch.observe(canvas, { attributes: true, attributeFilter: ["width", "height"] });
    return () => watch.disconnect();
  }, [live]);

  const openSpace = () => void desktop()?.invoke(ELECTRON_BRIDGE_CHANNEL, { id: `pip-open-${Date.now()}`, method: "spaces.open", args: { id: params.spaceId } });
  const badge = pipBadge(slot ? slot.phase : null, slot?.reason);
  const label = params.title && params.spaceName ? `${params.title} — ${params.spaceName}` : params.title || params.spaceName;

  return (
    <div data-pip={params.spaceId} className="group relative h-screen w-screen overflow-hidden bg-black text-white select-none">
      <div ref={ref} data-pip-stream className="absolute inset-0" />
      {badge ? (
        <div data-pip-status={badge.tone} className="absolute top-2 left-2 flex items-center gap-1.5 rounded-full bg-black/65 px-2 py-1 text-[11px] font-medium">
          <span className={cn("size-[7px] rounded-full", badge.tone === "red" ? "bg-red-500" : "bg-yellow-400")} />
          {badge.text}
          {slot?.phase === "failed" ? (
            <button type="button" className="ml-1 underline underline-offset-2" onClick={() => setAttempt((a) => a + 1)}>
              Try again
            </button>
          ) : null}
        </div>
      ) : null}
      <div className="app-drag absolute inset-x-0 top-0 flex h-8 items-center gap-1 bg-gradient-to-b from-black/75 to-transparent pr-1 pl-3 opacity-0 transition-opacity group-hover:opacity-100">
        <span className="min-w-0 flex-1 truncate text-xs text-white/90">{label}</span>
        <button type="button" title="Open Space" aria-label="Open Space" onClick={openSpace} className="grid size-6 place-items-center rounded-md text-white/80 hover:bg-white/15 hover:text-white">
          <AppWindowIcon className="size-3.5" />
        </button>
        <button type="button" title="Close" aria-label="Close" onClick={() => window.close()} className="grid size-6 place-items-center rounded-md text-white/80 hover:bg-white/15 hover:text-white">
          <XIcon className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
