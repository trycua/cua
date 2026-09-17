// Offline: exercise the candidate-only Perception stream with pinned Release Please.
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
const perceptionPath = '.github/releases/cua-perception';
const driverPath = 'libs/cua-driver';
const perceptionConfig = config.packages[perceptionPath];
const driverConfig = config.packages[driverPath];
const commitExclude = new CommitExclude({
  [perceptionPath]: { includePaths: perceptionConfig['include-paths'] },
  [driverPath]: { excludePaths: driverConfig['exclude-paths'] },
});

function commit(message, files) {
  return { sha: 'a'.repeat(40), message, files };
}

async function main() {
  const workerCommit = commit('feat(cua-perception): add offline visual parsing', [
    'libs/cua-driver/rust/crates/cua-perception/src/main.rs',
  ]);
  const filtered = commitExclude.excludeCommits({
    [perceptionPath]: [workerCommit],
    [driverPath]: [workerCommit],
  });
  assert.equal(filtered[perceptionPath].length, 1);
  assert.deepEqual(filtered[driverPath], []);
  const exampleOnly = commit('docs(cua-driver): update example', [
    'libs/cua-driver/examples/agent-sdks/README.md',
  ]);
  assert.deepEqual(
    commitExclude.excludeCommits({ [driverPath]: [exampleOnly] })[driverPath],
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
      sha: 'b'.repeat(40),
      notes: '',
    },
  );
  assert.notEqual(release, undefined);
  assert.equal(release.version.toString(), '0.2.0');
  assert.equal(perceptionConfig['skip-github-release'], true);

  const protocolFiles = [
    'libs/cua-driver/rust/crates/cua-perception/src/main.rs',
    'libs/cua-driver/rust/Cargo.toml',
    'libs/cua-driver/rust/Cargo.lock',
  ];
  const sharedCargoCommit = commit('feat(cua-perception): update protocol', protocolFiles);
  const driverShared = commitExclude.excludeCommits({ [driverPath]: [sharedCargoCommit] });
  assert.equal(driverShared[driverPath].length, 1);

  console.log('release-please 17.3.0 opens Perception version PRs without publishing releases');
}

main().catch(error => { console.error(error); process.exitCode = 1; });
