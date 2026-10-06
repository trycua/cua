// Export golden per-move trajectories for the styles Cua Driver ships, so the
// Rust port (cua-motion/src/plan.rs) can be checked against the lab.
//
//   node libs/cua-driver/tools/cursor-gallery/motion-lab/test/export-parity.mjs
//
// Writes libs/cua-driver/rust/crates/cua-motion/tests/fixtures/motion_parity.json.
// Every scene, seed 7, every timing mode. Moves aim at target centres and chain
// from the previous aim, so each case is one call of the candidate's `move`.

import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { byId } from '../motion/candidates.js';
import { dist } from '../motion/math.js';
import { fittsTimingMs, resolveParams } from '../motion/plan.js';
import { Rng } from '../motion/rng.js';
import { center, sceneList } from '../motion/scenes.js';

const STYLES = [
  'dc-signature-arc',
  'dc-spring-settle',
  'dc-magnetic',
  'dc-comet-swoop',
  'adaptive-auto',
  'dubins-glide',
];
const TIMINGS = ['native', 'fitts', 'fixed'];
const SEED = 7;
const FIXED_MS = 1430;

const round = (v) => Math.round(v * 1e3) / 1e3;
// Every 4th sample plus the last keeps the file small; Rust interpolates its
// own trajectory at these times.
const thin = (samples) => samples.filter((_, i) => i % 4 === 0 || i === samples.length - 1);
const cases = [];
for (const id of STYLES) {
  const c = byId[id];
  for (const timing of TIMINGS) {
    for (const scene of sceneList) {
      let from = { ...scene.start };
      scene.waypoints.forEach((wp, i) => {
        const aim = center(wp);
        const seed = `${SEED}|${id}|${scene.id}|${i}`;
        const ctx = {
          from: { ...from },
          aim,
          target: wp,
          rng: new Rng(seed),
          p: resolveParams(c),
          state: {},
          index: i,
          action: wp.action,
          prev: from,
          next: null,
          scene,
        };
        const out = c.move(ctx);
        let samples = Array.isArray(out) ? out : out.samples;
        let events = Array.isArray(out) ? [] : (out.events ?? []);
        if (timing !== 'native' && !c.fixedTiming) {
          const T = samples[samples.length - 1].t;
          const want =
            timing === 'fixed'
              ? FIXED_MS
              : fittsTimingMs(dist(ctx.from, samples[samples.length - 1]), Math.min(wp.w, wp.h));
          if (T > 0 && samples.length >= 3) {
            const k = want / T;
            samples = samples.map((q) => ({ ...q, t: q.t * k }));
            events = events.map((e) => ({ ...e, t: e.t * k }));
          }
        }
        cases.push({
          style: id,
          timing,
          scene: scene.id,
          index: i,
          seed,
          from: ctx.from,
          aim,
          target: [wp.x, wp.y, wp.w, wp.h],
          snap_ms: events.find((e) => e.type === 'snap')?.t ?? null,
          samples: thin(samples).map((q) => [round(q.t), round(q.x), round(q.y)]),
        });
        from = aim;
      });
    }
  }
}

const here = dirname(fileURLToPath(import.meta.url));
const outPath = join(
  here,
  '../../../../rust/crates/cua-motion/tests/fixtures/motion_parity.json'
);
writeFileSync(
  outPath,
  JSON.stringify({ source: 'motion-lab test/export-parity.mjs', seed: SEED, fixed_ms: FIXED_MS, cases })
);
console.log(`wrote ${cases.length} cases to ${outPath}`);
