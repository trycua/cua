// Offline: a merged release PR must tag only its own component.
//
// Runs the pinned Release Please engine (Manifest.buildReleases) on one merged
// PR per configured component, and on the edited cua-spaces 0.20.0 body that
// tagged 13 other components on 3 Nov 2026 (#6350).
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { PullRequestBody } = require('release-please/build/src/util/pull-request-body');
const { PullRequestTitle } = require('release-please/build/src/util/pull-request-title');
const { BranchName } = require('release-please/build/src/util/branch-name');
const { Version } = require('release-please/build/src/version');
const guard = require('../release_please_tag_guard.cjs');

const root = path.resolve(__dirname, '../../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const fixture = fs.readFileSync(path.join(__dirname, 'fixtures/release-pr-6350-body.md'), 'utf8');

function releasePr(component, version, notes) {
  const body = new PullRequestBody([{ component, version: Version.parse(version), notes }]);
  return {
    number: 1,
    title: PullRequestTitle.ofComponentTargetBranchVersion(component, 'main', Version.parse(version)).toString(),
    headRefName: BranchName.ofComponentTargetBranch(component, 'main').toString(),
    body: body.toString(),
  };
}

function heading(component, version) {
  return `## [${version}](https://github.com/trycua/cua/compare/${component}-v0.0.1...${component}-v${version}) (2026-11-03)\n\n\n### Bug Fixes\n\n* a fix`;
}

(async () => {
  // A pristine Release Please PR for each component tags that component only
  // (Perception skips GitHub releases, so it tags nothing here).
  for (const [pkgPath, pkg] of Object.entries(config.packages)) {
    const pr = releasePr(pkg.component, '9.9.9', heading(pkg.component, '9.9.9'));
    const result = await guard.check(pr, root);
    const expected = pkg['skip-github-release'] ? [] : [pkgPath];
    assert.deepEqual(result.releases.map(r => r.path), expected, `${pkg.component}: ${JSON.stringify(result)}`);
    assert.ok(result.ok, pkg.component);
  }

  // Notes added above the version heading: the engine tags every component.
  const edited = {
    number: 6350,
    title: 'chore(main): release cua-spaces 0.20.0',
    headRefName: 'release-please--branches--main--components--cua-spaces',
    body: fixture,
  };
  const before = await guard.check(edited, root);
  assert.equal(before.ok, false);
  assert.ok(before.stray.length > 1, JSON.stringify(before));
  assert.ok(before.stray.some(r => r.tag === 'cua-sdk-v0.20.0'));

  // The repaired body keeps every line and tags only cua-spaces.
  const repaired = guard.repairBody(fixture);
  assert.notEqual(repaired, null);
  const lines = s => s.split('\n').filter(l => l.trim()).sort();
  assert.deepEqual(lines(repaired), lines(fixture.trim()));
  const after = await guard.check({ ...edited, body: repaired }, root);
  assert.ok(after.ok, JSON.stringify(after));
  assert.deepEqual(after.releases.map(r => r.tag), ['cua-spaces-v0.20.0']);

  // A pristine body is left as is, and a body with no version heading is not
  // guessed at.
  const pristine = releasePr('cua-sdk', '0.11.0', heading('cua-sdk', '0.11.0'));
  assert.equal(guard.repairBody(pristine.body), pristine.body);
  assert.equal(guard.repairBody(pristine.body.replace(/^## \[0\.11\.0\].*$/m, '## Notes')), null);
  console.log('release PRs tag only their own component');
})().catch(error => {
  console.error(error);
  process.exit(1);
});
