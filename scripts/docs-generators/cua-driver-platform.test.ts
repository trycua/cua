import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import test from 'node:test';
import {
  extractDocumentation,
  loadSnapshots,
  mcpSnapshot,
  referenceFiles,
  referencePlatform,
  referencePlatforms,
  snapshotPath,
  undocumentedParameters,
  type DumpDocsOutput,
} from './cua-driver';
import { stableJson } from './lib/mdx';

const CLI_COMMANDS = [
  'mcp', 'mcp-config', 'list-tools', 'describe', 'call', 'manifest', 'dump-docs', 'serve', 'stop', 'status', 'sessions',
  'revoke', 'autostart', 'permissions', 'doctor', 'diagnose', 'recording', 'history', 'config', 'telemetry',
  'cursor-theme', 'skills', 'extension', 'perception', 'check-update', 'update', 'channel',
];

function docs(description = 'Inspect the native tree.'): DumpDocsOutput {
  return {
    cli: {
      name: 'cua-driver',
      version: '1.0.0',
      abstract: 'Shared CLI.',
      commands: CLI_COMMANDS.map((name) => ({ name, abstract: `${name}.`, arguments: [], options: [], flags: [], subcommands: [] })),
    },
    mcp: {
      version: '1.0.0',
      tools: [
        {
          name: 'get_window_state',
          description,
          input_schema: {
            type: 'object',
            required: ['pid'],
            properties: { pid: { type: 'integer', description: 'Native process.' } },
          },
        },
      ],
    },
  };
}

test('documentation extraction isolates both policy layers only in its metadata child', () => {
  const environment = Object.freeze({
    Path: 'native-toolchain',
    CARGO: 'cargo.exe',
    CUA_DRIVER_POLICY_FILE: 'restricted.yaml',
    cua_driver_policy_file: 'another-case.yaml',
    CuA_DrIvEr_MaNaGeD_PoLiCy_FiLe: 'managed.yaml',
  });
  const expected = docs();
  let calls = 0;
  const actual = extractDocumentation('native-driver', environment, (binary, args, options) => {
    calls++;
    assert.equal(binary, 'native-driver');
    assert.deepEqual(args, ['dump-docs', '--type', 'all', '--pretty']);
    assert.deepEqual(options.env, { Path: 'native-toolchain', CARGO: 'cargo.exe' });
    assert.equal(options.encoding, 'utf-8');
    return JSON.stringify(expected);
  });
  assert.equal(calls, 1);
  assert.deepEqual(actual, expected);
  assert.equal(environment.CUA_DRIVER_POLICY_FILE, 'restricted.yaml');
  assert.equal(environment.CuA_DrIvEr_MaNaGeD_PoLiCy_FiLe, 'managed.yaml');
});

test('maps native hosts and refuses unsupported hosts before generation', () => {
  assert.deepEqual(referencePlatform('darwin'), { host: 'darwin', key: 'macos', name: 'macOS' });
  assert.deepEqual(referencePlatform('win32').key, 'windows');
  assert.throws(() => referencePlatform('freebsd'), /not implemented/);
});

function specDir(t: { after: (fn: () => void) => void }): { out: string; specs: string } {
  const root = mkdtempSync(join(tmpdir(), 'cua-docs-driver-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const specs = join(root, 'specs');
  mkdirSync(specs);
  for (const p of referencePlatforms) {
    writeFileSync(snapshotPath(p, specs), stableJson(mcpSnapshot(docs(`${p.name} tree.`).mcp)));
  }
  return { out: join(root, 'reference'), specs };
}

test('each host refreshes only its own registry snapshot; pages merge all three', (t) => {
  const { out, specs } = specDir(t);
  const linux = referencePlatform('linux');
  const files = referenceFiles(docs('Fresh Linux tree.'), linux, out, specs);
  const snapshots = [...files.keys()].filter((f) => f.startsWith(specs)).map((f) => basename(f)).sort();
  assert.deepEqual(snapshots, ['cua-driver-mcp-linux.json', 'cua-driver.json']);
  const page = files.get(join(out, 'mcp-tools', 'window-state.mdx'))!;
  assert.ok(page.includes("<Tabs items={['macOS', 'Linux', 'Windows']}"));
  assert.ok(page.includes('Fresh Linux tree.') && page.includes('macOS tree.') && page.includes('Windows tree.'));
  // The shared parameter is listed once, outside the tabs.
  assert.ok(page.includes('<span id="get_window_state--pid"></span>`pid` | `integer` | required | Native process. |'));
});

test('pages are identical whichever host renders them', (t) => {
  const { out, specs } = specDir(t);
  const pages = (host: string) =>
    [...referenceFiles(docs(`${referencePlatform(host).name} tree.`), referencePlatform(host), out, specs)]
      .filter(([f]) => f.endsWith('.mdx'))
      .map(([f, c]) => `${f}\n${c}`)
      .join('\n');
  assert.equal(pages('darwin'), pages('linux'));
  assert.equal(pages('linux'), pages('win32'));
});

test('a tool without a docs category fails generation', (t) => {
  const { out, specs } = specDir(t);
  const extra = docs();
  extra.mcp.tools.push({ name: 'brand_new_tool', description: 'New.', input_schema: { type: 'object', properties: {} } });
  assert.throws(() => referenceFiles(extra, referencePlatform('darwin'), out, specs), /without a docs category: brand_new_tool/);
});

test('undocumentedParameters lists top-level params with a missing or blank description', () => {
  const snapshot = mcpSnapshot(docs().mcp);
  assert.deepEqual(undocumentedParameters(snapshot), []);
  snapshot.tools[0].input_schema.properties = {
    pid: { type: 'integer', description: 'Native process.' },
    window_id: { type: 'integer' },
    query: { type: 'string', description: '  ' },
  };
  assert.deepEqual(undocumentedParameters(snapshot), ['get_window_state.window_id', 'get_window_state.query']);
});

// Every parameter in a committed native snapshot must carry a description:
// the MCP reference renders it verbatim. Fix a failure at the tool's schema
// source in libs/cua-driver/rust/crates, then regenerate on that host. A
// snapshot with a `provenance` note is not a native dump and is enforced once
// a native run replaces it (the same rule runs natively in the Rust
// protocol_schema_test on every platform).
for (const [platform, snapshot] of loadSnapshots()) {
  test(`every ${platform.name} MCP parameter has a description`, (t) => {
    if (snapshot.provenance) {
      t.skip(`not a native dump: ${snapshot.provenance}`);
      return;
    }
    assert.deepEqual(undocumentedParameters(snapshot), []);
  });
}
