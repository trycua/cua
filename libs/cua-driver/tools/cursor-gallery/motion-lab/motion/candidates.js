// Candidate motion styles for the Cua Driver agent cursor.
//
// Each candidate = path model x velocity profile x micro-behaviours, with its
// parameters documented in `params` ({ v: default, doc }). Range values
// [lo, hi] are sampled per move from the seeded RNG. Ids follow
// research/cursor-motion/candidates.json where a candidate came from there.
//
// Hooks (all optional except `move`), called by plan.js:
//   move(ctx) -> samples | { samples, events }   one movement from ctx.from
//   intro / beforeMove / arrive / click / scroll / whileTyping / afterAction / outro
//   dwellMs / postMs / pressMs / dragPressMs / dragReleaseMs (p) -> ms | [lo, hi]
//   heading: fixed | tangent | bank | lean;  idle: name in behaviors.idles
//   fx: renderer effects (trail, fog, blur, ghost, ripple, shadow, magnet, squashPress)

import {
  DT_MS,
  add,
  arcPath,
  catmullRom,
  clamp,
  cubic,
  dist,
  ease,
  lerp,
  lerpPt,
  makePath,
  perp,
  sub,
  unit,
  wrapAngle,
} from './math.js';
import {
  bell,
  bumpProfile,
  wobbleProfile,
  chain,
  cuaToday,
  durations,
  glide,
  hold,
  hops,
  lognormalProfile,
  lqr,
  manhattan,
  naturalSide,
  paths,
  perfectCursors,
  pinEnds,
  rdp,
  resample,
  sampleTimed,
  sigmaLognormal,
  speedLaw,
  speedShaped,
  springFollow,
  submovements,
  targetWidth,
  teleport,
  trackpadFlick,
  windMouse,
} from './generators.js';
import {
  addTremor,
  dampedOsc,
  idles,
  noise1D,
  oneEuro,
  ouDrift,
  perpendicularNoise,
  quantize,
  springSmooth,
  stretchBySpeed,
} from './behaviors.js';
import { pick, resolveParams } from './plan.js';
import { STAGE, inside } from './scenes.js';

const prm = (v, doc) => ({ v, doc });
const D = (ctx) => dist(ctx.from, ctx.aim);
const side = (ctx) => naturalSide(ctx.from, ctx.aim);
const fittsMs = (ctx, a = 50, b = 150, lo = 180, hi = 1400) =>
  clamp(durations.fitts(D(ctx), targetWidth(ctx), a, b), lo, hi);
const FITTS = {
  a: prm(50, "Fitts' intercept a (ms)"),
  b: prm(150, "Fitts' slope b (ms/bit), MT = a + b log2(D/W + 1)"),
};

// Shared building blocks reused by several candidates.
const fittsMinJerk = (ctx, bow = 0.02, scaleT = 1) =>
  glide(ctx, {
    path: paths.bow(ctx.from, ctx.aim, bow * side(ctx)),
    profile: ease.minJerk,
    durationMs: fittsMs(ctx, ctx.p.a ?? 50, ctx.p.b ?? 150) * scaleT,
  });
const quintGlide = (ctx, scaleT = 1) =>
  glide(ctx, {
    path: paths.bow(ctx.from, ctx.aim, 0.05 * side(ctx)),
    profile: ease.inOutQuint,
    durationMs: clamp(300 + 0.3 * D(ctx), 400, 1000) * scaleT,
  });
const idleOffsets = (fn) => (t, T) => fn(t, T);

export const candidates = [];
const def = (c) =>
  candidates.push({ heading: 'fixed', fx: {}, bestScene: 'route', refs: [], ...c });

// ===========================================================================
// Human-realistic
// ===========================================================================

def({
  id: 'fitts-minjerk',
  name: 'Fitts min-jerk',
  category: 'human-realistic',
  technique: "2% bow / minimum-jerk / Fitts' law duration",
  look: 'The textbook human reach: soft start, symmetric bell, clean stop. Calm and believable; the baseline for every human style.',
  refs: ['fitts1954', 'flashhogan1985'],
  params: { ...FITTS, bow: prm(0.02, 'Perpendicular bow as a fraction of distance') },
  move: (ctx) => fittsMinJerk(ctx, ctx.p.bow),
});

def({
  id: 'minjerk-arc',
  name: 'Min-jerk arc',
  category: 'human-realistic',
  technique: 'Cua cubic Bezier (random small arc) / minimum-jerk / Fitts',
  look: 'Like fitts-minjerk but each reach bows a little to a random side, so consecutive moves never look copy-pasted.',
  refs: ['flashhogan1985'],
  params: {
    ...FITTS,
    arcSize: prm([0.04, 0.1], 'Arc size range (Cua arc_size), side is random'),
    arcFlow: prm([-0.2, 0.2], 'Arc flow range (Cua arc_flow)'),
  },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, {
        arcSize: pick(ctx.rng, ctx.p.arcSize) * ctx.rng.sign(),
        arcFlow: pick(ctx.rng, ctx.p.arcFlow),
      }),
      profile: ease.minJerk,
      durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b),
    }),
});

def({
  id: 'lognormal-stroke',
  name: 'Sigma-lognormal stroke',
  category: 'human-realistic',
  technique: 'Circular arc (angular sweep) / single lognormal velocity / Fitts x1.25',
  look: 'Fast launch, long gentle tail into the target. Reads as confident and organic; the asymmetry is what makes it feel human.',
  refs: ['plamondon1995', 'oreilly2009'],
  params: {
    ...FITTS,
    sigma: prm([0.18, 0.32], 'Lognormal sigma (peak sharpness)'),
    sweepDeg: prm([-12, 12], 'Total heading sweep along the stroke, degrees'),
  },
  move: (ctx) => {
    const sweep = (pick(ctx.rng, ctx.p.sweepDeg) * Math.PI) / 180;
    return glide(ctx, {
      path: paths.arc(ctx.from, ctx.aim, Math.tan(sweep / 4) / 2),
      profile: lognormalProfile(pick(ctx.rng, ctx.p.sigma)),
      durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b) * 1.25,
    });
  },
});

def({
  id: 'lognormal-multi',
  tier: 'reference',
  name: 'Sigma-lognormal multi-stroke',
  category: 'human-realistic',
  technique: 'Vector sum of 2-3 overlapping lognormal strokes',
  look: 'Curvature changes mid-flight and a soft corrective tail. The most "hand-drawn" of the analytic models.',
  refs: ['plamondon1995'],
  params: {
    strokes: prm([2, 3.99], 'Number of strokes (floored)'),
    overlap: prm([0.3, 0.6], 'Where the next stroke starts between the previous peak and end'),
    primary: prm(0.9, 'Fraction of distance covered by the first stroke'),
    sigma: prm([0.2, 0.3], 'Lognormal sigma'),
    dirJitterDeg: prm([3, 15], 'Direction jitter of the primary stroke, degrees'),
  },
  move: (ctx) =>
    sigmaLognormal(ctx, {
      strokes: Math.floor(pick(ctx.rng, ctx.p.strokes)),
      overlap: pick(ctx.rng, ctx.p.overlap),
      primary: ctx.p.primary,
      sigma: pick(ctx.rng, ctx.p.sigma),
      lateral: Math.tan((pick(ctx.rng, ctx.p.dirJitterDeg) * Math.PI) / 180) / 2,
    }),
});

def({
  id: 'meyer-two-component',
  tier: 'reference',
  name: 'Ballistic + corrective (Meyer)',
  category: 'human-realistic',
  technique: 'Primary min-jerk submovement with speed-dependent error, then 0-2 corrections',
  look: 'Lands a touch short, then one or two tiny homing nudges. Instantly reads as a person with a mouse.',
  refs: ['meyer1988'],
  params: {
    ...FITTS,
    primaryFraction: prm([0.9, 0.96], 'Mean distance fraction of the ballistic phase'),
    correctionMs: prm([120, 220], 'Corrective submovement duration'),
    gapMs: prm([0, 40], 'Pause between submovements'),
  },
  move: (ctx) =>
    submovements(ctx, {
      a: ctx.p.a,
      b: ctx.p.b,
      bias: pick(ctx.rng, ctx.p.primaryFraction) - 1,
      alongSd: 0.03,
      latSd: 0.012,
      maxCorrections: 2,
      correctionMs: pick(ctx.rng, ctx.p.correctionMs),
      gapMs: pick(ctx.rng, ctx.p.gapMs),
    }),
});

def({
  id: 'overshoot-correct',
  tier: 'reference',
  name: 'Overshoot and settle back',
  category: 'human-realistic',
  technique: 'Min-jerk throw past the target / pause / min-jerk correction back',
  look: 'Long throws sail a few percent past and come back. Very human, slightly sloppy; best kept rare.',
  refs: ['ghostcursor'],
  params: {
    ...FITTS,
    prob: prm(
      0.6,
      'Probability of overshooting a long move (research: 0.15-0.35; raised for review)'
    ),
    minDistance: prm(300, 'Only overshoot moves longer than this (pt)'),
    frac: prm([0.03, 0.08], 'Overshoot as a fraction of distance'),
    lateralPx: prm([0, 6], 'Sideways miss at the overshoot point'),
    pauseMs: prm([30, 90], 'Pause at the overshoot point'),
    correctionMs: prm([120, 200], 'Correction duration'),
  },
  move: (ctx) => {
    if (D(ctx) < ctx.p.minDistance || ctx.rng.next() > ctx.p.prob) return fittsMinJerk(ctx);
    const u = unit(ctx.from, ctx.aim);
    const n = perp(u);
    const over = D(ctx) * pick(ctx.rng, ctx.p.frac);
    const lat = pick(ctx.rng, ctx.p.lateralPx) * ctx.rng.sign();
    const land = { x: ctx.aim.x + u.x * over + n.x * lat, y: ctx.aim.y + u.y * over + n.y * lat };
    return chain([
      fittsMinJerk({ ...ctx, aim: land }),
      hold(land, pick(ctx.rng, ctx.p.pauseMs)),
      glide(
        { from: land, aim: ctx.aim },
        { profile: ease.minJerk, durationMs: pick(ctx.rng, ctx.p.correctionMs) }
      ),
    ]);
  },
});

def({
  id: 'windmouse-classic',
  tier: 'reference',
  name: 'WindMouse',
  category: 'human-realistic',
  technique: 'Gravity + wind force simulation, step clamp, damped near target',
  look: 'Organic wander with visible jitter; the classic bot-evasion look. Jagged at 120 Hz without smoothing.',
  refs: ['windmouse'],
  params: {
    gravity: prm(9, 'G0: pull toward the target'),
    wind: prm(3, 'W0: wind (random wander) magnitude'),
    maxStep: prm(15, 'M0: max step per tick (pt)'),
    damp: prm(12, 'D0: distance where wind stops and steps shrink'),
    stepMs: prm([5, 10], 'Milliseconds per simulation tick'),
  },
  move: (ctx) =>
    windMouse(ctx, {
      gravity: ctx.p.gravity,
      wind: ctx.p.wind,
      maxStep: ctx.p.maxStep,
      damp: ctx.p.damp,
      stepMs: pick(ctx.rng, ctx.p.stepMs),
    }),
});

def({
  id: 'windmouse-smoothed',
  name: 'WindMouse smoothed',
  category: 'human-realistic',
  technique: 'WindMouse path -> RDP simplify -> centripetal Catmull-Rom -> min-jerk retime',
  look: "Keeps WindMouse's organic, slightly wandering route but glides smoothly. Good human look without the jitter.",
  refs: ['windmouse'],
  params: {
    ...FITTS,
    wind: prm(2, 'W0 wind'),
    maxStep: prm(12, 'M0 max step'),
    rdpEps: prm(1.5, 'RDP simplification epsilon (pt)'),
  },
  move: (ctx) => {
    const raw = windMouse(ctx, {
      gravity: 9,
      wind: ctx.p.wind,
      maxStep: ctx.p.maxStep,
      damp: 12,
      stepMs: 8,
    });
    const pts = rdp(raw, ctx.p.rdpEps);
    const path =
      pts.length > 2 ? makePath(catmullRom(pts), 128 * pts.length) : paths.line(ctx.from, ctx.aim);
    return glide(ctx, { path, profile: ease.minJerk, durationMs: fittsMs(ctx, 80, 150) });
  },
});

def({
  id: 'sigmadrift',
  tier: 'reference',
  name: 'SigmaDrift biomech',
  category: 'human-realistic',
  technique: 'Lognormal primary (~93%) + corrections + OU lateral drift + speed-suppressed tremor',
  look: 'The richest human model: subtle drift, occasional overshoot, micro-corrections, alive at rest.',
  refs: ['sigmadrift', 'harriswolpert1998'],
  params: {
    overshootProb: prm(0.2, 'Chance the primary stroke overshoots (~104%)'),
    ouTheta: prm([4, 8], 'OU mean reversion'),
    ouSigma: prm([1, 3], 'OU noise (pt)'),
    tremorAmp: prm([0.2, 0.6], 'Tremor amplitude (pt)'),
  },
  idle: 'tremor',
  move: (ctx) => {
    const over = ctx.rng.next() < ctx.p.overshootProb;
    const s = sigmaLognormal(ctx, {
      strokes: over ? 2 : ctx.rng.next() < 0.5 ? 2 : 3,
      primary: over ? 1.045 : 0.93,
      sigma: 0.26,
    });
    ouDrift(s, ctx.rng, {
      theta: pick(ctx.rng, ctx.p.ouTheta),
      sigma: pick(ctx.rng, ctx.p.ouSigma),
    });
    return addTremor(s, ctx.rng, { amp: pick(ctx.rng, ctx.p.tremorAmp) });
  },
});

def({
  id: 'ghost-bezier',
  tier: 'reference',
  name: 'Ghost-cursor Bezier',
  category: 'human-realistic',
  technique: 'Random same-side cubic Bezier, natural (non arc-length) timing, overshoot when far',
  look: 'Lands off-centre inside targets, curvy approach, overshoot on long throws. Familiar from Puppeteer bots.',
  refs: ['ghostcursor'],
  params: {
    overshootThreshold: prm(500, 'Overshoot when the move is longer than this (pt)'),
    overshootRadius: prm(120, 'Overshoot point radius around the target (pt)'),
    padding: prm([0.2, 0.8], 'Landing point range inside the target box'),
  },
  aim: ({ target, rng, p }) => ({
    x: target.x + target.w * pick(rng, p.padding),
    y: target.y + target.h * pick(rng, p.padding),
  }),
  move: (ctx) => {
    const bez = (a, b, rng) => {
      const d = dist(a, b);
      const spread = clamp(d, 2, 200);
      const n = perp(unit(a, b));
      const s = rng.sign();
      const c1 = add(lerpPt(a, b, rng.range(0.1, 0.5)), {
        x: n.x * s * rng.range(0, spread),
        y: n.y * s * rng.range(0, spread),
      });
      const c2 = add(lerpPt(a, b, rng.range(0.5, 0.9)), {
        x: n.x * s * rng.range(0, spread),
        y: n.y * s * rng.range(0, spread),
      });
      const f = cubic(a, c1, c2, b);
      const ms = clamp(durations.fitts(d, targetWidth(ctx), 120, 110), 160, 1200);
      return pinEnds(
        sampleTimed((tau) => f(ease.inOutSine(tau)), ms),
        a,
        b
      );
    };
    if (D(ctx) > ctx.p.overshootThreshold) {
      const ang = ctx.rng.range(0, Math.PI * 2);
      const r = ctx.rng.range(0.3, 1) * ctx.p.overshootRadius;
      const over = { x: ctx.aim.x + Math.cos(ang) * r, y: ctx.aim.y + Math.sin(ang) * r };
      return chain([bez(ctx.from, over, ctx.rng), bez(over, ctx.aim, ctx.rng)]);
    }
    return bez(ctx.from, ctx.aim, ctx.rng);
  },
});

def({
  id: 'natural-motion-flow',
  tier: 'reference',
  name: 'NaturalMouseMotion flow',
  category: 'human-realistic',
  technique: 'Sinusoidal deviation + noise / piecewise "flow" speed buckets / small overshoots',
  look: 'Irregular rhythm: some moves speed up, some stall midway, some jitter. Human but a bit chaotic.',
  refs: ['naturalmousemotion'],
  params: {
    ...FITTS,
    slopeDivider: prm(10, 'Max deviation = D / slopeDivider'),
    noisePx: prm([0.5, 2], 'Noise amplitude (pt)'),
    overshoots: prm([0, 2.99], 'Number of small overshoots (floored)'),
  },
  move: (ctx) => {
    const { rng, p } = ctx;
    const flows = {
      constant: [1, 1, 1, 1, 1, 1, 1, 1],
      accelerating: [0.4, 0.6, 0.8, 1, 1.2, 1.4, 1.6, 1.8],
      decelerating: [1.8, 1.6, 1.4, 1.2, 1, 0.8, 0.6, 0.4],
      jaggy: [1, 0.4, 1.4, 0.6, 1.2, 0.5, 1.3, 0.8],
      stopping: [1.2, 1.2, 1, 0.3, 0.08, 0.3, 1, 1],
    };
    const names = Object.keys(flows);
    const flow = flows[names[Math.floor(rng.next() * names.length)]];
    const leg = (a, b, rngLeg, ms) => {
      const d = dist(a, b);
      const n = perp(unit(a, b));
      const dev = (d / p.slopeDivider) * rngLeg.range(-0.6, 0.6);
      const nz = noise1D(rngLeg);
      const amp = pick(rngLeg, p.noisePx);
      const path = makePath((u) => {
        const q = lerpPt(a, b, u);
        const k = dev * Math.sin(Math.PI * u) + amp * nz(u * 9) * Math.sin(Math.PI * u);
        return { x: q.x + n.x * k, y: q.y + n.y * k };
      });
      const shape = (s) =>
        flow[Math.min(7, Math.floor(s * 8))] * (0.12 + 0.88 * Math.sin(Math.PI * s) ** 0.6);
      return speedShaped({ from: a, aim: b }, { path, shape, durationMs: ms });
    };
    const count = Math.floor(pick(rng, p.overshoots));
    const parts = [];
    let pos = ctx.from;
    for (let i = 0; i < count && D(ctx) > 200; i++) {
      const r = 26 / (i + 1);
      const ang = rng.range(0, Math.PI * 2);
      const o = { x: ctx.aim.x + Math.cos(ang) * r, y: ctx.aim.y + Math.sin(ang) * r };
      parts.push(leg(pos, o, rng, i === 0 ? fittsMs(ctx, p.a, p.b) : 140));
      pos = o;
    }
    parts.push(leg(pos, ctx.aim, rng, parts.length ? 140 : fittsMs(ctx, p.a, p.b)));
    return chain(parts);
  },
});

def({
  id: 'noisy-minjerk',
  tier: 'reference',
  name: 'Signal-dependent noise reach',
  category: 'human-realistic',
  technique:
    'Min-jerk reach; endpoint error grows with speed (Harris-Wolpert); auto-correct if outside',
  look: 'Fast moves land sloppier and need a correction; slow ones land clean. Honest speed/accuracy trade-off.',
  refs: ['harriswolpert1998'],
  params: {
    ...FITTS,
    k: prm([0.03, 0.06], 'Endpoint SD as a fraction of distance at Fitts speed'),
    hurry: prm(0.75, 'Duration multiplier (< 1 = faster, noisier)'),
  },
  move: (ctx) => {
    const T = fittsMs(ctx, ctx.p.a, ctx.p.b) * ctx.p.hurry;
    const sd = pick(ctx.rng, ctx.p.k) * D(ctx) * (1 / ctx.p.hurry) * 0.5;
    const land = {
      x: ctx.aim.x + ctx.rng.clippedNormal(0, sd),
      y: ctx.aim.y + ctx.rng.clippedNormal(0, sd),
    };
    const first = glide(
      { from: ctx.from, aim: land },
      { path: paths.bow(ctx.from, land, 0.03 * side(ctx)), profile: ease.minJerk, durationMs: T }
    );
    if (inside(land, ctx.target, -2)) return first;
    return chain([
      first,
      hold(land, 40),
      glide({ from: land, aim: ctx.aim }, { profile: ease.minJerk, durationMs: 150 }),
    ]);
  },
});

def({
  id: 'wrist-pivot-arc',
  name: 'Wrist-pivot arc',
  category: 'human-realistic',
  technique: 'Polar interpolation around a virtual wrist pivot below the hand / min-jerk',
  look: 'Horizontal moves bow consistently, like a forearm rotating on the desk. Subtle and very natural.',
  refs: ['handedness-curvature'],
  params: {
    ...FITTS,
    handedness: prm(1, '1 = right hand, -1 = left'),
    pivotOffset: prm([250, 500], 'Pivot distance below the start (pt)'),
    blend: prm([0.5, 0.8], 'Blend of pivot arc vs straight line'),
  },
  move: (ctx) => {
    const r0 = pick(ctx.rng, ctx.p.pivotOffset);
    const pivot = { x: ctx.from.x + ctx.p.handedness * r0 * 0.35, y: ctx.from.y + r0 };
    const pa = {
      r: dist(pivot, ctx.from),
      th: Math.atan2(ctx.from.y - pivot.y, ctx.from.x - pivot.x),
    };
    const pb = {
      r: dist(pivot, ctx.aim),
      th: Math.atan2(ctx.aim.y - pivot.y, ctx.aim.x - pivot.x),
    };
    const dth = wrapAngle(pb.th - pa.th);
    const k = pick(ctx.rng, ctx.p.blend);
    const path = makePath((u) => {
      const r = lerp(pa.r, pb.r, u);
      const th = pa.th + dth * u;
      const polar = { x: pivot.x + r * Math.cos(th), y: pivot.y + r * Math.sin(th) };
      return lerpPt(lerpPt(ctx.from, ctx.aim, u), polar, k);
    });
    return glide(ctx, { path, profile: ease.minJerk, durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b) });
  },
});

def({
  id: 'two-thirds-power',
  name: 'Curvature-modulated speed',
  category: 'human-realistic',
  technique: 'Curved Bezier / v ~ curvature^(-1/3) (2/3 power law) x bell / Fitts x1.1',
  look: 'Slows in the bend and speeds on the straights, the way hands draw curves. Good companion to curved paths.',
  refs: ['lacquaniti1983'],
  params: {
    ...FITTS,
    beta: prm(0.33, 'Power-law exponent'),
    arcSize: prm(0.3, 'Arc size of the curved path'),
    arcFlow: prm(0.4, 'Arc flow (apex position)'),
  },
  move: (ctx) => {
    const path = paths.cua(ctx.from, ctx.aim, {
      arcSize: ctx.p.arcSize * side(ctx),
      arcFlow: ctx.p.arcFlow,
    });
    const L = Math.max(1, path.length);
    const curv = (s) => {
      const h = 0.01;
      const a = path.atFraction(Math.max(0, s - h));
      const b = path.atFraction(s);
      const c = path.atFraction(Math.min(1, s + h));
      const t1 = Math.atan2(b.y - a.y, b.x - a.x);
      const t2 = Math.atan2(c.y - b.y, c.x - b.x);
      return Math.abs(wrapAngle(t2 - t1)) / Math.max(1e-6, 2 * h * L);
    };
    const shape = (s) => (curv(s) + 1 / 3000) ** -ctx.p.beta * bell(s);
    return speedShaped(ctx, {
      path,
      shape,
      durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b) * 1.1,
      n: 300,
    });
  },
});

def({
  id: 'hesitant-reach',
  tier: 'reference',
  name: 'Hesitant reach',
  category: 'human-realistic',
  technique: 'Two chained min-jerk segments with a mid-path pause and tiny drift',
  look: 'Starts, stops to "decide", then commits. Communicates uncertainty; charming in small doses.',
  refs: [],
  params: {
    ...FITTS,
    pauseAt: prm([0.35, 0.65], 'Pause position as a fraction of the path'),
    pauseMs: prm([120, 350], 'Pause length'),
  },
  move: (ctx) => {
    const path = paths.bow(ctx.from, ctx.aim, 0.05 * side(ctx));
    const f = pick(ctx.rng, ctx.p.pauseAt);
    const mid = path.atFraction(f);
    const T = fittsMs(ctx, ctx.p.a, ctx.p.b);
    const pauseMs = pick(ctx.rng, ctx.p.pauseMs);
    const wig = idles.wiggle(ctx.rng, { amp: 1.5, hz: 1.5 });
    const first = glide(
      { from: ctx.from, aim: mid },
      { path: makePath((u) => path.atFraction(u * f)), profile: ease.minJerk, durationMs: T * 0.6 }
    );
    const samples = chain([
      first,
      hold(mid, pauseMs, (tau) => wig(tau * pauseMs, pauseMs)),
      glide(
        { from: mid, aim: ctx.aim },
        {
          path: makePath((u) => path.atFraction(f + u * (1 - f))),
          profile: ease.minJerk,
          durationMs: T * 0.6,
        }
      ),
    ]);
    return { samples, events: [{ t: first[first.length - 1].t, type: 'think', ms: pauseMs }] };
  },
});

def({
  id: 'tremor-dwell',
  tier: 'reference',
  name: 'Steady-hand dwell',
  category: 'human-realistic',
  technique: 'Fitts min-jerk + 8-12 Hz tremor and slow drift while hovering before the click',
  look: 'Never perfectly frozen on a target; sub-point life that shows on Retina recordings, invisible otherwise.',
  refs: ['tremor-physiology'],
  params: {
    ...FITTS,
    tremorAmp: prm([0.2, 0.5], 'Tremor amplitude (pt)'),
    dwell: prm([80, 250], 'Hover before click (ms)'),
  },
  idle: 'tremor',
  idleOpts: (p) => ({ amp: Array.isArray(p.tremorAmp) ? p.tremorAmp[1] : p.tremorAmp }),
  dwellMs: (p) => p.dwell,
  move: (ctx) => fittsMinJerk(ctx),
});

def({
  id: 'trackpad-strokes',
  tier: 'reference',
  name: 'Trackpad strokes',
  category: 'human-realistic',
  technique: 'Long moves split into 2-3 lognormal strokes with finger-lift (clutch) gaps',
  look: 'Swipe, lift, swipe, settle. Recognisably a laptop trackpad user.',
  refs: ['casiez2008'],
  params: {
    strokes: prm([2, 3.99], 'Number of strokes (floored)'),
    gapMs: prm([50, 140], 'Finger-lift gap'),
    minDistance: prm(380, 'Single stroke below this distance (pt)'),
  },
  move: (ctx) => {
    const n = D(ctx) < ctx.p.minDistance ? 1 : Math.floor(pick(ctx.rng, ctx.p.strokes));
    const fr = n === 3 ? [0.6, 0.3, 0.1] : n === 2 ? [0.75, 0.25] : [1];
    const parts = [];
    let pos = ctx.from;
    let acc = 0;
    fr.forEach((f, i) => {
      acc += f;
      const last = i === fr.length - 1;
      const n2 = perp(unit(ctx.from, ctx.aim));
      const lat = last ? 0 : D(ctx) * ctx.rng.clippedNormal(0, 0.025);
      const to = last
        ? ctx.aim
        : add(lerpPt(ctx.from, ctx.aim, acc), { x: n2.x * lat, y: n2.y * lat });
      if (i > 0) parts.push(hold(pos, pick(ctx.rng, ctx.p.gapMs)));
      const d = dist(pos, to);
      parts.push(
        glide(
          { from: pos, aim: to },
          {
            path: paths.bow(pos, to, 0.03 * side(ctx)),
            profile: lognormalProfile(0.24),
            durationMs: clamp(140 + 0.45 * d, 160, 700),
          }
        )
      );
      pos = to;
    });
    return chain(parts);
  },
});

def({
  id: 'trackpad-flick',
  tier: 'reference',
  name: 'Trackpad flick + adjust',
  category: 'human-realistic',
  technique: 'Inertial throw with exponential decay to ~90%, pause, slow sine-eased adjustment',
  look: 'A quick flick that coasts, then a deliberate little finger nudge onto the target.',
  refs: ['casiez2008'],
  params: {
    reach: prm(0.9, 'Mean fraction reached by the flick'),
    tauMs: prm(110, 'Inertia decay time constant'),
    adjustMs: prm(260, 'Adjustment duration'),
    pauseMs: prm(70, 'Pause before adjusting'),
  },
  move: (ctx) =>
    trackpadFlick(ctx, {
      reach: ctx.p.reach,
      tauMs: ctx.p.tauMs,
      adjustMs: ctx.p.adjustMs,
      pauseMs: ctx.p.pauseMs,
    }),
});

def({
  id: 'mouse-lift-reposition',
  tier: 'reference',
  name: 'Long-throw clutch',
  category: 'human-realistic',
  technique: 'Very long moves: min-jerk throw, clutch stop (mouse lifted), second throw',
  look: 'Only shows on huge distances: the pause where a real user runs out of mousepad.',
  refs: [],
  params: {
    ...FITTS,
    minDistance: prm(900, 'Clutch only beyond this distance (research: 1400 on multi-display)'),
    firstFraction: prm([0.55, 0.7], 'First throw fraction'),
    stopMs: prm([80, 200], 'Clutch stop'),
  },
  bestScene: 'long',
  move: (ctx) => {
    if (D(ctx) < ctx.p.minDistance) return fittsMinJerk(ctx);
    const mid = lerpPt(ctx.from, ctx.aim, pick(ctx.rng, ctx.p.firstFraction));
    return chain([
      fittsMinJerk({ ...ctx, aim: mid }, 0.03, 0.85),
      hold(mid, pick(ctx.rng, ctx.p.stopMs)),
      fittsMinJerk({ ...ctx, from: mid }, 0.03, 0.85),
    ]);
  },
});

def({
  id: 'second-order-lag',
  tier: 'reference',
  name: '2nd-order lag (2OL)',
  category: 'human-realistic',
  technique: 'Mass-spring-damper chasing a step target (control-theory pointing model)',
  look: 'Sharp start, exponential-feeling approach, may brush past by a pixel. Mechanical but plausible.',
  refs: ['muller2017'],
  params: {
    omegaN: prm([10, 16], 'Natural frequency (rad/s)'),
    zeta: prm([0.8, 1.0], 'Damping ratio'),
  },
  move: (ctx) =>
    springFollow(ctx, {
      freq: pick(ctx.rng, ctx.p.omegaN) / (2 * Math.PI),
      zeta: pick(ctx.rng, ctx.p.zeta),
      settlePx: 0.4,
    }),
});

def({
  id: 'perlin-wander',
  tier: 'reference',
  name: 'Perlin wander',
  category: 'human-realistic',
  technique: 'Line + low-frequency gradient noise perpendicular to travel, sin taper / min-jerk',
  look: 'A gently meandering line; organic without any overshoot or correction.',
  refs: [],
  params: {
    ...FITTS,
    ampFrac: prm([0.02, 0.06], 'Noise amplitude as a fraction of distance'),
    freq: prm([1.5, 3], 'Noise cycles along the path'),
  },
  move: (ctx) =>
    perpendicularNoise(
      glide(ctx, { profile: ease.minJerk, durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b) }),
      ctx.rng,
      { ampFrac: pick(ctx.rng, ctx.p.ampFrac), freq: pick(ctx.rng, ctx.p.freq) }
    ),
});

def({
  id: 'lqr-optimal',
  name: 'LQR optimal pointing',
  category: 'human-realistic',
  technique: 'Finite-horizon LQR on a triple integrator (jerk control), distance + effort cost',
  look: 'Early velocity peak and a long, smooth approach; closest analytic fit to real mouse data.',
  refs: ['fischer2022'],
  params: {
    ...FITTS,
    wDist: prm(0.1, 'Running cost on distance to target'),
    wVel: prm(0.01, 'Running cost on speed'),
    r: prm(1e-6, 'Jerk effort cost'),
    horizon: prm(1.15, 'Horizon as a multiple of the Fitts time'),
  },
  move: (ctx) =>
    lqr(ctx, {
      durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b) * ctx.p.horizon,
      wDist: ctx.p.wDist,
      wVel: ctx.p.wVel,
      r: ctx.p.r,
    }),
});

def({
  id: 'usb-polled',
  tier: 'reference',
  name: 'USB-polled mouse',
  category: 'human-realistic',
  technique: 'Fitts min-jerk sampled-and-held at 125 Hz, snapped to whole points, light tremor',
  look: 'Microscopically steppy like a real recorded mouse. Mostly matters for frame-accurate recordings.',
  refs: [],
  params: { ...FITTS, hz: prm(125, 'Polling rate (Hz)'), px: prm(1, 'Position quantum (pt)') },
  move: (ctx) =>
    quantize(addTremor(fittsMinJerk(ctx, 0.03), ctx.rng, { amp: 0.4 }), {
      hz: ctx.p.hz,
      px: ctx.p.px,
    }),
});

// ===========================================================================
// Cinematic / demo
// ===========================================================================

def({
  id: 'dubins-glide',
  name: 'Dubins glide (Cua today)',
  category: 'cinematic-demo',
  technique:
    'Dubins arc-straight-arc R=80 / speed-based smootherstep envelope / arrival spring (macOS constants)',
  look: "Today's Cua Driver cursor. Heading-continuous swoops, sometimes a loop when the target is behind the heading.",
  refs: ['cua-driver'],
  heading: 'tangent',
  params: {
    turnRadius: prm(80, 'Minimum turn radius (pt)'),
    peakSpeed: prm(900, 'Peak speed (pt/s)'),
    minStart: prm(300, 'Start floor speed (pt/s)'),
    minEnd: prm(200, 'End floor speed (pt/s)'),
    overshoot: prm(0.8, 'Arrival spring impulse factor'),
  },
  dwellMs: () => 0,
  pressMs: () => 120,
  postMs: () => 80,
  move: (ctx) =>
    cuaToday(ctx, {
      turnRadius: ctx.p.turnRadius,
      peakSpeed: ctx.p.peakSpeed,
      minStart: ctx.p.minStart,
      minEnd: ctx.p.minEnd,
      overshoot: ctx.p.overshoot,
    }),
});

def({
  id: 'studio-spring',
  name: 'Screen Studio spring',
  category: 'cinematic-demo',
  technique:
    'Quick straight raw move smoothed by a react-spring style spring (tension 170, friction 26)',
  look: 'The polished screen-recording look: buttery, slightly lagging, lands softly with no visible overshoot.',
  refs: ['screenstudio', 'react-spring'],
  heading: 'tangent',
  params: {
    tension: prm(170, 'Spring tension'),
    friction: prm(26, 'Spring friction'),
    mass: prm(1, 'Spring mass'),
  },
  move: (ctx) => {
    const raw = glide(ctx, { profile: ease.outCubic, durationMs: fittsMs(ctx) * 0.55 });
    const w = Math.sqrt(ctx.p.tension / ctx.p.mass);
    return springSmooth(raw, {
      freq: w / (2 * Math.PI),
      zeta: ctx.p.friction / (2 * Math.sqrt(ctx.p.tension * ctx.p.mass)),
    });
  },
});

def({
  id: 'keynote-swoop',
  name: 'Keynote swoop',
  category: 'cinematic-demo',
  technique: 'Cua cubic Bezier arc 0.25-0.35, flow +0.2 / easeInOutCubic / 350 + 0.35 D ms',
  look: 'Wide confident arc, the product-video move. Reads great on a launch video; too theatrical for every click.',
  refs: [],
  heading: 'tangent',
  params: { arcSize: prm([0.25, 0.35], 'Arc size'), arcFlow: prm(0.2, 'Arc flow') },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, {
        arcSize: pick(ctx.rng, ctx.p.arcSize) * side(ctx),
        arcFlow: ctx.p.arcFlow,
      }),
      profile: ease.inOutCubic,
      durationMs: clamp(350 + 0.35 * D(ctx), 450, 1100),
    }),
});

def({
  id: 'quint-glide',
  name: 'Quint glide',
  category: 'cinematic-demo',
  technique: '5% bow / easeInOutQuint / 300 + 0.3 D ms',
  look: 'Very soft start and stop with a fast middle. Clean, minimal, Apple-like.',
  refs: ['penner'],
  heading: 'tangent',
  params: {},
  move: (ctx) => quintGlide(ctx),
});

def({
  id: 'spline-weave',
  name: 'Waypoint weave',
  category: 'cinematic-demo',
  technique: 'Centripetal Catmull-Rom through previous / current / next targets, short dwell',
  look: 'Multi-step plans flow as one continuous curve; each approach already leans toward the next target.',
  refs: ['catmullrom'],
  heading: 'tangent',
  bestScene: 'sweep',
  params: {
    dwell: prm([20, 80], 'Dwell at each waypoint (ms)'),
    durationScale: prm(1, 'Duration multiplier'),
  },
  dwellMs: (p) => p.dwell,
  pressMs: () => 60,
  postMs: () => 40,
  move: (ctx) => {
    const prev = ctx.state.prevFrom ?? add(ctx.from, sub(ctx.from, ctx.aim));
    const next = ctx.next ?? add(ctx.aim, sub(ctx.aim, ctx.from));
    const cr = catmullRom([prev, ctx.from, ctx.aim, next]);
    ctx.state.prevFrom = ctx.from;
    const path = makePath((u) => cr(1 / 3 + u / 3));
    return glide(ctx, {
      path,
      profile: ease.inOutSine,
      durationMs: clamp(220 + 0.32 * path.length, 280, 900) * ctx.p.durationScale,
    });
  },
});

def({
  id: 'perfect-interp',
  name: 'Buffered stream interpolation',
  category: 'cinematic-demo',
  technique: 'Sparse jittery position stream (every 80 ms) replayed through a Catmull-Rom spline',
  look: 'Smooth but slightly rubbery: what multiplayer cursors (perfect-cursors) look like.',
  refs: ['perfectcursors'],
  heading: 'tangent',
  params: {
    bufferMs: prm(80, 'Interval between received points (ms)'),
    jitter: prm(6, 'Noise on received points (pt)'),
  },
  move: (ctx) => perfectCursors(ctx, { intervalMs: ctx.p.bufferMs, jitter: ctx.p.jitter }),
});

def({
  id: 'expo-landing',
  name: 'Fast cruise, slow landing',
  category: 'cinematic-demo',
  technique: 'Arc 0.1 / easeOutExpo / 400 + 0.25 D ms',
  look: 'Snaps away instantly and floats into the target. Pairs with a camera zoom on the click.',
  refs: ['penner'],
  heading: 'tangent',
  params: { arcSize: prm(0.1, 'Arc size') },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: ctx.p.arcSize * side(ctx) }),
      profile: ease.outExpo,
      durationMs: clamp(400 + 0.25 * D(ctx), 500, 1100),
    }),
});

def({
  id: 'heading-bank',
  name: 'Banking glide',
  category: 'cinematic-demo',
  technique: 'Arc 0.22 / easeInOutCubic / rotate to heading + bank by turn rate',
  look: 'The arrow leans into the curve like a plane and rights itself on arrival. Very "alive".',
  refs: [],
  heading: 'bank',
  headingOpts: (p) => ({ gain: p.bankGain, maxBank: (p.maxBankDeg * Math.PI) / 180 }),
  params: {
    maxBankDeg: prm(20, 'Max bank angle (deg)'),
    bankGain: prm(1.4, 'Bank gain on turn rate'),
  },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.22 * side(ctx), arcFlow: 0.1 }),
      profile: ease.inOutCubic,
      durationMs: durations.sqrt(D(ctx), 26, 300, 1000),
    }),
});

def({
  id: 'comet-trail',
  name: 'Comet trail',
  category: 'cinematic-demo',
  technique: 'Arc 0.18 / easeInOutCubic + fading trail (length grows with speed)',
  look: 'Viewers can follow fast moves; great for recordings, distracting live.',
  refs: [],
  heading: 'tangent',
  params: { trailMs: prm(240, 'Trail length (ms)'), trailOpacity: prm(0.45, 'Trail opacity') },
  fx: { trail: true },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.18 * side(ctx) }),
      profile: ease.inOutCubic,
      durationMs: durations.sqrt(D(ctx), 24, 280, 900),
    }),
});

def({
  id: 'motion-blur-streak',
  name: 'Motion blur streak',
  category: 'cinematic-demo',
  technique: 'Quint glide + velocity stretch (max 1.12) + directional blur at speed only',
  look: 'Crisp at rest, a short streak at peak speed. Feels fast without trails.',
  refs: [],
  heading: 'tangent',
  params: { stretchMax: prm(1.12, 'Max stretch along velocity') },
  fx: { blur: true },
  move: (ctx) => stretchBySpeed(quintGlide(ctx), { max: ctx.p.stretchMax, gain: 0.00012 }),
});

def({
  id: 'orbit-landing',
  name: 'Orbit landing',
  category: 'cinematic-demo',
  technique:
    'Approach Bezier into a shrinking spiral (0.25-0.5 turns) around the target / easeInOutSine',
  look: 'Theatrical: circles in and lands. Reserve for the one key click in a demo.',
  refs: [],
  heading: 'tangent',
  params: {
    turns: prm([0.25, 0.5], 'Spiral turns'),
    radius: prm([40, 90], 'Spiral start radius (pt)'),
  },
  move: (ctx) => {
    const r0 = Math.min(pick(ctx.rng, ctx.p.radius), D(ctx) * 0.45);
    const turns = pick(ctx.rng, ctx.p.turns);
    const dir = side(ctx);
    const th0 = Math.atan2(ctx.from.y - ctx.aim.y, ctx.from.x - ctx.aim.x) - dir * Math.PI * 0.5;
    const spiral = (q) => {
      const th = th0 + dir * q * turns * 2 * Math.PI;
      const r = r0 * (1 - q) ** 1.3;
      return { x: ctx.aim.x + r * Math.cos(th), y: ctx.aim.y + r * Math.sin(th) };
    };
    const E = spiral(0);
    const tE = unit(E, spiral(0.02));
    const approach = cubic(
      ctx.from,
      lerpPt(ctx.from, E, 0.4),
      { x: E.x - tE.x * r0, y: E.y - tE.y * r0 },
      E
    );
    const a = makePath(approach);
    const b = makePath(spiral);
    const L = a.length + b.length;
    const path = makePath(
      (u) =>
        u * L <= a.length
          ? a.atFraction((u * L) / Math.max(1, a.length))
          : b.atFraction((u * L - a.length) / Math.max(1, b.length)),
      512
    );
    return glide(ctx, {
      path,
      profile: ease.inOutSine,
      durationMs: durations.sqrt(L, 30, 500, 1400),
    });
  },
});

def({
  id: 'lift-hop',
  name: 'Lift hop',
  category: 'cinematic-demo',
  technique: 'Screen-space parabola / easeInOutSine / scale up + drop shadow, land squash',
  look: 'Pseudo-3D: the cursor lifts off the page, travels, lands with a tiny squash.',
  refs: [],
  heading: 'fixed',
  fx: { shadow: true },
  params: {
    lift: prm([1.12, 1.25], 'Peak scale'),
    height: prm([0.08, 0.18], 'Hop height as a fraction of distance'),
    landSquash: prm(0.92, 'Landing scale'),
  },
  move: (ctx) => {
    const lift = pick(ctx.rng, ctx.p.lift);
    const h = Math.max(24, D(ctx) * pick(ctx.rng, ctx.p.height));
    const ms = durations.sqrt(D(ctx), 26, 320, 1000);
    const s = sampleTimed((tau) => {
      const e = ease.inOutSine(tau);
      const q = lerpPt(ctx.from, ctx.aim, e);
      return { x: q.x, y: q.y - 4 * h * e * (1 - e) };
    }, ms);
    s.forEach((q, i) => {
      const tau = i / (s.length - 1);
      q.scale =
        1 +
        (lift - 1) * Math.sin(Math.PI * tau) -
        (1 - ctx.p.landSquash) * Math.exp(-(((tau - 1) / 0.06) ** 2));
    });
    return pinEnds(s, ctx.from, ctx.aim);
  },
  arrive: (ctx) =>
    ctx.api.idleAt(160, (t, T) => ({
      x: 0,
      y: 0,
      scale: 1 - (1 - ctx.p.landSquash) * (1 - ease.outBack(t / T)),
    })),
});

def({
  id: 'teleport-ripple',
  fixedTiming: true,
  name: 'Teleport ripple',
  category: 'cinematic-demo',
  technique: 'Fade out at origin, ripple in at destination (no travel)',
  look: 'For huge jumps or reduced motion. Honest and instant, but you lose the "where did it go" story.',
  refs: [],
  heading: 'fixed',
  fx: { ripple: true },
  params: { fadeMs: prm(120, 'Fade duration'), rippleMs: prm(350, 'Ripple ring duration') },
  move: (ctx) => {
    const s = teleport(ctx, { fadeMs: ctx.p.fadeMs, gapMs: 40 });
    return {
      samples: s,
      events: [
        { t: ctx.p.fadeMs + 40, type: 'ripple', x: ctx.aim.x, y: ctx.aim.y, ms: ctx.p.rippleMs },
      ],
    };
  },
});

def({
  id: 'camera-friendly',
  name: 'Zoom-friendly glide',
  category: 'cinematic-demo',
  technique: 'Single circular arc / min-jerk / duration x1.2, no overshoot',
  look: 'Low-jerk constant-curvature glide that auto-zoom recorders can follow without jitter.',
  refs: [],
  heading: 'tangent',
  params: {
    arc: prm(0.12, 'Arc size (sagitta = arc/2 of chord)'),
    durationScale: prm(1.2, 'Duration multiplier'),
  },
  move: (ctx) =>
    glide(ctx, {
      path: paths.arc(ctx.from, ctx.aim, (ctx.p.arc / 2) * side(ctx)),
      profile: ease.minJerk,
      durationMs: durations.sqrt(D(ctx), 24, 300, 1000) * ctx.p.durationScale,
    }),
});

def({
  id: 'heading-candidates',
  name: 'Heading chooser (Codex-like)',
  category: 'cinematic-demo',
  technique:
    'Score direct / turn-around / brake candidate arcs (length, angle energy, turn, bounds); spring progress',
  look: 'Closest public reference to the Codex Computer Use cursor: momentum-aware arcs on a 1.4 s spring. Every move takes about as long whatever the distance; Cua should beat this with Fitts timing.',
  refs: ['openara'],
  heading: 'tangent',
  fx: { fog: true },
  params: {
    wLength: prm(320, 'Excess-length weight'),
    wEnergy: prm(140, 'Angle-energy weight'),
    wMaxAngle: prm(180, 'Max-angle weight'),
    wTurn: prm(18, 'Total-turn weight'),
    wBounds: prm(45, 'Off-screen penalty per sample'),
    responseMs: prm(1400, 'Spring progress response time (Codex-like 1.4 s)'),
    damping: prm(0.9, 'Spring progress damping'),
  },
  move: (ctx) => {
    const { p } = ctx;
    const d = Math.max(1, D(ctx));
    const u = unit(ctx.from, ctx.aim);
    const n = perp(u);
    const mom = ctx.state.momentum;
    const options = [];
    for (const startDir of mom ? [u, mom] : [u]) {
      for (const arc of [-0.45, -0.3, -0.15, 0, 0.15, 0.3, 0.45]) {
        const c1 = {
          x: ctx.from.x + startDir.x * d * 0.35 + n.x * arc * d * 0.3,
          y: ctx.from.y + startDir.y * d * 0.35 + n.y * arc * d * 0.3,
        };
        const c2 = {
          x: ctx.aim.x - u.x * d * 0.3 + n.x * arc * d * 0.5,
          y: ctx.aim.y - u.y * d * 0.3 + n.y * arc * d * 0.5,
        };
        options.push(cubic(ctx.from, c1, c2, ctx.aim));
      }
    }
    let best = null;
    for (const f of options) {
      let len = 0;
      let energy = 0;
      let maxA = 0;
      let turn = 0;
      let oob = 0;
      let prevP = f(0);
      let prevH = mom ? Math.atan2(mom.y, mom.x) : null;
      for (let i = 1; i <= 40; i++) {
        const q = f(i / 40);
        const h = Math.atan2(q.y - prevP.y, q.x - prevP.x);
        len += dist(prevP, q);
        if (prevH !== null) {
          const da = Math.abs(wrapAngle(h - prevH));
          energy += da * da;
          maxA = Math.max(maxA, da);
          turn += da;
        }
        if (q.x < 0 || q.y < 0 || q.x > STAGE.w || q.y > STAGE.h) oob++;
        prevH = h;
        prevP = q;
      }
      const score =
        ((len - d) / d) * p.wLength +
        energy * p.wEnergy +
        maxA * p.wMaxAngle +
        turn * p.wTurn +
        oob * p.wBounds;
      if (!best || score < best.score) best = { f, score };
    }
    const path = makePath(best.f);
    const end = path.atFraction(0.995);
    ctx.state.momentum = unit(end, ctx.aim);
    // Spring progress 0 -> 1.
    const w = (2 * Math.PI) / (p.responseMs / 1000);
    let x = 0;
    let v = 0;
    const raw = [{ t: 0, x: ctx.from.x, y: ctx.from.y }];
    let t = 0;
    while (t < 3000) {
      for (let k = 0; k < 8; k++) {
        const sdt = DT_MS / 8000;
        v += (w * w * (1 - x) - 2 * p.damping * w * v) * sdt;
        x += v * sdt;
      }
      t += DT_MS;
      const q = path.atFraction(x);
      raw.push({ t, x: q.x, y: q.y });
      if (Math.abs(1 - x) < 0.0015 && Math.abs(v) < 0.02) break;
    }
    raw.push({ t: t + DT_MS, x: ctx.aim.x, y: ctx.aim.y });
    return raw;
  },
});

def({
  id: 'velocity-fog',
  name: 'Velocity fog',
  category: 'cinematic-demo',
  technique: 'Arc 0.12 min-jerk + soft glow offset opposite velocity, grows with speed',
  look: 'A soft comet-like glow without a hard trail; collapses at rest. Subtle polish layer.',
  refs: [],
  heading: 'tangent',
  fx: { fog: true },
  params: {},
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.12 * side(ctx) }),
      profile: ease.minJerk,
      durationMs: durations.sqrt(D(ctx), 24, 280, 950),
    }),
});

// ===========================================================================
// Expressive / personality
// ===========================================================================

def({
  id: 'elastic-arrival',
  name: 'Elastic arrival',
  category: 'expressive-personality',
  technique:
    'Arc 0.12 / min-jerk that hands over to a damped 2.5-cycle wobble (amplitude capped in pt)',
  look: 'Arrives with a couple of visible wobbles around the target. Playful; the cap keeps long moves from flailing.',
  refs: ['penner'],
  heading: 'lean',
  params: {
    ampPx: prm(12, 'First overshoot (pt)'),
    cycles: prm(2.2, 'Wobble cycles'),
    decay: prm(3.2, 'Wobble decay'),
  },
  move: (ctx) => {
    const d = Math.max(1, D(ctx));
    return glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.12 * side(ctx) }),
      profile: wobbleProfile((t) => ease.minJerk(Math.min(1, t / 0.6)), {
        amp: Math.min(0.08, ctx.p.ampPx / d),
        cycles: ctx.p.cycles,
        decay: ctx.p.decay,
        start: 0.45,
      }),
      durationMs: fittsMs(ctx) * 1.7,
    });
  },
});

def({
  id: 'anticipate-follow',
  name: 'Anticipation + follow-through',
  category: 'expressive-personality',
  technique: 'Arc 0.15 / min-jerk with an early pull-back bump and a late overshoot bump',
  look: 'Disney principles: tiny wind-up, launch, sail past, settle. Charismatic; signals intent before moving.',
  refs: ['thomasjohnston1981'],
  heading: 'lean',
  params: {
    antic: prm([0.04, 0.08], 'Pull-back as a fraction of distance'),
    over: prm(0.05, 'Follow-through as a fraction of distance'),
    maxPx: prm(26, 'Cap on both bumps (pt)'),
  },
  move: (ctx) => {
    const d = Math.max(1, D(ctx));
    const cap = ctx.p.maxPx / d;
    return glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.15 * side(ctx) }),
      profile: bumpProfile(ease.minJerk, {
        antic: Math.min(cap, pick(ctx.rng, ctx.p.antic)),
        over: Math.min(cap, ctx.p.over),
        anticAt: 0.14,
        overAt: 0.8,
      }),
      durationMs: fittsMs(ctx) * 1.35,
    });
  },
});

def({
  id: 'squash-stretch',
  name: 'Squash and stretch',
  category: 'expressive-personality',
  technique: 'Quint glide + volume-preserving stretch along velocity + squash on press',
  look: 'Rubbery cartoon energy. Reads instantly as "character", great for a playful brand moment.',
  refs: ['thomasjohnston1981'],
  heading: 'tangent',
  fx: { squashPress: 0.82 },
  params: { stretchMax: prm(1.18, 'Max stretch'), gain: prm(0.00022, 'Stretch per pt/s') },
  move: (ctx) => stretchBySpeed(quintGlide(ctx), { max: ctx.p.stretchMax, gain: ctx.p.gain }),
});

def({
  id: 'magnetic-snap',
  name: 'Magnetic snap',
  category: 'expressive-personality',
  technique:
    'Decelerating approach to a capture radius, then 1/d attraction pulls it in; target highlights',
  look: 'iPadOS-pointer feel: the target "grabs" the cursor. Makes the agent look decisive and the UI responsive.',
  refs: ['ipados-pointer'],
  heading: 'fixed',
  fx: { magnet: true },
  params: {
    captureRadius: prm([24, 60], 'Capture radius (pt)'),
    pull: prm([0.3, 0.6], 'Pull strength'),
    enterSpeed: prm(260, 'Speed at the capture radius (pt/s)'),
  },
  move: (ctx) => {
    const path = paths.bow(ctx.from, ctx.aim, 0.04 * side(ctx));
    const L = path.length;
    const R = Math.min(pick(ctx.rng, ctx.p.captureRadius), L * 0.5);
    const pull = pick(ctx.rng, ctx.p.pull);
    const out = [{ t: 0, x: ctx.from.x, y: ctx.from.y }];
    let s = 0;
    let v = 0;
    let t = 0;
    let snapT = null;
    const dt = DT_MS / 1000;
    while (s < L && t < 4) {
      const rem = L - s;
      if (rem > R) v = Math.min(1500, v + 7000 * dt, ctx.p.enterSpeed + 5.5 * (rem - R));
      else {
        if (snapT === null) snapT = t * 1000;
        v += 26000 * pull * (R / Math.max(rem, 6)) * dt;
      }
      s = Math.min(L, s + v * dt);
      t += dt;
      const q = path.atFraction(s / L);
      out.push({ t: t * 1000, x: q.x, y: q.y });
    }
    pinEnds(out, ctx.from, ctx.aim);
    return {
      samples: out,
      events: [{ t: snapT ?? t * 1000, type: 'snap', target: ctx.target.id }],
    };
  },
});

def({
  id: 'hover-commit',
  name: 'Hover then commit',
  category: 'expressive-personality',
  technique: 'Arc that stops 6-14 pt short, 250-600 ms considering hover, short decisive dash',
  look: 'Reads as "checking before clicking". Great for trust in agent demos; slows things down.',
  refs: [],
  heading: 'fixed',
  params: {
    stopShort: prm([6, 14], 'Stop short of the aim (pt)'),
    hoverMs: prm([250, 600], 'Hover duration'),
    commitMs: prm([80, 140], 'Commit dash duration'),
  },
  dwellMs: () => 0,
  pressMs: () => 70,
  move: (ctx) => {
    const u = unit(ctx.from, ctx.aim);
    const k = Math.min(pick(ctx.rng, ctx.p.stopShort), D(ctx) * 0.3);
    const short = { x: ctx.aim.x - u.x * k, y: ctx.aim.y - u.y * k };
    const hoverMs = pick(ctx.rng, ctx.p.hoverMs);
    const wig = idles.wiggle(ctx.rng, { amp: 1.2, hz: 0.9 });
    const first = fittsMinJerk({ ...ctx, aim: short }, 0.05);
    return {
      samples: chain([
        first,
        hold(short, hoverMs, (tau) => wig(tau * hoverMs, hoverMs)),
        glide(
          { from: short, aim: ctx.aim },
          { profile: ease.inOutCubic, durationMs: pick(ctx.rng, ctx.p.commitMs) }
        ),
      ]),
      events: [{ t: first[first.length - 1].t, type: 'consider', ms: hoverMs }],
    };
  },
});

def({
  id: 'thinking-wiggle',
  name: 'Thinking wiggle',
  category: 'expressive-personality',
  technique:
    'Bursty low-frequency noise jiggle at rest while "reasoning", then a Fitts min-jerk move',
  look: 'Like someone fidgeting with the mouse while reading. Shows the agent is busy, not frozen.',
  refs: [],
  heading: 'fixed',
  params: {
    amp: prm([2, 6], 'Wiggle amplitude (pt)'),
    hz: prm([0.6, 1.5], 'Wiggle frequency'),
    thinkMs: prm([700, 1100], 'Thinking time before each move'),
  },
  beforeMove: (ctx) => {
    const ms = pick(ctx.rng, ctx.p.thinkMs);
    ctx.api.emit('think', { ms });
    ctx.api.idleAt(
      ms,
      idles.wiggle(ctx.rng, { amp: pick(ctx.rng, ctx.p.amp), hz: pick(ctx.rng, ctx.p.hz) })
    );
  },
  move: (ctx) => fittsMinJerk(ctx),
});

def({
  id: 'figure-eight-idle',
  name: 'Figure-eight idle',
  category: 'expressive-personality',
  technique: 'Lissajous x = a sin t, y = b sin 2t while waiting, then quint glide',
  look: 'Calm, clearly "busy" loop. More legible than a wiggle, more mechanical too.',
  refs: [],
  heading: 'fixed',
  params: {
    a: prm([6, 12], 'Horizontal radius (pt)'),
    b: prm([3, 6], 'Vertical radius (pt)'),
    periodMs: prm([1600, 2600], 'Loop period'),
  },
  beforeMove: (ctx) => {
    const ms = pick(ctx.rng, ctx.p.periodMs);
    ctx.api.emit('think', { ms });
    ctx.api.idleAt(
      ms,
      idles.figureEight(ctx.rng, {
        a: pick(ctx.rng, ctx.p.a),
        b: pick(ctx.rng, ctx.p.b),
        periodMs: ms,
      })
    );
  },
  move: (ctx) => quintGlide(ctx),
});

def({
  id: 'breathing-idle',
  name: 'Breathing idle',
  category: 'expressive-personality',
  technique: 'Quint glide; at rest a slow scale pulse (to 1.05) and 1-2 pt bob',
  look: 'Alive but unobtrusive at rest. The safest "personality" layer to ship by default.',
  refs: [],
  heading: 'fixed',
  idle: 'breathe',
  idleOpts: () => ({ scale: 0.06, periodMs: 1400, bob: 1.5 }),
  params: { restMs: prm(900, 'Rest before each move (ms)') },
  beforeMove: (ctx) =>
    ctx.api.idleAt(ctx.p.restMs, idles.breathe(ctx.rng, { scale: 0.06, periodMs: 1400, bob: 1.5 })),
  dwellMs: () => 200,
  postMs: () => 300,
  move: (ctx) => quintGlide(ctx),
});

const polyline = (api, pts, rng, { speed, pauseMs, profile = ease.inOutSine }) => {
  for (let i = 0; i < pts.length; i++) {
    const from = api.pos;
    const to = pts[i];
    const d = dist(from, to);
    if (d > 0.5)
      api.push(
        glide(
          { from, aim: to },
          { profile, durationMs: Math.max(120, (d / pick(rng, speed)) * 1000) }
        )
      );
    if (pauseMs) api.idleAt(pick(rng, pauseMs));
  }
};

def({
  id: 'f-pattern-scan',
  name: 'F-pattern read',
  category: 'expressive-personality',
  technique:
    'Intro polyline: top row, shorter mid row, down the left edge (saccade pauses), then quint glides',
  look: 'Shows the agent "reading" the page before acting. Strong storytelling for demos.',
  refs: ['nielsen-fpattern'],
  heading: 'fixed',
  params: {
    rowSpeed: prm([500, 900], 'Row sweep speed (pt/s)'),
    saccadeMs: prm([60, 140], 'Pause between sweeps'),
  },
  intro: ({ api, rng, p }) => {
    const L = 170;
    const R = 1110;
    api.emit('read', { ms: 2400 });
    polyline(api, [{ x: L, y: 150 }], rng, { speed: [900, 1200], pauseMs: p.saccadeMs });
    polyline(api, [{ x: R, y: 150 }], rng, {
      speed: p.rowSpeed,
      pauseMs: p.saccadeMs,
      profile: ease.inOutQuad,
    });
    polyline(api, [{ x: L, y: 330 }], rng, { speed: [1400, 1800], pauseMs: p.saccadeMs });
    polyline(api, [{ x: L + 0.6 * (R - L), y: 330 }], rng, {
      speed: p.rowSpeed,
      pauseMs: p.saccadeMs,
      profile: ease.inOutQuad,
    });
    polyline(
      api,
      [
        { x: L, y: 400 },
        { x: L, y: 700 },
      ],
      rng,
      { speed: [500, 700], pauseMs: [40, 80] }
    );
  },
  move: (ctx) => quintGlide(ctx),
});

def({
  id: 'z-skim',
  name: 'Z-pattern skim',
  category: 'expressive-personality',
  technique: 'Intro Z polyline (easeInOutSine legs), then quint glides',
  look: 'A fast overview sweep of a sparse page before acting. Lighter than the F-read.',
  refs: [],
  heading: 'fixed',
  params: { legMs: prm([250, 450], 'Duration per leg') },
  intro: ({ api, rng, p }) => {
    const pts = [
      { x: 150, y: 140 },
      { x: 1130, y: 140 },
      { x: 150, y: 660 },
      { x: 1130, y: 660 },
    ];
    api.emit('read', { ms: 1600 });
    for (const q of pts)
      api.push(
        glide(
          { from: api.pos, aim: q },
          { profile: ease.inOutSine, durationMs: pick(rng, p.legMs) }
        )
      );
  },
  move: (ctx) => quintGlide(ctx),
});

def({
  id: 'option-sweep',
  name: 'Compare-options sweep',
  category: 'expressive-personality',
  technique: 'Visit 2-3 nearby candidate spots with short hovers, then go to the chosen target',
  look: 'Visible deliberation. Only honest when the agent really weighed alternatives.',
  refs: [],
  heading: 'fixed',
  params: {
    options: prm([2, 3.99], 'Alternatives visited (floored)'),
    hoverMs: prm([150, 300], 'Hover per alternative'),
  },
  beforeMove: (ctx) => {
    if (ctx.index % 2 === 1) return;
    const n = Math.floor(pick(ctx.rng, ctx.p.options));
    ctx.api.emit('think', { ms: 900 });
    for (let i = 0; i < n; i++) {
      const ang = ctx.rng.range(0, Math.PI * 2);
      const r = ctx.rng.range(110, 220);
      const q = {
        x: clamp(ctx.aim.x + Math.cos(ang) * r, 40, STAGE.w - 40),
        y: clamp(ctx.aim.y + Math.sin(ang) * r, 40, STAGE.h - 40),
      };
      ctx.api.push(
        glide(
          { from: ctx.api.pos, aim: q },
          { profile: ease.minJerk, durationMs: durations.sqrt(dist(ctx.api.pos, q), 20, 200, 600) }
        )
      );
      ctx.api.idleAt(pick(ctx.rng, ctx.p.hoverMs));
    }
  },
  move: (ctx) => fittsMinJerk(ctx, 0.04),
});

def({
  id: 'bounce-hop',
  name: 'Bouncy hop',
  category: 'expressive-personality',
  technique: 'Parabolic hop + two decaying bounces in place, landing squash',
  look: 'Toy-like and joyful. Strong brand moment, not a daily driver.',
  refs: ['penner'],
  heading: 'fixed',
  params: { height: prm([0.1, 0.2], 'Hop height as a fraction of distance') },
  move: (ctx) => {
    const h = Math.max(30, D(ctx) * pick(ctx.rng, ctx.p.height));
    const main = hops(ctx, {
      hopPx: 1e6,
      height: h / Math.max(1, D(ctx)),
      hopMs: durations.sqrt(D(ctx), 24, 300, 900),
    });
    const bounce = (k, ms) => {
      const s = sampleTimed(
        (tau) => ({ x: ctx.aim.x, y: ctx.aim.y - 4 * h * k * tau * (1 - tau) }),
        ms
      );
      s.forEach((q, i) => {
        const tau = i / (s.length - 1);
        q.sq =
          1 - 0.12 * Math.exp(-(((tau - 1) / 0.08) ** 2)) - 0.12 * Math.exp(-((tau / 0.08) ** 2));
      });
      return pinEnds(s, ctx.aim, ctx.aim);
    };
    return chain([main, bounce(0.22, 200), bounce(0.07, 120)]);
  },
});

def({
  id: 'circle-indicate',
  name: 'Indicate circle',
  category: 'expressive-personality',
  technique: 'Approach a point on a circle around the target, one loop (easeInOutSine), then in',
  look: '"This one." Unmistakable pointing gesture; great for narrating, slow for routine clicks.',
  refs: [],
  heading: 'fixed',
  params: {
    radius: prm([18, 30], 'Loop radius (pt, plus half the target)'),
    loopMs: prm([350, 550], 'Loop duration'),
  },
  move: (ctx) => {
    const r = pick(ctx.rng, ctx.p.radius) + Math.min(ctx.target.w, ctx.target.h) * 0.3;
    const th0 = Math.atan2(ctx.from.y - ctx.aim.y, ctx.from.x - ctx.aim.x);
    const E = { x: ctx.aim.x + Math.cos(th0) * r, y: ctx.aim.y + Math.sin(th0) * r };
    const dir = side(ctx);
    const loop = sampleTimed(
      (tau) => {
        const th = th0 + dir * 2 * Math.PI * ease.inOutSine(tau);
        return { x: ctx.aim.x + Math.cos(th) * r, y: ctx.aim.y + Math.sin(th) * r };
      },
      pick(ctx.rng, ctx.p.loopMs)
    );
    pinEnds(loop, E, E);
    return chain([
      fittsMinJerk({ ...ctx, aim: E }, 0.04),
      loop,
      glide({ from: E, aim: ctx.aim }, { profile: ease.minJerk, durationMs: 160 }),
    ]);
  },
});

const oscAfter = (ctx, { axis, amp, cycles, ms }) =>
  ctx.api.idleAt(ms, (t, T) => {
    const o = amp * dampedOsc(t, T, cycles, 0.2);
    return axis === 'y' ? { x: 0, y: o } : { x: o, y: 0 };
  });

def({
  id: 'nod-confirm',
  name: 'Nod confirm',
  category: 'expressive-personality',
  technique: 'Quint glide; after a successful action two small vertical damped nods',
  look: 'A tiny "yes" after each success. Cute, legible feedback without any badge.',
  refs: [],
  heading: 'fixed',
  params: { amp: prm([3, 5], 'Nod amplitude (pt)'), ms: prm(380, 'Nod duration') },
  afterAction: (ctx) => {
    if (!ctx.target.fails)
      oscAfter(ctx, { axis: 'y', amp: pick(ctx.rng, ctx.p.amp), cycles: 2, ms: ctx.p.ms });
    ctx.api.idleAt(120);
  },
  move: (ctx) => quintGlide(ctx),
});

def({
  id: 'shake-error',
  name: 'Shake error',
  category: 'expressive-personality',
  technique:
    'Quint glide; after a failed action a quick horizontal damped shake (macOS password shake)',
  look: 'Universal "no". The route scene marks the Docs link as a failed click to show it.',
  refs: ['macos-shake'],
  heading: 'fixed',
  params: { amp: prm([5, 8], 'Shake amplitude (pt)'), ms: prm(420, 'Shake duration') },
  afterAction: (ctx) => {
    if (ctx.target.fails) {
      ctx.api.emit('fail', { target: ctx.target.id });
      oscAfter(ctx, { axis: 'x', amp: pick(ctx.rng, ctx.p.amp), cycles: 3, ms: ctx.p.ms });
    }
    ctx.api.idleAt(140);
  },
  move: (ctx) => quintGlide(ctx),
});

def({
  id: 'drowsy-drift',
  name: 'Drowsy drift',
  category: 'expressive-personality',
  technique: 'Slow sine glides; when idle, OU random walk, slight sink and fade before auto-hide',
  look: 'The cursor visibly "falls asleep" when the agent is idle. Nice idle-hide transition.',
  refs: [],
  heading: 'fixed',
  params: {
    sinkPx: prm(6, 'Sink while drowsing (pt)'),
    fadeTo: prm(0.25, 'Opacity at the end of the drift'),
    driftMs: prm(2200, 'Drift duration'),
  },
  move: (ctx) =>
    glide(ctx, {
      path: paths.bow(ctx.from, ctx.aim, 0.05 * side(ctx)),
      profile: ease.inOutSine,
      durationMs: durations.sqrt(D(ctx), 34, 420, 1400),
    }),
  outro: ({ api, rng, p }) => {
    api.idleAt(400);
    const at = { ...api.pos };
    let x = 0;
    let y = 0;
    const s = sampleTimed(() => ({ ...at }), p.driftMs);
    s.forEach((q, i) => {
      const tau = i / (s.length - 1);
      if (i) {
        x += -0.5 * x * (DT_MS / 1000) + 3 * Math.sqrt(DT_MS / 1000) * rng.normal();
        y += -0.5 * y * (DT_MS / 1000) + 3 * Math.sqrt(DT_MS / 1000) * rng.normal();
      }
      q.x = at.x + x;
      q.y = at.y + y + p.sinkPx * ease.inOutSine(tau);
      q.opacity = 1 - (1 - p.fadeTo) * ease.inOutSine(tau);
    });
    api.push(s);
  },
});

def({
  id: 'eager-dash',
  name: 'Eager dash',
  category: 'expressive-personality',
  technique: 'Sharp lognormal (sigma 0.15) with a 5% follow-through bump, short dwell',
  look: 'Energetic persona: zips over, skids slightly past, clicks right away.',
  refs: ['plamondon1995'],
  heading: 'lean',
  params: {
    sigma: prm(0.15, 'Lognormal sigma'),
    over: prm(0.05, 'Overshoot fraction'),
    maxOverPx: prm(22, 'Overshoot cap (pt)'),
  },
  dwellMs: () => 40,
  pressMs: () => 60,
  postMs: () => 80,
  move: (ctx) =>
    glide(ctx, {
      path: paths.bow(ctx.from, ctx.aim, 0.04 * side(ctx)),
      profile: bumpProfile(lognormalProfile(ctx.p.sigma), {
        over: Math.min(ctx.p.over, ctx.p.maxOverPx / Math.max(1, D(ctx))),
        overAt: 0.62,
      }),
      durationMs: fittsMs(ctx) * 0.9,
    }),
});

def({
  id: 'careful-approach',
  name: 'Careful approach',
  category: 'expressive-personality',
  technique: 'Slow lognormal (sigma 0.32) to ~93%, 1-2 corrections, Fitts b = 200, 250 ms dwell',
  look: 'Cautious persona: long deceleration, a couple of tidy corrections, then a deliberate click.',
  refs: ['plamondon1995', 'meyer1988'],
  heading: 'fixed',
  params: {
    sigma: prm(0.32, 'Lognormal sigma'),
    b: prm(200, "Fitts' slope (ms/bit)"),
    dwell: prm(250, 'Dwell before click'),
  },
  dwellMs: (p) => p.dwell,
  move: (ctx) => {
    const u = unit(ctx.from, ctx.aim);
    const n = perp(u);
    const d = D(ctx);
    const land = {
      x: ctx.aim.x - u.x * d * 0.07 + n.x * d * ctx.rng.clippedNormal(0, 0.012),
      y: ctx.aim.y - u.y * d * 0.07 + n.y * d * ctx.rng.clippedNormal(0, 0.012),
    };
    const T = fittsMs({ ...ctx, aim: land }, 50, ctx.p.b);
    const mid = lerpPt(land, ctx.aim, 0.75 + ctx.rng.range(-0.1, 0.1));
    return chain([
      glide(
        { from: ctx.from, aim: land },
        {
          path: paths.bow(ctx.from, land, 0.05 * side(ctx)),
          profile: lognormalProfile(ctx.p.sigma),
          durationMs: T,
        }
      ),
      hold(land, 50),
      glide({ from: land, aim: mid }, { profile: ease.minJerk, durationMs: 170 }),
      hold(mid, 40),
      glide({ from: mid, aim: ctx.aim }, { profile: ease.minJerk, durationMs: 140 }),
    ]);
  },
});

def({
  id: 'curious-peek',
  name: 'Curious peek',
  category: 'expressive-personality',
  technique: 'Catmull-Rom through an off-path via point, speed dips at the via point',
  look: 'Glances at something on the way, then continues. Personality without wasting much time.',
  refs: [],
  heading: 'fixed',
  params: {
    viaOffset: prm([20, 50], 'Via point offset from the chord (pt)'),
    viaSlowdown: prm(0.5, 'Speed fraction at the via point'),
  },
  move: (ctx) => {
    const n = perp(unit(ctx.from, ctx.aim));
    const off = pick(ctx.rng, ctx.p.viaOffset) * ctx.rng.sign();
    const via = add(lerpPt(ctx.from, ctx.aim, ctx.rng.range(0.4, 0.6)), {
      x: n.x * off,
      y: n.y * off,
    });
    const path = makePath(catmullRom([ctx.from, via, ctx.aim]), 256);
    const slow = ctx.p.viaSlowdown;
    const shape = (s) => bell(s) * (1 - (1 - slow) * Math.exp(-(((s - 0.5) / 0.1) ** 2)));
    return speedShaped(ctx, { path, shape, durationMs: fittsMs(ctx) * 1.25 });
  },
});

def({
  id: 'click-wobble',
  name: 'Click pulse + wobble',
  category: 'expressive-personality',
  technique:
    'Fitts min-jerk; on click a ring pulse plus a damped angular wobble about the tip; faint angle wobble at rest',
  look: 'The arrow "taps" like a finger. Expressive click feedback that does not move the hotspot.',
  refs: [],
  heading: 'fixed',
  idle: 'rotWobble',
  idleOpts: (p) => ({ deg: p.idleDeg }),
  params: {
    wobbleDeg: prm([3, 7], 'Click wobble amplitude (deg)'),
    pulseMs: prm([220, 320], 'Wobble duration'),
    idleDeg: prm(1, 'Idle wobble amplitude (deg)'),
  },
  click: (ctx) => {
    const deg = pick(ctx.rng, ctx.p.wobbleDeg);
    ctx.api.emit('press', { target: ctx.target.id });
    ctx.api.state.pressed = true;
    ctx.api.idleAt(70);
    ctx.api.state.pressed = false;
    ctx.api.emit('release', { target: ctx.target.id });
    ctx.api.emit('click', { target: ctx.target.id, x: ctx.api.pos.x, y: ctx.api.pos.y });
    ctx.api.idleAt(pick(ctx.rng, ctx.p.pulseMs), (t, T) => ({
      x: 0,
      y: 0,
      rot: ((deg * Math.PI) / 180) * dampedOsc(t, T, 2.5, 0.35),
    }));
  },
  move: (ctx) => fittsMinJerk(ctx),
});

// ===========================================================================
// Functional
// ===========================================================================

def({
  id: 'precise-click',
  name: 'Precise click',
  category: 'functional',
  technique:
    'Min-jerk-like cruise, final 15% at <= 35% speed, no overshoot, lands in the inner 40%',
  look: 'Unhurried final approach that never overshoots. Right default for small targets.',
  refs: ['fitts1954'],
  heading: 'fixed',
  bestScene: 'precision',
  params: {
    ...FITTS,
    finalPhase: prm(0.15, 'Final phase as a fraction of the path'),
    finalSpeed: prm(0.35, 'Final phase speed fraction'),
    settle: prm([80, 150], 'Settle before press (ms)'),
  },
  aim: ({ target, rng }) => ({
    x: target.x + target.w * rng.range(0.3, 0.7),
    y: target.y + target.h * rng.range(0.3, 0.7),
  }),
  dwellMs: (p) => p.settle,
  move: (ctx) => {
    const fp = ctx.p.finalPhase;
    const fs = ctx.p.finalSpeed;
    const shape = (s) => {
      if (s < 0.4) return 0.04 + Math.sin((Math.PI * s) / 0.8);
      if (s < 1 - fp) return 1 - (1 - fs) * ease.inOutSine((s - 0.4) / (0.6 - fp));
      return 0.02 + fs * Math.sqrt(Math.max(0, (1 - s) / fp));
    };
    return speedShaped(ctx, {
      path: paths.bow(ctx.from, ctx.aim, 0.03 * side(ctx)),
      shape,
      durationMs: fittsMs(ctx, ctx.p.a, ctx.p.b) * 1.2,
    });
  },
});

def({
  id: 'drag-carry',
  name: 'Drag carry',
  category: 'functional',
  technique: 'Press-hold, loaded min-jerk (x1.3), carried object lags on a spring, release dwell',
  look: 'The dragged card has weight: it trails the pointer and settles into the drop zone.',
  refs: [],
  heading: 'fixed',
  bestScene: 'drag',
  params: {
    pressHold: prm([80, 150], 'Hold after press (ms)'),
    durationScale: prm(1.3, 'Drag duration multiplier'),
    followerZeta: prm(0.7, 'Carried object damping'),
    followerOmega: prm(18, 'Carried object stiffness (rad/s)'),
    releaseDwell: prm([60, 120], 'Dwell before release'),
  },
  dragPressMs: (p) => p.pressHold,
  dragReleaseMs: (p) => p.releaseDwell,
  carryLag: (p) => ({ omega: p.followerOmega, zeta: p.followerZeta }),
  move: (ctx) =>
    ctx.action === 'drag' ? fittsMinJerk(ctx, 0.05, ctx.p.durationScale) : fittsMinJerk(ctx),
});

def({
  id: 'steering-menu',
  name: 'Menu tunnel',
  category: 'functional',
  technique:
    'Horizontal-then-vertical L path with a rounded corner; speed capped by tunnel width (steering law)',
  look: 'Never cuts diagonally across sibling menu items, so hovering never opens the wrong submenu.',
  refs: ['accotzhai1997'],
  heading: 'fixed',
  bestScene: 'menu',
  params: {
    cornerPx: prm(10, 'Corner radius (pt)'),
    speedPerPx: prm(30, 'Max speed per pt of tunnel width (pt/s)'),
  },
  move: (ctx) => {
    const a = ctx.from;
    const b = ctx.aim;
    const knee = { x: b.x, y: a.y };
    const r = Math.min(ctx.p.cornerPx, dist(a, knee) * 0.45, dist(knee, b) * 0.45);
    const k1 = dist(a, knee) > 1 ? lerpPt(knee, a, r / dist(a, knee)) : knee;
    const k2 = dist(knee, b) > 1 ? lerpPt(knee, b, r / dist(knee, b)) : knee;
    const pts = [a, k1, knee, k2, b];
    const segs = [
      paths.line(a, k1),
      makePath((u) => {
        const v = 1 - u;
        return {
          x: v * v * k1.x + 2 * v * u * knee.x + u * u * k2.x,
          y: v * v * k1.y + 2 * v * u * knee.y + u * u * k2.y,
        };
      }),
      paths.line(k2, b),
    ];
    void pts;
    const lens = segs.map((s) => s.length);
    const total = lens.reduce((s, v) => s + v, 0) || 1;
    const path = makePath((u) => {
      let s = u * total;
      for (let i = 0; i < 3; i++) {
        if (s <= lens[i] || i === 2) return segs[i].atFraction(lens[i] ? s / lens[i] : 1);
        s -= lens[i];
      }
      return b;
    }, 512);
    const tunnel = Math.max(12, Math.min(ctx.target.h, ctx.target.w) / 2);
    return speedLaw(ctx, { path, vmax: ctx.p.speedPerPx * tunnel, accel: 6000, vmin: 30, gain: 7 });
  },
});

def({
  id: 'text-select-sweep',
  name: 'Text selection sweep',
  category: 'functional',
  technique: 'Drags: straight constant-speed sweep with eased ends; other moves Fitts min-jerk',
  look: 'Selecting text at an even reading pace, like a careful human.',
  refs: [],
  heading: 'fixed',
  bestScene: 'drag',
  params: { speed: prm([400, 700], 'Sweep speed (pt/s)') },
  move: (ctx) => {
    if (ctx.action !== 'drag') return fittsMinJerk(ctx);
    const v = pick(ctx.rng, ctx.p.speed);
    const shape = (s) => 0.05 + Math.min(1, s / 0.08, (1 - s) / 0.08) ** 0.7;
    return speedShaped(ctx, { shape, durationMs: (D(ctx) / v) * 1000 * 1.1 });
  },
});

def({
  id: 'scroll-aware',
  name: 'Scroll-aware park',
  category: 'functional',
  technique:
    'Fitts min-jerk; parks over the scroller and bobs 2 pt per wheel tick with eased content',
  look: 'You can see each wheel notch, so scrolling reads as an action rather than the page moving by itself.',
  refs: [],
  heading: 'fixed',
  bestScene: 'scroll',
  params: { tickBob: prm(2, 'Bob per wheel tick (pt)'), tickMs: prm(90, 'Time per tick') },
  scroll: (ctx) => {
    const ticks = Math.round((ctx.target.scroll ?? 240) / 40);
    for (let k = 0; k < ticks; k++) {
      ctx.api.emit('scroll', { dy: 40, target: ctx.target.id });
      ctx.api.idleAt(ctx.p.tickMs, (t, T) => ({
        x: 0,
        y: ctx.p.tickBob * Math.sin((Math.PI * t) / T),
      }));
    }
  },
  move: (ctx) => fittsMinJerk(ctx),
});

def({
  id: 'double-click-rhythm',
  name: 'Double-click rhythm',
  category: 'functional',
  technique:
    'Fitts min-jerk; clicks as two presses at a human interval with total stillness between',
  look: 'Readable double-clicks (two distinct pulses), useful for file managers and text selection.',
  refs: [],
  heading: 'fixed',
  params: {
    intervalMs: prm([90, 160], 'Gap between presses'),
    pressMs: prm([50, 80], 'Press length'),
  },
  click: (ctx) => {
    for (let k = 0; k < 2; k++) {
      ctx.api.emit('press', { target: ctx.target.id });
      ctx.api.state.pressed = true;
      ctx.api.idleAt(pick(ctx.rng, ctx.p.pressMs));
      ctx.api.state.pressed = false;
      ctx.api.emit('release', { target: ctx.target.id });
      ctx.api.emit('click', { target: ctx.target.id, x: ctx.api.pos.x, y: ctx.api.pos.y });
      if (k === 0) ctx.api.idleAt(pick(ctx.rng, ctx.p.intervalMs));
    }
  },
  move: (ctx) => fittsMinJerk(ctx),
});

def({
  id: 'type-park',
  name: 'Type park',
  category: 'functional',
  technique:
    "After focusing a field, slide out of the caret's way and dim while typing; restore before the next move",
  look: 'Keeps typed text visible in recordings; the cursor politely steps aside.',
  refs: [],
  heading: 'fixed',
  bestScene: 'type',
  params: { offset: prm([24, 40], 'Park offset (pt)'), dim: prm(0.6, 'Opacity while parked') },
  whileTyping: (ctx, ms) => {
    const from = { ...ctx.api.pos };
    const k = pick(ctx.rng, ctx.p.offset);
    const park = { x: from.x + k, y: from.y + k * 1.4 };
    const slide = glide({ from, aim: park }, { profile: ease.minJerk, durationMs: 200 });
    slide.forEach((q, i) => (q.opacity = 1 - (1 - ctx.p.dim) * (i / (slide.length - 1))));
    ctx.api.push(slide);
    const restMs = Math.max(0, ms - 320);
    const rest = sampleTimed(() => ({ ...park }), restMs);
    rest.forEach(
      (q, i) =>
        (q.opacity =
          i > rest.length - 14
            ? ctx.p.dim + (1 - ctx.p.dim) * ((i - (rest.length - 14)) / 13)
            : ctx.p.dim)
    );
    ctx.api.push(rest);
  },
  move: (ctx) => fittsMinJerk(ctx),
});

def({
  id: 'bang-bang-fast',
  name: 'Time-optimal fast',
  category: 'functional',
  technique: 'Straight line, max accel then max decel (triangular velocity)',
  look: 'Fastest readable move. Feels robotic, but perfect for a "turbo" or low-latency mode.',
  refs: [],
  heading: 'fixed',
  params: { aMax: prm([12000, 25000], 'Acceleration limit (pt/s^2)') },
  dwellMs: () => 30,
  pressMs: () => 60,
  postMs: () => 60,
  move: (ctx) => {
    const T = 2 * Math.sqrt(D(ctx) / pick(ctx.rng, ctx.p.aMax)) * 1000;
    return glide(ctx, {
      profile: (t) => (t < 0.5 ? 2 * t * t : 1 - 2 * (1 - t) ** 2),
      durationMs: Math.max(DT_MS * 3, T),
    });
  },
});

def({
  id: 'ghost-jump',
  fixedTiming: true,
  name: 'Instant with ghost arc',
  category: 'functional',
  technique: 'Jump to the target instantly; a faint ghost arc shows where it came from and fades',
  look: 'Zero wait, still comprehensible. Good for high-speed agent runs.',
  refs: [],
  heading: 'fixed',
  fx: { ghost: true },
  params: { ghostMs: prm([200, 350], 'Ghost fade (ms)') },
  move: (ctx) => ({
    samples: [
      { t: 0, x: ctx.from.x, y: ctx.from.y },
      { t: DT_MS, x: ctx.aim.x, y: ctx.aim.y },
    ],
    events: [
      {
        t: DT_MS,
        type: 'ghost',
        from: ctx.from,
        to: { ...ctx.aim },
        ms: pick(ctx.rng, ctx.p.ghostMs),
      },
    ],
  }),
  postMs: () => 300,
});

def({
  id: 'adaptive-auto',
  name: 'Adaptive auto',
  category: 'functional',
  technique:
    'Dispatch per move: precise-click for small targets, keynote-swoop for long throws, Fitts min-jerk otherwise',
  look: 'The proposed product default: calm for normal clicks, careful on tiny targets, cinematic on long jumps.',
  refs: [],
  heading: 'tangent',
  params: {
    smallTarget: prm(16, 'Small-target threshold (pt)'),
    longPx: prm(900, 'Long-move threshold (pt)'),
  },
  move: (ctx) => {
    const pickId =
      targetWidth(ctx) < ctx.p.smallTarget
        ? 'precise-click'
        : D(ctx) > ctx.p.longPx
          ? 'keynote-swoop'
          : 'fitts-minjerk';
    const other = byId[pickId];
    return other.move({ ...ctx, p: resolveParams(other) });
  },
});

def({
  id: 'oneeuro-follow',
  name: 'One-Euro follow',
  category: 'functional',
  technique:
    'Noisy 30 Hz target stream (e.g. live model estimates) through a 1-Euro filter, then settle',
  look: 'Smooth when the target barely moves, responsive when it jumps. For live or streamed agent coordinates.',
  refs: ['casiez2012'],
  heading: 'fixed',
  params: {
    minCutoff: prm(1, 'Min cutoff (Hz)'),
    beta: prm(0.007, 'Speed coefficient'),
    dCutoff: prm(1, 'Derivative cutoff (Hz)'),
    noise: prm(6, 'Stream noise (pt)'),
  },
  move: (ctx) => {
    const T = fittsMs(ctx) * 0.8;
    const total = T + 300;
    const stream = [];
    let held = { ...ctx.from };
    let nextUpdate = 0;
    for (let t = 0; t <= total; t += DT_MS) {
      if (t >= nextUpdate) {
        const f = Math.min(1, t / T);
        const base = lerpPt(ctx.from, ctx.aim, ease.inOutSine(f));
        const s = ctx.p.noise * (1 - 0.8 * f);
        held = {
          x: base.x + ctx.rng.normal(0, s) * (f < 1 ? 1 : 0.3),
          y: base.y + ctx.rng.normal(0, s) * (f < 1 ? 1 : 0.3),
        };
        nextUpdate += 1000 / 30;
      }
      stream.push({ t, x: held.x, y: held.y });
    }
    stream[0] = { t: 0, ...ctx.from };
    const filtered = oneEuro(stream, {
      minCutoff: ctx.p.minCutoff,
      beta: ctx.p.beta,
      dCutoff: ctx.p.dCutoff,
    });
    const last = filtered[filtered.length - 1];
    return chain([
      filtered,
      glide({ from: last, aim: ctx.aim }, { profile: ease.minJerk, durationMs: 110 }),
    ]);
  },
});

export const byId = {};

// ===========================================================================
// Director's cut: polished compositions for demos and the X post. All use
// distance-aware Fitts timing (the Codex-style cursor uses a fixed ~1.4 s).
// ===========================================================================

const dcMs = (ctx, scale = 1) =>
  clamp(150 + 120 * Math.log2(D(ctx) / targetWidth(ctx) + 1), 300, 1000) * scale;
const dc = (c) => def({ category: 'directors-cut', heading: 'tangent', ...c, refs: c.refs ?? [] });

dc({
  id: 'dc-signature-arc',
  name: 'Signature arc',
  basedOn: ['minjerk-arc', 'anticipate-follow', 'velocity-fog'],
  technique:
    'Cua arc 0.16 / min-jerk with a 1.8% follow-through (cap 8 pt) / Fitts timing / speed glow',
  look: 'Proposed Cua default: one confident arc, a whisper of follow-through, soft glow at speed, squish + ripple on click.',
  fx: { fog: true },
  params: {
    arcSize: prm(0.16, 'Arc size'),
    over: prm(0.018, 'Follow-through fraction'),
    overCapPx: prm(8, 'Follow-through cap (pt)'),
  },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: ctx.p.arcSize * side(ctx), arcFlow: 0.15 }),
      profile: bumpProfile(ease.minJerk, {
        over: Math.min(ctx.p.over, ctx.p.overCapPx / Math.max(1, D(ctx))),
        overAt: 0.82,
      }),
      durationMs: dcMs(ctx, 1.1),
    }),
});

dc({
  id: 'dc-spring-settle',
  name: 'Spring settle',
  basedOn: ['elastic-arrival', 'studio-spring'],
  technique: 'Arc 0.12 / min-jerk into one tasteful damped overshoot (6 pt, 1.3 cycles) / Fitts',
  look: 'Lands with a single soft bounce, like a well-tuned iOS spring. Satisfying without being cartoonish.',
  fx: { fog: true },
  params: { ampPx: prm(6, 'Overshoot (pt)'), cycles: prm(1.3, 'Wobble cycles') },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.12 * side(ctx) }),
      profile: wobbleProfile((t) => ease.minJerk(Math.min(1, t / 0.68)), {
        amp: Math.min(0.05, ctx.p.ampPx / Math.max(1, D(ctx))),
        cycles: ctx.p.cycles,
        decay: 2.6,
        start: 0.55,
      }),
      durationMs: dcMs(ctx, 1.35),
    }),
});

dc({
  id: 'dc-comet-swoop',
  name: 'Comet swoop',
  basedOn: ['keynote-swoop', 'comet-trail'],
  technique: 'Wide arc 0.24 / easeInOutCubic / Fitts / short fading trail (180 ms)',
  look: 'The launch-video move: a wide, readable swoop with a short comet tail that viewers can follow.',
  fx: { trail: { ms: 180, opacity: 0.38 } },
  params: { arcSize: prm(0.24, 'Arc size') },
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: ctx.p.arcSize * side(ctx), arcFlow: 0.2 }),
      profile: ease.inOutCubic,
      durationMs: dcMs(ctx, 1.15),
    }),
});

dc({
  id: 'dc-anticipate',
  name: 'Wind-up and land',
  basedOn: ['anticipate-follow'],
  heading: 'lean',
  technique:
    'Arc 0.14 / min-jerk with 3% pull-back and 3% follow-through (caps 12 / 10 pt) / Fitts x1.25',
  look: 'A tiny wind-up says "here I go", a tiny overshoot says "got it". Character with restraint.',
  fx: { squashPress: 0.84 },
  params: { antic: prm(0.03, 'Pull-back fraction'), over: prm(0.03, 'Follow-through fraction') },
  move: (ctx) => {
    const d = Math.max(1, D(ctx));
    return glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.14 * side(ctx) }),
      profile: bumpProfile(ease.minJerk, {
        antic: Math.min(ctx.p.antic, 12 / d),
        over: Math.min(ctx.p.over, 10 / d),
        anticAt: 0.13,
        overAt: 0.8,
      }),
      durationMs: dcMs(ctx, 1.25),
    });
  },
});

dc({
  id: 'dc-magnetic',
  name: 'Magnetic lock-on',
  basedOn: ['magnetic-snap'],
  heading: 'fixed',
  technique:
    'Arc approach decelerating to a 40 pt capture radius, then pulled in; target glows and ripples',
  look: 'The target reaches out and grabs the cursor. Makes every click feel intentional and the UI responsive.',
  fx: { magnet: true },
  params: { captureRadius: prm(40, 'Capture radius (pt)'), pull: prm(0.45, 'Pull strength') },
  move: (ctx) =>
    byId['magnetic-snap'].move({
      ...ctx,
      p: { captureRadius: ctx.p.captureRadius, pull: ctx.p.pull, enterSpeed: 300 },
    }),
});

dc({
  id: 'dc-bank-glide',
  name: 'Banking glide',
  basedOn: ['heading-bank', 'velocity-fog'],
  heading: 'bank',
  headingOpts: () => ({ gain: 1.1, maxBank: (14 * Math.PI) / 180 }),
  technique:
    'Arc 0.2 / easeInOutCubic / Fitts / tip leads, banks up to 14 deg into turns / speed glow',
  look: 'Flies like a paper plane: leans into the curve, levels out to land. Most "alive" of the calm options.',
  fx: { fog: true },
  params: {},
  move: (ctx) =>
    glide(ctx, {
      path: paths.cua(ctx.from, ctx.aim, { arcSize: 0.2 * side(ctx), arcFlow: 0.1 }),
      profile: ease.inOutCubic,
      durationMs: dcMs(ctx, 1.15),
    }),
});

dc({
  id: 'dc-lift-hop',
  name: 'Soft lift',
  basedOn: ['lift-hop'],
  heading: 'fixed',
  technique:
    'Low parabola (8% height) / easeInOutSine / Fitts / lift to 1.12x with shadow, land squash',
  look: 'Picks itself up off the page and sets down gently. Reads as 3D and premium.',
  fx: { shadow: true },
  params: {
    lift: prm(1.12, 'Peak scale'),
    height: prm(0.08, 'Hop height fraction'),
    landSquash: prm(0.93, 'Landing scale'),
  },
  move: (ctx) => {
    const out = byId['lift-hop'].move({
      ...ctx,
      p: { lift: ctx.p.lift, height: ctx.p.height, landSquash: ctx.p.landSquash },
    });
    const T = out[out.length - 1].t;
    const k = dcMs(ctx, 1.2) / T;
    return out.map((q) => ({ ...q, t: q.t * k }));
  },
  arrive: (ctx) =>
    ctx.api.idleAt(160, (t, T) => ({
      x: 0,
      y: 0,
      scale: 1 - (1 - ctx.p.landSquash) * (1 - ease.outBack(t / T)),
    })),
});

dc({
  id: 'dc-studio-smooth',
  name: 'Studio smooth',
  basedOn: ['studio-spring', 'velocity-fog'],
  technique:
    'Quick raw move smoothed by a critically damped spring (tension 190, friction 27) / speed glow',
  look: 'Screen Studio polish: buttery, unhurried landings that never overshoot. Safest cinematic choice.',
  fx: { fog: true },
  params: { tension: prm(190, 'Spring tension'), friction: prm(27, 'Spring friction') },
  move: (ctx) => {
    const raw = glide(ctx, {
      path: paths.bow(ctx.from, ctx.aim, 0.06 * side(ctx)),
      profile: ease.outCubic,
      durationMs: dcMs(ctx, 0.6),
    });
    const w = Math.sqrt(ctx.p.tension);
    return springSmooth(raw, { freq: w / (2 * Math.PI), zeta: ctx.p.friction / (2 * w) });
  },
});

dc({
  id: 'dc-weave',
  name: 'Flow weave',
  basedOn: ['spline-weave', 'comet-trail'],
  bestScene: 'sweep',
  technique:
    'Catmull-Rom through previous / current / next target / easeInOutSine / Fitts x0.9 / faint trail',
  look: 'Multi-step plans read as one fluid gesture; each approach already curves toward the next step.',
  fx: { trail: { ms: 140, opacity: 0.25 } },
  params: {},
  dwellMs: () => 40,
  pressMs: () => 70,
  postMs: () => 50,
  move: (ctx) => {
    const prev = ctx.state.prevFrom ?? add(ctx.from, sub(ctx.from, ctx.aim));
    const next = ctx.next ?? add(ctx.aim, sub(ctx.aim, ctx.from));
    const cr = catmullRom([prev, ctx.from, ctx.aim, next]);
    ctx.state.prevFrom = ctx.from;
    return glide(ctx, {
      path: makePath((u) => cr(1 / 3 + u / 3)),
      profile: ease.inOutSine,
      durationMs: dcMs(ctx, 0.95),
    });
  },
});

dc({
  id: 'dc-calm-breath',
  name: 'Calm companion',
  basedOn: ['quint-glide', 'breathing-idle', 'nod-confirm'],
  heading: 'fixed',
  technique: 'Quint glide / Fitts / breathing scale pulse at rest / small nod after each success',
  look: 'Quietly alive: breathes while waiting, nods when an action lands. Friendly without stealing focus.',
  idle: 'breathe',
  idleOpts: () => ({ scale: 0.05, periodMs: 1600, bob: 1 }),
  params: {},
  dwellMs: () => 160,
  move: (ctx) =>
    glide(ctx, {
      path: paths.bow(ctx.from, ctx.aim, 0.05 * side(ctx)),
      profile: ease.inOutQuint,
      durationMs: dcMs(ctx, 1.25),
    }),
  afterAction: (ctx) => {
    if (!ctx.target.fails) oscAfter(ctx, { axis: 'y', amp: 3, cycles: 2, ms: 340 });
    ctx.api.idleAt(260, idles.breathe(ctx.rng, { scale: 0.05, periodMs: 1600, bob: 1 }));
  },
});

dc({
  id: 'dc-squash-pop',
  name: 'Squash pop',
  basedOn: ['squash-stretch', 'quint-glide'],
  technique:
    'Quint glide / Fitts / stretch along velocity (max 1.1) / deep squash on press + ripple',
  look: 'Rubbery but tidy: stretches at speed, squishes satisfyingly on every click.',
  fx: { squashPress: 0.78 },
  params: { stretchMax: prm(1.1, 'Max stretch') },
  move: (ctx) =>
    stretchBySpeed(
      glide(ctx, {
        path: paths.bow(ctx.from, ctx.aim, 0.05 * side(ctx)),
        profile: ease.inOutQuint,
        durationMs: dcMs(ctx, 1.2),
      }),
      { max: ctx.p.stretchMax, gain: 0.00012 }
    ),
});

dc({
  id: 'dc-think-loop',
  name: 'Think, then glide',
  basedOn: ['figure-eight-idle', 'dc-signature-arc'],
  technique: 'Small figure-eight (5 x 2.5 pt) while the model reasons, then the signature arc',
  look: 'Shows the agent thinking without a spinner, then moves with purpose. Great narration beat for videos.',
  fx: { fog: true },
  params: { thinkMs: prm(1100, 'Think loop duration') },
  beforeMove: (ctx) => {
    if (ctx.index % 2 === 1) return;
    ctx.api.emit('think', { ms: ctx.p.thinkMs });
    ctx.api.idleAt(
      ctx.p.thinkMs,
      idles.figureEight(ctx.rng, { a: 5, b: 2.5, periodMs: ctx.p.thinkMs })
    );
  },
  move: (ctx) =>
    byId['dc-signature-arc'].move({ ...ctx, p: resolveParams(byId['dc-signature-arc']) }),
});

for (const c of candidates) byId[c.id] = c;

export const categories = [
  { id: 'directors-cut', name: "Director's cut" },
  { id: 'human-realistic', name: 'Human-realistic' },
  { id: 'cinematic-demo', name: 'Cinematic demo' },
  { id: 'expressive-personality', name: 'Expressive' },
  { id: 'functional', name: 'Functional' },
];

// Silence unused-import lints for helpers kept for candidates in progress.
void idleOffsets;
void manhattan;
void resample;
