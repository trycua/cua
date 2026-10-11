#!/usr/bin/env npx tsx

/**
 * Cua Fleets reference generator: docs/content/docs/fleets/reference/.
 *
 * There is no OpenAPI document for the Fleet resources (they are Kubernetes
 * custom resources behind the gateway's `/api/k8s` proxy), so each fact comes
 * from the nearest authoritative artifact:
 *
 * - object fields: the `osgym.cua.ai` CRDs as `cyclops-sdk-schema` renders
 *   them, plus the SDK's extension fields, secrets and example bodies, all
 *   printed by `cargo run -p cua-fleet --example fleet-docs-dump`; the Image
 *   resource from `cua-image`'s checked-in JSON Schema;
 * - REST operations on custom resources: the Fleet Rust client
 *   (`libs/fleet/sdk/src`): its route builders, HTTP methods and accepted
 *   statuses; the other endpoints and every error response: the gateway's
 *   swagger (`libs/fleet/backend/docs/swagger.json`);
 * - SDK: the UniFFI `Fleet` handle (`cua-sdk/src/native/fleet.rs`), linked to
 *   the Cua SDK reference instead of repeating signatures;
 * - Terraform: the provider's generated schema (`pool_generated.go`,
 *   `provider.go`), its CRD mapping and its registry docs;
 * - errors: `cua_fleet::Error`, the Fleet client's `SdkError`, and the SDK
 *   error each one surfaces as (`cua-daemon`).
 *
 * Every curated list below (which operations belong to which object) is
 * checked against those sources, so a renamed method or route fails here.
 *
 * Usage:
 *   pnpm --dir docs docs:generate:fleet
 *   pnpm --dir docs docs:check:fleet
 *
 * FLEET_DOCS_DUMP=<file> skips the cargo build and reads that dump instead.
 */

import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import {
  DOCS_CONTENT,
  REPO_ROOT,
  codeCell,
  codeFence,
  escapeMdxText,
  escapeTableCell,
  finish,
  isCheckMode,
  metaJson,
  renderPage,
  slug,
  syncFiles,
} from './lib/mdx';
import {
  enumVariants,
  firstSentence,
  flattenRustdocLinks,
  implFns,
  jsonTypeOfRust,
  matchBrace,
  structFields,
} from './lib/rust-source';
import { type Schema, cleanDescription, fieldTable, flattenSchema, taggedVariants, typeOf } from './lib/schema-fields';

const OUT_DIR = path.join(DOCS_CONTENT, 'fleets', 'reference');
const REGENERATE = 'pnpm --dir docs docs:generate:fleet';
const R = (...p: string[]) => path.join(REPO_ROOT, ...p);

export const SOURCES = {
  sdkDir: R('libs', 'fleet', 'sdk', 'src'),
  swagger: R('libs', 'fleet', 'backend', 'docs', 'swagger.json'),
  tfDir: R('libs', 'fleet', 'terraform-provider-fleets'),
  handle: R('libs', 'cua', 'crates', 'cua-sdk', 'src', 'native', 'fleet.rs'),
  fleetLib: R('libs', 'cua', 'crates', 'cua-fleet', 'src', 'lib.rs'),
  claimSecrets: R('libs', 'cua', 'crates', 'cua-fleet', 'src', 'claim_secrets.rs'),
  daemonLib: R('libs', 'cua', 'crates', 'cua-daemon', 'src', 'lib.rs'),
  imageSchema: R('libs', 'cua', 'crates', 'cua-image', 'schema', 'image-v1alpha1.schema.json'),
  sdkReference: R('docs', 'content', 'docs', 'cua-sdk', 'reference'),
  fleetDocs: R('docs', 'content', 'docs', 'fleets'),
};

// ------------------------------------------------------------------ inputs

export interface Crd {
  metadata: { name: string };
  spec: {
    group: string;
    names: { kind: string; plural: string; shortNames?: string[] };
    versions: Array<{ name: string; schema: { openAPIV3Schema: Schema } }>;
  };
}

export interface Extension {
  object: string;
  field: string;
  type: string;
  description: string;
}

export interface FleetDump {
  crds: Crd[];
  config: {
    default_base_url: string;
    default_token_url: string;
    missing_credentials: string;
    default_claim_bind_deadline_seconds: number;
    remote_builds_supported: boolean;
  };
  extensions: Extension[];
  examples: { pool: unknown; template: unknown; claim: unknown };
  secrets: {
    claim: { prefix: string; key: string; label: string; wait_seconds: number; example: unknown };
    registry: { prefix: string; max_sidecars: number; example: unknown };
  };
}

function loadDump(): FleetDump {
  const file = process.env.FLEET_DOCS_DUMP;
  if (file) return JSON.parse(fs.readFileSync(file, 'utf-8'));
  const out = execFileSync(
    process.env.CARGO || 'cargo',
    ['run', '-q', '--locked', '-p', 'cua-fleet', '--example', 'fleet-docs-dump'],
    { cwd: R('libs', 'cua'), encoding: 'utf-8', maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'] }
  );
  return JSON.parse(out);
}

const read = (p: string) => fs.readFileSync(p, 'utf-8');

export const DESCRIPTIONS_FILE = path.join(__dirname, 'fleet-descriptions.json');

export interface Descriptions {
  crd: Record<string, Record<string, string>>;
  swagger: Record<string, Record<string, string>>;
}

/** Applies `overrides` (path -> text) to `fields`; throws on a path that names no field. */
export function applyOverrides<T extends { path: string; description: string }>(
  fields: T[],
  overrides: Record<string, string>,
  where: string,
  strict = true
): T[] {
  const known = new Set(fields.map((f) => f.path));
  if (strict)
    for (const key of Object.keys(overrides))
      if (!known.has(key)) throw new Error(`fleet-descriptions.json: ${where} has no field ${key}`);
  return fields.map((f) => (overrides[f.path] ? { ...f, description: overrides[f.path] } : f));
}

// ------------------------------------------------------- REST: Fleet client

export interface RouteTemplate {
  fn: string;
  path: string;
}

/** Route builders in `routes.rs`, evaluated to path templates (`/api/.../{namespace}/...`). */
export function parseRoutes(src: string): Map<string, string> {
  const consts = new Map<string, string>();
  for (const m of src.matchAll(/const (\w+): &str = "([^"]*)";/g)) consts.set(m[1], m[2]);
  const fns = new Map<string, string>();
  const bodies = new Map<string, string>();
  for (const m of src.matchAll(/\npub fn (\w+)\(([^)]*)\) -> Result<Url, SdkError> \{/g)) {
    const open = (m.index ?? 0) + m[0].length - 1;
    bodies.set(m[1], src.slice(open, matchBrace(src, open) + 1));
  }
  const evalFn = (name: string, depth = 0): string => {
    if (fns.has(name)) return fns.get(name)!;
    if (depth > 4) throw new Error(`route ${name} recursion`);
    const body = bodies.get(name);
    if (!body) throw new Error(`route ${name} not found in routes.rs`);
    let p: string | null = null;
    const fmt = /route\(\s*base,\s*format!\(\s*"([^"]*)"/.exec(body);
    const constRoute = /route\(\s*base,\s*(\w+)\.into\(\)\s*\)/.exec(body);
    const inner = /(?:let (?:mut )?url = |^\s*)(\w+)\(base(?:,[^)]*)?\)\?/m.exec(body);
    if (fmt) {
      p = `/${fmt[1].replace(/\{([A-Z_]+)\}/g, (_, c: string) => {
        if (!consts.has(c)) throw new Error(`route ${name}: unknown const ${c}`);
        return consts.get(c)!;
      })}`;
    } else if (constRoute) {
      p = `/${consts.get(constRoute[1])}`;
    } else if (inner && bodies.has(inner[1])) {
      p = evalFn(inner[1], depth + 1);
      const q = /append_pair\("(\w+)",\s*(\w+)\)/.exec(body);
      if (q) p += `?${q[1]}={${q[2]}}`;
      const push = /\.push\((\w+)\)/.exec(body);
      if (push) p += `/{${push[1]}}`;
      const scheme = /"https" => "wss"/.test(body);
      if (scheme) p = `wss:${p}`;
    }
    if (!p) throw new Error(`route ${name}: unrecognized body`);
    p = p.replace(/\{service_name\}\{path\}/, '{service}{path}');
    fns.set(name, p);
    return p;
  };
  for (const name of bodies.keys()) evalFn(name);
  return fns;
}

export interface ClientOp {
  /** Client method (`create_pool`). */
  fn: string;
  /** The operation name the client reports in errors (`create pool`). */
  op: string;
  method: string;
  path: string;
  statuses: number[];
  /** Other client calls the method makes first or after. */
  calls: string[];
  doc: string;
}

/** Operations of the Fleet Rust client, keyed by method name. */
export function parseClientOps(dir: string, routes: Map<string, string>): Map<string, ClientOp> {
  const ops = new Map<string, ClientOp>();
  for (const file of fs.readdirSync(dir).filter((f) => f.endsWith('.rs')).sort()) {
    const src = read(path.join(dir, file));
    const helpers = new Map<string, string>();
    for (const h of src.matchAll(/fn (\w+_url)\(&self[^)]*\)[^{]*\{/g)) {
      const open = (h.index ?? 0) + h[0].length - 1;
      helpers.set(h[1], src.slice(open, matchBrace(src, open) + 1));
    }
    for (const f of implFns(src, 'CyclopsClient')) {
      const op = /send_(?:json|unit)(?:_crud)?\(\s*(?:self\.as_ref\(\),\s*)?"([^"]+)"/.exec(f.body);
      if (!op) continue;
      const method = /merge_patch_request\(/.test(f.body)
        ? 'PATCH'
        : /json_request\(\s*"(\w+)"/.exec(f.body)?.[1];
      if (!method) throw new Error(`${file}: ${f.name}: no HTTP method`);
      const routeFn = requestRoute(f.body, routes, helpers);
      if (!routeFn) throw new Error(`${file}: ${f.name}: no route`);
      const statuses = /&\[([\d,\s]+)\]/.exec(f.body)?.[1].split(',').map((s) => Number(s.trim())) ?? [];
      const calls = [...f.body.matchAll(/(?:self\)?\s*\.\s*|Arc::clone\(&self\)\s*\.\s*)(create_namespace_if_missing|delete_namespace)\(/g)].map(
        (m) => m[1]
      );
      ops.set(f.name, { fn: f.name, op: op[1], method, path: routes.get(routeFn)!, statuses, calls, doc: f.doc });
    }
  }
  return ops;
}

/**
 * The route the request goes to: the URL argument of `json_request(..)` or
 * `merge_patch_request(..)`, resolved through its `let` binding or a
 * `self.x_url(..)` helper. Other route calls in the body only validate names.
 */
function requestRoute(body: string, routes: Map<string, string>, helpers: Map<string, string>): string | undefined {
  const arg = /(?:json_request\(\s*"\w+",|merge_patch_request\()\s*(routes::\w+\(|\w+\()?(\w+)?/.exec(body);
  if (!arg) return undefined;
  if (arg[1]) {
    const name = arg[1].replace(/^routes::/, '').replace(/\($/, '');
    return routes.has(name) ? name : undefined;
  }
  const v = arg[2];
  const binding = new RegExp(`let (?:mut )?${v}(?::[^=]+)?\\s*=\\s*([^;]+);`).exec(body);
  if (!binding) {
    // `routes::claim_collection(base, ns).and_then(|url| Ok(json_request("POST", url, ..)))`:
    // the URL is the closure argument of the route builder's `and_then`.
    const chained = new RegExp(`(?:routes::)?\\b(\\w+)\\([^;]*?\\)\\s*\\.and_then\\(\\|${v}\\|`).exec(body);
    return chained && routes.has(chained[1]) ? chained[1] : undefined;
  }
  const direct = /(?:routes::)?\b(\w+)\(/.exec(binding[1]);
  if (direct && routes.has(direct[1])) return direct[1];
  const helper = /self\.(\w+_url)\(/.exec(binding[1]);
  if (helper && helpers.has(helper[1])) return routeOf(helpers.get(helper[1])!, routes);
  return undefined;
}

function routeOf(body: string, routes: Map<string, string>): string | undefined {
  // The last route builder the method calls is the one it sends to
  // (earlier calls validate names).
  const names = [...body.matchAll(/(?:routes::)?\b(\w+)\(\s*(?:self\.)?base_url\(\)/g)].map((m) => m[1]);
  return names.filter((n) => routes.has(n)).pop();
}


// ------------------------------------------------------------ REST: swagger

export interface SwaggerParam {
  name: string;
  in: string;
  type?: string;
  description?: string;
  required?: boolean;
  schema?: { $ref?: string };
}

export interface SwaggerOp {
  summary: string;
  description?: string;
  parameters?: SwaggerParam[];
  responses: Record<string, { description: string; schema?: Schema & { $ref?: string } }>;
  tags: string[];
}

export interface Swagger {
  paths: Record<string, Record<string, SwaggerOp>>;
  definitions: Record<string, Schema>;
  securityDefinitions: Record<string, { name: string; in: string }>;
}

// ------------------------------------------------------------- SDK handle

export interface HandleMethod {
  owner: string;
  name: string;
  summary: string;
  params: string[];
  returns: string;
  isAsync: boolean;
  /** Link into the Cua SDK reference, when that page documents the method. */
  href?: string;
}

export function parseHandle(src: string, sdkAnchors: Map<string, string>): Map<string, HandleMethod> {
  const out = new Map<string, HandleMethod>();
  for (const owner of ['Fleet', 'FleetPools']) {
    for (const f of implFns(src, owner)) {
      const key = `${owner}.${f.name}`;
      out.set(key, {
        owner,
        name: f.name,
        summary: firstSentence(f.doc),
        params: f.params.map((p) => p.name),
        returns: unwrapResult(f.returns),
        isAsync: f.isAsync,
        href: sdkAnchors.get(key),
      });
    }
  }
  return out;
}

function unwrapResult(t: string): string {
  const m = /^Result<(.+)>$/.exec(t.replace(/\s+/g, ' ').trim());
  const inner = m ? m[1] : t;
  return inner.replace(/^Arc<(.+)>$/, '$1');
}

/** `Fleet.acquire` -> `/cua-sdk/reference/.../page#fleetacquire`, from the generated SDK pages. */
export function sdkAnchors(dir: string): Map<string, string> {
  const out = new Map<string, string>();
  if (!fs.existsSync(dir)) return out;
  const walk = (d: string) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith('.mdx')) {
        const rel = path.relative(DOCS_CONTENT, p).replace(/\\/g, '/').replace(/\.mdx$/, '').replace(/\/index$/, '');
        for (const m of read(p).matchAll(/^#{2,4} `?((?:Fleet|FleetPools)\.\w+)`?\s*$/gm)) {
          if (!out.has(m[1])) out.set(m[1], `/${rel}#${slug(m[1])}`);
        }
      }
    }
  };
  walk(dir);
  return out;
}

// -------------------------------------------------------------- Terraform

export interface TfAttr {
  name: string;
  kind: 'String' | 'Int64' | 'Bool' | 'List';
  /** Element kind of a `List` attribute. */
  element?: 'String' | 'Int64' | 'Bool';
  required: boolean;
  optional: boolean;
  computed: boolean;
  sensitive: boolean;
  description: string;
  oneOf?: string[];
  atLeast?: number;
  between?: [number, number];
  length?: [number, number];
  pattern?: string;
  requiresReplace: boolean;
  json: boolean;
}

export interface TfBlock {
  name: string;
  nesting: 'set' | 'single';
  attrs: TfAttr[];
}

export interface TfSchema {
  description: string;
  attrs: TfAttr[];
  blocks: TfBlock[];
}

function goString(lit: string): string {
  return JSON.parse(lit);
}

/** The entries of a Go `map[string]...{ "k": v, ... }` literal starting at its `{`. */
function goMapEntries(src: string, open: number): Array<[string, string]> {
  const close = matchBrace(src, open);
  const body = src.slice(open + 1, close);
  const out: Array<[string, string]> = [];
  let i = 0;
  while (i < body.length) {
    const k = /\s*"([^"]+)":\s*/y;
    k.lastIndex = i;
    const m = k.exec(body);
    if (!m) break;
    let j = k.lastIndex;
    const start = j;
    // value ends at the top-level comma
    let depth = 0;
    for (; j < body.length; j += 1) {
      const c = body[j];
      if (c === '"') {
        for (j += 1; body[j] !== '"'; j += 1) if (body[j] === '\\') j += 1;
      } else if ('{(['.includes(c)) depth += 1;
      else if ('})]'.includes(c)) depth -= 1;
      else if (c === ',' && depth === 0) break;
    }
    out.push([m[1], body.slice(start, j).trim()]);
    i = j + 1;
  }
  return out;
}

function tfAttr(name: string, v: string, regexes: Map<string, string>): TfAttr {
  const kind = /^schema\.(String|Int64|Bool|List)Attribute\{/.exec(v)?.[1] as TfAttr['kind'] | undefined;
  if (!kind) throw new Error(`terraform attribute ${name}: unsupported ${v.slice(0, 40)}`);
  const element =
    kind === 'List'
      ? (/ElementType:\s*types\.(String|Int64|Bool)Type/.exec(v)?.[1] as TfAttr['element'] | undefined)
      : undefined;
  if (kind === 'List' && !element) throw new Error(`terraform attribute ${name}: unsupported list ${v.slice(0, 60)}`);
  const desc = /Description:\s*("(?:[^"\\]|\\.)*")/.exec(v);
  const oneOf = /OneOf\(([^)]*)\)/.exec(v);
  const atLeast = /AtLeast\((\d+)\)/.exec(v);
  const between = /\.Between\((\d+),\s*(\d+)\)/.exec(v);
  const length = /LengthBetween\((\d+),\s*(\d+)\)/.exec(v);
  const re = /RegexMatches\((\w+)/.exec(v);
  return {
    name,
    kind,
    element,
    required: /Required:\s*true/.test(v),
    optional: /Optional:\s*true/.test(v),
    computed: /Computed:\s*true/.test(v),
    sensitive: /Sensitive:\s*true/.test(v),
    description: desc ? goString(desc[1]) : '',
    oneOf: oneOf ? [...oneOf[1].matchAll(/"([^"]*)"/g)].map((m) => m[1]) : undefined,
    atLeast: atLeast ? Number(atLeast[1]) : undefined,
    between: between ? [Number(between[1]), Number(between[2])] : undefined,
    length: length ? [Number(length[1]), Number(length[2])] : undefined,
    pattern: re ? regexes.get(re[1]) : undefined,
    requiresReplace: /RequiresReplace\(\)/.test(v),
    json: /jsontypes\.NormalizedType/.test(v),
  };
}

export function parseTfSchema(src: string, fnName: string): TfSchema {
  const regexes = new Map<string, string>();
  for (const m of src.matchAll(/var (\w+) = regexp\.MustCompile\(`([^`]*)`\)/g)) regexes.set(m[1], m[2]);
  const fn = src.indexOf(`func ${fnName}(`);
  if (fn < 0) throw new Error(`terraform: ${fnName} not found`);
  const fnBody = src.slice(fn, matchBrace(src, src.indexOf('{', fn)) + 1);
  const description = /Description:\s*("(?:[^"\\]|\\.)*")/.exec(fnBody);
  const attrsAt = fnBody.indexOf('Attributes: map[string]schema.Attribute{');
  const attrs = goMapEntries(fnBody, fnBody.indexOf('{', attrsAt + 'Attributes: map[string]schema.Attribute'.length)).map(
    ([k, v]) => tfAttr(k, v, regexes)
  );
  const blocks: TfBlock[] = [];
  const blocksAt = fnBody.indexOf('Blocks: map[string]schema.Block{');
  if (blocksAt >= 0) {
    for (const [k, v] of goMapEntries(fnBody, fnBody.indexOf('{', blocksAt + 'Blocks: map[string]schema.Block'.length))) {
      const nesting = /^schema\.SetNestedBlock/.test(v) ? 'set' : /^schema\.SingleNestedBlock/.test(v) ? 'single' : null;
      if (!nesting) throw new Error(`terraform block ${k}: unsupported ${v.slice(0, 40)}`);
      const at = v.indexOf('Attributes: map[string]schema.Attribute{');
      const nested = goMapEntries(v, v.indexOf('{', at + 'Attributes: map[string]schema.Attribute'.length)).map(([n, a]) =>
        tfAttr(n, a, regexes)
      );
      blocks.push({ name: k, nesting, attrs: nested });
    }
  }
  return { description: description ? goString(description[1]) : '', attrs, blocks };
}

/** `- \`name\` - text` argument lines of the provider's registry docs. */
export function parseTfDocs(md: string): { args: Map<string, string>; importCode?: string } {
  const args = new Map<string, string>();
  for (const m of md.matchAll(/^- `([\w.]+)`(?: \/ `([\w.]+)`)? - (.+)$/gm)) {
    args.set(m[1], m[3].trim());
    if (m[2]) args.set(m[2], m[3].trim());
  }
  const imp = /## Import\s*\n+```\w*\n([\s\S]*?)```/.exec(md);
  return { args, importCode: imp?.[1].trim() };
}

interface TfMapping {
  attributes: Array<{ name: string; cr?: string; crd_path?: string }>;
  blocks: Array<{
    name: string;
    cr: string;
    crd_path: string;
    collection: string;
    fields: Array<{ name: string; crd_path: string }>;
  }>;
}

// ----------------------------------------------------------------- errors

export interface ErrorEntry {
  name: string;
  message: string;
  doc: string;
  surfaces?: string;
}

/** `cua_fleet::Error::X => Error::Y` arms of cua-daemon's `From<cua_fleet::Error>`. */
/** Variants `cua_fleet::Error::is_not_found` can answer true for. */
export function notFoundVariants(src: string): Set<string> {
  const at = src.indexOf('pub fn is_not_found(&self) -> bool');
  if (at < 0) throw new Error('cua-fleet: is_not_found not found');
  const open = src.indexOf('{', at);
  const body = src.slice(open, matchBrace(src, open));
  return new Set([...body.matchAll(/Error::(\w+)[^=]*=>\s*(?!false)/g)].map((m) => m[1]));
}

export function daemonMapping(src: string): { map: Map<string, string>; notFound: string; other: string; guest: string[] } {
  const at = src.indexOf('impl From<cua_fleet::Error> for Error');
  if (at < 0) throw new Error('cua-daemon: From<cua_fleet::Error> not found');
  const open = src.indexOf('{', src.indexOf('match e', at));
  const body = src.slice(open, matchBrace(src, open));
  const map = new Map<string, string>();
  for (const m of body.matchAll(/cua_fleet::Error::(\w+)[^=]*=>\s*(?:\{\s*)?Error::(\w+)/g)) map.set(m[1], m[2]);
  const notFound = /is_not_found\(\)\s*=>\s*Error::(\w+)/.exec(body)?.[1];
  const other = /other\s*=>\s*Error::(\w+)/.exec(body)?.[1];
  if (!notFound || !other) throw new Error('cua-daemon: fleet error mapping changed');
  const g = src.indexOf('impl From<cua_spacesd_client::Error> for Error');
  const gOpen = src.indexOf('{', src.indexOf('match e', g));
  const guest = [...new Set([...src.slice(gOpen, matchBrace(src, gOpen)).matchAll(/=>\s*(?:\{\s*)?Error::(\w+)/g)].map((m) => m[1]))];
  if (g < 0 || !guest.length) throw new Error('cua-daemon: guest error mapping changed');
  return { map, notFound, other, guest };
}

// ------------------------------------------------------------ the objects

type RestRef = { client: string } | { swagger: string; method: string };

interface ObjectDef {
  slug: string;
  title: string;
  /** One line for the index and the page description. */
  summary: string;
  task: string;
  crd?: string;
  schema?: 'image';
  rest: Array<RestRef & { title?: string }>;
  sdk: string[];
  terraform?: 'pool' | 'template';
  extensionsFor?: string;
  example?: 'pool' | 'template' | 'claim';
}

const K8S_PASSTHROUGH = '/api/k8s/{path}';

export const OBJECTS: ObjectDef[] = [
  {
    slug: 'pool',
    title: 'Pool',
    summary: 'Warm capacity: how many sandboxes of one template Fleet keeps ready to claim.',
    task: 'Keep capacity',
    crd: 'OSGymSandboxWarmPool',
    rest: ['create_pool', 'list_pools', 'get_pool', 'update_pool', 'delete_pool'].map((client) => ({ client })),
    sdk: [
      'Fleet.apply_pool',
      'Fleet.apply',
      'Fleet.get_pool',
      'Fleet.list_pools',
      'Fleet.set_pool_replicas',
      'Fleet.wait_pool_ready',
      'Fleet.export_pool',
      'Fleet.delete_pool',
      'Fleet.ephemeral_pool_name',
      'Fleet.pools',
      'FleetPools.list',
      'FleetPools.gc',
      'FleetPools.gc_pools',
    ],
    terraform: 'pool',
    example: 'pool',
  },
  {
    slug: 'template',
    title: 'Template',
    summary: "What each sandbox of a pool runs: image, runtime, size, services and probes.",
    task: 'Keep capacity',
    crd: 'OSGymSandboxTemplate',
    rest: ['create_template', 'list_templates', 'get_template', 'update_template', 'delete_template'].map((client) => ({
      client,
    })),
    sdk: ['Fleet.apply_pool_template', 'Fleet.check_pool_spec', 'Fleet.list_templates'],
    terraform: 'template',
    extensionsFor: 'Template',
    example: 'template',
  },
  {
    slug: 'claim',
    title: 'Claim',
    summary: 'A lease on one sandbox from a pool, until it is released or expires.',
    task: 'Use a sandbox',
    crd: 'OSGymSandboxClaim',
    rest: ['create_claim', 'list_claims', 'get_claim', 'renew_claim', 'delete_claim'].map((client) => ({ client })),
    sdk: [
      'Fleet.acquire',
      'Fleet.acquire_with',
      'Fleet.claim',
      'Fleet.attach_claim',
      'Fleet.list_claims',
      'Fleet.keep_alive',
      'Fleet.release',
    ],
    extensionsFor: 'Claim',
    example: 'claim',
  },
  {
    slug: 'sandbox',
    title: 'Sandbox',
    summary: 'One running machine of a pool, bound to at most one claim.',
    task: 'Use a sandbox',
    crd: 'OSGymSandbox',
    rest: [],
    sdk: ['Fleet.attach_claim', 'Fleet.service_url'],
  },
  {
    slug: 'secret',
    title: 'Secret',
    summary: "Secrets the SDK writes in a pool's namespace: a claim's env token and registry pull credentials.",
    task: 'Use a sandbox',
    rest: [],
    sdk: [],
  },
  {
    slug: 'service-url',
    title: 'Service URL',
    summary: "HTTP and WebSocket access to a sandbox's services, and shareable signed URLs.",
    task: 'Use a sandbox',
    rest: [
      { swagger: '/api/svc/{namespace}/{service}/{path}', method: 'get', title: 'Call a service' },
      { swagger: '/api/signed-service-urls/{namespace}', method: 'post', title: 'Create a signed URL' },
      { swagger: '/api/signed-service-urls/{namespace}', method: 'get', title: 'List signed URLs' },
      { swagger: '/api/signed-service-urls/{namespace}/{id}', method: 'delete', title: 'Revoke a signed URL' },
    ],
    sdk: ['Fleet.service_url', 'Fleet.create_signed_service_url'],
  },
  {
    slug: 'image',
    title: 'Image',
    summary: 'A build recipe Fleet turns into a runnable image, and its build status.',
    task: 'Build images',
    schema: 'image',
    rest: [
      { client: 'create_image' },
      { client: 'list_images' },
      { client: 'get_image' },
      { client: 'delete_image' },
      { client: 'presign_image_uploads' },
    ],
    sdk: ['Fleet.create_image', 'Fleet.list_images', 'Fleet.get_image', 'Fleet.delete_image'],
  },
  {
    slug: 'namespace',
    title: 'Namespace',
    summary: 'The account-owned scope every pool, template, claim and secret lives in.',
    task: 'Account',
    rest: [
      { swagger: '/api/namespaces', method: 'post', title: 'Create a namespace' },
      { swagger: '/api/namespaces', method: 'get', title: 'List namespaces' },
      { swagger: '/api/namespaces/{name}', method: 'get', title: 'Get a namespace' },
      { swagger: '/api/namespaces/{name}', method: 'delete', title: 'Delete a namespace' },
    ],
    sdk: [],
  },
  {
    slug: 'api-key',
    title: 'API key',
    summary: 'OAuth client credentials that act for your user, for the SDK, CLI and Terraform.',
    task: 'Account',
    rest: [
      { swagger: '/api/user-keys', method: 'post', title: 'Create an API key' },
      { swagger: '/api/user-keys', method: 'get', title: 'List API keys' },
      { swagger: '/api/user-keys/{id}', method: 'delete', title: 'Revoke an API key' },
    ],
    sdk: [],
  },
];

// ------------------------------------------------------------ rendering

interface Ctx {
  dump: FleetDump;
  routes: Map<string, string>;
  ops: Map<string, ClientOp>;
  swagger: Swagger;
  handle: Map<string, HandleMethod>;
  tf: TfSchema;
  tfProvider: TfSchema;
  tfDocs: { args: Map<string, string>; importCode?: string };
  tfMapping: TfMapping;
  tfSource: string;
  tfExample?: { code: string; source: string };
  imageSchema: Schema;
  secretRoute: string;
  errors: { fleet: ErrorEntry[]; sdk: ErrorEntry[] };
  descriptions: Descriptions;
  version: string;
}

function crdOf(ctx: Ctx, kind: string): Crd {
  const crd = ctx.dump.crds.find((c) => c.spec.names.kind === kind);
  if (!crd) throw new Error(`CRD ${kind} missing from the dump`);
  return crd;
}

/** A CRD's fields with the public descriptions applied (a sandbox shares its template's `spec`). */
function crdFields(ctx: Ctx, kind: string) {
  const fields = flattenSchema(crdSchema(crdOf(ctx, kind)));
  const own = ctx.descriptions.crd[kind] ?? {};
  const out = applyOverrides(fields, own, kind);
  if (kind === 'OSGymSandbox') {
    const shared = ctx.descriptions.crd.OSGymSandboxTemplate ?? {};
    return applyOverrides(out, shared, kind, false).map((f) => (own[f.path] ? { ...f, description: own[f.path] } : f));
  }
  return out;
}

function crdSchema(crd: Crd): Schema {
  return crd.spec.versions[0].schema.openAPIV3Schema;
}

function apiVersion(crd: Crd): string {
  return `${crd.spec.group}/${crd.spec.versions[0].name}`;
}

const lower = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

function objectAnchor(def: ObjectDef): string {
  return slug(`The ${lower(def.title)} object`);
}

function sdkCell(ctx: Ctx, key: string): string {
  const m = ctx.handle.get(key);
  if (!m) throw new Error(`SDK method ${key} not found in cua-sdk native/fleet.rs`);
  const sig = `${key}(${m.params.join(', ')})`;
  return m.href ? `[${codeCell(sig)}](${m.href})` : codeCell(sig);
}

function restHeading(ctx: Ctx, ref: RestRef & { title?: string }): string {
  if ('client' in ref) {
    const op = ctx.ops.get(ref.client);
    if (!op) throw new Error(`Fleet client method ${ref.client} not found`);
    return ref.title ?? cap(op.op);
  }
  const op = swaggerOp(ctx, ref.swagger, ref.method);
  return ref.title ?? op.summary;
}

function swaggerOp(ctx: Ctx, p: string, method: string): SwaggerOp {
  const op = ctx.swagger.paths[p]?.[method];
  if (!op) throw new Error(`swagger: ${method.toUpperCase()} ${p} not found`);
  return op;
}

function restLine(ctx: Ctx, ref: RestRef): string {
  if ('client' in ref) {
    const op = ctx.ops.get(ref.client)!;
    return `${op.method} ${op.path}`;
  }
  return `${ref.method.toUpperCase()} ${ref.swagger}`;
}

function pathParams(p: string): string[] {
  return [...p.matchAll(/\{(\w+)\}/g)].map((m) => m[1]);
}

const PARAM_DOCS: Record<string, string> = {
  namespace: "The pool's namespace. A pool, its namespace and its template share one name.",
  name: 'The object name: a DNS label (lowercase letters, digits and `-`, at most 63 characters).',
  service: 'The Kubernetes Service of the sandbox service: `<sandbox>-<service>`.',
  path: 'The upstream path, forwarded verbatim (it must start with `/`).',
  id: 'The object id.',
  claim: 'The claim name.',
};

function definitionRef(ref?: string): string | undefined {
  return ref?.replace(/^#\/definitions\//, '');
}

function schemaRows(ctx: Ctx, def: string, intro: string): string[] {
  const schema = ctx.swagger.definitions[def];
  if (!schema) throw new Error(`swagger definition ${def} missing`);
  const fields = applyOverrides(
    flattenSchema(schema, { ...schema, definitions: ctx.swagger.definitions } as Schema),
    ctx.descriptions.swagger[def] ?? {},
    def
  );
  if (fields.every((f) => !f.description && !f.constraints.length)) {
    const list = fields.map((f) => `${codeCell(f.path)} (${f.type})`).join(', ');
    return [`${intro} an object with ${list}.`, ''];
  }
  return [`${intro}:`, '', ...fieldTable(fields)];
}

function renderRest(ctx: Ctx, def: ObjectDef): string[] {
  if (!def.rest.length) return [];
  const out = ['## REST', ''];
  const errorsLink = '[Errors](/fleets/reference/errors#http-status-codes)';
  for (const ref of def.rest) {
    out.push(`### ${restHeading(ctx, ref)}`, '');
    out.push(codeFence('http', restLine(ctx, ref)), '');
    if ('client' in ref) {
      const op = ctx.ops.get(ref.client)!;
      if (op.doc) out.push(escapeMdxText(flattenRustdocLinks(op.doc.replace(/\s*\n\s*/g, ' '))), '');
      if (/\{name\}\/\w+\/\{name\}/.test(op.path))
        out.push("A pool's namespace is its name, so the name fills both slots.", '');
      const params = [...new Set(pathParams(op.path))];
      if (params.length) {
        out.push('| Parameter | In | Description |', '| --- | --- | --- |');
        for (const p of params) {
          const where = op.path.includes(`?${p}=`) ? 'query' : 'path';
          out.push(`| ${codeCell(p)} | ${where} | ${escapeTableCell(PARAM_DOCS[p] ?? '')} |`);
        }
        out.push('');
      }
      const isItem = /\{name\}$/.test(op.path);
      const objectLink = def.crd || def.schema ? `[${lower(def.title)} object](#${objectAnchor(def)})` : 'object';
      if (ref.client === 'presign_image_uploads') {
        out.push(...presignBody(ctx));
      } else {
        const body =
          op.method === 'POST'
            ? `Body: the ${objectLink} (\`application/json\`).`
            : op.method === 'PATCH'
              ? `Body: a JSON merge patch of the ${objectLink} (\`application/merge-patch+json\`).`
              : '';
        if (body) out.push(body, '');
        const ok = op.statuses.filter((s) => s < 400);
        const tolerated = op.statuses.filter((s) => s >= 400);
        const returns =
          op.method === 'DELETE'
            ? 'with no body'
            : op.method === 'GET' && !isItem
              ? `with \`{"items": [...]}\`, a list of the ${objectLink}`
              : `with the ${objectLink}`;
        out.push(`Returns: ${ok.map((s) => `\`${s}\``).join(', ')} ${returns}.`, '');
        if (tolerated.length)
          out.push(`${tolerated.map((s) => `\`${s}\``).join(', ')} is treated as success: the object is already gone.`, '');
      }
      if (op.calls.includes('create_namespace_if_missing'))
        out.push(
          'The SDK first creates the namespace named after the pool ([Create a namespace](/fleets/reference/namespace#create-a-namespace)) and deletes it again if the pool is refused.',
          ''
        );
      if (op.calls.includes('delete_namespace') && !op.calls.includes('create_namespace_if_missing'))
        out.push('The SDK then deletes the namespace, with everything left in it.', '');
      if (op.path.startsWith('/api/k8s/')) {
        const passthrough = swaggerOp(ctx, K8S_PASSTHROUGH, 'get');
        out.push(`Errors: ${errorStatuses(passthrough)} (${errorsLink}).`, '');
      } else out.push(`Errors: see ${errorsLink}.`, '');
    } else {
      const op = swaggerOp(ctx, ref.swagger, ref.method);
      if (op.description && ref.swagger.startsWith('/api/signed-service-urls'))
        out.push(escapeMdxText(cleanDescription(op.description)), '');
      const params = (op.parameters ?? []).filter((p) => p.in !== 'body');
      if (params.length) {
        out.push('| Parameter | In | Description |', '| --- | --- | --- |');
        for (const p of params)
          out.push(
            `| ${codeCell(p.name)} | ${p.in} | ${escapeTableCell(
              [PARAM_DOCS[p.name] ?? cleanDescription(p.description), p.required ? '' : 'Optional.']
                .filter(Boolean)
                .join(' ')
            )} |`
          );
        out.push('');
      }
      const body = (op.parameters ?? []).find((p) => p.in === 'body');
      const bodyDef = definitionRef(body?.schema?.$ref);
      if (bodyDef) out.push(...schemaRows(ctx, bodyDef, 'Body (`application/json`)'));
      const okEntry = Object.entries(op.responses).find(([s]) => Number(s) < 400);
      if (okEntry) {
        const [status, r] = okEntry;
        const itemsRef = definitionRef((r.schema?.items as { $ref?: string } | undefined)?.$ref);
        const ref = definitionRef(r.schema?.$ref);
        if (ref) out.push(...schemaRows(ctx, ref, `Returns: \`${status}\` with`));
        else if (itemsRef) out.push(...schemaRows(ctx, itemsRef, `Returns: \`${status}\` with an array; each item is`));
        else out.push(`Returns: \`${status}\`. ${escapeMdxText(r.description)}.`, '');
      }
      out.push(`Errors: ${errorStatuses(op)} (${errorsLink}).`, '');
    }
  }
  return out;
}

function errorStatuses(op: SwaggerOp): string {
  return Object.keys(op.responses)
    .filter((s) => Number(s) >= 400)
    .map((s) => `[\`${s}\`](/fleets/reference/errors#http-${s})`)
    .join(', ');
}

function presignBody(ctx: Ctx): string[] {
  const src = read(path.join(SOURCES.sdkDir, 'image_uploads.rs'));
  // The request types carry no doc comments: list their fields inline.
  const fields = (name: string) =>
    structFields(src, name)
      .map((f) => `${codeCell(f.name)} (${jsonTypeOfRust(f.type).replace(/^ImageUploadFileRequest/, 'object').replace(/^PresignedPut$/, 'object')})`)
      .join(', ');
  void ctx;
  return [
    `Body (\`application/json\`): ${fields('ImageUploadRequest')}. Each of \`files[]\` has ${fields('ImageUploadFileRequest')}: the file's SHA-256 digest, size and name.`,
    '',
    `Returns: \`200\` with \`files[]\`, one instruction per file: ${fields('ImageUploadInstruction')}. \`reference\` is what an image recipe's \`files[].source.reference\` names. \`upload\` (${fields('PresignedPut')}) is a presigned request to send the bytes with; it is absent when Fleet already has the file.`,
    '',
  ];
}

function renderSdk(ctx: Ctx, def: ObjectDef): string[] {
  if (!def.sdk.length) return [];
  const out = [
    '## SDK',
    '',
    'Methods of the `Fleet` handle (`cua.fleet()`), with signatures in every language in the [Cua SDK reference](/cua-sdk/reference).',
    '',
    '| Method | Returns | Description |',
    '| --- | --- | --- |',
  ];
  for (const key of def.sdk) {
    const m = ctx.handle.get(key)!;
    if (!m) throw new Error(`SDK method ${key} not found`);
    out.push(`| ${sdkCell(ctx, key)} | ${neutralType(m.returns)} | ${escapeTableCell(m.summary)} |`);
  }
  return [...out, ''];
}

/** A Rust return type as the bindings show it: `FleetPool[]`, `string`, none. */
export function neutralType(t: string): string {
  const s = t.replace(/\s+/g, '');
  if (s === '()') return 'none';
  const vec = /^Vec<(.+)>$/.exec(s);
  if (vec) return codeCell(`${neutralType(vec[1]).replace(/`/g, '')}[]`);
  const opt = /^Option<(.+)>$/.exec(s);
  if (opt) return codeCell(`${neutralType(opt[1]).replace(/`/g, '')}?`);
  if (s === 'String') return codeCell('string');
  if (s === 'bool') return codeCell('bool');
  if (/^[ui](8|16|32|64)$/.test(s)) return codeCell('integer');
  return codeCell(s);
}

function tfType(a: TfAttr): string {
  if (a.json) return 'string (JSON)';
  const scalar = { String: 'string', Int64: 'number', Bool: 'bool' } as const;
  if (a.kind === 'List') return `list(${scalar[a.element!]})`;
  return scalar[a.kind];
}

function tfDefault(ctx: Ctx, a: TfAttr, crdPath?: { cr?: string; path?: string }): string {
  if (a.required) return 'required';
  if (crdPath?.cr && crdPath.path) {
    const kind = crdPath.cr === 'warmpool' ? 'OSGymSandboxWarmPool' : 'OSGymSandboxTemplate';
    const f = crdFields(ctx, kind).find((x) => x.path === crdPath.path);
    if (f?.default !== undefined) return codeCell(JSON.stringify(f.default));
  }
  return 'none';
}

function tfDescription(ctx: Ctx, a: TfAttr, docKey: string, crd?: { cr?: string; path?: string }): string {
  let text = ctx.tfDocs.args.get(docKey) ?? cleanDescription(a.description);
  if (!text && crd?.cr && crd.path) {
    const kind = crd.cr === 'warmpool' ? 'OSGymSandboxWarmPool' : 'OSGymSandboxTemplate';
    text = crdFields(ctx, kind).find((x) => x.path === crd.path)?.description ?? '';
  }
  const notes: string[] = [];
  const says = (v: string) => text.toLowerCase().includes(v.toLowerCase());
  if (a.oneOf && !a.oneOf.every((v) => says(`\`${v}\``))) notes.push(`One of ${a.oneOf.map((v) => `\`${v}\``).join(', ')}.`);
  if (a.length) notes.push(`${a.length[0]} to ${a.length[1]} characters.`);
  if (a.pattern) notes.push(`Matches ${codeCell(a.pattern)}.`);
  if (a.atLeast !== undefined) notes.push(`At least ${a.atLeast}.`);
  if (a.between) notes.push(`${a.between[0]} to ${a.between[1]}.`);
  if (a.requiresReplace && !says('replaces the resource')) notes.push('Changing it replaces the resource.');
  if (a.sensitive) notes.push('Sensitive.');
  return [escapeTableCell(text), ...notes].filter(Boolean).join(' ');
}

function tfTable(ctx: Ctx, attrs: TfAttr[], crdOfAttr: (a: TfAttr) => { cr?: string; path?: string } | undefined, prefix = ''): string[] {
  const rows = ['| Argument | Type | Default | Description |', '| --- | --- | --- | --- |'];
  for (const a of attrs) {
    const crd = crdOfAttr(a);
    rows.push(
      `| ${codeCell(prefix + a.name)} | ${codeCell(tfType(a))} | ${tfDefault(ctx, a, crd)} | ${tfDescription(ctx, a, prefix + a.name, crd)} |`
    );
  }
  return [...rows, ''];
}

function mappingFor(ctx: Ctx, name: string, block?: string): { cr?: string; path?: string } | undefined {
  if (block) {
    const b = ctx.tfMapping.blocks.find((x) => x.name === block);
    const f = b?.fields.find((x) => x.name === name);
    return b && f ? { cr: b.cr, path: `${b.crd_path}${b.collection === 'set' ? '[]' : ''}.${f.crd_path}` } : undefined;
  }
  const a = ctx.tfMapping.attributes.find((x) => x.name === name);
  return a?.cr ? { cr: a.cr, path: a.crd_path } : undefined;
}

function renderTerraform(ctx: Ctx, def: ObjectDef): string[] {
  if (def.terraform === 'template') {
    const rows = ['| Terraform argument | Template field |', '| --- | --- |'];
    for (const a of ctx.tfMapping.attributes.filter((x) => x.cr === 'template'))
      rows.push(`| [${codeCell(a.name)}](/fleets/reference/pool#terraform) | ${codeCell(a.crd_path!)} |`);
    for (const b of ctx.tfMapping.blocks.filter((x) => x.cr === 'template'))
      rows.push(`| [${codeCell(`${b.name} {}`)}](/fleets/reference/pool#terraform) | ${codeCell(b.crd_path)} |`);
    return [
      '## Terraform',
      '',
      'Terraform manages the template as part of its pool: [`fleets_pool`](/fleets/reference/pool#terraform) writes a template named `<pool>-template` from these arguments.',
      '',
      ...rows,
      '',
    ];
  }
  if (def.terraform !== 'pool') return [];
  const tf = ctx.tf;
  const inputs = tf.attrs.filter((a) => a.required || a.optional);
  const computed = tf.attrs.filter((a) => a.computed && !a.required && !a.optional);
  const out = ['## Terraform', '', `Resource \`fleets_pool\` of the [\`trycua/fleets\` provider](/fleets/reference/terraform). ${escapeMdxText(tf.description)}`, ''];
  if (ctx.tfExample) {
    out.push(
      `{/* Example source: ${ctx.tfExample.source} (tested by the docs terraform lane) */}`,
      '',
      codeFence('hcl', ctx.tfExample.code, 'title="main.tf"'),
      ''
    );
  }
  out.push('### Arguments', '', 'Exactly one of `replicas` or `autoscaling` is required.', '');
  out.push(...tfTable(ctx, inputs, (a) => mappingFor(ctx, a.name)));
  for (const b of tf.blocks) {
    const kind = b.nesting === 'set' ? 'Repeatable block' : 'Block';
    out.push(`### \`${b.name}\` block`, '', `${kind}. ${escapeTableCell(ctx.tfDocs.args.get(b.name) ?? '')}`.trim(), '');
    out.push(...tfTable(ctx, b.attrs, (a) => mappingFor(ctx, a.name, b.name), `${b.name}.`));
  }
  out.push('### Attributes', '', 'Read-only, set by the provider.', '', '| Attribute | Type | Description |', '| --- | --- | --- |');
  for (const a of computed) {
    const crd = mappingFor(ctx, a.name);
    const d = tfDescription(ctx, a, a.name, crd) || (a.name === 'id' ? 'The pool name.' : a.name === 'namespace' ? "The pool's namespace (the pool name)." : '');
    out.push(`| ${codeCell(a.name)} | ${codeCell(tfType(a))} | ${d} |`);
  }
  out.push('', '`replicas` is also set in autoscaling mode: it reports the current pool target.', '');
  if (ctx.tfDocs.importCode) out.push('### Import', '', 'Import IDs are pool names.', '', codeFence('shell', ctx.tfDocs.importCode), '');
  return out;
}

function renderObjectFields(ctx: Ctx, def: ObjectDef): string[] {
  const out: string[] = [];
  if (def.crd) {
    const crd = crdOf(ctx, def.crd);
    const fields = crdFields(ctx, def.crd).filter((f) => f.path !== 'spec' && f.path !== 'status');
    const spec = fields.filter((f) => f.path.startsWith('spec.'));
    const status = fields.filter((f) => f.path.startsWith('status.'));
    out.push(
      `## The ${lower(def.title)} object`,
      '',
      `\`apiVersion: ${apiVersion(crd)}\`, \`kind: ${crd.spec.names.kind}\`, in the pool's namespace. Plural \`${crd.spec.names.plural}\`${
        crd.spec.names.shortNames?.length ? `, short name \`${crd.spec.names.shortNames.join('`, `')}\`` : ''
      }. \`metadata\` is standard Kubernetes object metadata (\`name\`, \`namespace\`, \`labels\`).`,
      ''
    );
    out.push('### Spec', '', ...fieldTable(spec));
    if (status.length) out.push('### Status', '', 'Set by Fleet; read-only.', '', ...fieldTable(status));
  } else if (def.schema === 'image') {
    const root = ctx.imageSchema;
    out.push(
      `## The ${lower(def.title)} object`,
      '',
      `${escapeMdxText(cleanDescription(root.description))} \`apiVersion: images.cua.ai/v1alpha1\`, \`kind: Image\`. The same resource \`cua image build\` reads from \`image.json\`.`,
      ''
    );
    if (!ctx.dump.config.remote_builds_supported)
      out.push(
        'Fleet does not build `kind: container` recipes yet: a cloud Image with layers fails with a clear error. Build it locally and push it to a registry instead.',
        ''
      );
    const all = flattenSchema(root).filter((f) => !['apiVersion', 'kind', 'metadata', 'spec', 'status'].includes(f.path));
    out.push('### Metadata and spec', '', ...fieldTable(all.filter((f) => !f.path.startsWith('status'))));
    out.push('### Status', '', 'Set by Fleet; read-only.', '', ...fieldTable(all.filter((f) => f.path.startsWith('status'))));
    out.push(...layerTypes(root));
  }
  if (def.extensionsFor) {
    const ext = ctx.dump.extensions.filter((e) => e.object === def.extensionsFor);
    if (ext.length) {
      out.push(
        '### Fields the SDK adds',
        '',
        'Written as JSON beyond the published schema; Fleet reads them.',
        '',
        '| Field | Type | Description |',
        '| --- | --- | --- |'
      );
      for (const e of ext) out.push(`| ${codeCell(e.field)} | ${codeCell(e.type)} | ${escapeTableCell(e.description)} |`);
      out.push('');
    }
  }
  if (def.example) {
    out.push(
      '### Example',
      '',
      `What the SDK sends for a default pool of the cua Linux image (\`PoolSpec::new\`, built by \`cua-fleet\`).`,
      '',
      codeFence('json', JSON.stringify(ctx.dump.examples[def.example], null, 2)),
      ''
    );
  }
  return out;
}

function layerTypes(root: Schema): string[] {
  const recipe = flattenSchema(root).find((f) => f.path.endsWith('recipe.layers'));
  if (!recipe) return [];
  const spec = root.$defs?.ImageLayer;
  if (!spec) return [];
  const tagged = taggedVariants(spec, root);
  if (!tagged) return [];
  const out = ['### Layer types', '', '`spec.recipe.layers[]` holds build steps, each tagged by `type`.', '', '| `type` | Fields |', '| --- | --- |'];
  for (const v of tagged) {
    const fields = Object.entries(v.schema.properties ?? {})
      .filter(([k]) => k !== v.key)
      .map(([k, p]) => `${codeCell(k)} ${codeCell(typeOf(p, root))}`);
    out.push(`| ${codeCell(v.tag)} | ${fields.join(', ')} |`);
  }
  return [...out, ''];
}

function renderSecret(ctx: Ctx): string[] {
  const c = ctx.dump.secrets.claim;
  const r = ctx.dump.secrets.registry;
  return [
    '## Claim secret',
    '',
    `Every claim the SDK makes carries a fresh env token for the sandbox's cua-spacesd. The SDK writes it as an Opaque Secret \`${c.prefix}<claim>\` (key \`${c.key}\`, label \`${c.label}: <claim>\`), then creates the claim with \`spec.secretRef\` naming it. Fleet delivers it into the bound sandbox at \`/run/cua/env-token\`; the token never crosses the network to the guest. After the claim binds, the SDK waits up to ${c.wait_seconds} seconds for the guest to accept the token, then releases the claim with [\`ClaimSecretsNotDelivered\`](/fleets/reference/errors#claimsecretsnotdelivered).`,
    '',
    codeFence('json', JSON.stringify(c.example, null, 2)),
    '',
    '## Registry pull secret',
    '',
    `Credentials for a private registry, written as a \`kubernetes.io/dockerconfigjson\` Secret \`${r.prefix}<16 hex>\` (a hash of registry and user, so a new password updates the same Secret) and named by the template's \`imagePullSecret\`.`,
    '',
    codeFence('json', JSON.stringify(r.example, null, 2)),
    '',
    '## REST',
    '',
    'Secrets are core Kubernetes objects behind the same proxy as the Fleet resources.',
    '',
    '### Create a secret',
    '',
    codeFence('http', `POST ${ctx.secretRoute}`),
    '',
    'Body: the Secret. `409` means a Secret of that name exists (another claim attempt); the SDK does not overwrite it.',
    '',
    '### Delete a secret',
    '',
    codeFence('http', `DELETE ${ctx.secretRoute}/{name}`),
    '',
    '`404` is treated as success. The SDK deletes a claim\'s Secret when it releases the claim.',
    '',
    `Errors: ${errorStatuses(swaggerOp(ctx, K8S_PASSTHROUGH, 'get'))} ([Errors](/fleets/reference/errors#http-status-codes)).`,
    '',
  ];
}

function renderSandboxExtras(): string[] {
  return [
    'A sandbox is created by its pool, never directly. `status.sandbox` of a bound [claim](/fleets/reference/claim) names it; the SDK returns it as a `FleetSandbox` (namespace, claim, name and services).',
    '',
  ];
}

function renderObjectPage(ctx: Ctx, def: ObjectDef): string {
  const body: string[] = [];
  if (def.slug === 'sandbox') body.push(...renderSandboxExtras());
  if (def.rest.length || def.sdk.length) {
    const sections = [
      def.crd || def.schema ? `[Object](#${objectAnchor(def)})` : '',
      def.rest.length ? '[REST](#rest)' : '',
      def.sdk.length ? '[SDK](#sdk)' : '',
      def.terraform ? '[Terraform](#terraform)' : '',
    ].filter(Boolean);
    body.push(`On this page: ${sections.join(' · ')}`, '');
  }
  body.push(...renderObjectFields(ctx, def));
  if (def.slug === 'secret') body.push(...renderSecret(ctx));
  body.push(...renderRest(ctx, def), ...renderSdk(ctx, def), ...renderTerraform(ctx, def));
  return renderPage({
    title: def.title,
    description: def.summary,
    generator: REGENERATE,
    source: pageSource(def),
    version: ctx.version,
    body: body.join('\n'),
  });
}

function pageSource(def: ObjectDef): string {
  const s: string[] = [];
  if (def.crd) s.push('cua-fleet fleet-docs-dump (osgym.cua.ai CRDs)');
  if (def.schema === 'image') s.push('cua-image schema/image-v1alpha1.schema.json');
  if (def.slug === 'secret') s.push('cua-fleet fleet-docs-dump (secrets)');
  if (def.rest.some((r) => 'client' in r)) s.push('libs/fleet/sdk/src (routes, methods)');
  if (def.rest.some((r) => 'swagger' in r) || def.rest.length) s.push('libs/fleet/backend/docs/swagger.json');
  if (def.sdk.length) s.push('cua-sdk src/native/fleet.rs');
  if (def.terraform) s.push('terraform-provider-fleets schema');
  return s.join('; ');
}

function renderIndex(ctx: Ctx): string {
  const sec = ctx.swagger.securityDefinitions.BearerAuth;
  const body: string[] = [
    'Everything Cua Fleets exposes, by object: the REST resources behind the Fleet API, the SDK `Fleet` handle, and the `trycua/fleets` Terraform provider.',
    '',
    '## Objects',
    '',
    '| Task | Object | Description |',
    '| --- | --- | --- |',
  ];
  for (const def of OBJECTS)
    body.push(`| ${def.task} | [${def.title}](/fleets/reference/${def.slug}) | ${escapeTableCell(def.summary)} |`);
  body.push(
    `| Configure | [Terraform provider](/fleets/reference/terraform) | Provider settings and the resources it manages. |`,
    `| Debug | [Errors](/fleets/reference/errors) | HTTP statuses and SDK errors, with causes and fixes. |`,
    ''
  );
  body.push(
    '## Base URL and authentication',
    '',
    `- Base URL: \`${ctx.dump.config.default_base_url}\` (\`CUA_FLEET_BASE_URL\` overrides it for the SDK).`,
    `- Every request sends \`${sec.name}: Bearer <token>\`. Exchange an [API key](/fleets/reference/api-key) for a token with the OAuth client-credentials grant at \`${ctx.dump.config.default_token_url}\`.`,
    '- The SDK and CLI read `CUA_CLIENT_ID` and `CUA_CLIENT_SECRET`, a `cua auth login` session, or `FLEETS_TOKEN`.',
    '',
    '## Conventions',
    '',
    '- Pools, templates, claims and sandboxes are Kubernetes custom resources (`osgym.cua.ai/v1alpha1`) under `/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/{namespace}/`. Images are `images.cua.ai/v1alpha1`.',
    '- A pool owns the namespace of the same name; its template and claims live there too.',
    '- Names are DNS labels: lowercase letters, digits and `-`, at most 63 characters.',
    '- Lists return `{"items": [...]}`. Updates are JSON merge patches (`Content-Type: application/merge-patch+json`).',
    '- Errors outside the Kubernetes proxy return `{"error": "<message>"}`.',
    ''
  );
  body.push('## Operations', '', '| Object | REST | SDK | Terraform |', '| --- | --- | --- | --- |');
  for (const def of OBJECTS) {
    const rest = def.rest.map((r) => `[${restHeading(ctx, r)}](/fleets/reference/${def.slug}#${slug(restHeading(ctx, r))})`).join(', ');
    const sdk = def.sdk.filter((k) => k.startsWith('Fleet.')).map((k) => codeCell(k)).join(', ');
    const tf = def.terraform ? '`fleets_pool`' : '';
    body.push(`| [${def.title}](/fleets/reference/${def.slug}) | ${rest || 'none'} | ${sdk || 'none'} | ${tf || 'none'} |`);
  }
  body.push('');
  return renderPage({
    title: 'Reference',
    description: 'Cua Fleets objects, REST operations, SDK methods and Terraform schema.',
    generator: REGENERATE,
    source: 'fleet-docs-dump; libs/fleet (client, swagger, Terraform provider); cua-sdk native/fleet.rs',
    version: ctx.version,
    body: body.join('\n'),
  });
}

function renderTerraformPage(ctx: Ctx): string {
  const p = ctx.tfProvider;
  const envs = p.attrs.map((a) => /CYCLOPS_\w+/.exec(a.description)?.[0]).filter(Boolean);
  const body: string[] = [
    'The `trycua/fleets` provider manages Fleet pools. Install it from the Terraform Registry with `terraform init`; OpenTofu works the same with `tofu`.',
    '',
    codeFence(
      'hcl',
      ['terraform {', '  required_providers {', '    fleets = {', `      source = "${ctx.tfSource}"`, '    }', '  }', '}'].join('\n')
    ),
    '',
    '## Provider arguments',
    '',
    'Use either `access_token`, or all three of `client_id`, `client_secret` and `token_url` (an [API key](/fleets/reference/api-key)). An argument wins over its environment variable.',
    '',
    '| Argument | Type | Default | Environment variable | Description |',
    '| --- | --- | --- | --- | --- |',
  ];
  for (const a of p.attrs) {
    const env = /CYCLOPS_\w+/.exec(a.description)?.[0] ?? (a.name === 'endpoint' ? 'CYCLOPS_ENDPOINT' : '');
    const desc = a.description
      .replace(/\s*May also be set with CYCLOPS_\w+\.?/, '')
      .replace(/Cyclops base URL, for example https:\/\/cyclops\.example\.com\./, `Fleet API base URL, for example \`${ctx.dump.config.default_base_url}\`.`)
      .replace(/Cyclops /g, 'Fleet ');
    out(a, env, desc);
  }
  function out(a: TfAttr, env: string, desc: string) {
    body.push(
      `| ${codeCell(a.name)} | ${codeCell(tfType(a))} | ${a.required ? 'required' : 'none'} | ${env ? codeCell(env) : 'none'} | ${escapeTableCell(desc)}${a.sensitive ? ' Sensitive.' : ''} |`
    );
  }
  void envs;
  body.push(
    '',
    '`CUA_CLIENT_ID`, `CUA_CLIENT_SECRET`, `FLEETS_TOKEN` and `cua auth login` do not configure the provider.',
    '',
    '## Resources',
    '',
    '| Resource | Manages | Reference |',
    '| --- | --- | --- |',
    `| \`fleets_pool\` | ${escapeTableCell(ctx.tf.description)} | [Pool](/fleets/reference/pool#terraform) |`,
    '',
    'The provider has no data sources.',
    ''
  );
  return renderPage({
    title: 'Terraform provider',
    description: 'The trycua/fleets provider: settings, environment variables and resources.',
    generator: REGENERATE,
    source: 'terraform-provider-fleets internal/provider (provider.go, pool_generated.go), main.go',
    version: ctx.version,
    body: body.join('\n'),
  });
}

const HTTP_MEANING: Record<string, [cause: string, fix: string]> = {
  '400': ['The request is malformed: a missing field, a bad name, or a value out of range.', 'Check the body against the object reference; the `error` message names the field.'],
  '401': ['No token, or an expired or invalid one.', 'Send `Authorization: Bearer <token>` with a fresh token from your API key.'],
  '403': [
    'The token is valid but may not act on this namespace, or the proxy does not allow this request. On a read, Fleet answers 403 for a namespace that no longer exists; on a pool write, the pool name belongs to another account.',
    'Check the namespace and name. For a new pool, pick another pool name (names are unique across accounts).',
  ],
  '404': ['The object does not exist.', 'Check the name and namespace; list the collection to see what exists.'],
  '409': ['An object with that name exists.', 'Use another name, or read the existing object.'],
  '500': ['Fleet failed to process the request.', 'Retry; report persistent failures with the request and time.'],
  '502': ['Fleet could not reach the backend behind the gateway (the Kubernetes API or the sandbox service).', 'Retry. For a service URL, check that the sandbox is bound and its service is listening.'],
  '503': ['The feature is not configured in this environment (for example signed service URLs).', 'Use the SDK service URL instead, or contact support.'],
};

function renderErrorsPage(ctx: Ctx): string {
  const statuses = new Set<string>();
  for (const methods of Object.values(ctx.swagger.paths))
    for (const op of Object.values(methods))
      if (INCLUDED_TAGS.has(op.tags[0])) for (const s of Object.keys(op.responses)) if (Number(s) >= 400) statuses.add(s);
  statuses.add('404');
  statuses.add('409');
  const sorted = [...statuses].sort();
  const body: string[] = [
    'Every error the Fleet API and the Fleet SDK layer return, with its cause and fix. SDK languages surface them as [`CuaError`](/cua-sdk/reference/errors) cases, named in each entry.',
    '',
    '## HTTP status codes',
    '',
    'Errors from the gateway carry `{"error": "<message>"}`; errors from the Kubernetes proxy carry a Kubernetes `Status` object with `message` and `reason`.',
    '',
    '| Status | Meaning |',
    '| --- | --- |',
  ];
  for (const s of sorted) {
    const m = HTTP_MEANING[s];
    if (!m) throw new Error(`no meaning for HTTP ${s}: add it to HTTP_MEANING`);
    body.push(`| [\`${s}\`](#http-${s}) | ${escapeTableCell(m[0])} |`);
  }
  body.push('');
  for (const s of sorted) {
    const [cause, fix] = HTTP_MEANING[s];
    body.push(`### HTTP ${s}`, '', escapeMdxText(cause), '', `Fix: ${escapeMdxText(fix)}`, '');
  }
  body.push('## SDK errors', '', 'Raised by the Fleet layer of the SDK (`cua_fleet::Error`).', '');
  body.push('| Error | Surfaces as |', '| --- | --- |');
  for (const e of ctx.errors.fleet) body.push(`| [\`${e.name}\`](#${slug(e.name)}) | ${surfacesCell(e.surfaces)} |`);
  body.push('');
  for (const e of ctx.errors.fleet) body.push(...errorEntry(e));
  body.push(
    '## Fleet API client errors',
    '',
    'The Fleet API client under the SDK (`SdkError`). They reach SDK callers through `Sdk` above: as `CuaError.NotFound` for a missing object, otherwise `CuaError.Fleet`, with this message.',
    ''
  );
  for (const e of ctx.errors.sdk) body.push(...errorEntry(e));
  return renderPage({
    title: 'Errors',
    description: 'Fleet HTTP status codes and SDK errors, with causes and fixes.',
    generator: REGENERATE,
    source: 'swagger.json responses; cua-fleet src/lib.rs Error; libs/fleet/sdk/src/error.rs SdkError; cua-daemon mapping',
    version: ctx.version,
    body: body.join('\n'),
  });
}

function errorEntry(e: ErrorEntry): string[] {
  const out = [`### ${e.name}`, ''];
  if (e.doc) out.push(escapeMdxText(flattenRustdocLinks(e.doc.replace(/\s*\n\s*/g, ' '))), '');
  if (e.message && e.message !== 'transparent') out.push(codeFence('text', messageTemplate(e.message)), '');
  if (e.surfaces) out.push(`Surfaces as ${surfacesCell(e.surfaces)}.`, '');
  return out;
}

/** `NotFound (a missing object), else Fleet` with each case as `CuaError.X` code. */
function surfacesCell(s: string): string {
  return s.replace(/\b([A-Z]\w+)\b/g, (m) => `\`CuaError.${m}\``);
}

/** `{field:?}` and `{}` placeholders to readable `<field>` slots. */
export function messageTemplate(msg: string): string {
  return msg.replace(/\{(\w*)(?::\?)?\}/g, (_, name: string) => (name ? `<${name}>` : '<detail>'));
}

const INCLUDED_TAGS = new Set(['passthrough', 'gateway', 'namespaces', 'signed-service-urls', 'user-keys']);

// ------------------------------------------------------------------ build

function tfExample(): { code: string; source: string } | undefined {
  const dir = SOURCES.fleetDocs;
  const walk = (d: string): string[] =>
    fs.existsSync(d)
      ? fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(d, e.name)) : e.name.endsWith('.mdx') ? [path.join(d, e.name)] : []))
      : [];
  for (const file of walk(dir).sort()) {
    if (file.startsWith(OUT_DIR)) continue;
    const m = /^```hcl[^\n]*test="terraform"[^\n]*id="main-tf"[^\n]*\n([\s\S]*?)^```$/m.exec(read(file));
    if (m) return { code: m[1], source: `${path.relative(DOCS_CONTENT, file).replace(/\\/g, '/')}#main-tf` };
  }
  return undefined;
}

export function buildContext(dump: FleetDump): Ctx {
  const routes = parseRoutes(read(path.join(SOURCES.sdkDir, 'routes.rs')));
  const ops = parseClientOps(SOURCES.sdkDir, routes);
  const swagger: Swagger = JSON.parse(read(SOURCES.swagger));
  const handle = parseHandle(read(SOURCES.handle), sdkAnchors(SOURCES.sdkReference));
  const providerDir = path.join(SOURCES.tfDir, 'internal', 'provider');
  const tf = parseTfSchema(read(path.join(providerDir, 'pool_generated.go')), 'poolResourceSchema');
  const providerSrc = read(path.join(providerDir, 'provider.go'));
  const schemaFn = /func \(p \*fleetsProvider\) Schema\(/.exec(providerSrc);
  if (!schemaFn) throw new Error('provider.go: Schema not found');
  const tfProvider = parseTfSchema(providerSrc.replace('func (p *fleetsProvider) Schema(', 'func providerSchema('), 'providerSchema');
  const tfSource = /Address:\s*"registry\.terraform\.io\/([^"]+)"/.exec(read(path.join(SOURCES.tfDir, 'main.go')))?.[1];
  if (!tfSource) throw new Error('terraform main.go: registry address not found');
  const claimSrc = read(SOURCES.claimSecrets);
  const secret = /format!\("\{base\}(\/api\/k8s\/api\/v1\/namespaces\/)\{ns\}(\/secrets)"\)/.exec(claimSrc);
  if (!secret) throw new Error('claim_secrets.rs: secret collection route not found');
  const daemon = daemonMapping(read(SOURCES.daemonLib));
  const fleetSrc = read(SOURCES.fleetLib);
  const notFound = notFoundVariants(fleetSrc);
  const fleetErrors = enumVariants(fleetSrc, 'Error').map((v) => {
    let surfaces = daemon.map.get(v.name) ?? daemon.other;
    if (surfaces === 'from') surfaces = daemon.guest.join(', ');
    if (notFound.has(v.name)) surfaces = v.name === 'Sdk' ? `${daemon.notFound} (a missing object), else ${surfaces}` : daemon.notFound;
    const message = v.message === '{}' && v.name === 'MissingCredentials' ? dump.config.missing_credentials : v.message;
    return { name: v.name, doc: v.doc, message, surfaces };
  });
  const sdkErrors = enumVariants(read(path.join(SOURCES.sdkDir, 'error.rs')), 'SdkError').map((v) => ({
    name: v.name,
    doc: v.doc,
    message: v.message,
  }));
  for (const e of sdkErrors) if (fleetErrors.some((f) => f.name === e.name)) e.name = `SdkError.${e.name}`;
  const version = read(R('libs', 'cua', 'VERSION')).trim();
  return {
    dump,
    routes,
    ops,
    swagger,
    handle,
    tf,
    tfProvider,
    tfDocs: parseTfDocs(read(path.join(SOURCES.tfDir, 'docs', 'resources', 'pool.md'))),
    tfMapping: JSON.parse(read(path.join(providerDir, 'generate', 'pool_mapping.json'))),
    tfSource,
    tfExample: tfExample(),
    imageSchema: JSON.parse(read(SOURCES.imageSchema)),
    secretRoute: `${secret[1]}{namespace}${secret[2]}`,
    errors: { fleet: fleetErrors, sdk: sdkErrors },
    descriptions: JSON.parse(read(DESCRIPTIONS_FILE)),
    version,
  };
}

export function renderAll(ctx: Ctx): Map<string, string> {
  const files = new Map<string, string>();
  files.set(path.join(OUT_DIR, 'index.mdx'), renderIndex(ctx));
  for (const def of OBJECTS) files.set(path.join(OUT_DIR, `${def.slug}.mdx`), renderObjectPage(ctx, def));
  files.set(path.join(OUT_DIR, 'terraform.mdx'), renderTerraformPage(ctx));
  files.set(path.join(OUT_DIR, 'errors.mdx'), renderErrorsPage(ctx));
  files.set(
    path.join(OUT_DIR, 'meta.json'),
    metaJson('Reference', ['index', ...OBJECTS.map((d) => d.slug), 'terraform', 'errors'])
  );
  return files;
}

function main(): void {
  const checkOnly = isCheckMode();
  const ctx = buildContext(loadDump());
  finish('Cua Fleets', syncFiles(renderAll(ctx), checkOnly, [OUT_DIR]), checkOnly, REGENERATE);
}

if (require.main === module) main();
