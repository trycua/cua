import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { chooseForTask } from './jev_adapter.js';
import {
  GOLDEN,
  encode,
  legacyBuild,
  legacyChoose,
  payloads,
  type Build,
  type Choose,
} from './provider_request_golden.js';
import { FixtureFormTask, fixtureSources } from './tasks.js';

// The golden was recorded before the candidate-source and task-spec refactor
// (RFC #4268 Phase 0), so equality proves the refactor sends byte-identical
// model input and offers identical executable candidates.
async function assertGolden(build: Build, choose: Choose) {
  assert.equal(encode(await payloads(build, choose)), readFileSync(GOLDEN, 'utf8'));
}

const taskBuild: Build = (snapshot, token, visual, captureBoundClick, visualDelivery) =>
  new FixtureFormTask(token).candidates(
    fixtureSources(snapshot, visual, captureBoundClick, visualDelivery)
  );

const taskChoose: Choose = (client, candidates, snapshot, visual, history, token, visualPath) =>
  chooseForTask(
    client as never,
    new FixtureFormTask(token),
    fixtureSources(snapshot, visual, false, 'background', visualPath),
    candidates,
    history
  );

test('core entry points send byte-identical provider requests', async () => {
  await assertGolden(legacyBuild, legacyChoose);
});

test('task spec and sources send byte-identical provider requests', async () => {
  await assertGolden(taskBuild, taskChoose);
});
