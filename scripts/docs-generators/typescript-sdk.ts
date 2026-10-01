#!/usr/bin/env npx tsx

/**
 * TypeScript reference generator: typedoc + typedoc-plugin-markdown over the
 * hand-written layer of `@trycua/cua` (libs/cua/typescript/src). The UniFFI
 * glue under `src/native/` is excluded; the UniFFI reference documents it.
 *
 * typedoc renders Markdown into a temporary directory; a post-pass then turns
 * each module into an MDX page under reference/cua-sdk/typescript/ with
 * frontmatter, the AUTO-GENERATED banner, site routes for cross-page links,
 * and an explicit meta.json.
 *
 * Usage:
 *   pnpm --dir docs docs:generate:cua-sdk-ts
 *   pnpm --dir docs docs:check:cua-sdk-ts
 *
 * Needs `npm ci` in libs/cua/typescript (the package's own type dependencies)
 * and `pnpm --dir docs install` (typedoc and typedoc-plugin-markdown, pinned).
 */

import * as fs from 'fs';
import { createRequire } from 'module';
import * as os from 'os';
import * as path from 'path';
import {
  DOCS_CONTENT,
  REPO_ROOT,
  finish,
  isCheckMode,
  metaJson,
  readHeader,
  renderPage,
  slug,
  syncFiles,
} from './lib/mdx';

const PACKAGE_DIR = path.join(REPO_ROOT, 'libs', 'cua', 'typescript');
const SOURCE_DIR = path.join(PACKAGE_DIR, 'src');
const OUTPUT_DIR = path.join(DOCS_CONTENT, 'cua-sdk', 'reference', 'typescript');
const ROUTE = '/cua-sdk/reference/typescript';

/** One page per public entry point (the package.json `exports`). */
export const MODULES: Array<{
  entry: string;
  /** typedoc-plugin-markdown's file for the module (outputFileStrategy=modules). */
  typedocFile: string;
  page: string;
  title: string;
  importPath: string;
  description: string;
}> = [
  {
    entry: 'index.ts',
    typedocFile: 'index-1.md',
    page: 'index',
    title: 'TypeScript additions',
    importPath: '@trycua/cua',
    description:
      'The hand-written helpers of @trycua/cua: embedded, connect, Image, probes, sidecars, specs and MCP.',
  },
  {
    entry: 'spaces/index.ts',
    typedocFile: 'spaces.md',
    page: 'spaces',
    title: 'Spaces (TypeScript)',
    importPath: '@trycua/cua/spaces',
    description:
      'The @trycua/cua/spaces helpers: threads, events, teleport approvals and typed errors.',
  },
  {
    entry: 'spaces/transport/index.ts',
    typedocFile: 'spaces/transport.md',
    page: 'spaces-transport',
    title: 'Spaces transport (TypeScript)',
    importPath: '@trycua/cua/spaces/transport',
    description: 'MCP-over-HTTP and Tauri transports for Spaces in webviews and browsers.',
  },
  {
    entry: 'spaces/host.ts',
    typedocFile: 'spaces/host.md',
    page: 'spaces-host',
    title: 'Spaces host (TypeScript)',
    importPath: '@trycua/cua/spaces/host',
    description: 'The @trycua/cua/spaces/host helpers for hosting a Space from Node.',
  },
];

/** Options passed to typedoc (and typedoc-plugin-markdown). */
export function typedocOptions(outDir: string, plugin: string): Record<string, unknown> {
  return {
    tsconfig: path.join(PACKAGE_DIR, 'tsconfig.json'),
    entryPoints: MODULES.map((m) => path.join(SOURCE_DIR, m.entry)),
    plugin: [plugin],
    out: outDir,
    readme: 'none',
    outputFileStrategy: 'modules',
    entryFileName: 'index',
    hidePageHeader: true,
    hideBreadcrumbs: true,
    parametersFormat: 'table',
    interfacePropertiesFormat: 'table',
    classPropertiesFormat: 'table',
    typeDeclarationFormat: 'table',
    enumMembersFormat: 'table',
    propertyMembersFormat: 'table',
    typeAliasPropertiesFormat: 'table',
    sanitizeComments: true,
    useCodeBlocks: true,
    excludePrivate: true,
    excludeProtected: true,
    excludeInternal: true,
    excludeExternals: true,
    sort: ['kind', 'alphabetical'],
    logLevel: 'Error',
  };
}

async function runTypedoc(outDir: string): Promise<void> {
  const docsRequire = createRequire(path.join(REPO_ROOT, 'docs', 'package.json'));
  const td = await import(docsRequire.resolve('typedoc'));
  const plugin = docsRequire.resolve('typedoc-plugin-markdown');
  if (!fs.existsSync(path.join(PACKAGE_DIR, 'node_modules'))) {
    throw new Error(
      'libs/cua/typescript/node_modules is missing: run `npm ci --ignore-scripts` there first.'
    );
  }
  const app = await td.Application.bootstrapWithPlugins(typedocOptions(outDir, plugin));
  const isHandWritten = (file: string) =>
    file.startsWith(SOURCE_DIR + path.sep) &&
    !file.startsWith(path.join(SOURCE_DIR, 'native') + path.sep);
  app.converter.on(td.Converter.EVENT_RESOLVE_BEGIN, (context: any) => {
    const project = context.project;
    // Drop the UniFFI glue re-exported from src/native, and members inherited
    // from outside the package (Error.stack and friends).
    for (const reflection of Object.values(project.reflections) as any[]) {
      if (!project.reflections[reflection.id]) continue; // already removed with a parent
      if (!reflection.kindOf(td.ReflectionKind.SomeExport | td.ReflectionKind.SomeMember)) continue;
      const file: string = reflection.sources?.[0]?.fullFileName ?? '';
      if (!file || !isHandWritten(path.resolve(file))) project.removeReflection(reflection);
    }
  });
  app.converter.on(td.Converter.EVENT_RESOLVE_END, (context: any) => {
    // Sources were only needed for the filter; the pages do not show paths.
    for (const reflection of Object.values(context.project.reflections) as any[]) {
      delete reflection.sources;
    }
  });
  const project = await app.convert();
  if (!project) throw new Error('typedoc could not convert @trycua/cua');
  await app.generateOutputs(project);
}

/** typedoc output file (relative, posix) -> site route. */
function routeFor(file: string): string | undefined {
  const m = MODULES.find((mod) => mod.typedocFile === file);
  if (!m) return undefined;
  return m.page === 'index' ? ROUTE : `${ROUTE}/${m.page}`;
}

/**
 * Post-pass over one typedoc Markdown file: drop the H1 (the page title comes
 * from frontmatter), rewrite `.md` links to site routes, and normalise
 * whitespace.
 */
export function postProcess(markdown: string, file: string): string {
  const dir = path.posix.dirname(file);
  let out = markdown.replace(/^# .*\n+/, '');
  out = out.replace(/\]\(([^)\s]+?\.md)(#[^)\s]*)?\)/g, (whole, target: string, hash = '') => {
    const resolved = path.posix.normalize(path.posix.join(dir, target));
    const route = routeFor(resolved);
    return route ? `](${route}${hash})` : whole;
  });
  // typedoc escapes `_` inside words; harmless, but keep identifiers readable.
  out = out.replace(/([A-Za-z0-9])\\_(?=[A-Za-z0-9])/g, '$1_');
  out = dropTableColumn(out, 'Defined in');
  return (
    out
      .replace(/[ \t]+$/gm, '')
      .replace(/\n{3,}/g, '\n\n')
      .trim() + '\n'
  );
}

/** The anchors a page defines: heading slugs (with rehype-slug's `-N` for repeats) and `<a id>`s. */
export function pageAnchors(markdown: string): Set<string> {
  const anchors = new Set<string>();
  const counts = new Map<string, number>();
  let fence = false;
  for (const line of markdown.split('\n')) {
    if (/^\s*```/.test(line)) fence = !fence;
    if (fence) continue;
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    if (heading) {
      const text = heading[1]
        .replace(/`/g, '')
        .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
        .replace(/\\/g, '');
      const base = slug(text);
      const n = counts.get(base) ?? 0;
      counts.set(base, n + 1);
      anchors.add(n === 0 ? base : `${base}-${n}`);
    }
    for (const m of line.matchAll(/<a id="([^"]+)"><\/a>/g)) anchors.add(m[1]);
  }
  return anchors;
}

/**
 * typedoc-plugin-markdown numbers anchors of merged declarations (`#x-1`)
 * differently from rehype-slug; point every fragment link at an anchor the
 * target page really has, or drop the link and keep its text.
 */
export function fixFragments(pages: Map<string, string>): Map<string, string> {
  const anchors = new Map([...pages].map(([route, body]) => [route, pageAnchors(body)]));
  const out = new Map<string, string>();
  const route = ROUTE.replace(/[/]/g, '\\/');
  const link = new RegExp(`\\[([^\\]]*)\\]\\(((?:${route}[^)#\\s]*)?)#([^)\\s]+)\\)`, 'g');
  for (const [route, body] of pages) {
    out.set(
      route,
      body.replace(link, (whole, text: string, target: string, frag: string) => {
        const page = anchors.get(target || route);
        if (!page || page.has(frag)) return whole;
        const base = frag.replace(/-\d+$/, '');
        return page.has(base) ? `[${text}](${target}#${base})` : text;
      })
    );
  }
  return out;
}

/** Splits a Markdown table row into cells, honouring `\|` escapes. */
function splitRow(row: string): string[] {
  const cells: string[] = [];
  let cell = '';
  const inner = row.trim().replace(/^\|/, '').replace(/\|$/, '');
  for (let i = 0; i < inner.length; i += 1) {
    if (inner[i] === '\\' && inner[i + 1] === '|') {
      cell += '\\|';
      i += 1;
    } else if (inner[i] === '|') {
      cells.push(cell);
      cell = '';
    } else {
      cell += inner[i];
    }
  }
  cells.push(cell);
  return cells.map((c) => c.trim());
}

/**
 * Removes the column titled `header` from every Markdown table (sources are
 * stripped, so typedoc-plugin-markdown leaves it empty).
 */
export function dropTableColumn(markdown: string, header: string): string {
  const lines = markdown.split('\n');
  let drop = -1;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (!line.trimStart().startsWith('|')) {
      drop = -1;
      continue;
    }
    const cells = splitRow(line);
    if (drop === -1 && lines[i + 1]?.trimStart().startsWith('| --')) {
      drop = cells.indexOf(header);
      if (drop === -1) {
        drop = -2; // a table without the column: leave it alone
        continue;
      }
    }
    if (drop < 0) continue;
    cells.splice(drop, 1);
    lines[i] = `| ${cells.join(' | ')} |`;
  }
  return lines.join('\n');
}

export function buildPages(typedocDir: string, version: string): Map<string, string> {
  const bodies = new Map<string, string>();
  for (const m of MODULES) {
    const source = path.join(typedocDir, ...m.typedocFile.split('/'));
    if (!fs.existsSync(source)) throw new Error(`typedoc produced no ${m.typedocFile}`);
    bodies.set(
      routeFor(m.typedocFile)!,
      postProcess(fs.readFileSync(source, 'utf-8'), m.typedocFile)
    );
  }
  const fixed = fixFragments(bodies);
  const files = new Map<string, string>();
  for (const m of MODULES) {
    const intro =
      `Import from \`${m.importPath}\`. This page covers the hand-written TypeScript layer; ` +
      'the generated native binding (sandboxes, guest, fleet, spaces objects) is documented per object in the ' +
      '[Cua SDK reference](/cua-sdk/reference).';
    const header = readHeader(`cua-sdk/typescript/${m.page}.md`, { version });
    files.set(
      path.join(OUTPUT_DIR, `${m.page}.mdx`),
      renderPage({
        title: m.title,
        description: m.description,
        generator: 'pnpm --dir docs docs:generate:cua-sdk-ts',
        source: `typedoc + typedoc-plugin-markdown over libs/cua/typescript/src/${m.entry}`,
        version: `@trycua/cua ${version}`,
        body: [intro, header, fixed.get(routeFor(m.typedocFile)!)].filter(Boolean).join('\n\n'),
      })
    );
  }
  files.set(
    path.join(OUTPUT_DIR, 'meta.json'),
    metaJson(
      'TypeScript additions',
      MODULES.map((m) => m.page).filter((p) => p !== 'index')
    )
  );
  return files;
}

async function main(): Promise<void> {
  const checkOnly = isCheckMode();
  const version = JSON.parse(fs.readFileSync(path.join(PACKAGE_DIR, 'package.json'), 'utf-8'))
    .version as string;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'cua-typedoc-'));
  try {
    await runTypedoc(tmp);
    const drift = syncFiles(buildPages(tmp, version), checkOnly, [OUTPUT_DIR], 'pnpm --dir docs docs:generate:cua-sdk-ts');
    finish('TypeScript', drift, checkOnly, 'pnpm --dir docs docs:generate:cua-sdk-ts');
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

if (require.main === module) {
  main().catch((error) => {
    console.error('Error:', error);
    process.exit(1);
  });
}
