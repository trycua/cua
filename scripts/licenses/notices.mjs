#!/usr/bin/env node
// Regenerate the npm dependency section of THIRD_PARTY_NOTICES.md for the
// Cua Spaces UI apps and fail on licences outside the allow-list.
//
//   node scripts/licenses/notices.mjs           # rewrite the section, then check
//   node scripts/licenses/notices.mjs --check   # fail if the section is stale
//
// Walks each app's installed node_modules (npm, pnpm or yarn layouts) from the
// production `dependencies` and `optionalDependencies` in its package.json.
// Packages that resolve inside this repository (workspace packages) are first
// party and skipped; scripts/check-license-direction.py covers them. An app
// whose node_modules is missing is skipped with a note. No dependencies.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const NOTICES = path.join(ROOT, 'THIRD_PARTY_NOTICES.md');
const APPS = ['apps/cua-spaces-web', 'apps/cua-spaces-desktop'];
const BEGIN = '<!-- BEGIN GENERATED NPM DEPENDENCIES (scripts/licenses/notices.mjs) -->';
const END = '<!-- END GENERATED NPM DEPENDENCIES -->';

const ALLOWED = new Set([
  'MIT',
  'ISC',
  'BSD-2-Clause',
  'BSD-3-Clause',
  'Apache-2.0',
  '0BSD',
  'Unlicense',
  'CC0-1.0',
  'BlueOak-1.0.0',
  'Python-2.0', // PSF licence, permissive (argparse via electron-updater)
  'OFL-1.1', // fonts
]);
const REVIEW = new Set(['MPL-2.0']);
const ALIASES = new Map(
  Object.entries({
    'mit license': 'MIT',
    'the mit license': 'MIT',
    'apache 2.0': 'Apache-2.0',
    'apache-2': 'Apache-2.0',
    'apache license 2.0': 'Apache-2.0',
    'apache license, version 2.0': 'Apache-2.0',
    bsd: 'BSD-3-Clause',
    'bsd-3': 'BSD-3-Clause',
    'bsd-2': 'BSD-2-Clause',
    cc0: 'CC0-1.0',
    'ofl-1.1-rfn': 'OFL-1.1',
    'ofl-1.1-no-rfn': 'OFL-1.1',
    'sil ofl 1.1': 'OFL-1.1',
    'mpl 2.0': 'MPL-2.0',
  })
);
const CANON = new Map([...ALLOWED, ...REVIEW].map((id) => [id.toLowerCase(), id]));

const check = process.argv.includes('--check');

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function canon(id) {
  const key = id.trim().toLowerCase();
  return CANON.get(key) ?? ALIASES.get(key) ?? id.trim();
}

function declaredLicense(pkg) {
  let lic = pkg.license ?? pkg.licenses;
  if (Array.isArray(lic))
    lic = lic
      .map((l) => (typeof l === 'string' ? l : l?.type))
      .filter(Boolean)
      .join(' OR ');
  else if (lic && typeof lic === 'object') lic = lic.type;
  return typeof lic === 'string' && lic.trim() ? lic.trim() : null;
}

// Classify an SPDX expression: "ok", "review" or "fail". OR takes the best
// branch, AND the worst. Parentheses are handled by a tiny recursive parser.
const RANK = { ok: 0, review: 1, fail: 2 };
function classify(expr) {
  if (!expr || /^see licen[cs]e in/i.test(expr) || /^unlicensed$/i.test(expr)) return 'fail';
  const tokens = expr.replace(/[()]/g, ' $& ').split(/\s+/).filter(Boolean);
  let i = 0;
  const atom = () => {
    if (tokens[i] === '(') {
      i++;
      const v = or();
      i++; // ")"
      return v;
    }
    let id = tokens[i++] ?? '';
    if (tokens[i]?.toUpperCase() === 'WITH') i += 2; // exceptions only widen grants
    id = canon(id.replace(/\+$/, ''));
    return ALLOWED.has(id) ? 'ok' : REVIEW.has(id) ? 'review' : 'fail';
  };
  const and = () => {
    let v = atom();
    while (tokens[i]?.toUpperCase() === 'AND') {
      i++;
      const w = atom();
      if (RANK[w] > RANK[v]) v = w;
    }
    return v;
  };
  const or = () => {
    let v = and();
    while (tokens[i]?.toUpperCase() === 'OR') {
      i++;
      const w = and();
      if (RANK[w] < RANK[v]) v = w;
    }
    return v;
  };
  return or();
}

// Node-style lookup: the nearest node_modules/<name> walking up from `from`.
function resolvePkg(name, from) {
  for (let dir = from; ; dir = path.dirname(dir)) {
    const candidate = path.join(dir, 'node_modules', name, 'package.json');
    if (fs.existsSync(candidate)) return fs.realpathSync(path.dirname(candidate));
    if (dir === path.dirname(dir)) return null;
  }
}

function isFirstParty(dir) {
  const rel = path.relative(ROOT, dir);
  return !rel.startsWith('..') && !rel.split(path.sep).includes('node_modules');
}

function prodDeps(pkg) {
  return [...Object.keys(pkg.dependencies ?? {}), ...Object.keys(pkg.optionalDependencies ?? {})];
}

function walkApp(appDir, found, missing) {
  const queue = prodDeps(readJson(path.join(appDir, 'package.json'))).map((name) => [
    name,
    appDir,
    true,
  ]);
  const seen = new Set();
  while (queue.length) {
    const [name, from, direct] = queue.shift();
    const dir = resolvePkg(name, from);
    if (!dir) {
      // Optional or platform-specific packages may legitimately be absent.
      if (direct) missing.push(name);
      continue;
    }
    if (seen.has(dir)) continue;
    seen.add(dir);
    const pkg = readJson(path.join(dir, 'package.json'));
    if (!isFirstParty(dir)) {
      const key = `${pkg.name}@${pkg.version}`;
      if (!found.has(key)) {
        const license = declaredLicense(pkg);
        const repo = typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url;
        found.set(key, { name: pkg.name, version: pkg.version, license, repo, apps: new Set() });
      }
      found.get(key).apps.add(path.basename(appDir));
    }
    for (const dep of prodDeps(pkg)) queue.push([dep, dir, false]);
  }
}

function repoUrl(repo) {
  if (!repo) return null;
  let url = repo
    .replace(/^git\+/, '')
    .replace(/\.git$/, '')
    .replace(/^git:\/\//, 'https://');
  url = url.replace(/^git@github\.com:/, 'https://github.com/');
  if (/^github:/.test(url)) url = url.replace(/^github:/, 'https://github.com/');
  if (/^[\w.-]+\/[\w.-]+$/.test(url)) url = `https://github.com/${url}`;
  return /^https?:\/\//.test(url) ? url : null;
}

const found = new Map();
const notes = [];
for (const rel of APPS) {
  const appDir = path.join(ROOT, rel);
  if (!fs.existsSync(path.join(appDir, 'package.json'))) {
    notes.push(`${rel}: not present, skipped`);
    continue;
  }
  if (!fs.existsSync(path.join(appDir, 'node_modules'))) {
    notes.push(`${rel}: node_modules missing (run pnpm install), skipped`);
    continue;
  }
  const missing = [];
  walkApp(appDir, found, missing);
  notes.push(`${rel}: scanned`);
  if (missing.length) notes.push(`${rel}: not installed, skipped: ${missing.join(', ')}`);
}

const entries = [...found.values()].sort(
  (a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version)
);
const failures = [];
const review = [];
for (const e of entries) {
  const verdict = classify(e.license);
  if (verdict === 'fail') failures.push(e);
  if (verdict === 'review') review.push(e);
}

let body;
if (entries.length === 0) {
  body =
    "No dependencies listed yet: neither app's dependencies were installed when this section was generated.";
} else {
  body = entries
    .map((e) => {
      const url = repoUrl(e.repo);
      const name = url ? `[\`${e.name}\`](${url})` : `\`${e.name}\``;
      return `- ${name} ${e.version}: ${e.license ?? 'no licence declared'} (${[...e.apps].sort().join(', ')})`;
    })
    .join('\n');
}
const section = `${BEGIN}\n\n${body}\n\n${END}`;

const current = fs.readFileSync(NOTICES, 'utf8');
const start = current.indexOf(BEGIN);
const stop = current.indexOf(END);
if (start < 0 || stop < start) {
  console.error(`THIRD_PARTY_NOTICES.md: markers not found (${BEGIN} ... ${END})`);
  process.exit(1);
}
const next = current.slice(0, start) + section + current.slice(stop + END.length);
let stale = false;
if (next !== current) {
  if (check) stale = true;
  else fs.writeFileSync(NOTICES, next);
}

const counts = new Map();
for (const e of entries)
  counts.set(e.license ?? '(none)', (counts.get(e.license ?? '(none)') ?? 0) + 1);
console.log('Cua Spaces UI third-party npm dependencies');
for (const n of notes) console.log(`  ${n}`);
console.log(`  ${entries.length} packages`);
for (const [lic, n] of [...counts].sort((a, b) => b[1] - a[1]))
  console.log(`    ${String(n).padStart(4)}  ${lic}`);
for (const e of review) console.log(`  REVIEW ${e.name}@${e.version}: ${e.license}`);
for (const e of failures)
  console.log(`  FAIL   ${e.name}@${e.version}: ${e.license ?? 'no licence declared'}`);
if (stale)
  console.log('  FAIL   THIRD_PARTY_NOTICES.md is stale; run node scripts/licenses/notices.mjs');
else
  console.log(
    check
      ? '  THIRD_PARTY_NOTICES.md is up to date'
      : next !== current
        ? '  THIRD_PARTY_NOTICES.md updated'
        : '  THIRD_PARTY_NOTICES.md unchanged'
  );

process.exit(failures.length || stale ? 1 : 0);
