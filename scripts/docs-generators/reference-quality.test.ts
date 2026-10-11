/**
 * Mechanical checks on the committed CLI and MCP references of cua, cua-driver,
 * lume and cb (the pages the shared renderers write):
 *
 * - every page carries the generator marker and has unique anchors;
 * - no page exceeds 5,000 words (docs.cua.ai eager-imports every page);
 * - no placeholder leaks (`undefined`, `[object Object]`) or em dashes;
 * - every visible command in the CLI spec has a section on its group page and
 *   at least one example (tagged for the cli-shape lane) somewhere on that page.
 */

import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { anchors, assertUniqueAnchors } from './cli-mdx.test';
import { type CLIDocumentation, type CommandDoc, commandAnchor } from './lib/cli-mdx';
import { DOCS_CONTENT, GENERATED_MARKER } from './lib/mdx';

const PRODUCTS: Array<[product: string, spec: string]> = [
  ['cua-cli', 'cua'],
  ['cua-driver', 'cua-driver'],
  ['lume', 'lume'],
  ['cua-bench', 'cb'],
];

function walk(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    return statSync(full).isDirectory() ? walk(full) : [full];
  });
}

function words(page: string): number {
  return page.replace(/<span id="[^"]*"><\/span>/g, '').split(/\s+/).filter(Boolean).length;
}

for (const [product, spec] of PRODUCTS) {
  const dir = join(DOCS_CONTENT, product, 'reference');
  const pages = walk(dir).filter((f) => f.endsWith('.mdx'));

  test(`${product} reference pages are generated, sane and uniquely anchored`, () => {
    assert.ok(pages.length > 2, `${product} has no generated reference`);
    for (const file of pages) {
      const page = readFileSync(file, 'utf-8');
      const rel = file.slice(DOCS_CONTENT.length + 1);
      assert.ok(page.slice(0, 600).includes(GENERATED_MARKER), `${rel} lacks the marker`);
      assert.ok(words(page) <= 5000, `${rel} has ${words(page)} words (limit 5000)`);
      assert.ok(!/\bundefined\b|\[object Object\]|—/.test(page), `${rel} leaks a placeholder or an em dash`);
      assertUniqueAnchors(rel, page);
    }
  });

  test(`${product}: every command has a section and an example`, () => {
    const cli: CLIDocumentation = JSON.parse(
      readFileSync(join(__dirname, 'cli-specs', `${spec}.json`), 'utf-8')
    );
    const groupPages = pages.filter((f) => f.includes(`${join('reference', 'cli')}`) && !f.endsWith('index.mdx'));
    const text = groupPages.map((f) => readFileSync(f, 'utf-8')).join('\n');
    const ids = new Set(groupPages.flatMap((f) => anchors(readFileSync(f, 'utf-8'))));
    const missing: string[] = [];
    const noExample: string[] = [];
    const visit = (cmd: CommandDoc, parents: string[]) => {
      if (cmd.hidden) return;
      const path = [...parents, cmd.name];
      const anchor = commandAnchor(path);
      if (!ids.has(anchor)) missing.push(path.join(' '));
      const subs = cmd.subcommands.filter((s) => !s.hidden);
      if (!subs.length && !text.includes(`id="ref-${anchor}"`)) noExample.push(path.join(' '));
      for (const sub of subs) visit(sub, path);
    };
    for (const cmd of cli.commands) visit(cmd, [cli.name]);
    assert.deepEqual(missing, [], `${product}: commands without a section`);
    assert.deepEqual(noExample, [], `${product}: leaf commands without an example`);
  });
}
