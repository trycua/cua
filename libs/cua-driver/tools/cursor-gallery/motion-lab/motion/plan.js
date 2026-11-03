// Sequence planner: runs one candidate over a scene and returns a timeline
// `{ points, events, duration, segments }` that a renderer can play back by
// time. Pure and deterministic: same (candidate, scene, seed, params) gives
// the same output.

import { DT_MS, clamp, dist, wrapAngle } from './math.js';
import { Rng } from './rng.js';
import { sampleTimed } from './generators.js';
import { idles } from './behaviors.js';
import { center, inside } from './scenes.js';

// The Cua arrow artwork points up-left at rest; the tip angle in screen space.
export const TIP_ANGLE = -0.75 * Math.PI;

export function resolveParams(candidate, overrides = {}) {
  const out = {};
  for (const [k, spec] of Object.entries(candidate.params ?? {})) out[k] = spec.v;
  return { ...out, ...overrides };
}

// Draw a range param: arrays [lo, hi] are sampled uniformly per move.
export function pick(rng, v) {
  return Array.isArray(v) ? rng.range(v[0], v[1]) : v;
}

function defaultAim(target, rng, spread) {
  const c = center(target);
  const mx = Math.max(0, target.w / 2 - Math.max(2, target.w * 0.15));
  const my = Math.max(0, target.h / 2 - Math.max(2, target.h * 0.15));
  return {
    x: c.x + clamp(rng.clippedNormal(0, target.w * spread), -mx, mx),
    y: c.y + clamp(rng.clippedNormal(0, target.h * spread), -my, my),
  };
}

// Global timing modes, applied to every travel segment by uniform time scaling
// so the path shape is untouched:
//   native - the candidate's own timing
//   fitts  - distance- and size-aware: MT = 150 + 120 log2(D / W + 1), 300-1000 ms
//   fixed  - one duration for every move (default 1430 ms, the Codex-style
//            spring move time that ignores distance)
export const TIMING_MODES = ['native', 'fitts', 'fixed'];
export function fittsTimingMs(D, W) {
  return clamp(150 + 120 * Math.log2(D / Math.max(4, W) + 1), 300, 1000);
}

function retime(samples, targetMs) {
  const T = samples[samples.length - 1].t;
  if (!(T > 0) || samples.length < 3) return samples;
  const k = targetMs / T;
  return samples.map((q) => ({ ...q, t: q.t * k }));
}

export function plan(
  candidate,
  scene,
  { seed = 1, params = {}, stage, timing = 'native', fixedMs = 1430 } = {}
) {
  const p = resolveParams(candidate, params);
  const points = [{ t: 0, x: scene.start.x, y: scene.start.y }];
  const events = [];
  const segments = [];
  const state = { pressed: false };
  let t = 0;
  let pos = { ...scene.start };

  const push = (samples, extra = {}) => {
    if (!samples || samples.length === 0) return;
    for (let i = 1; i < samples.length; i++) {
      const q = samples[i];
      const dt = q.t - samples[i - 1].t;
      if (!(dt > 0)) continue;
      points.push({ ...q, ...extra, t: t + q.t, pressed: state.pressed || undefined });
    }
    t = points[points.length - 1].t;
    pos = { x: points[points.length - 1].x, y: points[points.length - 1].y };
  };
  const emit = (type, data = {}) => events.push({ t, type, ...data });
  const idleAt = (ms, idleFn) => {
    if (ms <= 0) return;
    const at = { ...pos };
    const samples = sampleTimed(() => ({ ...at }), ms);
    if (idleFn) {
      for (const q of samples) {
        const o = idleFn(q.t, ms);
        q.x = at.x + o.x;
        q.y = at.y + o.y;
        if (o.scale !== undefined) q.scale = o.scale;
        if (o.rot !== undefined) q.rot = o.rot;
        if (o.opacity !== undefined) q.opacity = o.opacity;
      }
    }
    samples[samples.length - 1].x = at.x;
    samples[samples.length - 1].y = at.y;
    push(samples);
  };
  const idleFor = (rng) => {
    const name = candidate.idle ?? 'still';
    return idles[name] ? idles[name](rng, candidate.idleOpts?.(p) ?? {}) : null;
  };
  const api = {
    push,
    emit,
    idleAt,
    idleFor,
    get t() {
      return t;
    },
    get pos() {
      return pos;
    },
    state,
  };

  if (candidate.intro) {
    const rng = new Rng(`${seed}|${candidate.id}|intro`);
    candidate.intro({ api, rng, p, scene, stage, from: pos });
  }

  scene.waypoints.forEach((wp, i) => {
    const rng = new Rng(`${seed}|${candidate.id}|${i}`);
    const aim = candidate.aim
      ? candidate.aim({ target: wp, rng, p })
      : defaultAim(wp, rng, p.aimSpread ?? 0.12);
    const ctx = {
      from: { ...pos },
      aim,
      target: wp,
      rng,
      p,
      state,
      index: i,
      action: wp.action,
      prev: i > 0 ? center(scene.waypoints[i - 1]) : scene.start,
      next: scene.waypoints[i + 1] ? center(scene.waypoints[i + 1]) : null,
      scene,
      stage,
      api,
    };

    if (candidate.beforeMove) candidate.beforeMove(ctx);
    ctx.from = { ...pos };

    if (wp.action === 'drag') {
      const pressMs = candidate.dragPressMs ? pick(rng, candidate.dragPressMs(p)) : 60;
      emit('press', { target: wp.carries });
      state.pressed = true;
      state.carry = wp.carries;
      idleAt(pressMs);
      ctx.from = { ...pos };
    }

    const segStart = t;
    const out = candidate.move(ctx);
    let samples = Array.isArray(out) ? out : out.samples;
    let segEvents = Array.isArray(out) ? [] : (out.events ?? []);
    if (timing !== 'native' && !candidate.fixedTiming) {
      const T = samples[samples.length - 1].t;
      const W = Math.min(wp.w, wp.h);
      const want =
        timing === 'fixed'
          ? fixedMs
          : fittsTimingMs(dist(ctx.from, samples[samples.length - 1]), W) *
            (candidate.timingScale ?? 1);
      if (T > 0) {
        samples = retime(samples, want);
        segEvents = segEvents.map((e) => ({ ...e, t: (e.t * want) / T }));
      }
    }
    for (const e of segEvents) events.push({ ...e, t: t + e.t });
    const firstIdx = points.length;
    push(samples);
    // First moment the hotspot enters the target rect = hover.
    for (let k = firstIdx; k < points.length; k++) {
      if (inside(points[k], wp)) {
        events.push({ t: points[k].t, type: 'hover', target: wp.id });
        break;
      }
    }
    segments.push({
      index: i,
      target: wp.id,
      from: ctx.from,
      aim: { ...pos },
      t0: segStart,
      t1: t,
    });

    if (candidate.arrive) candidate.arrive(ctx);
    else idleAt(pick(rng, candidate.dwellMs?.(p, ctx) ?? 110), idleFor(rng));

    const action = wp.action;
    if (action === 'click' || action === 'type' || action === 'dblclick') {
      if (candidate.click) candidate.click(ctx);
      else {
        const pressMs = pick(rng, candidate.pressMs?.(p) ?? 90);
        emit('press', { target: wp.id });
        state.pressed = true;
        idleAt(pressMs);
        state.pressed = false;
        emit('release', { target: wp.id });
        emit('click', { target: wp.id, x: pos.x, y: pos.y });
      }
    } else if (action === 'drag') {
      idleAt(candidate.dragReleaseMs ? pick(rng, candidate.dragReleaseMs(p)) : 50);
      state.pressed = false;
      emit('release', { target: wp.carries, drop: wp.id });
      state.carry = null;
    } else if (action === 'scroll') {
      if (candidate.scroll) candidate.scroll(ctx);
      else {
        const ticks = Math.round((wp.scroll ?? 240) / 40);
        for (let k = 0; k < ticks; k++) {
          emit('scroll', { dy: 40, target: wp.id });
          idleAt(80);
        }
      }
    }
    if (action === 'type') {
      const ms = (wp.text?.length ?? 10) * 55;
      emit('type-start', { target: wp.id, text: wp.text, ms });
      if (candidate.whileTyping) candidate.whileTyping(ctx, ms);
      else idleAt(ms);
      emit('type-end', { target: wp.id });
    }
    if (candidate.afterAction) candidate.afterAction(ctx);
    else idleAt(pick(rng, candidate.postMs?.(p) ?? 160), idleFor(rng));
  });

  if (candidate.outro) {
    const rng = new Rng(`${seed}|${candidate.id}|outro`);
    candidate.outro({ api, rng, p, scene });
  } else idleAt(500, idleFor(new Rng(`${seed}|${candidate.id}|outro`)));

  if (candidate.carryLag) applyCarryLag(points, candidate.carryLag(p));
  applyHeading(points, candidate.heading ?? 'fixed', candidate.headingOpts?.(p) ?? {});
  return { id: candidate.id, scene: scene.id, seed, points, events, segments, duration: t };
}

// The dragged object follows the cursor through a spring (lag + settle).
function applyCarryLag(points, { omega = 18, zeta = 0.7 } = {}) {
  let x = points[0].x;
  let y = points[0].y;
  let vx = 0;
  let vy = 0;
  for (let i = 0; i < points.length; i++) {
    const q = points[i];
    const dt = i ? (q.t - points[i - 1].t) / 1000 : 0;
    if (!q.pressed) {
      x = q.x;
      y = q.y;
      vx = vy = 0;
      continue;
    }
    for (let k = 0; k < 4; k++) {
      const s = dt / 4;
      vx += (omega * omega * (q.x - x) - 2 * zeta * omega * vx) * s;
      vy += (omega * omega * (q.y - y) - 2 * zeta * omega * vy) * s;
      x += vx * s;
      y += vy * s;
    }
    q.cx = x;
    q.cy = y;
  }
}

// Heading channel `rot` (radians added to the artwork's rest pose).
//   fixed   - classic OS pointer, never rotates
//   tangent - today's Cua glide: the tip leads along the velocity
//   bank    - tangent plus a lean into turns, rights itself on arrival
//   lean    - tilts with horizontal velocity (cartoon), no full rotation
function applyHeading(points, mode, { gain = 1, maxBank = 0.35 } = {}) {
  const n = points.length;
  let rot = 0;
  let prevHeading = null;
  for (let i = 0; i < n; i++) {
    const a = points[Math.max(0, i - 2)];
    const b = points[Math.min(n - 1, i + 2)];
    const dt = Math.max(1e-3, (b.t - a.t) / 1000);
    const vx = (b.x - a.x) / dt;
    const vy = (b.y - a.y) / dt;
    const speed = Math.hypot(vx, vy);
    const q = points[i];
    let want = 0;
    if (mode === 'tangent' || mode === 'bank') {
      const w = clamp((speed - 40) / 260, 0, 1);
      const h = Math.atan2(vy, vx);
      want = wrapAngle(h - TIP_ANGLE) * w;
      if (mode === 'bank' && prevHeading !== null && speed > 40) {
        const dh = wrapAngle(h - prevHeading) / Math.max(1e-3, (q.t - points[i - 1].t) / 1000);
        want += clamp(dh * 0.08 * gain, -maxBank, maxBank) * w;
      }
      prevHeading = speed > 40 ? h : prevHeading;
    } else if (mode === 'lean') {
      want = clamp(vx * 0.00022 * gain, -0.4, 0.4);
    }
    // Low-pass toward the wanted angle along the shortest arc.
    const dtStep = i ? (q.t - points[i - 1].t) / 1000 : 0;
    const k = 1 - Math.exp(-dtStep * 22);
    rot = i === 0 ? want : rot + wrapAngle(want - rot) * k;
    q.heading = rot + (q.rot ?? 0);
  }
}

// ---------------------------------------------------------------------------
// Playback and analysis helpers
// ---------------------------------------------------------------------------

export function sampleAt(points, t) {
  if (t <= points[0].t) return points[0];
  const last = points[points.length - 1];
  if (t >= last.t) return last;
  let lo = 0;
  let hi = points.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (points[mid].t <= t) lo = mid;
    else hi = mid;
  }
  const a = points[lo];
  const b = points[hi];
  const f = (t - a.t) / (b.t - a.t);
  const mix = (k, d) => (a[k] ?? d) + ((b[k] ?? d) - (a[k] ?? d)) * f;
  return {
    t,
    x: mix('x', 0),
    y: mix('y', 0),
    heading: a.heading + wrapAngle((b.heading ?? 0) - (a.heading ?? 0)) * f,
    opacity: mix('opacity', 1),
    scale: mix('scale', 1),
    sq: mix('sq', 1),
    pressed: a.pressed,
    cx: a.cx !== undefined && b.cx !== undefined ? mix('cx', 0) : a.cx,
    cy: a.cy !== undefined && b.cy !== undefined ? mix('cy', 0) : a.cy,
  };
}

// Uniform 120 Hz kinematics: speed (pt/s), acceleration and jerk magnitudes.
export function kinematics(points, { t0 = 0, t1 = points[points.length - 1].t } = {}) {
  const n = Math.max(3, Math.floor((t1 - t0) / DT_MS));
  const ts = [];
  const xs = [];
  const ys = [];
  for (let i = 0; i <= n; i++) {
    const t = t0 + (i / n) * (t1 - t0);
    const q = sampleAt(points, t);
    ts.push(t);
    xs.push(q.x);
    ys.push(q.y);
  }
  const h = (t1 - t0) / n / 1000;
  const diff = (arr) =>
    arr.map(
      (_, i) =>
        (arr[Math.min(arr.length - 1, i + 1)] - arr[Math.max(0, i - 1)]) /
        ((Math.min(arr.length - 1, i + 1) - Math.max(0, i - 1)) * h)
    );
  const vx = diff(xs);
  const vy = diff(ys);
  const ax = diff(vx);
  const ay = diff(vy);
  const jx = diff(ax);
  const jy = diff(ay);
  return {
    t: ts,
    speed: vx.map((v, i) => Math.hypot(v, vy[i])),
    accel: ax.map((v, i) => Math.hypot(v, ay[i])),
    // Signed tangential acceleration, easier to read than the magnitude.
    along: ax.map((v, i) => {
      const s = Math.hypot(vx[i], vy[i]);
      return s > 1e-6 ? (v * vx[i] + ay[i] * vy[i]) / s : 0;
    }),
    jerk: jx.map((v, i) => Math.hypot(v, jy[i])),
  };
}

export function metrics(result) {
  const { points, segments } = result;
  let path = 0;
  let chord = 0;
  let moveMs = 0;
  let overshoot = 0;
  let submoves = 0;
  let peak = 0;
  let maxAccel = 0;
  for (const s of segments) {
    chord += dist(s.from, s.aim);
    moveMs += s.t1 - s.t0;
    const seg = points.filter((q) => q.t >= s.t0 && q.t <= s.t1);
    for (let i = 1; i < seg.length; i++) path += dist(seg[i - 1], seg[i]);
    // Overshoot: furthest projection past the aim along the approach chord.
    const D = dist(s.from, s.aim);
    if (D > 1) {
      const ux = (s.aim.x - s.from.x) / D;
      const uy = (s.aim.y - s.from.y) / D;
      for (const q of seg)
        overshoot = Math.max(overshoot, (q.x - s.aim.x) * ux + (q.y - s.aim.y) * uy);
    }
    if (s.t1 - s.t0 > DT_MS * 3) {
      const k = kinematics(points, { t0: s.t0, t1: s.t1 });
      const segPeak = Math.max(...k.speed);
      peak = Math.max(peak, segPeak);
      maxAccel = Math.max(maxAccel, ...k.accel);
      // Count speed valleys that dip below 35% of the peak between moving parts.
      let below = false;
      let moved = false;
      for (const v of k.speed) {
        if (v > segPeak * 0.5) {
          if (below && moved) submoves++;
          moved = true;
          below = false;
        } else if (v < segPeak * 0.35 && moved) below = true;
      }
    }
  }
  return {
    durationMs: result.duration,
    moveMs,
    pathPt: path,
    efficiency: path > 0 ? chord / path : 1,
    peakSpeed: peak,
    maxAccel,
    overshootPt: Math.max(0, overshoot),
    extraSubmovements: submoves,
  };
}
