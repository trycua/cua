// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** The desktop's share of the column when nothing was dragged. */
export const DEFAULT_SPLIT = 0.52;
/** Neither side gets smaller than this (px). */
const MIN_STAGE = 160;
const MIN_BELOW = 200;
const KEY = "cua-spaces:call-split";
/** The divider's grab area (px, its `h-5`): taller than the bar, so it is easy to hit. */
export const DIVIDER_HIT_PX = 20;

function savedSplit(): number {
  try {
    const v = Number(localStorage.getItem(KEY));
    return v > 0 && v < 1 ? v : DEFAULT_SPLIT;
  } catch {
    return DEFAULT_SPLIT;
  }
}

function save(v: number) {
  try {
    localStorage.setItem(KEY, String(v));
  } catch {
    // Private windows and blocked storage: the split just isn't remembered.
  }
}

/** `fraction` of `height` for the stage, kept inside both minimums. */
export function clampSplit(fraction: number, height: number): number {
  if (height <= MIN_STAGE + MIN_BELOW) return 0.5;
  return Math.min(1 - MIN_BELOW / height, Math.max(MIN_STAGE / height, fraction));
}

/**
 * The call layout's column: the desktop on top, the thread below, and a
 * divider between them. Drag it to give either side room; double-click it
 * to snap back; arrow keys move it when it has focus. The split is
 * remembered on this device.
 */
export function CallSplit({
  stage,
  below,
  className,
  leading,
}: {
  stage: ReactNode;
  below: ReactNode;
  className?: string;
  /** On the divider's row, at its left edge: shown on the row's hover or keyboard focus only (Thread view). */
  leading?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [height, setHeight] = useState(0);
  const [split, setSplit] = useState(savedSplit);
  const [dragging, setDragging] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setHeight(el.clientHeight);
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const set = (v: number) => {
    setSplit(v);
    save(v);
  };
  const fraction = height ? clampSplit(split, height) : split;
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragging(true);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!dragging || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    setSplit(clampSplit((e.clientY - r.top) / r.height, r.height));
  };
  const onPointerUp = () => {
    if (!dragging) return;
    setDragging(false);
    save(split);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? 0.1 : 0.03;
    if (e.key === "ArrowUp") set(clampSplit(fraction - step, height));
    else if (e.key === "ArrowDown") set(clampSplit(fraction + step, height));
    else if (e.key === "Home" || e.key === "Enter") set(DEFAULT_SPLIT);
    else return;
    e.preventDefault();
  };
  return (
    <div ref={ref} data-call-split={fraction.toFixed(2)} className={cn("flex min-h-0 min-w-0 flex-1 flex-col", dragging && "select-none", className)}>
      <div data-call-stage="" className="relative min-h-0 shrink-0" style={{ height: `${fraction * 100}%` }}>
        {stage}
      </div>
      <div data-call-divider-row="" className="group/divider relative shrink-0">
        {leading ? (
          // Its own control on the row: pressing it never starts a drag.
          <div
            data-call-divider-leading=""
            onPointerDown={(e) => e.stopPropagation()}
            className="absolute top-1/2 left-5 z-10 -translate-y-1/2 opacity-0 transition-opacity duration-150 group-hover/divider:opacity-100 focus-within:opacity-100"
          >
            {leading}
          </div>
        ) : null}
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize the desktop and the thread"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(fraction * 100)}
          tabIndex={0}
          data-call-divider=""
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onDoubleClick={() => set(DEFAULT_SPLIT)}
          onKeyDown={onKeyDown}
          className="group flex h-5 shrink-0 cursor-row-resize touch-none items-center justify-center outline-none"
        >
          {/* The bar stays small and quiet; the whole row (DIVIDER_HIT_PX) takes the drag. */}
          <span
            data-call-divider-bar=""
            className={cn(
              "h-1 w-10 rounded-full bg-border transition-[width,background-color] duration-150 group-hover:w-14 group-hover:bg-muted-foreground/40 group-focus-visible:bg-ring",
              dragging && "w-14 bg-ring group-hover:bg-ring",
            )}
          />
        </div>
      </div>
      <div className="flex min-h-0 flex-1 flex-col">{below}</div>
    </div>
  );
}
