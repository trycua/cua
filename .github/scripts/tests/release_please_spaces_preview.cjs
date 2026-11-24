// Offline: exercise the pinned Release Please engine without a GitHub client.
// Cua Spaces releases are driven by the macOS SwiftUI app: a commit under
// apps/cua-spaces-macos must produce a `cua-spaces` release PR that also bumps
// the Tauri app (apps/cua-spaces) and the Electron app
// (apps/cua-spaces-desktop) as extra files. So must a commit under one of its
// additional paths (libs/cua, the web UI, the Electron app), through
// release_please_additional_paths.cjs. A Tauri-only commit must not.
require('../release_please_additional_paths.cjs');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { buildStrategy } = require('release-please/build/src/factory');
const { parseConventionalCommits } = require('release-please/build/src/commit');
const { CommitSplit } = require('release-please/build/src/util/commit-split');
const { TagName } = require('release-please/build/src/util/tag-name');

const root = path.resolve(__dirname, '../../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const manifest = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json')));
const packagePath = 'apps/cua-spaces-macos';
const tauriPath = 'apps/cua-spaces';
const electronPath = 'apps/cua-spaces-desktop';
assert.equal(config.packages[packagePath].component, 'cua-spaces');
assert.equal(config.packages[tauriPath], undefined, 'the Tauri app must not drive releases');
assert.equal(manifest[tauriPath], undefined);
assert.equal(config.packages[electronPath], undefined, 'the Electron app must not drive releases');
assert.equal(manifest[electronPath], undefined);
const options = Object.fromEntries(
  Object.entries({ ...config, ...config.packages[packagePath] })
    .map(([key, value]) => [key.replace(/-([a-z])/g, (_, c) => c.toUpperCase()), value])
);

// The same split Release Please applies to every commit on main.
function split(files) {
  const splitter = new CommitSplit({ packagePaths: Object.keys(config.packages) });
  return Object.keys(splitter.split([{ sha: 'c'.repeat(40), message: 'fix: x', files }]));
}

async function preview(message, files, expectedVersion) {
  const commits = split(files).includes(packagePath)
    ? [{ sha: 'a'.repeat(40), message, files }]
    : [];
  const strategy = await buildStrategy({
    ...options,
    path: packagePath,
    targetBranch: 'main',
    github: { repository: { owner: 'example', repo: 'repo', defaultBranch: 'main' } },
  });
  const release = await strategy.buildReleasePullRequest(parseConventionalCommits(commits), {
    tag: TagName.parse(`cua-spaces-v${manifest[packagePath]}`),
    sha: 'b'.repeat(40),
    notes: '',
  });
  if (expectedVersion === null) {
    assert.equal(release, undefined);
    console.log(JSON.stringify({ message, files, release: null }));
    return;
  }
  assert.equal(release.version.toString(), expectedVersion);
  assert.match(release.headRefName, /components--cua-spaces$/);
  assert.match(release.title.toString(), /cua-spaces/);
  const updates = Object.fromEntries(release.updates.map(update => [
    update.path,
    update.updater.updateContent(fs.readFileSync(path.join(root, update.path), 'utf8')),
  ]));
  assert.deepEqual(Object.keys(updates).sort(), [
    `${packagePath}/CHANGELOG.md`,
    `${packagePath}/VERSION`,
    `${packagePath}/Support/Info.plist`,
    `${tauriPath}/package.json`,
    `${tauriPath}/src-tauri/Cargo.lock`,
    `${tauriPath}/src-tauri/Cargo.toml`,
    `${tauriPath}/src-tauri/tauri.conf.json`,
    `${electronPath}/package.json`,
  ].sort());
  assert.equal(updates[`${packagePath}/VERSION`].trim(), expectedVersion);
  assert.ok(updates[`${packagePath}/CHANGELOG.md`].includes(`cua-spaces-v${expectedVersion}`));
  const plist = fs.readFileSync(path.join(root, packagePath, 'Support/Info.plist'), 'utf8');
  assert.equal(updates[`${packagePath}/Support/Info.plist`], plist.replace(
    /(<key>CFBundleShortVersionString<\/key>\s*<string>)[^<]+(<\/string>)/,
    (_, prefix, suffix) => `${prefix}${expectedVersion}${suffix}`,
  ));
  assert.equal(JSON.parse(updates[`${tauriPath}/package.json`]).version, expectedVersion);
  assert.equal(JSON.parse(updates[`${tauriPath}/src-tauri/tauri.conf.json`]).version, expectedVersion);
  assert.equal(JSON.parse(updates[`${electronPath}/package.json`]).version, expectedVersion);
  assert.ok(updates[`${tauriPath}/src-tauri/Cargo.toml`].includes(`version = "${expectedVersion}"`));
  const lock = fs.readFileSync(path.join(root, tauriPath, 'src-tauri/Cargo.lock'), 'utf8');
  assert.equal(updates[`${tauriPath}/src-tauri/Cargo.lock`], lock.replace(
    /(\[\[package\]\]\nname = "cua-spaces-app"\nversion = ")[^"]+("\n)/,
    (_, prefix, suffix) => `${prefix}${expectedVersion}${suffix}`,
  ));
  console.log(JSON.stringify({ message, files, title: release.title.toString(),
    version: expectedVersion, tag: `cua-spaces-v${expectedVersion}`, updates: Object.keys(updates) }));
}

async function main() {
  const [major, minor, patch] = manifest[packagePath].split('.').map(Number);
  const swift = [`${packagePath}/Sources/CuaSpacesMacKit/Views/MainWindow.swift`];
  await preview('fix(spaces-macos): keep the onboarding window in front', swift,
    `${major}.${minor}.${patch + 1}`);
  await preview('feat(spaces-macos): add a menu bar Space switcher', swift,
    `${major}.${minor + 1}.0`);
  await preview('docs(spaces-macos): clarify setup', swift, null);
  // The Tauri app (not shipped) shares the version but never drives a release.
  await preview('fix(spaces): tauri-only change', [`${tauriPath}/src/main.ts`], null);
  // The shipped Electron app, web UI and app core each cut a patch (#7038, #7062).
  const nextPatch = `${major}.${minor}.${patch + 1}`;
  await preview('fix(spaces): electron-only change', [`${electronPath}/src/main.ts`], nextPatch);
  await preview('fix(spaces): web-only change', ['apps/cua-spaces-web/src/lib/spaces.ts'], nextPatch);
  const core = ['libs/cua/crates/cua-agents/src/runs.rs'];
  await preview('fix(cua-agents): core change', core, nextPatch);
  // The core commit still counts for cua-sdk too, and nothing else moves.
  assert.deepEqual(split(core).sort(), ['apps/cua-spaces-macos', 'libs/cua']);
  assert.deepEqual(split(['libs/cua-spacesd/src/main.rs']), ['libs/cua-spacesd']);
}

main().catch(error => { console.error(error); process.exitCode = 1; });
