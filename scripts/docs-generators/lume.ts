#!/usr/bin/env npx tsx

/**
 * Lume reference generator.
 *
 * Builds Lume (`swift build -c release` in libs/lume), runs
 * `lume dump-docs --type all --pretty` (the CLI, HTTP API and MCP tool
 * definitions) and writes, through the shared renderers in lib/:
 *
 * - docs/content/docs/lume/reference/index.mdx (the Reference index)
 * - docs/content/docs/lume/reference/cli/ (one page per command group)
 * - docs/content/docs/lume/reference/http-api.mdx
 * - docs/content/docs/lume/reference/mcp-tools.mdx
 * - scripts/docs-generators/cli-specs/lume.json (the CLI-shape lane's oracle)
 *
 * Usage:
 *   pnpm --dir docs docs:generate:lume
 *   pnpm --dir docs docs:check:lume        # drift check (CI)
 *
 * LUME_BINARY=<path> skips the build and uses that binary.
 */

import { execFileSync } from 'child_process';
import * as path from 'path';
import {
  type CLIDocumentation,
  type CliGroup,
  type CliReference,
  cliIndexRows,
  renderCliReference,
  renderReferenceIndex,
  sentence,
} from './lib/cli-mdx';
import { readHeader } from './lib/headers';
import {
  type JsonSchema,
  type McpCategory,
  type McpReference,
  mcpIndexRows,
  renderMcpReference,
} from './lib/mcp-mdx';
import { DOCS_CONTENT, REPO_ROOT, codeCell, codeFence, escapeMdxText, escapeTableCell, finish, isCheckMode, metaJson, stableJson, syncFiles } from './lib/mdx';
import { normalizeEmDashes } from './prose-style';

export type { CLIDocumentation };

const LUME_DIR = path.join(REPO_ROOT, 'libs', 'lume');
const OUT_DIR = path.join(DOCS_CONTENT, 'lume', 'reference');
export const LUME_CLI_SPEC = path.join(__dirname, 'cli-specs', 'lume.json');
const REGENERATE = 'pnpm --dir docs docs:generate:lume';

export interface LumeMcpTool {
  name: string;
  description: string;
  input_schema: JsonSchema;
  annotations?: { read_only?: boolean; destructive?: boolean; idempotent?: boolean };
}

export interface LumeDumpDocs {
  cli: CLIDocumentation;
  api: HTTPAPIDocumentation;
  mcp?: { version?: string; tools: LumeMcpTool[] };
}

// ---------------------------------------------------------------- CLI

export const CLI_GROUPS: CliGroup[] = [
  {
    slug: 'vms',
    title: 'VMs',
    summary: 'Create, run, inspect, change, clone and delete virtual machines',
    commands: ['create', 'run', 'attach', 'stop', 'shutdown', 'restart', 'ls', 'get', 'set', 'clone', 'delete'],
  },
  {
    slug: 'images',
    title: 'Images',
    summary: 'Pull, push and convert VM images, find restore images, and prune the cache',
    commands: ['images', 'pull', 'push', 'convert', 'ipsw', 'prune'],
  },
  {
    slug: 'guest',
    title: 'Guest access',
    summary: 'Prepare unattended setup, open SSH sessions and change SIP in a guest',
    commands: ['setup', 'ssh', 'sip'],
  },
  {
    slug: 'server',
    title: 'Server',
    summary: 'Run the HTTP API and MCP server, read its logs, and dump the CLI and API definitions',
    commands: ['serve', 'logs', 'dump-docs'],
  },
  {
    slug: 'config',
    title: 'Configuration',
    summary: 'Storage locations, the image cache and telemetry settings',
    commands: ['config'],
  },
  {
    slug: 'updates',
    title: 'Updates',
    summary: 'Check for, apply and choose the channel of Lume updates',
    commands: ['check-update', 'update', 'channel'],
  },
];

export function cliReference(cli: CLIDocumentation): CliReference {
  return {
    product: 'lume',
    cli,
    groups: CLI_GROUPS,
    generator: REGENERATE,
    source: 'lume dump-docs --type cli',
    intro: readHeader('lume', 'cli'),
  };
}

// ---------------------------------------------------------------- MCP

const MCP_CATEGORIES: McpCategory[] = [
  { slug: 'vms', title: 'VM tools', summary: 'Create, list, run, stop, clone, resize and delete VMs' },
  { slug: 'guest', title: 'Guest tools', summary: 'Run commands inside a running VM' },
  { slug: 'maintenance', title: 'Maintenance tools', summary: 'Check for Lume updates' },
];

function mcpCategory(name: string): string {
  if (name === 'lume_exec') return 'guest';
  if (name.startsWith('lume_')) return 'vms';
  return 'maintenance';
}

export function mcpReference(mcp: NonNullable<LumeDumpDocs['mcp']>, version: string): McpReference {
  return {
    product: 'lume',
    server: 'lume serve --mcp',
    generator: REGENERATE,
    source: 'lume dump-docs --type mcp',
    version,
    description: "Every tool Lume's MCP server exposes, with its parameters.",
    categories: MCP_CATEGORIES,
    tools: mcp.tools.map((t) => ({
      name: t.name,
      description: t.description,
      category: mcpCategory(t.name),
      input_schema: t.input_schema,
      annotations: t.annotations,
    })),
    header: readHeader('lume', 'mcp-tools'),
    singlePage: true,
  };
}

// ---------------------------------------------------------------- main

export function renderAll(docs: LumeDumpDocs): Map<string, string> {
  const cli = cliReference(docs.cli);
  const rel = new Map<string, string>(renderCliReference(cli));
  rel.set('http-api.mdx', normalizeEmDashes(generateHTTPAPIMDX(docs.api)));
  const sections = [
    { title: 'Commands', column: 'Command group', rows: cliIndexRows(cli) },
    {
      title: 'HTTP API',
      column: 'API',
      rows: [{ name: 'HTTP API', href: '/lume/reference/http-api', description: 'Every endpoint `lume serve` exposes, with parameters and examples.' }],
    },
  ];
  const pages = ['index', 'cli', 'http-api'];
  if (docs.mcp) {
    const mcp = mcpReference(docs.mcp, docs.cli.version);
    for (const [p, content] of renderMcpReference(mcp)) rel.set(p, content);
    sections.push({ title: 'MCP tools', column: 'Tools', rows: mcpIndexRows(mcp) });
    pages.push('mcp-tools');
  }
  rel.set(
    'index.mdx',
    renderReferenceIndex({
      productName: 'Lume',
      description: 'Every lume command, HTTP endpoint and MCP tool, generated from the Lume binary.',
      generator: REGENERATE,
      source: 'lume dump-docs --type all',
      version: docs.cli.version,
      sections,
    })
  );
  rel.set('meta.json', metaJson('Reference', pages));
  const files = new Map<string, string>();
  for (const [p, content] of rel) files.set(path.join(OUT_DIR, p), content);
  files.set(LUME_CLI_SPEC, stableJson(docs.cli));
  return files;
}

function lumeBinary(): string {
  if (process.env.LUME_BINARY) return path.resolve(process.env.LUME_BINARY);
  execFileSync('swift', ['build', '-c', 'release'], { cwd: LUME_DIR, stdio: 'inherit' });
  return path.join(LUME_DIR, '.build', 'release', 'lume');
}

export function dumpDocs(binary: string): LumeDumpDocs {
  const json = execFileSync(binary, ['dump-docs', '--type', 'all', '--pretty'], {
    cwd: LUME_DIR,
    encoding: 'utf-8',
    maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, LUME_TELEMETRY_ENABLED: 'false' },
  });
  return JSON.parse(json);
}

function main(): void {
  const checkOnly = isCheckMode();
  const docs = dumpDocs(lumeBinary());
  const owned = [OUT_DIR, path.join(OUT_DIR, 'cli')];
  finish('Lume', syncFiles(renderAll(docs), checkOnly, owned), checkOnly, REGENERATE);
}

if (require.main === module) main();

// ============================================================================
// HTTP API types
// ============================================================================

export interface HTTPAPIDocumentation {
  base_path: string;
  version: string;
  description: string;
  endpoints: APIEndpointDoc[];
}

export interface APIEndpointDoc {
  method: string;
  path: string;
  description: string;
  category: string;
  path_parameters: APIParameterDoc[];
  query_parameters: APIParameterDoc[];
  request_body?: APIRequestBodyDoc;
  response_body: APIResponseDoc;
  status_codes: APIStatusCodeDoc[];
}

export interface APIParameterDoc {
  name: string;
  type: string;
  required: boolean;
  description: string;
}

export interface APIRequestBodyDoc {
  content_type: string;
  description: string;
  fields: APIFieldDoc[];
}

export interface APIResponseDoc {
  content_type: string;
  description: string;
  fields?: APIFieldDoc[];
}

export interface APIFieldDoc {
  name: string;
  type: string;
  required: boolean;
  description: string;
  default_value?: string;
}

export interface APIStatusCodeDoc {
  code: number;
  description: string;
}

// ============================================================================
// HTTP API Reference Generator
// ============================================================================

export function generateHTTPAPIMDX(docs: HTTPAPIDocumentation): string {
  const lines: string[] = [];

  const documentedVersion = docs.version;

  // Header - frontmatter MUST be at the very beginning of the file
  lines.push('---');
  lines.push('title: "HTTP API"');
  lines.push('description: "Every endpoint of the Lume HTTP API server (lume serve)."');
  lines.push('---');
  lines.push('');
  lines.push(`{/*
  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY
  Generated by: ${REGENERATE}
  Source: lume dump-docs --type api
  Version: ${documentedVersion}
*/}`);
  lines.push('');
  lines.push("import { Tabs, Tab } from 'fumadocs-ui/components/tabs';");
  lines.push('');

  // Introduction
  lines.push(docs.description);
  lines.push('');
  lines.push(
    `Documented against Lume **${documentedVersion}**. Run \`lume --version\` for your installed version.`
  );
  lines.push('');
  lines.push('## Default URL');
  lines.push('');
  lines.push('```');
  lines.push('http://localhost:7777');
  lines.push('```');
  lines.push('');
  lines.push(
    'Start the server with `lume serve` or specify a custom port with `lume serve --port <port>`.'
  );
  lines.push('');

  // Group endpoints by category
  const categories = [...new Set(docs.endpoints.map((e) => e.category))];

  for (const category of categories) {
    lines.push(`## ${category}`);
    lines.push('');

    const categoryEndpoints = docs.endpoints.filter((e) => e.category === category);

    for (const endpoint of categoryEndpoints) {
      lines.push(...generateEndpointDoc(endpoint));
    }
  }

  return lines.join('\n');
}

export function generateEndpointDoc(endpoint: APIEndpointDoc): string[] {
  const lines: string[] = [];

  lines.push(`### ${escapeMdxText(endpoint.description)}`);
  lines.push('');
  lines.push(codeFence('http', `${endpoint.method} ${endpoint.path}`, 'output'));
  lines.push('');

  // Parameters table (path + query)
  const allParams = [
    ...endpoint.path_parameters.map((p) => ({ ...p, location: 'path' })),
    ...endpoint.query_parameters.map((p) => ({ ...p, location: 'query' })),
  ];

  if (allParams.length > 0) {
    lines.push('**Parameters**');
    lines.push('');
    lines.push('| Parameter | In | Type | Default | Description |');
    lines.push('| --- | --- | --- | --- | --- |');
    for (const param of allParams) {
      const def = param.required ? 'required' : '';
      lines.push(
        `| ${codeCell(param.name)} | ${param.location} | ${codeCell(param.type)} | ${def} | ${escapeTableCell(sentence(param.description))} |`
      );
    }
    lines.push('');
  }

  // Request body
  if (endpoint.request_body) {
    lines.push('**Request body**');
    lines.push('');
    lines.push('| Field | Type | Default | Description |');
    lines.push('| --- | --- | --- | --- |');
    for (const field of endpoint.request_body.fields) {
      const def = field.required ? 'required' : field.default_value ? codeCell(field.default_value) : '';
      lines.push(
        `| ${codeCell(field.name)} | ${codeCell(field.type)} | ${def} | ${escapeTableCell(sentence(field.description))} |`
      );
    }
    lines.push('');
  }

  // Example request
  lines.push('**Example request**');
  lines.push('');
  lines.push("<Tabs groupId=\"language\" persist items={['Curl', 'Python', 'TypeScript']}>");

  // Generate curl example
  lines.push('  <Tab value="Curl">');
  lines.push('```bash');
  lines.push(generateCurlExample(endpoint));
  lines.push('```');
  lines.push('  </Tab>');

  // Generate Python example
  lines.push('  <Tab value="Python">');
  lines.push('```python');
  lines.push(generatePythonExample(endpoint));
  lines.push('```');
  lines.push('  </Tab>');

  // Generate TypeScript example
  lines.push('  <Tab value="TypeScript">');
  lines.push('```typescript');
  lines.push(generateTypeScriptExample(endpoint));
  lines.push('```');
  lines.push('  </Tab>');

  lines.push('</Tabs>');
  lines.push('');

  // Status codes
  lines.push('**Response**');
  lines.push('');
  for (const status of endpoint.status_codes) {
    lines.push(`- \`${status.code}\`: ${escapeMdxText(sentence(status.description))}`);
  }
  lines.push('');

  return lines;
}

function generateCurlExample(endpoint: APIEndpointDoc): string {
  const path = endpoint.path.replace(/:(\w+)/g, (_, name) => getExamplePathValue(name));
  const url = `http://localhost:7777${path}`;

  if (endpoint.method === 'GET' || endpoint.method === 'DELETE') {
    if (endpoint.method === 'DELETE') {
      return `curl -X DELETE "${url}"`;
    }
    return `curl "${url}"`;
  }

  // POST/PATCH with body
  if (endpoint.request_body && endpoint.request_body.fields.length > 0) {
    const bodyObj: Record<string, unknown> = {};
    for (const field of endpoint.request_body.fields) {
      if (field.required) {
        bodyObj[field.name] = getExampleValue(field);
      }
    }
    const bodyJson = JSON.stringify(bodyObj, null, 2);
    return `curl -X ${endpoint.method} "http://localhost:7777${path}" \\
  -H "Content-Type: application/json" \\
  -d '${bodyJson}'`;
  }

  return `curl -X ${endpoint.method} "${url}"`;
}

function generatePythonExample(endpoint: APIEndpointDoc): string {
  const path = endpoint.path.replace(/:(\w+)/g, (_, name) => getExamplePathValue(name));
  const method = endpoint.method.toLowerCase();

  const lines: string[] = [];
  lines.push('import requests');
  lines.push('');

  if (
    endpoint.request_body &&
    endpoint.request_body.fields.length > 0 &&
    (endpoint.method === 'POST' || endpoint.method === 'PATCH')
  ) {
    lines.push('data = {');
    for (const field of endpoint.request_body.fields) {
      if (field.required) {
        const value = getExampleValuePython(field);
        lines.push(`    "${field.name}": ${value},`);
      }
    }
    lines.push('}');
    lines.push('');
    lines.push(`response = requests.${method}("http://localhost:7777${path}", json=data)`);
  } else {
    lines.push(`response = requests.${method}("http://localhost:7777${path}")`);
  }

  lines.push('print(response.json())');

  return lines.join('\n');
}

function generateTypeScriptExample(endpoint: APIEndpointDoc): string {
  const path = endpoint.path.replace(/:(\w+)/g, (_, name) => getExamplePathValue(name));

  const lines: string[] = [];

  if (
    endpoint.request_body &&
    endpoint.request_body.fields.length > 0 &&
    (endpoint.method === 'POST' || endpoint.method === 'PATCH')
  ) {
    lines.push(`const response = await fetch(\`http://localhost:7777${path}\`, {`);
    lines.push(`  method: "${endpoint.method}",`);
    lines.push('  headers: { "Content-Type": "application/json" },');
    lines.push('  body: JSON.stringify({');
    for (const field of endpoint.request_body.fields) {
      if (field.required) {
        const value = getExampleValueTS(field);
        lines.push(`    ${field.name}: ${value},`);
      }
    }
    lines.push('  }),');
    lines.push('});');
  } else if (endpoint.method === 'DELETE') {
    lines.push(`const response = await fetch(\`http://localhost:7777${path}\`, {`);
    lines.push(`  method: "DELETE",`);
    lines.push('});');
  } else {
    lines.push(`const response = await fetch(\`http://localhost:7777${path}\`);`);
  }

  lines.push('const data = await response.json();');

  return lines.join('\n');
}

function getExampleValue(field: APIFieldDoc): unknown {
  switch (field.type) {
    case 'string':
      if (field.name === 'name') return 'my-vm';
      if (field.name === 'os') return 'macOS';
      if (field.name === 'memory') return '8GB';
      if (field.name === 'diskSize') return '50GB';
      if (field.name === 'display') return '1024x768';
      if (field.name === 'image') return 'macos-tahoe-vanilla:latest';
      if (field.name === 'path') return '/path/to/storage';
      return 'example';
    case 'integer':
      if (field.name === 'cpu') return 4;
      return 1;
    case 'boolean':
      return false;
    case 'array':
      if (field.name === 'tags') return ['latest'];
      return [];
    default:
      return 'value';
  }
}

function getExampleValuePython(field: APIFieldDoc): string {
  const val = getExampleValue(field);
  if (typeof val === 'string') return `"${val}"`;
  if (typeof val === 'boolean') return val ? 'True' : 'False';
  if (Array.isArray(val)) return JSON.stringify(val);
  return String(val);
}

function getExampleValueTS(field: APIFieldDoc): string {
  const val = getExampleValue(field);
  if (typeof val === 'string') return `"${val}"`;
  if (Array.isArray(val)) return JSON.stringify(val);
  return String(val);
}

function getExamplePathValue(name: string): string {
  if (name === 'name') return 'my-vm';
  if (name === 'id') return 'example-id';
  return `example-${name}`;
}
