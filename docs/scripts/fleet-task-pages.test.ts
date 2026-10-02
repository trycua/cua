import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import * as path from 'node:path';
import test from 'node:test';

const docs = path.resolve(__dirname, '..');
const fleets = path.join(docs, 'content/docs/fleets');

async function page(slug: string) {
  return readFile(path.join(fleets, `${slug}.mdx`), 'utf8');
}

async function mdxFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map((entry) => {
      const target = path.join(directory, entry.name);
      if (entry.isDirectory()) return mdxFiles(target);
      return entry.name.endsWith('.mdx') ? [target] : [];
    })
  );
  return nested.flat();
}

test('the capacity page uses persistent language tabs on the cua SDK', async () => {
  const source = await page('guides/capacity-and-claims');
  assert.match(source, /<Tabs groupId="language" persist items=\{\['Python', 'TypeScript'\]\}>/);
  assert.match(source, /<Tabs groupId="language" persist items=\{\['CLI', 'Python', 'TypeScript'\]\}>/);
  assert.match(source, /<Tab value="Python">/);
  assert.match(source, /<Tab value="TypeScript">/);
  assert.match(source, /@trycua\/cua/);
  assert.doesNotMatch(source, /computer-server|@trycua\/fleet/);
});

test('one page creates capacity, claims from it and expires it', async () => {
  const source = await page('guides/capacity-and-claims');
  assert.match(source, /Sandbox\.create\(image, local=False\)/);
  assert.match(source, /Pool\.apply\(\s*os\.environ/);
  assert.match(source, /Pool\.apply\(\s*fleet,/);
  assert.doesNotMatch(source, /applyPool|FleetPoolSpec/);
  assert.match(source, /Sandbox\.ephemeral/);
  assert.match(source, /cua sb create --on fleet --pool/);
  assert.match(source, /delete_\(\)/);
  assert.match(source, /## Expiry/);
  assert.match(source, /ttl_seconds_after_created/);
});

test('the Fleets guides are the advanced cloud tasks, in order', async () => {
  const meta = JSON.parse(await readFile(path.join(fleets, 'guides/meta.json'), 'utf8'));
  assert.deepEqual(meta.pages, [
    'capacity-and-claims',
    'warm-pools',
    'terraform',
    'images',
    'sidecars',
    'claim-secrets',
    'troubleshoot',
  ]);
  for (const slug of meta.pages) await page(`guides/${slug}`);
});

test('no page retains a legacy language-specific capacity link', async () => {
  const files = await mdxFiles(path.join(docs, 'content/docs'));
  const legacySlug = /create-pool-with-(python|typescript)/;
  const remaining: string[] = [];
  for (const file of files) {
    if (legacySlug.test(await readFile(file, 'utf8'))) {
      remaining.push(path.relative(docs, file));
    }
  }
  assert.deepEqual(remaining, []);
});

test('legacy capacity URLs permanently redirect to the capacity page', async () => {
  const config = (await import('../next.config.mjs')).default;
  const redirects = (await config.redirects?.()) ?? [];
  for (const language of ['python', 'typescript']) {
    const source = `/how-to-guides/sandbox/create-pool-with-${language}`;
    assert.deepEqual(
      redirects.find((redirect) => redirect.source === source),
      { source, destination: '/fleets/guides/capacity-and-claims', permanent: true }
    );
  }
});
