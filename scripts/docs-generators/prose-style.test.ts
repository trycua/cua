import assert from 'node:assert/strict';
import test from 'node:test';

import { normalizeEmDashes } from './prose-style';

test('leaves text without em dashes untouched', () => {
  assert.equal(normalizeEmDashes('a - b, c: d'), 'a - b, c: d');
});

test('turns empty table cells into none', () => {
  assert.equal(normalizeEmDashes('| `--x` | String | — | — |'), '| `--x` | String | none | none |');
});

test('turns a pair in one sentence into commas', () => {
  assert.equal(
    normalizeEmDashes('For just opening an app — running or not — call launch_app.'),
    'For just opening an app, running or not, call launch_app.'
  );
});

test('turns a single dash into a colon, or a semicolon after a colon', () => {
  assert.equal(normalizeEmDashes('Unsaved state is lost — prefer quit.'), 'Unsaved state is lost: prefer quit.');
  assert.equal(normalizeEmDashes('Default: "left" — omit it.'), 'Default: "left"; omit it.');
});

test('handles each sentence separately and unspaced dashes', () => {
  assert.equal(
    normalizeEmDashes('One — two. Three — four — five. a—b'),
    'One: two. Three, four, five. a, b'
  );
});

test('never leaves an em dash behind', () => {
  const input = '— lead —\n| — |—|\ntrail —';
  assert.ok(!normalizeEmDashes(input).includes('—'));
});
