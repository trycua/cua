// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Maximize2Icon } from "lucide-react";
import { useLayoutEffect, useRef, useState, type PointerEvent, type ReactNode } from "react";

import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export type Corner = "tl" | "tr" | "bl" | "br";

/** Gap to the thread's edges (px). */
const MARGIN = 12;
/** The bar under the picture (px, its `h-7`). */
const BAR = 28;
export const MIN_WIDTH = 220;
const MAX_WIDTH = 640;
export const DEFAULT_WIDTH = 320;
/** Movement (px) under which a press is a click (expand), not a drag. */
const CLICK_SLOP = 4;

interface Placement {
  corner: Corner;
  width: number;
}

const key = (spaceId: string) => `cua-spaces:mini-player:${spaceId}`;

function saved(spaceId: string): Placement {
  try {
    const v = JSON.parse(globalThis.localStorage?.getItem(key(spaceId)) ?? "null") as Partial<Placement> | null;
    const corner = v?.corner && ["tl", "tr", "bl", "br"].includes(v.corner) ? v.corner : "br";
    const width = typeof v?.width === "number" && v.width >= MIN_WIDTH ? v.width : DEFAULT_WIDTH;
    return { corner, width };
  } catch {
    return { corner: "br", width: DEFAULT_WIDTH };
  }
}

function save(spaceId: string, p: Placement) {
  try {
    globalThis.localStorage?.setItem(key(spaceId), JSON.stringify(p));
  } catch {
    // Blocked storage: not remembered.
  }
}

/** The corner nearest to a point in a `w`×`h` area. */
export function nearestCorner(x: number, y: number, w: number, h: number): Corner {
  return `${y < h / 2 ? "t" : "b"}${x < w / 2 ? "l" : "r"}` as Corner;
}

/** The player's width, kept inside the area it floats in. */
export function clampWidth(width: number, areaWidth: number, areaHeight: number, aspect: number): number {
  const byWidth = areaWidth - 2 * MARGIN;
  const byHeight = (areaHeight - 2 * MARGIN - BAR) * aspect;
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, byWidth, byHeight, width));
}

/**
 * Thread mode's mini-player (option A): the Space's live desktop, small,
 * floating in a corner of the thread. Drag it anywhere and it snaps to the
 * nearest corner; drag its grip to resize; click it (or its expand button)
 * to go back to the call layout. Corner and size are remembered per Space.
 * It floats over the thread only, never the composer, and `onReserve`
 * tells the thread how much room to keep clear at its end when it sits at
 * the bottom, so the newest message stays in view.
 */
export function MiniPlayer({
  spaceId,
  label,
  aspect,
  children,
  onExpand,
  onReserve,
}: {
  spaceId: string;
  /** Under the picture ("Desktop", or the window shown). */
  label: string;
  aspect: number;
  /** The stream (a view-only `StreamSurface`, `h-full`). */
  children: ReactNode;
  onExpand: () => void;
  /** Room to keep clear at the thread's end (px; 0 at the top corners). */
  onReserve?: (px: number) => void;
}) {
  const area = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [place, setPlace] = useState<Placement>(() => saved(spaceId));
  const [drag, setDrag] = useState<{ x: number; y: number; dx: number; dy: number; moved: boolean } | null>(null);
  const [resize, setResize] = useState<{ x: number; start: number; width: number } | null>(null);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    const read = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const width = size.w ? clampWidth(resize ? resize.width : place.width, size.w, size.h, aspect) : place.width;
  const height = Math.round(width / aspect) + BAR;
  const left = place.corner.endsWith("l") ? MARGIN : size.w - MARGIN - width;
  const top = place.corner.startsWith("t") ? MARGIN : size.h - MARGIN - height;
  const bottom = place.corner.startsWith("b");

  useLayoutEffect(() => {
    onReserve?.(bottom ? height + MARGIN : 0);
  }, [bottom, height, onReserve]);

  const commit = (next: Placement) => {
    setPlace(next);
    save(spaceId, next);
  };

  const onDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ x: e.clientX, y: e.clientY, dx: 0, dy: 0, moved: false });
  };
  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    setDrag({ ...drag, dx, dy, moved: drag.moved || Math.hypot(dx, dy) > CLICK_SLOP });
  };
  const onUp = () => {
    if (!drag) return;
    setDrag(null);
    if (!drag.moved) return onExpand();
    const cx = left + drag.dx + width / 2;
    const cy = top + drag.dy + height / 2;
    commit({ ...place, corner: nearestCorner(cx, cy, size.w, size.h) });
  };

  // The grip sits on the bar's inner end (toward the thread's middle):
  // dragging it away from the corner makes the player bigger.
  const inward = place.corner.endsWith("r") ? -1 : 1;
  const onGripDown = (e: PointerEvent<HTMLButtonElement>) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setResize({ x: e.clientX, start: width, width });
  };
  const onGripMove = (e: PointerEvent<HTMLButtonElement>) => {
    if (!resize) return;
    // Toward the thread's middle grows it, toward its corner shrinks it.
    setResize({ ...resize, width: resize.start + (e.clientX - resize.x) * inward });
  };
  const onGripUp = (e: PointerEvent<HTMLButtonElement>) => {
    e.stopPropagation();
    if (!resize) return;
    setResize(null);
    commit({ ...place, width: clampWidth(width, size.w, size.h, aspect) });
  };

  return (
    <div ref={area} data-mini-player-area="" className="pointer-events-none absolute inset-0">
      <div
        data-mini-player={place.corner}
        style={{ left: left + (drag?.dx ?? 0), top: top + (drag?.dy ?? 0), width, height }}
        className={cn(
          "pointer-events-auto absolute flex flex-col overflow-hidden rounded-lg bg-chrome shadow-float select-none",
          drag?.moved ? "cursor-grabbing" : "transition-[left,top] duration-200 ease-out",
        )}
      >
        <div
          role="button"
          tabIndex={0}
          aria-label={`${label}: click to go back to the call view, drag to move`}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={() => setDrag(null)}
          // A click with no pointer behind it (keyboard, accessibility): expand.
          onClick={(e) => e.detail === 0 && onExpand()}
          onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onExpand())}
          className="min-h-0 flex-1 cursor-pointer touch-none outline-none"
        >
          {children}
        </div>
        <div className={cn("relative flex h-7 shrink-0 items-center justify-between px-2 text-xs text-muted-foreground", inward > 0 && "flex-row-reverse")}>
          <button
            type="button"
            aria-label="Resize"
            data-mini-player-grip=""
            onPointerDown={onGripDown}
            onPointerMove={onGripMove}
            onPointerUp={onGripUp}
            className={cn("size-4 shrink-0 touch-none outline-none", inward < 0 ? "cursor-nesw-resize" : "cursor-nwse-resize")}
          >
            <svg viewBox="0 0 16 16" className={cn("size-4 text-muted-foreground/60", inward < 0 && "-scale-x-100")} aria-hidden>
              <path d="M5 15 15 5M10 15l5-5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
          {/* Centred on the bar, whatever sits at its ends. */}
          <span data-mini-player-label="" className="pointer-events-none absolute inset-x-8 truncate text-center">
            {label}
          </span>
          <Tooltip content="Call view">
            <button
              type="button"
              aria-label="Call view"
              data-mini-player-expand=""
              onPointerDown={(e) => e.stopPropagation()}
              onClick={onExpand}
              className="inline-flex size-5 shrink-0 cursor-default items-center justify-center rounded-lg outline-none hover:bg-accent hover:text-foreground"
            >
              <Maximize2Icon className="size-3.5" />
            </button>
          </Tooltip>
        </div>
      </div>
    </div>
  );
}
