import assert from 'node:assert/strict';
import * as path from 'node:path';
import test from 'node:test';

import {
  causeAndFix,
  errorMessages,
  pageProblems,
  placeItems,
  type PageSpec,
  langType,
  lowerCamel,
  parseRenames,
  renderAll,
  renderDoc,
  renamed,
  shoutySnake,
  signature,
  SPACES_NOTE,
  verifyBindingNames,
  type SdkApi,
  type SdkCallable,
} from './cua-sdk';

const exec: SdkCallable = {
  name: 'exec',
  docstring: 'Runs a command. See [`Sandbox::spacesd`].',
  async: true,
  arguments: [
    { name: 'command', type: { kind: 'sequence', inner: { kind: 'string' } }, default: null },
    {
      name: 'timeout_ms',
      type: { kind: 'optional', inner: { kind: 'u64' } },
      default: { kind: 'none' },
    },
  ],
  return_type: { kind: 'record', name: 'ExecResult' },
  throws: { kind: 'enum', name: 'CuaError' },
};

function fixture(): SdkApi {
  return {
    namespace: 'cua_sdk',
    docstring: null,
    objects: [
      {
        name: 'Sandbox',
        module: 'sandbox',
        docstring: 'A sandbox handle.',
        trait_interface: false,
        constructors: [],
        methods: [
          exec,
          { name: 'delete', docstring: null, async: true, arguments: [], return_type: null, throws: exec.throws },
          { name: 'close', docstring: null, async: true, arguments: [], return_type: null, throws: null },
        ],
      },
      {
        name: 'FrameSink',
        module: 'media',
        docstring: 'Receives frames.',
        trait_interface: true,
        constructors: [],
        methods: [],
      },
    ],
    records: [
      {
        name: 'ExecResult',
        module: 'types',
        docstring: 'The result.',
        fields: [
          { name: 'exit_code', type: { kind: 'i32' }, default: null, docstring: 'Exit code.' },
          { name: 'stdout', type: { kind: 'bytes' }, default: null, docstring: 'Output | raw.' },
        ],
        methods: [],
      },
    ],
    enums: [
      {
        name: 'CuaError',
        module: 'root',
        docstring: 'Errors.',
        error: true,
        flat: true,
        non_exhaustive: false,
        variants: [{ name: 'NotFound', docstring: 'Missing.', fields: [] }],
        methods: [],
      },
      {
        name: 'SandboxPhase',
        module: 'sandbox',
        docstring: null,
        error: false,
        flat: true,
        non_exhaustive: false,
        variants: [{ name: 'Running', docstring: null, fields: [] }],
        methods: [],
      },
    ],
    callback_interfaces: [],
    functions: [
      {
        name: 'cua_sdk_version',
        module: 'root',
        docstring: 'The version.',
        async: false,
        arguments: [],
        return_type: { kind: 'string' },
        throws: null,
      },
    ],
  };
}

const ctx = {
  traits: new Set(['FrameSink']),
  errors: new Set(['CuaError']),
  renames: parseRenames('[bindings.kotlin.rename]\n"Sandbox.close" = "closeSandbox"\n'),
};

test('names follow the UniFFI (heck) conventions', () => {
  assert.equal(lowerCamel('open_media_decoded_with_audio'), 'openMediaDecodedWithAudio');
  assert.equal(lowerCamel('bundle_sha256'), 'bundleSha256');
  assert.equal(shoutySnake('SpacesdNotAvailable'), 'SPACESD_NOT_AVAILABLE');
  assert.equal(shoutySnake('AptInstall'), 'APT_INSTALL');
  assert.equal(renamed('TypeScript', 'Sandbox', 'delete', ctx.renames), 'delete_');
  assert.equal(renamed('Kotlin', 'Sandbox', 'close', ctx.renames), 'closeSandbox');
  assert.equal(renamed('Swift', 'Sandbox', 'close', ctx.renames), 'close');
});

test('types render per language', () => {
  const t = { kind: 'optional', inner: { kind: 'sequence', inner: { kind: 'u64' } } } as const;
  assert.equal(langType('Python', t, ctx), 'Optional[List[int]]');
  assert.equal(langType('TypeScript', t, ctx), 'Array<bigint> | undefined');
  assert.equal(langType('Swift', t, ctx), '[UInt64]?');
  assert.equal(langType('Kotlin', t, ctx), 'List<ULong>?');
  assert.equal(langType('TypeScript', { kind: 'object', name: 'Sandbox' }, ctx), 'SandboxLike');
  assert.equal(langType('TypeScript', { kind: 'object', name: 'FrameSink' }, ctx), 'FrameSink');
  assert.equal(langType('Kotlin', { kind: 'enum', name: 'CuaError' }, ctx), 'CuaException');
});

test('signatures carry async, throws and defaults in each language', () => {
  const owner = { name: 'Sandbox', kind: 'object' as const };
  assert.equal(
    signature('Python', exec, owner, ctx, 'method'),
    'async def exec(self, command: List[str], timeout_ms: Optional[int] = None) -> ExecResult'
  );
  assert.equal(
    signature('TypeScript', exec, owner, ctx, 'method'),
    'exec(command: Array<string>, timeoutMs: bigint | undefined = undefined, asyncOpts_?: { signal: AbortSignal }): Promise<ExecResult>'
  );
  assert.equal(
    signature('Swift', exec, owner, ctx, 'method'),
    'func exec(command: [String], timeoutMs: UInt64? = nil) async throws -> ExecResult'
  );
  assert.equal(
    signature('Kotlin', exec, owner, ctx, 'method'),
    '@Throws(CuaException::class)\nsuspend fun exec(command: List<String>, timeoutMs: ULong? = null): ExecResult'
  );
});

test('docstrings: intra-doc links become code, headings become bold, MDX is escaped', () => {
  assert.equal(renderDoc('See [`Sandbox::spacesd`].'), 'See `Sandbox.spacesd`.');
  assert.equal(renderDoc('# Errors\nA <b> {x}'), '**Errors**\nA &lt;b&gt; &#123;x&#125;');
  assert.equal(renderDoc('```\nlet a = 1;\n```'), '```rust\nlet a = 1;\n```');
});

const PAGES_FIXTURE: PageSpec[] = [
  { slug: 'cua', title: 'Cua', description: 'Client.', intro: 'Intro.', items: ['cua_sdk_version'], modules: ['root'] },
  { slug: 'sandbox', title: 'Sandbox', description: 'Sandboxes.', intro: 'Intro.', items: ['Sandbox'], modules: ['sandbox'] },
  {
    slug: 'sandbox/life',
    title: 'Lifecycle',
    description: 'Lifecycle.',
    intro: 'Intro.',
    items: [],
    members: [{ object: 'Sandbox', title: 'Lifecycle methods', methods: ['delete'] }],
  },
  { slug: 'local', title: 'Local', description: 'Local.', intro: 'Intro.', items: [], modules: ['media', 'types'] },
];

const render = (api = fixture(), examples = new Map()) =>
  renderAll(api, '1.2.3', {
    renames: ctx.renames,
    outputDir: '/out',
    pages: PAGES_FIXTURE,
    examples,
    errorMessages: new Map([['NotFound', 'not found: <detail>']]),
  });

test('pages are deterministic, complete and use only the shimmed components', () => {
  const a = render();
  const b = render();
  assert.deepEqual([...a], [...b]);
  assert.deepEqual(
    [...a.keys()].map((f) => path.relative('/out', f)).sort(),
    [
      'cua.mdx',
      'errors.mdx',
      'index.mdx',
      'local.mdx',
      'meta.json',
      'sandbox/index.mdx',
      'sandbox/life.mdx',
      'sandbox/meta.json',
      'types.mdx',
    ]
  );
  const meta = JSON.parse(a.get('/out/meta.json')!);
  assert.ok(!meta.pages.includes('index'), 'a folder index is the folder link');
  assert.ok(meta.pages.includes('---Errors and types---'));
  assert.deepEqual(JSON.parse(a.get('/out/sandbox/meta.json')!).pages, ['life']);
  const sandbox = a.get('/out/sandbox/index.mdx')!;
  assert.match(sandbox, /AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY/);
  assert.match(sandbox, /Version: 1\.2\.3/);
  assert.match(sandbox, /^### `Sandbox\.exec`$/m);
  assert.match(sandbox, /<Tabs items=\{\['Python', 'TypeScript', 'Swift', 'Kotlin', 'Rust'\]\} groupId="sdk-lang" persist>/);
  assert.match(sandbox, /pub async fn exec\(&self, command: Vec<String>, timeout_ms: Option<u64>\) -> Result<ExecResult, CuaError>/);
  assert.match(sandbox, /\| `command` \| `Vec<String>` \| required \|/);
  assert.match(sandbox, /\| `timeout_ms` \| `Option<u64>` \| `None` \|/);
  assert.match(sandbox, /\*\*Returns\*\* \[`ExecResult`\]\(\/cua-sdk\/reference\/local#execresult-record\)/);
  // A method documented on another page is indexed on its object and rendered there.
  assert.match(sandbox, /\[`delete`\]\(\/cua-sdk\/reference\/sandbox\/life#sandboxdelete\)/);
  assert.doesNotMatch(sandbox, /^### `Sandbox\.delete`$/m);
  assert.match(a.get('/out/sandbox/life.mdx')!, /^### `Sandbox\.delete`$/m);
  assert.match(sandbox, /SandboxPhase\.RUNNING/);
  assert.match(sandbox, /^## `SandboxPhase` enum$/m);
  const media = a.get('/out/local.mdx')!;
  assert.match(media, /\| `exit_code` \/ `exitCode` \| `i32` \|/);
  assert.match(media, /Output \\\| raw\./);
  assert.match(media, /<Callout>/);
  const errors = a.get('/out/errors.mdx')!;
  assert.match(errors, /`CuaException`/);
  assert.match(errors, /^## `NotFound`$/m);
  assert.match(errors, /\*\*Message\*\* `not found: <detail>`/);
  // `CuaError.doc_url` links to `errors#<variant in lower case>`: the
  // heading anchors below, documented per language.
  assert.match(errors, /\| `e\.doc_url\(\)` \|/);
  assert.match(errors, /errors#<variant in lower case>/);
  assert.match(a.get('/out/cua.mdx')!, /^## `cua_sdk_version`$/m);
  assert.match(a.get('/out/types.mdx')!, /\[`Sandbox`\]\(\/cua-sdk\/reference\/sandbox#sandbox\)/);
  assert.deepEqual(pageProblems(a), []);
  for (const [file, content] of a) {
    for (const m of content.matchAll(/^import .* from '([^']+)';$/gm)) {
      assert.ok(
        ['fumadocs-ui/components/tabs', 'fumadocs-ui/components/callout'].includes(m[1]),
        `${file} imports ${m[1]}`
      );
    }
    assert.ok(!content.includes('\u2014'), `${file} has an em dash`);
  }
});

test('examples render as tested fences under their item, and unknown targets fail', () => {
  const ex = {
    target: 'Sandbox.exec',
    lang: 'Python',
    fence: 'python',
    attrs: { test: 'docs', prelude: 'spacesd' },
    code: 'print(1)\n',
    file: 'scripts/docs-generators/examples/cua-sdk/Sandbox.exec.py',
  };
  const out = render(fixture(), new Map([['Sandbox.exec', [ex]]])).get('/out/sandbox/index.mdx')!;
  assert.match(out, /```python test="docs" id="sandbox-exec-py" session="sandbox-exec-py" prelude="spacesd"\nprint\(1\)\n```/);
  assert.match(out, /Examples: scripts\/docs-generators\/examples\/cua-sdk\/Sandbox\.exec\.py/);
  assert.throws(() => render(fixture(), new Map([['Sandbox.nope', [ex]]])), /Sandbox\.nope/);
});

test('an unmapped module or unknown item fails loudly', () => {
  const api = fixture();
  api.records.push({ name: 'Orphan', module: 'brand_new', docstring: null, fields: [], methods: [] });
  assert.throws(() => placeItems(api, PAGES_FIXTURE), /brand_new.*PAGES/);
  const bad = [...PAGES_FIXTURE, { slug: 'x', title: 'X', description: 'x', intro: 'x', items: ['Nope'] }];
  assert.throws(() => placeItems(fixture(), bad), /Nope/);
});

test('prefixes place unlisted items: listed names first, then the longest prefix, then the module', () => {
  const page = (slug: string, extra: Partial<PageSpec>): PageSpec => ({ slug, title: slug, description: 'x', intro: 'x', items: [], ...extra });
  const pages: PageSpec[] = [
    page('cua', { items: ['cua_sdk_version'], modules: ['root'] }),
    page('sandbox', { prefixes: ['Sandbox'], modules: ['sandbox'] }),
    page('phase', { prefixes: ['SandboxP'] }),
    page('local', { prefixes: ['Exec'], modules: ['media', 'types'] }),
  ];
  const placed = placeItems(fixture(), pages);
  const names = (s: string) => placed.get(s)!.items.map((i) => i.name);
  assert.deepEqual(names('sandbox'), ['Sandbox']);
  assert.deepEqual(names('phase'), ['SandboxPhase']);
  assert.deepEqual(names('local'), ['FrameSink', 'ExecResult']);
  // A listed name beats every prefix, so a prefix left with nothing fails, as does one on two pages.
  const listed = [page('cua', { items: ['cua_sdk_version', 'ExecResult'], modules: ['root'] }), ...pages.slice(1)];
  assert.throws(() => placeItems(fixture(), listed), /`Exec` \(local\)/);
  assert.throws(() => placeItems(fixture(), [...pages, page('stale', { prefixes: ['Gone'] })]), /`Gone` \(stale\)/);
  assert.throws(() => placeItems(fixture(), [...pages, page('twice', { prefixes: ['Exec'] })]), /`Exec` is on local and twice/);
});

test('nested folders get their own meta.json; a catalog page lists its subtree instead of Types', () => {
  const pages: PageSpec[] = [
    PAGES_FIXTURE[0],
    { ...PAGES_FIXTURE[1], catalog: true },
    { ...PAGES_FIXTURE[2], items: ['SandboxPhase'] },
    { slug: 'sandbox/life/results', title: 'Results', description: 'Results.', intro: 'Intro.', items: [], prefixes: ['Exec'] },
    PAGES_FIXTURE[3],
  ];
  const out = renderAll(fixture(), '1.2.3', {
    renames: ctx.renames,
    outputDir: '/out',
    pages,
    examples: new Map(),
    errorMessages: new Map(),
  });
  assert.ok(out.has('/out/sandbox/life/index.mdx'));
  assert.ok(out.has('/out/sandbox/life/results.mdx'));
  assert.deepEqual(JSON.parse(out.get('/out/sandbox/meta.json')!).pages, ['life']);
  assert.deepEqual(JSON.parse(out.get('/out/sandbox/life/meta.json')!), { title: 'Lifecycle', pages: ['results'] });
  const home = out.get('/out/sandbox/index.mdx')!;
  assert.match(home, /^## All items$/m);
  // The index is grouped by page: one line of item links per page.
  assert.match(home, /\*\*Results\*\*: .*\[`ExecResult`\]\(\/cua-sdk\/reference\/sandbox\/life\/results#execresult-record\)/);
  assert.match(home, /\[`Sandbox`\]\(#sandbox\)/);
  const types = out.get('/out/types.mdx')!;
  assert.doesNotMatch(types, /\[`ExecResult`\]/);
  assert.doesNotMatch(types, /\[`Sandbox`\]/);
  assert.match(types, /\[`cua_sdk_version`\]/);
  assert.match(types, /The 3 items of Sandbox are listed on \[its page\]\(\/cua-sdk\/reference\/sandbox#all-items\)\./);
});

test('error docs split into cause and fix; messages come from #[error]', () => {
  assert.deepEqual(causeAndFix('Bad input.\n\nFix: Check it.'), { cause: 'Bad input.', fix: 'Check it.' });
  assert.deepEqual(causeAndFix('Bad input.'), { cause: 'Bad input.', fix: '' });
  const src = 'pub enum CuaError {\n    /// Bad.\n    #[error("invalid argument: {0}")]\n    InvalidArgument(String),\n}\n';
  assert.equal(errorMessages(src).get('InvalidArgument'), 'invalid argument: <detail>');
});

test('page checks catch duplicate anchors and oversized pages', () => {
  const files = new Map([['/out/a.mdx', '## `X`\n\n## `X`\n']]);
  assert.match(pageProblems(files)[0], /duplicate anchor #x/);
  assert.match(pageProblems(new Map([['/out/b.mdx', 'word '.repeat(20)]]), 10)[0], /20 words/);
});

test('binding cross-check reports names the bindings lack', () => {
  const api = fixture();
  api.objects = [api.objects[0]];
  api.records = [];
  api.enums = [];
  api.functions = [];
  api.objects[0].methods = [exec];
  const ok = {
    python: 'class Sandbox:\n    async def exec(',
    typescript: 'export class Sandbox extends X\n  exec(',
    swift: 'open class Sandbox: P\n  func exec(',
    kotlin: 'class Sandbox\n  fun `exec`(',
  };
  assert.deepEqual(verifyBindingNames(api, ok), []);
  const problems = verifyBindingNames(api, { ...ok, swift: 'open class Sandbox: P\n' });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /Swift: Sandbox\.exec/);
});

/** A Cua Spaces app export dump: an object and a function that takes an SDK object. */
function spacesFixture(): SdkApi {
  const open = (): SdkCallable => ({
    name: 'open_app',
    docstring: 'Opens an app.',
    async: true,
    arguments: [{ name: 'sandbox', type: { kind: 'object', name: 'Sandbox' }, default: null }],
    return_type: null,
    throws: { kind: 'enum', name: 'CuaError' },
  });
  return {
    namespace: 'cua_spaces_ffi',
    docstring: null,
    objects: [{ name: 'Porter', module: 'teleport', docstring: 'Moves apps.', trait_interface: false, constructors: [], methods: [open()] }],
    records: [],
    enums: [
      {
        name: 'PorterMode',
        module: 'teleport',
        docstring: null,
        error: false,
        flat: true,
        non_exhaustive: false,
        variants: [{ name: 'AppOnly', docstring: null, fields: [] }],
        methods: [],
      },
    ],
    callback_interfaces: [],
    functions: [
      {
        name: 'porter_for',
        module: 'teleport',
        docstring: 'The porter.',
        async: false,
        arguments: [],
        return_type: { kind: 'object', name: 'Porter' },
        throws: null,
      },
    ],
  };
}

const SPACES_PAGE: PageSpec = { slug: 'sandbox/porter', title: 'Porter', description: 'Porting.', intro: 'Intro.', items: [], modules: ['teleport'], spaces: true };

test('app export pages: Swift and Rust only, the FSL note, linked into the SDK and off the Types list', () => {
  const out = renderAll(fixture(), '1.2.3', {
    renames: ctx.renames,
    outputDir: '/out',
    pages: [...PAGES_FIXTURE, SPACES_PAGE],
    examples: new Map(),
    errorMessages: new Map(),
    spaces: spacesFixture(),
    spacesRenames: {},
  });
  const page = out.get('/out/sandbox/porter.mdx')!;
  assert.ok(page.includes(SPACES_NOTE));
  assert.match(SPACES_NOTE, /source-available under FSL-1\.1-MIT/);
  assert.match(page, /cua-bindgen docs --namespace cua_spaces_ffi/);
  assert.match(page, /<Tabs items=\{\['Swift', 'Rust'\]\} groupId="sdk-lang" persist>/);
  assert.doesNotMatch(page, /'Python'|'TypeScript'|'Kotlin'|```python|```ts|```kotlin/);
  assert.match(page, /func openApp\(sandbox: Sandbox\) async throws/);
  assert.match(page, /pub fn porter_for\(\) -> Arc<Porter>/);
  assert.match(page, /\| `sandbox` \| \[`Sandbox`\]\(\/cua-sdk\/reference\/sandbox#sandbox\) \| required \|/);
  assert.match(page, /Returned by \[`porter_for`\]\(#porter_for\)/);
  // SDK pages keep every language.
  assert.match(out.get('/out/sandbox/index.mdx')!, /<Tabs items=\{\['Python', 'TypeScript', 'Swift', 'Kotlin', 'Rust'\]\}/);
  const types = out.get('/out/types.mdx')!;
  assert.doesNotMatch(types, /\[`Porter`\]/);
  assert.match(types, /The 3 items of the Cua Spaces app export \(source-available, FSL-1\.1-MIT, Swift only\) are listed on their pages: \[Porter\]\(\/cua-sdk\/reference\/sandbox\/porter\)\./);
  assert.match(out.get('/out/index.mdx')!, /\| \[Porter\]\(\/cua-sdk\/reference\/sandbox\/porter\) \| Porting\. \*Cua Spaces app export\.\* \|/);
  assert.deepEqual(pageProblems(out), []);
});

test('app export items stay on app export pages, and SDK items off them', () => {
  const pages = [...PAGES_FIXTURE, SPACES_PAGE];
  assert.throws(() => placeItems(fixture(), pages), /document the Cua Spaces app export, but no cua_spaces_ffi dump/);
  const placed = placeItems(fixture(), pages, spacesFixture());
  assert.deepEqual(placed.get('sandbox/porter')!.items.map((i) => [i.name, i.origin]), [
    ['Porter', 'spaces'],
    ['PorterMode', 'spaces'],
    ['porter_for', 'spaces'],
  ]);
  assert.equal(placed.get('sandbox')!.items[0].origin, 'sdk');
  // An SDK item named on an app export page is not in that dump, and an app export module no app export page owns fails.
  assert.throws(() => placeItems(fixture(), [...PAGES_FIXTURE, { ...SPACES_PAGE, items: ['Sandbox'] }], spacesFixture()), /`Sandbox` is not exported by the Cua Spaces app export/);
  assert.throws(() => placeItems(fixture(), [...PAGES_FIXTURE, { ...SPACES_PAGE, modules: [] }], spacesFixture()), /Porter \(the Cua Spaces app export \(cua_spaces_ffi\)\) is declared in module `teleport`/);
});

test('the app export is checked against its Swift binding and Rust source only', () => {
  const api = spacesFixture();
  const swift = 'open class Porter: P\n  func openApp(\npublic enum PorterMode\n    case appOnly\nfunc porterFor(';
  const rust = 'pub struct Porter\npub async fn open_app(\npub enum PorterMode {\n    AppOnly,\n}\npub fn porter_for(';
  assert.deepEqual(verifyBindingNames(api, { swift, rust }), []);
  const problems = verifyBindingNames(api, { swift: swift.replace('func porterFor(', ''), rust });
  assert.deepEqual(problems, ['Swift: (function).porter_for (expected /func porterFor\\(/)']);
});
