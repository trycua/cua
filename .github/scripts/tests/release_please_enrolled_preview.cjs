// Offline: exercise the pinned Release Please engine for the PyPI and npm
// packages that moved from the legacy bump2version workflow (CUA-1531).
// For each package: a fix bumps the patch, a feat the minor, a docs commit
// releases nothing, the tag keeps its legacy format, and only that package's
// version files (plus the lockfiles that record it) change.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { buildStrategy } = require('release-please/build/src/factory');
const { parseConventionalCommits } = require('release-please/build/src/commit');
const { TagName } = require('release-please/build/src/util/tag-name');
const { CommitSplit } = require('release-please/build/src/util/commit-split');

const root = path.resolve(__dirname, '../../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const manifest = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json')));

// path -> [legacy tag prefix, a source file inside it, files the release PR must update]
const PACKAGES = {
  'libs/python/agent': ['agent-v', 'cua_agent/__init__.py',
    ['libs/python/agent/CHANGELOG.md', 'libs/python/agent/pyproject.toml', 'uv.lock', 'libs/cua-bench/uv.lock']],
  'libs/python/core': ['core-v', 'cua_core/__init__.py',
    ['libs/python/core/CHANGELOG.md', 'libs/python/core/pyproject.toml', 'uv.lock', 'libs/cua-bench/uv.lock']],
  'libs/cua-bench': ['bench-v', 'cua_bench/__init__.py',
    ['libs/cua-bench/CHANGELOG.md', 'libs/cua-bench/pyproject.toml', 'libs/cua-bench/uv.lock']],
  'libs/python/bench-ui': ['bench-ui-v', 'bench_ui/__init__.py',
    ['libs/python/bench-ui/CHANGELOG.md', 'libs/python/bench-ui/pyproject.toml', 'uv.lock']],
  'libs/python/som': ['som-v', 'som/__init__.py',
    ['libs/python/som/CHANGELOG.md', 'libs/python/som/pyproject.toml', 'uv.lock']],
  'libs/python/cua-sandbox-apps': ['sandbox-apps-v', 'cua_sandbox_apps/__init__.py',
    ['libs/python/cua-sandbox-apps/CHANGELOG.md', 'libs/python/cua-sandbox-apps/pyproject.toml',
      'libs/python/cua-sandbox-apps/uv.lock']],
  'libs/typescript/core': ['npm-core-v', 'src/index.ts',
    ['libs/typescript/core/CHANGELOG.md', 'libs/typescript/core/package.json']],
  'libs/typescript/playground': ['npm-playground-v', 'src/index.ts',
    ['libs/typescript/playground/CHANGELOG.md', 'libs/typescript/playground/package.json']],
  'libs/cuabot': ['cuabot-v', 'src/index.ts',
    ['libs/cuabot/CHANGELOG.md', 'libs/cuabot/package.json']],
};

function optionsFor(packagePath) {
  const merged = { ...config, ...config.packages[packagePath] };
  delete merged.packages;
  return Object.fromEntries(Object.entries(merged)
    .map(([key, value]) => [key.replace(/-([a-z])/g, (_, c) => c.toUpperCase()), value]));
}

function read(file) {
  const full = path.join(root, file);
  return fs.existsSync(full) ? fs.readFileSync(full, 'utf8') : '';
}

function versionOf(packagePath) {
  const pyproject = read(`${packagePath}/pyproject.toml`);
  if (pyproject) return pyproject.match(/^version = "([^"]+)"$/m)[1];
  return JSON.parse(read(`${packagePath}/package.json`)).version;
}

async function preview(packagePath, message, expectedVersion) {
  const [prefix, sourceFile, expectedFiles] = PACKAGES[packagePath];
  const strategy = await buildStrategy({
    ...optionsFor(packagePath),
    path: packagePath,
    targetBranch: 'main',
    github: {
      repository: { owner: 'example', repo: 'repo', defaultBranch: 'main' },
      // The node and python strategies read package metadata through the client.
      getFileContentsOnBranch: async file => ({ parsedContent: read(file) }),
    },
  });
  const current = manifest[packagePath];
  const release = await strategy.buildReleasePullRequest(parseConventionalCommits([
    { sha: 'a'.repeat(40), message, files: [`${packagePath}/${sourceFile}`] },
  ]), { tag: TagName.parse(`${prefix}${current}`), sha: 'b'.repeat(40), notes: '' });
  if (expectedVersion === null) {
    assert.equal(release, undefined, `${packagePath}: ${message} must not release`);
    return null;
  }
  assert.equal(release.version.toString(), expectedVersion, packagePath);
  const component = config.packages[packagePath].component;
  assert.equal(`${component}-v`, prefix, `${packagePath} keeps its legacy tag prefix`);
  assert.match(release.headRefName, new RegExp(`components--${component}$`));
  const updates = {};
  for (const update of release.updates) {
    const before = read(update.path);
    if (!before && !update.createIfMissing) continue; // optional updaters for absent files
    updates[update.path] = update.updater.updateContent(before);
  }
  const changed = Object.keys(updates).filter(file => updates[file] !== read(file)).sort();
  assert.deepEqual(changed, [...expectedFiles].sort(), `${packagePath} release PR files`);
  for (const file of changed.filter(f => !f.endsWith('CHANGELOG.md'))) {
    assert.ok(updates[file].includes(expectedVersion), `${file} carries ${expectedVersion}`);
    // Only the package's own version moves: no other line changes.
    const before = read(file).split('\n');
    const after = updates[file].split('\n');
    const diff = after.filter((line, i) => line !== before[i]);
    assert.ok(diff.length >= 1 && diff.length <= 2, `${file} changes ${diff.length} lines`);
  }
  assert.ok(updates[`${packagePath}/CHANGELOG.md`].includes(`${prefix}${expectedVersion}`));
  return { path: packagePath, message, tag: `${prefix}${expectedVersion}`, files: changed };
}

function bumped(version, kind) {
  const [major, minor, patch] = version.split('.').map(Number);
  if (kind === 'patch') return `${major}.${minor}.${patch + 1}`;
  if (kind === 'minor' || major === 0) return `${major}.${minor + 1}.0`;
  return `${major + 1}.0.0`;
}

async function main() {
  // A commit in a sibling directory that shares a name prefix must not leak.
  const split = new CommitSplit({ packagePaths: Object.keys(config.packages) });
  const leaked = split.split([
    { sha: 'c'.repeat(40), message: 'fix: x', files: ['libs/cua-bench-rl/a.py'] },
    { sha: 'd'.repeat(40), message: 'fix: y', files: ['libs/python/cua-sandbox/z.py'] },
  ]);
  assert.equal(leaked['libs/cua-bench'], undefined, 'libs/cua-bench-rl must not bump cua-bench');
  assert.equal(leaked['libs/python/cua-sandbox-apps'], undefined,
    'libs/python/cua-sandbox must not bump sandbox-apps');

  for (const packagePath of Object.keys(PACKAGES)) {
    assert.ok(config.packages[packagePath], `${packagePath} is enrolled`);
    const current = manifest[packagePath];
    assert.equal(current, versionOf(packagePath), `${packagePath} manifest matches its source`);
    const results = [
      await preview(packagePath, 'fix: correct a behavior', bumped(current, 'patch')),
      await preview(packagePath, 'feat: add a capability', bumped(current, 'minor')),
      await preview(packagePath, 'feat!: change a contract', bumped(current, 'major')),
      await preview(packagePath, 'docs: clarify setup', null),
    ];
    for (const result of results.filter(Boolean)) console.log(JSON.stringify(result));
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
