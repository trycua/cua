import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test, { type TestContext } from 'node:test';

function runnerFixture(t: TestContext) {
  const directory = mkdtempSync(join(tmpdir(), 'cua-docs-runner-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = join(directory, 'scripts', 'docs-generators');
  cpSync(__dirname, source, { recursive: true });
  const docs = join(directory, 'docs');
  mkdirSync(docs);
  symlinkSync(
    resolve(__dirname, '../../docs/node_modules'),
    join(docs, 'node_modules'),
    'junction'
  );
  copyFileSync(resolve(__dirname, '../../docs/pnpm-lock.yaml'), join(docs, 'pnpm-lock.yaml'));
  writeFileSync(
    join(directory, 'fixture.mjs'),
    'console.log("FIXTURE_GENERATOR_RAN", JSON.stringify(process.argv.slice(2)));\n'
  );
  writeFileSync(
    join(source, 'config.json'),
    JSON.stringify({
      description: 'Command regression fixture',
      generators: {
        fixture: {
          name: 'Fixture',
          language: 'JavaScript',
          sourcePath: 'fixture',
          docsOutputPath: 'docs',
          generatorScript: 'fixture.mjs',
          watchPaths: ['fixture/**'],
          buildCommand: null,
          buildDirectory: '.',
          extractionMethod: 'fixture',
          outputs: [],
          enabled: true,
        },
      },
    })
  );
  return (args: string[]) => {
    const result = spawnSync(
      process.execPath,
      [...process.execArgv, join(source, 'runner.ts'), ...args],
      {
        cwd: directory,
        env: { ...process.env, NODE_TEST_CONTEXT: undefined },
        encoding: 'utf8',
        timeout: 60_000,
      }
    );
    assert.ifError(result.error);
    return result;
  };
}

test('help prints usage without starting a generator', (t) => {
  const run = runnerFixture(t);
  const ordinary = run([]);
  assert.equal(ordinary.status, 0, ordinary.stdout + ordinary.stderr);
  assert.match(ordinary.stdout, /FIXTURE_GENERATOR_RAN \[\]/);
  const help = run(['--help']);
  assert.equal(help.status, 0, help.stdout + help.stderr);
  assert.match(help.stdout, /Usage:/);
  assert.match(help.stdout, /--library/);
  assert.doesNotMatch(help.stdout + help.stderr, /FIXTURE_GENERATOR_RAN/);
});

test('unknown options fail without starting a generator', (t) => {
  const run = runnerFixture(t);
  const invalid = run(['--not-a-real-option']);
  assert.equal(invalid.status, 1, invalid.stdout + invalid.stderr);
  assert.match(invalid.stderr, /Unknown option.*--not-a-real-option/);
  assert.doesNotMatch(invalid.stdout + invalid.stderr, /FIXTURE_GENERATOR_RAN/);
});

test('an empty library name does not fall back to running all generators', (t) => {
  const run = runnerFixture(t);
  const invalid = run(['--library', '']);
  assert.equal(invalid.status, 1, invalid.stdout + invalid.stderr);
  assert.match(invalid.stderr, /Unknown library/);
  assert.doesNotMatch(invalid.stdout + invalid.stderr, /FIXTURE_GENERATOR_RAN/);
});

test('an empty changed-files path does not fall back to running generators', (t) => {
  const run = runnerFixture(t);
  const invalid = run(['--changed-files-file', '']);
  assert.equal(invalid.status, 1, invalid.stdout + invalid.stderr);
  assert.match(invalid.stderr, /Changed-files input not found/);
  assert.doesNotMatch(invalid.stdout + invalid.stderr, /FIXTURE_GENERATOR_RAN/);
});

test('both check spellings still reach the selected generator', (t) => {
  const run = runnerFixture(t);
  for (const flag of ['--check', '--check-only']) {
    const checked = run(['--library', 'fixture', flag]);
    assert.equal(checked.status, 0, checked.stdout + checked.stderr);
    assert.match(checked.stdout, /FIXTURE_GENERATOR_RAN \["--check"\]/);
  }
});

test('list and changed-file selection report without running generators', (t) => {
  const run = runnerFixture(t);
  const listed = run(['--list']);
  assert.equal(listed.status, 0, listed.stdout + listed.stderr);
  assert.match(listed.stdout, /Fixture \(JavaScript\)/);
  assert.doesNotMatch(listed.stdout + listed.stderr, /FIXTURE_GENERATOR_RAN/);

  const directory = mkdtempSync(join(tmpdir(), 'cua-docs-changes-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const input = join(directory, 'changed-files.txt');
  writeFileSync(input, 'fixture/example.mjs\n');
  const selected = run(['--changed-files-file', input]);
  assert.equal(selected.status, 0, selected.stdout + selected.stderr);
  assert.equal(selected.stdout.trim(), 'fixture');
});
