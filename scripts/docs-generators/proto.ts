#!/usr/bin/env npx tsx

/**
 * gRPC contract reference generator (cua.env.v1 and cua.daemon.v1).
 *
 * Source: `buf build libs/cua/proto -o -#format=json`, a FileDescriptorSet as
 * proto3 JSON that keeps SourceCodeInfo, so every leading comment (mandatory
 * under the module's `COMMENTS` lint) reaches the page. The gRPC-Web
 * fallbacks, metadata keys and protocol revision come from
 * `libs/cua/crates/cua-proto/src/lib.rs`.
 *
 * Usage:
 *   pnpm --dir docs docs:generate:cua-proto
 *   pnpm --dir docs docs:check:cua-proto     # drift gate (CI)
 *
 * Set BUF=/path/to/buf to pick the binary (CI pins buf 1.73.0).
 */

import { execFileSync } from 'child_process';
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
  readHeader,
  renderPage,
  slug,
  syncFiles,
  wordCount,
} from './lib/mdx';

// ============================================================================
// Descriptor types (the subset of descriptor.proto the renderer reads)
// ============================================================================

export interface FieldDescriptor {
  name: string;
  number: number;
  label?: string;
  type?: string;
  typeName?: string;
  oneofIndex?: number;
  proto3Optional?: boolean;
  options?: { deprecated?: boolean };
}

export interface MessageDescriptor {
  name: string;
  field?: FieldDescriptor[];
  nestedType?: MessageDescriptor[];
  enumType?: EnumDescriptor[];
  oneofDecl?: Array<{ name: string }>;
  options?: { mapEntry?: boolean; deprecated?: boolean };
}

export interface EnumDescriptor {
  name: string;
  value?: Array<{ name: string; number: number; options?: { deprecated?: boolean } }>;
  options?: { deprecated?: boolean };
}

export interface MethodDescriptor {
  name: string;
  inputType: string;
  outputType: string;
  clientStreaming?: boolean;
  serverStreaming?: boolean;
  options?: { deprecated?: boolean };
}

export interface ServiceDescriptor {
  name: string;
  method?: MethodDescriptor[];
  options?: { deprecated?: boolean };
}

export interface FileDescriptor {
  name: string;
  package?: string;
  messageType?: MessageDescriptor[];
  enumType?: EnumDescriptor[];
  service?: ServiceDescriptor[];
  sourceCodeInfo?: { location?: Array<{ path?: number[]; leadingComments?: string }> };
}

export interface FileDescriptorSet {
  file: FileDescriptor[];
}

/** Facts from cua-proto's lib.rs that the descriptors do not carry. */
export interface ContractFacts {
  /** `/pkg.Service/Method` of a client stream -> its unary gRPC-Web fallback. */
  fallbacks: Array<[string, string]>;
  /** Metadata keys and HTTP paths: constant name -> [value, doc]. */
  metadata: Array<{ name: string; value: string; doc: string }>;
  protocolVersion: string;
  protocolRevision: string;
}

// ============================================================================
// Page layout
// ============================================================================

export interface PageSpec {
  slug: string;
  title: string;
  description: string;
  intro: string;
  files: string[];
}

const ROUTE = '/cua-sdk/reference/protocol';
const OUTPUT_DIR = path.join(DOCS_CONTENT, 'cua-sdk', 'reference', 'protocol');
const GENERATOR = 'pnpm --dir docs docs:generate:cua-proto';
/** Keeps each page readable (the reference checklist caps pages at 5,000 words). */
export const MAX_WORDS = 5000;
export const MAX_BYTES = 60_000;
const PROTO_DIR = path.join(REPO_ROOT, 'libs', 'cua', 'proto');
const LIB_RS = path.join(REPO_ROOT, 'libs', 'cua', 'crates', 'cua-proto', 'src', 'lib.rs');
const VERSION_FILE = path.join(REPO_ROOT, 'libs', 'cua', 'VERSION');

/**
 * Every contract file belongs to exactly one page. A new .proto file fails
 * generation until it is placed here, so the nav never silently grows.
 */
export const PAGES: PageSpec[] = [
  {
    slug: 'env-system',
    title: 'System',
    description: 'cua.env.v1 SystemService: capabilities, init, health, shutdown, viewer tickets and relay attach.',
    intro: 'The handshake (`GetCapabilities`), session init, health, shutdown, viewer tickets and joining a relay (`AttachRelay`).',
    files: ['cua/env/v1/system.proto'],
  },
  {
    slug: 'env-diagnose',
    title: 'Diagnose',
    description: 'cua.env.v1 Diagnose: the image self-test report `SystemService.Diagnose` streams.',
    intro: 'The report of the image self-test (`SystemService.Diagnose` / `DiagnoseOnce`): checks, their evidence and the summary.',
    files: ['cua/env/v1/diagnose.proto'],
  },
  {
    slug: 'env-common',
    title: 'Shared types and errors',
    description: 'The cua.env.v1 types every spacesd service shares: ErrorInfo, principals and geometry.',
    intro: 'Errors (`google.rpc.Status` carrying `ErrorInfo`), principals and geometry, used by every `cua.env.v1` service.',
    files: ['cua/env/v1/common.proto'],
  },
  {
    slug: 'env-process',
    title: 'Processes',
    description: 'cua.env.v1 ProcessService: start, attach to, feed and signal guest processes.',
    intro: 'Detached, reattachable guest processes with PTY support and bounded scrollback.',
    files: ['cua/env/v1/process.proto'],
  },
  {
    slug: 'env-filesystem',
    title: 'Filesystem',
    description:
      'cua.env.v1 FilesystemService: stat, list, watch, read, chunked upload and signed URLs.',
    intro: 'File and directory operations, watchers, chunked transfers and signed HTTP URLs.',
    files: ['cua/env/v1/filesystem.proto'],
  },
  {
    slug: 'env-computer',
    title: 'Computer and driver',
    description:
      'cua.env.v1 ComputerService and DriverService: screenshots, pointer, keyboard, clipboard, displays and Cua Driver tools.',
    intro:
      'Desktop control. These services need the desktop provider; input goes through Cua Driver, which `DriverService` also exposes tool by tool.',
    files: ['cua/env/v1/computer.proto', 'cua/env/v1/driver.proto'],
  },
  {
    slug: 'env-windows',
    title: 'Windows and accessibility',
    description: 'cua.env.v1 WindowsService and AccessibilityService: list, watch and arrange windows, launch apps, and the accessibility tree.',
    intro: 'Windows, app launch and the accessibility tree. These services need the desktop provider.',
    files: ['cua/env/v1/windows.proto', 'cua/env/v1/accessibility.proto'],
  },
  {
    slug: 'env-stream',
    title: 'Streams and presence',
    description:
      'cua.env.v1 StreamService and PresenceService: media sessions and multiplayer cursors.',
    intro:
      'Media sessions (video and audio over the media wire, see `libs/cua/proto/MEDIA.md`) and presence for shared sessions.',
    files: ['cua/env/v1/stream.proto', 'cua/env/v1/presence.proto'],
  },
  {
    slug: 'env-teleport-tunnel',
    title: 'Teleport, tunnels and the volume',
    description:
      'cua.env.v1 TeleportService, TunnelService and VolumeService: receiving teleported sessions and files, port forwards, hotspots and the guest mount of Cua Volume.',
    intro:
      'The receiving side of teleport (sending lives in the cua SDK), TCP port forwards, the reverse-SOCKS hotspot, and the guest mount of Cua Volume served by the client.',
    files: ['cua/env/v1/teleport.proto', 'cua/env/v1/tunnel.proto', 'cua/env/v1/volume.proto'],
  },
  {
    slug: 'env-host-spaces',
    title: 'Host Spaces',
    description:
      'cua.env.v1 HostSpacesService: Spaces a host creates for its owner\'s devices through the relay.',
    intro:
      'Spaces a machine set up with `cua host setup --provide-spaces` creates for its owner\'s enrolled devices: settings, capacity, create and delete. The host\'s cua-spacesd forwards each call to its cua daemon.',
    files: ['cua/env/v1/host.proto'],
  },
  {
    slug: 'daemon-sandboxes',
    title: 'Daemon: sandboxes',
    description:
      'cua.daemon.v1 SandboxService: the local cua daemon API for sandboxes in every location.',
    intro:
      'The local `cua daemon` API (a Unix socket on the host, not cua-spacesd): sandbox lifecycle across locations.',
    files: ['cua/daemon/v1/sandboxes.proto'],
  },
  {
    slug: 'daemon-runtimes',
    title: 'Daemon: local runtimes',
    description:
      'cua.daemon.v1 RuntimeService: the local engines and images the cua daemon orchestrates.',
    intro:
      'The local runtimes (gVisor, runc, QEMU, Lume) and images the `cua daemon` orchestrates.',
    files: ['cua/daemon/v1/runtime.proto'],
  },
  {
    slug: 'daemon-spaces',
    title: 'Daemon: Spaces',
    description: 'cua.daemon.v1 SpaceService: Spaces and the hosts that provide them, through the local cua daemon.',
    intro: 'Spaces through the local `cua daemon`: create, list, power, delete, and the machines that provide Spaces.',
    files: ['cua/daemon/v1/spaces.proto'],
  },
  {
    slug: 'daemon-service',
    title: 'Daemon: service',
    description: 'cua.daemon.v1 DaemonService: the local cua daemon identity and the loopback media bridge for webviews.',
    intro: 'The `cua daemon` identity (`GetInfo`) and the loopback media bridge for webviews.',
    files: ['cua/daemon/v1/daemon.proto'],
  },
];

// ============================================================================
// Inputs
// ============================================================================

export function buildDescriptorSet(buf = process.env.BUF || 'buf'): FileDescriptorSet {
  const json = execFileSync(buf, ['build', PROTO_DIR, '-o', '-#format=json'], {
    cwd: REPO_ROOT,
    encoding: 'utf-8',
    maxBuffer: 256 * 1024 * 1024,
  });
  return JSON.parse(json) as FileDescriptorSet;
}

/** Reads fallbacks, metadata keys and the protocol revision from lib.rs. */
export function parseContractFacts(libRs: string): ContractFacts {
  const fallbackBlock = libRs.match(/CLIENT_STREAM_FALLBACKS[^=]*=\s*&\[([\s\S]*?)\];/);
  const fallbacks: Array<[string, string]> = [];
  if (fallbackBlock) {
    for (const m of fallbackBlock[1].matchAll(/\(\s*"([^"]+)",\s*"([^"]+)",?\s*\)/g)) {
      fallbacks.push([m[1], m[2]]);
    }
  }
  const metadata: ContractFacts['metadata'] = [];
  const metaBlock = libRs.match(/pub mod metadata \{([\s\S]*?)\n\}/);
  if (metaBlock) {
    let doc: string[] = [];
    for (const line of metaBlock[1].split('\n')) {
      const d = line.match(/^\s*\/\/\/\s?(.*)$/);
      if (d) {
        doc.push(d[1]);
        continue;
      }
      const c = line.match(/pub const (\w+): &str = "([^"]*)";/);
      if (c) metadata.push({ name: c[1], value: c[2], doc: doc.join(' ').trim() });
      doc = [];
    }
  }
  const num = (name: string) => libRs.match(new RegExp(`${name}: u32 = (\\d+);`))?.[1] ?? '';
  return {
    fallbacks,
    metadata,
    protocolVersion: num('ENV_PROTOCOL_VERSION'),
    protocolRevision: num('ENV_PROTOCOL_REVISION'),
  };
}

// ============================================================================
// Rendering
// ============================================================================

interface TypeRef {
  page: string;
  anchor: string;
  display: string;
}

/** Leading comments by source path (`4,0,2,1`). */
function commentIndex(file: FileDescriptor): Map<string, string> {
  const out = new Map<string, string>();
  for (const loc of file.sourceCodeInfo?.location ?? []) {
    if (loc.path && loc.leadingComments) out.set(loc.path.join(','), loc.leadingComments);
  }
  return out;
}

/** A proto comment as Markdown: one leading space stripped, MDX escaped. */
export function commentMarkdown(raw: string | undefined): string {
  if (!raw) return '';
  return raw
    .replace(/\n$/, '')
    .split('\n')
    .map((line) => line.replace(/^ /, ''))
    .map((line) => (/^\s*#/.test(line) ? line.replace('#', '\\#') : line))
    .map((line) => escapeMdxText(line))
    .join('\n')
    .trim();
}

/** A comment flattened for a table cell. */
function commentCell(raw: string | undefined): string {
  if (!raw) return '';
  return escapeTableCell(
    raw
      .split('\n')
      .map((l) => l.trim())
      .join(' ')
      .replace(/\s+/g, ' ')
  );
}

const SCALARS: Record<string, string> = {
  TYPE_DOUBLE: 'double',
  TYPE_FLOAT: 'float',
  TYPE_INT64: 'int64',
  TYPE_UINT64: 'uint64',
  TYPE_INT32: 'int32',
  TYPE_FIXED64: 'fixed64',
  TYPE_FIXED32: 'fixed32',
  TYPE_BOOL: 'bool',
  TYPE_STRING: 'string',
  TYPE_BYTES: 'bytes',
  TYPE_UINT32: 'uint32',
  TYPE_SFIXED32: 'sfixed32',
  TYPE_SFIXED64: 'sfixed64',
  TYPE_SINT32: 'sint32',
  TYPE_SINT64: 'sint64',
};

class Renderer {
  /** Fully qualified name (`.cua.env.v1.Foo`) -> where it is documented. */
  private readonly types = new Map<string, TypeRef>();
  /** Map-entry messages, rendered inline as `map<K, V>`. */
  private readonly mapEntries = new Map<string, MessageDescriptor>();

  constructor(
    private readonly set: FileDescriptorSet,
    private readonly pages: PageSpec[],
    private readonly facts: ContractFacts
  ) {
    const owner = new Map<string, string>();
    for (const page of pages) for (const f of page.files) owner.set(f, page.slug);
    for (const file of this.contractFiles()) {
      const page = owner.get(file.name);
      if (!page) {
        throw new Error(
          `${file.name} is not assigned to a reference page; add it to PAGES in scripts/docs-generators/proto.ts`
        );
      }
      const pkg = `.${file.package}`;
      const visitMessage = (m: MessageDescriptor, prefix: string) => {
        const display = prefix ? `${prefix}.${m.name}` : m.name;
        const fq = `${pkg}.${display}`;
        if (m.options?.mapEntry) this.mapEntries.set(fq, m);
        else this.types.set(fq, { page, anchor: slug(display), display });
        for (const e of m.enumType ?? []) {
          const ed = `${display}.${e.name}`;
          this.types.set(`${pkg}.${ed}`, { page, anchor: slug(ed), display: ed });
        }
        for (const n of m.nestedType ?? []) visitMessage(n, display);
      };
      for (const m of file.messageType ?? []) visitMessage(m, '');
      for (const e of file.enumType ?? []) {
        this.types.set(`${pkg}.${e.name}`, { page, anchor: slug(e.name), display: e.name });
      }
    }
    for (const page of pages) {
      for (const f of page.files) {
        if (!this.set.file.some((file) => file.name === f)) {
          throw new Error(`${f} is listed in PAGES but buf did not build it`);
        }
      }
    }
  }

  contractFiles(): FileDescriptor[] {
    return this.set.file
      .filter((f) => f.package?.startsWith('cua.'))
      .sort((a, b) => a.name.localeCompare(b.name));
  }

  private file(name: string): FileDescriptor {
    return this.set.file.find((f) => f.name === name)!;
  }

  /** A type reference as a (linked) inline code span. */
  private typeLink(fq: string, fromPage: string): string {
    const ref = this.types.get(fq);
    if (!ref) {
      // Well-known and external types: name without the leading dot.
      return codeCell(fq.replace(/^\./, ''));
    }
    const href = ref.page === fromPage ? `#${ref.anchor}` : `${ROUTE}/${ref.page}#${ref.anchor}`;
    return `[${codeCell(ref.display)}](${href})`;
  }

  private fieldType(f: FieldDescriptor, page: string, oneofs: string[]): string {
    let base: string;
    const entry = f.typeName ? this.mapEntries.get(f.typeName) : undefined;
    if (entry) {
      const [k, v] = entry.field ?? [];
      return `map of ${this.fieldType(k, page, [])} to ${this.fieldType(v, page, [])}`;
    }
    if (f.type === 'TYPE_MESSAGE' || f.type === 'TYPE_ENUM')
      base = this.typeLink(f.typeName ?? '', page);
    else
      base = codeCell(SCALARS[f.type ?? ''] ?? (f.type ?? '').replace(/^TYPE_/, '').toLowerCase());
    if (f.label === 'LABEL_REPEATED') return `repeated ${base}`;
    if (f.proto3Optional) return `optional ${base}`;
    if (f.oneofIndex !== undefined && oneofs[f.oneofIndex])
      return `${base} (oneof ${codeCell(oneofs[f.oneofIndex])})`;
    return base;
  }

  private renderMessage(
    m: MessageDescriptor,
    prefix: string,
    pathPrefix: number[],
    comments: Map<string, string>,
    page: string,
    pkg: string
  ): string[] {
    if (m.options?.mapEntry) return [];
    const display = prefix ? `${prefix}.${m.name}` : m.name;
    const out: string[] = [`### ${display}`, ''];
    const doc = commentMarkdown(comments.get(pathPrefix.join(',')));
    if (m.options?.deprecated) out.push('**Deprecated.**', '');
    if (doc) out.push(doc, '');
    const fields = m.field ?? [];
    // Synthetic oneofs (proto3 `optional`) are not user-visible groups.
    const oneofs = (m.oneofDecl ?? []).map((o, i) =>
      fields.some((f) => f.oneofIndex === i && f.proto3Optional) ? '' : o.name
    );
    if (fields.length) {
      out.push('| Field | # | Type | Description |', '| --- | --- | --- | --- |');
      fields.forEach((f, i) => {
        let desc = commentCell(comments.get([...pathPrefix, 2, i].join(',')));
        if (f.options?.deprecated) desc = `Deprecated. ${desc}`.trim();
        out.push(
          `| ${codeCell(f.name)} | ${f.number} | ${this.fieldType(f, page, oneofs)} | ${desc} |`
        );
      });
      out.push('');
    } else {
      out.push('No fields.', '');
    }
    (m.enumType ?? []).forEach((e, i) => {
      out.push(...this.renderEnum(e, display, [...pathPrefix, 4, i], comments));
    });
    (m.nestedType ?? []).forEach((n, i) => {
      out.push(...this.renderMessage(n, display, [...pathPrefix, 3, i], comments, page, pkg));
    });
    return out;
  }

  private renderEnum(
    e: EnumDescriptor,
    prefix: string,
    pathPrefix: number[],
    comments: Map<string, string>
  ): string[] {
    const display = prefix ? `${prefix}.${e.name}` : e.name;
    const out: string[] = [`### ${display}`, ''];
    const doc = commentMarkdown(comments.get(pathPrefix.join(',')));
    if (e.options?.deprecated) out.push('**Deprecated.**', '');
    if (doc) out.push(doc, '');
    out.push('| Value | # | Description |', '| --- | --- | --- |');
    (e.value ?? []).forEach((v, i) => {
      let desc = commentCell(comments.get([...pathPrefix, 2, i].join(',')));
      if (v.options?.deprecated) desc = `Deprecated. ${desc}`.trim();
      out.push(`| ${codeCell(v.name)} | ${v.number} | ${desc} |`);
    });
    out.push('');
    return out;
  }

  private rpcKind(m: MethodDescriptor): string {
    if (m.clientStreaming && m.serverStreaming) return 'bidi stream';
    if (m.clientStreaming) return 'client stream';
    if (m.serverStreaming) return 'server stream';
    return 'unary';
  }

  private renderService(
    s: ServiceDescriptor,
    index: number,
    file: FileDescriptor,
    comments: Map<string, string>,
    page: string
  ): string[] {
    const fqService = `${file.package}.${s.name}`;
    const out: string[] = [`## ${s.name}`, '', `${codeCell(fqService)}`, ''];
    const doc = commentMarkdown(comments.get(`6,${index}`));
    if (s.options?.deprecated) out.push('**Deprecated.**', '');
    if (doc) out.push(doc, '');
    const methods = s.method ?? [];
    out.push('| RPC | Request | Response | Kind |', '| --- | --- | --- | --- |');
    for (const m of methods) {
      const anchor = slug(`${s.name}.${m.name}`);
      out.push(
        `| [${codeCell(m.name)}](#${anchor}) | ${this.typeLink(m.inputType, page)} | ${this.typeLink(m.outputType, page)} | ${this.rpcKind(m)} |`
      );
    }
    out.push('');
    methods.forEach((m, i) => {
      out.push(`### ${s.name}.${m.name}`, '');
      out.push(
        `${codeCell(`/${fqService}/${m.name}`)}, ${this.rpcKind(m)}: ${this.typeLink(m.inputType, page)} to ${this.typeLink(m.outputType, page)}.`,
        ''
      );
      if (m.options?.deprecated) out.push('**Deprecated.**', '');
      const mdoc = commentMarkdown(comments.get(`6,${index},2,${i}`));
      if (mdoc) out.push(mdoc, '');
      const fallback = this.facts.fallbacks.find(
        ([stream]) => stream === `/${fqService}/${m.name}`
      );
      if (fallback) {
        const [, unary] = fallback;
        const name = unary.split('/').pop()!;
        out.push(
          `<Callout type="info">gRPC-Web cannot carry client streams. Over gRPC-Web, call the unary [${codeCell(name)}](#${slug(`${s.name}.${name}`)}) instead (${codeCell(unary)}).</Callout>`,
          ''
        );
      }
    });
    return out;
  }

  renderPageBody(spec: PageSpec): { body: string; usesCallout: boolean } {
    const out: string[] = [spec.intro, ''];
    const services: string[] = [];
    const types: string[] = [];
    for (const name of spec.files) {
      const file = this.file(name);
      const comments = commentIndex(file);
      (file.service ?? []).forEach((s, i) =>
        services.push(...this.renderService(s, i, file, comments, spec.slug))
      );
      const fileTypes: string[] = [];
      (file.messageType ?? []).forEach((m, i) =>
        fileTypes.push(
          ...this.renderMessage(m, '', [4, i], comments, spec.slug, file.package ?? '')
        )
      );
      (file.enumType ?? []).forEach((e, i) =>
        fileTypes.push(...this.renderEnum(e, '', [5, i], comments))
      );
      if (fileTypes.length) types.push(`## Types in ${codeCell(name)}`, '', ...fileTypes);
    }
    const sources = spec.files.map((f) => codeCell(`libs/cua/proto/${f}`)).join(', ');
    out.push(`Source: ${sources}.`, '');
    out.push(...services, ...types);
    const body = out.join('\n');
    return { body, usesCallout: body.includes('<Callout') };
  }

  renderIndexBody(header = ''): string {
    const out: string[] = [
      'The gRPC contracts, generated from the `.proto` files in `libs/cua/proto`. `cua.env.v1` is served by cua-spacesd inside a sandbox; `cua.daemon.v1` is served by the local `cua daemon`. Changes are additive and pass `buf breaking`.',
      '',
      `Contract revision: \`cua.env.v1\` protocol version ${this.facts.protocolVersion}, revision ${this.facts.protocolRevision} (reported by \`SystemService.GetCapabilities\`).`,
      '',
      ...(header ? [header, ''] : []),
      '## Services',
      '',
      '| Service | Package | Page | RPCs |',
      '| --- | --- | --- | --- |',
    ];
    for (const spec of this.pages) {
      for (const name of spec.files) {
        const file = this.file(name);
        for (const s of file.service ?? []) {
          out.push(
            `| [${codeCell(s.name)}](${ROUTE}/${spec.slug}#${slug(s.name)}) | ${codeCell(file.package ?? '')} | [${escapeTableCell(spec.title)}](${ROUTE}/${spec.slug}) | ${(s.method ?? []).length} |`
          );
        }
      }
    }
    out.push('', '## Metadata and HTTP routes', '');
    out.push('Well-known metadata keys and HTTP paths (`cua_proto::metadata`).', '');
    out.push('| Constant | Value | Meaning |', '| --- | --- | --- |');
    for (const m of this.facts.metadata) {
      out.push(`| ${codeCell(m.name)} | ${codeCell(m.value)} | ${escapeTableCell(m.doc)} |`);
    }
    out.push('', '## gRPC-Web fallbacks', '');
    out.push(
      'gRPC-Web cannot carry client streams, so every client-streaming RPC has a unary fallback (`cua_proto::CLIENT_STREAM_FALLBACKS`).',
      ''
    );
    out.push('| Client stream | Unary fallback |', '| --- | --- |');
    for (const [stream, unary] of this.facts.fallbacks) {
      out.push(`| ${codeCell(stream)} | ${codeCell(unary)} |`);
    }
    out.push(
      '',
      '## Errors',
      '',
      'Errors are `google.rpc.Status` with typed details: spacesd packs [`ErrorInfo`](' +
        `${ROUTE}/env-common#errorinfo` +
        ') and the daemon packs [`DaemonErrorInfo`](' +
        `${ROUTE}/daemon-service#daemonerrorinfo` +
        ').',
      ''
    );
    return out.join('\n');
  }

  hasType(fq: string): boolean {
    return this.types.has(fq);
  }
}

export function generateProtoReference(
  set: FileDescriptorSet,
  facts: ContractFacts,
  version: string,
  pages: PageSpec[] = PAGES
): Map<string, string> {
  const r = new Renderer(set, pages, facts);
  const generator = GENERATOR;
  const source = 'buf build libs/cua/proto -o -#format=json';
  const files = new Map<string, string>();
  files.set(
    path.join(OUTPUT_DIR, 'index.mdx'),
    renderPage({
      title: 'Protocol',
      description:
        'The gRPC contracts of cua-spacesd (cua.env.v1) and the cua daemon (cua.daemon.v1): ports, routes, auth and every RPC.',
      generator,
      source,
      version,
      body: r.renderIndexBody(readHeader('cua-sdk/protocol/index.md', { version })),
    })
  );
  for (const spec of pages) {
    const { body, usesCallout } = r.renderPageBody(spec);
    files.set(
      path.join(OUTPUT_DIR, `${spec.slug}.mdx`),
      renderPage({
        title: spec.title,
        description: spec.description,
        generator,
        source,
        version,
        components: usesCallout ? ['Callout'] : [],
        body,
      })
    );
  }
  files.set(
    path.join(OUTPUT_DIR, 'meta.json'),
    metaJson('Protocol', pages.map((p) => p.slug))
  );
  return files;
}

function main(): void {
  const checkOnly = isCheckMode();
  console.log('cua gRPC contract reference generator');
  const set = buildDescriptorSet();
  const facts = parseContractFacts(fs.readFileSync(LIB_RS, 'utf-8'));
  const version = fs.readFileSync(VERSION_FILE, 'utf-8').trim();
  const files = generateProtoReference(set, facts, version);
  const big = [...files].filter(
    ([f, c]) => f.endsWith('.mdx') && (wordCount(c) > MAX_WORDS || Buffer.byteLength(c) > MAX_BYTES)
  );
  if (big.length) {
    for (const [f, c] of big) {
      console.error(`${path.basename(f)}: ${wordCount(c)} words, ${Buffer.byteLength(c)} bytes (max ${MAX_WORDS}, ${MAX_BYTES}); split it in PAGES`);
    }
    process.exit(1);
  }
  const drift = syncFiles(files, checkOnly, [OUTPUT_DIR], GENERATOR);
  finish('cua gRPC', drift, checkOnly, GENERATOR);
}

if (require.main === module) {
  main();
}
