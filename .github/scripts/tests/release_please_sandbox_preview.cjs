// Offline: exercise the pinned Release Please engine without a GitHub client.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { buildStrategy } = require('release-please/build/src/factory');
const { parseConventionalCommits } = require('release-please/build/src/commit');
const { TagName } = require('release-please/build/src/util/tag-name');

const root = path.resolve(__dirname, '../../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const manifest = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json')));
const packagePath = 'libs/python/cua-sandbox';
const options = Object.fromEntries(
  Object.entries({ ...config, ...config.packages[packagePath] })
    .map(([key, value]) => [key.replace(/-([a-z])/g, (_, c) => c.toUpperCase()), value])
);

async function preview(message, expectedVersion) {
  const strategy = await buildStrategy({
    ...options,
    path: packagePath,
    targetBranch: 'main',
    github: { repository: { owner: 'example', repo: 'repo', defaultBranch: 'main' } },
  });
  const release = await strategy.buildReleasePullRequest(parseConventionalCommits([
    { sha: 'a'.repeat(40), message, files: [`${packagePath}/cua_sandbox/sandbox.py`] },
  ]), {
    tag: TagName.parse(`sandbox-v${manifest[packagePath]}`),
    sha: 'b'.repeat(40),
    notes: '',
  });
  if (expectedVersion === null) {
    assert.equal(release, undefined);
    return;
  }
  assert.equal(release.version.toString(), expectedVersion);
  assert.match(release.headRefName, /components--sandbox$/);
  assert.match(release.title.toString(), /sandbox/);
  const updates = Object.fromEntries(release.updates.map(update => [
    update.path,
    update.updater.updateContent(fs.readFileSync(path.join(root, update.path), 'utf8')),
  ]));
  assert.deepEqual(Object.keys(updates).sort(), [
    `${packagePath}/CHANGELOG.md`, `${packagePath}/VERSION`,
    `${packagePath}/cua_sandbox/__init__.py`, `${packagePath}/pyproject.toml`,
    `${packagePath}/uv.lock`,
  ].sort());
  assert.equal(updates[`${packagePath}/VERSION`].trim(), expectedVersion);
  assert.ok(updates[`${packagePath}/pyproject.toml`].includes(`version = "${expectedVersion}"`));
  assert.ok(updates[`${packagePath}/cua_sandbox/__init__.py`].includes(`__version__ = "${expectedVersion}"`));
  assert.ok(updates[`${packagePath}/CHANGELOG.md`].includes(`sandbox-v${expectedVersion}`));
  const originalLock = fs.readFileSync(path.join(root, packagePath, 'uv.lock'), 'utf8');
  // Release Please's TOML parser wraps scalars in { value, start, end }.
  // Exercise the filter against that real representation and preserve all dependencies.
  assert.equal(updates[`${packagePath}/uv.lock`], originalLock.replace(
    /(\[\[package\]\]\nname = "cua-sandbox"\nversion = ")[^"]+("\n)/,
    (_, prefix, suffix) => `${prefix}${expectedVersion}${suffix}`,
  ));
  console.log(JSON.stringify({ message, title: release.title.toString(), version: expectedVersion,
    tag: `sandbox-v${expectedVersion}`, files: Object.keys(updates) }));
}

async function main() {
  const [major, minor, patch] = manifest[packagePath].split('.').map(Number);
  await preview('fix(sandbox): correct connection cleanup', `${major}.${minor}.${patch + 1}`);
  await preview('feat(sandbox): add typed desktop access', `${major}.${minor + 1}.0`);
  await preview('feat(sandbox)!: change connection contract',
    major === 0 ? `0.${minor + 1}.0` : `${major + 1}.0.0`);
  await preview('docs(sandbox): clarify setup instructions', null);
}

main().catch(error => { console.error(error); process.exitCode = 1; });
