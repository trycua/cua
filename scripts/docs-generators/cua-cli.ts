#!/usr/bin/env npx tsx

/**
 * cua CLI and MCP reference generator.
 *
 * Builds the `cua` binary (libs/cua, debug), runs its hidden
 * `cua dump-docs --type all` (the clap definition and the `cua mcp` tool
 * list), and writes, through the shared renderers in lib/:
 *
 * - docs/content/docs/cua-cli/reference/index.mdx (the Reference index)
 * - docs/content/docs/cua-cli/reference/cli/ (one page per command group)
 * - docs/content/docs/cua-cli/reference/mcp-tools.mdx (every tool on one page)
 * - scripts/docs-generators/cli-specs/cua.json (the CLI-shape lane's oracle)
 *
 * Usage:
 *   pnpm --dir docs docs:generate:cua-cli
 *   pnpm --dir docs docs:check:cua-cli        # drift check (CI)
 *
 * CUA_CLI_BINARY=<path> skips the build and uses that binary.
 */

import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import {
  type CLIDocumentation,
  type CliGroup,
  type CliReference,
  cliIndexRows,
  renderCliReference,
  renderReferenceIndex,
} from './lib/cli-mdx';
import { readHeader } from './lib/headers';
import { pageOfCategory, slugify, toolHref } from './spaces';
import {
  type JsonSchema,
  type McpCategory,
  type McpReference,
  type McpToolEntry,
  mcpIndexRows,
  renderMcpReference,
  schemaType,
} from './lib/mcp-mdx';
import {
  DOCS_CONTENT,
  REPO_ROOT,
  codeCell,
  finish,
  isCheckMode,
  metaJson,
  stableJson,
  syncFiles,
} from './lib/mdx';

export { schemaType };

const CUA_DIR = path.join(REPO_ROOT, 'libs', 'cua');
const OUT_DIR = path.join(DOCS_CONTENT, 'cua-cli', 'reference');
export const CLI_SPEC = path.join(__dirname, 'cli-specs', 'cua.json');
const REGENERATE = 'pnpm --dir docs docs:generate:cua-cli';

export interface McpTool {
  name: string;
  group: string;
  permission: string;
  description: string;
  instructions?: string;
  input_schema: JsonSchema;
  providers?: string[];
  platforms?: string[];
  metering?: string;
  capabilities?: string[];
  annotations?: { read_only: boolean; destructive: boolean; idempotent: boolean; open_world: boolean };
  sdk_symbol?: string;
  rust_symbol?: string;
  notes?: string[];
}

export interface McpDocumentation {
  version: string;
  contract_version: string;
  mcp_protocol_version: string;
  permission_groups: Record<string, { all: string[]; readonly: string[] }>;
  tools: McpTool[];
}

export interface CuaDumpDocs {
  cli: CLIDocumentation;
  mcp: McpDocumentation;
}

// ---------------------------------------------------------------- CLI

/** Command groups: one page each (several commands per page), in the order readers meet them. */
export const CLI_GROUPS: CliGroup[] = [
  {
    slug: 'spaces-and-computers',
    title: 'Spaces, computers and teams',
    summary:
      'Your Spaces and machines, unattended access on this machine, devices, teams, your remote use this month, and a cua-spacesd by URL',
    commands: ['spaces', 'host', 'devices', 'team', 'usage', 'spacesd'],
  },
  {
    slug: 'files-teleport-and-keys',
    title: 'Files, teleport and keys',
    summary: 'Cua Volume files and sharing, moving an app session into a sandbox, and the Cua Keyvault',
    commands: ['volume', 'teleport', 'keyvault'],
  },
  {
    slug: 'sandboxes',
    title: 'Sandboxes and images',
    summary:
      'Create, reach and delete sandboxes; pull, build and push images; local runtimes, disk cleanup, your own cloud account and the sandbox viewer',
    commands: ['sandbox', 'image', 'images', 'runtime', 'cache', 'cloud', 'viewer'],
  },
  {
    slug: 'computer-actions',
    title: 'Computer actions',
    summary: 'One-shot computer actions (screenshot, click, type, windows) against a selected target, and their recorded trajectories',
    commands: ['do', 'do-host-consent', 'trajectory'],
  },
  {
    slug: 'agents',
    title: 'Agents, MCP and skills',
    summary: 'Run coding agents in a sandbox, set up your AI agents, the stdio MCP server, and recorded skills',
    commands: ['agent', 'agents', 'mcp', 'skills'],
  },
  {
    slug: 'account-and-setup',
    title: 'Account and setup',
    summary: 'Sign in and out, user defaults, the cua daemon, health checks and usage telemetry',
    commands: ['auth', 'config', 'daemon', 'doctor', 'telemetry'],
  },
];

export function cliReference(cli: CLIDocumentation): CliReference {
  return {
    product: 'cua-cli',
    cli,
    groups: CLI_GROUPS,
    generator: REGENERATE,
    source: 'cua dump-docs --type cli',
    intro: readHeader('cua-cli', 'cli'),
  };
}

// ---------------------------------------------------------------- MCP

/** The Spaces contract: its tools are documented once, under /spaces/reference. */
export const SPACES_MANIFEST = path.join(REPO_ROOT, 'libs', 'cua', 'spaces-contract', 'manifest.json');

export interface SpacesLinks {
  /** Tool name -> its entry (`/spaces/reference/<category>#<tool>`). */
  anchors: Map<string, string>;
  /** Tool name -> its category page (title and link). */
  sections: Map<string, { title: string; href: string }>;
}

/** Where each Spaces tool is documented, from the contract manifest. */
export function spacesAnchors(manifest: {
  categories?: Array<{ id: string; title: string }>;
  tools: Array<{ name: string; category: string }>;
}): SpacesLinks {
  const categories = manifest.categories ?? [];
  const titles = new Map(categories.map((c) => [c.id, c.title]));
  const pages = pageOfCategory(categories);
  return {
    anchors: new Map(manifest.tools.map((t) => [t.name, toolHref(pages, t)])),
    sections: new Map(
      manifest.tools.map((t) => {
        const title = titles.get(t.category) ?? t.category;
        return [t.name, { title, href: `/spaces/reference/${pages.get(t.category)}#${slugify(title)}` }];
      })
    ),
  };
}

function mcpCategories(spaces: SpacesLinks): McpCategory[] {
  const canonical = (tool: string) => {
    const href = spaces.anchors.get(tool);
    if (!href) throw new Error(`cua mcp serves Spaces tool ${tool}, which the Spaces contract manifest lacks`);
    return href;
  };
  return [
    {
      slug: 'spaces',
      title: 'Spaces tools',
      summary: 'The Spaces contract: register, claim and release Spaces, run commands, move files, stream, run agents and teleport',
      intro:
        'These tools are the [Spaces contract](/spaces/reference), with the same names and schemas on every surface. Each links to its entry there (parameters, result, errors, providers, metering); `cua mcp` gates each one behind the permission `spaces:<tool>`.',
      canonical,
      canonicalSection: (tool) => spaces.sections.get(tool)!,
    },
    { slug: 'images', title: 'Image tools', summary: 'The sandbox image catalog: which images exist and which ship a browser' },
    { slug: 'sandbox', title: 'Sandbox tools', summary: 'Create and manage sandboxes, local or on your own machines' },
    {
      slug: 'teleport',
      title: 'Teleport tools',
      summary: 'Move a signed-in browser session, per site, into a sandbox through the Cua Keyvault, with consent',
    },
    {
      slug: 'computer',
      title: 'Computer tools',
      summary: 'Screenshots, input, windows, files and shell on a sandbox desktop',
      intro: 'Computer tools act on a sandbox desktop through its cua-spacesd. Coordinates are in the pixel space of the last `computer_screenshot` of that sandbox.',
    },
    { slug: 'skills', title: 'Skills tools', summary: 'List, read, record and delete skills (recorded demonstrations)' },
  ];
}

function permissionsSection(mcp: McpDocumentation): string {
  const lines = [
    '## Permissions',
    '',
    'Tools are gated by `--permissions` (or `CUA_MCP_PERMISSIONS`), a comma-separated list of permissions and groups; the default is `all`. Each group has an `all` and a `readonly` form (`sandbox:readonly`); each tool lists its own permission.',
    '',
    '| Group | Grants | `:readonly` grants |',
    '| --- | --- | --- |',
  ];
  for (const [group, perms] of Object.entries(mcp.permission_groups)) {
    const list = (p: string[]) => (p.length > 6 ? `${p.length} permissions` : p.map((x) => codeCell(x)).join(', ') || 'none');
    lines.push(`| \`${group}:all\` | ${list(perms.all)} | ${list(perms.readonly)} |`);
  }
  return lines.join('\n');
}

export function mcpReference(mcp: McpDocumentation, spaces: SpacesLinks): McpReference {
  const tools: McpToolEntry[] = mcp.tools.map((t) => {
    const facts: Array<[string, string]> = [];
    if (t.providers?.length) facts.push(['Providers', t.providers.join(', ')]);
    if (t.metering) facts.push(['Metering', t.metering]);
    if (t.sdk_symbol) facts.push(['SDK', `\`${t.sdk_symbol}\``]);
    return {
      name: t.name,
      description: t.description,
      category: t.group,
      input_schema: t.input_schema,
      instructions: t.instructions,
      annotations: t.annotations,
      permission: t.permission,
      facts,
      notes: t.notes,
    };
  });
  const header = [
    readHeader('cua-cli', 'mcp-tools') ?? '',
    `Contract version ${mcp.contract_version}; MCP protocol revision ${mcp.mcp_protocol_version}.`,
  ]
    .filter(Boolean)
    .join('\n\n');
  return {
    product: 'cua-cli',
    server: 'cua mcp',
    generator: REGENERATE,
    source: 'cua dump-docs --type mcp',
    version: mcp.version,
    description: 'Every tool the cua MCP server exposes, with its permission and parameters.',
    categories: mcpCategories(spaces),
    tools,
    header,
    footer: permissionsSection(mcp),
    singlePage: true,
  };
}

// ---------------------------------------------------------------- main

export function renderAll(
  docs: CuaDumpDocs,
  spaces: SpacesLinks = spacesAnchors(JSON.parse(fs.readFileSync(SPACES_MANIFEST, 'utf-8')))
): Map<string, string> {
  const cli = cliReference(docs.cli);
  const mcp = mcpReference(docs.mcp, spaces);
  const rel = new Map<string, string>([...renderCliReference(cli), ...renderMcpReference(mcp)]);
  rel.set(
    'index.mdx',
    renderReferenceIndex({
      productName: 'Cua CLI',
      description: 'Every cua command and every cua MCP tool, generated from the CLI definition.',
      generator: REGENERATE,
      source: 'cua dump-docs --type all',
      version: docs.cli.version,
      sections: [
        { title: 'Commands', column: 'Command group', rows: cliIndexRows(cli) },
        { title: 'MCP tools', column: 'Tools', rows: mcpIndexRows(mcp) },
      ],
    })
  );
  rel.set('meta.json', metaJson('Reference', ['index', 'cli', 'mcp-tools']));
  const files = new Map<string, string>();
  for (const [p, content] of rel) files.set(path.join(OUT_DIR, p), content);
  files.set(CLI_SPEC, stableJson(docs.cli));
  return files;
}

export function targetDir(dir: string): string {
  return process.env.CARGO_TARGET_DIR
    ? path.resolve(dir, process.env.CARGO_TARGET_DIR)
    : path.join(dir, 'target');
}

function cuaBinary(): string {
  if (process.env.CUA_CLI_BINARY) return path.resolve(process.env.CUA_CLI_BINARY);
  execFileSync(process.env.CARGO || 'cargo', ['build', '-p', 'cua-cli'], {
    cwd: CUA_DIR,
    stdio: 'inherit',
  });
  return path.join(targetDir(CUA_DIR), 'debug', process.platform === 'win32' ? 'cua.exe' : 'cua');
}

export function dumpDocs(binary: string): CuaDumpDocs {
  // A temp-free, credential-free invocation: dump-docs never opens the SDK.
  return JSON.parse(
    execFileSync(binary, ['dump-docs', '--type', 'all'], {
      encoding: 'utf-8',
      maxBuffer: 64 * 1024 * 1024,
    })
  );
}

function main(): void {
  const checkOnly = isCheckMode();
  const docs = dumpDocs(cuaBinary());
  const owned = [OUT_DIR, path.join(OUT_DIR, 'cli'), path.join(OUT_DIR, 'mcp-tools')];
  finish('cua CLI', syncFiles(renderAll(docs), checkOnly, owned), checkOnly, REGENERATE);
}

if (require.main === module) main();
