// Segment generators: one cursor movement from `ctx.from` to `ctx.aim`.
//
// Every generator returns an array of samples `{ t, x, y }` with `t` in
// milliseconds from the start of the segment. The first sample is exactly
// `from` and the last is exactly `aim`. Optional channels: `opacity`, `sq`
// (stretch factor), `pressed`.

import {
  DT_MS,
  add,
  arcPath,
  asymmetricMinJerk,
  catmullRom,
  clamp,
  cuaBezier,
  cubic,
  dist,
  dubinsPath,
  ease,
  lerp,
  lerpPt,
  lognormalCdf,
  makePath,
  perp,
  quad,
  sub,
  unit,
} from './math.js';

// ---------------------------------------------------------------------------
// Durations
// ---------------------------------------------------------------------------

export const durations = {
  // Fitts' law (Shannon form): MT = a + b * log2(D / W + 1).
  fitts: (D, W, a = 90, b = 140) => a + b * Math.log2(D / Math.max(4, W) + 1),
  // Screen-recording tools tend to use a sqrt(distance) cadence.
  sqrt: (D, k = 24, lo = 200, hi = 950) => clamp(k * Math.sqrt(D), lo, hi),
  // Constant average speed (px/s).
  speed: (D, v = 900, lo = 120) => Math.max(lo, (D / v) * 1000),
  fixed: (_D, ms = 600) => ms,
};

export function targetWidth(ctx) {
  return Math.max(4, Math.min(ctx.target.w, ctx.target.h));
}

// ---------------------------------------------------------------------------
// Sampling helpers
// ---------------------------------------------------------------------------

export function sampleTimed(pos, durationMs) {
  const n = Math.max(2, Math.ceil(durationMs / DT_MS));
  const out = [];
  for (let i = 0; i <= n; i++) {
    const tau = i / n;
    const p = pos(tau);
    out.push({ t: tau * durationMs, x: p.x, y: p.y });
  }
  return out;
}

export function pinEnds(samples, from, aim) {
  samples[0].x = from.x;
  samples[0].y = from.y;
  const last = samples[samples.length - 1];
  last.x = aim.x;
  last.y = aim.y;
  return samples;
}

// Concatenate segments that each start where the previous one ended.
export function chain(parts) {
  const out = [];
  let t0 = 0;
  for (const part of parts) {
    if (!part.length) continue;
    const start = out.length ? 1 : 0;
    for (let i = start; i < part.length; i++) out.push({ ...part[i], t: part[i].t + t0 });
    t0 = out[out.length - 1].t;
  }
  return out;
}

// Hold still (or idle-wiggle via `offset(tau)`) at `p` for `ms`.
export function hold(p, ms, offset) {
  if (ms <= 0) return [{ t: 0, x: p.x, y: p.y }];
  return sampleTimed((tau) => (offset ? add(p, offset(tau)) : { x: p.x, y: p.y }), ms);
}

// ---------------------------------------------------------------------------
// Path builders. Each returns a path object from math.makePath.
// ---------------------------------------------------------------------------

export const paths = {
  line: (a, b) => makePath((u) => lerpPt(a, b, u), 8),
  cua: (a, b, o) => makePath(cuaBezier(a, b, o)),
  arc: (a, b, bulge) => makePath(arcPath(a, b, bulge)),
  // Cubic bezier with control points scattered around the chord, the
  // "random bezier" humaniser used by many bot frameworks.
  randomBezier: (a, b, rng, spread = 0.3) => {
    const D = dist(a, b);
    const u = unit(a, b);
    const n = perp(u);
    const side = rng.sign();
    const c1 = add(lerpPt(a, b, rng.range(0.15, 0.4)), {
      x: n.x * side * D * rng.range(0.02, spread),
      y: n.y * side * D * rng.range(0.02, spread),
    });
    const c2 = add(lerpPt(a, b, rng.range(0.6, 0.9)), {
      x: n.x * side * D * rng.range(-spread * 0.4, spread * 0.6),
      y: n.y * side * D * rng.range(-spread * 0.4, spread * 0.6),
    });
    return makePath(cubic(a, c1, c2, b));
  },
  // Gentle single-sided curve with the apex `bias` along the chord.
  bow: (a, b, amount, bias = 0.5) => {
    const D = dist(a, b);
    const n = perp(unit(a, b));
    const c = add(lerpPt(a, b, bias), { x: n.x * amount * D, y: n.y * amount * D });
    return makePath(quad(a, c, b));
  },
  through: (points) => makePath(catmullRom(points), 64 * points.length),
};

// Chord-side sign that bends paths consistently "outward" on screen, so a
// tile reads as one style instead of random left/right flips.
export function naturalSide(a, b) {
  const u = unit(a, b);
  // Bend so the arc bulges upward for horizontal moves (like a wrist pivot).
  return u.x >= 0 ? -1 : 1;
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

// The general path x velocity-profile x duration generator.
export function glide(ctx, { path, profile = ease.minJerk, durationMs }) {
  const p = path ?? paths.line(ctx.from, ctx.aim);
  const samples = sampleTimed((tau) => p.atFraction(profile(tau)), durationMs);
  return pinEnds(samples, ctx.from, ctx.aim);
}

// Speed law along a path: v = min(vmax, accel * t, vmin + gain * remaining).
// Covers "cruise then precise approach" style motion without a fixed duration.
export function speedLaw(
  ctx,
  { path, vmax = 1400, accel = 9000, vmin = 40, gain = 6, maxMs = 4000 }
) {
  const p = path ?? paths.line(ctx.from, ctx.aim);
  const L = p.length;
  const out = [{ t: 0, x: ctx.from.x, y: ctx.from.y }];
  let s = 0;
  let v = 0;
  let t = 0;
  const dt = DT_MS / 1000;
  while (s < L && t * 1000 < maxMs) {
    const remaining = L - s;
    const target = Math.min(vmax, vmin + gain * remaining);
    // Accelerate at `accel`, brake instantly to the remaining-distance law.
    v = Math.min(target, v + accel * dt);
    s = Math.min(L, s + v * dt);
    t += dt;
    const q = p.atFraction(L > 0 ? s / L : 1);
    out.push({ t: t * 1000, x: q.x, y: q.y });
  }
  return pinEnds(out, ctx.from, ctx.aim);
}

// Today's Cua Driver glide: Dubins arc-straight-arc path, smootherstep speed
// envelope between start/end floor speeds, then a spring settle that
// overshoots along the arrival heading (render_state.rs tick_swift_constants).
export function cuaToday(ctx, o = {}) {
  const {
    peakSpeed = 900,
    minStart = 300,
    minEnd = 200,
    springK = 400,
    springC = 17,
    overshoot = 0.8,
    turnRadius = 80,
    restHeading = Math.PI / 4,
  } = o;
  const th1 = restHeading + Math.PI;
  const th0 = ctx.state.cuaHeading ?? th1;
  const dub = dubinsPath(ctx.from, th0, ctx.aim, th1, turnRadius);
  const p = dub ? makePath(dub.fn, 512) : paths.line(ctx.from, ctx.aim);
  const L = Math.max(1, p.length);
  const dt = DT_MS / 1000;
  const out = [{ t: 0, x: ctx.from.x, y: ctx.from.y }];
  let s = 0;
  let t = 0;
  let speed = 0;
  while (s < L) {
    const u = Math.min(1, s / L);
    const profile = (30 * u * u * (1 - u) * (1 - u)) / 1.875;
    const floor = u < 0.5 ? minStart : minEnd;
    speed = floor + (peakSpeed - floor) * profile;
    s += speed * dt;
    t += dt;
    const q = p.atFraction(Math.min(1, s / L));
    out.push({ t: t * 1000, x: q.x, y: q.y });
  }
  // Spring settle around the aim, kicked along the arrival heading.
  const end = ctx.aim;
  const before = p.atFraction(0.995);
  const h = Math.atan2(end.y - before.y, end.x - before.x);
  let ox = 0;
  let oy = 0;
  let vx = speed * overshoot * Math.cos(h);
  let vy = speed * overshoot * Math.sin(h);
  out[out.length - 1].x = end.x;
  out[out.length - 1].y = end.y;
  for (let i = 0; i < 400; i++) {
    const sub = 4;
    for (let k = 0; k < sub; k++) {
      const sdt = dt / sub;
      vx += (-springK * ox - springC * vx) * sdt;
      vy += (-springK * oy - springC * vy) * sdt;
      ox += vx * sdt;
      oy += vy * sdt;
    }
    t += dt;
    if (Math.hypot(ox, oy) < 0.3 && Math.hypot(vx, vy) < 2) break;
    out.push({ t: t * 1000, x: end.x + ox, y: end.y + oy });
  }
  out.push({ t: (t + dt) * 1000, x: end.x, y: end.y });
  ctx.state.cuaHeading = th1;
  return out;
}

// Benjamin Land's WindMouse: gravity toward the target plus a wandering wind
// force, velocity clipped to a random fraction of `maxStep`.
export function windMouse(
  ctx,
  { gravity = 9, wind = 3, maxStep = 15, damp = 12, stepMs = 6, scaleSpeed = 1 }
) {
  const rng = ctx.rng;
  const s3 = Math.sqrt(3);
  const s5 = Math.sqrt(5);
  let { x, y } = ctx.from;
  const { x: xe, y: ye } = ctx.aim;
  let vx = 0;
  let vy = 0;
  let wx = 0;
  let wy = 0;
  let M = maxStep * scaleSpeed;
  const raw = [{ t: 0, x, y }];
  let t = 0;
  let d = Math.hypot(xe - x, ye - y);
  let guard = 0;
  while (d >= 1 && guard++ < 2000) {
    const wMag = Math.min(wind, d);
    if (d >= damp) {
      wx = wx / s3 + ((2 * rng.next() - 1) * wMag) / s5;
      wy = wy / s3 + ((2 * rng.next() - 1) * wMag) / s5;
    } else {
      wx /= s3;
      wy /= s3;
      if (M < 3) M = rng.next() * 3 + 3;
      else M /= s5;
    }
    vx += wx + (gravity * (xe - x)) / d;
    vy += wy + (gravity * (ye - y)) / d;
    const vMag = Math.hypot(vx, vy);
    if (vMag > M) {
      const clip = M / 2 + (rng.next() * M) / 2;
      vx = (vx / vMag) * clip;
      vy = (vy / vMag) * clip;
    }
    x += vx;
    y += vy;
    t += stepMs;
    raw.push({ t, x, y });
    d = Math.hypot(xe - x, ye - y);
  }
  raw.push({ t: t + stepMs, x: xe, y: ye });
  return resample(raw);
}

// Plamondon's kinematic theory: velocity is a sum of overlapping lognormal
// strokes. Stroke displacements are drawn so they sum exactly to the target.
export function sigmaLognormal(
  ctx,
  { strokes = 3, mu = -1.6, sigma = 0.28, overlap = 0.55, primary = 0.93, lateral = 0.06 }
) {
  const rng = ctx.rng;
  const D = dist(ctx.from, ctx.aim);
  const u = unit(ctx.from, ctx.aim);
  const total = sub(ctx.aim, ctx.from);
  const parts = [];
  let remaining = { ...total };
  let t0 = 0;
  for (let i = 0; i < strokes; i++) {
    const last = i === strokes - 1;
    let disp;
    if (last) disp = remaining;
    else {
      const frac = i === 0 ? rng.clippedNormal(primary, 0.03) : rng.clippedNormal(0.85, 0.08);
      const lat = rng.clippedNormal(0, lateral) * D * (i === 0 ? 1 : 0.3);
      const along = (i === 0 ? D : Math.hypot(remaining.x, remaining.y)) * frac;
      const dir = i === 0 ? u : unit({ x: 0, y: 0 }, remaining);
      const dn = perp(dir);
      disp = { x: dir.x * along + dn.x * lat, y: dir.y * along + dn.y * lat };
    }
    // Bigger strokes take longer: mu grows with log amplitude.
    const amp = Math.hypot(disp.x, disp.y);
    const m = mu + 0.18 * Math.log(Math.max(1, amp) / 300) + rng.clippedNormal(0, 0.05);
    const s = clamp(sigma * (i === 0 ? 1 : 0.85) + rng.clippedNormal(0, 0.03), 0.12, 0.5);
    parts.push({ disp, t0, mu: m, sigma: s });
    remaining = sub(remaining, disp);
    // Next stroke starts while this one is decelerating.
    const peakT = t0 + Math.exp(m - s * s);
    const endT = t0 + Math.exp(m + 2.5 * s);
    t0 = lerp(peakT, endT, overlap);
  }
  const endT = Math.max(...parts.map((p) => p.t0 + Math.exp(p.mu + 3 * p.sigma)));
  const durationMs = endT * 1000;
  const samples = sampleTimed((tau) => {
    const t = tau * endT;
    let x = ctx.from.x;
    let y = ctx.from.y;
    for (const p of parts) {
      const c = lognormalCdf(t, p.t0, p.mu, p.sigma);
      x += p.disp.x * c;
      y += p.disp.y * c;
    }
    return { x, y };
  }, durationMs);
  return pinEnds(samples, ctx.from, ctx.aim);
}

// Meyer et al. optimized-submovement model: a ballistic primary minimum-jerk
// movement lands with speed-dependent error, then corrective submovements
// home in. `bias` < 0 undershoots, > 0 overshoots.
export function submovements(
  ctx,
  {
    a = 80,
    b = 130,
    bias = -0.04,
    alongSd = 0.05,
    latSd = 0.018,
    maxCorrections = 3,
    correctionMs = 150,
    gapMs = 30,
    curve = 0.06,
  }
) {
  const rng = ctx.rng;
  const W = targetWidth(ctx);
  const parts = [];
  let pos = { ...ctx.from };
  const D = dist(pos, ctx.aim);
  const u = unit(pos, ctx.aim);
  const n = perp(u);
  const along = D * (1 + rng.clippedNormal(bias, alongSd));
  const lat = D * rng.clippedNormal(0, latSd);
  let land = { x: pos.x + u.x * along + n.x * lat, y: pos.y + u.y * along + n.y * lat };
  const primaryMs = durations.fitts(D, W, a, b) * 0.8;
  parts.push(
    glide(
      { from: pos, aim: land },
      {
        path: paths.bow(pos, land, curve * naturalSide(pos, land)),
        profile: asymmetricMinJerk(0.45),
        durationMs: primaryMs,
      }
    )
  );
  pos = land;
  for (let i = 0; i < maxCorrections; i++) {
    const err = dist(pos, ctx.aim);
    if (err < 0.5) break;
    const last = i === maxCorrections - 1 || err < W * 0.18;
    let next;
    if (last) next = { ...ctx.aim };
    else {
      const cu = unit(pos, ctx.aim);
      const cn = perp(cu);
      const ca = err * (1 + rng.clippedNormal(bias * 0.5, 0.12));
      const cl = err * rng.clippedNormal(0, 0.08);
      next = { x: pos.x + cu.x * ca + cn.x * cl, y: pos.y + cu.y * ca + cn.y * cl };
    }
    if (gapMs > 0) parts.push(hold(pos, gapMs));
    const ms = correctionMs * (0.7 + (0.3 * Math.log2(1 + err / 4)) / 3);
    parts.push(glide({ from: pos, aim: next }, { profile: ease.minJerk, durationMs: ms }));
    pos = next;
    if (last) break;
  }
  if (dist(pos, ctx.aim) > 1e-6)
    parts.push(glide({ from: pos, aim: ctx.aim }, { profile: ease.minJerk, durationMs: 90 }));
  return chain(parts);
}

// Second-order follower: x'' = w^2 (goal(t) - x) - 2 zeta w x'.
// With a step goal this is a spring; with a moving goal it is a smoother
// (critically damped at zeta = 1, bouncy below).
export function springFollow(
  ctx,
  { freq = 3.2, zeta = 0.5, goal, goalMs = 0, maxMs = 3000, settlePx = 0.25, settleV = 6, v0 }
) {
  const w = 2 * Math.PI * freq;
  const dt = DT_MS / 1000;
  const subSteps = 8;
  let x = ctx.from.x;
  let y = ctx.from.y;
  let vx = v0?.x ?? 0;
  let vy = v0?.y ?? 0;
  const out = [{ t: 0, x, y }];
  let t = 0;
  while (t * 1000 < maxMs) {
    for (let k = 0; k < subSteps; k++) {
      const st = t + (k * dt) / subSteps;
      const g = goal ? goal(Math.min(1, (st * 1000) / Math.max(1, goalMs))) : ctx.aim;
      const sdt = dt / subSteps;
      vx += (w * w * (g.x - x) - 2 * zeta * w * vx) * sdt;
      vy += (w * w * (g.y - y) - 2 * zeta * w * vy) * sdt;
      x += vx * sdt;
      y += vy * sdt;
    }
    t += dt;
    out.push({ t: t * 1000, x, y });
    const settled =
      t * 1000 >= goalMs &&
      Math.hypot(ctx.aim.x - x, ctx.aim.y - y) < settlePx &&
      Math.hypot(vx, vy) < settleV;
    if (settled) break;
  }
  out.push({ t: (t + dt) * 1000, x: ctx.aim.x, y: ctx.aim.y });
  return out;
}

// Steve Ruiz's perfect-cursors idea: a sparse, jittery stream of positions
// (e.g. network updates every ~80 ms) is replayed through a spline so the
// in-between frames are smooth while every received point is honoured.
export function perfectCursors(ctx, { intervalMs = 80, jitter = 6, sourceMs }) {
  const rng = ctx.rng;
  const D = dist(ctx.from, ctx.aim);
  const W = targetWidth(ctx);
  const ms = sourceMs ?? durations.fitts(D, W, 120, 150);
  const src = paths.bow(ctx.from, ctx.aim, 0.08 * naturalSide(ctx.from, ctx.aim), 0.45);
  const keys = [];
  const count = Math.max(2, Math.round(ms / intervalMs));
  for (let i = 0; i <= count; i++) {
    const tau = i / count;
    const q = src.atFraction(ease.minJerk(tau));
    const j = i === 0 || i === count ? 0 : jitter * Math.sin(Math.PI * tau);
    keys.push({ x: q.x + rng.normal(0, j), y: q.y + rng.normal(0, j) });
  }
  keys[0] = { ...ctx.from };
  keys[keys.length - 1] = { ...ctx.aim };
  const spline = catmullRom(keys);
  // Uniform time per key interval, as perfect-cursors animates one spline
  // segment per received update.
  const samples = sampleTimed((tau) => spline(tau), count * intervalMs);
  return pinEnds(samples, ctx.from, ctx.aim);
}

// Trackpad flick: an inertial throw that decays exponentially and lands
// short or wide, followed by a slow deliberate finger adjustment.
export function trackpadFlick(
  ctx,
  { reach = 0.9, lateral = 0.05, tauMs = 110, adjustMs = 260, pauseMs = 70 }
) {
  const rng = ctx.rng;
  const D = dist(ctx.from, ctx.aim);
  const u = unit(ctx.from, ctx.aim);
  const n = perp(u);
  const r = clamp(rng.clippedNormal(reach, 0.04), 0.7, 0.98);
  const land = add(ctx.from, {
    x: u.x * D * r + n.x * D * rng.clippedNormal(0, lateral),
    y: u.y * D * r + n.y * D * rng.clippedNormal(0, lateral),
  });
  // x(t) = land * (1 - e^{-t/tau}) with a short finger ramp-in.
  const flickMs = tauMs * 5;
  const flick = sampleTimed((tau) => {
    const t = tau * flickMs;
    const ramp = ease.inOutSine(Math.min(1, t / 40));
    const k = (1 - Math.exp(-t / tauMs)) / (1 - Math.exp(-5));
    return lerpPt(ctx.from, land, k * ramp + (1 - ramp) * k * 0.5);
  }, flickMs);
  pinEnds(flick, ctx.from, land);
  const adjust = glide(
    { from: land, aim: ctx.aim },
    { profile: ease.inOutSine, durationMs: adjustMs }
  );
  return chain([flick, hold(land, pauseMs), adjust]);
}

// Bouncing-ball hops: each hop is a parabola, the number of hops grows with
// distance, and each landing squashes a little.
export function hops(ctx, { hopPx = 260, height = 0.32, hopMs = 260 }) {
  const D = dist(ctx.from, ctx.aim);
  const count = Math.max(1, Math.round(D / hopPx));
  const parts = [];
  for (let i = 0; i < count; i++) {
    const a = lerpPt(ctx.from, ctx.aim, i / count);
    const b = lerpPt(ctx.from, ctx.aim, (i + 1) / count);
    const h = Math.max(18, dist(a, b) * height) * (1 - (0.35 * i) / count);
    const ms = hopMs * (i === count - 1 ? 1.15 : 1);
    const hop = sampleTimed((tau) => {
      const s = ease.inOutSine(tau);
      const p = lerpPt(a, b, s);
      return { x: p.x, y: p.y - 4 * h * s * (1 - s) };
    }, ms);
    pinEnds(hop, a, b);
    // Landing squash: stretch < 1 briefly at the contact point.
    hop.forEach((q, k) => {
      const tau = k / (hop.length - 1);
      q.sq = 1 + 0.25 * Math.sin(Math.PI * tau) - 0.18 * Math.exp(-(((tau - 1) / 0.06) ** 2));
    });
    parts.push(hop);
  }
  return chain(parts);
}

// Axis-aligned "menu-safe" move: travel horizontally then vertically (or the
// reverse) with a rounded corner, so a submenu path never crosses siblings.
export function manhattan(ctx, { corner = 0.18, durationMs, verticalFirst = false }) {
  const a = ctx.from;
  const b = ctx.aim;
  const knee = verticalFirst ? { x: a.x, y: b.y } : { x: b.x, y: a.y };
  const r = Math.min(dist(a, knee), dist(knee, b)) * corner * 2;
  const k1 = lerpPt(knee, a, Math.min(0.9, r / Math.max(1, dist(a, knee))));
  const k2 = lerpPt(knee, b, Math.min(0.9, r / Math.max(1, dist(knee, b))));
  const parts = [paths.line(a, k1), makePath(quad(k1, knee, k2)), paths.line(k2, b)];
  const lens = parts.map((p) => p.length);
  const total = lens.reduce((s, v) => s + v, 0) || 1;
  const p = makePath((u) => {
    let s = u * total;
    for (let i = 0; i < parts.length; i++) {
      if (s <= lens[i] || i === parts.length - 1)
        return parts[i].atFraction(lens[i] ? s / lens[i] : 1);
      s -= lens[i];
    }
    return b;
  }, 512);
  return glide(ctx, {
    path: p,
    profile: ease.inOutCubic,
    durationMs: durationMs ?? durations.sqrt(total, 26, 260, 1000),
  });
}

// Instant relocation: fade out, jump, fade in (reduced-motion friendly).
export function teleport(ctx, { fadeMs = 110, gapMs = 40 }) {
  const out = sampleTimed((tau) => ({ ...ctx.from }), fadeMs).map((q, i, arr) => ({
    ...q,
    opacity: 1 - i / (arr.length - 1),
  }));
  const t1 = fadeMs;
  out.push({ t: t1 + gapMs * 0.5, x: ctx.from.x, y: ctx.from.y, opacity: 0 });
  out.push({ t: t1 + gapMs, x: ctx.aim.x, y: ctx.aim.y, opacity: 0 });
  const fadeIn = sampleTimed(() => ({ ...ctx.aim }), fadeMs);
  for (let i = 1; i < fadeIn.length; i++)
    out.push({
      t: t1 + gapMs + fadeIn[i].t,
      x: ctx.aim.x,
      y: ctx.aim.y,
      opacity: i / (fadeIn.length - 1),
    });
  return out;
}

// Resample an irregular sample stream to the fixed DT grid.
export function resample(raw) {
  const T = raw[raw.length - 1].t;
  const out = [];
  let j = 0;
  const n = Math.max(1, Math.ceil(T / DT_MS));
  for (let i = 0; i <= n; i++) {
    const t = (i / n) * T;
    while (j < raw.length - 2 && raw[j + 1].t < t) j++;
    const a = raw[j];
    const b = raw[j + 1];
    const f = b.t > a.t ? clamp((t - a.t) / (b.t - a.t), 0, 1) : 1;
    out.push({ t, x: lerp(a.x, b.x, f), y: lerp(a.y, b.y, f) });
  }
  out[0] = { t: 0, x: raw[0].x, y: raw[0].y };
  out[out.length - 1] = { t: T, x: raw[raw.length - 1].x, y: raw[raw.length - 1].y };
  return out;
}

// ---------------------------------------------------------------------------
// Profile builders
// ---------------------------------------------------------------------------

// Lognormal stroke profile normalised to tau in [0, 1]: fast rise, long tail.
export function lognormalProfile(sigma = 0.25) {
  // Choose mu so the stroke is ~99.5% complete at t = 1.
  const mu = -2.6 * sigma;
  const end = lognormalCdf(1, 0, mu, sigma);
  return (tau) => (tau <= 0 ? 0 : lognormalCdf(tau, 0, mu, sigma) / end);
}

// Base profile plus smooth bumps: `antic` pulls back early (negative s),
// `over` pushes past the end late and settles. Both are fractions of D.
export function bumpProfile(base, { antic = 0, over = 0, anticAt = 0.18, overAt = 0.78 } = {}) {
  const bump = (tau, at) => {
    const a = Math.max(1.5, at * 10);
    const b = Math.max(1.5, (1 - at) * 10);
    const peak = (a / (a + b)) ** a * (b / (a + b)) ** b;
    return (tau ** a * (1 - tau) ** b) / peak;
  };
  return (tau) => base(tau) - antic * bump(tau, anticAt) + over * bump(tau, overAt);
}

// Damped wobble around the end added on top of `base` from `start` on.
// Starts with zero velocity (smoothstep ramp) and dies to zero at tau = 1.
export function wobbleProfile(base, { amp = 0.02, cycles = 2, decay = 3, start = 0.5 } = {}) {
  return (tau) => {
    if (tau <= start) return base(tau);
    const u = (tau - start) / (1 - start);
    const ramp = ease.smootherstep(Math.min(1, u / 0.18));
    return (
      base(tau) +
      (amp * ramp * Math.exp(-decay * u) * Math.sin(2 * Math.PI * cycles * u) * (1 - u) ** 2) /
        Math.max(1e-6, Math.exp(-decay * 0.12) * 0.77)
    );
  };
}

// Integrate a speed shape v(s) (s = arc-length fraction) into a timed glide
// with total duration `durationMs`. v may be any positive function.
export function speedShaped(ctx, { path, shape, durationMs, n = 600 }) {
  const p = path ?? paths.line(ctx.from, ctx.aim);
  const ts = [0];
  for (let i = 1; i <= n; i++) {
    const s = (i - 0.5) / n;
    ts.push(ts[i - 1] + 1 / Math.max(1e-3, shape(s)));
  }
  const T = ts[n];
  const samples = sampleTimed((tau) => {
    const target = tau * T;
    let lo = 0;
    let hi = n;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (ts[mid] < target) lo = mid;
      else hi = mid;
    }
    const f = (target - ts[lo]) / Math.max(1e-9, ts[hi] - ts[lo]);
    return p.atFraction((lo + f) / n);
  }, durationMs);
  return pinEnds(samples, ctx.from, ctx.aim);
}

// A bell that is zero at both ends; `rise` shifts the peak earlier (< 0.5).
export const bell = (s, rise = 0.5, floor = 0.03) => {
  const g = Math.log(0.5) / Math.log(clamp(rise, 0.2, 0.8));
  return floor + Math.sin(Math.PI * s ** g) ** 1.4;
};

// Ramer-Douglas-Peucker simplification.
export function rdp(points, eps) {
  if (points.length < 3) return points.slice();
  const a = points[0];
  const b = points[points.length - 1];
  const L = dist(a, b) || 1;
  let idx = 0;
  let dmax = 0;
  for (let i = 1; i < points.length - 1; i++) {
    const d = Math.abs((b.x - a.x) * (a.y - points[i].y) - (a.x - points[i].x) * (b.y - a.y)) / L;
    if (d > dmax) {
      dmax = d;
      idx = i;
    }
  }
  if (dmax <= eps) return [a, b];
  const left = rdp(points.slice(0, idx + 1), eps);
  const right = rdp(points.slice(idx), eps);
  return [...left.slice(0, -1), ...right];
}

// Finite-horizon LQR on a triple integrator per axis (state: offset, vel,
// acc; control: jerk). Running cost on distance gives the early, asymmetric
// velocity peak seen in real pointing data; the terminal cost lands it.
export function lqr(
  ctx,
  { durationMs, wDist = 4e3, wVel = 0, r = 1e-6, dtMs = 4, terminal = [1e9, 1e5, 1e1] }
) {
  const dt = dtMs / 1000;
  const N = Math.max(10, Math.round(durationMs / dtMs));
  const A = [
    [1, dt, (dt * dt) / 2],
    [0, 1, dt],
    [0, 0, 1],
  ];
  const B = [(dt * dt * dt) / 6, (dt * dt) / 2, dt];
  const Q = [
    [wDist, 0, 0],
    [0, wVel, 0],
    [0, 0, 0],
  ];
  // Backward Riccati recursion: K_k = (R + B'PB)^-1 B'PA.
  let P = [
    [terminal[0], 0, 0],
    [0, terminal[1], 0],
    [0, 0, terminal[2]],
  ];
  const gains = new Array(N);
  const mulMV = (M, v) => M.map((row) => row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);
  for (let k = N - 1; k >= 0; k--) {
    const PB = mulMV(P, B);
    const s = r + B[0] * PB[0] + B[1] * PB[1] + B[2] * PB[2];
    // B'PA as a row vector.
    const BPA = [0, 1, 2].map((j) => PB[0] * A[0][j] + PB[1] * A[1][j] + PB[2] * A[2][j]);
    const K = BPA.map((v) => v / s);
    gains[k] = K;
    // P = Q + A'P A - A'P B K
    const PA = P.map((row) =>
      [0, 1, 2].map((j) => row[0] * A[0][j] + row[1] * A[1][j] + row[2] * A[2][j])
    );
    const APA = [0, 1, 2].map((i) =>
      [0, 1, 2].map((j) => A[0][i] * PA[0][j] + A[1][i] * PA[1][j] + A[2][i] * PA[2][j])
    );
    const APB = [0, 1, 2].map((i) => A[0][i] * PB[0] + A[1][i] * PB[1] + A[2][i] * PB[2]);
    P = [0, 1, 2].map((i) => [0, 1, 2].map((j) => Q[i][j] + APA[i][j] - APB[i] * K[j]));
  }
  const run = (offset) => {
    let z = [offset, 0, 0];
    const xs = [offset];
    for (let k = 0; k < N; k++) {
      const u = -(gains[k][0] * z[0] + gains[k][1] * z[1] + gains[k][2] * z[2]);
      z = [
        A[0][0] * z[0] + A[0][1] * z[1] + A[0][2] * z[2] + B[0] * u,
        z[1] + dt * z[2] + B[1] * u,
        z[2] + B[2] * u,
      ];
      xs.push(z[0]);
    }
    return xs;
  };
  const xs = run(ctx.from.x - ctx.aim.x);
  const ys = run(ctx.from.y - ctx.aim.y);
  const raw = xs.map((x, k) => ({ t: k * dtMs, x: ctx.aim.x + x, y: ctx.aim.y + ys[k] }));
  raw[raw.length - 1] = { t: N * dtMs, x: ctx.aim.x, y: ctx.aim.y };
  return resample(raw);
}
