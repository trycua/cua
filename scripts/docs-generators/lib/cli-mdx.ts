/**
 * The one CLI reference renderer shared by `cua`, `cua-driver`, `lume` and
 * `cb`, and the `dump-docs --type cli` JSON schema they all emit.
 *
 * Layout (every CLI renders the same way):
 *
 * - `reference/cli/index.mdx`: the base command (synopsis, global options,
 *   exit codes) and an index of every command, grouped. Old per-command
 *   anchors of the single-page reference resolve here (hidden anchors that
 *   link on to the command's section).
 * - `reference/cli/<group>.mdx`: one page per command group. Each command has
 *   a heading anchored by its full path (`#cua-sandbox-create`), a synopsis,
 *   an arguments table and a flags table (`Flag | Short | Type | Default |
 *   Env var | Description`, one anchor per flag: `#cua-sandbox-create--name`),
 *   and examples. Examples come from the CLI source (help text) and are tagged
 *   `test="cli-shape"`, so the docs CLI-shape lane parses each one against the
 *   same JSON. Exit codes close the page.
 *
 * cua-driver and lume emit the base fields; `cua dump-docs` (clap) adds usage,
 * aliases, hidden commands, global options, env vars and repeatability; `cb`
 * (argparse) adds usage and choices. The docs CLI-shape lane reads the same
 * JSON (`scripts/docs-generators/cli-specs/<cli>.json`).
 */

import {
  codeCell,
  codeFence,
  escapeMdxText,
  escapeTableCell,
  metaJson,
  renderPage,
  slug,
} from './mdx';
import { readHeader } from './headers';

export interface ArgumentDoc {
  name: string;
  help: string;
  type: string;
  is_optional: boolean;
  repeatable?: boolean;
  hidden?: boolean;
  default_value?: string | null;
  possible_values?: string[];
}

export interface OptionDoc {
  name: string;
  short_name?: string | null;
  aliases?: string[];
  help: string;
  type: string;
  value_name?: string | null;
  possible_values?: string[];
  default_value?: string | null;
  is_optional: boolean;
  repeatable?: boolean;
  is_repeatable?: boolean;
  /** One occurrence takes several values (`--tags a b`). */
  multiple_values?: boolean;
  env?: string | null;
  hidden?: boolean;
  takes_value?: boolean;
}

export interface FlagDoc {
  name: string;
  short_name?: string | null;
  aliases?: string[];
  help: string;
  default_value: boolean;
  repeatable?: boolean;
  env?: string | null;
  hidden?: boolean;
  takes_value?: boolean;
}

/** One example invocation, with an optional one-line description. */
export interface ExampleDoc {
  command: string;
  description?: string;
}

export interface ExitCodeDoc {
  code: number;
  meaning: string;
}

export interface CommandDoc {
  name: string;
  abstract: string;
  discussion?: string;
  usage?: string;
  aliases?: string[];
  hidden_aliases?: string[];
  hidden?: boolean;
  arguments: ArgumentDoc[];
  options: OptionDoc[];
  flags: FlagDoc[];
  subcommands: CommandDoc[];
  /** The subcommand that runs when none is given. */
  default_subcommand?: string;
  /** The group also runs without a subcommand. */
  subcommand_optional?: boolean;
  /** Structured examples; an `Examples:` section in `discussion` or `after_help` also works. */
  examples?: ExampleDoc[];
  after_help?: string;
  /** How this command's exit status differs from the CLI's exit codes. */
  exit_status?: string;
}

export interface CLIDocumentation {
  name: string;
  version: string;
  abstract: string;
  usage?: string;
  global_options?: Array<OptionDoc | FlagDoc>;
  exit_codes?: ExitCodeDoc[];
  commands: CommandDoc[];
}

/** A page of the CLI reference: one or more top-level commands. */
export interface CliGroup {
  /** Page slug under `reference/cli/`. */
  slug: string;
  /** Page title: the command (`cua sandbox`) or a topic (`VMs`). */
  title: string;
  /** One line for the index and the page description. */
  summary: string;
  /** Top-level command names on this page, in order. */
  commands: string[];
  /**
   * Split a large command over several pages: this page documents only these
   * subcommands of its one listed command. Every page that lists the command
   * must name its share, the shares must cover every visible subcommand
   * exactly once, and the first such page also documents the command itself.
   */
  subcommands?: string[];
}

/** A command on a group page, under its parent path. */
interface PageCommand {
  cmd: CommandDoc;
  parents: string[];
}

export interface CliReference {
  /** Docs product folder (`cua-cli`); pages go to `<product>/reference/cli/`. */
  product: string;
  cli: CLIDocumentation;
  groups: CliGroup[];
  /** Command that regenerates the pages (for the banner). */
  generator: string;
  /** What the pages are generated from (for the banner). */
  source: string;
  /** Top-level commands deliberately left out (internal tooling). */
  omit?: string[];
  /** Curated Markdown shown at the top of the CLI index. */
  intro?: string;
  /** Curated Markdown per group slug, shown under the page's command table. */
  headers?: Record<string, string>;
}

/** clap strips the final period from doc comments; put it back for prose. */
export function sentence(text: string): string {
  const t = (text ?? '').trim();
  return !t || /[.!?:]$/.test(t) ? t : `${t}.`;
}

/** The first sentence of `text` (for one-line summaries). */
export function firstSentence(text: string): string {
  const t = (text ?? '').replace(/\s+/g, ' ').trim();
  const m = t.match(/^(.+?[.!?])(\s|$)/);
  return sentence(m ? m[1] : t);
}

// ---------------------------------------------------------------- examples

/**
 * Splits an `Examples:` section off help text. Each example is a command
 * line, optionally preceded by `# description` lines or followed by
 * `  # description`; a leading `$ ` is dropped.
 */
export function splitExamples(text: string | undefined): { prose: string; examples: ExampleDoc[] } {
  if (!text) return { prose: '', examples: [] };
  const lines = text.split('\n');
  const at = lines.findIndex((l) => /^\s*Examples?:\s*$/.test(l));
  if (at < 0) return { prose: text.trim(), examples: [] };
  const examples: ExampleDoc[] = [];
  let pending: string[] = [];
  let end = lines.length;
  for (let i = at + 1; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.trim();
    if (!line) continue;
    // A new unindented section ends the examples.
    if (!/^\s/.test(raw) && /:$/.test(line) && !line.startsWith('#')) {
      end = i;
      break;
    }
    if (line.startsWith('#')) {
      pending.push(line.replace(/^#+\s*/, ''));
      continue;
    }
    let command = line.replace(/^\$\s+/, '');
    let description = pending.join(' ');
    const trailing = command.match(/^(.*?\S)\s{2,}#\s*(.+)$/);
    if (trailing) {
      command = trailing[1];
      description = [description, trailing[2]].filter(Boolean).join(' ');
    }
    examples.push(description ? { command, description } : { command });
    pending = [];
  }
  const prose = [...lines.slice(0, at), ...lines.slice(end)].join('\n').trim();
  return { prose, examples };
}

function commandExamples(cmd: CommandDoc): { prose: string; examples: ExampleDoc[] } {
  const fromDiscussion = splitExamples(cmd.discussion);
  const fromAfter = splitExamples(cmd.after_help);
  return {
    prose: [fromDiscussion.prose, fromAfter.prose].filter(Boolean).join('\n\n'),
    examples: [...(cmd.examples ?? []), ...fromDiscussion.examples, ...fromAfter.examples],
  };
}

// ---------------------------------------------------------------- types

const TYPE_NAMES: Record<string, string> = {
  string: 'string',
  str: 'string',
  osstring: 'string',
  int: 'integer',
  uint: 'integer',
  int32: 'integer',
  int64: 'integer',
  integer: 'integer',
  u8: 'integer',
  u16: 'integer',
  u32: 'integer',
  u64: 'integer',
  usize: 'integer',
  i32: 'integer',
  i64: 'integer',
  double: 'number',
  float: 'number',
  number: 'number',
  f32: 'number',
  f64: 'number',
  bool: 'boolean',
  boolean: 'boolean',
  path: 'path',
  pathbuf: 'path',
  url: 'URL',
};

/** A reader-facing type: `string`, `integer`, `path`, or the allowed values. */
export function displayType(value: {
  type?: string;
  possible_values?: string[];
  value_name?: string | null;
}): string {
  if (value.possible_values?.length) return value.possible_values.map((v) => codeCell(v)).join(' \\| ');
  const raw = (value.type ?? '').trim();
  if (raw.includes(' | ')) {
    return raw
      .split(' | ')
      .map((v) => codeCell(v.trim()))
      .join(' \\| ');
  }
  const inner = raw.replace(/^\[(.*)\]$/, '$1');
  const known = TYPE_NAMES[inner.toLowerCase()];
  if (known) return known;
  // An upper-case clap value name (`NAME`, `ON`) says nothing about the type.
  if (!inner || /^[A-Z0-9_]+$/.test(inner)) return 'string';
  return escapeTableCell(inner);
}

function isRepeatable(o: OptionDoc | FlagDoc | ArgumentDoc): boolean {
  return Boolean(
    o.repeatable || (o as OptionDoc).is_repeatable || /^\[.*\]$/.test((o as OptionDoc).type ?? '')
  );
}

function takesValue(o: OptionDoc | FlagDoc): o is OptionDoc {
  return 'type' in o && o.takes_value !== false;
}

// ---------------------------------------------------------------- tables

/** The anchor of a command path: `cua sandbox create` -> `cua-sandbox-create`. */
export function commandAnchor(path: string[]): string {
  return slug(path.join(' '));
}

/** The anchor of a flag: `cua-sandbox-create--name`. */
export function flagAnchor(path: string[], flag: string): string {
  return `${commandAnchor(path)}--${slug(flag)}`;
}

function anchorSpan(id: string): string {
  return `<span id="${id}"></span>`;
}

function flagDescription(opt: OptionDoc | FlagDoc): string {
  const extra: string[] = [];
  const aliases = (opt.aliases ?? []).filter((a) => !a.startsWith('-'));
  if (aliases.length) extra.push(`Alias: ${aliases.map((a) => `\`--${a}\``).join(', ')}.`);
  if ((opt as OptionDoc).multiple_values) extra.push('Takes one or more values.');
  else if (isRepeatable(opt) && !/\brepeatable\b/i.test(opt.help || '')) extra.push('Repeatable.');
  return [escapeTableCell(sentence(opt.help || '')), ...extra].filter(Boolean).join(' ');
}

/**
 * `Flag | Short | Type | Default | Env var | Description`; the Short and
 * Env var columns are dropped when no row uses them. `path` (the command
 * path) gives every flag its own anchor; omit it for tables that are not
 * under a command (global options).
 */
export function flagsTable(options: Array<OptionDoc | FlagDoc>, path?: string[]): string[] {
  const visible = options.filter((o) => !o.hidden && o.name);
  if (!visible.length) return [];
  const withShort = visible.some((o) => o.short_name);
  const withEnv = visible.some((o) => o.env);
  const header = ['Flag', ...(withShort ? ['Short'] : []), 'Type', 'Default', ...(withEnv ? ['Env var'] : []), 'Description'];
  const lines = [`| ${header.join(' | ')} |`, `| ${header.map(() => '---').join(' | ')} |`];
  for (const opt of visible) {
    const flag = `${path ? anchorSpan(flagAnchor(path, opt.name)) : ''}${codeCell(`--${opt.name}`)}`;
    let type: string;
    let def: string;
    if (takesValue(opt)) {
      type = displayType(opt);
      def = !opt.is_optional && opt.default_value == null
        ? 'required'
        : opt.default_value != null && opt.default_value !== ''
          ? codeCell(String(opt.default_value))
          : '';
    } else {
      type = 'boolean';
      def = codeCell('false');
    }
    const row = [
      flag,
      ...(withShort ? [opt.short_name ? codeCell(`-${opt.short_name}`) : ''] : []),
      type,
      def,
      ...(withEnv ? [opt.env ? codeCell(opt.env) : ''] : []),
      flagDescription(opt),
    ];
    lines.push(`| ${row.join(' | ')} |`);
  }
  return [...lines, ''];
}

/** `Argument | Type | Default | Description`; required arguments show `required`. */
export function argumentsTable(args: ArgumentDoc[]): string[] {
  const visible = args.filter((a) => !a.hidden);
  if (!visible.length) return [];
  const lines = ['| Argument | Type | Default | Description |', '| --- | --- | --- | --- |'];
  for (const arg of visible) {
    const name = `<${arg.name}>${isRepeatable(arg) ? '...' : ''}`;
    const def = !arg.is_optional
      ? 'required'
      : arg.default_value
        ? codeCell(arg.default_value)
        : 'optional';
    const extra = isRepeatable(arg) ? ' Repeatable.' : '';
    lines.push(
      `| ${codeCell(name)} | ${displayType(arg)} | ${def} | ${escapeTableCell(sentence(arg.help || ''))}${extra} |`
    );
  }
  return [...lines, ''];
}

/** A synthesized synopsis for CLIs that do not report clap-style usage. */
export function synopsis(cmd: CommandDoc, path: string[]): string {
  if (cmd.usage) {
    const usage = cmd.usage.replace(/^Usage:\s*/, '').trim();
    // clap renders the full path already; argparse renders `prog` paths too.
    return usage;
  }
  const parts = [...path];
  const visibleOpts = [...cmd.options, ...cmd.flags].filter((o) => !o.hidden);
  if (visibleOpts.length) parts.push('[OPTIONS]');
  for (const arg of cmd.arguments.filter((a) => !a.hidden)) {
    const name = `<${arg.name}>${isRepeatable(arg) ? '...' : ''}`;
    parts.push(arg.is_optional ? `[${name}]` : name);
  }
  if (cmd.subcommands.some((s) => !s.hidden)) {
    parts.push(cmd.default_subcommand || cmd.subcommand_optional ? '[COMMAND]' : '<COMMAND>');
  }
  return parts.join(' ');
}

// ---------------------------------------------------------------- commands

function visibleSubs(cmd: CommandDoc): CommandDoc[] {
  return cmd.subcommands.filter((s) => !s.hidden);
}

/** Every visible command under (and including) `cmd`, with its path. */
function flatten(cmd: CommandDoc, parents: string[]): Array<{ cmd: CommandDoc; path: string[] }> {
  if (cmd.hidden) return [];
  const path = [...parents, cmd.name];
  return [{ cmd, path }, ...visibleSubs(cmd).flatMap((s) => flatten(s, path))];
}

function examplesBlock(examples: ExampleDoc[], anchor: string): string[] {
  if (!examples.length) return [];
  const lines: string[] = [];
  examples.forEach((ex, i) => {
    if (ex.description) lines.push(`# ${ex.description.replace(/\n/g, ' ')}`);
    lines.push(ex.command);
    if (i < examples.length - 1 && examples[i + 1].description) lines.push('');
  });
  return ['**Examples**', '', codeFence('bash', lines.join('\n'), `test="cli-shape" id="ref-${anchor}"`), ''];
}

interface RenderContext {
  cli: CLIDocumentation;
  /** Page URL of the CLI index, for the global options link. */
  indexUrl: string;
}

/**
 * One command's section. `level` is its heading level, or 0 for the page's
 * own command (no heading: the page title names it).
 */
export function renderCommand(
  cmd: CommandDoc,
  parents: string[],
  level: number,
  ctx?: RenderContext
): string[] {
  if (cmd.hidden) return [];
  const path = [...parents, cmd.name];
  const anchor = commandAnchor(path);
  const lines: string[] = [];
  if (level > 0) lines.push(`${'#'.repeat(Math.min(level, 6))} \`${path.join(' ')}\``, '');
  const { prose, examples } = commandExamples(cmd);
  if (cmd.abstract) lines.push(escapeMdxText(sentence(cmd.abstract)), '');
  if (prose) lines.push(escapeMdxText(prose), '');
  lines.push(codeFence('text', synopsis(cmd, path), 'output'), '');
  const aliases = cmd.aliases ?? [];
  if (aliases.length) {
    lines.push(`Alias: ${aliases.map((a) => `\`${[...parents, a].join(' ')}\``).join(', ')}.`, '');
  }
  if (cmd.default_subcommand) {
    lines.push(`Without a subcommand, runs \`${[...path, cmd.default_subcommand].join(' ')}\`.`, '');
  }
  // A deprecated alias keeps its synopsis and examples; its options are
  // those of the command it aliases, so the tables are not repeated.
  if (!/^deprecated\b/i.test(cmd.abstract ?? '')) {
    lines.push(...argumentsTable(cmd.arguments));
    lines.push(...flagsTable([...cmd.options, ...cmd.flags], path));
  }
  lines.push(...examplesBlock(examples, anchor));
  if (cmd.exit_status) lines.push(`Exit status: ${escapeMdxText(sentence(cmd.exit_status))}`, '');
  const next = level === 0 ? 2 : level + 1;
  for (const sub of visibleSubs(cmd)) lines.push(...renderCommand(sub, path, next, ctx));
  return lines;
}

/** `Code | Meaning`. */
export function exitCodesTable(codes: ExitCodeDoc[]): string[] {
  if (!codes.length) return [];
  const lines = ['| Code | Meaning |', '| --- | --- |'];
  for (const c of codes) lines.push(`| \`${c.code}\` | ${escapeTableCell(sentence(c.meaning))} |`);
  return [...lines, ''];
}

// ---------------------------------------------------------------- pages

function groupCommands(ref: CliReference): Map<string, PageCommand[]> {
  const byName = new Map(ref.cli.commands.map((c) => [c.name, c]));
  const omit = new Set(ref.omit ?? []);
  const root = [ref.cli.name];
  // A command split over pages (`subcommands`) is listed by each of them.
  const split = new Map<string, CliGroup[]>();
  for (const g of ref.groups) {
    if (!g.subcommands) continue;
    if (g.commands.length !== 1) throw new Error(`${ref.cli.name} CLI group ${g.slug}: subcommands need exactly one command`);
    split.set(g.commands[0], [...(split.get(g.commands[0]) ?? []), g]);
  }
  const assigned = ref.groups.filter((g) => !g.subcommands).flatMap((g) => g.commands);
  assigned.push(...split.keys());
  const dupes = assigned.filter((n, i) => assigned.indexOf(n) !== i);
  const visible = ref.cli.commands.filter((c) => !c.hidden && !omit.has(c.name)).map((c) => c.name);
  const missing = visible.filter((n) => !assigned.includes(n));
  const unknown = assigned.filter((n) => !byName.has(n) || byName.get(n)!.hidden);
  if (dupes.length || missing.length || unknown.length) {
    throw new Error(
      `${ref.cli.name} CLI groups do not match the command tree. ` +
        `Ungrouped: ${missing.join(', ') || 'none'}; duplicated: ${dupes.join(', ') || 'none'}; ` +
        `unknown or hidden: ${unknown.join(', ') || 'none'}`
    );
  }
  for (const [name, groups] of split) {
    const subs = visibleSubs(byName.get(name)!).map((c) => c.name);
    const shares = groups.flatMap((g) => g.subcommands!);
    const twice = shares.filter((n, i) => shares.indexOf(n) !== i);
    const left = subs.filter((n) => !shares.includes(n));
    const stray = shares.filter((n) => !subs.includes(n));
    if (twice.length || left.length || stray.length) {
      throw new Error(
        `${ref.cli.name} ${name} is split over ${groups.map((g) => g.slug).join(', ')} unevenly. ` +
          `Unplaced: ${left.join(', ') || 'none'}; duplicated: ${twice.join(', ') || 'none'}; unknown: ${stray.join(', ') || 'none'}`
      );
    }
  }
  const pages = new Map<string, PageCommand[]>();
  for (const g of ref.groups) {
    if (!g.subcommands) {
      pages.set(g.slug, g.commands.map((n) => ({ cmd: byName.get(n)!, parents: root })));
      continue;
    }
    const cmd = byName.get(g.commands[0])!;
    const share = new Set(g.subcommands);
    if (split.get(cmd.name)![0] === g) {
      // The first page: the command itself, with this page's subcommands.
      const own = { ...cmd, subcommands: cmd.subcommands.filter((s) => s.hidden || share.has(s.name)) };
      pages.set(g.slug, [{ cmd: own, parents: root }]);
    } else {
      pages.set(
        g.slug,
        visibleSubs(cmd)
          .filter((s) => share.has(s.name))
          .map((s) => ({ cmd: s, parents: [...root, cmd.name] }))
      );
    }
  }
  return pages;
}

function isCommandTitle(group: CliGroup, root: string): boolean {
  return group.title.startsWith(`${root} `) || group.title === root;
}

function groupTitle(group: CliGroup, root: string): string {
  return isCommandTitle(group, root) ? `\`${group.title}\`` : group.title;
}

function cliUrl(ref: CliReference, page?: string): string {
  return `/${ref.product}/reference/cli${page ? `/${page}` : ''}`;
}

function commandRows(
  entries: Array<{ cmd: CommandDoc; path: string[] }>,
  href: (anchor: string) => string,
  used?: Set<string>
): string[] {
  const lines = ['| Command | Description |', '| --- | --- |'];
  for (const { cmd, path } of entries) {
    const anchor = commandAnchor(path);
    let cell = `[\`${path.join(' ')}\`](${href(anchor)})`;
    if (used && !used.has(anchor)) {
      used.add(anchor);
      cell = `${anchorSpan(anchor)}${cell}`;
    }
    lines.push(`| ${cell} | ${escapeTableCell(firstSentence(cmd.abstract))} |`);
  }
  return [...lines, ''];
}

/**
 * Words a generated reference page may hold (hidden anchors excluded); the
 * reference-quality test caps every page at 5000. Past this budget the index
 * switches to its compact form.
 */
export const INDEX_WORD_BUDGET = 4500;

export function pageWords(page: string): number {
  return page.replace(/<span id="[^"]*"><\/span>/g, '').split(/\s+/).filter(Boolean).length;
}

/**
 * The compact index table: one row per page command (described), its nested
 * commands linked by their last word. Every nested command keeps its hidden
 * anchor (on its page command's row), so old anchors still resolve here.
 */
function compactCommandRows(
  roots: PageCommand[],
  href: (anchor: string) => string,
  used: Set<string>
): string[] {
  const lines = ['| Command | Description | Subcommands |', '| --- | --- | --- |'];
  for (const { cmd, parents } of roots) {
    const [root, ...nested] = flatten(cmd, parents);
    if (!root) continue;
    const spans: string[] = [];
    for (const { path } of [root, ...nested]) {
      const anchor = commandAnchor(path);
      if (!used.has(anchor)) {
        used.add(anchor);
        spans.push(anchorSpan(anchor));
      }
    }
    const rootAnchor = commandAnchor(root.path);
    const cell = `${spans.join('')}[\`${root.path.join(' ')}\`](${href(rootAnchor)})`;
    const subs = nested
      .map(({ path }) => `[\`${path.slice(root.path.length).join(' ')}\`](${href(commandAnchor(path))})`)
      .join(', ');
    lines.push(`| ${cell} | ${escapeTableCell(firstSentence(root.cmd.abstract))} | ${subs} |`);
  }
  return [...lines, ''];
}

function renderIndex(ref: CliReference, pages: Map<string, PageCommand[]>): string {
  const full = renderIndexPage(ref, pages, false);
  return pageWords(full) <= INDEX_WORD_BUDGET ? full : renderIndexPage(ref, pages, true);
}

function renderIndexPage(ref: CliReference, pages: Map<string, PageCommand[]>, compact: boolean): string {
  const { cli } = ref;
  const body: string[] = [];
  const intro = ref.intro ?? readHeader(ref.product, 'cli');
  if (intro) body.push(intro.trim(), '');
  body.push(
    codeFence('text', cli.usage ?? `${cli.name} [OPTIONS] <COMMAND>`, 'output'),
    ''
  );
  body.push(`Run \`${cli.name} <command> --help\` for the build you have.`, '');
  // Heading ids on this page, so the hidden per-command anchors never collide.
  const used = new Set<string>(['global-options', 'exit-codes', 'commands']);
  body.push('## Commands', '');
  for (const group of ref.groups) {
    const heading = groupTitle(group, cli.name);
    used.add(slug(group.title));
    body.push(`### ${heading}`, '', `${escapeMdxText(sentence(group.summary))} [Reference](${cliUrl(ref, group.slug)}).`, '');
    const roots = pages.get(group.slug) ?? [];
    const href = (a: string) => `${cliUrl(ref, group.slug)}#${a}`;
    if (compact) body.push(...compactCommandRows(roots, href, used));
    else body.push(...commandRows(roots.flatMap((c) => flatten(c.cmd, c.parents)), href, used));
  }
  body.push('## Global options', '');
  const globals = (cli.global_options ?? []).filter((o) => !o.hidden);
  if (globals.length) {
    body.push('Accepted by every command, before or after the subcommand.', '');
    body.push(...flagsTable(globals));
  }
  body.push(`\`--help\` prints help for any command; \`--version\` prints the version.`, '');
  if (cli.exit_codes?.length) {
    body.push('## Exit codes', '');
    body.push(...exitCodesTable(cli.exit_codes));
  }
  return renderPage({
    title: cli.name,
    description: sentence(cli.abstract),
    generator: ref.generator,
    source: ref.source,
    version: cli.version,
    body: body.join('\n'),
  });
}

function renderGroup(ref: CliReference, group: CliGroup, commands: PageCommand[]): string {
  const { cli } = ref;
  const body: string[] = [];
  const entries = commands.flatMap((c) => flatten(c.cmd, c.parents));
  const single = commands.length === 1;
  // The page's own command (single-command pages) has no heading of its own.
  const listed = single ? entries.slice(1) : entries;
  if (listed.length > 1) body.push(...commandRows(listed, (a) => `#${a}`));
  const header = ref.headers?.[group.slug] ?? readHeader(ref.product, `cli-${group.slug}`);
  if (header) body.push(header.trim(), '');
  body.push(
    `Every command also accepts the [global options](${cliUrl(ref)}#global-options).`,
    ''
  );
  if (single) {
    // Keep the page command's own anchor (`#cua-sandbox`) for index links.
    const [{ cmd, parents }] = commands;
    body.push(`<span id="${commandAnchor([...parents, cmd.name])}"></span>`, '');
    body.push(...renderCommand(cmd, parents, 0));
  }
  else for (const { cmd, parents } of commands) body.push(...renderCommand(cmd, parents, 2));
  if (cli.exit_codes?.length) {
    body.push('## Exit codes', '');
    body.push(...exitCodesTable(cli.exit_codes));
  }
  return renderPage({
    title: group.title,
    description: sentence(group.summary),
    generator: ref.generator,
    source: ref.source,
    version: cli.version,
    body: body.join('\n'),
  });
}

/**
 * Renders `reference/cli/`: index, one page per group and `meta.json`, keyed
 * by path relative to the product's `reference/` folder.
 */
export function renderCliReference(ref: CliReference): Map<string, string> {
  const pages = groupCommands(ref);
  const out = new Map<string, string>();
  out.set('cli/index.mdx', renderIndex(ref, pages));
  for (const group of ref.groups) {
    out.set(`cli/${group.slug}.mdx`, renderGroup(ref, group, pages.get(group.slug)!));
  }
  out.set('cli/meta.json', metaJson('CLI', ['index', ...ref.groups.map((g) => g.slug)]));
  return out;
}

/** Rows for a product Reference index: one per CLI group. */
export function cliIndexRows(ref: CliReference): IndexRow[] {
  return [
    { name: `\`${ref.cli.name}\``, href: cliUrl(ref), description: 'Synopsis, global options, exit codes and every command.' },
    ...ref.groups.map((g) => ({
      name: groupTitle(g, ref.cli.name),
      href: cliUrl(ref, g.slug),
      description: sentence(g.summary),
    })),
  ];
}

// ---------------------------------------------------------------- reference index

export interface IndexRow {
  /** Markdown for the first cell (already formatted). */
  name: string;
  href: string;
  description: string;
}

export interface IndexSection {
  title: string;
  intro?: string;
  /** First column header, e.g. `Command group`. */
  column: string;
  rows: IndexRow[];
}

/** The product's `reference/index.mdx`: one table per surface. */
export function renderReferenceIndex(page: {
  productName: string;
  description: string;
  generator: string;
  source: string;
  version?: string;
  sections: IndexSection[];
}): string {
  const body: string[] = [];
  for (const section of page.sections) {
    body.push(`## ${section.title}`, '');
    if (section.intro) body.push(section.intro.trim(), '');
    body.push(`| ${section.column} | Description |`, '| --- | --- |');
    for (const row of section.rows) {
      body.push(`| [${row.name}](${row.href}) | ${escapeTableCell(row.description)} |`);
    }
    body.push('');
  }
  return renderPage({
    title: 'Overview',
    description: page.description,
    generator: page.generator,
    source: page.source,
    version: page.version,
    body: body.join('\n'),
  });
}
