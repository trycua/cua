import assert from 'node:assert/strict';
import { access, readFile, readdir } from 'node:fs/promises';
import * as path from 'node:path';
import test from 'node:test';

const docs = path.resolve(__dirname, '..');
const sandbox = path.join(docs, 'content/docs/how-to-guides/sandbox');

async function page(slug: string) {
  return readFile(path.join(sandbox, `${slug}.mdx`), 'utf8');
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

test('neutral Fleet task pages use persistent Python and TypeScript tabs', async () => {
  for (const slug of ['create-fleet-capacity', 'claim-a-sandbox']) {
    const source = await page(slug);
    assert.match(source, /<Tabs groupId="language" persist items=\{\['Python', 'TypeScript'\]\}>/);
    assert.match(source, /<Tab value="Python">/);
    assert.match(source, /<Tab value="TypeScript">/);
    assert.match(source, /Cua Sandbox SDK/);
    assert.match(source, /Fleet SDK/);
    assert.match(source, /not a TypeScript Sandbox SDK/);
    assert.doesNotMatch(source, /<Tab value="CLI">/);
  }
});

test('capacity and claim remain separate tasks', async () => {
  const capacity = await page('create-fleet-capacity');
  const claim = await page('claim-a-sandbox');
  assert.match(capacity, /They do\n\+?not claim a Sandbox\./);
  assert.doesNotMatch(capacity, /CreateClaimRequestBuilder/);
  assert.match(claim, /Pool\.get/);
  assert.match(claim, /client\.getPool/);
  assert.match(claim, /createClaim/);
  assert.match(claim, /deleteClaim/);
});

test('the Sandbox navigation swaps only the two legacy slugs', async () => {
  const metadata = JSON.parse(await readFile(path.join(sandbox, 'meta.json'), 'utf8'));
  const first = metadata.pages.indexOf('create-fleet-capacity');
  assert.ok(first >= 0);
  assert.equal(metadata.pages[first + 1], 'claim-a-sandbox');
  assert.ok(!metadata.pages.includes('create-pool-with-python'));
  assert.ok(!metadata.pages.includes('create-pool-with-typescript'));

  for (const slug of ['create-pool-with-python', 'create-pool-with-typescript']) {
    await assert.rejects(access(path.join(sandbox, `${slug}.mdx`)));
  }
});

test('only the integration-owned tutorial retains a legacy link handoff', async () => {
  const files = await mdxFiles(path.join(docs, 'content/docs'));
  const legacySlug = /create-pool-with-(python|typescript)/;
  const remaining: string[] = [];
  for (const file of files) {
    if (legacySlug.test(await readFile(file, 'utf8'))) {
      remaining.push(path.relative(docs, file));
    }
  }
  assert.deepEqual(remaining, ['content/docs/tutorials/your-first-cloud-fleet.mdx']);
});

test('legacy language-specific URLs permanently redirect to neutral capacity', async () => {
  const config = (await import('../next.config.mjs')).default;
  const redirects = await config.redirects();
  for (const language of ['python', 'typescript']) {
    assert.deepEqual(
      redirects.find(
        (redirect) => redirect.source === `/how-to-guides/sandbox/create-pool-with-${language}`
      ),
      {
        source: `/how-to-guides/sandbox/create-pool-with-${language}`,
        destination: '/how-to-guides/sandbox/create-fleet-capacity',
        permanent: true,
      }
    );
  }
});
