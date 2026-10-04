/**
 * JSON Schema to reference field tables, shared by the Fleets (CRD and Image
 * schemas) and Spaces (MCP tool input schemas) generators.
 *
 * `flattenSchema` walks a schema into dotted field paths
 * (`spec.vmTemplate.services[].name`), resolving local `$ref`s, so one table
 * lists every field of an object with its type, default, constraints and
 * description. `fieldTable` renders the rows as
 * `Field | Type | Default | Description`, with `required` in the Default
 * column for required fields.
 */

import { codeCell, escapeTableCell } from './mdx';

export interface Schema {
  type?: string | string[];
  description?: string;
  title?: string;
  default?: unknown;
  enum?: unknown[];
  const?: unknown;
  format?: string;
  items?: Schema;
  properties?: Record<string, Schema>;
  required?: string[];
  additionalProperties?: boolean | Schema;
  oneOf?: Schema[];
  anyOf?: Schema[];
  allOf?: Schema[];
  $ref?: string;
  $defs?: Record<string, Schema>;
  definitions?: Record<string, Schema>;
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  minItems?: number;
  maxItems?: number;
  pattern?: string;
  nullable?: boolean;
  deprecated?: boolean;
  'x-kubernetes-preserve-unknown-fields'?: boolean;
}

export interface Field {
  path: string;
  type: string;
  required: boolean;
  default?: unknown;
  description: string;
  constraints: string[];
}

/** Resolves a local `#/$defs/X` (or `#/definitions/X`) reference. */
export function resolve(schema: Schema, root: Schema): Schema {
  let s = schema;
  for (let i = 0; s.$ref && i < 16; i += 1) {
    const m = /^#\/(\$defs|definitions)\/(.+)$/.exec(s.$ref);
    if (!m) throw new Error(`unsupported $ref ${s.$ref}`);
    const defs = (m[1] === '$defs' ? root.$defs : root.definitions) ?? {};
    const target = defs[m[2]];
    if (!target) throw new Error(`unresolved $ref ${s.$ref}`);
    const { $ref: _ref, ...rest } = s;
    s = { ...target, ...rest };
  }
  return s;
}

function variants(s: Schema): Schema[] | undefined {
  return s.oneOf ?? s.anyOf;
}

/** The discriminator values of a `oneOf` of objects tagged by a `const` property. */
export function taggedVariants(
  s: Schema,
  root: Schema
): Array<{ tag: string; key: string; schema: Schema }> | null {
  const alts = variants(s)?.map((v) => resolve(v, root));
  if (!alts?.length) return null;
  const out: Array<{ tag: string; key: string; schema: Schema }> = [];
  for (const alt of alts) {
    const entry = Object.entries(alt.properties ?? {}).find(([, p]) => p.const !== undefined);
    if (!entry) return null;
    out.push({ tag: String(entry[1].const), key: entry[0], schema: alt });
  }
  return out;
}

/** A short type expression: `string`, `integer`, `"a" | "b"`, `string[]`, `map of string`. */
export function typeOf(schema: Schema, root: Schema = schema): string {
  const s = resolve(schema, root);
  if (s.const !== undefined) return JSON.stringify(s.const);
  if (s.enum?.length) return s.enum.map((v) => JSON.stringify(v)).join(' | ');
  const alts = variants(s);
  if (alts?.length) {
    const tagged = taggedVariants(s, root);
    if (tagged) return 'object';
    const parts = alts
      .map((a) => typeOf(a, root))
      .filter((t) => t !== 'null');
    return [...new Set(parts)].join(' | ') || 'any';
  }
  if (s['x-kubernetes-preserve-unknown-fields'] && !s.properties) return 'object (any JSON)';
  const types = (Array.isArray(s.type) ? s.type : [s.type ?? (s.properties ? 'object' : 'any')]).filter(
    (t) => t !== 'null'
  );
  return types
    .map((t) => {
      if (t === 'array') return `${typeOf(s.items ?? {}, root)}[]`;
      if (t === 'object' && s.additionalProperties && typeof s.additionalProperties === 'object')
        return `map of ${typeOf(s.additionalProperties, root)}`;
      return t;
    })
    .join(' | ');
}

function num(n: number): string {
  return Number.isInteger(n) ? String(n) : String(n);
}

/** Human constraints, table-safe: ranges, lengths, patterns (enums are in the type). */
export function constraintsOf(schema: Schema, root: Schema = schema): string[] {
  const s = resolve(schema, root);
  const out: string[] = [];
  const range = (lo?: number, hi?: number, unit = '') => {
    if (lo !== undefined && hi !== undefined) return `${num(lo)} to ${num(hi)}${unit}`;
    if (lo !== undefined) return `At least ${num(lo)}${unit}`;
    if (hi !== undefined) return `At most ${num(hi)}${unit}`;
    return '';
  };
  // A `u32` range is the type's own range, not a documented limit.
  const hi = s.maximum !== undefined && s.maximum >= 4294967295 ? undefined : s.maximum;
  const r = range(s.minimum, hi);
  if (r) out.push(`${r}.`);
  const l = range(s.minLength, s.maxLength, ' characters');
  if (l) out.push(`${l}.`);
  const items = range(s.minItems, s.maxItems, ' items');
  if (items) out.push(`${items}.`);
  if (s.pattern) out.push(`Matches ${codeCell(s.pattern)}.`);
  if (s.deprecated) out.push('Deprecated.');
  return out;
}

/**
 * Normalizes a schema description for a table cell: joins hard-wrapped
 * lines, and drops references to internal design notes and issue numbers
 * (`See docs/decisions/....md`, `(trycua/cloud#123)`) that readers of the
 * public docs cannot follow.
 */
export function cleanDescription(text: string | undefined): string {
  if (!text) return '';
  return text
    .replace(/\r/g, '')
    .split(/\n{2,}/)
    .map((para) => para.replace(/\s*\n\s*/g, ' ').trim())
    .join(' ')
    .replace(/\s*See docs\/[^\s]+\.md\.?/g, '')
    .replace(/\s*\((?:see )?trycua\/[\w-]+#\d+\)/g, '')
    .replace(/\s{2,}/g, ' ')
    .trim();
}

/** Walks `schema` into dotted field rows. `prefix` is the path of `schema` itself. */
export function flattenSchema(schema: Schema, root: Schema = schema, prefix = ''): Field[] {
  const out: Field[] = [];
  const s = resolve(schema, root);
  const required = new Set(s.required ?? []);
  for (const [name, raw] of Object.entries(s.properties ?? {})) {
    const p = resolve(raw, root);
    const path = prefix ? `${prefix}.${name}` : name;
    out.push({
      path,
      type: typeOf(p, root),
      required: required.has(name),
      default: p.default,
      description: cleanDescription(p.description),
      constraints: constraintsOf(p, root),
    });
    out.push(...children(p, root, path));
  }
  return out;
}

function children(p: Schema, root: Schema, path: string): Field[] {
  if (taggedVariants(p, root)) return [];
  if (p.properties) return flattenSchema(p, root, path);
  const t = Array.isArray(p.type) ? p.type : [p.type];
  if (t.includes('array') && p.items) {
    const items = resolve(p.items, root);
    if (items.properties && !taggedVariants(items, root)) return flattenSchema(items, root, `${path}[]`);
  }
  if (p.additionalProperties && typeof p.additionalProperties === 'object') {
    const v = resolve(p.additionalProperties, root);
    if (v.properties) return flattenSchema(v, root, `${path}.<key>`);
  }
  return [];
}

/** `Field | Type | Default | Description`; required fields show `required` as the default. */
export function fieldTable(fields: Field[]): string[] {
  if (!fields.length) return ['No fields.', ''];
  const lines = ['| Field | Type | Default | Description |', '| --- | --- | --- | --- |'];
  for (const f of fields) {
    const def = f.required
      ? 'required'
      : f.default !== undefined
        ? codeCell(JSON.stringify(f.default))
        : 'none';
    // Constraints are table-safe already (code spans come from codeCell).
    const desc = [escapeTableCell(f.description), ...f.constraints].filter(Boolean).join(' ');
    lines.push(`| ${codeCell(f.path)} | ${codeCell(f.type)} | ${def} | ${desc} |`);
  }
  return [...lines, ''];
}
