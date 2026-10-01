// Larger candidate sets and relevance capping (#4312). Mirrors
// python/tests/test_native_density.py; the fixtures are get_window_state
// results recorded from the AppKit, GTK3, WPF, and WinUI3 harnesses with
// CUA_<HARNESS>_TASK_DENSITY set.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { validateRequest } from './choose_action.js';
import { parseWindowState } from './native.js';
import {
  HARNESSES,
  MAX_EXECUTABLE_CANDIDATES,
  NativeTask,
  compose,
  nativeChoiceRequest,
  nativeTask,
  type CapOrder,
} from './native_tasks.js';
import { NativeAccessibilitySource, immutableCandidate } from './sources.js';
import type { TaskSources } from './tasks.js';

const KINDS = ['counter', 'save-note', 'choose-size'] as const;
const TARGETS: Record<string, string[]> = {
  counter: ['ax:button:increment'],
  'save-note': ['ax:text_input:note:set:note', 'ax:button:save-note'],
  'choose-size': ['ax:radio:large', 'ax:checkbox:i-agree'],
};

function fixture(name: string): any {
  return JSON.parse(readFileSync(new URL(`../fixtures/native/${name}`, import.meta.url), 'utf8'));
}

function task(kind: string, capOrder: CapOrder = 'relevance', harness = 'gtk3'): NativeTask {
  return new NativeTask({ ...nativeTask(`${harness}-${kind}`, '/tmp/none.json'), capOrder });
}

function sources(t: NativeTask, payload: any, harness = 'gtk3'): TaskSources {
  return {
    ax: NativeAccessibilitySource.fromObservation(
      parseWindowState(payload, payload.pid, payload.window_id),
      HARNESSES[harness].platform,
      { redact: t.redactText, textMethod: t.textMethod }
    ),
    visualPath: false,
    foregroundIds: new Set(),
  };
}

test('compose keeps relevant candidates over the cap in element order', () => {
  const group = Array.from({ length: 30 }, (_, n) =>
    immutableCandidate({ id: `ax:button:b${n}`, description: 'd', tool: 'click', arguments: {}, source: 'ax' })
  );
  const relevant = new Set(['ax:button:b29', 'ax:button:b27']);
  const rank = (c: { id: string }) => (relevant.has(c.id) ? 0 : 2);
  const { candidates, stats } = compose({ ax: group }, new Set(), undefined, undefined, rank);
  const ids = candidates.map((c) => c.id);
  assert.equal(ids.length, MAX_EXECUTABLE_CANDIDATES + 2);
  assert.equal(stats.dropped, 6);
  assert.deepEqual(ids.slice(0, 22), Array.from({ length: 22 }, (_, n) => `ax:button:b${n}`));
  assert.deepEqual(ids.slice(22), ['ax:button:b27', 'ax:button:b29', 'reobserve', 'abstain']);
  const within = compose({ ax: group.slice(0, 24) }, new Set(), undefined, undefined, rank);
  assert.deepEqual(
    within.candidates.map((c) => c.id),
    compose({ ax: group.slice(0, 24) }, new Set()).candidates.map((c) => c.id)
  );
});

test('density fixtures reproduce the golden candidate sets', () => {
  const golden = fixture('native-density-candidates-v1.json').sets;
  const harnesses = ['gtk3', 'appkit', 'wpf', 'winui3'];
  assert.equal(Object.keys(golden).length, harnesses.length * 2 * 3 * 2);
  for (const harness of harnesses) {
    for (const density of [12, 24]) {
      const payload = fixture(`${harness}-window-state-density-${density}-v1.json`);
      for (const kind of KINDS) {
        for (const capOrder of ['relevance', 'depth_first'] as const) {
          const t = task(kind, capOrder, harness);
          const src = sources(t, payload, harness);
          const plan = t.plan(src);
          assert.deepEqual(
            { ids: plan.candidates.map((c) => c.id), dropped: plan.stats.dropped },
            golden[`${harness}-${kind}-d${density}-${capOrder}`],
            `${harness} ${kind} d${density} ${capOrder}`
          );
          const offered = new Set(plan.candidates.map((c) => c.id));
          const present = TARGETS[kind].filter((id) => offered.has(id));
          assert.equal(present.length, density === 24 && capOrder === 'depth_first' ? 0 : TARGETS[kind].length);
          validateRequest(nativeChoiceRequest(t, src, plan, []));
        }
      }
    }
  }
});

test('expectedNext follows the declared steps', () => {
  const counter = task('counter');
  const history = [] as ReturnType<NativeTask['historyEntry']>[];
  for (let i = 0; i < 3; i += 1) {
    assert.deepEqual(counter.expectedNext(history), ['ax:button:increment']);
    history.push(counter.historyEntry(i + 1, 'ax:button:increment', undefined, { outcome: 'done' }));
  }
  assert.deepEqual(counter.expectedNext(history), []);
  const note = task('save-note');
  assert.deepEqual(note.expectedNext([]), ['ax:text_input:note:set:note']);
  assert.deepEqual(
    note.expectedNext([note.historyEntry(1, 'ax:text_input:note:set:note', undefined, { outcome: 'done' })]),
    ['ax:button:save-note']
  );
  const size = task('choose-size');
  assert.deepEqual(size.expectedNext([]), ['ax:radio:large', 'ax:checkbox:i-agree']);
  assert.deepEqual(size.expectedNext([size.historyEntry(1, 'ax:radio:large', 'background_denied')]), [
    'ax:radio:large',
    'ax:checkbox:i-agree',
  ]);
  assert.deepEqual(
    size.expectedNext([size.historyEntry(1, 'ax:radio:large:foreground', undefined, { outcome: 'done' })]),
    ['ax:checkbox:i-agree']
  );
  assert.throws(() => new NativeTask({ ...nativeTask('gtk3-counter', '/tmp/none.json'), capOrder: 'random' as CapOrder }));
});
