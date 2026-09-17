// Offline: exercise perception exclusions with the pinned Release Please engine.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { buildStrategy } = require('release-please/build/src/factory');
const { parseConventionalCommits } = require('release-please/build/src/commit');
const { CommitExclude } = require('release-please/build/src/util/commit-exclude');
const { TagName } = require('release-please/build/src/util/tag-name');

const root = path.resolve(__dirname, '../../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const manifest = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json')));
const packagePath = 'libs/cua-driver';
const releaseConfig = config.packages[packagePath];
const commitExclude = new CommitExclude({
  [packagePath]: { excludePaths: releaseConfig['exclude-paths'] },
});

function commit(message, files) {
  return { sha: 'a'.repeat(40), message, files };
}

function filter(commits) {
  return commitExclude.excludeCommits({ [packagePath]: commits })[packagePath];
}

async function main() {
  assert.deepEqual(filter([commit('test(cua-perception): fixture', [
    'libs/cua-driver/rust/crates/cua-perception/src/main.rs',
  ])]), []);
  assert.deepEqual(filter([commit('build(cua-perception): inference spike', [
    'libs/cua-driver/experiments/cua-perception-inference/src/main.rs',
    'libs/cua-driver/docs/cua-perception-rust-inference-spike.md',
  ])]), []);

  const protocolFiles = [
    'libs/cua-driver/rust/crates/cua-perception/src/main.rs',
    'libs/cua-driver/rust/Cargo.toml',
    'libs/cua-driver/rust/Cargo.lock',
  ];
  const protocol = filter([commit(
    'test(cua-perception): add protocol fixtures', protocolFiles,
  )]);
  assert.equal(protocol.length, 1);

  const unscopedFeature = filter([commit(
    'feat: add offline visual parsing', protocolFiles,
  )]);
  assert.equal(unscopedFeature.length, 1);
  const revert = filter([commit(
    'revert: restore previous protocol', protocolFiles,
  )]);
  assert.equal(revert.length, 1);

  const options = Object.fromEntries(
    Object.entries({ ...config, ...releaseConfig })
      .map(([key, value]) => [key.replace(/-([a-z])/g, (_, c) => c.toUpperCase()), value])
  );
  const strategy = await buildStrategy({
    ...options,
    path: packagePath,
    targetBranch: 'main',
    github: { repository: { owner: 'example', repo: 'repo', defaultBranch: 'main' } },
  });
  const nonReleasing = await strategy.buildReleasePullRequest(
    parseConventionalCommits(protocol),
    {
      tag: TagName.parse(`cua-driver-rs-v${manifest[packagePath]}`),
      sha: 'b'.repeat(40),
      notes: '',
    },
  );
  assert.equal(nonReleasing, undefined);

  const accidentalDriverRelease = await strategy.buildReleasePullRequest(
    parseConventionalCommits(unscopedFeature),
    {
      tag: TagName.parse(`cua-driver-rs-v${manifest[packagePath]}`),
      sha: 'b'.repeat(40),
      notes: '',
    },
  );
  assert.notEqual(accidentalDriverRelease, undefined);
  assert.equal(accidentalDriverRelease.version.toString(), '0.29.0');

  const mixedDriverCommit = filter([commit('feat: add integrated visual parsing', [
    ...protocolFiles,
    'libs/cua-driver/rust/crates/cua-driver/src/main.rs',
  ])]);
  assert.equal(mixedDriverCommit.length, 1);

  assert.equal(filter([commit('fix(cua-driver): preserve input', [
    'libs/cua-driver/rust/crates/cua-driver/src/main.rs',
    'libs/cua-driver/rust/Cargo.toml',
    'libs/cua-driver/rust/Cargo.lock',
  ])]).length, 1);

  console.log(
    'release-please 17.3.0 exposes exact-file exclusion limits and preserves mixed Driver commits',
  );
}

main().catch(error => { console.error(error); process.exitCode = 1; });
