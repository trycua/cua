import assert from 'node:assert/strict';
import test from 'node:test';

import { NoProgressGuard, observedProgressScore } from './no_progress.js';

test('repeated reobserve stops at the bounded limit', () => {
  const guard = new NoProgressGuard('appkit-counter');
  assert.equal(guard.beforeStep({ counter: 0 }), undefined);
  for (let index = 0; index < 2; index += 1) {
    guard.note('reobserve', 'reobserve');
    assert.equal(guard.beforeStep({ counter: 0 }), undefined);
  }
  guard.note('reobserve', 'reobserve');
  assert.deepEqual(guard.beforeStep({ counter: 0 }), { pattern: 'reobserve', streak: 3 });
});

test('proven app progress resets the streak', () => {
  const guard = new NoProgressGuard('appkit-counter');
  guard.beforeStep({ counter: 0 });
  guard.note('reobserve', 'reobserve');
  guard.beforeStep({ counter: 0 });
  guard.note('performed', 'ax:button:increment');
  assert.equal(guard.beforeStep({ counter: 1 }), undefined);
  guard.note('reobserve', 'reobserve');
  assert.equal(guard.beforeStep({ counter: 1 }), undefined);
});

test('same delivered candidate without app progress stops', () => {
  const guard = new NoProgressGuard('appkit-counter');
  guard.beforeStep({ counter: 1 });
  for (let index = 0; index < 2; index += 1) {
    guard.note('performed', 'ax:button:increment');
    assert.equal(guard.beforeStep({ counter: 1 }), undefined);
  }
  guard.note('performed', 'ax:button:increment:foreground');
  assert.deepEqual(guard.beforeStep({ counter: 1 }), { pattern: 'same_candidate', streak: 3 });
});

test('mixed stale/refused/reobserve recovery stops without replay', () => {
  const guard = new NoProgressGuard('appkit-counter');
  guard.beforeStep({ counter: 0 });
  guard.note('stale', 'ax:button:increment');
  guard.beforeStep({ counter: 0 });
  guard.note('refused', 'ax:button:increment');
  guard.beforeStep({ counter: 0 });
  guard.note('reobserve', 'reobserve');
  assert.deepEqual(guard.beforeStep({ counter: 0 }), { pattern: 'recovery', streak: 3 });
});

test('repeated unobservable save-note action stops', () => {
  const guard = new NoProgressGuard('appkit-save-note');
  guard.beforeStep({ note_saved: null });
  for (let index = 0; index < 2; index += 1) {
    guard.note('performed', 'ax:text_input:note:set:note');
    assert.equal(guard.beforeStep({ note_saved: null }), undefined);
  }
  guard.note('performed', 'ax:text_input:note:set:note');
  assert.deepEqual(guard.beforeStep({ note_saved: null }), { pattern: 'same_candidate', streak: 3 });
});

test('unobservable candidate change starts a fresh evidence window', () => {
  const guard = new NoProgressGuard('appkit-save-note');
  guard.beforeStep({ note_saved: null });
  for (let index = 0; index < 2; index += 1) {
    guard.note('performed', 'ax:text_input:note:set:note');
    assert.equal(guard.beforeStep({ note_saved: null }), undefined);
  }
  guard.note('performed', 'ax:button:save-note');
  assert.equal(guard.beforeStep({ note_saved: null }), undefined);
  guard.note('performed', 'ax:button:save-note');
  assert.equal(guard.beforeStep({ note_saved: null }), undefined);
});

test('unobservable recovery plus dispatch remains bounded', () => {
  for (const recovery of ['stale', 'refused'] as const) {
    const guard = new NoProgressGuard('appkit-save-note');
    guard.beforeStep({ note_saved: null });
    for (const kind of [recovery, 'performed'] as const) {
      guard.note(kind, 'ax:button:save-note');
      assert.equal(guard.beforeStep({ note_saved: null }), undefined);
    }
    guard.note(recovery, 'ax:button:save-note');
    assert.deepEqual(guard.beforeStep({ note_saved: null }), { pattern: 'recovery', streak: 3 });
  }
});

test('unobservable new candidate after recovery resets', () => {
  const guard = new NoProgressGuard('appkit-save-note');
  guard.beforeStep({ note_saved: null });
  guard.note('performed', 'ax:text_input:note:set:note');
  guard.beforeStep({ note_saved: null });
  guard.note('refused', 'ax:button:save-note');
  guard.beforeStep({ note_saved: null });
  guard.note('performed', 'ax:button:save-note:foreground');
  assert.equal(guard.beforeStep({ note_saved: null }), undefined);
});

test('observed scores are task-local', () => {
  assert.equal(observedProgressScore('appkit-counter', { counter: 2 }), 2);
  assert.equal(observedProgressScore('gtk3-choose-size', { size: 'large', agreed: false }), 1);
  assert.equal(observedProgressScore('wpf-save-note', { note_saved: null }), undefined);
});
