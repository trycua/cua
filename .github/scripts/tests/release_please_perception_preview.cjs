// Offline: exercise the candidate-only Perception stream with pinned Release Please.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { buildStrategy } = require('release-please/build/src/factory');
const { parseConventionalCommits } = require('release-please/build/src/commit');
const { CommitExclude } = require('release-please/build/src/util/commit-exclude');
const { CommitSplit } = require('release-please/build/src/util/commit-split');
const { TagName } = require('release-please/build/src/util/tag-name');

const root = path.resolve(__dirname, '../../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const manifest = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json')));
const perceptionPath = 'libs/cua-driver/rust/crates/cua-perception';
const driverPath = 'libs/cua-driver';
const perceptionConfig = config.packages[perceptionPath];
const releaseConfigs = Object.fromEntries(Object.entries(config.packages).map(
  ([packagePath, packageConfig]) => [packagePath, {
    excludePaths: packageConfig['exclude-paths'],
  }],
));
const commitSplit = new CommitSplit({packagePaths: Object.keys(config.packages)});
const commitExclude = new CommitExclude(releaseConfigs);

function commit(message, files, shaByte) {
  return { sha: shaByte.repeat(40), message, files };
}

async function main() {
  const workerCommit = commit('feat(cua-perception): add offline visual parsing', [
    'libs/cua-driver/rust/crates/cua-perception/src/main.rs',
  ], 'a');
  const lumeCommit = commit('feat(lume): add guest support', [
    'libs/lume/Sources/LumeCLI/main.swift',
  ], 'b');
  const driverCommit = commit('feat(cua-driver): add input support', [
    'libs/cua-driver/rust/crates/cua-driver/src/main.rs',
  ], 'c');
  const topLevelCommit = commit('docs: refresh repository readme', ['README.md'], 'd');
  const filtered = commitExclude.excludeCommits(commitSplit.split([
    workerCommit,
    lumeCommit,
    driverCommit,
    topLevelCommit,
  ]));
  assert.deepEqual(filtered[perceptionPath], [workerCommit]);
  assert.deepEqual(filtered[driverPath], [driverCommit]);
  assert.equal(filtered[perceptionPath].includes(lumeCommit), false);
  assert.equal(filtered[perceptionPath].includes(driverCommit), false);
  assert.equal(filtered[perceptionPath].includes(topLevelCommit), false);

  const exampleOnly = commit('docs(cua-driver): update example', [
    'libs/cua-driver/examples/agent-sdks/README.md',
  ], 'e');
  assert.deepEqual(
    commitExclude.excludeCommits(commitSplit.split([exampleOnly]))[driverPath],
    [],
  );

  const options = Object.fromEntries(
    Object.entries({ ...config, ...perceptionConfig })
      .map(([key, value]) => [key.replace(/-([a-z])/g, (_, c) => c.toUpperCase()), value])
  );
  const strategy = await buildStrategy({
    ...options,
    path: perceptionPath,
    targetBranch: 'main',
    github: { repository: { owner: 'example', repo: 'repo', defaultBranch: 'main' } },
  });
  const release = await strategy.buildReleasePullRequest(
    parseConventionalCommits(filtered[perceptionPath]),
    {
      tag: TagName.parse(`cua-perception-v${manifest[perceptionPath]}`),
      sha: 'f'.repeat(40),
      notes: '',
    },
  );
  assert.notEqual(release, undefined);
  const [major, minor] = manifest[perceptionPath].split('.').map(Number);
  assert.equal(release.version.toString(), `${major}.${minor + 1}.0`);
  assert.equal(perceptionConfig['skip-github-release'], true);
  assert.equal(perceptionConfig.component, 'cua-perception');
  assert.equal(perceptionConfig['version-file'], 'VERSION');
  assert.equal(perceptionConfig['changelog-path'], 'CHANGELOG.md');
  assert.deepEqual(
    release.updates.map(update => update.path).sort(),
    [
      'libs/cua-driver/rust/crates/cua-perception/CHANGELOG.md',
      'libs/cua-driver/rust/crates/cua-perception/Cargo.toml',
      'libs/cua-driver/rust/crates/cua-perception/tests/fixtures/parse-response.json',
      'libs/cua-driver/rust/crates/cua-perception/VERSION',
    ].sort(),
  );
  assert.equal(release.updates.some(update => update.path.includes('..')), false);

  const sharedCargoCommit = commit('feat(cua-perception): update protocol', [
    'libs/cua-driver/rust/crates/cua-perception/src/main.rs',
    'libs/cua-driver/rust/Cargo.toml',
    'libs/cua-driver/rust/Cargo.lock',
  ], '1');
  const shared = commitExclude.excludeCommits(commitSplit.split([sharedCargoCommit]));
  assert.equal(shared[driverPath].length, 1);
  assert.equal(shared[perceptionPath].length, 1);

  console.log('release-please 17.3.0 scopes Perception and stages only crate-local updates');
}

main().catch(error => { console.error(error); process.exitCode = 1; });
