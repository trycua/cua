#!/usr/bin/env npx tsx

/**
 * Cua Spaces reference generator: docs/content/docs/spaces/reference/.
 *
 * Source: the Spaces contract, `libs/cua/spaces-contract/manifest.json`
 * (generated from `libs/cua/crates/cua-spaces-contract` and gated by
 * `cua-spaces-contract-gen --check`): its categories, tools, input schemas,
 * results and error kinds. Three facts come from the code that serves the
 * contract and are checked here, so a change there fails the generator:
 *
 * - the tool-error envelope (`cua-spaces/src/mcp/mod.rs`, `ToolOutcome::error`);
 * - the JSON-RPC protocol error codes (`mcp::codes`, with their doc comments);
 * - the permission names `cua mcp --permissions` accepts (`cua-cli/src/mcp.rs`).
 *
 * Usage:
 *   pnpm --dir docs docs:generate:spaces
 *   pnpm --dir docs docs:check:spaces
 */

import * as fs from 'fs';
import * as path from 'path';
import {
  DOCS_CONTENT,
  REPO_ROOT,
  codeCell,
  escapeMdxText,
  escapeTableCell,
  finish,
  isCheckMode,
  metaJson,
  renderPage,
  syncFiles,
} from './lib/mdx';
import { docAbove } from './lib/rust-source';
import { type Schema, cleanDescription, flattenSchema } from './lib/schema-fields';

const OUT_DIR = path.join(DOCS_CONTENT, 'spaces', 'reference');
const REGENERATE = 'pnpm --dir docs docs:generate:spaces';
const R = (...p: string[]) => path.join(REPO_ROOT, ...p);

export const SOURCES = {
  manifest: R('libs', 'cua', 'spaces-contract', 'manifest.json'),
  contract: R('libs', 'cua', 'crates', 'cua-spaces-contract', 'src', 'lib.rs'),
  mcp: R('libs', 'cua', 'crates', 'cua-spaces', 'src', 'mcp', 'mod.rs'),
  cliMcp: R('libs', 'cua', 'crates', 'cua-cli', 'src', 'mcp.rs'),
};

export interface Annotations {
  read_only: boolean;
  destructive: boolean;
  idempotent: boolean;
  open_world: boolean;
}

export interface Tool {
  name: string;
  category: string;
  description: string;
  instructions: string;
  platforms: string[];
  providers: string[];
  metering: string;
  capabilities: string[];
  requires: string[];
  host_requires: string[];
  annotations: Annotations;
  sdk_symbol: string;
  rust_symbol: string;
  notes: string[];
  result: string;
  errors: string[];
  input_schema: Schema;
}

export interface Manifest {
  experimental: boolean;
  contract_version: string;
  capability_version: string;
  mcp_protocol_version: string;
  transports: string[];
  tool_count: number;
  categories: Array<{ id: string; title: string; description: string }>;
  error_kinds: Array<{ kind: string; meaning: string; fix: string }>;
  tools: Tool[];
}

export interface Protocol {
  /** JSON-RPC codes with their doc comments. */
  codes: Array<{ name: string; code: number; doc: string }>;
  /** Whether a tool error carries `structuredContent.error.{kind,message}`. */
  envelope: boolean;
  /** Whether permissions are `spaces:<tool>` with a read-only group. */
  permissions: boolean;
}

const read = (p: string) => fs.readFileSync(p, 'utf-8');

export function readProtocol(mcpSrc: string, cliSrc: string): Protocol {
  const at = mcpSrc.indexOf('pub mod codes {');
  if (at < 0) throw new Error('cua-spaces mcp: `codes` module not found');
  const block = mcpSrc.slice(at, mcpSrc.indexOf('\n}', at));
  const codes = [...block.matchAll(/pub const (\w+): i64 = (-?\d+);/g)].map((m) => ({
    name: m[1],
    code: Number(m[2]),
    doc: docAbove(block, m.index ?? 0).replace(/\s*\n\s*/g, ' '),
  }));
  if (!codes.length) throw new Error('cua-spaces mcp: no JSON-RPC codes');
  const envelope = /"error":\s*\{"kind":\s*e\.tag\(\),\s*"message"/.test(mcpSrc) && /"isError": self\.is_error/.test(mcpSrc);
  if (!envelope) throw new Error('cua-spaces mcp: the tool error envelope changed');
  const permissions =
    /format!\("spaces:\{\}", t\.name\)/.test(cliSrc) && /!readonly \|\| t\.annotations\.read_only/.test(cliSrc);
  if (!permissions) throw new Error('cua-cli mcp: the spaces permission rule changed');
  return { codes, envelope, permissions };
}

// --------------------------------------------------------------- rendering

const PROVIDER_ID: Record<string, string> = {
  cloud: '`cloud:<name>`',
  local: '`local:<name>`',
  direct: '`direct:<host:port>`',
  relay: '`relay:<machine-id>`',
};

function effect(a: Annotations): string {
  const kind = a.read_only ? 'read-only' : a.destructive ? 'destructive' : 'mutating';
  return a.idempotent ? `${kind}, idempotent` : kind;
}

/** The `Default ...` a parameter description states, as a cell. */
export function statedDefault(description: string): string | undefined {
  const m = /\bDefault(?::|\s)\s*(`[^`]+`|true|false|-?\d+(?:\.\d+)?)/.exec(description);
  return m ? (m[1].startsWith('`') ? m[1] : codeCell(m[1])) : undefined;
}

function constAlternatives(s: Schema): string[] {
  const alts = s.oneOf ?? s.anyOf ?? [];
  return alts
    .filter((a) => a.const !== undefined && a.description)
    .map((a) => `\`${String(a.const)}\`: ${cleanDescription(a.description)}`);
}

export function paramsTable(schema: Schema): string[] {
  const fields = flattenSchema(schema);
  if (!fields.length) return ['No parameters.', ''];
  const props = schema.properties ?? {};
  const rows = ['| Parameter | Type | Default | Description |', '| --- | --- | --- | --- |'];
  for (const f of fields) {
    const own = props[f.path];
    const extra = own ? constAlternatives(own) : [];
    const deprecated = own?.deprecated ? ['Deprecated.'] : [];
    const def = f.required ? 'required' : f.default !== undefined ? codeCell(JSON.stringify(f.default)) : statedDefault(f.description) ?? 'none';
    const desc = [escapeTableCell(f.description), ...extra.map(escapeTableCell), ...f.constraints.filter((c) => c !== 'Deprecated.'), ...deprecated]
      .filter(Boolean)
      .join(' ');
    rows.push(`| ${codeCell(f.path)} | ${codeCell(f.type)} | ${def} | ${desc} |`);
  }
  return [...rows, ''];
}

/** Error kinds a tool can return: the ones every call, Space, spacesd feature and host prerequisite implies, plus its own. */
export function errorKinds(tool: Tool): string[] {
  const kinds = ['invalid_argument'];
  const props = tool.input_schema.properties ?? {};
  if ('space' in props) kinds.push('not_found', 'ambiguous_sandbox');
  if (tool.requires.length) kinds.push('spacesd_not_available', 'capability_missing');
  if (tool.host_requires.length) kinds.push('host_capability_missing');
  for (const k of tool.errors) if (!kinds.includes(k)) kinds.push(k);
  return kinds;
}

function errorLink(kind: string): string {
  return `[\`${kind}\`](/spaces/reference/errors#${kind})`;
}

function toolSection(tool: Tool, manifest: Manifest): string[] {
  const known = new Set(manifest.error_kinds.map((k) => k.kind));
  const out = [`## ${tool.name}`, '', escapeMdxText(tool.description), ''];
  if (tool.instructions) out.push(escapeMdxText(tool.instructions), '');
  const facts: Array<[string, string]> = [
    [
      'Providers',
      tool.providers.length === Object.keys(PROVIDER_ID).length
        ? 'all (cloud, local, direct, relay)'
        : tool.providers.map((p) => `${p} only (${PROVIDER_ID[p] ?? p})`).join(', '),
    ],
    ['Platforms', tool.platforms.length === 3 ? 'all (macos, windows, linux)' : `${tool.platforms.join(', ')} (the operator's machine)`],
    ['Metering', tool.metering],
    [
      'Approval',
      `permission \`spaces:${tool.name}\`${tool.annotations.read_only ? ', in `spaces:readonly`' : ''}; ${effect(tool.annotations)}`,
    ],
  ];
  if (tool.requires.length) facts.push(['Requires (spacesd)', tool.requires.map((r) => codeCell(r)).join(', ')]);
  if (tool.host_requires.length) facts.push(['Requires (this machine)', tool.host_requires.map((r) => codeCell(r)).join(', ')]);
  if (tool.sdk_symbol) facts.push(['Swift SDK', codeCell(tool.sdk_symbol)]);
  facts.push(['Rust', codeCell(tool.rust_symbol)]);
  out.push('| | |', '| --- | --- |');
  for (const [k, v] of facts) out.push(`| ${k} | ${v.includes('`') ? v : escapeTableCell(v)} |`);
  out.push('', '### Parameters', '', ...paramsTable(tool.input_schema));
  out.push('### Result', '', escapeMdxText(tool.result), '');
  const kinds = errorKinds(tool);
  for (const k of kinds) if (!known.has(k)) throw new Error(`${tool.name}: error kind ${k} is not in error_kinds`);
  out.push('### Errors', '', kinds.map(errorLink).join(', '), '');
  if (tool.notes.length) {
    out.push('### Notes', '');
    for (const n of tool.notes) out.push(`- ${escapeMdxText(n.replace(/\s*FRICTION\.md( §)?\s*\d+\.?/g, '').trim())}`);
    out.push('');
  }
  return out;
}

function categoryPage(manifest: Manifest, cat: Manifest['categories'][number]): string {
  const tools = manifest.tools.filter((t) => t.category === cat.id);
  const body = [
    '| Tool | Description |',
    '| --- | --- |',
    ...tools.map((t) => `| [\`${t.name}\`](#${t.name}) | ${escapeTableCell(t.description)} |`),
    '',
  ];
  for (const t of tools) body.push(...toolSection(t, manifest));
  return renderPage({
    title: cat.title,
    description: cat.description,
    generator: REGENERATE,
    source: 'libs/cua/spaces-contract/manifest.json',
    version: manifest.contract_version,
    body: body.join('\n'),
  });
}

function indexPage(manifest: Manifest): string {
  const body: string[] = [
    `The Spaces contract: ${manifest.tool_count} tools with the same names and schemas on every surface: \`cua mcp\`, \`cua daemon mcp\` and the Spaces SDKs. Contract ${manifest.contract_version}, MCP protocol revision ${manifest.mcp_protocol_version}, transports ${manifest.transports.map((t) => `\`${t}\``).join(' and ')}.${manifest.experimental ? ' Experimental: names and shapes can change between minor versions.' : ''}`,
    '',
    '## Tools',
    '',
  ];
  for (const cat of manifest.categories) {
    body.push(`### [${cat.title}](/spaces/reference/${cat.id})`, '', escapeMdxText(cat.description), '');
    body.push('| Tool | Description | Effect |', '| --- | --- | --- |');
    for (const t of manifest.tools.filter((x) => x.category === cat.id))
      body.push(
        `| [\`${t.name}\`](/spaces/reference/${cat.id}#${t.name}) | ${escapeTableCell(t.description)} | ${effect(t.annotations)}${t.metering === 'metered' ? ', metered' : ''} |`
      );
    body.push('');
  }
  body.push(
    '## Space ids',
    '',
    'Every tool that takes `space` accepts a Space id or its name.',
    '',
    '| Provider | Id | Tools |',
    '| --- | --- | --- |'
  );
  for (const p of ['cloud', 'local', 'direct', 'relay']) {
    const only = manifest.tools.filter((t) => t.providers.length === 1 && t.providers[0] === p).map((t) => `\`${t.name}\``);
    body.push(`| \`${p}\` | ${PROVIDER_ID[p]} | ${only.length ? `all, and only here: ${only.join(', ')}` : 'all but the provider-only ones'} |`);
  }
  body.push(
    '',
    '## Approval',
    '',
    '- `cua mcp --permissions` (or `CUA_MCP_PERMISSIONS`) gates each tool as `spaces:<tool>`. `spaces:all` grants every tool, `spaces:readonly` only the read-only ones.',
    '- Each tool publishes MCP annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`); clients use them to decide what to confirm. The **Approval** row of each tool shows them.',
    `- Metered tools (${manifest.tools.filter((t) => t.metering === 'metered').map((t) => `\`${t.name}\``).join(', ')}) allocate or extend a billed cloud Space. Everything else is free.`,
    '- Agent runs are auto-approved inside the Space: the Space is the sandbox. A session teleport needs an approval minted from its manifest, and `acknowledge_sensitive` for sensitive items.',
    '',
    '## Errors',
    '',
    'A failed call returns a tool result with `isError: true` and a machine `kind`. See [Errors](/spaces/reference/errors).',
    ''
  );
  return renderPage({
    title: 'Reference',
    description: 'Every Spaces tool by category, with parameters, results, errors and approval.',
    generator: REGENERATE,
    source: 'libs/cua/spaces-contract/manifest.json',
    version: manifest.contract_version,
    body: body.join('\n'),
  });
}

function errorsPage(manifest: Manifest, protocol: Protocol): string {
  const body: string[] = [
    'A tool that fails returns a normal result with `isError: true`, never a protocol error:',
    '',
    '```json',
    '{',
    '  "content": [{ "type": "text", "text": "error: <message>" }],',
    '  "isError": true,',
    '  "structuredContent": { "error": { "kind": "<kind>", "message": "<message>" } }',
    '}',
    '```',
    '',
    'Branch on `kind`; the message is for people. Every tool can return `invalid_argument`. A tool that takes `space` can also return `not_found` and `ambiguous_sandbox`; one with spacesd `requires`, `spacesd_not_available` and `capability_missing`; one with host requirements, `host_capability_missing`. Each tool lists the rest.',
    '',
    '## Error kinds',
    '',
    '| Kind | Meaning |',
    '| --- | --- |',
    ...manifest.error_kinds.map((k) => `| [\`${k.kind}\`](#${k.kind}) | ${escapeTableCell(k.meaning)} |`),
    '',
  ];
  for (const k of manifest.error_kinds) {
    const tools = manifest.tools.filter((t) => t.errors.includes(k.kind));
    body.push(`### ${k.kind}`, '', escapeMdxText(k.meaning), '', `Fix: ${escapeMdxText(k.fix)}`, '');
    if (tools.length)
      body.push(
        `Listed by ${tools.map((t) => `[\`${t.name}\`](/spaces/reference/${t.category}#${t.name})`).join(', ')}.`,
        ''
      );
  }
  body.push('## Protocol errors', '', 'JSON-RPC errors are about the message, not the tool.', '', '| Code | Name | When |', '| --- | --- | --- |');
  for (const c of protocol.codes) body.push(`| \`${c.code}\` | \`${c.name}\` | ${escapeTableCell(c.doc)} |`);
  body.push('');
  return renderPage({
    title: 'Errors',
    description: 'Tool error kinds and JSON-RPC protocol errors of the Spaces tools.',
    generator: REGENERATE,
    source: 'libs/cua/spaces-contract/manifest.json; cua-spaces src/mcp/mod.rs',
    version: manifest.contract_version,
    body: body.join('\n'),
  });
}

export function renderAll(manifest: Manifest, protocol: Protocol): Map<string, string> {
  const ids = new Set(manifest.categories.map((c) => c.id));
  for (const t of manifest.tools) if (!ids.has(t.category)) throw new Error(`${t.name}: unknown category ${t.category}`);
  const files = new Map<string, string>();
  files.set(path.join(OUT_DIR, 'index.mdx'), indexPage(manifest));
  for (const cat of manifest.categories) files.set(path.join(OUT_DIR, `${cat.id}.mdx`), categoryPage(manifest, cat));
  files.set(path.join(OUT_DIR, 'errors.mdx'), errorsPage(manifest, protocol));
  files.set(path.join(OUT_DIR, 'meta.json'), metaJson('Reference', ['index', ...manifest.categories.map((c) => c.id), 'errors']));
  return files;
}

function main(): void {
  const checkOnly = isCheckMode();
  const manifest: Manifest = JSON.parse(read(SOURCES.manifest));
  const protocol = readProtocol(read(SOURCES.mcp), read(SOURCES.cliMcp));
  finish('Cua Spaces', syncFiles(renderAll(manifest, protocol), checkOnly, [OUT_DIR]), checkOnly, REGENERATE);
}

if (require.main === module) main();
