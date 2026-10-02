// The Cua SDK and Cua Fleets trees: short task guides, local and cloud on the
// same pages, and every removed slug redirected to the page that replaced it.
import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';

const docs = path.resolve(__dirname, '..');
const content = path.join(docs, 'content/docs');

async function metadata(relativePath: string): Promise<{ title?: string; pages: string[] }> {
  return JSON.parse(await readFile(path.join(content, relativePath, 'meta.json'), 'utf8'));
}

async function routeExists(url: string): Promise<boolean> {
  const relative = url.replace(/^\//, '').replace(/#.*$/, '');
  for (const candidate of [`${relative}.mdx`, path.join(relative, 'index.mdx')]) {
    try {
      await access(path.join(content, candidate));
      return true;
    } catch {}
  }
  return false;
}

test('the Cua SDK has its task guides and one concept page', async () => {
  const guides = await metadata('cua-sdk/guides');
  assert.equal(guides.title, 'Guides');
  assert.deepEqual(guides.pages, [
    'images',
    'services',
    'secrets',
    'sidecars',
    'lifecycle',
    'desktop',
    'local-runtimes',
    'disk-usage',
    'cloud',
    'contrib-providers',
    'test-in-ci',
    'run-a-coding-agent',
    'browse-the-web',
    'agent-frameworks',
    'omarchy',
  ]);
  assert.deepEqual((await metadata('cua-sdk/concepts')).pages, ['how-sandboxes-work']);
  for (const slug of guides.pages) {
    assert.equal(await routeExists(`/cua-sdk/guides/${slug}`), true, slug);
  }
});

test('the quickstart runs locally and lists the images and benchmark images', async () => {
  const body = await readFile(path.join(content, 'cua-sdk/quickstart.mdx'), 'utf8');
  assert.ok(body.includes('<Tab value="Python">'), 'the quickstart leads with Python');
  assert.ok(!body.includes('local=False') && !body.includes('<Tab value="Cloud">'), 'local only');
  for (const ref of [
    'ghcr.io/trycua/linux:24.04',
    'ghcr.io/trycua/linux:24.04-disk',
    'ghcr.io/trycua/windows:2022',
    'ghcr.io/trycua/macos:26',
    'ghcr.io/trycua/macos:15',
    'ghcr.io/trycua/bench-osworld:verified',
    'ghcr.io/trycua/bench-web:1.0',
  ]) {
    assert.ok(body.includes(`\`${ref}\``), `missing ${ref}`);
  }
  assert.ok(body.includes('(/cua-bench/guides/adapter-benchmarks#osworld-verified)'));
  assert.ok(body.includes('(/cua-bench/guides/adapter-benchmarks#miniwob)'));
});

test('entry pages link only to existing routes', async () => {
  for (const page of [
    'cua-sdk/index.mdx',
    'cua-sdk/quickstart.mdx',
    'cua-sdk/guides/cloud.mdx',
    'cua-sdk/guides/local-runtimes.mdx',
    'fleets/index.mdx',
    'fleets/quickstart.mdx',
  ]) {
    const body = await readFile(path.join(content, page), 'utf8');
    const urls = [...body.matchAll(/\[[^\]]+\]\((\/[^)]+)\)/g)].map((match) => match[1]);
    assert.ok(urls.length > 0, `${page} should provide next steps`);
    for (const url of urls) {
      assert.equal(await routeExists(url), true, `${page} points to missing ${url}`);
    }
  }
});

test('removed Sandbox and Fleet pages redirect to the page that replaced them', async () => {
  const { redirects } = JSON.parse(await readFile(path.join(content, 'redirects.json'), 'utf8'));
  const expected: Record<string, string> = {
    'cua-sdk/guides/choose-an-image': 'cua-sdk/guides/images',
    'cua-sdk/guides/mcp': 'cua-sdk/guides/services',
    'cua-sdk/guides/control-the-desktop-with-cua-driver': 'cua-sdk/guides/desktop',
    'cua-sdk/guides/set-up-fleet-credentials': 'cua-sdk/guides/cloud',
    'cua-sdk/guides/examples/minecraft': 'cua-sdk/guides/agent-frameworks',
    'cua-sdk/concepts/architecture': 'cua-sdk/concepts/how-sandboxes-work',
    'fleets/concepts/pools-claims-and-guest-services': 'fleets',
    'fleets/guides/claim-a-sandbox': 'fleets/guides/capacity-and-claims',
    'fleets/guides/recover-first-cloud-fleet': 'fleets/guides/troubleshoot',
  };
  for (const [from, to] of Object.entries(expected)) {
    assert.equal(redirects[from], to, from);
    assert.equal(await routeExists(`/${to}`), true, to);
  }
});
