import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  GOLDEN,
  encode,
  legacyBuild,
  legacyChoose,
  payloads,
  type Build,
  type Choose,
} from './provider_request_golden.js';

async function assertGolden(build: Build, choose: Choose) {
  assert.equal(encode(await payloads(build, choose)), readFileSync(GOLDEN, 'utf8'));
}

test('core entry points send byte-identical provider requests', async () => {
  await assertGolden(legacyBuild, legacyChoose);
});
