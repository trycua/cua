// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState } from "react";

import type { PointerSettings } from "@/bridge";
import { cn } from "@/lib/utils";

/**
 * The agent pointer as it looks on this Mac, played in a small window: it
 * travels to Continue the way `cursor.motion.style` says, clicks, and rests
 * (bobbing while `cursor.float` is on). It follows the page's settings live
 * (tilt, marks, the name tag). A drawing of the driver's pointer, not a
 * screenshot of macOS; the real one moves during Test now.
 */

const W = 360;
const H = 200;
const START = { x: 36, y: 184 };
const END = { x: 296, y: 162 };
const LOOP_MS = 4200;
/** The agent's colour (Claude Code's in the driver's default theme). */
const FILL = "#2bb5d8";

type Pt = { x: number; y: number };

const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

function quad(a: Pt, c: Pt, b: Pt, t: number): Pt {
  const u = 1 - t;
  return { x: u * u * a.x + 2 * u * t * c.x + t * t * b.x, y: u * u * a.y + 2 * u * t * c.y + t * t * b.y };
}

/** Where the pointer is `t` (0..1) along the trip, for a motion style. */
function along(style: string, t: number): Pt {
  switch (style) {
    case "classic":
      return quad(START, { x: (START.x + END.x) / 2, y: (START.y + END.y) / 2 }, END, ease(t));
    case "magnetic": {
      // Quick most of the way, then pulled onto the target.
      const k = t < 0.7 ? 0.85 * ease(t / 0.7) : 0.85 + 0.15 * (1 - (1 - (t - 0.7) / 0.3) ** 3);
      return quad(START, { x: 170, y: 150 }, END, k);
    }
    case "spring_settle": {
      const k = 1 - Math.cos(t * Math.PI * 2.5) * Math.exp(-5 * t);
      return quad(START, { x: 160, y: 110 }, END, Math.min(1.08, k));
    }
    case "comet_swoop":
      return quad(START, { x: 250, y: 20 }, END, ease(t));
    case "adaptive":
      return quad(START, { x: 160, y: 90 }, END, ease(t));
    default:
      return quad(START, { x: 160, y: 30 }, END, ease(t));
  }
}

/** The dashed trail, for the style. */
function trail(style: string): string {
  const pts = Array.from({ length: 25 }, (_, i) => along(style, i / 24));
  return pts.map((p, i) => `${i ? "L" : "M"}${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
}

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => typeof window !== "undefined" && Boolean(window.matchMedia?.("(prefers-reduced-motion: reduce)").matches));
  useEffect(() => {
    const m = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (!m) return;
    const on = () => setReduced(m.matches);
    m.addEventListener?.("change", on);
    return () => m.removeEventListener?.("change", on);
  }, []);
  return reduced;
}

/** Milliseconds into the loop, ticking while on screen. `fixedMs` pins it (screenshots, tests). */
function useLoopClock(fixedMs: number | undefined, still: boolean): number {
  const [t, setT] = useState(0);
  const start = useRef<number | null>(null);
  useEffect(() => {
    if (fixedMs !== undefined || still) return;
    let raf = 0;
    const tick = (now: number) => {
      start.current ??= now;
      setT((now - start.current) % LOOP_MS);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [fixedMs, still]);
  return fixedMs ?? (still ? LOOP_MS * 0.8 : t);
}

export function PointerPreview({
  settings,
  caption,
  agent = "Claude Code",
  fixedMs,
  className,
}: {
  settings: PointerSettings;
  caption?: string;
  agent?: string;
  fixedMs?: number;
  className?: string;
}) {
  const reduced = useReducedMotion();
  const ms = useLoopClock(fixedMs, reduced);
  const style = settings.style ?? "signature_arc";
  const p = ms / LOOP_MS;
  // 0-12%: waiting at the start; 12-56%: the trip; 56-66%: the click; then resting on Continue.
  const travel = Math.min(1, Math.max(0, (p - 0.12) / 0.44));
  const clicking = p >= 0.56 && p < 0.66;
  const resting = p >= 0.66 || p < 0.12;
  const pos = p < 0.12 ? START : travel < 1 ? along(style, travel) : END;
  const bob = settings.float !== false && resting && !reduced ? Math.sin(((ms % 2000) / 2000) * Math.PI * 2) * 2.5 : 0;
  const ahead = along(style, Math.min(1, travel + 0.03));
  const heading = settings.rotate && !resting ? (Math.atan2(ahead.y - pos.y, ahead.x - pos.x) * 180) / Math.PI + 90 - 25 : 0;
  const shown = settings.show !== false;
  const mark = settings.marks ? (clicking ? "Clicking" : resting ? null : "Moving") : null;
  const tag = settings.nameTag === "verbose" || (settings.nameTag === "minimal" && clicking);
  return (
    <figure className={cn("flex flex-col gap-2", className)} data-pointer-preview={style} data-float={String(settings.float !== false)}>
      <div className="relative overflow-hidden rounded-lg bg-muted p-4 dark:bg-white/[0.04]">
        <svg viewBox={`0 0 ${W} ${H}`} className="block w-full" role="img" aria-label={caption ?? "The agent pointer moving to Continue and clicking it"}>
          <rect x="0" y="0" width={W} height={H} rx="10" className="fill-background" />
          <circle cx="16" cy="14" r="4" fill="#ff5f57" />
          <circle cx="30" cy="14" r="4" fill="#febc2e" />
          <circle cx="44" cy="14" r="4" fill="#28c840" />
          <text x="16" y="44" className="fill-foreground" fontSize="12" fontWeight="500">
            New document
          </text>
          <rect x="16" y="54" width={W - 32} height="24" rx="5" className="fill-muted" />
          <text x="26" y="70" className="fill-foreground" fontSize="11">
            Quarterly report
          </text>
          <g transform={`translate(${END.x - 40} ${END.y - 13})`}>
            <rect width="80" height="26" rx="6" fill="#2563eb" opacity={clicking ? 0.82 : 1} />
            <text x="40" y="17" textAnchor="middle" fill="white" fontSize="11" fontWeight="500">
              Continue
            </text>
          </g>
          {shown ? (
            <>
              <path d={trail(style)} fill="none" className="stroke-foreground/35" strokeWidth="1.2" strokeDasharray="3 4" />
              <g transform={`translate(${pos.x} ${pos.y + bob}) rotate(${heading}) scale(${clicking ? 0.88 : 1})`} data-agent-pointer="">
                {/* Upright like the system arrow (trycua/cua#6142); the tip is the click point. */}
                <path d="M0,0 L0,20 L5,15.5 L8.6,23.5 L12,22 L8.5,14.2 L14.5,14.2 Z" fill={FILL} stroke="white" strokeWidth="1.5" strokeLinejoin="round" style={{ filter: `drop-shadow(0 0 3px ${FILL})` }} />
              </g>
              {mark ? (
                <text x={pos.x + 18} y={pos.y + bob + 30} className="fill-muted-foreground" fontSize="10">
                  {mark}
                </text>
              ) : null}
              {tag ? (
                <g transform={`translate(${pos.x + 14} ${pos.y + bob - 20})`}>
                  <rect width={agent.length * 6 + 12} height="16" rx="4" fill={FILL} />
                  <text x="6" y="11.5" fill="white" fontSize="10" fontWeight="500">
                    {agent}
                  </text>
                </g>
              ) : null}
            </>
          ) : null}
        </svg>
      </div>
      {caption ? <figcaption className="px-1 text-xs text-muted-foreground">{caption}</figcaption> : null}
    </figure>
  );
}
