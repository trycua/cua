import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import { buildVisualCandidates } from './visual-adapter.js';

test('frozen visual_regions_v1 cases match the Python candidate contract', async () => {
  const fixtureUrl = new URL('../fixtures/visual_regions_v1.json', import.meta.url);
  const fixture = JSON.parse(await readFile(fixtureUrl, 'utf8')) as { cases: Record<string, any>[] };
  for (const item of fixture.cases) {
    const candidates = buildVisualCandidates(item.observation, item.visual_regions);
    assert.deepEqual(candidates.map((candidate) => candidate.id), item.expected, item.name);
    const executable = candidates.filter((candidate) => candidate.tool);
    if (item.expected_arguments) {
      assert.equal(executable.length, 1, item.name);
      assert.deepEqual(executable[0].arguments, item.expected_arguments, item.name);
    } else {
      assert.deepEqual(executable, [], item.name);
    }
  }
});
