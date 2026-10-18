#!/usr/bin/env npx tsx

/**
 * Python reference generator (griffe, static analysis: nothing is imported).
 *
 * Two hand-written Python layers are documented here; the UniFFI binding
 * (`cua._native`) is documented from UniFFI metadata instead.
 *
 * - `cua` (libs/cua/python/src/cua, `_native` excluded) and the package
 *   facts -> cua-sdk/reference/python/index.mdx
 * - `cua_sandbox` (libs/python/cua-sandbox) public API ->
 *   cua-sdk/reference/python/*.mdx (+ an explicit meta.json)
 *
 * Curated prose comes from headers/cua-sdk/python/<page>.md; tested examples
 * from examples/cua-sdk/python/<Owner>.<member>.py, placed under the entry
 * they show.
 *
 * Usage:
 *   pnpm --dir docs docs:generate:python            # write the pages
 *   pnpm --dir docs docs:check:python               # drift check (CI)
 *   tsx scripts/docs-generators/python-sdk.ts [--check] [--only cua|cua-sandbox]
 *
 * griffe is pinned in requirements.txt and run through `uv run --no-project`.
 * Set PYTHON_DOCS_EXTRACTOR="python3" to use an interpreter that already has
 * the pinned griffe instead.
 */

import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import {
  DOCS_CONTENT,
  EXAMPLES_DIR,
  REPO_ROOT,
  codeCell,
  codeFence,
  escapeMdxText,
  escapeTableCell,
  finish,
  isCheckMode,
  loadExamples,
  metaJson,
  readHeader,
  renderExamples,
  renderPage,
  slug,
  syncFiles,
  type Example,
} from './lib/mdx';

// ============================================================================
// Extractor output
// ============================================================================

export interface PyParam {
  name: string;
  type: string;
  /** `required`, the literal default, or '' when the signature lacks the parameter. */
  default?: string;
  description: string;
}

export interface PyDoc {
  kind: 'class' | 'function' | 'attribute' | 'module' | 'external';
  name: string;
  path?: string;
  defined_in?: string;
  labels?: string[];
  signature?: string;
  type?: string;
  value?: string;
  bases?: string[];
  members?: PyDoc[];
  description?: string;
  params?: PyParam[];
  returns?: string;
  raises?: Array<{ type: string; description: string }>;
  target?: string;
}

export interface PyModuleDoc extends PyDoc {
  all: string[];
  lazy: Array<{ name: string; module: string; attribute: string; extra: string }>;
  aliases: Array<{ name: string; target: string }>;
  imports: Record<string, string>;
}

interface Extracted {
  objects: PyDoc[];
  modules: PyModuleDoc[];
}

// ============================================================================
// Page specifications
// ============================================================================

interface Section {
  title: string;
  intro?: string;
  objects: string[];
  /** Hide constructors (objects users get from a sandbox, not build). */
  hideInit?: boolean;
}

interface PageSpec {
  file: string;
  title: string;
  description: string;
  intro: string;
  sections: Section[];
}

const SB = 'cua_sandbox';
const SANDBOX_API_DIR = path.join(DOCS_CONTENT, 'cua-sdk', 'reference', 'python');
const CUA_PYTHON_PAGE = path.join(SANDBOX_API_DIR, 'index.mdx');
const SANDBOX_ROUTE = '/cua-sdk/reference/python';
const GENERATOR = 'pnpm --dir docs docs:generate:python';
const PY_EXAMPLES = path.join(EXAMPLES_DIR, 'cua-sdk', 'python');

export const SANDBOX_PAGES: PageSpec[] = [
  {
    file: 'sandbox',
    title: 'Sandbox',
    description:
      'Sandbox, SandboxInfo, the sandbox() helper and the creation options of cua-sandbox.',
    intro:
      'The `Sandbox` class is the entry point of `cua-sandbox`: create, connect to, or reattach a sandbox, then use its interfaces.',
    sections: [
      {
        title: 'Sandboxes',
        objects: [`${SB}.sandbox.Sandbox`, `${SB}.sandbox.sandbox`, `${SB}.sandbox.SandboxInfo`],
      },
      {
        title: 'Creation options',
        objects: [
          `${SB}.options.CloudOptions`,
          `${SB}.options.Probe`,
          `${SB}.options.http`,
          `${SB}.options.tcp`,
          `${SB}.options.PublicUrl`,
        ],
      },
      { title: 'Sandbox references', objects: [`${SB}._refs.AmbiguousSandbox`] },
    ],
  },
  {
    file: 'image',
    title: 'Image',
    description: 'The Image builder, sidecar containers and registry credentials of cua-sandbox.',
    intro:
      '`Image` is an immutable, chainable description of what a sandbox boots. Builder methods return a new `Image`.',
    sections: [
      { title: 'Image builder', objects: [`${SB}.image.Image`, `${SB}.image.ImageInfo`] },
      {
        title: 'Containers and registries',
        objects: [
          `${SB}.containers.Container`,
          `${SB}.containers.RegistrySecret`,
          `${SB}.generated.image_models.ImageFileReference`,
        ],
      },
    ],
  },
  {
    file: 'pool',
    title: 'Pool',
    description: 'Pools, templates, sandbox specs and pool errors of cua-sandbox.',
    intro:
      '`Pool` keeps warm cloud capacity for a `SandboxSpec`. `Sandbox.create` uses shared capacity; a named pool is for dedicated capacity.',
    sections: [
      { title: 'Pools and templates', objects: [`${SB}.pool.Pool`, `${SB}.pool.Template`] },
      {
        title: 'Managed pools (cua_sandbox.pools)',
        objects: [
          `${SB}.pools`,
          `${SB}._autopool.list_pools`,
          `${SB}._autopool.list_claims`,
          `${SB}._autopool.gc`,
          `${SB}._autopool.gc_pools`,
          `${SB}._autopool.is_managed_pool_name`,
          `${SB}._autopool.ManagedPoolInfo`,
          `${SB}._autopool.ClaimInfo`,
          `${SB}._autopool.GcReport`,
        ],
      },
      {
        title: 'Specs',
        objects: [
          `${SB}.spec.SandboxSpec`,
          `${SB}.spec.PoolOptions`,
          `${SB}.spec.PoolExport`,
          `${SB}.spec.generate_claim_token`,
        ],
      },
      {
        title: 'Errors',
        objects: [
          `${SB}.spec.PoolSpecMismatch`,
          `${SB}.spec.ClaimSecretsNotDelivered`,
          `${SB}.transport.fleet_cloud.PoolAccessDeniedError`,
        ],
      },
    ],
  },
  {
    file: 'interfaces',
    title: 'Interfaces',
    description:
      'Shell, mouse, keyboard, screen, clipboard, window, terminal, files, apps, mobile and driver interfaces.',
    intro:
      'Every sandbox exposes these interfaces as attributes (`sb.shell`, `sb.mouse`, ...). Input goes through cua-driver inside the guest.',
    sections: [
      {
        title: 'Shell commands',
        hideInit: true,
        objects: [`${SB}.interfaces.shell.Shell`, `${SB}.interfaces.shell.CommandResult`],
      },
      {
        title: 'Input and display',
        hideInit: true,
        objects: [
          `${SB}.interfaces.mouse.Mouse`,
          `${SB}.interfaces.keyboard.Keyboard`,
          `${SB}.interfaces.screen.Screen`,
          `${SB}.interfaces.clipboard.Clipboard`,
          `${SB}.interfaces.window.Window`,
        ],
      },
      {
        title: 'Terminal, files and apps',
        hideInit: true,
        objects: [
          `${SB}.interfaces.terminal.Terminal`,
          `${SB}.interfaces.files.Files`,
          `${SB}.interfaces.files.FileEntry`,
          `${SB}.interfaces.apps.Apps`,
        ],
      },
      { title: 'Mobile gestures', hideInit: true, objects: [`${SB}.interfaces.mobile.Mobile`] },
      {
        title: 'Cua Driver',
        hideInit: true,
        objects: [
          `${SB}.interfaces.driver.Driver`,
          `${SB}.interfaces.driver.DriverConnectionError`,
        ],
      },
    ],
  },
  {
    file: 'services',
    title: 'Services, tunnels and MCP',
    description:
      'Named services, signed and public URLs, tunnels and MCP endpoints of a cua-sandbox sandbox.',
    intro: 'Reach ports inside a sandbox: named services, tunnels, shareable URLs and MCP servers.',
    sections: [
      {
        title: 'Named services',
        hideInit: true,
        objects: [
          `${SB}.interfaces.services.ServiceHandle`,
          `${SB}.interfaces.services.Services`,
          `${SB}.interfaces.services.SignedServiceURL`,
        ],
      },
      {
        title: 'Tunnels',
        hideInit: true,
        objects: [`${SB}.interfaces.tunnel.Tunnel`, `${SB}.interfaces.tunnel.TunnelInfo`],
      },
      {
        title: 'MCP',
        objects: [
          `${SB}.interfaces.mcp.mcp_config`,
          `${SB}.interfaces.mcp.connect`,
          `${SB}.interfaces.mcp.open_mcp`,
        ],
      },
    ],
  },
  {
    file: 'runtimes',
    title: 'Runtimes',
    description:
      'Local runtimes (Docker, QEMU, Lume, Hyper-V, Tart, Android emulator) and local support checks.',
    intro:
      'Local sandboxes run on a runtime the SDK picks for the image. Pass one explicitly to `Sandbox.create(..., runtime=...)` to override it.',
    sections: [
      {
        title: 'Support checks',
        objects: [
          `${SB}.runtime.compat.RuntimeSupport`,
          `${SB}.runtime.compat.check_local_support`,
          `${SB}.runtime.compat.skip_if_unsupported`,
        ],
      },
      {
        title: 'Runtimes',
        objects: [
          `${SB}.runtime.base.Runtime`,
          `${SB}.runtime.base.RuntimeInfo`,
          `${SB}.runtime.docker.DockerRuntime`,
          `${SB}.runtime.qemu.QEMURuntime`,
          `${SB}.runtime.qemu.QEMUDockerRuntime`,
          `${SB}.runtime.qemu.QEMUBaremetalRuntime`,
          `${SB}.runtime.qemu.QEMUWSL2Runtime`,
          `${SB}.runtime.lume.LumeRuntime`,
          `${SB}.runtime.hyperv.HyperVRuntime`,
          `${SB}.runtime.tart.TartRuntime`,
          `${SB}.runtime.android_emulator.AndroidEmulatorRuntime`,
        ],
      },
    ],
  },
  {
    file: 'configuration',
    title: 'Configuration and errors',
    description: 'configure, login, whoami, transports and the errors cua-sandbox raises.',
    intro:
      'Process-wide settings, sign-in helpers, transports and the typed errors of `cua-sandbox`.',
    sections: [
      {
        title: 'Configuration and sign-in',
        objects: [
          `${SB}._config.configure`,
          `${SB}._config.fleet_auth_source`,
          `${SB}._auth.login`,
          `${SB}._auth.whoami`,
        ],
      },
      {
        title: 'Errors',
        objects: [
          `${SB}._sdk.SpacesdNotAvailable`,
          `${SB}._sdk.InvalidArgument`,
          `${SB}._sdk.InvalidPlacement`,
          `${SB}._sdk.Unsupported`,
        ],
      },
      {
        title: 'Transports',
        hideInit: true,
        objects: [`${SB}.transport.cloud.CloudTransport`, `${SB}.transport.env.EnvTransport`],
      },
    ],
  },
];

const CUA_OBJECTS = ['cua.embedded', 'cua.connect', 'cua.images.Image'];
const CUA_MODULES = ['cua', 'cua.runtime', 'cua.tools', 'cua.callbacks'];

// ============================================================================
// Extraction
// ============================================================================

function extractorCommand(): [string, string[]] {
  const script = path.join('scripts', 'docs-generators', 'extract_python_docs.py');
  const explicit = process.env.PYTHON_DOCS_EXTRACTOR;
  if (explicit) return [explicit, [script]];
  return [
    'uv',
    [
      'run',
      '--quiet',
      '--no-project',
      '--python',
      '3.12',
      '--with-requirements',
      path.join('scripts', 'docs-generators', 'requirements.txt'),
      'python',
      script,
    ],
  ];
}

export function extract(request: object): Extracted {
  const [cmd, args] = extractorCommand();
  const out = execFileSync(cmd, args, {
    cwd: REPO_ROOT,
    input: JSON.stringify(request),
    encoding: 'utf-8',
    maxBuffer: 64 * 1024 * 1024,
  });
  return JSON.parse(out) as Extracted;
}

// ============================================================================
// Rendering
// ============================================================================

/**
 * Docstring code MDX would not render: a four-space-indented block after a
 * blank line (outside lists), or a run of doctest `>>>` lines, becomes a
 * fenced `python` block.
 */
export function fenceIndentedCode(markdown: string): string {
  const lines = markdown.split('\n');
  const out: string[] = [];
  let fence: string | null = null;
  let lastText = '';
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const m = line.match(/^\s*(`{3,}|~{3,})/);
    if (m) {
      if (fence === null) fence = m[1];
      else if (m[1].startsWith(fence)) fence = null;
      out.push(line);
      continue;
    }
    if (fence !== null) {
      out.push(line);
      continue;
    }
    const prevBlank = i === 0 || lines[i - 1].trim() === '';
    const inList = /^\s*([-*+]|\d+[.)])\s/.test(lastText);
    const indented = /^ {4,}\S/.test(line) && prevBlank && !inList;
    const doctest = /^\s*>>> /.test(line);
    if (indented || doctest) {
      const block: string[] = [];
      const indent = indented ? 4 : line.match(/^\s*/)![0].length;
      while (i < lines.length) {
        const l = lines[i];
        const keep = indented
          ? l.trim() === '' || l.startsWith(' '.repeat(4))
          : l.trim() !== '' && l.match(/^\s*/)![0].length >= indent;
        if (!keep) break;
        block.push(l.slice(Math.min(indent, l.match(/^\s*/)![0].length)));
        i += 1;
      }
      i -= 1;
      while (block.length && block[block.length - 1].trim() === '') block.pop();
      out.push('```python', ...block.map((b) => b.replace(/^>>> |^\.\.\. /, '')), '```');
      if (lines[i + 1]?.trim()) out.push('');
      continue;
    }
    if (line.trim()) lastText = line;
    out.push(line);
  }
  return out.join('\n');
}

/** Escapes Markdown prose for MDX, leaving fenced code blocks untouched. */
export function mdxProse(markdown: string): string {
  markdown = fenceIndentedCode(markdown);
  const out: string[] = [];
  let fence: string | null = null;
  for (const line of markdown.split('\n')) {
    const m = line.match(/^\s*(`{3,}|~{3,})/);
    if (m) {
      if (fence === null) fence = m[1];
      else if (m[1].startsWith(fence)) fence = null;
      out.push(line);
      continue;
    }
    out.push(fence === null ? escapeMdxText(line) : line);
  }
  if (fence !== null) out.push(fence);
  return out.join('\n');
}

function publicName(doc: PyDoc): string {
  return doc.name;
}

function displaySignature(doc: PyDoc, owner?: string): string {
  let sig = doc.signature ?? '';
  if (doc.name === '__init__' && owner) {
    sig = sig.replace(/^def /, '').replace(/ -> None$/, '');
  }
  return sig;
}

/** The Default cell: `required`, or the literal default as code. */
export function defaultCell(value: string | undefined): string {
  if (!value) return '';
  return value === 'required' ? 'required' : codeCell(value);
}

export function paramTable(params: PyParam[]): string[] {
  if (!params.length) return [];
  const lines = ['| Parameter | Type | Default | Description |', '| --- | --- | --- | --- |'];
  for (const p of params) {
    lines.push(
      `| ${codeCell(p.name)} | ${p.type ? codeCell(p.type) : ''} | ${defaultCell(p.default)} | ${escapeTableCell(p.description)} |`
    );
  }
  return [...lines, ''];
}

function docTail(doc: PyDoc): string[] {
  const lines: string[] = [];
  if (doc.description) lines.push(mdxProse(doc.description), '');
  lines.push(...paramTable(doc.params ?? []));
  if (doc.returns) lines.push(`**Returns:** ${escapeMdxText(doc.returns)}`, '');
  if (doc.raises?.length) {
    lines.push('**Raises:**', '');
    for (const r of doc.raises) {
      lines.push(
        `- ${r.type ? codeCell(r.type) : ''}${r.description ? ': ' + escapeMdxText(r.description) : ''}`
      );
    }
    lines.push('');
  }
  return lines;
}

function attributeTable(attrs: PyDoc[]): string[] {
  if (!attrs.length) return [];
  const lines = ['| Attribute | Type | Description |', '| --- | --- | --- |'];
  for (const a of attrs) {
    const desc = (a.description ?? '').split('\n\n')[0];
    lines.push(
      `| ${codeCell(a.name)} | ${a.type ? codeCell(a.type) : ''} | ${escapeTableCell(desc)} |`
    );
  }
  return [...lines, ''];
}

export interface ExampleSet {
  examples: Map<string, Example[]>;
  used: Set<string>;
}

/** Tested examples for `target` (an object, `Owner.member` or a function), if any. */
function examplesFor(target: string, set?: ExampleSet): string[] {
  const list = set?.examples.get(target);
  if (!list) return [];
  set!.used.add(target);
  return ['**Example**', '', renderExamples(list), ''];
}

export function renderObject(doc: PyDoc, opts: { hideInit?: boolean; examples?: ExampleSet } = {}): string[] {
  const name = publicName(doc);
  const lines: string[] = [`### ${name}`, ''];
  if (doc.kind === 'external') {
    lines.push(`Re-exported from ${codeCell(doc.target ?? '')}.`, '');
    return lines;
  }
  if (doc.kind === 'class') {
    const bases = (doc.bases ?? []).filter((b) => b !== 'object');
    const init = doc.members?.find((m) => m.name === '__init__');
    const head = [`class ${name}${bases.length ? `(${bases.join(', ')})` : ''}`];
    if (init && !opts.hideInit) head.push('', displaySignature(init, name));
    lines.push(codeFence('python', head.join('\n')), '');
    lines.push(...docTail(doc));
    if (init && !opts.hideInit && (init.description || init.params?.length))
      lines.push(...docTail(init));
    const members = (doc.members ?? []).filter((m) => m.name !== '__init__');
    lines.push(...attributeTable(members.filter((m) => m.kind === 'attribute')));
    lines.push(...examplesFor(name, opts.examples));
    for (const m of members.filter((m) => m.kind === 'function')) {
      lines.push(`#### ${name}.${m.name}`, '');
      const decorators = (m.labels ?? []).filter(
        (l) => l === 'staticmethod' || l === 'classmethod'
      );
      const sig = [...decorators.map((d) => `@${d}`), displaySignature(m)].join('\n');
      lines.push(codeFence('python', sig), '');
      lines.push(...docTail(m));
      lines.push(...examplesFor(`${name}.${m.name}`, opts.examples));
    }
    return lines;
  }
  if (doc.kind === 'function') {
    lines.push(codeFence('python', displaySignature(doc)), '');
    lines.push(...docTail(doc));
    lines.push(...examplesFor(name, opts.examples));
    return lines;
  }
  // attribute or module
  if (doc.type || doc.value) {
    lines.push(
      codeFence(
        'python',
        `${name}${doc.type ? `: ${doc.type}` : ''}${doc.value ? ` = ${doc.value}` : ''}`
      ),
      ''
    );
  }
  lines.push(...docTail(doc));
  return lines;
}

/** `name` -> `/cua-sdk/reference/python/<page>#<anchor>` for every documented object. */
export function sandboxAnchors(pages: PageSpec[] = SANDBOX_PAGES): Map<string, string> {
  const anchors = new Map<string, string>();
  for (const page of pages) {
    for (const section of page.sections) {
      for (const obj of section.objects) {
        const name = obj.split('.').at(-1)!;
        anchors.set(name, `${SANDBOX_ROUTE}/${page.file}#${slug(name)}`);
      }
    }
  }
  return anchors;
}

export function renderSandboxPage(
  page: PageSpec,
  docs: Map<string, PyDoc>,
  version: string,
  examples?: ExampleSet
): string {
  const body: string[] = [escapeMdxText(page.intro), ''];
  const header = readHeader(`cua-sdk/python/${page.file}.md`, { version });
  if (header) body.push(header, '');
  for (const section of page.sections) {
    body.push(`## ${section.title}`, '');
    if (section.intro) body.push(escapeMdxText(section.intro), '');
    for (const obj of section.objects) {
      const doc = docs.get(obj);
      if (!doc) throw new Error(`griffe returned no documentation for ${obj}`);
      body.push(...renderObject(doc, { hideInit: section.hideInit, examples }));
    }
  }
  return renderPage({
    title: page.title,
    description: page.description,
    generator: GENERATOR,
    source: `griffe over libs/python/cua-sandbox/cua_sandbox (${SB}.${page.file === 'sandbox' ? 'sandbox' : page.file})`,
    version: `cua-sandbox ${version}`,
    components: body.some((l) => l.startsWith('<Tabs')) ? ['Tabs'] : [],
    body: body.join('\n'),
  });
}

export function renderReexports(module: PyModuleDoc, documented: Set<string>): string {
  const rows = module.all
    .filter((name) => !documented.has(name))
    .map((name) => [name, module.imports[name] ?? module.path ?? '']);
  if (!rows.length) return '';
  const byModule = new Map<string, string[]>();
  for (const [name, mod] of rows) byModule.set(mod, [...(byModule.get(mod) ?? []), name]);
  const lines = [
    '## Re-exported names',
    '',
    '`cua_sandbox` also re-exports these names from other packages; see their own documentation.',
    '',
    '| Module | Names |',
    '| --- | --- |',
  ];
  for (const [mod, names] of [...byModule].sort(([a], [b]) => a.localeCompare(b))) {
    lines.push(`| ${codeCell(mod)} | ${names.sort().map(codeCell).join(', ')} |`);
  }
  return lines.join('\n');
}

export function renderCuaPython(
  docs: Map<string, PyDoc>,
  modules: Map<string, PyModuleDoc>,
  anchors: Map<string, string>,
  version: string,
  sandboxVersion: string,
  examples?: ExampleSet
): string {
  const root = modules.get('cua')!;
  const body: string[] = [];
  body.push(
    'The high-level Python API: `cua_sandbox` (`Sandbox`, `Image`, the computer interfaces, `Pool`) and the helpers the `cua` package adds. It is a thin layer over the [Cua SDK objects](/cua-sdk/reference): the cloud, the local runtimes and cua-spacesd all go through `cua`, whose generated binding (`cua._native`) those pages document.',
    ''
  );
  const header = readHeader('cua-sdk/python/index.md', { version, sandboxVersion });
  if (header) body.push(header, '');
  body.push('## Pages', '', '| Page | Covers |', '| --- | --- |');
  for (const page of SANDBOX_PAGES) {
    body.push(`| [${escapeTableCell(page.title)}](${SANDBOX_ROUTE}/${page.file}) | ${escapeTableCell(page.description)} |`);
  }
  body.push('');
  body.push('## The cua package', '');
  body.push(
    'The hand-written layer of `cua`: two module-level constructors, a canonical-image helper and lazy re-exports of the extras.',
    ''
  );
  for (const name of ['cua.embedded', 'cua.connect']) body.push(...renderObject(docs.get(name)!, { examples }));
  const image = renderObject(docs.get('cua.images.Image')!, { examples });
  image.splice(
    2,
    0,
    'Without `cua[sandbox]`, `cua.Image` is this helper. With it installed, `cua.Image` is the richer [cua_sandbox Image](' +
      `${SANDBOX_ROUTE}/image#image).`,
    ''
  );
  body.push(...image);
  body.push('### Names from the extras', '');
  body.push(
    '`cua` resolves these names lazily from the optional extras. Install the extra to use them.',
    ''
  );
  body.push('| Name | Resolves to | Extra |', '| --- | --- | --- |');
  const lazy = [
    { name: 'Image', module: 'cua_sandbox', attribute: 'Image', extra: 'sandbox' },
    ...root.lazy,
  ].sort((a, b) => a.name.localeCompare(b.name));
  for (const row of lazy) {
    const target = codeCell(`${row.module}.${row.attribute}`);
    const link = row.extra === 'sandbox' ? anchors.get(row.attribute) : undefined;
    body.push(
      `| ${codeCell(`cua.${row.name}`)} | ${link ? `[${target}](${link})` : target} | ${codeCell(`cua[${row.extra}]`)} |`
    );
  }
  body.push('');
  for (const name of ['cua.runtime', 'cua.tools', 'cua.callbacks']) {
    const mod = modules.get(name)!;
    body.push(`### ${name}`, '');
    body.push(mdxProse(firstParagraph(mod.description ?? '')), '');
    const names = mod.all.map((n) => {
      const link = name === 'cua.runtime' ? anchors.get(n) : undefined;
      return link ? `[${codeCell(n)}](${link})` : codeCell(n);
    });
    body.push(`Exports: ${names.join(', ')}.`, '');
  }
  if (root.aliases.length) {
    body.push('### Deprecated aliases', '');
    body.push('Kept for one release after a rename. Use the new names.', '');
    body.push('| Deprecated | Use |', '| --- | --- |');
    for (const a of root.aliases)
      body.push(`| ${codeCell(`cua.${a.name}`)} | ${codeCell(`cua.${a.target}`)} |`);
    body.push('');
  }
  return renderPage({
    title: 'Python high-level API',
    description:
      'cua_sandbox and the cua package helpers: packages, configuration, and the page for each class.',
    generator: GENERATOR,
    source: 'griffe over libs/cua/python/src/cua (cua._native excluded) and libs/python/cua-sandbox',
    version: `cua ${version}, cua-sandbox ${sandboxVersion}`,
    components: body.some((l) => l.startsWith('<Tabs')) ? ['Tabs'] : [],
    body: body.join('\n'),
  });
}

function firstParagraph(markdown: string): string {
  return markdown.split('\n\n')[0] ?? '';
}

function readVersion(file: string): string {
  const text = fs.readFileSync(path.join(REPO_ROOT, file), 'utf-8');
  const m = text.match(/^__version__\s*=\s*"([^"]+)"/m);
  if (!m) throw new Error(`no __version__ in ${file}`);
  return m[1];
}

// ============================================================================
// Main
// ============================================================================

export function buildFiles(
  extracted: Extracted,
  examples: Map<string, Example[]> = loadExamples(PY_EXAMPLES)
): Map<string, string> {
  const docs = new Map(extracted.objects.map((d) => [d.path!, d]));
  const modules = new Map(extracted.modules.map((m) => [m.path!, m]));
  const files = new Map<string, string>();
  const anchors = sandboxAnchors();
  const set: ExampleSet = { examples, used: new Set() };

  const sandboxVersion = readVersion('libs/python/cua-sandbox/cua_sandbox/__init__.py');
  for (const page of SANDBOX_PAGES) {
    let content = renderSandboxPage(page, docs, sandboxVersion, set);
    if (page.file === 'pool') {
      const reexports = renderReexports(modules.get(SB)!, new Set(anchors.keys()));
      if (reexports) content = content.replace(/\n$/, `\n\n${reexports}\n`);
    }
    files.set(path.join(SANDBOX_API_DIR, `${page.file}.mdx`), content);
  }
  files.set(
    path.join(SANDBOX_API_DIR, 'meta.json'),
    metaJson('Python high-level API', SANDBOX_PAGES.map((p) => p.file))
  );
  files.set(
    CUA_PYTHON_PAGE,
    renderCuaPython(docs, modules, anchors, readVersion('libs/cua/python/src/cua/__init__.py'), sandboxVersion, set)
  );
  const unused = [...examples.keys()].filter((k) => !set.used.has(k));
  if (unused.length) {
    throw new Error(`Python examples with no matching entry: ${unused.join(', ')} (scripts/docs-generators/examples/cua-sdk/python)`);
  }
  return files;
}

/** Every public `cua_sandbox` name is documented or listed as a re-export. */
export function assertCoverage(extracted: Extracted): void {
  const root = extracted.modules.find((m) => m.path === SB);
  if (!root) throw new Error('cua_sandbox module was not extracted');
  const documented = new Set(sandboxAnchors().keys());
  const missing = root.all.filter(
    (n) => !documented.has(n) && !root.imports[n]?.startsWith('fleet_sdk')
  );
  if (missing.length) {
    throw new Error(
      `cua_sandbox.__all__ names without a reference entry: ${missing.join(', ')}. Add them to SANDBOX_PAGES in python-sdk.ts.`
    );
  }
}

function main(): void {
  const checkOnly = isCheckMode();
  const objects = [
    ...SANDBOX_PAGES.flatMap((p) => p.sections.flatMap((s) => s.objects)),
    ...CUA_OBJECTS,
  ];
  const extracted = extract({
    search_paths: ['libs/python/cua-sandbox', 'libs/cua/python/src'],
    objects,
    modules: [SB, ...CUA_MODULES],
  });
  assertCoverage(extracted);
  const drift = syncFiles(buildFiles(extracted), checkOnly, [SANDBOX_API_DIR], GENERATOR);
  finish('Python', drift, checkOnly, GENERATOR);
}

if (require.main === module) main();
