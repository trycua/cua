// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The spaces.cua.ai hero behind the new first run: the site's starfield photo, its twinkling stars and
 * its floating keycaps, at the places the site's wide layout gives them. The images are the site's own
 * public renders (public/onboarding/float, copied unchanged from spaces.cua.ai/_spaces/img/float).
 * The three product keys carry the Apple, Windows and Linux marks from the site's download menu, set on
 * each key's top face the way the site sets its product marks (a projective map onto the face's four
 * corners). Keys drift slowly; with reduced motion they stay still.
 */

import { useEffect, useRef, useState, type CSSProperties } from "react";

import { OS_MARKS, type MarkOs } from "./os-marks";

/** A key's centre (% of the frame), its width (% of the frame's width), depth (0 far, 1 near) and tilt. */
type Place = { src: string; x: number; y: number; size: number; depth: number; rot: number; soft?: boolean; mark?: MarkOs };

/** The site's WIDE layout (spaces-site float-layout.ts), with the OS marks on its product keys. */
export const KEYS: Place[] = [
  { src: "far-2", x: 54, y: 47, size: 5.2, depth: 0.1, rot: -8 },
  { src: "far-4", x: 46, y: 74, size: 5, depth: 0.1, rot: 12 },
  { src: "far-1", x: 58, y: 90, size: 5.6, depth: 0.12, rot: 4 },
  { src: "far-3", x: 97, y: 50, size: 5, depth: 0.1, rot: 6 },
  { src: "ivory-3", x: 85, y: 13, size: 9.5, depth: 0.45, rot: 4 },
  { src: "ivory-1", x: 64, y: 60, size: 13.5, depth: 0.5, rot: -6 },
  { src: "ivory-5", x: 95, y: 68, size: 10, depth: 0.45, rot: 8 },
  { src: "ivory-6", x: 70, y: 86, size: 9, depth: 0.45, rot: -3 },
  { src: "product-4", x: 61, y: 30, size: 14, depth: 0.55, rot: -4 },
  { src: "product-1", x: 78, y: 40, size: 15, depth: 0.6, rot: 6, mark: "windows" },
  { src: "product-2", x: 93, y: 27, size: 13, depth: 0.55, rot: -9, mark: "macos" },
  { src: "product-3", x: 77, y: 66, size: 14, depth: 0.6, rot: 3, mark: "linux" },
  { src: "near-2", x: 56, y: 6, size: 15, depth: 0.9, rot: 0 },
  { src: "near-1", x: 3, y: 92, size: 19, depth: 1, rot: 0, soft: true },
  { src: "near-3", x: 92, y: 96, size: 19, depth: 0.95, rot: 0 },
];

/** Each render's size in px and, for the product keys, its top face's corners (spaces-site float-sprites.json). */
const SPRITES: Record<string, { w: number; h: number; quad?: number[][] }> = {
  "far-1": { w: 236, h: 220 },
  "far-2": { w: 225, h: 212 },
  "far-3": { w: 220, h: 220 },
  "far-4": { w: 237, h: 226 },
  "ivory-1": { w: 397, h: 383 },
  "ivory-3": { w: 392, h: 393 },
  "ivory-5": { w: 352, h: 350 },
  "ivory-6": { w: 365, h: 398 },
  "product-1": {
    w: 486,
    h: 470,
    quad: [
      [105, 78],
      [325, 22],
      [415, 215],
      [165, 268],
    ],
  },
  "product-2": {
    w: 494,
    h: 481,
    quad: [
      [100, 92],
      [300, 15],
      [408, 222],
      [185, 298],
    ],
  },
  "product-3": {
    w: 491,
    h: 477,
    quad: [
      [95, 82],
      [318, 12],
      [410, 210],
      [172, 282],
    ],
  },
  "product-4": { w: 484, h: 469 },
  "near-1": { w: 473, h: 397 },
  "near-2": { w: 467, h: 471 },
  "near-3": { w: 463, h: 459 },
};

/** The blue key's face is printed dark, the way the site prints its template mark there; the navy and copper keys in white. */
const MARK_STYLE: Record<MarkOs, CSSProperties> = {
  windows: { color: "#000", opacity: 0.8, mixBlendMode: "multiply" },
  macos: { color: "#fff", opacity: 0.88 },
  linux: { color: "#fff", opacity: 0.88 },
};

/** The site's frame for windows near 3:2 (1512 x 982): its layout is drawn on it and scaled to cover. */
const FRAME = { w: 1486, h: 956 };

/** CSS matrix3d that maps a 100 x 100 box onto four points (the site's `homography`). */
export function homography(q: number[][]): string {
  const [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q as unknown as [[number, number], [number, number], [number, number], [number, number]];
  const dx1 = x1 - x2,
    dx2 = x3 - x2,
    dy1 = y1 - y2,
    dy2 = y3 - y2,
    sx = x0 - x1 + x2 - x3,
    sy = y0 - y1 + y2 - y3;
  const den = dx1 * dy2 - dx2 * dy1;
  const g = (sx * dy2 - dx2 * sy) / den,
    h = (dx1 * sy - sx * dy1) / den;
  const a = x1 - x0 + g * x1,
    b = x3 - x0 + h * x3,
    d = y1 - y0 + g * y1,
    e = y3 - y0 + h * y3;
  return `matrix3d(${a / 100},${d / 100},0,${g / 100},${b / 100},${e / 100},0,${h / 100},0,0,1,0,${x0},${y0},0,1)`;
}

/** The gap kept between the copy's right edge and the nearest key, in px. */
export const KEY_GAP = 24;

/**
 * Where each key goes in a box of `w` x `h` whose copy ends `copyRight` px from the left: the frame
 * scaled to cover the box, then (like the site's `wide`) the keys right of centre pressed into the
 * space right of the copy when the copy runs past the middle. No key ever sits on the copy (its text,
 * its buttons and links): a key that would still reach into the copy column is moved just right of
 * it, or left out when it no longer fits there or belongs on the left. The soft key in the bottom-left
 * corner is the one exception: it is drawn faint, behind the dots, under the copy's buttons.
 */
export function placeKeys(w: number, h: number, copyRight: number): (Place & { left: number; top: number; width: number; height: number })[] {
  const s = Math.max(w / FRAME.w, h / FRAME.h);
  const ox = (w - FRAME.w * s) / 2,
    oy = (h - FRAME.h * s) / 2;
  const edge = (copyRight / w) * 100 + 3;
  const k = edge <= 50 ? 1 : (100 - Math.min(edge, 70)) / 50;
  const clear = copyRight + KEY_GAP;
  return KEYS.flatMap((p) => {
    const sp = SPRITES[p.src]!;
    let x = ((ox + (p.x / 100) * FRAME.w * s) / w) * 100;
    let size = (p.size * FRAME.w * s) / w;
    const y = ((oy + (p.y / 100) * FRAME.h * s) / h) * 100;
    if (k < 1 && x >= 40) {
      x = 100 - (100 - x) * k;
      size *= Math.max(0.8, k);
    }
    const width = (size / 100) * w;
    const height = (width * sp.h) / sp.w;
    let left = (x / 100) * w - width / 2;
    if (!p.soft && left < clear) {
      // A key of the left half has no place right of the copy; one that would hang mostly off the right edge neither.
      if (p.x < 40 || clear + width * 0.4 > w) return [];
      left = clear;
      x = ((left + width / 2) / w) * 100;
    }
    return [{ ...p, x, y, size, width, height, left, top: (y / 100) * h - height / 2 }];
  });
}

/** The site's 56 twinkling stars (same seed). */
const STARS = (() => {
  let s = 7;
  const r = () => ((s = (s * 16807) % 2147483647) - 1) / 2147483646;
  return Array.from({ length: 56 }, () => ({ x: r() * 100, y: r() * 100, d: 1.2 + r() * 1.1, o: 0.55 + r() ** 2 * 0.45, t: 2.4 + r() * 4, delay: -r() * 6 }));
})();

/** Each key's idle loop (the site's): a slow drift and a slight tumble. */
const loop = (i: number) => ({ dur: 8 + ((i * 2.7) % 6), delay: -((i * 1.9) % 8), dx: 3 + (i % 4) * 2, dy: 4 + ((i + 1) % 3) * 3, dr: 2 + (i % 3) });

/** The starfield and keys, filling the page. `copyRight`: where the page's text column ends, in px. */
export function FloatKeys({ copyRight }: { copyRight: number }) {
  const box = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // Too narrow for keys beside the copy: the starfield alone.
  const keys = size && size.w >= 860 ? placeKeys(size.w, size.h, copyRight) : [];
  return (
    <div ref={box} aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden bg-[#050607]" data-float-keys="">
      <div className="absolute -inset-3">
        <img
          src="/onboarding/float/space-1920.webp"
          alt=""
          draggable={false}
          className="absolute inset-0 size-full object-cover brightness-[1.7] contrast-[1.08] select-none"
        />
        {STARS.map((s, i) => (
          <span
            key={i}
            className="first-run-star"
            style={
              {
                left: `${s.x.toFixed(2)}%`,
                top: `${s.y.toFixed(2)}%`,
                width: s.d,
                height: s.d,
                "--o": s.o.toFixed(2),
                "--t": `${s.t.toFixed(1)}s`,
                "--d": `${s.delay.toFixed(1)}s`,
              } as CSSProperties
            }
          />
        ))}
      </div>
      {keys.map((k, i) => {
        const sp = SPRITES[k.src]!;
        const sc = k.width / sp.w;
        const l = loop(i);
        return (
          <div
            key={k.src}
            className="absolute"
            style={{ left: k.left, top: k.top, width: k.width, height: k.height, zIndex: Math.round(k.depth * 10) }}
            data-key={k.src}
            data-mark={k.mark}
          >
            {/* The site's entrance timing for this key (FloatEarly.astro). */}
            <div
              className="first-run-key-in absolute inset-0"
              style={
                {
                  "--ed": `${(0.85 + k.depth * 0.35).toFixed(2)}s`,
                  "--edl": `${(0.04 + (1 - k.depth) * 0.2 + (i % 4) * 0.04).toFixed(2)}s`,
                  "--fy": `${(24 + k.depth * 70).toFixed(0)}px`,
                  "--fs": (0.86 - k.depth * 0.12).toFixed(2),
                } as CSSProperties
              }
            >
              <div
                className="first-run-key absolute inset-0"
                style={
                  {
                    "--r": `${k.rot}deg`,
                    "--dx": `${l.dx}px`,
                    "--dy": `${l.dy}px`,
                    "--dr": `${l.dr}deg`,
                    "--dur": `${l.dur}s`,
                    "--delay": `${l.delay}s`,
                  } as CSSProperties
                }
              >
                <img
                  src={`/onboarding/float/${k.src}.webp`}
                  alt=""
                  draggable={false}
                  className="absolute inset-0 size-full select-none"
                  style={k.soft ? { opacity: 0.72 } : undefined}
                />
                {k.mark && sp.quad ? (
                  <div
                    className="absolute top-0 left-0 size-[100px]"
                    style={{ transformOrigin: "0 0", transform: homography(sp.quad.map(([x, y]) => [x! * sc, y! * sc])) }}
                  >
                    <svg viewBox={OS_MARKS[k.mark].viewBox} fill="currentColor" className="absolute inset-[24%] size-[52%]" style={MARK_STYLE[k.mark]}>
                      <path d={OS_MARKS[k.mark].d} />
                    </svg>
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
