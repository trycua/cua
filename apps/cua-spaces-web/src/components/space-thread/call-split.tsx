// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";

import { LAYOUT_CHANGE_EVENT } from "@/lib/stream-surface";
import { clampSplit, decideDock, DEFAULT_H_SPLIT, DEFAULT_V_SPLIT, DOCK_MIN_WIDTH, effectiveDock, splitOf, type Dock } from "@/lib/space-layout";
import { cn } from "@/lib/utils";
import { useSpaceLayoutStore } from "@/stores/space-layout";

/** The desktop's share of the column when nothing was dragged. */
export const DEFAULT_SPLIT = DEFAULT_V_SPLIT;
/** The divider's grab area (px, its `h-5` / `w-5`): wider than the bar, so it is easy to hit. */
export const DIVIDER_HIT_PX = 20;

export { clampSplit };

/** Share of the column that the arrow keys move a divider by (Shift: more). */
const STEP = 0.03;
const STEP_BIG = 0.1;

interface Drag {
  /** The dock when the drag began. */
  from: Dock;
  /** Where letting go would leave the desktop. */
  target: Dock;
  /** The desktop's share along the current dock's axis while it is dragged. */
  split: number;
}

/** The desktop's share of the column for a pointer at `x`, `y` (shares of the column) in `dock`. */
const shareAt = (dock: Dock, x: number, y: number) => (dock === "top" ? y : dock === "left" ? x : 1 - x);

/**
 * The call layout's column: the desktop and the thread with a divider
 * between them. The desktop is on top by default; drag the divider to give
 * either side room, or to the left or right edge to dock the desktop there
 * (side by side), and back toward the top to return. Double-click or Home
 * snaps the size back; arrow keys move the divider; Escape cancels a drag.
 * The dock and sizes are remembered (`stores/space-layout.ts`); a column too
 * narrow for side by side shows the desktop on top and says nothing.
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
  /** On the divider, at its start edge: shown on the divider's hover or keyboard focus only (Thread view). */
  leading?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const prefs = useSpaceLayoutStore();
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const setDragBoth = useCallback((d: Drag | null) => {
    dragRef.current = d;
    setDrag(d);
  }, []);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setSize({ width: el.clientWidth, height: el.clientHeight });
    const ro = new ResizeObserver(() => setSize({ width: el.clientWidth, height: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const dock = effectiveDock(prefs.dock, size.width);
  const horizontal = dock !== "top";
  const saved = splitOf(prefs, dock);
  const measured = size.width > 0 && size.height > 0;
  const fraction = drag && drag.from === dock ? drag.split : measured ? clampSplit(dock, saved, size.width, size.height) : saved;

  // Tell the video host the panes moved, in the frame the layout committed (see `LAYOUT_CHANGE_EVENT`).
  useLayoutEffect(() => {
    window.dispatchEvent(new Event(LAYOUT_CHANGE_EVENT));
  }, [dock, fraction, size.width, size.height]);

  /** A new size on the dock's own axis (a narrow column's top layout never overwrites a saved side dock). */
  const commit = (d: Dock, f: number) => useSpaceLayoutStore.getState().setSplit(d, f);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    setDragBoth({ from: dock, target: dock, split: fraction });
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (!d || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return;
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    const target = r.width < DOCK_MIN_WIDTH ? d.from : decideDock(d.from, d.target, x, y);
    // Over a dock zone the size holds and the preview shows; anywhere else the divider follows the pointer.
    const split = target === d.from ? clampSplit(d.from, shareAt(d.from, x, y), r.width, r.height) : d.split;
    if (target !== d.target || split !== d.split) setDragBoth({ ...d, target, split });
  };
  const finish = (e?: PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (!d) return;
    setDragBoth(null);
    if (e?.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    if (d.target === d.from) commit(d.from, d.split);
    else useSpaceLayoutStore.getState().setLayout(d.target, clampSplit(d.target, splitOf(prefs, d.target), size.width, size.height));
  };
  const cancel = useCallback(() => setDragBoth(null), [setDragBoth]);

  // Escape cancels a drag from anywhere (the pointer, not the keyboard, owns it).
  useEffect(() => {
    if (!drag) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        cancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [drag, cancel]);

  const nudge = (delta: number) => commit(dock, clampSplit(dock, fraction + delta, size.width, size.height));
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? STEP_BIG : STEP;
    // The arrows that move the divider along its axis: toward the desktop's start shrinks it.
    const grow = dock === "right" ? "ArrowLeft" : dock === "left" ? "ArrowRight" : "ArrowDown";
    const shrink = dock === "right" ? "ArrowRight" : dock === "left" ? "ArrowLeft" : "ArrowUp";
    if (e.key === "Escape" && drag) cancel();
    else if (e.key === grow) nudge(step);
    else if (e.key === shrink) nudge(-step);
    else if (e.key === "Home" || e.key === "Enter") commit(dock, horizontal ? DEFAULT_H_SPLIT : DEFAULT_V_SPLIT);
    else return;
    e.preventDefault();
  };
  const reset = () => commit(dock, horizontal ? DEFAULT_H_SPLIT : DEFAULT_V_SPLIT);

  const previewDock = drag && drag.target !== drag.from ? drag.target : null;
  const stageStyle = horizontal ? { width: `${fraction * 100}%` } : { height: `${fraction * 100}%` };
  return (
    <div
      ref={ref}
      data-call-split={fraction.toFixed(2)}
      data-call-dock={dock}
      className={cn(
        "relative flex min-h-0 min-w-0 flex-1",
        horizontal ? (dock === "right" ? "flex-row-reverse" : "flex-row") : "flex-col",
        drag && "select-none",
        className,
      )}
    >
      <div data-call-stage="" className={cn("relative min-h-0 min-w-0 shrink-0", horizontal ? "h-full" : "w-full")} style={stageStyle}>
        <div className={cn("size-full", dock === "top" ? "px-5 pt-4" : dock === "left" ? "py-4 pr-1 pl-5" : "py-4 pr-5 pl-1")}>{stage}</div>
      </div>
      <div data-call-divider-row="" className={cn("group/divider relative shrink-0", horizontal && "flex")}>
        {leading ? (
          // Its own control on the divider: pressing it never starts a drag.
          <div
            data-call-divider-leading=""
            onPointerDown={(e) => e.stopPropagation()}
            className={cn(
              "absolute z-10 opacity-0 transition-opacity duration-150 group-hover/divider:opacity-100 focus-within:opacity-100",
              horizontal ? "top-3 left-1/2 -translate-x-1/2" : "top-1/2 left-5 -translate-y-1/2",
            )}
          >
            {leading}
          </div>
        ) : null}
        <div
          role="separator"
          aria-orientation={horizontal ? "vertical" : "horizontal"}
          aria-label="Resize the desktop and the thread"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(fraction * 100)}
          tabIndex={0}
          data-call-divider=""
          data-dragging={drag ? "" : undefined}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={finish}
          onPointerCancel={cancel}
          onLostPointerCapture={cancel}
          onDoubleClick={reset}
          onKeyDown={onKeyDown}
          className={cn(
            "group flex shrink-0 touch-none items-center justify-center outline-none",
            horizontal ? "h-full w-5 cursor-col-resize" : "h-5 cursor-row-resize",
          )}
        >
          {/* The grabber stays small and quiet, and shows more on hover, focus or a drag; the whole divider (DIVIDER_HIT_PX) takes the drag. */}
          <span
            data-call-divider-bar=""
            className={cn(
              "rounded-full transition-[width,height,opacity,background-color] duration-150",
              horizontal ? "h-10 w-1 group-hover:h-14" : "h-1 w-10 group-hover:w-14",
              "bg-border group-hover:bg-muted-foreground/40 group-focus-visible:bg-ring",
              drag && (horizontal ? "h-14 bg-ring group-hover:bg-ring" : "w-14 bg-ring group-hover:bg-ring"),
            )}
          />
        </div>
      </div>
      <div data-call-below="" className="flex min-h-0 min-w-0 flex-1 flex-col">
        {below}
      </div>
      {previewDock ? <DockPreview dock={previewDock} share={splitOf(prefs, previewDock)} width={size.width} height={size.height} /> : null}
    </div>
  );
}

/** Where the desktop will go if the drag ends here: a thin outline, four bars the host cuts the video out under (`data-video-hole`), the desktop itself untouched. */
function DockPreview({ dock, share, width, height }: { dock: Dock; share: number; width: number; height: number }) {
  const w = dock === "top" ? width : Math.round(clampSplit(dock, share, width, height) * width);
  const h = dock === "top" ? Math.round(clampSplit(dock, share, width, height) * height) : height;
  const left = dock === "right" ? width - w : 0;
  const T = 2;
  const inset = 6;
  const bar = "pointer-events-none absolute rounded-full bg-ring/80";
  const box = {
    left: left + inset,
    top: inset,
    width: Math.max(0, w - 2 * inset),
    height: Math.max(0, h - 2 * inset),
  };
  return (
    <div data-dock-preview={dock} aria-hidden className="pointer-events-none absolute inset-0 z-20">
      <span data-video-hole="" className={bar} style={{ left: box.left, top: box.top, width: box.width, height: T }} />
      <span data-video-hole="" className={bar} style={{ left: box.left, top: box.top + box.height - T, width: box.width, height: T }} />
      <span data-video-hole="" className={bar} style={{ left: box.left, top: box.top, width: T, height: box.height }} />
      <span data-video-hole="" className={bar} style={{ left: box.left + box.width - T, top: box.top, width: T, height: box.height }} />
    </div>
  );
}
