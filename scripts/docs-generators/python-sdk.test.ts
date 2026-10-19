import assert from 'node:assert/strict';
import test from 'node:test';

import {
  fenceIndentedCode,
  SANDBOX_PAGES,
  assertCoverage,
  mdxProse,
  paramTable,
  renderObject,
  renderReexports,
  sandboxAnchors,
  type PyDoc,
  type PyModuleDoc,
} from './python-sdk';

const module = (all: string[], imports: Record<string, string> = {}): PyModuleDoc => ({
  kind: 'module',
  name: 'cua_sandbox',
  path: 'cua_sandbox',
  all,
  lazy: [],
  aliases: [],
  imports,
});

test('prose is MDX-escaped outside fences and left alone inside them', () => {
  const out = mdxProse('Use {a} <b> and `{c}`.\n\n```python\nx = {1: 2}  # <tag>\n```\nafter {d}');
  assert.equal(
    out,
    'Use &#123;a&#125; &lt;b&gt; and `{c}`.\n\n```python\nx = {1: 2}  # <tag>\n```\nafter &#123;d&#125;'
  );
});

test('an unterminated fence is closed so the page still compiles', () => {
  assert.equal(mdxProse('```python\nx = 1'), '```python\nx = 1\n```');
});

test('classes render a signature fence, attributes table and one heading per method', () => {
  const doc: PyDoc = {
    kind: 'class',
    name: 'Image',
    bases: [],
    description: 'Immutable image.',
    members: [
      {
        kind: 'function',
        name: '__init__',
        signature: 'def Image(os_type: str) -> None',
        labels: [],
      },
      { kind: 'attribute', name: 'os_type', type: 'str', description: 'The OS | family.' },
      {
        kind: 'function',
        name: 'linux',
        signature: "def linux(version: str = '24.04') -> Image",
        labels: ['staticmethod'],
        description: 'Linux.',
        params: [{ name: 'version', type: 'str', default: "'24.04'", description: 'The release.' }],
      },
    ],
  };
  const text = renderObject(doc).join('\n');
  assert.match(text, /^### Image$/m);
  assert.match(text, /```python\nclass Image\n\nImage\(os_type: str\)\n```/);
  assert.match(text, /\| `os_type` \| `str` \| The OS \\\| family\. \|/);
  assert.match(text, /^#### Image\.linux$/m);
  assert.match(
    text,
    /```python\n@staticmethod\ndef linux\(version: str = '24\.04'\) -> Image\n```/
  );
  assert.match(text, /\| `version` \| `str` \| `'24.04'` \| The release\. \|/);
  assert.doesNotMatch(renderObject(doc, { hideInit: true }).join('\n'), /Image\(os_type/);
});

test('every page section maps to a unique anchor on its page', () => {
  const anchors = sandboxAnchors();
  assert.equal(anchors.get('Image'), '/cua-sdk/reference/python/image#image');
  assert.equal(anchors.get('Pool'), '/cua-sdk/reference/python/pool#pool');
  assert.equal(anchors.get('Shell'), '/cua-sdk/reference/python/interfaces#shell');
  const names = SANDBOX_PAGES.flatMap((p) =>
    p.sections.flatMap((s) => s.objects.map((o) => o.split('.').at(-1)))
  );
  assert.equal(new Set(names).size, names.length, 'object names must be unique across pages');
  // Section titles must not collide with object anchors on the same page.
  for (const page of SANDBOX_PAGES) {
    const objects = new Set(
      page.sections.flatMap((s) => s.objects.map((o) => o.split('.').at(-1)!.toLowerCase()))
    );
    for (const section of page.sections)
      assert.ok(!objects.has(section.title.toLowerCase()), section.title);
  }
});

test('coverage fails on an undocumented public name and tolerates fleet_sdk re-exports', () => {
  assert.doesNotThrow(() =>
    assertCoverage({
      objects: [],
      modules: [module(['Image', 'VmTemplate'], { VmTemplate: 'fleet_sdk' })],
    })
  );
  assert.throws(
    () => assertCoverage({ objects: [], modules: [module(['Image', 'Brand New'])] }),
    /Brand New/
  );
});

test('re-exports are grouped by source module', () => {
  const text = renderReexports(
    module(['Image', 'B', 'A'], { A: 'fleet_sdk', B: 'fleet_sdk' }),
    new Set(['Image'])
  );
  assert.match(text, /\| `fleet_sdk` \| `A`, `B` \|/);
  assert.equal(renderReexports(module(['Image']), new Set(['Image'])), '');
});

test('indented and doctest docstring code becomes fenced python', () => {
  assert.equal(fenceIndentedCode('Use it:\n\n    x = 1\n    y = 2\n\nDone.'), 'Use it:\n\n```python\nx = 1\ny = 2\n```\n\nDone.');
  assert.equal(fenceIndentedCode('>>> f(1)\n2\n\nText.'), '```python\nf(1)\n2\n```\n\nText.');
  assert.equal(fenceIndentedCode('- item\n\n    continued'), '- item\n\n    continued');
  assert.equal(fenceIndentedCode('```\n    kept\n```'), '```\n    kept\n```');
});

test('parameter tables carry a Default column: required or the literal default', () => {
  const rows = paramTable([
    { name: 'image', type: 'str', default: 'required', description: 'Image ref.' },
    { name: 'local', type: 'bool', default: 'False', description: 'Run locally.' },
    { name: 'region', type: 'str', default: "'us-east-1'", description: 'Region.' },
  ]);
  assert.equal(rows[0], '| Parameter | Type | Default | Description |');
  assert.equal(rows[2], '| `image` | `str` | required | Image ref. |');
  assert.equal(rows[3], '| `local` | `bool` | `False` | Run locally. |');
  assert.equal(rows[4], "| `region` | `str` | `'us-east-1'` | Region. |");
});
