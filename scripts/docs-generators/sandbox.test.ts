import assert from 'node:assert/strict';
import test from 'node:test';

import {
  loadManifest,
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
  assert.match(catalog, /Image\.linux\(\).*mapping/);
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
