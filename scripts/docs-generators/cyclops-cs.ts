#!/usr/bin/env npx tsx

/**
 * Cyclops CS Backend Documentation Generator
 *
 * Generates MDX documentation for the Cyclops CS backend HTTP API from its
 * committed OpenAPI/Swagger 2.0 description.
 *
 * Unlike the in-tree generators (cua-driver, lume), the Cyclops CS backend
 * lives in a different repository (trycua/cloud). Its spec is therefore
 * *vendored* into this repo under `specs/cyclops-cs.swagger.json` and kept in
 * sync by the routine in `.github/workflows/sync-cyclops-cs-spec.yml`. This
 * generator only ever reads the vendored copy, so it runs hermetically here.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/cyclops-cs.ts          # Generate docs
 *   npx tsx scripts/docs-generators/cyclops-cs.ts --check  # Check for drift (CI mode)
 */

import * as fs from 'fs';
import * as path from 'path';

// ============================================================================
// Paths
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const SPEC_PATH = path.join(__dirname, 'specs', 'cyclops-cs.swagger.json');
const DOCS_OUTPUT_DIR = path.join(ROOT_DIR, 'docs/content/docs/reference/cyclops-cs');
const OUTPUT_FILE = 'http-api.mdx';

// ============================================================================
// Minimal Swagger 2.0 types (only what this generator consumes)
// ============================================================================

interface Schema {
  $ref?: string;
  type?: string;
  format?: string;
  items?: Schema;
  additionalProperties?: Schema | boolean;
  enum?: unknown[];
  properties?: Record<string, Schema>;
  required?: string[];
  description?: string;
}

interface Parameter {
  name: string;
  in: 'body' | 'path' | 'query' | 'header' | 'formData';
  required?: boolean;
  type?: string;
  format?: string;
  items?: Schema;
  enum?: unknown[];
  description?: string;
  schema?: Schema;
}

interface Response {
  description?: string;
  schema?: Schema;
}

interface Operation {
  summary?: string;
  description?: string;
  tags?: string[];
  deprecated?: boolean;
  parameters?: Parameter[];
  responses?: Record<string, Response>;
}

interface Swagger {
  swagger?: string;
  info?: { title?: string; version?: string; description?: string };
  basePath?: string;
  host?: string;
  paths?: Record<string, Record<string, Operation>>;
  definitions?: Record<string, Schema>;
}

const HTTP_METHODS = ['get', 'post', 'put', 'patch', 'delete', 'head', 'options'] as const;

// ============================================================================
// MDX escaping (mirrors scripts/docs-generators/cua-driver.ts)
// ============================================================================

function escapeMdxText(value: string): string {
  return value
    .split(/(`[^`]*`)/g)
    .map((segment) => {
      if (segment.startsWith('`') && segment.endsWith('`')) {
        return segment;
      }
      return segment
        .replace(/\{/g, '&#123;')
        .replace(/\}/g, '&#125;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
    })
    .join('');
}

function escapeTableCell(value: string): string {
  return escapeMdxText(value.replace(/\s*\n\s*/g, ' ').trim()).replace(/\|/g, '\\|');
}

// ============================================================================
// Schema / type rendering
// ============================================================================

function refName(ref: string): string {
  const parts = ref.split('/');
  return parts[parts.length - 1];
}

/** Render a schema as a short, MDX-safe inline-code type expression. */
function schemaToText(schema: Schema | undefined): string {
  if (!schema) return '—';

  if (schema.$ref) {
    return `\`${refName(schema.$ref)}\``;
  }

  if (schema.type === 'array') {
    return `array of ${schemaToText(schema.items)}`;
  }

  if (schema.type === 'object' || (!schema.type && schema.additionalProperties)) {
    if (schema.additionalProperties && typeof schema.additionalProperties === 'object') {
      return `object (map of ${schemaToText(schema.additionalProperties)})`;
    }
    return '`object`';
  }

  if (schema.type) {
    return schema.format ? `\`${schema.type}\` (${schema.format})` : `\`${schema.type}\``;
  }

  return '—';
}

/** Render a non-body parameter's type. */
function paramTypeText(param: Parameter): string {
  if (param.type === 'array') {
    const itemType = param.items?.type ?? 'string';
    return `array of \`${itemType}\``;
  }
  if (param.type) {
    return param.format ? `\`${param.type}\` (${param.format})` : `\`${param.type}\``;
  }
  return '—';
}

function enumNote(values: unknown[] | undefined): string {
  if (!values || values.length === 0) return '';
  const rendered = values.map((v) => `\`${String(v)}\``).join(', ');
  return ` One of: ${rendered}.`;
}

// ============================================================================
// MDX assembly
// ============================================================================

function titleCase(tag: string): string {
  return tag
    .split(/[-_\s]+/)
    .map((w) => (w ? w[0].toUpperCase() + w.slice(1) : w))
    .join(' ');
}

function endpointHeading(method: string, route: string): string {
  // Path templates contain `{...}` which MDX parses as expressions, so the
  // method + path always live inside a code span (matches the existing
  // `### \`cua-driver list-tools\`` convention in cua-driver docs).
  return `### \`${method.toUpperCase()} ${route}\``;
}

function generateOperation(method: string, route: string, op: Operation): string[] {
  const lines: string[] = [];
  lines.push(endpointHeading(method, route));
  lines.push('');

  if (op.summary) {
    const summary = op.deprecated ? `**Deprecated.** ${op.summary}` : `**${op.summary}**`;
    lines.push(escapeMdxText(summary));
    lines.push('');
  } else if (op.deprecated) {
    lines.push('**Deprecated.**');
    lines.push('');
  }

  if (op.description && op.description.trim() && op.description.trim() !== op.summary?.trim()) {
    lines.push(escapeMdxText(op.description.trim()));
    lines.push('');
  }

  const params = op.parameters ?? [];
  const bodyParam = params.find((p) => p.in === 'body');
  const otherParams = params.filter((p) => p.in !== 'body');

  if (otherParams.length > 0) {
    lines.push('**Parameters:**');
    lines.push('');
    lines.push('| Name | In | Type | Required | Description |');
    lines.push('| ---- | -- | ---- | -------- | ----------- |');
    for (const p of otherParams) {
      // Swagger 2.0 requires every `in: path` parameter to be required; enforce
      // it here so the table is correct even when an upstream spec omits the flag.
      const required = p.required || p.in === 'path' ? 'Yes' : 'No';
      const description = `${(p.description ?? '').trim()}${enumNote(p.enum)}`.trim() || '—';
      lines.push(
        `| \`${p.name}\` | ${p.in} | ${paramTypeText(p)} | ${required} | ${escapeTableCell(description)} |`
      );
    }
    lines.push('');
  }

  if (bodyParam) {
    const required = bodyParam.required ? ' (required)' : '';
    const body = schemaToText(bodyParam.schema);
    const desc = bodyParam.description ? ` — ${escapeMdxText(bodyParam.description.trim())}` : '';
    lines.push(`**Request body:** ${body}${required}${desc}`);
    lines.push('');
  }

  if (op.responses && Object.keys(op.responses).length > 0) {
    lines.push('**Responses:**');
    lines.push('');
    lines.push('| Status | Description | Schema |');
    lines.push('| ------ | ----------- | ------ |');
    const codes = Object.keys(op.responses).sort((a, b) => a.localeCompare(b));
    for (const code of codes) {
      const r = op.responses[code];
      const description = escapeTableCell(r.description ?? '');
      const schema = r.schema ? schemaToText(r.schema) : '—';
      lines.push(`| \`${code}\` | ${description || '—'} | ${schema} |`);
    }
    lines.push('');
  }

  return lines;
}

function generateModel(name: string, schema: Schema): string[] {
  const lines: string[] = [];
  lines.push(`### \`${name}\``);
  lines.push('');

  if (schema.description && schema.description.trim()) {
    lines.push(escapeMdxText(schema.description.trim()));
    lines.push('');
  }

  const properties = schema.properties ?? {};
  const propNames = Object.keys(properties).sort((a, b) => a.localeCompare(b));
  const requiredSet = new Set(schema.required ?? []);

  if (propNames.length === 0) {
    lines.push('_No documented properties._');
    lines.push('');
    return lines;
  }

  lines.push('| Property | Type | Required | Description |');
  lines.push('| -------- | ---- | -------- | ----------- |');
  for (const prop of propNames) {
    const p = properties[prop];
    const required = requiredSet.has(prop) ? 'Yes' : 'No';
    const description = `${(p.description ?? '').trim()}${enumNote(p.enum)}`.trim() || '—';
    lines.push(
      `| \`${prop}\` | ${schemaToText(p)} | ${required} | ${escapeTableCell(description)} |`
    );
  }
  lines.push('');
  return lines;
}

export function generateHttpApiMdx(spec: Swagger): string {
  const lines: string[] = [];
  const title = spec.info?.title ?? 'Cyclops CS Backend API';
  const version = spec.info?.version ?? '';
  const basePath = spec.basePath && spec.basePath !== '/' ? spec.basePath : '';

  // Frontmatter — must be the very first thing in the file.
  lines.push('---');
  lines.push('title: HTTP API');
  lines.push(`description: REST API reference for the Cyclops CS backend${version ? `, v${version}` : ''}.`);
  lines.push('---');
  lines.push('');
  lines.push(`{/*
  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY
  Generated by: npx tsx scripts/docs-generators/cyclops-cs.ts
  Source: trycua/cloud cyclops-cs/backend/docs/swagger.json (vendored)
  Spec version: ${version || 'unknown'}
*/}`);
  lines.push('');

  // Intro
  lines.push(
    `Reference for the **${escapeMdxText(title)}** HTTP surface${version ? `, version \`${version}\`` : ''}. ` +
      `Generated from the backend's committed OpenAPI (Swagger 2.0) description.`
  );
  lines.push('');
  if (spec.info?.description && spec.info.description.trim()) {
    lines.push(escapeMdxText(spec.info.description.trim()));
    lines.push('');
  }
  if (basePath) {
    lines.push(`All paths are relative to the base path \`${basePath}\`.`);
    lines.push('');
  }

  // Collect operations grouped by tag.
  const paths = spec.paths ?? {};
  const byTag = new Map<string, Array<{ method: string; route: string; op: Operation }>>();
  for (const route of Object.keys(paths)) {
    const item = paths[route];
    for (const method of HTTP_METHODS) {
      const op = item[method];
      if (!op) continue;
      const tag = op.tags && op.tags.length > 0 ? op.tags[0] : 'Other';
      if (!byTag.has(tag)) byTag.set(tag, []);
      byTag.get(tag)!.push({ method, route, op });
    }
  }

  const tags = Array.from(byTag.keys()).sort((a, b) => {
    if (a === 'Other') return 1;
    if (b === 'Other') return -1;
    return a.localeCompare(b);
  });

  // Endpoint overview table.
  const totalEndpoints = Array.from(byTag.values()).reduce((n, arr) => n + arr.length, 0);
  lines.push('## Endpoints');
  lines.push('');
  lines.push(`The backend exposes ${totalEndpoints} endpoints across ${tags.length} groups.`);
  lines.push('');
  lines.push('| Method | Path | Summary |');
  lines.push('| ------ | ---- | ------- |');
  for (const tag of tags) {
    const entries = byTag
      .get(tag)!
      .slice()
      .sort((a, b) => a.route.localeCompare(b.route) || a.method.localeCompare(b.method));
    for (const { method, route, op } of entries) {
      const summary = escapeTableCell(op.summary ?? '');
      lines.push(`| \`${method.toUpperCase()}\` | \`${route}\` | ${summary || '—'} |`);
    }
  }
  lines.push('');

  // Detailed sections per tag.
  for (const tag of tags) {
    lines.push(`## ${escapeMdxText(titleCase(tag))}`);
    lines.push('');
    const entries = byTag
      .get(tag)!
      .slice()
      .sort((a, b) => a.route.localeCompare(b.route) || a.method.localeCompare(b.method));
    for (const { method, route, op } of entries) {
      lines.push(...generateOperation(method, route, op));
    }
  }

  // Models.
  const definitions = spec.definitions ?? {};
  const modelNames = Object.keys(definitions).sort((a, b) => a.localeCompare(b));
  if (modelNames.length > 0) {
    lines.push('## Models');
    lines.push('');
    lines.push(
      'Schema definitions referenced by the request bodies and responses above.'
    );
    lines.push('');
    for (const name of modelNames) {
      lines.push(...generateModel(name, definitions[name]));
    }
  }

  // Normalise trailing whitespace and guarantee a single trailing newline.
  while (lines.length > 0 && lines[lines.length - 1].trim() === '') {
    lines.pop();
  }
  return lines.join('\n') + '\n';
}

// ============================================================================
// Main
// ============================================================================

function main(): void {
  const args = process.argv.slice(2);
  const checkOnly = args.includes('--check') || args.includes('--check-only');

  console.log('Cyclops CS Backend Documentation Generator');
  console.log('==========================================\n');

  if (!fs.existsSync(SPEC_PATH)) {
    console.error(`Vendored spec not found: ${path.relative(ROOT_DIR, SPEC_PATH)}`);
    console.error('Run the spec sync routine before generating.');
    process.exit(1);
  }

  const spec: Swagger = JSON.parse(fs.readFileSync(SPEC_PATH, 'utf-8'));
  const mdx = generateHttpApiMdx(spec);
  const outPath = path.join(DOCS_OUTPUT_DIR, OUTPUT_FILE);

  if (checkOnly) {
    console.log('Checking for documentation drift...');
    if (!fs.existsSync(outPath)) {
      console.error(`${OUTPUT_FILE} does not exist`);
      process.exit(1);
    }
    const existing = fs.readFileSync(outPath, 'utf-8');
    if (existing !== mdx) {
      console.error(`${OUTPUT_FILE} is out of sync with the vendored spec`);
      console.error("\nRun 'npx tsx scripts/docs-generators/cyclops-cs.ts' to update documentation");
      process.exit(1);
    }
    console.log(`${OUTPUT_FILE} is up to date`);
    return;
  }

  fs.mkdirSync(DOCS_OUTPUT_DIR, { recursive: true });
  fs.writeFileSync(outPath, mdx);
  console.log(`Generated ${path.relative(ROOT_DIR, outPath)}`);
}

main();
