import assert from 'node:assert/strict';
import * as path from 'node:path';
import test from 'node:test';
import { readFileFromPath, readFiles } from 'next-validate-link';
import { buildScannedUrls, checkLinks, pathToUrl } from './check-links';

const fixtures = path.join(__dirname, 'fixtures/link-checks');
const docs = path.resolve(__dirname, '..');

async function readFixture(name: string) {
  return readFileFromPath(path.join(fixtures, `${name}.mdx`), () => `/${name}`);
}

test('inventories Fumadocs heading IDs and literal MDX anchors', async () => {
  const file = await readFixture('headings');
  const scanned = await buildScannedUrls([file]);
  assert.deepEqual(scanned.urls.get('/headings')?.hashes, [
    'lume-set',
    'native-wayland-background-keyboard-input-is-focus-bound',
    'run-lume-set-with-care',
    'repeat',
    'repeat-1',
    'repeat-1-1',
    'stable-id',
    'custom-heading',
    'explicit-section',
    'inline-anchor',
  ]);
});

test('valid fragments and existing URL exceptions continue passing', async () => {
  const files = await Promise.all(['headings', 'empty', 'valid'].map(readFixture));
  const scanned = await buildScannedUrls(files);
  assert.deepEqual(scanned.urls.get('/empty')?.hashes, []);
  assert.deepEqual(await checkLinks(files, scanned), []);
});

test('rejects obsolete, missing, and non-rendered fragments with source locations', async () => {
  const files = await Promise.all(['headings', 'empty', 'invalid'].map(readFixture));
  const results = await checkLinks(files, await buildScannedUrls(files));
  assert.equal(results.length, 1);
  assert.equal(results[0].file, path.join(fixtures, 'invalid.mdx'));
  assert.deepEqual(
    results[0].errors.map(({ url, line, column, reason }) => [url, line, column, reason]),
    [
      ['/headings#set', 1, 1, 'invalid-fragment'],
      ['/headings#native-wayland-apps-cant-receive-synthetic-keystrokes', 2, 1, 'invalid-fragment'],
      ['/headings#repeat-2', 3, 1, 'invalid-fragment'],
      ['/headings#custom-heading-stable-id', 4, 1, 'invalid-fragment'],
      ['/headings#fixture-title-is-not-an-anchor', 5, 1, 'invalid-fragment'],
      ['/headings#fenced-heading', 6, 1, 'invalid-fragment'],
      ['/headings#fenced-anchor', 7, 1, 'invalid-fragment'],
      ['/empty#missing', 8, 1, 'invalid-fragment'],
      ['/invalid#missing', 9, 1, 'invalid-fragment'],
      ['/missing-page#section', 10, 1, 'not-found'],
    ]
  );
});

test('maps docs paths consistently for absolute paths and nested index pages', () => {
  assert.equal(pathToUrl('content/docs/index.mdx'), '/');
  assert.equal(
    pathToUrl('content/docs/reference/lume/cli-reference.mdx'),
    '/reference/lume/cli-reference'
  );
  assert.equal(
    pathToUrl(path.join(docs, 'content/docs/reference/lume/index.mdx')),
    '/reference/lume'
  );
});

test('preserves root-page fragments rather than replacing them with an unchecked root', async () => {
  const file = { ...(await readFixture('headings')), url: '/' };
  const scanned = await buildScannedUrls([file]);
  assert.ok(scanned.urls.get('/')?.hashes?.includes('lume-set'));
  const source = {
    path: path.join(fixtures, 'root-links.mdx'),
    url: '/root-links',
    content: '[Good](/#lume-set)\n[Bad](/#missing)',
  };
  const results = await checkLinks([source], scanned);
  assert.deepEqual(
    results[0].errors.map(({ url, reason }) => [url, reason]),
    [['/#missing', 'invalid-fragment']]
  );
});

test('the four repaired source links validate against their current destinations', async () => {
  const files = await readFiles(path.join(docs, 'content/docs/**/*.mdx'), { pathToUrl });
  const scanned = await buildScannedUrls(files);
  const referringPages = files.filter((file) =>
    [
      '/concepts/how-lume-expands-macos-disks',
      '/concepts/the-no-foreground-contract',
      '/reference/cua-driver/action-selection-policy',
    ].includes(file.url!)
  );
  assert.equal(referringPages.length, 3);
  assert.deepEqual(await checkLinks(referringPages, scanned), []);
  for (const file of referringPages) {
    assert.doesNotMatch(
      file.content,
      /cli-reference#set\)|limits#native-wayland-apps-cant-receive-synthetic-keystrokes\)/
    );
  }
});
