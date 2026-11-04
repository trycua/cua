import assert from 'node:assert/strict';
import test from 'node:test';

import { MODULES, dropTableColumn, fixFragments, pageAnchors, postProcess } from './typescript-sdk';

test('the H1 is dropped and module links become site routes', () => {
  const out = postProcess(
    '# spaces/transport\n\nSee [a](../spaces.md#startthread), [b](../index-1.md) and [c](#local).\n',
    'spaces/transport.md'
  );
  assert.equal(
    out,
    'See [a](/cua-sdk/reference/typescript/spaces#startthread), [b](/cua-sdk/reference/typescript) and [c](#local).\n'
  );
});

test('the empty Defined in column is removed, escaped pipes survive', () => {
  const table = [
    '| Property | Type | Defined in |',
    '| ------ | ------ | ------ |',
    '| `a` | `string` \\| `undefined` |  |',
    '',
    '| Name | Type |',
    '| ------ | ------ |',
    '| `b` | `number` |',
  ].join('\n');
  assert.equal(
    dropTableColumn(table, 'Defined in'),
    [
      '| Property | Type |',
      '| ------ | ------ |',
      '| `a` | `string` \\| `undefined` |',
      '',
      '| Name | Type |',
      '| ------ | ------ |',
      '| `b` | `number` |',
    ].join('\n')
  );
});

test('word-internal escaped underscores are unescaped', () => {
  assert.equal(
    postProcess('`x` and spacesd\\_not\\_available\n', 'index-1.md'),
    '`x` and spacesd_not_available\n'
  );
});

test('every package entry point has one page', () => {
  assert.deepEqual(
    MODULES.map((m) => m.importPath),
    ['@trycua/cua', '@trycua/cua/spaces', '@trycua/cua/spaces/transport', '@trycua/cua/spaces/host']
  );
});

test('fragment links are pointed at anchors the page really defines', () => {
  const pages = new Map([
    [
      '/cua-sdk/reference/typescript/spaces',
      '### ThreadStatus\n\nSee [a](#threadstatus-1), [b](#gone) and [c](/cua-sdk/reference/typescript#image).\n',
    ],
    ['/cua-sdk/reference/typescript', '### Image\n\n| <a id="property-x"></a> `x` |\n'],
  ]);
  assert.equal(
    fixFragments(pages).get('/cua-sdk/reference/typescript/spaces'),
    '### ThreadStatus\n\nSee [a](#threadstatus), b and [c](/cua-sdk/reference/typescript#image).\n'
  );
  assert.deepEqual(
    [...pageAnchors('## A\n## A\n```\n## not\n```\n| <a id="z"></a> |')],
    ['a', 'a-1', 'z']
  );
});
