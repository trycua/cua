/**
 * The one MCP tool reference renderer shared by `cua mcp`, `cua-driver mcp`
 * and Lume's MCP server.
 *
 * Layout:
 *
 * - folder mode (`reference/mcp-tools/`): `index.mdx` (curated header, then
 *   one table per category listing every tool with its one-line summary; each
 *   row carries the tool's anchor so old `mcp-tools#<tool>` links still land)
 *   and one page per category;
 * - single-page mode (`reference/mcp-tools.mdx`) for small servers.
 *
 * Each tool: a heading anchored by its name (`#get_window_state`), the
 * description, one facts line (effect, permission, platforms), a parameters
 * table (`Parameter | Type | Default | Description`, required parameters show
 * `required`, one anchor per parameter: `#get_window_state--pid`) and minimal
 * example arguments built from the schema and validated against it.
 */

import { codeCell, codeFence, escapeMdxText, escapeTableCell, metaJson, renderPage, slug, stableJson } from './mdx';
import { firstSentence, sentence } from './cli-mdx';

export interface JsonSchema {
  type?: string | string[];
  description?: string;
  enum?: unknown[];
  const?: unknown;
  items?: JsonSchema;
  oneOf?: JsonSchema[];
  anyOf?: JsonSchema[];
  properties?: Record<string, JsonSchema>;
  required?: string[];
  additionalProperties?: boolean | JsonSchema;
  default?: unknown;
  deprecated?: boolean;
  format?: string;
  minimum?: number;
  maximum?: number;
  minItems?: number;
  maxItems?: number;
  minLength?: number;
  maxLength?: number;
  /** Renderer-only: the platforms that accept this parameter, when not all. */
  'x-platforms'?: string[];
}

export interface McpToolEntry {
  name: string;
  description: string;
  /** Category slug. */
  category: string;
  input_schema: JsonSchema;
  /** Longer usage notes after the description. */
  instructions?: string;
  annotations?: { read_only?: boolean; destructive?: boolean; idempotent?: boolean };
  permission?: string;
  /** Platforms that serve the tool, when not all of the server's platforms. */
  platforms?: string[];
  /** Extra `Label: value` facts (providers, metering, SDK symbol). */
  facts?: Array<[string, string]>;
  notes?: string[];
  /**
   * Per-platform variants when the tool's text or schema differs by platform:
   * rendered as synced platform tabs (a platform without the tool says so).
   */
  variants?: McpToolVariant[];
}

export interface McpToolVariant {
  platform: string;
  description: string;
  instructions?: string;
  input_schema: JsonSchema;
}

export interface McpCategory {
  slug: string;
  title: string;
  summary: string;
  /** Curated Markdown under the category heading. */
  intro?: string;
  /**
   * The canonical entry of each tool when another generated reference owns
   * it (the Spaces contract): the category page is then a short table linking
   * there, with the tool anchors kept, instead of repeating the entries.
   */
  canonical?: (tool: string) => string;
  /** With `canonical`: the canonical page section a tool belongs to, one heading per section. */
  canonicalSection?: (tool: string) => { title: string; href: string };
}

export interface McpReference {
  /** Docs product folder (`cua-cli`). */
  product: string;
  /** How the server starts, e.g. `cua mcp`. */
  server: string;
  generator: string;
  source: string;
  version?: string;
  description: string;
  categories: McpCategory[];
  tools: McpToolEntry[];
  /** Curated Markdown at the top of the index (or the single page). */
  header?: string;
  /** Curated Markdown sections after the tool index (for example permissions). */
  footer?: string;
  /** One page for every tool instead of a folder of category pages. */
  singlePage?: boolean;
  /** Every platform the server runs on, in tab order (for per-platform variants). */
  platforms?: string[];
}

// ---------------------------------------------------------------- schema

/** A reader-facing type label for a JSON Schema. */
export function schemaType(s: JsonSchema): string {
  const alts = s.oneOf ?? s.anyOf;
  if (alts?.length) {
    const labels = alts
      .filter((a) => a.type !== 'null')
      .flatMap((a) => (a.oneOf?.length ? a.oneOf : [a]))
      .map((a) => {
        const kind = a.properties?.kind?.const;
        return typeof kind === 'string' ? `${kind} target` : schemaType(a);
      });
    return [...new Set(labels)].join(' | ');
  }
  if (s.const !== undefined) return JSON.stringify(s.const);
  if (s.enum?.length) return s.enum.map((v) => JSON.stringify(v)).join(' | ');
  const types = (Array.isArray(s.type) ? s.type : [s.type ?? 'any']).filter((t) => t !== 'null');
  return types
    .map((t) => {
      if (t === 'array') return `${schemaType(s.items ?? {})}[]`;
      if (t === 'object' && s.additionalProperties && typeof s.additionalProperties === 'object')
        return `map of ${schemaType(s.additionalProperties)}`;
      return t;
    })
    .join(' | ');
}

function describe(prop: JsonSchema): string {
  if (prop.description) return prop.description;
  for (const alt of prop.anyOf ?? prop.oneOf ?? []) {
    const nested = describe(alt);
    if (nested) return nested;
  }
  return '';
}

function constraints(prop: JsonSchema): string[] {
  const out: string[] = [];
  if (prop.minimum !== undefined || prop.maximum !== undefined) {
    if (prop.minimum !== undefined && prop.maximum !== undefined)
      out.push(`Range: ${prop.minimum} to ${prop.maximum}.`);
    else if (prop.minimum !== undefined) out.push(`Minimum: ${prop.minimum}.`);
    else out.push(`Maximum: ${prop.maximum}.`);
  }
  if (prop.minItems !== undefined || prop.maxItems !== undefined) {
    out.push(`Items: ${prop.minItems ?? 0} to ${prop.maxItems ?? 'any'}.`);
  }
  if (prop.deprecated) out.push('Deprecated.');
  if (prop['x-platforms']?.length) out.push(`${prop['x-platforms'].join(', ')} only.`);
  return out;
}

function paramRows(
  tool: string,
  schema: JsonSchema,
  prefix = '',
  parentRequired = true
): string[] {
  // `tool` is the anchor prefix: `click` or, inside a platform tab, `click--linux`.
  const required = new Set(schema.required ?? []);
  const rows: string[] = [];
  for (const [name, prop] of Object.entries(schema.properties ?? {})) {
    const full = prefix ? `${prefix}.${name}` : name;
    const isRequired = parentRequired && required.has(name);
    const def = isRequired
      ? 'required'
      : prop.default !== undefined
        ? codeCell(JSON.stringify(prop.default))
        : '';
    const desc = [escapeTableCell(sentence(describe(prop))), ...constraints(prop)].filter(Boolean).join(' ');
    const anchor = `<span id="${tool}--${slug(full.replace(/\./g, '-'))}"></span>`;
    rows.push(`| ${anchor}${codeCell(full)} | ${codeCell(schemaType(prop))} | ${def} | ${desc} |`);
    // One level of nested object fields (not unions, not maps).
    if (!prefix && prop.properties && !prop.oneOf && !prop.anyOf) {
      rows.push(...paramRows(tool, prop, full, isRequired));
    }
  }
  return rows;
}

function paramsTable(anchor: string, schema: JsonSchema): string[] {
  const rows = paramRows(anchor, schema);
  if (!rows.length) return ['No parameters.', ''];
  return ['| Parameter | Type | Default | Description |', '| --- | --- | --- | --- |', ...rows, ''];
}

// ---------------------------------------------------------------- examples

function sampleValue(name: string, prop: JsonSchema): unknown {
  if (prop.default !== undefined) return prop.default;
  if (prop.const !== undefined) return prop.const;
  if (prop.enum?.length) return prop.enum[0];
  const alt = (prop.oneOf ?? prop.anyOf)?.find((a) => a.type !== 'null');
  if (alt) return sampleValue(name, alt);
  const type = Array.isArray(prop.type) ? prop.type.find((t) => t !== 'null') : prop.type;
  switch (type) {
    case 'integer':
    case 'number': {
      const n = /(^|_)(x|x1|x2|from_x|to_x)$/.test(name) ? 100 : /(^|_)(y|y1|y2|from_y|to_y)$/.test(name) ? 200 : 1;
      if (prop.minimum !== undefined && n < prop.minimum) return prop.minimum;
      if (prop.maximum !== undefined && n > prop.maximum) return prop.maximum;
      return n;
    }
    case 'boolean':
      return false;
    case 'array': {
      const count = Math.max(prop.minItems ?? 1, 1);
      return Array.from({ length: count }, () => sampleValue(name.replace(/s$/, ''), prop.items ?? {}));
    }
    case 'object': {
      const obj: Record<string, unknown> = {};
      for (const key of prop.required ?? []) obj[key] = sampleValue(key, prop.properties?.[key] ?? {});
      return obj;
    }
    default:
      return `<${name}>`;
  }
}

/** Why `value` does not satisfy `schema` (the subset the samples use), or null. */
export function validate(value: unknown, schema: JsonSchema): string | null {
  const alts = schema.oneOf ?? schema.anyOf;
  if (alts?.length) return alts.some((a) => validate(value, a) === null) ? null : 'no alternative matches';
  if (schema.const !== undefined && value !== schema.const) return `must be ${JSON.stringify(schema.const)}`;
  if (schema.enum && !schema.enum.includes(value)) return 'not in enum';
  const types = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  if (types.length) {
    const actual = Array.isArray(value)
      ? 'array'
      : value === null
        ? 'null'
        : Number.isInteger(value)
          ? 'integer'
          : typeof value;
    const ok = types.some((t) => t === actual || (t === 'number' && actual === 'integer'));
    if (!ok) return `expected ${types.join('|')}, got ${actual}`;
  }
  if (typeof value === 'number') {
    if (schema.minimum !== undefined && value < schema.minimum) return 'below minimum';
    if (schema.maximum !== undefined && value > schema.maximum) return 'above maximum';
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) return 'too few items';
    for (const item of value) {
      const why = schema.items ? validate(item, schema.items) : null;
      if (why) return `item: ${why}`;
    }
  }
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    for (const key of schema.required ?? []) {
      if (!(key in (value as object))) return `missing ${key}`;
    }
    for (const [key, v] of Object.entries(value as object)) {
      const sub = schema.properties?.[key];
      const why = sub ? validate(v, sub) : null;
      if (why) return `${key}: ${why}`;
    }
  }
  return null;
}

/** Minimal arguments (required parameters only), or null when none are required. */
export function exampleArguments(name: string, schema: JsonSchema): Record<string, unknown> | null {
  const required = schema.required ?? [];
  if (!required.length) return null;
  const args: Record<string, unknown> = {};
  for (const key of required) args[key] = sampleValue(key, schema.properties?.[key] ?? {});
  const why = validate(args, schema);
  if (why) throw new Error(`example arguments for ${name} do not match its schema: ${why}`);
  return args;
}

// ---------------------------------------------------------------- pages

function effect(a: McpToolEntry['annotations']): string | null {
  if (!a) return null;
  const kind = a.read_only ? 'read-only' : a.destructive ? 'destructive' : 'mutating';
  return `${kind}${a.idempotent ? ', idempotent' : ''}`;
}

function toolBody(anchor: string, name: string, v: { description: string; instructions?: string; input_schema: JsonSchema }, notes: string[] = []): string[] {
  const lines = [escapeMdxText(sentence(v.description)), ''];
  if (v.instructions) lines.push(escapeMdxText(v.instructions), '');
  lines.push(...paramsTable(anchor, v.input_schema));
  const args = exampleArguments(name, v.input_schema);
  if (args) lines.push('**Example arguments**', '', codeFence('json', JSON.stringify(args)), '');
  for (const note of notes) lines.push(`Note: ${escapeMdxText(sentence(note))}`, '');
  return lines;
}

function platformKey(platform: string): string {
  return slug(platform);
}

/** Whether the variants differ in anything the page shows. */
function variantsDiffer(variants: McpToolVariant[]): boolean {
  const key = (v: McpToolVariant) => stableJson([v.description, v.instructions ?? '', v.input_schema]);
  return new Set(variants.map(key)).size > 1;
}

export function toolSection(tool: McpToolEntry, level: number, allPlatforms: string[] = []): string[] {
  const lines = [`${'#'.repeat(level)} \`${tool.name}\``, ''];
  const facts: string[] = [];
  const e = effect(tool.annotations);
  if (e) facts.push(`Effect: ${e}.`);
  if (tool.permission) facts.push(`Permission: \`${tool.permission}\`.`);
  if (tool.platforms?.length) facts.push(`Platforms: ${tool.platforms.join(', ')}.`);
  for (const [label, value] of tool.facts ?? []) facts.push(`${label}: ${value}.`);
  const variants = tool.variants ?? [];
  if (variants.length > 1 && variantsDiffer(variants)) {
    if (facts.length) lines.push(facts.join(' '), '');
    // Parameters with the same schema on every platform are listed once, after
    // the tabs; each tab keeps its description and the parameters that differ.
    const props = variants.map((v) => v.input_schema.properties ?? {});
    const common = Object.keys(props[0]).filter((name) =>
      props.every((p) => p[name] !== undefined && stableJson(p[name]) === stableJson(props[0][name]))
    );
    const requiredEverywhere = (name: string) => variants.every((v) => (v.input_schema.required ?? []).includes(name));
    const subset = (schema: JsonSchema, keep: (name: string) => boolean): JsonSchema => ({
      ...schema,
      properties: Object.fromEntries(Object.entries(schema.properties ?? {}).filter(([n]) => keep(n))),
      required: (schema.required ?? []).filter(keep),
    });
    const tabs = allPlatforms.length ? allPlatforms : variants.map((v) => v.platform);
    lines.push(`<Tabs items={[${tabs.map((p) => `'${p}'`).join(', ')}]} groupId="platform" persist>`);
    for (const platform of tabs) {
      const v = variants.find((x) => x.platform === platform);
      lines.push(`<Tab value="${platform}">`, '');
      if (!v) {
        lines.push(`Not available on ${platform}.`, '', '</Tab>');
        continue;
      }
      lines.push(escapeMdxText(sentence(v.description)), '');
      if (v.instructions) lines.push(escapeMdxText(v.instructions), '');
      const own = subset(v.input_schema, (n) => !common.includes(n));
      if (Object.keys(own.properties ?? {}).length) {
        lines.push(`**${common.length ? `${platform} parameters` : 'Parameters'}**`, '');
        lines.push(...paramsTable(`${tool.name}--${platformKey(platform)}`, own));
      }
      lines.push('</Tab>');
    }
    lines.push('</Tabs>', '');
    if (common.length) {
      const shared = subset(variants[0].input_schema, (n) => common.includes(n) && (requiredEverywhere(n) || !(variants[0].input_schema.required ?? []).includes(n)));
      shared.required = common.filter(requiredEverywhere);
      lines.push(`**Parameters on every platform**`, '');
      lines.push(...paramsTable(tool.name, { ...shared, properties: Object.fromEntries(common.map((n) => [n, props[0][n]])) }));
    }
    let args: Record<string, unknown> | null = null;
    try {
      args = exampleArguments(tool.name, variants[0].input_schema);
      for (const v of variants.slice(1)) if (args && validate(args, v.input_schema)) args = null;
    } catch {
      args = null;
    }
    if (args) lines.push('**Example arguments**', '', codeFence('json', JSON.stringify(args)), '');
    for (const note of tool.notes ?? []) lines.push(`Note: ${escapeMdxText(sentence(note))}`, '');
    return lines;
  }
  const only = variants[0] ?? tool;
  lines.push(escapeMdxText(sentence(only.description)), '');
  if (only.instructions) lines.push(escapeMdxText(only.instructions), '');
  if (facts.length) lines.push(facts.join(' '), '');
  lines.push(...paramsTable(tool.name, only.input_schema));
  const args = exampleArguments(tool.name, only.input_schema);
  if (args) lines.push('**Example arguments**', '', codeFence('json', JSON.stringify(args)), '');
  for (const note of tool.notes ?? []) lines.push(`Note: ${escapeMdxText(sentence(note))}`, '');
  return lines;
}

function mcpUrl(ref: McpReference, page?: string): string {
  return `/${ref.product}/reference/mcp-tools${page ? `/${page}` : ''}`;
}

function byCategory(ref: McpReference): Map<string, McpToolEntry[]> {
  const known = new Set(ref.categories.map((c) => c.slug));
  const stray = ref.tools.filter((t) => !known.has(t.category)).map((t) => t.name);
  if (stray.length) throw new Error(`MCP tools without a category: ${stray.join(', ')}`);
  const names = ref.tools.map((t) => t.name);
  const dupes = names.filter((n, i) => names.indexOf(n) !== i);
  if (dupes.length) throw new Error(`duplicate MCP tools: ${dupes.join(', ')}`);
  return new Map(ref.categories.map((c) => [c.slug, ref.tools.filter((t) => t.category === c.slug)]));
}

function indexTable(tools: McpToolEntry[], href: (tool: string) => string, withPlatforms: boolean): string[] {
  const lines = withPlatforms
    ? ['| Tool | Description | Platforms |', '| --- | --- | --- |']
    : ['| Tool | Description |', '| --- | --- |'];
  for (const tool of tools) {
    const cell = `<span id="${tool.name}"></span>[\`${tool.name}\`](${href(tool.name)})`;
    const desc = escapeTableCell(firstSentence(tool.description));
    lines.push(withPlatforms ? `| ${cell} | ${desc} | ${tool.platforms?.join(', ') ?? 'all'} |` : `| ${cell} | ${desc} |`);
  }
  return [...lines, ''];
}

/**
 * Renders the MCP reference, keyed by path relative to the product's
 * `reference/` folder (`mcp-tools/index.mdx`, ... or `mcp-tools.mdx`).
 */
export function renderMcpReference(ref: McpReference): Map<string, string> {
  const groups = byCategory(ref);
  const withPlatforms = ref.tools.some((t) => t.platforms?.length);
  const out = new Map<string, string>();
  const page = (title: string, description: string, body: string[]) => {
    const text = body.join('\n');
    return renderPage({
      title,
      description,
      generator: ref.generator,
      source: ref.source,
      version: ref.version,
      components: text.includes('<Tabs ') ? ['Tabs'] : [],
      body: text,
    });
  };
  const all = ref.platforms ?? [];

  if (ref.singlePage) {
    const body: string[] = [];
    if (ref.header) body.push(ref.header.trim(), '');
    for (const cat of ref.categories) {
      const tools = groups.get(cat.slug) ?? [];
      if (!tools.length) continue;
      body.push(`## ${cat.title}`, '');
      if (cat.intro) body.push(cat.intro.trim(), '');
      for (const tool of tools) body.push(...toolSection(tool, 3, all));
    }
    if (ref.footer) body.push(ref.footer.trim(), '');
    out.set('mcp-tools.mdx', page('MCP tools', ref.description, body));
    return out;
  }

  const index: string[] = [];
  if (ref.header) index.push(ref.header.trim(), '');
  for (const cat of ref.categories) {
    const tools = groups.get(cat.slug) ?? [];
    if (!tools.length) continue;
    index.push(`## ${cat.title}`, '', `${escapeMdxText(sentence(cat.summary))} [Reference](${mcpUrl(ref, cat.slug)}).`, '');
    index.push(...indexTable(tools, cat.canonical ?? ((t) => `${mcpUrl(ref, cat.slug)}#${t}`), withPlatforms));
  }
  if (ref.footer) index.push(ref.footer.trim(), '');
  out.set('mcp-tools/index.mdx', page('All tools', ref.description, index));

  const slugs: string[] = [];
  for (const cat of ref.categories) {
    const tools = groups.get(cat.slug) ?? [];
    if (!tools.length) continue;
    slugs.push(cat.slug);
    const body: string[] = [];
    if (cat.canonical) {
      if (cat.intro) body.push(cat.intro.trim(), '');
      const sections = new Map<string, { href: string; tools: McpToolEntry[] }>();
      for (const tool of tools) {
        const sec = cat.canonicalSection?.(tool.name) ?? { title: '', href: '' };
        const entry = sections.get(sec.title) ?? { href: sec.href, tools: [] };
        entry.tools.push(tool);
        sections.set(sec.title, entry);
      }
      for (const [title, sec] of sections) {
        if (title) body.push(`## ${title}`, '', `Full entries: [${title}](${sec.href}).`, '');
        body.push(...indexTable(sec.tools, cat.canonical, withPlatforms));
      }
      body.push(`Served by \`${ref.server}\`; see [MCP tools](${mcpUrl(ref)}) for every tool.`, '');
      out.set(`mcp-tools/${cat.slug}.mdx`, page(cat.title, sentence(cat.summary), body));
      continue;
    }
    if (tools.length > 1) body.push(...indexTable(tools, (t) => `#${t}`, withPlatforms).map((l) => l.replace(/<span id="[^"]*"><\/span>/, '')));
    if (cat.intro) body.push(cat.intro.trim(), '');
    body.push(`Served by \`${ref.server}\`; see [MCP tools](${mcpUrl(ref)}) for every tool.`, '');
    for (const tool of tools) body.push(...toolSection(tool, 2, all));
    out.set(`mcp-tools/${cat.slug}.mdx`, page(cat.title, sentence(cat.summary), body));
  }
  out.set('mcp-tools/meta.json', metaJson('MCP tools', ['index', ...slugs]));
  return out;
}

/** Rows for a product Reference index: one per MCP category. */
export function mcpIndexRows(ref: McpReference): Array<{ name: string; href: string; description: string }> {
  const groups = byCategory(ref);
  if (ref.singlePage) {
    return [{ name: 'MCP tools', href: mcpUrl(ref).replace(/\/mcp-tools$/, '/mcp-tools'), description: ref.description }];
  }
  return [
    { name: 'All tools', href: mcpUrl(ref), description: `Every \`${ref.server}\` tool with a one-line summary.` },
    ...ref.categories
      .filter((c) => (groups.get(c.slug) ?? []).length)
      .map((c) => ({
        name: c.title,
        href: mcpUrl(ref, c.slug),
        description: `${sentence(c.summary)} ${(groups.get(c.slug) ?? []).length} tools.`,
      })),
  ];
}
