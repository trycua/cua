// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { AppWindowIcon, ChevronDownIcon, MonitorIcon, PictureInPicture2Icon } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import type { CuaDesktopBridge } from "@/bridge/detect";
import { ELECTRON_BRIDGE_CHANNEL } from "@/bridge/electron-channels";
import type { RemoteWindow } from "@/bridge/contracts/teleport";
import { isWebkitResponse } from "@/bridge/webkit-protocol";
import { pipBadge } from "@/components/pip/pip-view";
import { Button } from "@/components/ui/button";
import { useVideoSlot } from "@/components/video/use-video-slot";
import { useThemeSync } from "@/lib/theme";
import { cn } from "@/lib/utils";

/**
 * A Space's desktop in a window of its own (`/viewer`, "Open in window"),
 * in the Electron shell's viewer window (apps/cua-spaces-desktop/src/viewer.ts),
 * as the SwiftUI app's `SpaceWindowView` with `SpaceScreenView`'s controls:
 * a toolbar with the source picker (Full desktop, each of the Space's
 * windows, Refresh windows), the stream's size and Pop out, then the
 * stream on black, interactive (a click gives it the keyboard). Popped
 * out, the stream plays in a picture-in-picture panel instead and this
 * window says so, with Bring it back. Once the first frame says the
 * stream's size, the window takes its shape (`window.resizeTo`, which the
 * shell reads as the shape only).
 */
export interface ViewerParams {
  spaceId: string;
  spaceName: string;
  os?: string;
}

export function viewerParams(search: string): ViewerParams {
  const q = new URLSearchParams(search);
  return { spaceId: q.get("space") ?? "", spaceName: q.get("name") ?? "", os: q.get("os") || undefined };
}

/** What the viewer shows: the whole desktop, or one of the Space's windows. */
export type ViewerSource = { kind: "desktop" } | { kind: "window"; window: RemoteWindow };

/** The core's Stream row id for the desktop (`stream::DESKTOP_ROW_ID`). */
export const DESKTOP_ROW = "desktop";

/** The picker's label for a source (`StreamSource.label`). */
export function sourceLabel(source: ViewerSource): string {
  if (source.kind === "desktop") return "Full desktop";
  return windowLabel(source.window);
}

export function windowLabel(w: RemoteWindow): string {
  return w.title.trim() || w.appName.trim() || w.appId;
}

/** The Stream row (and picture-in-picture panel) a source stands for. */
export const sourceRow = (source: ViewerSource) => (source.kind === "desktop" ? DESKTOP_ROW : source.window.id);

/** The desktop's size ("1280×800"), or a dash before the first frame
 * (`SpaceScreenView.dimensionsLabel`; the frame count stays debug detail). */
export function dimensionsLabel(width: number, height: number): string {
  return width > 0 && height > 0 ? `${width}×${height}` : "—";
}

/** How often the window list is read while the viewer is open (`StreamRowsModel.poll`). */
export const VIEWER_WINDOWS_MS = 5_000;

const desktop = () => (window as { cuaDesktop?: CuaDesktopBridge }).cuaDesktop;
let seq = 0;

/** One bridge call on the shell's channel; rejects with the host's message. */
async function call<T>(method: string, args: Record<string, unknown>): Promise<T> {
  const bridge = desktop();
  if (!bridge) throw new Error("no host");
  const reply = await bridge.invoke(ELECTRON_BRIDGE_CHANNEL, { id: `viewer-${++seq}`, method, args });
  if (!isWebkitResponse(reply)) throw new Error(`${method}: no envelope`);
  if (!reply.ok) throw new Error(reply.error?.message ?? `${method} failed`);
  return reply.result as T;
}

interface WindowsAnswer {
  windows: RemoteWindow[];
  display: { widthPx: number; heightPx: number } | null;
  open?: string[];
}

export function ViewerView({ params }: { params: ViewerParams }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [source, setSource] = useState<ViewerSource>({ kind: "desktop" });
  const [windows, setWindows] = useState<RemoteWindow[] | null>(null);
  const [open, setOpen] = useState<string[]>([]);
  const [attempt, setAttempt] = useState(0);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const row = sourceRow(source);
  const poppedOut = open.includes(row);
  const windowId = source.kind === "window" ? source.window.id : undefined;
  const epoch = source.kind === "window" ? source.window.targetEpoch : undefined;
  const slot = useVideoSlot(ref, {
    active: params.spaceId !== "" && !poppedOut,
    attempt,
    spaceId: params.spaceId,
    tier: "full",
    interactive: true,
    radius: 0,
    os: params.os,
    windowId,
    epoch,
  });

  // The toolbar follows the app's appearance; the stream is on black.
  useThemeSync();
  useEffect(() => {
    document.title = params.spaceName || params.spaceId || "Cua Spaces";
  }, [params.spaceName, params.spaceId]);

  // The Space's windows and the open panels, now and every few seconds.
  const refresh = useCallback(async () => {
    if (!params.spaceId) return;
    try {
      const a = await call<WindowsAnswer>("spaces.windows", { spaceId: params.spaceId });
      setWindows(a.windows);
      if (a.open) setOpen(a.open);
    } catch {
      // A failed read keeps what was listed.
      setWindows((w) => w ?? []);
    }
  }, [params.spaceId]);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), VIEWER_WINDOWS_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  // The stream's size, from the canvas the slot draws into; the window
  // takes the stream's shape the first time it is known.
  const live = slot?.phase === "live";
  const shaped = useRef(false);
  useEffect(() => {
    const canvas = ref.current?.querySelector("canvas");
    if (!live || !canvas) return;
    const read = () => {
      if (canvas.width <= 0 || canvas.height <= 0) return;
      setSize({ width: canvas.width, height: canvas.height });
      if (!shaped.current) {
        shaped.current = true;
        window.resizeTo(canvas.width, canvas.height);
      }
    };
    read();
    const watch = new MutationObserver(read);
    watch.observe(canvas, { attributes: true, attributeFilter: ["width", "height"] });
    return () => watch.disconnect();
  }, [live]);

  const choose = (next: ViewerSource) => {
    setSize({ width: 0, height: 0 });
    setSource(next);
  };
  // Pop out, or Pop in / Bring it back: the shell's panel for this source.
  const togglePip = () =>
    void call<string[]>("stream.pip", { spaceId: params.spaceId, command: { type: poppedOut ? "close" : "open", row } })
      .then(setOpen)
      .catch(() => {});

  const badge = poppedOut ? null : pipBadge(slot ? slot.phase : null, slot?.reason);

  return (
    <div data-viewer={params.spaceId} className="flex h-screen w-screen flex-col overflow-hidden bg-black select-none">
      <div data-viewer-toolbar className="flex h-11 shrink-0 items-center gap-2.5 border-b bg-background px-3 text-foreground">
        <SourcePicker source={source} windows={windows} onChoose={choose} onRefresh={() => void refresh()} />
        <span className="flex-1" />
        <span data-viewer-size className="font-mono text-[11px] text-muted-foreground tabular-nums">
          {dimensionsLabel(size.width, size.height)}
        </span>
        <Button
          variant="outline"
          size="sm"
          data-viewer-pip=""
          aria-pressed={poppedOut}
          title="Detach this stream into a floating always-on-top window. The session keeps running."
          onClick={togglePip}
        >
          {poppedOut ? <AppWindowIcon /> : <PictureInPicture2Icon />}
          {poppedOut ? "Pop in" : "Pop out"}
        </Button>
      </div>
      <div className="relative min-h-0 flex-1 text-white">
        {poppedOut ? (
          <div data-viewer-popped-out className="absolute inset-0 flex flex-col items-center justify-center gap-2">
            <PictureInPicture2Icon className="size-7 text-white/50" />
            <p className="text-xs text-white/60">Playing in a floating window</p>
            <button type="button" onClick={togglePip} className="h-6 cursor-default rounded-md bg-white/10 px-2.5 text-xs text-white/90 outline-none hover:bg-white/15">
              Bring it back
            </button>
          </div>
        ) : (
          <div ref={ref} data-viewer-stream className="absolute inset-0" />
        )}
        {badge ? (
          <div data-viewer-status={badge.tone} className="absolute top-2.5 left-2.5 flex items-center gap-1.5 rounded-full bg-black/65 px-2 py-1 text-[11px] font-medium">
            <span className={cn("size-[7px] rounded-full", badge.tone === "red" ? "bg-red-500" : "bg-yellow-400")} />
            {badge.text}
            {slot?.phase === "failed" ? (
              <button type="button" className="ml-1 underline underline-offset-2" onClick={() => setAttempt((a) => a + 1)}>
                Try again
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** Desktop ↔ single-window switcher (`StreamSourcePicker`). */
function SourcePicker({
  source,
  windows,
  onChoose,
  onRefresh,
}: {
  source: ViewerSource;
  windows: RemoteWindow[] | null;
  onChoose: (s: ViewerSource) => void;
  onRefresh: () => void;
}) {
  const item = "flex cursor-default items-center gap-2 rounded-md px-2 py-1 text-[13px] outline-none data-highlighted:bg-brand data-highlighted:text-white";
  const Icon = source.kind === "desktop" ? MonitorIcon : AppWindowIcon;
  return (
    <MenuPrimitive.Root onOpenChange={(open) => open && onRefresh()}>
      <MenuPrimitive.Trigger
        data-viewer-source=""
        className="inline-flex h-7 max-w-80 min-w-0 cursor-default items-center gap-1.5 rounded-md px-1.5 text-[13px] outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <Icon className="size-3.5 shrink-0" />
        <span className="truncate">{sourceLabel(source)}</span>
        <ChevronDownIcon className="size-3 shrink-0 text-muted-foreground" />
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="start" className="z-50 outline-none">
          <MenuPrimitive.Popup className="glass max-w-96 min-w-44 rounded-lg border p-1 text-popover-foreground shadow-float outline-none">
            <MenuPrimitive.Item className={item} onClick={() => onChoose({ kind: "desktop" })}>
              <MonitorIcon className="size-3.5" /> Full desktop
            </MenuPrimitive.Item>
            {windows && windows.length > 0 ? (
              <>
                <MenuPrimitive.Separator className="mx-1 my-1 h-px bg-border" />
                {windows.map((w) => (
                  <MenuPrimitive.Item key={w.id} data-viewer-window={w.id} className={item} onClick={() => onChoose({ kind: "window", window: w })}>
                    <span className="truncate">{windowLabel(w)}</span>
                  </MenuPrimitive.Item>
                ))}
              </>
            ) : null}
            <MenuPrimitive.Separator className="mx-1 my-1 h-px bg-border" />
            <MenuPrimitive.Item className={item} onClick={onRefresh}>
              Refresh windows
            </MenuPrimitive.Item>
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
