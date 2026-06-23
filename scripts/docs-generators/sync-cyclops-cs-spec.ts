#!/usr/bin/env npx tsx

/**
 * Cyclops CS spec sync
 *
 * Refreshes the vendored Cyclops CS backend OpenAPI spec from its source of
 * truth in trycua/cloud (`cyclops-cs/backend/docs/swagger.json`) and
 * regenerates the reference MDX. This is the "sync whenever the JSON changes"
 * half of the routine; the page-vs-spec half is enforced by `docs:check`.
 *
 * Source resolution (first match wins):
 *   1. `--from <path-or-url>` CLI argument
 *   2. `CYCLOPS_CS_SPEC_SOURCE` environment variable
 *   3. local sibling checkout: ../cloud/cyclops-cs/backend/docs/swagger.json
 *
 * A `--from` value starting with http(s):// is fetched over the network; an
 * optional `GITHUB_TOKEN` is sent as a Bearer token (needed for the private
 * trycua/cloud repo via the raw contents API).
 *
 * Usage:
 *   npx tsx scripts/docs-generators/sync-cyclops-cs-spec.ts
 *   npx tsx scripts/docs-generators/sync-cyclops-cs-spec.ts --from ../cloud/cyclops-cs/backend/docs/swagger.json
 *   npx tsx scripts/docs-generators/sync-cyclops-cs-spec.ts --from https://api.github.com/repos/trycua/cloud/contents/cyclops-cs/backend/docs/swagger.json?ref=main
 *   npx tsx scripts/docs-generators/sync-cyclops-cs-spec.ts --check   # fail if the vendored copy is stale
 */

import { spawnSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

const ROOT_DIR = path.resolve(__dirname, '../..');
const VENDORED_SPEC = path.join(__dirname, 'specs', 'cyclops-cs.swagger.json');
const GENERATOR = path.join(__dirname, 'cyclops-cs.ts');
const DEFAULT_LOCAL = path.resolve(ROOT_DIR, '../cloud/cyclops-cs/backend/docs/swagger.json');

function resolveSource(args: string[]): string {
  const i = args.indexOf('--from');
  if (i !== -1) {
    const value = args[i + 1];
    if (!value || value.startsWith('--')) {
      throw new Error('`--from` requires a path or URL value.');
    }
    return value;
  }
  if (process.env.CYCLOPS_CS_SPEC_SOURCE) return process.env.CYCLOPS_CS_SPEC_SOURCE;
  return DEFAULT_LOCAL;
}

async function readSource(source: string): Promise<string> {
  if (/^https?:\/\//.test(source)) {
    const headers: Record<string, string> = { Accept: 'application/vnd.github.raw+json' };
    if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
    // Abort a stalled connection so a CI sync job can't hang indefinitely.
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 30_000);
    let res: Response;
    try {
      res = await fetch(source, { headers, signal: controller.signal });
    } finally {
      clearTimeout(timer);
    }
    if (!res.ok) {
      throw new Error(`Failed to fetch spec from ${source}: ${res.status} ${res.statusText}`);
    }
    return await res.text();
  }
  const abs = path.isAbsolute(source) ? source : path.resolve(process.cwd(), source);
  if (!fs.existsSync(abs)) {
    throw new Error(`Spec source not found: ${abs}`);
  }
  return fs.readFileSync(abs, 'utf-8');
}

/** Stable, formatted JSON so byte-for-byte diffs are meaningful. */
function normalize(raw: string): string {
  const parsed = JSON.parse(raw);
  if (!parsed || typeof parsed !== 'object' || !('paths' in parsed)) {
    throw new Error('Source does not look like an OpenAPI/Swagger document (no `paths`).');
  }
  return JSON.stringify(parsed, null, 2) + '\n';
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const checkOnly = args.includes('--check');
  const source = resolveSource(args);

  console.log('Cyclops CS spec sync');
  console.log('====================\n');
  console.log(`Source: ${source}`);

  const incoming = normalize(await readSource(source));
  const current = fs.existsSync(VENDORED_SPEC) ? fs.readFileSync(VENDORED_SPEC, 'utf-8') : '';
  const changed = incoming !== current;

  if (checkOnly) {
    if (changed) {
      console.error('\nVendored spec is stale relative to the source.');
      console.error('Run: npx tsx scripts/docs-generators/sync-cyclops-cs-spec.ts');
      process.exit(1);
    }
    console.log('\nVendored spec is up to date.');
    return;
  }

  if (!changed) {
    console.log('\nVendored spec already current — nothing to do.');
    return;
  }

  fs.mkdirSync(path.dirname(VENDORED_SPEC), { recursive: true });
  fs.writeFileSync(VENDORED_SPEC, incoming);
  console.log(`Updated ${path.relative(ROOT_DIR, VENDORED_SPEC)}`);

  console.log('\nRegenerating reference docs...');
  const result = spawnSync('npx', ['tsx', GENERATOR], { cwd: ROOT_DIR, stdio: 'inherit' });
  if (result.status !== 0) process.exit(result.status ?? 1);
}

main().catch((error) => {
  console.error('Error:', error instanceof Error ? error.message : error);
  process.exit(1);
});
