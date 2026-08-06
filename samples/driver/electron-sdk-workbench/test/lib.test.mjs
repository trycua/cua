import assert from 'node:assert/strict';
import test from 'node:test';

import { assertAllowedTool, parseArgumentsJson, validateSessionRequest } from '../dist/lib.mjs';

test('the renderer can invoke only curated SDK tools', () => {
  assert.doesNotThrow(() => assertAllowedTool('list_windows'));
  assert.throws(() => assertAllowedTool('kill_app'), /Unsupported tool/);
});

test('tool arguments must be a JSON object', () => {
  assert.deepEqual(parseArgumentsJson('{"pid":42}'), { pid: 42 });
  assert.throws(() => parseArgumentsJson('[1,2,3]'), /JSON object/);
  assert.throws(() => parseArgumentsJson('not json'), /valid JSON/);
});

test('bounded sessions require a manifest', () => {
  assert.deepEqual(validateSessionRequest({ name: 'demo', mode: 'standard' }), {
    name: 'demo',
    mode: 'standard',
  });
  assert.throws(
    () => validateSessionRequest({ name: 'demo', mode: 'bounded' }),
    /manifest path/,
  );
});
