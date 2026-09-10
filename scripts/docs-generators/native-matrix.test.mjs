import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { selectNativeMatrix } from './native-matrix.mjs';

const driver = ['cua-driver/linux', 'cua-driver/macos', 'cua-driver/windows'];
const all = [...driver, 'lume/macos'];
const selected = (files) =>
  selectNativeMatrix(files).include.map((row) => `${row.library}/${row.platform}`);

test('unrelated and curated changes allocate no native runners', () => {
  assert.deepEqual(selected([]), []);
  assert.deepEqual(
    selected([
      'README.md',
      'docs/content/docs/reference/cua-driver/macos-permissions.mdx',
      'docs/content/docs/reference/cua-driver/mcp-tool-notes.mdx',
      'libs/python/agent/agent/main.py',
    ]),
    []
  );
});

test('platform source, manifests, and build scripts select their owner', () => {
  for (const platform of ['linux', 'macos', 'windows']) {
    for (const input of ['src/lib.rs', 'Cargo.toml', 'build.rs']) {
      assert.deepEqual(selected([`libs/cua-driver/rust/crates/platform-${platform}/${input}`]), [
        `cua-driver/${platform}`,
      ]);
    }
  }
});

test('shared source and dependency/build inputs select all driver hosts', () => {
  for (const input of [
    'libs/cua-driver/rust/crates/cua-driver-core/src/lib.rs',
    'libs/cua-driver/rust/crates/cua-driver-contract/Cargo.toml',
    'libs/cua-driver/rust/crates/cua-driver/build.rs',
    'libs/cua-driver/rust/crates/new-shared-crate/src/lib.rs',
    'libs/cua-driver/rust/Cargo.lock',
    'libs/cua-driver/rust/Cargo.toml',
    'libs/cua-driver/rust/.cargo/config.toml',
    '.cargo/config.toml',
    'rust-toolchain.toml',
  ])
    assert.deepEqual(selected([input]), driver);
});

test('generated references select configured owners; shared CLI checks all hosts', () => {
  for (const [file, expected] of [
    ['mcp-tools.mdx', ['cua-driver/macos']],
    ['mcp-tools-linux.mdx', ['cua-driver/linux']],
    ['mcp-tools-windows.mdx', ['cua-driver/windows']],
    ['cli-reference.mdx', driver],
  ])
    assert.deepEqual(selected([`docs/content/docs/reference/cua-driver/${file}`]), expected);
});

test('lume source, build configuration, and reference changes stay on lume', () => {
  for (const input of [
    'libs/lume/src/Commands/List.swift',
    'libs/lume/Package.swift',
    'libs/lume/Package.resolved',
    'docs/content/docs/reference/lume/http-api.mdx',
    'scripts/docs-generators/lume.ts',
  ])
    assert.deepEqual(selected([input]), ['lume/macos']);
});

test('generator dependencies and workflow changes conservatively fan out', () => {
  for (const input of [
    'scripts/docs-generators/runner.ts',
    'scripts/docs-generators/config.json',
    'scripts/docs-generators/native-matrix.mjs',
    '.github/workflows/ci-check-docs.yml',
    '.gitattributes',
    'docs/package.json',
    'docs/pnpm-lock.yaml',
    'package.json',
  ])
    assert.deepEqual(selected([input]), all);
  assert.deepEqual(selected(['scripts/docs-generators/cua-driver-policy.test.ts']), driver);
});

test('mixed and duplicate inputs produce a deterministic union', () => {
  assert.deepEqual(
    selected([
      'libs/lume/Package.swift',
      'libs/cua-driver/rust/crates/platform-windows/Cargo.toml',
      'libs/cua-driver/rust/crates/platform-macos/src/lib.rs',
      'libs/cua-driver/rust/crates/platform-windows/Cargo.toml',
    ]),
    ['cua-driver/macos', 'cua-driver/windows', 'lume/macos']
  );
});

test('diff against a stacked parent includes both rename paths but excludes parent changes', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'native-matrix-git-'));
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' });
  try {
    git('init', '-q');
    git('config', 'user.name', 'Routing Test');
    git('config', 'user.email', 'routing@example.invalid');
    const oldPath = 'libs/cua-driver/rust/crates/platform-linux/src/old.rs';
    const newPath = 'libs/cua-driver/rust/crates/platform-macos/src/new.rs';
    fs.mkdirSync(path.dirname(path.join(dir, oldPath)), { recursive: true });
    fs.writeFileSync(path.join(dir, oldPath), 'fixture');
    git('add', '.');
    git(
      '-c',
      'core.hooksPath=/dev/null',
      '-c',
      'commit.gpgsign=false',
      'commit',
      '-qm',
      'baseline'
    );
    fs.mkdirSync(path.join(dir, 'libs/lume/src'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'libs/lume/src/Parent.swift'), 'parent');
    git('add', '.');
    git('-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'parent');
    const base = git('rev-parse', 'HEAD').trim();
    fs.mkdirSync(path.dirname(path.join(dir, newPath)), { recursive: true });
    fs.renameSync(path.join(dir, oldPath), path.join(dir, newPath));
    git('add', '-A');
    git('-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'child');
    const files = git('diff', '--name-only', '--no-renames', '-z', base, 'HEAD')
      .split('\0')
      .filter(Boolean);
    assert.deepEqual(files.sort(), [oldPath, newPath].sort());
    assert.deepEqual(selected(files), ['cua-driver/linux', 'cua-driver/macos']);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('required-check shell accepts only successful selected jobs or intentional skips', () => {
  const workflow = fs.readFileSync(
    new URL('../../.github/workflows/ci-check-docs.yml', import.meta.url),
    'utf8'
  );
  const gate = workflow.split('  check-docs-sync:')[1].split('        run: |\n')[1];
  assert.ok(gate);
  for (const [plan, work, native, succeeds] of [
    ['success', 'true', 'success', true],
    ['success', 'false', 'skipped', true],
    ['failure', 'false', 'skipped', false],
    ['cancelled', '', 'skipped', false],
    ['success', 'true', 'failure', false],
    ['success', 'true', 'cancelled', false],
    ['success', 'true', 'skipped', false],
    ['success', '', 'skipped', false],
    ['success', 'false', 'success', false],
  ]) {
    const run = () =>
      execFileSync('bash', ['-e', '-c', gate], {
        env: { ...process.env, PLAN_RESULT: plan, HAS_WORK: work, NATIVE_RESULT: native },
        stdio: 'pipe',
      });
    if (succeeds) assert.doesNotThrow(run);
    else assert.throws(run);
  }
});

test('CLI reads NUL-delimited paths and reports intentional empty selections', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'native-matrix-'));
  try {
    const input = path.join(dir, 'changes');
    const script = fileURLToPath(new URL('./native-matrix.mjs', import.meta.url));
    fs.writeFileSync(input, 'unrelated\nfile.md\0');
    assert.equal(
      execFileSync(process.execPath, [script, input], { encoding: 'utf8' }),
      'matrix={"include":[]}\nhas_work=false\n'
    );
    fs.writeFileSync(input, 'libs/cua-driver/rust/crates/platform-macos/src/deleted.rs\0');
    const result = execFileSync(process.execPath, [script, input], { encoding: 'utf8' });
    assert.match(result, /has_work=true/);
    assert.deepEqual(
      JSON.parse(result.split('\n')[0].slice('matrix='.length)),
      selectNativeMatrix(['libs/cua-driver/rust/crates/platform-macos/src/deleted.rs'])
    );
    assert.throws(() =>
      execFileSync(process.execPath, [script, path.join(dir, 'missing')], { stdio: 'pipe' })
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
