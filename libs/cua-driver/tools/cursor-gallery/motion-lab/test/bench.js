// Planning cost per candidate: mean milliseconds to plan one move (all
// scenes, 5 seeds), plus route-scene metrics. `node test/bench.js --json`.
import { candidates } from '../motion/candidates.js';
import { metrics, plan } from '../motion/plan.js';
import { sceneList, scenes } from '../motion/scenes.js';

const rows = [];
for (const c of candidates) {
  let ms = 0;
  let moves = 0;
  for (let rep = 0; rep < 5; rep++) {
    for (const s of sceneList) {
      const t0 = performance.now();
      plan(c, s, { seed: rep + 1 });
      ms += performance.now() - t0;
      moves += s.waypoints.length;
    }
  }
  const m = metrics(plan(c, scenes.route, { seed: 7, timing: 'native' }));
  const f = metrics(plan(c, scenes.route, { seed: 7, timing: 'fitts' }));
  rows.push({
    id: c.id,
    category: c.category,
    tier: c.tier ?? 'showcase',
    msPerMove: ms / moves,
    routeMoveMs: m.moveMs,
    routeMoveMsFitts: f.moveMs,
    peakSpeed: m.peakSpeed,
    overshootPt: m.overshootPt,
    efficiency: m.efficiency,
    extraSubmovements: m.extraSubmovements,
  });
}
if (process.argv.includes('--json')) console.log(JSON.stringify(rows, null, 2));
else
  for (const r of rows)
    console.log(
      `${r.id.padEnd(24)} ${r.msPerMove.toFixed(3)} ms/move  move ${(r.routeMoveMs / 1000).toFixed(2)} s  peak ${r.peakSpeed.toFixed(0)}  over ${r.overshootPt.toFixed(1)}`
    );
