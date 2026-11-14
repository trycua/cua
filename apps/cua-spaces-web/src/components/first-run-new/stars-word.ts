// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The welcome headline's last word, as on cua.ai (variant B2, CUA-3842): points of light drift in from
// across the screen and settle into its letterforms from left to right, each taking the word's colour at
// its place (the keys' gradient on .sc-ink); then the real text sharpens over them and the points fade.
// The canvas sits over `host` and is removed afterwards; the word keeps its laid-out box, so nothing
// moves. Callers skip this under prefers-reduced-motion: the word is then simply there.
const SETTLE = 0.6,
  SETTLE_DUR = 0.78,
  SPREAD = 0.42,
  JITTER = 0.18,
  WORD_IN = 0.95,
  WORD_DUR = 0.5,
  FADE = 1.05,
  FADE_DUR = 0.4;
/** The keys' colours, left to right: Cua Blue, the navy key lifted to read on dark, terracotta. Keep in step with .sc-ink. */
export const SUPER_INK = [
  [0x5e, 0xc0, 0xe8],
  [0x6a, 0x8b, 0xb0],
  [0xb5, 0x69, 0x4b],
] as const;
const STAR = [233, 241, 255];
const inkAt = (f: number) => {
  const s = f < 0.5 ? 0 : 1,
    u = f < 0.5 ? f * 2 : f * 2 - 1;
  const a = SUPER_INK[s]!,
    b = SUPER_INK[s + 1]!;
  return a.map((v, j) => v + (b[j]! - v) * u);
};
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
const inOut = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

export function starsWord(word: HTMLElement, host: HTMLElement): () => void {
  // No Web Animations (an older web view, a test page): the word is simply there.
  if (typeof word.animate !== "function") return () => {};
  const rest = getComputedStyle(word).filter.replace(/^none$/, "");
  const fade = word.animate(
    [
      { opacity: 0, filter: `blur(6px) ${rest}` },
      { opacity: 1, filter: `blur(0px) ${rest}` },
    ],
    { duration: WORD_DUR * 1000, delay: (SETTLE + WORD_IN) * 1000, easing: "cubic-bezier(0.33, 1, 0.68, 1)", fill: "backwards" },
  );
  const cv = document.createElement("canvas");
  cv.setAttribute("aria-hidden", "true");
  cv.style.cssText = "position:absolute;inset:0;width:100%;height:100%;z-index:25;pointer-events:none";
  let raf = 0,
    live = true;
  const start = () => {
    if (!live) return;
    const c = host.getBoundingClientRect(),
      b = word.getBoundingClientRect(),
      dpr = Math.min(2, devicePixelRatio || 1);
    cv.width = Math.round(c.width * dpr);
    cv.height = Math.round(c.height * dpr);
    host.append(cv);
    const g = cv.getContext("2d");
    if (!g) return;
    g.scale(dpr, dpr);
    const cs = getComputedStyle(word),
      off = document.createElement("canvas");
    off.width = Math.ceil(b.width) + 8;
    off.height = Math.ceil(b.height) + 8;
    const o = off.getContext("2d", { willReadFrequently: true });
    if (!o) return;
    o.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
    if ("letterSpacing" in o) (o as CanvasRenderingContext2D & { letterSpacing: string }).letterSpacing = cs.letterSpacing;
    const m = o.measureText(word.textContent ?? "");
    const asc = m.fontBoundingBoxAscent,
      desc = m.fontBoundingBoxDescent;
    o.fillText(word.textContent ?? "", 0, (b.height - (asc + desc)) / 2 + asc);
    const px = o.getImageData(0, 0, off.width, off.height).data;
    const step = b.height > 56 ? 3 : 2.5,
      ox = b.left - c.left,
      oy = b.top - c.top;
    let seed = 7;
    const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
    const P: { tx: number; ty: number; sx: number; sy: number; r: number; tw: number; d: number; c: number[] }[] = [];
    for (let y = 0; y < off.height; y += step)
      for (let x = 0; x < off.width; x += step) {
        if ((px[(Math.round(y) * off.width + Math.round(x)) * 4 + 3] ?? 0) <= 140) continue;
        P.push({
          tx: ox + x,
          ty: oy + y,
          sx: rnd() * c.width,
          sy: rnd() * c.height,
          r: 0.8 + rnd() * 1.1,
          tw: rnd() * 6.28,
          d: (x / b.width) * SPREAD + rnd() * JITTER,
          c: inkAt(clamp01(x / b.width)),
        });
      }
    fade.currentTime = 0;
    const t0 = performance.now(),
      END = SETTLE + SPREAD + JITTER + SETTLE_DUR;
    const draw = (now: number) => {
      const t = (now - t0) / 1000;
      g.clearRect(0, 0, c.width, c.height);
      const out = 1 - clamp01((t - (SETTLE + FADE)) / FADE_DUR),
        appear = clamp01(t / 0.4);
      for (const p of P) {
        const k = inOut(clamp01((t - SETTLE - p.d) / SETTLE_DUR));
        g.globalAlpha = ((0.35 + 0.35 * Math.sin(p.tw + t * 3)) * (1 - k) + 0.95 * k) * out * appear;
        g.fillStyle = `rgb(${STAR.map((v, j) => Math.round(v + (p.c[j]! - v) * k)).join(",")})`;
        const r = p.r * (1 - k) + 1.6 * k;
        g.fillRect(p.sx + (p.tx - p.sx) * k - r / 2, p.sy + (p.ty - p.sy) * k - r / 2, r, r);
      }
      if (t < Math.max(END, SETTLE + FADE + FADE_DUR) && live) raf = requestAnimationFrame(draw);
      else cv.remove();
    };
    raf = requestAnimationFrame(draw);
  };
  (document.fonts?.ready ?? Promise.resolve()).then(start);
  return () => {
    live = false;
    cancelAnimationFrame(raf);
    cv.remove();
    fade.cancel();
  };
}
