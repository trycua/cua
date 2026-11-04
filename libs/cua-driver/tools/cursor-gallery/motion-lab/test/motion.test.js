import assert from 'node:assert/strict';
import test from 'node:test';

import { byId, candidates, categories } from '../motion/candidates.js';
import { DT_MS, cuaBezier, dubinsPath, ease } from '../motion/math.js';
import { fittsTimingMs, kinematics, metrics, plan } from '../motion/plan.js';
import { inside, sceneList, scenes } from '../motion/scenes.js';

const SEEDS = [1, 7, 4242];

function lastPointOfSegment(result, seg) {
  let q = null;
  for (const p of result.points) {
    if (p.t > seg.t1) break;
    q = p;
  }
  return q;
}

test('catalog: at least 50 showcase candidates, unique ids, documented params', () => {
  const showcase = candidates.filter((c) => c.tier !== 'reference');
  assert.ok(showcase.length >= 50, `showcase has ${showcase.length}`);
  assert.equal(new Set(candidates.map((c) => c.id)).size, candidates.length);
  const cats = new Set(categories.map((c) => c.id));
  for (const c of candidates) {
    assert.ok(cats.has(c.category), `${c.id} category ${c.category}`);
    assert.ok(c.name && c.technique && c.look, `${c.id} docs`);
    assert.equal(typeof c.move, 'function');
    for (const [k, spec] of Object.entries(c.params ?? {}))
      assert.ok(spec.doc && 'v' in spec, `${c.id}.${k} documented`);
  }
  assert.equal(candidates.filter((c) => c.category === 'directors-cut').length, 12);
});

for (const c of candidates) {
  test(`${c.id}: finite, monotonic, lands in every target, deterministic`, () => {
    for (const scene of sceneList) {
      for (const seed of SEEDS) {
        const r = plan(c, scene, { seed });
        assert.ok(r.points.length > 2);
        assert.deepEqual([r.points[0].x, r.points[0].y], [scene.start.x, scene.start.y]);
        for (let i = 0; i < r.points.length; i++) {
          const q = r.points[i];
          assert.ok(
            Number.isFinite(q.x) &&
              Number.isFinite(q.y) &&
              Number.isFinite(q.t) &&
              Number.isFinite(q.heading),
            `${scene.id}/${seed} NaN at ${i}`
          );
          if (i)
            assert.ok(q.t > r.points[i - 1].t, `${scene.id}/${seed} time not increasing at ${i}`);
        }
        assert.equal(r.segments.length, scene.waypoints.length);
        for (const seg of r.segments) {
          const q = lastPointOfSegment(r, seg);
          assert.ok(
            inside(q, scene.waypoints[seg.index], 0.01),
            `${scene.id}/${seed} misses ${seg.target}: ${q.x.toFixed(1)},${q.y.toFixed(1)}`
          );
        }
        // Clicks happen inside their target.
        for (const e of r.events.filter((e) => e.type === 'click')) {
          const wp = scene.waypoints.find((w) => w.id === e.target);
          assert.ok(inside(e, wp, 0.01), `${scene.id}/${seed} click outside ${e.target}`);
        }
      }
    }
    const a = plan(c, scenes.route, { seed: 99 });
    const b = plan(c, scenes.route, { seed: 99 });
    assert.deepEqual(a, b, 'same seed gives identical plans');
  });
}

test('speed stays bounded: no teleporting except candidates that are meant to', () => {
  const jumpers = new Set(['ghost-jump', 'teleport-ripple']);
  for (const c of candidates) {
    if (jumpers.has(c.id)) continue;
    const r = plan(c, scenes.route, { seed: 3 });
    const k = kinematics(r.points);
    const peak = Math.max(...k.speed);
    assert.ok(peak < 20000, `${c.id} peak speed ${peak.toFixed(0)} pt/s`);
  }
});

test("director's cut is smooth: bounded jerk and tasteful overshoot under Fitts timing", () => {
  for (const c of candidates.filter((c) => c.category === 'directors-cut')) {
    for (const scene of [scenes.route, scenes.long, scenes.precision]) {
      const r = plan(c, scene, { seed: 5, timing: 'fitts' });
      const m = metrics(r);
      assert.ok(m.overshootPt <= 14, `${c.id}/${scene.id} overshoot ${m.overshootPt.toFixed(1)}`);
      for (const seg of r.segments) {
        const k = kinematics(r.points, { t0: seg.t0, t1: seg.t1 });
        const maxAccel = Math.max(...k.accel);
        assert.ok(
          maxAccel < 2.5e5,
          `${c.id}/${scene.id}/${seg.target} accel ${maxAccel.toFixed(0)}`
        );
      }
    }
  }
});

test('timing modes: fixed ignores distance, Fitts grows with distance and shrinks with size', () => {
  const c = byId['dc-signature-arc'];
  const fixed = plan(c, scenes.route, { seed: 1, timing: 'fixed' });
  for (const s of fixed.segments)
    assert.ok(Math.abs(s.t1 - s.t0 - 1430) < 2 * DT_MS, `fixed segment ${s.t1 - s.t0}`);
  assert.ok(fittsTimingMs(1000, 40) > fittsTimingMs(200, 40));
  assert.ok(fittsTimingMs(400, 12) > fittsTimingMs(400, 120));
  const fitts = plan(c, scenes.route, { seed: 1, timing: 'fitts' });
  const durs = fitts.segments.map((s) => s.t1 - s.t0);
  assert.ok(Math.max(...durs) - Math.min(...durs) > 100, 'Fitts timing varies per move');
});

test('primitives: easing endpoints, minimum-jerk symmetry, Cua bezier and Dubins endpoints', () => {
  for (const [name, f] of Object.entries(ease)) {
    assert.ok(Math.abs(f(0)) < 1e-9, `${name}(0)`);
    assert.ok(Math.abs(f(1) - 1) < 1e-9, `${name}(1)`);
  }
  assert.ok(Math.abs(ease.minJerk(0.5) - 0.5) < 1e-12);
  const a = { x: 10, y: 20 };
  const b = { x: 900, y: 500 };
  const bz = cuaBezier(a, b, { arcSize: 0.25 });
  assert.deepEqual(bz(0), a);
  assert.ok(Math.hypot(bz(1).x - b.x, bz(1).y - b.y) < 1e-9);
  const d = dubinsPath(a, Math.PI * 1.25, b, Math.PI * 1.25, 80);
  const end = d.fn(1);
  assert.ok(Math.hypot(end.x - b.x, end.y - b.y) < 1e-6, `dubins end ${end.x},${end.y}`);
});
