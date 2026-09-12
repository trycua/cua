import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';

const docs = path.resolve(__dirname, '..');
const content = path.join(docs, 'content/docs');
const plannedPages = new Set([
  '/how-to-guides/sandbox/control-the-desktop-with-cua-driver',
  '/how-to-guides/sandbox/recover-first-cloud-fleet',
]);

async function metadata(relativePath: string): Promise<{ title?: string; pages: string[] }> {
  return JSON.parse(await readFile(path.join(content, relativePath, 'meta.json'), 'utf8'));
}

function link(entry: string): { label: string; url: string } | undefined {
  const match = /^\[([^\]]+)]\(([^)]+)\)$/.exec(entry);
  if (match) return { label: match[1], url: match[2] };
}

function separators(pages: string[]): string[] {
  return pages.filter((entry) => entry.startsWith('---')).map((entry) => entry.slice(3, -3));
}

function links(pages: string[]): Array<{ label: string; url: string }> {
  return pages.flatMap((entry) => {
    const parsed = link(entry);
    return parsed ? [parsed] : [];
  });
}

async function routeExists(url: string): Promise<boolean> {
  if (plannedPages.has(url)) return true;
  const relative = url.replace(/^\//, '');
  for (const candidate of [`${relative}.mdx`, path.join(relative, 'index.mdx')]) {
    try {
      await access(path.join(content, candidate));
      return true;
    } catch {}
  }
  return false;
}

test('desktop and mobile sidebars share the approved top-level product order', async () => {
  const root = await metadata('.');
  const expected = [
    'index',
    'tutorials',
    '(cloud-fleets)',
    '(local-sandboxes)',
    '---Products---',
    '[Cua Driver](/how-to-guides/driver/install)',
    '[Cua CLI](/reference/cua-cli)',
    '[Lume](/how-to-guides/lume/install-lume)',
    '[Cua-Bench](/how-to-guides/cua-bench/run-a-task)',
    '---Browse by type---',
    'concepts',
    'use-cua-with',
    'how-to-guides',
    'reference',
  ];

  // Fumadocs gives both responsive sidebar variants this one page tree.
  for (const viewport of ['desktop', 'mobile']) {
    assert.deepEqual(root.pages, expected, `${viewport} sidebar order`);
  }
  assert.equal(root.pages.filter((entry) => entry === 'tutorials').length, 1);
});

test('Cua Fleets uses the approved stages and contains no local setup tasks', async () => {
  const fleet = await metadata('(cloud-fleets)');
  assert.equal(fleet.title, 'Cua Fleets');
  assert.deepEqual(separators(fleet.pages), [
    'Overview',
    'Set up',
    'Create and connect',
    'Use the computer',
    'Operate',
    'Use Fleets with',
    'Images',
    'Reference',
  ]);

  const fleetLinks = links(fleet.pages);
  const localOnly = new Set([
    '/local-sandboxes',
    '/how-to-guides/sandbox/manage-local-lifecycle',
    '/how-to-guides/sandbox/interactive-shell',
    '/how-to-guides/sandbox/scale-out',
    '/how-to-guides/sandbox/images',
  ]);
  assert.equal(
    fleetLinks.some(({ url }) => localOnly.has(url)),
    false
  );
  assert.equal(
    fleetLinks.some(({ url }) => url.startsWith('/tutorials/')),
    false
  );
  assert.ok(fleetLinks.some(({ url }) => url.endsWith('/control-the-desktop-with-cua-driver')));
  assert.ok(fleetLinks.some(({ url }) => url.endsWith('/recover-first-cloud-fleet')));
});

test('Local Sandboxes owns local creation and lifecycle without Fleet setup', async () => {
  const local = await metadata('(local-sandboxes)');
  assert.equal(local.title, 'Local Sandboxes');
  assert.deepEqual(separators(local.pages), ['Overview', 'Create and use', 'Images', 'Reference']);

  const localLinks = links(local.pages);
  for (const required of [
    '/local-sandboxes',
    '/how-to-guides/sandbox/manage-local-lifecycle',
    '/how-to-guides/sandbox/interactive-shell',
    '/how-to-guides/sandbox/images',
  ]) {
    assert.ok(
      localLinks.some(({ url }) => url === required),
      `missing ${required}`
    );
  }
  assert.equal(
    localLinks.some(({ url }) =>
      /fleet|claim-a-sandbox|configure-pool|set-up-fleet-credentials/.test(url)
    ),
    false
  );
  assert.equal(
    localLinks.some(({ url }) => url.startsWith('/tutorials/')),
    false
  );
});

test('navigation labels point to existing or explicitly stacked pages', async () => {
  for (const group of ['(cloud-fleets)', '(local-sandboxes)']) {
    const nav = await metadata(group);
    for (const { label, url } of links(nav.pages)) {
      assert.ok(label.length > 0, `${group} has an empty label`);
      assert.equal(url.startsWith('/'), true, `${label} must use a stable root-relative URL`);
      assert.equal(await routeExists(url), true, `${label} points to missing ${url}`);
    }
  }
});

test('Track E entry and concept pages link only to existing routes', async () => {
  for (const page of [
    'cloud-fleets.mdx',
    'local-sandboxes.mdx',
    'concepts/pools-claims-and-guest-services.mdx',
  ]) {
    const body = await readFile(path.join(content, page), 'utf8');
    const urls = [...body.matchAll(/\[[^\]]+\]\((\/[^)]+)\)/g)].map((match) => match[1]);
    assert.ok(urls.length > 0, `${page} should provide next steps`);
    for (const url of urls) {
      assert.equal(await routeExists(url), true, `${page} points to missing ${url}`);
    }
  }
});

test('browse-by-type metadata reserves incoming Track F and G pages in stage order', async () => {
  const sandbox = await metadata('how-to-guides/sandbox');
  const driver = sandbox.pages.indexOf('control-the-desktop-with-cua-driver');
  const recovery = sandbox.pages.indexOf('recover-first-cloud-fleet');
  assert.ok(driver > sandbox.pages.indexOf('---Cua Fleets: Use the computer---'));
  assert.ok(driver < sandbox.pages.indexOf('---Cua Fleets: Operate---'));
  assert.ok(recovery > sandbox.pages.indexOf('---Cua Fleets: Operate---'));
  assert.ok(recovery < sandbox.pages.indexOf('troubleshoot-fleet-pools'));

  const concepts = await metadata('concepts');
  assert.ok(concepts.pages.includes('pools-claims-and-guest-services'));
});
