import assert from 'node:assert/strict';
import test from 'node:test';

import { decisionTimingFields } from './run.js';

test('step timing field names and values are stable', () => {
  assert.deepEqual(
    decisionTimingFields({
      decisionMs: 14.5,
      semanticObserveMs: 4,
      visualObserveMs: 3,
      candidateBuildMs: 2,
      providerDecisionMs: 5,
    }),
    {
      decision_ms: 14.5,
      semantic_observe_ms: 4,
      visual_observe_ms: 3,
      candidate_build_ms: 2,
      provider_decision_ms: 5,
    }
  );
});
