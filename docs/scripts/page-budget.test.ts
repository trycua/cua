// Every docs page compiles and renders within the dev server's memory. Large
// generated pages (hundreds of highlighted blocks, many Tabs) exhausted a 4 GB
// heap in `next dev` (the generated UniFFI API pages: 50 KB, 440 fences).
// Split such pages by object or command group instead of raising the limits.
//
//   pnpm exec tsx --test scripts/page-budget.test.ts
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const content = path.resolve(__dirname, '../content/docs');
export const PAGE_BUDGET = { bytes: 100_000, fences: 150, tabs: 30 };

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    return statSync(full).isDirectory() ? walk(full) : full.endsWith('.mdx') ? [full] : [];
  });
}

test('every page stays within the page budget', () => {
  const over: string[] = [];
  for (const file of walk(content)) {
    const text = readFileSync(file, 'utf8');
    const fences = (text.match(/^\s*```/gm) ?? []).length / 2;
    const tabs = (text.match(/<Tabs\b/g) ?? []).length;
    const bytes = Buffer.byteLength(text);
    const rel = path.relative(content, file);
    if (bytes > PAGE_BUDGET.bytes) over.push(`${rel}: ${bytes} bytes > ${PAGE_BUDGET.bytes}`);
    if (fences > PAGE_BUDGET.fences) over.push(`${rel}: ${fences} code blocks > ${PAGE_BUDGET.fences}`);
    if (tabs > PAGE_BUDGET.tabs) over.push(`${rel}: ${tabs} <Tabs> > ${PAGE_BUDGET.tabs}`);
  }
  assert.deepEqual(over, [], 'split these pages (by object or command group) or fix their generator');
});
