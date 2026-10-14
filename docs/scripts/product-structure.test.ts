// Every product has the same sections, in the same order, with the same names,
// and its Reference holds only generated pages that `docs:check` covers.
//
//   pnpm exec tsx --test scripts/product-structure.test.ts
import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const docs = path.resolve(__dirname, '..');
const repo = path.resolve(docs, '..');
const content = path.join(docs, 'content/docs');

const PRODUCTS: Array<[string, string]> = [
  ['cua-sdk', 'Cua SDK'],
  ['cua-driver', 'Cua Driver'],
  ['spaces', 'Cua Spaces'],
  ['fleets', 'Cua Fleets'],
  ['cua-bench', 'Cua Bench'],
  ['lume', 'Lume'],
  ['cua-cli', 'Cua CLI'],
];
// Sections in their only allowed order; `examples` and `concepts` are optional.
const SECTIONS = ['index', 'quickstart', 'guides', 'examples', 'concepts', 'reference'];
const OPTIONAL = new Set(['examples', 'concepts']);
const SECTION_TITLES: Record<string, string> = {
  guides: 'Guides',
  examples: 'Examples',
  concepts: 'Concepts',
  reference: 'Reference',
};
// The one non-product section: curated end-to-end walkthroughs, listed first.
const START_HERE: [string, string] = ['start-here', 'Start here'];
// The docs home is the designed landing; its frontmatter feeds it.
// Each example: a title, a page, alt text and its media (an image, or a
// video with its poster); the description is optional.
const LANDING_EXAMPLE_KEYS = ['title', 'href', 'alt'];
// Pages allowed at the root besides the product folders.
const ROOT_PAGES = new Set(['index.mdx', 'docs-for-agents.mdx']);
const GENERATED_MARKER = /AUTO-GENERATED|\{\/\*\s*GENERATED:[\w-]+:start\s*\*\/\}/;

type Meta = { title?: string; root?: boolean; pages?: string[] };

function meta(dir: string): Meta {
  return JSON.parse(readFileSync(path.join(content, dir, 'meta.json'), 'utf8'));
}

function walk(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    return statSync(full).isDirectory() ? walk(full) : [full];
  });
}

function slugs(): Set<string> {
  return new Set(
    walk(content)
      .filter((f) => f.endsWith('.mdx'))
      .map((f) =>
        path
          .relative(content, f)
          .replace(/\.mdx$/, '')
          .replace(/(^|\/)index$/, '')
      )
  );
}

test('the root lists Start here, then the products in order', () => {
  assert.deepEqual(meta('').pages, [
    'index',
    START_HERE[0],
    ...PRODUCTS.map(([dir]) => dir),
  ]);
  const stray = readdirSync(content).filter(
    (name) =>
      name.endsWith('.mdx') && !ROOT_PAGES.has(name)
  );
  assert.deepEqual(stray, [], 'pages at the root belong in a product');
  const dirs = readdirSync(content).filter((name) =>
    statSync(path.join(content, name)).isDirectory()
  );
  assert.deepEqual(
    dirs.sort(),
    [START_HERE[0], ...PRODUCTS.map(([dir]) => dir)].sort()
  );
});

test('Start here is a flat list of walkthroughs', () => {
  const [dir, title] = START_HERE;
  const m = meta(dir);
  assert.equal(m.title, title);
  assert.notEqual(m.root, true, `${dir} lives in the main sidebar, not its own tab`);
  const pages = m.pages ?? [];
  // Its Overview is the docs home (index.mdx), so it has no index of its own.
  assert.ok(!pages.includes('index'), `${dir} has no index: its Overview is the docs home`);
  assert.equal(pages[0], 'sandbox-images', `${dir}/meta.json opens with Sandbox images`);
  const files = readdirSync(path.join(content, dir)).filter((name) => name !== 'meta.json');
  for (const name of files) {
    assert.ok(name.endsWith('.mdx'), `${dir}/${name}: Start here has no subfolders`);
  }
  assert.deepEqual(
    files.map((name) => name.replace(/\.mdx$/, '')).sort(),
    [...pages].sort(),
    `${dir}/meta.json lists exactly its pages`
  );
});

test('the docs home features its examples with real pages and media', () => {
  const home = readFileSync(path.join(content, 'index.mdx'), 'utf8');
  const frontmatter = home.match(/^---\n([\s\S]*?)\n---/)?.[1] ?? '';
  assert.match(frontmatter, /^landing:/m, 'index.mdx has landing frontmatter');
  const featured = frontmatter.match(/^  products: \[(.*)\]$/m)?.[1].split(/,\s*/) ?? [];
  assert.ok(featured.length <= 4, 'the landing features at most four products (one row)');
  for (const dir of featured) {
    assert.ok(PRODUCTS.some(([p]) => p === dir), `featured product ${dir} is not a product`);
  }
  const live = slugs();
  const examples = frontmatter.split(/\n    - title: /).slice(1);
  assert.ok(examples.length > 0, 'the landing features at least one example');
  for (const example of examples) {
    const fields = `title: ${example}`;
    for (const key of LANDING_EXAMPLE_KEYS) {
      assert.match(fields, new RegExp(`^\\s*${key}: `, 'm'), `example lacks ${key}`);
    }
    for (const [, link] of fields.matchAll(/href: (\S+)/g)) {
      if (/^https:\/\//.test(link)) continue;
      assert.ok(live.has(link.replace(/^\//, '')), `example link ${link} is not a page`);
    }
    const media = ['image', 'video', 'poster'].map(
      (key) => fields.match(new RegExp(`^\\s*${key}: (\\S+)`, 'm'))?.[1]
    );
    const [image, video, poster] = media;
    assert.ok(image || (video && poster), 'example has an image, or a video with its poster');
    for (const src of media) {
      if (!src || /^https:\/\//.test(src)) continue;
      assert.ok(existsSync(path.join(docs, 'public', src)), `example media ${src} is missing`);
    }
  }
});

for (const [dir, title] of PRODUCTS) {
  test(`${title} has the standard sections`, () => {
    const m = meta(dir);
    assert.equal(m.title, title);
    assert.equal(m.root, true, `${dir}/meta.json must be a root folder (its own sidebar tab)`);
    const pages = m.pages ?? [];
    const expected = SECTIONS.filter(
      (section) => !OPTIONAL.has(section) || pages.includes(section)
    );
    assert.deepEqual(pages, expected, `${dir}/meta.json pages must be ${expected.join(', ')}`);
    assert.ok(existsSync(path.join(content, dir, 'index.mdx')), `${dir}/index.mdx (Overview)`);
    assert.ok(existsSync(path.join(content, dir, 'quickstart.mdx')), `${dir}/quickstart.mdx`);
    for (const section of pages.filter((p) => SECTION_TITLES[p])) {
      assert.equal(meta(path.join(dir, section)).title, SECTION_TITLES[section]);
    }
    const extra = readdirSync(path.join(content, dir)).filter(
      (name) =>
        name !== 'meta.json' &&
        !pages.includes(name.replace(/\.mdx$/, ''))
    );
    assert.deepEqual(extra, [], `${dir} has files outside its sections`);
  });

  test(`${title} Reference holds only generated pages covered by docs:check`, () => {
    const config = JSON.parse(
      readFileSync(path.join(repo, 'scripts/docs-generators/config.json'), 'utf8')
    ) as { generators: Record<string, { enabled: boolean; docsOutputPath: string; outputs: Array<{ outputFile: string }> }> };
    const owned = Object.values(config.generators)
      .filter((g) => g.enabled)
      .flatMap((g) => [
        path.join(repo, g.docsOutputPath),
        ...g.outputs.map((o) => path.dirname(path.join(repo, g.docsOutputPath, o.outputFile))),
      ]);
    const pages = walk(path.join(content, dir, 'reference')).filter((f) => f.endsWith('.mdx'));
    for (const file of pages) {
      const rel = path.relative(content, file);
      assert.ok(GENERATED_MARKER.test(readFileSync(file, 'utf8')), `${rel} lacks the generator marker`);
      assert.ok(
        owned.some((out) => path.dirname(file) === out || path.dirname(file).startsWith(out + path.sep)),
        `${rel} is not under an enabled generator's output in scripts/docs-generators/config.json`
      );
    }
  });
}

test('redirects point at pages that exist', () => {
  const { redirects } = JSON.parse(
    readFileSync(path.join(content, 'redirects.json'), 'utf8')
  ) as { redirects: Record<string, string> };
  const live = slugs();
  for (const [from, to] of Object.entries(redirects)) {
    assert.ok(!live.has(from), `redirect source ${from} is a live page`);
    let target = to;
    for (let hop = 0; hop < 8 && Object.hasOwn(redirects, target); hop++) target = redirects[target];
    assert.ok(live.has(target.replace(/#.*$/, '')), `redirect ${from} -> ${to} does not reach a page`);
  }
});
