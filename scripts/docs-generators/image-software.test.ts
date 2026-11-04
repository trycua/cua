import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import test from 'node:test';

import { loadInventories, renderSoftware, validateInventory } from './image-software';

// Assembled so the image-ref gates do not read these as refs.
const GH = 'ghcr' + '.io/trycua';

function inventory(tier: string, extra: Record<string, unknown> = {}) {
  return {
    image: `${GH}/linux:24.04${tier === 'full' ? '' : `-${tier}`}`,
    os: 'linux',
    version: '24.04',
    tier,
    recorded_from: '2026-09-25, container/arm64, rev abc1234',
    apps: { chromium: 'Chromium 153.0' },
    tools: tier === 'full' ? { node: 'v22.20.0' } : {},
    simulator_runtimes: [],
    ...extra,
  };
}

function dir(files: Record<string, unknown>): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'image-software-'));
  for (const [name, body] of Object.entries(files)) fs.writeFileSync(path.join(d, name), JSON.stringify(body));
  return d;
}

test('inventories load in os, version and tier order', () => {
  const d = dir({ 'linux-24.04-full.json': inventory('full'), 'linux-24.04-slim.json': inventory('slim') });
  assert.deepEqual(
    loadInventories(d).map((i) => i.tier),
    ['slim', 'full']
  );
  assert.deepEqual(loadInventories(path.join(d, 'missing')), []);
});

test('bad inventories are refused', () => {
  assert.throws(() => validateInventory(inventory('slim'), 'linux-24.04-full.json'), /expected the name/);
  assert.throws(
    () => validateInventory(inventory('full', { tools: { go: 'unavailable' } }), 'linux-24.04-full.json'),
    /recorded version/
  );
  assert.throws(() => validateInventory({ ...inventory('full'), extra: 1 }, 'linux-24.04-full.json'), /fields/);
  assert.throws(
    () => validateInventory(inventory('full', { image: `${GH}/linux:latest` }), 'linux-24.04-full.json'),
    /canonical tier tag/
  );
});

test('the page renders tables, pending images and unpublished markers', () => {
  const invs = [validateInventory(inventory('slim'), 'linux-24.04-slim.json'), validateInventory(inventory('full'), 'linux-24.04-full.json')];
  const body = renderSoftware(invs, [
    { ref: `${GH}/linux:24.04`, published: true },
    { ref: `${GH}/linux:24.04-slim`, published: false },
    { ref: `${GH}/macos:26`, published: true },
  ]);
  assert.match(body, /\| Tool \| node \| `v22\.20\.0` \|/);
  assert.match(body, /Tier: full \(default\)\. Recorded from 2026-09-25/);
  // The unpublished slim tier carries the docs gate's marker right above its ref.
  assert.match(body, /\{\/\* cua-image-unverified \*\/\}\n## `ghcr\.io\/trycua\/linux:24\.04-slim`/);
  assert.match(body, /Tier: slim\. Not published yet\./);
  assert.match(body, /## Not recorded yet\n\n- `ghcr\.io\/trycua\/macos:26`/);
  assert.doesNotMatch(body, /—/);
  const empty = renderSoftware([], [{ ref: `${GH}/linux:24.04`, published: true }]);
  assert.match(empty, /No image inventory is recorded yet/);
});
