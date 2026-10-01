import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import test from 'node:test';

import {
  PAGES,
  assign,
  cleanSignature,
  declaration,
  dedupe,
  docMarkdown,
  renderAll,
  shortValue,
  summary,
  type Dump,
  type ItemDoc,
} from './rust-crates';

const dump: Dump = JSON.parse(
  fs.readFileSync(path.join(__dirname, 'fixtures', 'rust-crates-dump.json'), 'utf-8')
);

test('every item lands on exactly one page of its crate', () => {
  const pages = assign(dump);
  const placed = [...pages.values()].flat().map((i) => i.path).sort();
  const all = dump.crates.flatMap((c) => c.items.map((i) => i.path)).sort();
  assert.deepEqual(placed, all);
  const slugOf = (p: string) => [...pages].find(([, items]) => items.some((i) => i.path === p))![0];
  assert.equal(slugOf('cua_sdk::Sandbox'), 'cua-sdk');
  // Unclaimed sources fall back to the crate's primary page.
  assert.equal(slugOf('cua_sdk::Stray'), 'cua-sdk');
  assert.equal(slugOf('cua_spaces::client::model::Space'), 'cua-spaces-client');
  assert.equal(slugOf('cua_spaces::mcp'), 'cua-spaces-agents-mcp');
});

test('a crate without a page is an error, not silently dropped', () => {
  const extra = structuredClone(dump);
  extra.crates.push({ name: 'cua-new', lib: 'cua_new', docs: '', items: [] });
  assert.throws(() => assign(extra), /no reference page for crate cua-new/);
});

test('rustdoc Markdown becomes MDX-safe Markdown', () => {
  const md = docMarkdown(
    '# Examples\n\nSee [`Cua`] and [text][`Sandbox`] at <https://cua.ai>, `Vec<u8>` or Vec<u8> {x}.\n\n[`Cua`]: crate::Cua\n\n```no_run\n# use hidden;\nlet x = 1;\n```'
  );
  assert.equal(
    md,
    '**Examples**\n\nSee `Cua` and text at https://cua.ai, `Vec<u8>` or Vec&lt;u8&gt; &#123;x&#125;.\n\n\n```rust\nlet x = 1;\n```'
  );
});

test('rendered pages are deterministic, bannered, compact and listed in meta.json', () => {
  const out = '/virtual/rust';
  const uniffi = new Map([['Sandbox', '/cua-sdk/reference/sandbox#sandbox']]);
  const a = renderAll(dump, '9.9.9', out, uniffi);
  const b = renderAll(dump, '9.9.9', out, uniffi);
  assert.deepEqual([...a], [...b]);
  const meta = JSON.parse(a.get(`${out}/meta.json`)!);
  assert.deepEqual(meta.pages, PAGES.map((p) => p.slug));
  const index = a.get(`${out}/index.mdx`)!;
  assert.match(index, /AUTO-GENERATED FILE/);
  assert.match(index, /\| `cua-sdk` \|[^\n]*\[cua-sdk\]\(\/cua-sdk\/reference\/rust\/cua-sdk\)[,)| ]/);
  const sdk = a.get(`${out}/cua-sdk.mdx`)!;
  assert.match(sdk, /AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY/);
  assert.match(sdk, /Version: 9\.9\.9/);
  assert.match(sdk, /nightly-2026-09-20, format 61/);
  // A type: heading (anchor), summary line, one declaration block with its methods.
  assert.match(sdk, /### `Cua`\n\n[^\n]+\n\n```rust\npub struct Cua \{ \/\* private fields \*\/ \}\nimpl Cua \{\n  fn connect\(address: Option<String>\) -> Result<Arc<Self>>;\n\}\n```/);
  assert.ok(!sdk.includes('**Implements:**'), 'trait impl lists are omitted');
  assert.match(sdk, /#\[deprecated = "since 0\.2\.0: renamed to SpacesdClient"\]\n\/\/\/ Old name for `Vec<u8>` \{x\}\.\npub type OldClient = SpacesdClient;/);
  // UniFFI-exported items link to the cua SDK API reference instead of repeating it.
  assert.match(sdk, /## Exported API/);
  assert.match(sdk, /\[`Sandbox`\]\(\/cua-sdk\/reference\/sandbox#sandbox\)/);
  assert.ok(!sdk.includes('### `Sandbox`'));
  for (const [file, content] of a) {
    if (!file.endsWith('.mdx')) continue;
    assert.ok(!content.includes('\u2014'), `${file}: em dash`);
    const outside = content.replace(/```[\s\S]*?```/g, '');
    assert.ok(!/^# /m.test(outside), `${file}: body H1`);
  }
});

test('summaries are the first sentence, capped', () => {
  assert.equal(summary('Connects to a [`Cua`] daemon. More text here.\n\nSecond para.'), 'Connects to a `Cua` daemon.');
  assert.equal(summary('```rust\ncode\n```'), '');
  const long = summary('word '.repeat(80), 40);
  assert.ok(long.length <= 44 && long.endsWith(' ...'), long);
  assert.equal(summary('Uses `unbalanced code', 200), 'Uses unbalanced code');
});

test('async_trait signatures read as async fn; long constants are elided', () => {
  assert.equal(
    cleanSignature(
      "pub fn probe<'life0, 'life1, 'async_trait>(&'life0 self, token: &'life1 str) -> Pin<Box<dyn Future<Output = TokenState> + Send + 'async_trait>> where Self: 'async_trait, 'life0: 'async_trait, 'life1: 'async_trait"
    ),
    'pub async fn probe(&self, token: &str) -> TokenState'
  );
  assert.equal(cleanSignature('pub fn plain(x: u8) -> u8'), 'pub fn plain(x: u8) -> u8');
  assert.equal(shortValue('pub const A: &str = "short";'), 'pub const A: &str = "short";');
  assert.equal(shortValue(`pub const B: &str = "${'x'.repeat(60)}";`), 'pub const B: &str = /* ... */;');
});

test('an item reachable through several paths is documented once', () => {
  const item = (path: string): ItemDoc => ({
    path, module: 'm', name: 'Opts', kind: 'struct', source: 'src/a.rs', signature: 'pub struct Opts', docs: '',
  });
  const out = dedupe([item('c::autopool::Opts'), item('c::Opts')]);
  assert.equal(out.length, 1);
  assert.equal(out[0].path, 'c::Opts');
  assert.deepEqual(out[0].aliases, ['c::autopool::Opts']);
});

test('fields and variants are declared inside one block', () => {
  const block = declaration({
    path: 'c::E', module: 'c', name: 'E', kind: 'enum', source: 'src/e.rs', signature: 'pub enum E', docs: '',
    variants: [{ name: 'A', signature: 'A(String)', docs: 'The A case. Details.' }],
  });
  assert.equal(block, '```rust\npub enum E {\n  A(String),\n}\n```');
});
