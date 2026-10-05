// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

function sentinelRenderer() {
  const listeners = new Map();
  const ipcListeners = new Map();
  const pending = [];
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'preload.js'), 'utf8'), {
    require: () => ({
      contextBridge: { exposeInMainWorld() {} },
      ipcRenderer: {
        sendSync: () => ({ sentinelMode: true }),
        send: (channel, entry) => pending.push({ channel, entry }),
        on: (channel, handler) => ipcListeners.set(channel, handler),
      },
    }),
    window: { addEventListener: (kind, handler) => listeners.set(kind, handler) },
  });
  return {
    arm: token => ipcListeners.get('cua-e2e-sentinel-arm-setup-click')(null, token),
    heartbeat: () => ipcListeners.get('cua-e2e-sentinel-heartbeat-probe')(),
    input: kind => listeners.get(kind)({ button: 0, clientX: 10, clientY: 20 }),
    pending,
  };
}

test('setup barrier follows the full click on the same journal IPC channel', () => {
  const renderer = sentinelRenderer();
  renderer.arm('first');
  renderer.heartbeat();
  renderer.input('pointerdown');
  renderer.input('pointerup');
  assert.equal(renderer.pending.some(({ entry }) => entry.kind === 'setup-click-drained'), false);
  renderer.input('click');
  assert.deepEqual(renderer.pending.map(({ entry }) => entry.kind), [
    'setup-click-armed', 'heartbeat', 'pointerdown', 'pointerup', 'click', 'setup-click-drained',
  ]);
  assert.ok(renderer.pending.every(({ channel }) => channel === 'cua-e2e-sentinel-event'));
  assert.equal(renderer.pending.at(-1).entry.token, 'first');
  // An action's later pointer input remains observable after setup completes.
  renderer.input('pointerdown');
  assert.equal(renderer.pending.at(-1).entry.kind, 'pointerdown');
});

test('heartbeats, stale clicks, and partial clicks cannot complete a new setup token', () => {
  const renderer = sentinelRenderer();
  renderer.input('pointerdown');
  renderer.input('pointerup');
  renderer.input('click');
  renderer.arm('first');
  renderer.input('pointerdown');
  renderer.arm('second');
  renderer.input('pointerup');
  renderer.input('click');
  renderer.heartbeat();
  assert.equal(renderer.pending.some(({ entry }) => entry.kind === 'setup-click-drained'), false);
  renderer.input('pointerdown');
  renderer.input('pointerup');
  renderer.input('click');
  assert.equal(renderer.pending.at(-1).entry.kind, 'setup-click-drained');
  assert.equal(renderer.pending.at(-1).entry.token, 'second');
});
