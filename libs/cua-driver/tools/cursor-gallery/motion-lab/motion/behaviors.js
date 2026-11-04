// Micro-behaviours: post-processors applied to a generated segment, and idle
// functions that animate the cursor while it waits at a point.
//
// Post-processors mutate and return the sample array; they must keep the first
// and last samples exactly where they were.

import { DT_MS, clamp, dist, ease, lerp, perp, unit } from './math.js';

// Smooth 1-D gradient noise in roughly [-1, 1], one lattice cell per unit.
export function noise1D(rng, cells = 64) {
  const g = Array.from({ length: cells + 1 }, () => rng.range(-1, 1));
  return (x) => {
    const xi = ((Math.floor(x) % cells) + cells) % cells;
    const f = x - Math.floor(x);
    const a = g[xi] * f;
    const b = g[xi + 1] * (f - 1);
    const s = f * f * (3 - 2 * f);
    return 2 * lerp(a, b, s);
  };
}

// Physiological tremor: two incommensurate 8-12 Hz components per axis.
export function tremorField(rng, { amp = 0.4, hz = [8, 12] } = {}) {
  const comps = [0, 1, 2, 3].map(() => ({
    f: rng.range(hz[0], hz[1]),
    ph: rng.range(0, Math.PI * 2),
    w: rng.range(0.4, 1),
  }));
  const norm = amp / Math.max(1e-6, comps[0].w + comps[1].w);
  return (tMs) => {
    const t = tMs / 1000;
    return {
      x:
        norm *
        (comps[0].w * Math.sin(2 * Math.PI * comps[0].f * t + comps[0].ph) +
          comps[1].w * Math.sin(2 * Math.PI * comps[1].f * t + comps[1].ph)),
      y:
        norm *
        (comps[2].w * Math.sin(2 * Math.PI * comps[2].f * t + comps[2].ph) +
          comps[3].w * Math.sin(2 * Math.PI * comps[3].f * t + comps[3].ph)),
    };
  };
}

// Window that is 0 at both ends and 1 in the middle (rise/fall in ms).
function window(t, T, rise) {
  if (T <= 0) return 0;
  const r = Math.min(rise, T / 2);
  if (t < r) return ease.inOutSine(t / r);
  if (t > T - r) return ease.inOutSine((T - t) / r);
  return 1;
}

// Add tremor to a moving segment, suppressed at speed (tremor is masked by
// the movement itself) and faded to zero at both ends.
export function addTremor(samples, rng, { amp = 0.4, hz = [8, 12], speedMask = 600 } = {}) {
  const f = tremorField(rng, { amp, hz });
  const T = samples[samples.length - 1].t;
  for (let i = 1; i < samples.length - 1; i++) {
    const q = samples[i];
    const p = samples[i - 1];
    const v = (Math.hypot(q.x - p.x, q.y - p.y) / Math.max(1e-6, q.t - p.t)) * 1000;
    const k = window(q.t, T, 60) / (1 + v / speedMask);
    const o = f(q.t);
    q.x += o.x * k;
    q.y += o.y * k;
  }
  return samples;
}

// Low-frequency perpendicular displacement (Perlin wander / OU-like drift),
// tapered by sin(pi s) so endpoints are untouched.
export function perpendicularNoise(samples, rng, { ampFrac = 0.04, freq = 2, maxPx = 40 } = {}) {
  const a = samples[0];
  const b = samples[samples.length - 1];
  const D = dist(a, b);
  if (D < 1) return samples;
  const n = perp(unit(a, b));
  const noise = noise1D(rng);
  const off = rng.range(0, 32);
  const amp = Math.min(maxPx, D * ampFrac);
  for (let i = 1; i < samples.length - 1; i++) {
    const q = samples[i];
    const s = clamp(((q.x - a.x) * (b.x - a.x) + (q.y - a.y) * (b.y - a.y)) / (D * D), 0, 1);
    const k = amp * Math.sin(Math.PI * s) * noise(off + s * freq);
    q.x += n.x * k;
    q.y += n.y * k;
  }
  return samples;
}

// Ornstein-Uhlenbeck lateral drift, integrated over the segment and tapered.
export function ouDrift(samples, rng, { theta = 6, sigma = 2 } = {}) {
  const a = samples[0];
  const b = samples[samples.length - 1];
  const n = perp(unit(a, b));
  const T = samples[samples.length - 1].t;
  let x = 0;
  for (let i = 1; i < samples.length - 1; i++) {
    const dt = (samples[i].t - samples[i - 1].t) / 1000;
    x += -theta * x * dt + sigma * 6 * Math.sqrt(dt) * rng.normal();
    const k = window(samples[i].t, T, Math.min(150, T / 3)) * x;
    samples[i].x += n.x * k;
    samples[i].y += n.y * k;
  }
  return samples;
}

// Mouse hardware realism: sample-and-hold at the USB polling rate and snap to
// whole pixels.
export function quantize(samples, { hz = 125, px = 1 } = {}) {
  const period = 1000 / hz;
  let held = { x: samples[0].x, y: samples[0].y };
  let next = period;
  for (let i = 1; i < samples.length - 1; i++) {
    const q = samples[i];
    if (q.t >= next) {
      held = { x: Math.round(q.x / px) * px, y: Math.round(q.y / px) * px };
      next += period * Math.ceil((q.t - next + 1e-9) / period);
    }
    q.x = held.x;
    q.y = held.y;
  }
  return samples;
}

// Critically damped (or underdamped) smoothing of a sample stream, then a
// settle tail until the output reaches the final point. Screen Studio style.
export function springSmooth(samples, { freq = 4, zeta = 1, settlePx = 0.25 } = {}) {
  const w = 2 * Math.PI * freq;
  const out = [{ ...samples[0] }];
  let x = samples[0].x;
  let y = samples[0].y;
  let vx = 0;
  let vy = 0;
  const end = samples[samples.length - 1];
  const step = (gx, gy, dt) => {
    const n = 6;
    for (let k = 0; k < n; k++) {
      const sdt = dt / n;
      vx += (w * w * (gx - x) - 2 * zeta * w * vx) * sdt;
      vy += (w * w * (gy - y) - 2 * zeta * w * vy) * sdt;
      x += vx * sdt;
      y += vy * sdt;
    }
  };
  for (let i = 1; i < samples.length; i++) {
    step(samples[i].x, samples[i].y, (samples[i].t - samples[i - 1].t) / 1000);
    out.push({ ...samples[i], x, y });
  }
  let t = end.t;
  for (let i = 0; i < 600; i++) {
    if (Math.hypot(end.x - x, end.y - y) < settlePx && Math.hypot(vx, vy) < 5) break;
    step(end.x, end.y, DT_MS / 1000);
    t += DT_MS;
    out.push({ t, x, y });
  }
  out.push({ t: t + DT_MS, x: end.x, y: end.y });
  return out;
}

// 1-Euro filter (Casiez et al. 2012) over a sample stream.
export function oneEuro(samples, { minCutoff = 1, beta = 0.007, dCutoff = 1 } = {}) {
  const alpha = (cutoff, dt) => {
    const tau = 1 / (2 * Math.PI * cutoff);
    return 1 / (1 + tau / dt);
  };
  const out = [{ ...samples[0] }];
  let px = samples[0].x;
  let py = samples[0].y;
  let dx = 0;
  let dy = 0;
  for (let i = 1; i < samples.length; i++) {
    const dt = Math.max(1e-4, (samples[i].t - samples[i - 1].t) / 1000);
    const rx = (samples[i].x - px) / dt;
    const ry = (samples[i].y - py) / dt;
    const ad = alpha(dCutoff, dt);
    dx = lerp(dx, rx, ad);
    dy = lerp(dy, ry, ad);
    const cutoff = minCutoff + beta * Math.hypot(dx, dy);
    const a = alpha(cutoff, dt);
    px = lerp(px, samples[i].x, a);
    py = lerp(py, samples[i].y, a);
    out.push({ ...samples[i], x: px, y: py });
  }
  return out;
}

// Stretch along velocity (volume-preserving in the renderer).
export function stretchBySpeed(samples, { gain = 0.00018, max = 1.18 } = {}) {
  for (let i = 1; i < samples.length; i++) {
    const p = samples[i - 1];
    const q = samples[i];
    const v = (Math.hypot(q.x - p.x, q.y - p.y) / Math.max(1e-6, q.t - p.t)) * 1000;
    q.sq = Math.min(max, 1 + v * gain);
  }
  samples[samples.length - 1].sq = 1;
  return samples;
}

// ---------------------------------------------------------------------------
// Idle functions: offset(tauMs, totalMs) around a rest point. Each returns
// {x, y} and optionally {scale, rot, opacity}. Offsets start and end at zero.
// ---------------------------------------------------------------------------

export const idles = {
  still: () => () => ({ x: 0, y: 0 }),
  tremor: (rng, { amp = 0.35, driftPxPerS = 1.2 } = {}) => {
    const f = tremorField(rng, { amp });
    const dir = rng.range(0, Math.PI * 2);
    return (t, T) => {
      const w = window(t, T, 40);
      const drift = (driftPxPerS * Math.min(t, T - t)) / 1000;
      const o = f(t);
      return { x: (o.x + Math.cos(dir) * drift) * w, y: (o.y + Math.sin(dir) * drift) * w };
    };
  },
  wiggle: (rng, { amp = 4, hz = 1.1 } = {}) => {
    const nx = noise1D(rng);
    const ny = noise1D(rng);
    const off = rng.range(0, 30);
    return (t, T) => {
      const w = window(t, T, 120);
      const s = off + (t / 1000) * hz * 2;
      // Bursty: amplitude itself breathes on a slower noise.
      const burst = 0.55 + 0.45 * Math.abs(nx(off * 0.5 + (t / 1000) * 0.6));
      return { x: nx(s) * amp * w * burst, y: ny(s + 7) * amp * 0.7 * w * burst };
    };
  },
  figureEight: (rng, { a = 9, b = 4.5, periodMs = 2000 } = {}) => {
    const ph = rng.sign();
    return (t, T) => {
      const w = window(t, T, 200);
      const th = (2 * Math.PI * t) / periodMs;
      return { x: ph * a * Math.sin(th) * w, y: b * Math.sin(2 * th) * w };
    };
  },
  breathe:
    (_rng, { scale = 0.05, periodMs = 3200, bob = 1.5 } = {}) =>
    (t, T) => {
      const w = window(t, T, 200);
      const s = Math.sin((2 * Math.PI * t) / periodMs);
      return {
        x: 0,
        y: -bob * (0.5 - 0.5 * Math.cos((2 * Math.PI * t) / periodMs)) * w,
        scale: 1 + scale * 0.5 * (1 + s) * w,
      };
    },
  rotWobble: (rng, { deg = 1 } = {}) => {
    const n = noise1D(rng);
    return (t, T) => ({
      x: 0,
      y: 0,
      rot: ((deg * Math.PI) / 180) * n(t / 600) * window(t, T, 100),
    });
  },
};

// Damped oscillation used for nod / shake / click wobble.
export function dampedOsc(t, T, cycles, zeta = 0.25) {
  const tau = clamp(t / T, 0, 1);
  return Math.sin(2 * Math.PI * cycles * tau) * Math.exp(-zeta * 8 * tau) * (1 - tau);
}
