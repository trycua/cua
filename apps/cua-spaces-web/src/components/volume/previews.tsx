// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from "react";

import {
  driveFrame,
  driveScene,
  driveStill,
  driverFrame,
  driverScene,
  driverStill,
  useBridge,
  useVolumePins,
  type DriveFrame,
  type DriverFrame,
  type PreviewPoint,
  type PreviewRect,
  type PreviewWindow,
} from "@/bridge";

/**
 * The two miniatures the core animates: the cua-driver card's (an agent
 * ticking boxes in a back window while you select text in front) and the
 * Cua Volume page's (files flying from a Space into the volume in Finder).
 * The scene and every frame are the core's, as the SwiftUI app's
 * `DriverPreview` and `DriveMountPreview` draw them; this only places them.
 * The pointers are drawn inside the picture; the page's own cursor is never
 * touched.
 */

/** Whether the system asks for reduced motion (live). */
function useReducedMotion(): boolean {
  const query = "(prefers-reduced-motion: reduce)";
  const [reduced, setReduced] = useState(() => typeof window !== "undefined" && window.matchMedia?.(query).matches === true);
  useEffect(() => {
    const mq = window.matchMedia?.(query);
    if (!mq) return;
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.("change", on);
    return () => mq.removeEventListener?.("change", on);
  }, []);
  return reduced;
}

/** Milliseconds into the loop, at about 30 fps while the picture is on screen in a visible page. */
function useLoopTime(ref: RefObject<HTMLElement | null>, run: boolean, loopMs: number): number {
  const [t, setT] = useState(0);
  useEffect(() => {
    if (!run || loopMs <= 0) return;
    let visible = true;
    let raf = 0;
    let last = 0;
    let elapsed = 0;
    let prev: number | null = null;
    const active = () => visible && document.visibilityState === "visible";
    const tick = (now: number) => {
      if (prev !== null) elapsed += now - prev;
      prev = now;
      if (now - last >= 1000 / 30) {
        last = now;
        setT(elapsed % loopMs);
      }
      raf = requestAnimationFrame(tick);
    };
    const update = () => {
      if (active() && !raf) {
        prev = null;
        raf = requestAnimationFrame(tick);
      } else if (!active() && raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
    };
    const io =
      typeof IntersectionObserver === "undefined"
        ? null
        : new IntersectionObserver((entries) => {
            visible = entries.some((e) => e.isIntersecting);
            update();
          });
    if (io && ref.current) io.observe(ref.current);
    document.addEventListener("visibilitychange", update);
    update();
    return () => {
      io?.disconnect();
      document.removeEventListener("visibilitychange", update);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [ref, run, loopMs]);
  return t;
}

/** `r` (stage points) relative to the window frame `f`. */
const at = (r: PreviewRect, f: PreviewRect): CSSProperties => ({ position: "absolute", left: r.x - f.x, top: r.y - f.y, width: r.width, height: r.height });

const points = (ps: PreviewPoint[]) => ps.map((p) => `${p.x},${p.y}`).join(" ");

/** The desktop the miniature sits on: the stage centred in the card's width. */
function Stage({ width, height, children, label, still }: { width: number; height: number; children: ReactNode; label: string; still: boolean }) {
  return (
    <div
      role="img"
      aria-label={label}
      data-still={still ? "true" : undefined}
      className="flex justify-center overflow-hidden rounded-md bg-linear-to-b from-sky-200/70 to-indigo-200/70 dark:from-slate-800 dark:to-slate-900"
      style={{ height }}
    >
      <div className="relative" style={{ width, height }} aria-hidden="true">
        {children}
      </div>
    </div>
  );
}

function MiniWindow({ window: w, role, children }: { window: PreviewWindow; role: string; children?: ReactNode }) {
  const f = w.frame;
  return (
    <div
      data-window={role}
      className="absolute overflow-hidden border border-black/10 bg-white shadow-[0_2px_4px_rgba(0,0,0,0.18)] dark:border-white/10 dark:bg-neutral-800"
      style={{ left: f.x, top: f.y, width: f.width, height: f.height, borderRadius: w.radius }}
    >
      <span className="absolute inset-x-0 top-0 flex items-center gap-[2.5px] border-b border-black/10 pl-[5px] dark:border-white/10" style={{ height: w.titleBar }}>
        {[0, 1, 2].map((i) => (
          <i key={i} className="block size-[3.6px] rounded-full bg-black/20 dark:bg-white/25" />
        ))}
      </span>
      {children}
    </div>
  );
}

const Bar = ({ rect, frame, style }: { rect: PreviewRect; frame: PreviewRect; style?: CSSProperties }) => (
  <span className="rounded-full bg-black/15 dark:bg-white/20" style={{ ...at(rect, frame), ...style }} />
);

/** A miniature's clock: a fixed moment, the still (reduced motion), or the loop. */
function useClock(fixed: number | "still" | undefined, loopMs: number) {
  // Parity pins both miniatures at a replay's moment.
  const pinned = useVolumePins().driver?.tMs;
  const fixedMs = fixed ?? pinned;
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const still = fixedMs === "still" || (fixedMs === undefined && reduced);
  const t = useLoopTime(ref, fixedMs === undefined && !reduced, loopMs);
  return { ref, still, tMs: typeof fixedMs === "number" ? fixedMs : t };
}

/* ---- The cua-driver card --------------------------------------------------------------- */

export function DriverPreview({ fixedMs, label }: { fixedMs?: number | "still"; label: string }) {
  const { core } = useBridge();
  const scene = driverScene(core);
  const { ref, still, tMs } = useClock(fixedMs, scene?.loopMs ?? 0);
  if (!scene) return null;
  const frame: DriverFrame | null = still ? driverStill(core) : driverFrame(core, tMs);
  if (!frame) return null;
  const line = scene.lines[scene.selectedLine];
  return (
    <div
      ref={ref}
      data-preview="driver"
      data-agent={`${frame.agent.x.toFixed(1)},${frame.agent.y.toFixed(1)}`}
      data-pointer={`${frame.pointer.x.toFixed(1)},${frame.pointer.y.toFixed(1)}`}
      data-pressed={String(frame.pressed)}
      data-agent-pressed={String(frame.agentPressed)}
      data-selection={frame.selection.toFixed(3)}
      data-ripple={frame.ripple.toFixed(3)}
      data-checked={frame.checked.map((c) => c.toFixed(3)).join(",")}
    >
      <Stage width={scene.width} height={scene.height} label={label} still={still}>
        <MiniWindow window={scene.back} role="back">
          {scene.checkboxes.map((c, i) => {
            const on = frame.checked[i] ?? 0;
            return (
              <span key={`box-${i}`} className="rounded-[2px] border-[0.75px] border-black/40 dark:border-white/50" style={at(c, scene.back.frame)}>
                <span className="absolute inset-0 rounded-[1.5px] bg-brand" style={{ opacity: on }} />
                <svg viewBox="0 0 8 8" className="absolute inset-0 size-full">
                  <polyline
                    points="2,4.2 3.5,5.7 6.2,2.4"
                    fill="none"
                    stroke="white"
                    strokeWidth={1.1}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    pathLength={1}
                    strokeDasharray={1}
                    strokeDashoffset={1 - on}
                  />
                </svg>
              </span>
            );
          })}
          {scene.labels.map((l, i) => (
            <Bar key={`label-${i}`} rect={l} frame={scene.back.frame} />
          ))}
        </MiniWindow>
        <svg className="absolute top-0 left-0 overflow-visible" width="16" height="16" style={{ transform: `translate(${frame.agent.x}px, ${frame.agent.y}px)` }}>
          {frame.ripple > 0 ? (
            <g opacity={1 - frame.ripple} transform={`scale(${1 + 0.4 * frame.ripple})`} stroke={scene.agentFill} strokeWidth={1.2} strokeLinecap="round">
              {scene.agentRays.map((r, i) => (
                <line key={i} x1={r.from.x} y1={r.from.y} x2={r.to.x} y2={r.to.y} />
              ))}
            </g>
          ) : null}
          <g transform={`scale(${frame.agentPressed ? 0.85 : 1})`}>
            <polygon
              points={points(scene.agentPointer)}
              fill={scene.agentFill}
              stroke="white"
              strokeWidth={1.4}
              strokeLinejoin="round"
              paintOrder="stroke"
              style={{ filter: `drop-shadow(0 0 2px ${scene.agentFill})` }}
            />
          </g>
        </svg>
        <MiniWindow window={scene.front} role="front">
          {line && frame.selection > 0 ? (
            <span
              className="absolute rounded-[1.5px] bg-brand/30"
              style={{ left: line.x - scene.front.frame.x, top: line.y + line.height / 2 - 4 - scene.front.frame.y, width: line.width * frame.selection, height: 8 }}
            />
          ) : null}
          {scene.lines.map((l, i) => (
            <Bar key={`line-${i}`} rect={l} frame={scene.front.frame} />
          ))}
        </MiniWindow>
        <svg
          className="absolute top-0 left-0 overflow-visible"
          width="12"
          height="16"
          style={{ transform: `translate(${frame.pointer.x}px, ${frame.pointer.y}px) scale(${frame.pressed ? 0.85 : 1})`, transformOrigin: "0 0" }}
        >
          <polygon points={points(scene.pointer)} fill="black" stroke="white" strokeWidth={1} strokeLinejoin="round" paintOrder="stroke" />
        </svg>
      </Stage>
    </div>
  );
}

/* ---- The Cua Volume page ------------------------------------------------------------------ */

export function DriveMountPreview({ fixedMs, label }: { fixedMs?: number | "still"; label: string }) {
  const { core } = useBridge();
  const scene = driveScene(core);
  const { ref, still, tMs } = useClock(fixedMs, scene?.loopMs ?? 0);
  if (!scene) return null;
  const frame: DriveFrame | null = still ? driveStill(core) : driveFrame(core, tMs);
  if (!frame) return null;
  const finder = scene.finder.frame;
  const icon = scene.sourceIcons[0];
  return (
    <div ref={ref} data-preview="drive" data-volume={frame.volume.toFixed(3)} data-arrived={frame.arrived.map((a) => a.toFixed(3)).join(",")}>
      <Stage width={scene.width} height={scene.height} label={label} still={still}>
        <MiniWindow window={scene.space} role="space">
          {scene.sourceIcons.map((r, i) => (
            <FileIcon key={`src-${i}`} rect={r} frame={scene.space.frame} />
          ))}
          {scene.sourceLabels.map((l, i) => (
            <Bar key={`src-label-${i}`} rect={l} frame={scene.space.frame} />
          ))}
        </MiniWindow>
        <MiniWindow window={scene.finder} role="finder">
          <span
            className="bg-black/[0.05] dark:bg-white/[0.06]"
            style={{ ...at(scene.sidebar, finder), top: scene.finder.titleBar, height: scene.sidebar.height - scene.finder.titleBar }}
          />
          {scene.places.map((p, i) => (
            <Bar key={`place-${i}`} rect={p} frame={finder} />
          ))}
          <span className="rounded-[2px] bg-brand/25" style={{ ...at(scene.volume, finder), opacity: frame.volume }} />
          <span className="rounded-[1px] bg-black/45 dark:bg-white/60" style={{ ...at(scene.volumeIcon, finder), opacity: frame.volume }} />
          <span
            className="absolute font-medium whitespace-nowrap text-foreground"
            style={{
              left: scene.volumeLabelX - finder.x,
              top: scene.volume.y - finder.y,
              height: scene.volume.height,
              lineHeight: `${scene.volume.height}px`,
              fontSize: scene.fontSize,
              opacity: frame.volume,
            }}
          >
            {scene.volumeLabel}
          </span>
          {scene.destIcons.map((r, i) => (
            <FileIcon key={`dst-${i}`} rect={r} frame={finder} opacity={frame.arrived[i] ?? 0} />
          ))}
          {scene.destLabels.map((l, i) => (
            <Bar key={`dst-label-${i}`} rect={l} frame={finder} style={{ opacity: frame.arrived[i] ?? 0 }} />
          ))}
        </MiniWindow>
        {frame.flight && icon ? (
          <FileIcon rect={{ x: frame.flight.x, y: frame.flight.y, width: icon.width, height: icon.height }} frame={{ x: 0, y: 0, width: scene.width, height: scene.height }} />
        ) : null}
      </Stage>
    </div>
  );
}

/** A document glyph: a page with a folded corner. */
function FileIcon({ rect, frame, opacity }: { rect: PreviewRect; frame: PreviewRect; opacity?: number }) {
  const { width: w, height: h } = rect;
  const fold = Math.min(w, h) * 0.4;
  return (
    <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} style={{ ...at(rect, frame), opacity }}>
      <path d={`M0.5 0.5 H${w - fold} L${w - 0.5} ${fold} V${h - 0.5} H0.5 Z`} className="fill-white stroke-black/40 dark:fill-neutral-200 dark:stroke-black/50" strokeWidth={0.75} />
    </svg>
  );
}
