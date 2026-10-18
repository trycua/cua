import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import test from 'node:test';

import {
  catalogRegionDocuments,
  fillCatalogRegions,
  renderBenchmarks,
  renderCuaImages,
  renderImageBullets,
  loadImageList,
  loadManifest,
  pickerImages,
  renderImageList,
  renderTiers,
  validateImageList,
  loadSourceFacts,
  renderCatalog,
  replaceGeneratedSection,
  validateManifest,
} from './sandbox';

test('qualified image facts conform to the local schema contract', () => {
  const manifest = loadManifest();
  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.qualifiedImages.length, 1);
  assert.match(manifest.qualifiedImages[0].image, /@sha256:[0-9a-f]{64}$/);
});

test('schema validation rejects mutable qualified images and undeclared fields', () => {
  const manifest = structuredClone(loadManifest()) as unknown as Record<string, unknown>;
  const images = manifest.qualifiedImages as Array<Record<string, unknown>>;
  images[0].image = 'public.ecr.aws/example/workspace:latest';
  assert.throws(() => validateManifest(manifest), /image is invalid/);

  const withUndeclaredEvidence = structuredClone(loadManifest()) as unknown as Record<
    string,
    unknown
  >;
  const qualified = withUndeclaredEvidence.qualifiedImages as Array<Record<string, unknown>>;
  qualified[0].evidenceUrl = 'https://example.invalid/qualification-run';
  assert.throws(() => validateManifest(withUndeclaredEvidence), /keys must be/);
});

test('catalog distinguishes source mappings from deployment qualification', () => {
  const catalog = renderCatalog(loadManifest(), loadSourceFacts());
  assert.match(catalog, /ghcr\.io\/trycua\/linux:24\.04.*Image\.linux\(\)` default/);
  assert.match(catalog, /ghcr\.io\/trycua\/windows:2022/);
  assert.match(catalog, /Deployment qualification recorded boot and readiness/);
  assert.match(catalog, /server:8000/);
  assert.match(catalog, /mcp:3000/);
  assert.match(catalog, /embedded Driver 0\.22\.2/);
  assert.match(catalog, /direct Driver qualification used 0\.26\.1/);
});

test('generated section replacement is deterministic and requires markers', () => {
  const input = 'before\n{/* GENERATED:demo:start */}\nold\n{/* GENERATED:demo:end */}\nafter\n';
  const once = replaceGeneratedSection(input, 'demo', 'new');
  assert.equal(replaceGeneratedSection(once, 'demo', 'new'), once);
  assert.throws(() => replaceGeneratedSection('unmarked', 'demo', 'new'), /Expected one/);
});

test('the shared image list matches canonical.rs and hides unpublished images', () => {
  const source = loadSourceFacts();
  const list = loadImageList(source);
  const shown = pickerImages(list).map((i) => i.ref);
  assert.ok(shown.includes(source.linuxImage));
  assert.ok(shown.includes(`${source.linuxImage}-disk`));
  for (const image of list.images.filter((i) => !i.published)) {
    assert.ok(!shown.includes(image.ref));
    assert.ok(!renderImageList(list).includes(image.ref));
  }
  for (const ref of shown) assert.ok(renderImageList(list).includes(`\`${ref}\``));

  const drifted = structuredClone(list);
  drifted.images[0].ref = 'ghcr.io/trycua/linux:22.04';
  assert.throws(() => validateImageList(drifted, source), /disagree with/);

  const extra = structuredClone(list) as unknown as { images: Array<Record<string, unknown>> };
  extra.images[0].color = 'blue';
  assert.throws(() => validateImageList(extra, source), /has fields/);

  const guided = structuredClone(list) as unknown as { images: Array<Record<string, unknown>> };
  guided.images[0].guide = '/cua-bench/guides/adapter-benchmarks#bench-web';
  validateImageList(guided, source);
  guided.images[0].guide = 'https://example.com';
  assert.throws(() => validateImageList(guided, source), /not a docs path/);
});

test('tiers match their tags and list every canonical tier with its status', () => {
  const source = loadSourceFacts();
  const list = loadImageList(source);
  const clone = () => JSON.parse(JSON.stringify(list));
  const wrong = clone();
  wrong.images.find((i: { ref: string }) => i.ref.endsWith('linux:24.04-slim')).tier = 'full';
  assert.throws(() => validateImageList(wrong, source), /does not match tier/);
  const xcodeLinux = clone();
  xcodeLinux.images.find((i: { ref: string }) => i.ref.endsWith('linux:24.04')).tier = 'xcode';
  assert.throws(() => validateImageList(xcodeLinux, source), /macOS only/);
  const benchTier = clone();
  benchTier.images.find((i: { group: string }) => i.group === 'benchmark').tier = 'slim';
  assert.throws(() => validateImageList(benchTier, source), /canonical image/);
  const tiers = renderTiers(list);
  assert.match(tiers, /`ghcr\.io\/trycua\/linux:24\.04-slim` \| slim \| `Image\.linux\(tier="slim"\)`/);
  assert.match(tiers, /macos:26-xcode` \| xcode/);
  assert.doesNotMatch(tiers, /-disk`/);
});

test('benchmark entries carry their lock and benchmarks; others carry neither', () => {
  const source = loadSourceFacts();
  const list = loadImageList(source);
  for (const image of list.images.filter((i) => i.group === 'benchmark')) {
    assert.ok(image.lock, `${image.ref} has a lock`);
  }
  const bad = structuredClone(list) as unknown as { images: Array<Record<string, unknown>> };
  bad.images[0].lock = 'libs/images/bench/web/lock.json';
  assert.throws(() => validateImageList(bad, source), /only benchmark images/);
  const benchIndex = list.images.findIndex((i) => i.group === 'benchmark');
  const wrongLock = structuredClone(list) as unknown as { images: Array<Record<string, unknown>> };
  wrongLock.images[benchIndex].lock = 'libs/images/bench/does-not-exist/lock.json';
  assert.throws(() => validateImageList(wrongLock, source), /existing lock/);
  const badBench = structuredClone(list) as unknown as { images: Array<Record<string, unknown>> };
  badBench.images[benchIndex].benchmarks = [{ name: 'x', what: 'y', tasks: 'many', guide: '/a' }];
  assert.throws(() => validateImageList(badBench, source), /invalid benchmark/);
});

test('catalog regions render published Cua images and every benchmark with its guide', () => {
  const list = loadImageList(loadSourceFacts());
  const images = renderCuaImages(list);
  for (const image of pickerImages(list).filter((i) => i.group !== 'benchmark')) {
    assert.ok(images.includes(`\`${image.ref}\``));
  }
  assert.ok(!images.includes('windows:2022-disk'), 'unpublished variants stay out');
  const benches = renderBenchmarks(list);
  for (const image of list.images.filter((i) => i.group === 'benchmark')) {
    for (const b of image.benchmarks ?? []) assert.ok(benches.includes(`[${b.name}](${b.guide})`));
  }
  const page =
    'a\n{/* GENERATED:catalog-benchmarks:start */}\nstale\n{/* GENERATED:catalog-benchmarks:end */}\nb\n';
  const filled = fillCatalogRegions(page, list);
  assert.equal(fillCatalogRegions(filled, list), filled);
  assert.ok(filled.includes(benches) && !filled.includes('stale'));
  assert.throws(
    () => fillCatalogRegions('{/* GENERATED:catalog-cua-images:start */}', list),
    /no end marker/
  );
});

test('every page with catalog regions is a watch path of the sandbox generator', () => {
  const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'config.json'), 'utf8')) as {
    generators: Record<string, { watchPaths: string[] }>;
  };
  const watched = new Set(config.generators.sandbox.watchPaths);
  const root = path.resolve(__dirname, '../..');
  for (const file of catalogRegionDocuments(loadImageList(loadSourceFacts())).keys()) {
    // config.json watch paths are POSIX; path.relative uses \ on Windows.
    const rel = path.relative(root, file).split(path.sep).join('/');
    assert.ok(watched.has(rel), `${rel} is watched`);
  }
});

test('the image list region is terse: headings per OS, one ref per bullet', () => {
  const list = loadImageList(loadSourceFacts());
  const bullets = renderImageBullets(list);
  for (const heading of ['### Linux', '### Windows', '### macOS', '### Adapter benchmark images']) {
    assert.ok(bullets.includes(heading), heading);
  }
  for (const image of pickerImages(list).filter((i) => i.group === 'canonical')) {
    assert.ok(bullets.includes(`- \`${image.ref}\``), image.ref);
  }
  for (const image of list.images.filter((i) => i.group === 'benchmark')) {
    assert.ok(bullets.includes(`- \`${image.ref}\``), image.ref);
  }
  assert.ok(bullets.includes('`ghcr.io/trycua/linux:24.04-disk` (VM)'));
  assert.ok(bullets.includes('`ghcr.io/trycua/macos:26` (Tahoe)'));
  assert.ok(
    bullets.includes(
      '`ghcr.io/trycua/bench-web:1.0` (run [MiniWoB++](/cua-bench/guides/adapter-benchmarks#miniwob), '
    ),
    'benchmark images link each benchmark they run'
  );
  assert.ok(!bullets.includes('windows:2022-disk'), 'unpublished Cua images stay out');
  for (const line of bullets.split('\n').filter((l) => l.startsWith('- '))) {
    // Measure what the reader sees: link targets don't count.
    const visible = line.replace(/\]\([^)]*\)/g, ']');
    assert.ok(visible.length <= 130, `bullet too long: ${visible}`);
    assert.ok(!line.includes('cua sb create'), `no per-bullet commands: ${line}`);
  }

  const bad = structuredClone(list) as unknown as { images: Array<Record<string, unknown>> };
  bad.images[0].arch = ['riscv'];
  assert.throws(() => validateImageList(bad, loadSourceFacts()), /arch must list/);
});
