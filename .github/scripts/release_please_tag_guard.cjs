// Predict, with the pinned Release Please engine, which components a merged
// release PR will tag, and repair a PR body that would tag the wrong ones.
//
// Release Please reads each component's version from the merged PR body. A
// single-component body parses only when its notes block starts with the
// version heading ("## [0.20.0](...)"). Text inserted above that heading makes
// the body parse to zero releases, and Release Please then falls back to the
// PR title's version for every configured component. On 3 Nov 2026 that
// tagged 13 unrelated components v0.20.0 from the cua-spaces 0.20.0 PR (#6350).
//
//   node release_please_tag_guard.cjs check  <pr.json>   prints the components it would tag
//   node release_please_tag_guard.cjs repair <pr.json>   prints a repaired body, or exits 1
//
// pr.json is `gh pr view --json number,title,headRefName,body,mergeCommit`.
// NODE_PATH must point at node_modules holding release-please@17.3.0.
const fs = require('node:fs');
const path = require('node:path');
const { Manifest } = require('release-please/build/src/manifest');
const { BranchName } = require('release-please/build/src/util/branch-name');

const root = path.resolve(__dirname, '../..');
const CONFIG = 'release-please-config.json';
const MANIFEST = '.release-please-manifest.json';
const DELIMITER = '---';
// Release Please 17.3.0's COMPARE_REGEX, matched per line.
const VERSION_HEADING = /^#{2,} \[?\d+\.\d+\.\d+[^\]]*\]?/;

function offlineGithub(repoRoot) {
  return {
    repository: { owner: 'trycua', repo: 'cua', defaultBranch: 'main' },
    async getFileJson(file) {
      return JSON.parse(fs.readFileSync(path.join(repoRoot, file), 'utf8'));
    },
  };
}

function quietLogger() {
  const noop = () => {};
  return { error: noop, warn: noop, info: noop, debug: noop, trace: noop };
}

// The engine's own candidate releases for one merged PR: Manifest.buildReleases
// with the PR search replaced by this one PR.
async function wouldTag(pr, repoRoot = root) {
  const { setLogger } = require('release-please/build/src/util/logger');
  setLogger(quietLogger());
  const manifest = await Manifest.fromManifest(offlineGithub(repoRoot), 'main', CONFIG, MANIFEST);
  const merged = {
    number: pr.number,
    title: pr.title,
    headBranchName: pr.headRefName,
    baseBranchName: 'main',
    body: pr.body,
    labels: ['autorelease: pending'],
    files: [],
    sha: (pr.mergeCommit && pr.mergeCommit.oid) || 'f'.repeat(40),
  };
  manifest.findMergedReleasePullRequests = async function* () { yield merged; };
  const releases = await manifest.buildReleases();
  return releases.map(r => ({ path: r.path, tag: r.tag.toString() }));
}

function branchComponent(pr) {
  const branch = BranchName.parse(pr.headRefName, quietLogger());
  return branch ? branch.component || '' : '';
}

function configuredPathFor(component, repoRoot = root) {
  const config = JSON.parse(fs.readFileSync(path.join(repoRoot, CONFIG), 'utf8'));
  return Object.keys(config.packages).find(p => config.packages[p].component === component);
}

// Every candidate must belong to the PR's own component.
async function check(pr, repoRoot = root) {
  const component = branchComponent(pr);
  const ownPath = configuredPathFor(component, repoRoot);
  const releases = await wouldTag(pr, repoRoot);
  const stray = releases.filter(r => r.path !== ownPath);
  return { number: pr.number, component, path: ownPath || null, releases, stray, ok: stray.length === 0 };
}

// Move the first version heading to the top of the notes block, keeping every
// line of the PR body. Returns null when the body has no notes block or no
// version heading to move.
function repairBody(body) {
  const lines = body.trim().replace(/\r\n/g, '\n').split('\n');
  const first = lines.indexOf(DELIMITER);
  if (first === -1) return null;
  let last = lines.lastIndexOf(DELIMITER);
  if (last === first) return null;
  const content = lines.slice(first + 1, last);
  const heading = content.findIndex(line => VERSION_HEADING.test(line));
  if (heading === -1) return null;
  const before = content.slice(0, heading);
  if (before.every(line => line.trim() === '')) return body;
  const notes = [content[heading], '', ...before, ...content.slice(heading + 1)];
  return [...lines.slice(0, first + 1), '', '', ...notes, ...lines.slice(last)].join('\n');
}

async function main() {
  const [command, file] = process.argv.slice(2);
  if (!['check', 'repair'].includes(command) || !file) {
    console.error('usage: release_please_tag_guard.cjs check|repair <pr.json>');
    process.exit(2);
  }
  const pr = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (command === 'check') {
    const result = await check(pr);
    console.log(JSON.stringify(result));
    process.exit(result.ok ? 0 : 1);
  }
  const repaired = repairBody(pr.body || '');
  if (repaired === null) process.exit(1);
  const result = await check({ ...pr, body: repaired });
  if (!result.ok) process.exit(1);
  process.stdout.write(repaired);
}

module.exports = { wouldTag, check, repairBody, branchComponent };

if (require.main === module) {
  main().catch(error => {
    console.error(error);
    process.exit(2);
  });
}
