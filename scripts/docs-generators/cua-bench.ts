#!/usr/bin/env npx tsx

/**
 * Cua Bench (`cb`) reference generator.
 *
 * Runs scripts/docs-generators/extract_cb_docs.py in the cua-bench project
 * environment (`uv run --frozen --project libs/cua-bench`): argparse
 * introspection of `cb` plus the task and result schemas from the source
 * (`#:` field comments). Writes, through the shared renderers in lib/:
 *
 * - docs/content/docs/cua-bench/reference/index.mdx (the Reference index)
 * - docs/content/docs/cua-bench/reference/cli/ (one page per command group)
 * - docs/content/docs/cua-bench/reference/task-definition.mdx
 * - docs/content/docs/cua-bench/reference/results.mdx
 * - scripts/docs-generators/cli-specs/cb.json (the CLI-shape lane's oracle)
 *
 * Usage:
 *   pnpm --dir docs docs:generate:cua-bench
 *   pnpm --dir docs docs:check:cua-bench        # drift check (CI)
 *
 * CB_DOCS_JSON=<path> reads a saved extractor dump instead of running it.
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
  sentence,
} from './lib/cli-mdx';
import { readHeader } from './lib/headers';
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
  stableJson,
  syncFiles,
} from './lib/mdx';

const BENCH_DIR = path.join(REPO_ROOT, 'libs', 'cua-bench');
const OUT_DIR = path.join(DOCS_CONTENT, 'cua-bench', 'reference');
export const CB_CLI_SPEC = path.join(__dirname, 'cli-specs', 'cb.json');
const EXTRACTOR = path.join(__dirname, 'extract_cb_docs.py');
const REGENERATE = 'pnpm --dir docs docs:generate:cua-bench';
const SOURCE = 'scripts/docs-generators/extract_cb_docs.py (cua_bench)';
/** The bundled example the task-definition page shows; CI runs it (test_cli_golden). */
const EXAMPLE_TASK = 'libs/cua-bench/example_tasks/hello_file_env/main.py';

export interface FieldDoc {
  name: string;
  type: string;
  default: string | null;
  description: string;
}

export interface ClassDoc {
  name: string;
  doc: string;
  fields: FieldDoc[];
}

export interface CbDocs {
  cli: CLIDocumentation;
  task: {
    task: ClassDoc;
    setup_config: ClassDoc;
    decorators: Array<{ name: string; summary: string; doc: string }>;
    session: {
      doc: string;
      methods: Array<{ name: string; kind: string; is_async: boolean; signature: string; returns: string; summary: string }>;
    };
    actions: ClassDoc[];
  };
  results: {
    job_result: ClassDoc;
    harbor: ClassDoc;
    span: ClassDoc;
    summary: ClassDoc;
    summary_result: ClassDoc;
    summary_stats: ClassDoc;
    summary_target: ClassDoc;
    golden_result: Record<string, unknown>;
    golden_summary: Record<string, unknown>;
  };
}

// ---------------------------------------------------------------- CLI

export const CLI_GROUPS: CliGroup[] = [
  {
    slug: 'run',
    title: 'cb run',
    summary: 'Run a task or dataset with an agent or the oracle, run one interactively, and follow runs',
    commands: ['run', 'interact'],
  },
  { slug: 'task', title: 'cb task', summary: 'Inspect, list, scaffold and generate tasks', commands: ['task'] },
  { slug: 'dataset', title: 'cb dataset', summary: 'List registry datasets and build training data from runs', commands: ['dataset'] },
  { slug: 'trace', title: 'cb trace', summary: 'View traces and agent trajectories', commands: ['trace'] },
  {
    slug: 'env',
    title: 'Sandboxes and account',
    summary: 'Cloud pools and local sandboxes, sign-in, status and cleanup',
    commands: ['env', 'login', 'status', 'prune'],
  },
  { slug: 'agent', title: 'cb agent', summary: 'Scaffold custom agents', commands: ['agent'] },
  {
    slug: 'images',
    title: 'Images and platforms',
    summary: 'The canonical images and platform configurations',
    commands: ['image', 'platform'],
  },
];

export function cliReference(cli: CLIDocumentation): CliReference {
  return {
    product: 'cua-bench',
    cli,
    groups: CLI_GROUPS,
    generator: REGENERATE,
    source: SOURCE,
    intro: readHeader('cua-bench', 'cli'),
  };
}

// ---------------------------------------------------------------- tables

/** reST inline markup in docstrings and `#:` comments to Markdown. */
export function rst(text: string): string {
  return text
    .replace(/:(?:func|meth|class|attr|mod|data):`~?([^`]+)`/g, '`$1`')
    .replace(/``([^`]+)``/g, '`$1`')
    .replace(/\s*\n\s*/g, ' ')
    .trim();
}

function fieldsTable(fields: FieldDoc[], opts: { defaults?: boolean; prefix?: string; fallback?: Map<string, string> } = {}): string[] {
  const head = opts.defaults ? '| Field | Type | Default | Description |' : '| Key | Type | Description |';
  const sep = opts.defaults ? '| --- | --- | --- | --- |' : '| --- | --- | --- |';
  const lines = [head, sep];
  for (const f of fields) {
    const description = f.description || opts.fallback?.get(f.name) || '';
    if (!description) throw new Error(`cua-bench: ${opts.prefix ?? ''}${f.name} has no #: description in the source`);
    const name = codeCell(`${opts.prefix ?? ''}${f.name}`);
    const cells = [name, codeCell(f.type)];
    if (opts.defaults) cells.push(f.default == null ? 'required' : codeCell(f.default));
    cells.push(escapeTableCell(sentence(rst(description))));
    lines.push(`| ${cells.join(' | ')} |`);
  }
  return [...lines, ''];
}

function checkKeys(where: string, documented: string[], golden: Record<string, unknown>): void {
  const want = Object.keys(golden).sort();
  const have = [...new Set(documented)].sort();
  const missing = want.filter((k) => !have.includes(k));
  const extra = have.filter((k) => !want.includes(k));
  if (missing.length || extra.length) {
    throw new Error(
      `cua-bench ${where}: documented keys differ from the export golden. ` +
        `Undocumented: ${missing.join(', ') || 'none'}; not in the golden: ${extra.join(', ') || 'none'}`
    );
  }
}

// ---------------------------------------------------------------- task definition

export function taskDefinitionPage(docs: CbDocs, example: string, version: string): string {
  const { task } = docs;
  const body: string[] = [];
  const header = readHeader('cua-bench', 'task-definition');
  if (header) body.push(header, '');

  body.push('## `cb.Task`', '', escapeMdxText(sentence(rst(task.task.doc))), '');
  body.push(...fieldsTable(task.task.fields, { defaults: true }));

  body.push('## `setup_config`', '', escapeMdxText(rst(task.setup_config.doc)), '');
  body.push(...fieldsTable(task.setup_config.fields));

  body.push('## Lifecycle decorators', '');
  body.push('| Decorator | Description |', '| --- | --- |');
  for (const d of task.decorators) body.push(`| ${codeCell(`@cb.${d.name}`)} | ${escapeTableCell(sentence(rst(d.doc)))} |`);
  body.push('');

  body.push('## Session', '', escapeMdxText(sentence(rst(task.session.doc))), '');
  body.push('| Method | Returns | Description |', '| --- | --- | --- |');
  for (const m of task.session.methods) {
    if (!m.summary) throw new Error(`cua-bench: session.${m.name} has no docstring`);
    const sig = m.kind === 'property' ? m.name : `${m.is_async ? 'await ' : ''}${m.signature}`;
    body.push(`| <span id="session-${m.name.replace(/_/g, '-')}"></span>${codeCell(sig)} | ${codeCell(m.returns)} | ${escapeTableCell(sentence(rst(m.summary)))} |`);
  }
  body.push('');

  body.push('## Actions', '', 'Input actions for `session.execute_action` (from `cua_bench`).', '');
  body.push('| Action | Fields |', '| --- | --- |');
  for (const a of task.actions) {
    const fields = a.fields.map((f) => codeCell(`${f.name}: ${f.type}${f.default != null ? ` = ${f.default}` : ''}`)).join(', ');
    body.push(`| ${codeCell(a.name)} | ${fields || 'none'} |`);
  }
  body.push('');

  body.push('## Example', '');
  body.push(`{/* Source: ${EXAMPLE_TASK} (run by libs/cua-bench/cua_bench/tests/test_cli_golden.py) */}`, '');
  body.push(codeFence('python', example.trim()), '');

  return renderPage({
    title: 'Task definition',
    description: 'The task module contract: cb.Task, setup_config, the lifecycle decorators, the session API and actions.',
    generator: REGENERATE,
    source: SOURCE,
    version,
    body: body.join('\n'),
  });
}

// ---------------------------------------------------------------- results

export function resultsPage(docs: CbDocs, version: string): string {
  const r = docs.results;
  const jobDocs = new Map(r.job_result.fields.map((f) => [f.name, f.description]));
  const summaryDocs = new Map(r.summary.fields.map((f) => [f.name, f.description]));
  const schemaVersion: FieldDoc = {
    name: 'schema_version',
    type: 'int',
    default: null,
    description: summaryDocs.get('schema_version') ?? '',
  };
  const resultFields = [schemaVersion, ...r.job_result.fields, ...r.harbor.fields];
  checkKeys('result.json', resultFields.map((f) => f.name), r.golden_result);
  checkKeys('summary.json', r.summary.fields.map((f) => f.name), r.golden_summary);
  const firstRow = (key: string) => {
    const v = r.golden_summary[key];
    return (Array.isArray(v) ? v[0] : v) as Record<string, unknown>;
  };
  checkKeys('summary.json results[]', r.summary_result.fields.map((f) => f.name), firstRow('results'));
  checkKeys('summary.json stats', r.summary_stats.fields.map((f) => f.name), firstRow('stats'));
  checkKeys('summary.json targets[]', r.summary_target.fields.map((f) => f.name), firstRow('targets'));

  const body: string[] = [];
  const header = readHeader('cua-bench', 'results');
  if (header) body.push(header, '');
  body.push('## `result.json`', '', escapeMdxText(sentence(rst(r.job_result.doc))), '');
  body.push(...fieldsTable(resultFields, { fallback: jobDocs }));
  body.push(
    `Phase spans (\`environment_setup\`, \`agent_execution\`, \`verifier\`) are ${r.span.fields.map((f) => codeCell(f.name)).join(' and ')} timestamps (ISO 8601, UTC).`,
    ''
  );
  body.push('## `summary.json`', '', escapeMdxText(sentence(rst(r.summary.doc))), '');
  body.push(...fieldsTable(r.summary.fields));
  body.push('### `results[]`', '', escapeMdxText(sentence(rst(r.summary_result.doc))), '');
  body.push(...fieldsTable(r.summary_result.fields, { prefix: 'results[].', fallback: jobDocs }));
  body.push('### `stats`', '');
  body.push(...fieldsTable(r.summary_stats.fields, { prefix: 'stats.' }));
  body.push('### `targets[]`', '', escapeMdxText(sentence(rst(r.summary_target.doc))), '');
  body.push(...fieldsTable(r.summary_target.fields, { prefix: 'targets[].', fallback: jobDocs }));
  return renderPage({
    title: 'Results',
    description: 'The result.json and summary.json a run writes: every key, its type and meaning.',
    generator: REGENERATE,
    source: SOURCE,
    version,
    body: body.join('\n'),
  });
}

// ---------------------------------------------------------------- main

export function renderAll(docs: CbDocs, example: string): Map<string, string> {
  const cli = cliReference(docs.cli);
  const version = docs.cli.version;
  const rel = new Map<string, string>(renderCliReference(cli));
  rel.set('task-definition.mdx', taskDefinitionPage(docs, example, version));
  rel.set('results.mdx', resultsPage(docs, version));
  rel.set(
    'index.mdx',
    renderReferenceIndex({
      productName: 'Cua Bench',
      description: 'Every cb command, the task module contract and the result files, generated from cua-bench.',
      generator: REGENERATE,
      source: SOURCE,
      version,
      sections: [
        { title: 'Commands', column: 'Command group', rows: cliIndexRows(cli) },
        {
          title: 'Schemas',
          column: 'Schema',
          rows: [
            { name: 'Task definition', href: '/cua-bench/reference/task-definition', description: 'cb.Task, setup_config, the lifecycle decorators, the session API and actions.' },
            { name: 'Results', href: '/cua-bench/reference/results', description: 'Every key of result.json and summary.json.' },
          ],
        },
      ],
    })
  );
  rel.set('meta.json', metaJson('Reference', ['index', 'cli', 'task-definition', 'results']));
  const files = new Map<string, string>();
  for (const [p, content] of rel) files.set(path.join(OUT_DIR, p), content);
  files.set(CB_CLI_SPEC, stableJson(docs.cli));
  return files;
}

export function extract(): CbDocs {
  if (process.env.CB_DOCS_JSON) return JSON.parse(fs.readFileSync(process.env.CB_DOCS_JSON, 'utf-8'));
  const out = execFileSync(
    process.env.UV || 'uv',
    ['run', '--frozen', '--quiet', '--python', '3.12', '--project', BENCH_DIR, 'python', EXTRACTOR],
    {
      cwd: REPO_ROOT,
      encoding: 'utf-8',
      maxBuffer: 64 * 1024 * 1024,
      env: { ...process.env, CUA_BENCH_NO_BANNER: '1', CUA_TELEMETRY_ENABLED: 'false' },
    }
  );
  return JSON.parse(out);
}

function main(): void {
  const checkOnly = isCheckMode();
  const docs = extract();
  const example = fs.readFileSync(path.join(REPO_ROOT, EXAMPLE_TASK), 'utf-8');
  const owned = [OUT_DIR, path.join(OUT_DIR, 'cli')];
  finish('Cua Bench', syncFiles(renderAll(docs, example), checkOnly, owned), checkOnly, REGENERATE);
}

if (require.main === module) main();
