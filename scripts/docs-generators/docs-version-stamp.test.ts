/**
 * Generated reference pages carry no release version.
 *
 * A page stamped with `Version: x.y.z` goes stale when a PR regenerates it
 * before a release and merges after: the release sync never saw it, so main's
 * generator check ("Check Documentation Sync", "Reference / cua-sdk / linux")
 * fails for every later PR (#6819, fixed by #7283; the bench pages in #7252).
 * The generated output must not depend on any release version, so a release
 * never makes an untouched page out of sync.
 */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { renderCliReference, type CLIDocumentation, type CliReference } from './lib/cli-mdx';
import { GENERATED_MARKER } from './lib/mdx';
import { renderAll as renderLume, type CLIDocumentation as LumeCli } from './lume';

const ROOT = join(__dirname, '..', '..');
const read = (rel: string) => readFileSync(join(ROOT, rel), 'utf-8').trim();

test('a version bump does not change any rendered page', () => {
  const baseCli: CLIDocumentation = { name: 'demo', version: '1.2.3', abstract: 'A demo CLI', commands: [] } as unknown as CLIDocumentation;
  const ref = (version: string): CliReference =>
    ({ product: 'demo', cli: { ...baseCli, version }, groups: [], generator: 'gen', source: 'src' }) as unknown as CliReference;
  assert.equal(JSON.stringify([...renderCliReference(ref('1.2.3'))]), JSON.stringify([...renderCliReference(ref('9.9.9'))]));

  const lumeSpec: LumeCli = JSON.parse(readFileSync(join(__dirname, 'cli-specs', 'lume.json'), 'utf-8'));
  const lume = (version: string) =>
    [...renderLume({ cli: { ...lumeSpec, version }, api: { base_path: '/lume', version, description: 'API', endpoints: [] } })];
  assert.equal(JSON.stringify(lume('0.7.0')), JSON.stringify(lume('0.8.0')));
});

test('committed generated pages carry no release version', () => {
  const versions = [
    read('libs/cua/VERSION'),
    read('libs/python/cua-sandbox/VERSION'),
    read('libs/cua/typescript/package.json').match(/"version":\s*"([^"]+)"/)![1],
  ];
  const pages = execFileSync('git', ['grep', '-l', GENERATED_MARKER, '--', 'docs/content/docs'], { cwd: ROOT, encoding: 'utf-8' })
    .split('\n')
    .filter(Boolean);
  assert.ok(pages.length > 50, `expected the generated pages, found ${pages.length}`);
  for (const page of pages) {
    const text = readFileSync(join(ROOT, page), 'utf-8');
    assert.ok(!/^\s*Version:/m.test(text), `${page} has a "Version:" banner stamp`);
    for (const v of versions) {
      assert.ok(!new RegExp(`(?<![\\d.])${v.replace(/\./g, '\\.')}(?![\\d]|\\.\\d)`).test(text), `${page} names release ${v}`);
    }
  }
});

test('header templates take no release-version placeholders', () => {
  const dir = join(__dirname, 'headers');
  const walk = (d: string): string[] =>
    readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(d, e.name)) : [join(d, e.name)]));
  for (const file of walk(dir)) {
    assert.ok(!/\{\{\s*\w*[vV]ersion\w*\s*\}\}/.test(readFileSync(file, 'utf-8')), `${file} has a version placeholder`);
  }
});
