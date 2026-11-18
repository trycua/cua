#!/usr/bin/env node
// Checks the docs IA redirects against a running site (a preview or production).
//
//   node scripts/docs/check-redirects.mjs <base-url> [csv]
//
// <base-url> is the site origin, for example https://cua.ai or a preview URL.
// The CSV (default docs/redirects-ia-2026-10.csv) has the header `old,new` and
// site paths such as /docs/start-here/foo,/docs/spaces/quickstart.
//
// For every row it asserts that:
//   1. (unless old == new) the old URL answers 301 with a Location that is the new URL (a fragment
//      in the new URL is ignored: browsers keep it, servers never see it);
//   2. the new URL answers 200 directly, with no further redirect (no chains);
//   3. the new page's <link rel="canonical"> is the new URL.
//
// Canonical hosts differ between a preview and production, so only the path is
// compared unless --canonical-origin <origin> is given.
//
// Options:
//   --concurrency <n>          parallel requests (default 8)
//   --canonical-origin <url>   also require this origin in the canonical URL
//   --accept-308 1             accept 308 as well; for `next dev`, which sends 308 (production must send 301)
//
// Exits 1 when any row fails.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const flags = {};
const positional = [];
for (let i = 0; i < args.length; i++) {
  if (args[i].startsWith('--')) flags[args[i].slice(2)] = args[++i];
  else positional.push(args[i]);
}
const [baseArg, csvArg] = positional;
if (!baseArg) {
  console.error('usage: node scripts/docs/check-redirects.mjs <base-url> [csv] [--concurrency n] [--canonical-origin url]');
  process.exit(2);
}
const base = new URL(baseArg.replace(/\/+$/, '') + '/');
const csvPath = resolve(csvArg ?? resolve(here, '../../docs/redirects-ia-2026-10.csv'));
const concurrency = Number(flags.concurrency ?? 8);
const canonicalOrigin = flags['canonical-origin']?.replace(/\/+$/, '');

const rows = readFileSync(csvPath, 'utf8')
  .split(/\r?\n/)
  .map((line) => line.trim())
  .filter(Boolean)
  .slice(1)
  .map((line) => {
    const [old, next] = line.split(',');
    return { old, next };
  });

const pathOf = (p) => p.replace(/#.*$/, '').replace(/(.)\/+$/, '$1');
const urlFor = (p) => new URL(p.replace(/^\/+/, ''), base).href;

async function get(url) {
  return fetch(url, { redirect: 'manual', headers: { 'user-agent': 'cua-docs-redirect-check' } });
}

async function check({ old, next }) {
  const errors = [];
  // A row with old == new is a page that must keep its URL: only the checks on the new URL run.
  if (pathOf(old) !== pathOf(next)) {
    const oldRes = await get(urlFor(pathOf(old)));
    const location = oldRes.headers.get('location');
    if (oldRes.status !== 301 && !(flags['accept-308'] !== undefined && oldRes.status === 308)) errors.push(`${old}: expected 301, got ${oldRes.status}`);
    else if (!location) errors.push(`${old}: 301 without a Location header`);
    else {
      const to = new URL(location, urlFor(old));
      if (pathOf(to.pathname) !== pathOf(next)) errors.push(`${old}: Location is ${to.pathname}, expected ${pathOf(next)}`);
    }
  }

  const newRes = await get(urlFor(pathOf(next)));
  if (newRes.status !== 200) {
    errors.push(`${next}: expected 200, got ${newRes.status}${newRes.headers.get('location') ? ` (redirects to ${newRes.headers.get('location')})` : ''}`);
  } else {
    const html = await newRes.text();
    const tag = html.match(/<link\b[^>]*\brel=["']canonical["'][^>]*>/i)?.[0];
    const href = tag?.match(/\bhref=["']([^"']+)["']/i)?.[1];
    if (!href) errors.push(`${next}: no canonical link`);
    else {
      const canonical = new URL(href, urlFor(next));
      if (pathOf(canonical.pathname) !== pathOf(next)) errors.push(`${next}: canonical is ${canonical.pathname}`);
      else if (canonicalOrigin && canonical.origin !== canonicalOrigin) errors.push(`${next}: canonical origin is ${canonical.origin}, expected ${canonicalOrigin}`);
    }
  }
  return errors;
}

const failures = [];
let next = 0;
async function worker() {
  while (next < rows.length) {
    const row = rows[next++];
    try {
      failures.push(...(await check(row)));
    } catch (error) {
      failures.push(`${row.old}: ${error.message}`);
    }
  }
}
await Promise.all(Array.from({ length: concurrency }, worker));

if (failures.length) {
  console.error(failures.sort().join('\n'));
  console.error(`\n${failures.length} problem(s) across ${rows.length} redirects (${base.origin})`);
  process.exit(1);
}
console.log(`ok: ${rows.length} redirects 301 to a 200 page with the right canonical (${base.origin})`);
