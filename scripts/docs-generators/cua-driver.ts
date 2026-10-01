#!/usr/bin/env npx tsx

/**
 * Cua Driver reference generator.
 *
 * Builds `cua-driver` (release), runs `cua-driver dump-docs --type all` and
 * writes, through the shared renderers in lib/:
 *
 * - docs/content/docs/cua-driver/reference/index.mdx (the Reference index)
 * - docs/content/docs/cua-driver/reference/cli/ (one page per command group)
 * - docs/content/docs/cua-driver/reference/mcp-tools/ (one page per tool
 *   category, every platform on one page: tabs where macOS, Linux and
 *   Windows differ)
 * - scripts/docs-generators/cli-specs/cua-driver.json (the CLI, host
 *   independent: the CLI-shape lane's oracle)
 * - scripts/docs-generators/cli-specs/cua-driver-mcp-<platform>.json (the
 *   tool registry of each platform)
 *
 * The tool registry differs per OS and only a native build can dump it, so
 * each host owns its own snapshot: a run refreshes (or, with --check,
 * verifies) the snapshot of the host it runs on and renders every page from
 * the three committed snapshots. The pages are therefore identical on every
 * host, and CI on macOS, Linux and Windows each checks its own registry.
 * A run verifies only its own host's registry: the other two snapshots are
 * rendered as committed. A snapshot that was not dumped natively carries a
 * `provenance` note, which every run prints.
 *
 * Usage:
 *   pnpm --dir docs docs:generate:cua-driver
 *   pnpm --dir docs docs:check:cua-driver        # drift check (CI)
 *
 * CUA_DRIVER_BINARY=<path> skips the build and uses that binary.
 */

import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
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
import {
  type JsonSchema,
  type McpCategory,
  type McpReference,
  type McpToolEntry,
  mcpIndexRows,
  renderMcpReference,
} from './lib/mcp-mdx';
import { DOCS_CONTENT, REPO_ROOT, finish, isCheckMode, metaJson, stableJson, syncFiles } from './lib/mdx';

export type { CLIDocumentation };

const CUA_DRIVER_DIR = path.join(REPO_ROOT, 'libs', 'cua-driver', 'rust');
const OUT_DIR = path.join(DOCS_CONTENT, 'cua-driver', 'reference');
const SPEC_DIR = path.join(__dirname, 'cli-specs');
export const DRIVER_CLI_SPEC = path.join(SPEC_DIR, 'cua-driver.json');
const REGENERATE = 'pnpm --dir docs docs:generate:cua-driver';
const TAG_PREFIX = 'cua-driver-rs-v';

// ---------------------------------------------------------------- types

export interface MCPToolDoc {
  name: string;
  description: string;
  input_schema: JsonSchema;
  read_only?: boolean;
  destructive?: boolean;
  idempotent?: boolean;
}

export interface MCPDocumentation {
  version?: string;
  tools: MCPToolDoc[];
}

export interface DumpDocsOutput {
  cli: CLIDocumentation;
  mcp: MCPDocumentation;
}

/** A platform's committed registry snapshot. */
export interface McpSnapshot {
  /**
   * Set only when the snapshot was not dumped by a native build: says where it
   * came from. A native run on that host (the generator locally, or the
   * ci-check-docs.yml artifact) rewrites the file without it.
   */
  provenance?: string;
  tools: MCPToolDoc[];
}

/** `tool.param` for every top-level input parameter without a description. */
export function undocumentedParameters(snapshot: McpSnapshot): string[] {
  return snapshot.tools.flatMap((tool) =>
    Object.entries(tool.input_schema?.properties ?? {})
      .filter(([, schema]) => typeof schema?.description !== 'string' || !schema.description.trim())
      .map(([param]) => `${tool.name}.${param}`)
  );
}

export interface ReferencePlatform {
  /** `process.platform` of the host that owns the snapshot. */
  host: string;
  /** Snapshot key: `cli-specs/cua-driver-mcp-<key>.json`. */
  key: string;
  /** Display name (tab label). */
  name: string;
}

export const referencePlatforms: ReferencePlatform[] = [
  { host: 'darwin', key: 'macos', name: 'macOS' },
  { host: 'linux', key: 'linux', name: 'Linux' },
  { host: 'win32', key: 'windows', name: 'Windows' },
];

export function referencePlatform(host: string): ReferencePlatform {
  const platform = referencePlatforms.find((candidate) => candidate.host === host);
  if (!platform) throw new Error(`Native MCP reference generation is not implemented for ${host}`);
  return platform;
}

export function snapshotPath(platform: ReferencePlatform, dir = SPEC_DIR): string {
  return path.join(dir, `cua-driver-mcp-${platform.key}.json`);
}

// ---------------------------------------------------------------- version

export type GitRunner = (command: string, args: readonly string[]) => string;

/**
 * Select the highest stable semantic version from Cua Driver release tags.
 * Nightlies use their own `nightly-cua-driver-rs-v` prefix and must not affect
 * the version recorded in stable reference docs.
 */
export function selectLatestReleasedVersion(tags: readonly string[]): string | undefined {
  const versions = tags.flatMap((tag) => {
    const match = tag.match(/^cua-driver-rs-v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
    if (!match) return [];
    return [
      {
        version: tag.slice(TAG_PREFIX.length),
        parts: [BigInt(match[1]), BigInt(match[2]), BigInt(match[3])] as const,
      },
    ];
  });
  versions.sort((a, b) => {
    for (let index = 0; index < a.parts.length; index += 1) {
      if (a.parts[index] < b.parts[index]) return -1;
      if (a.parts[index] > b.parts[index]) return 1;
    }
    return 0;
  });
  return versions.at(-1)?.version;
}

function runGit(command: string, args: readonly string[]): string {
  return execFileSync(command, [...args], { encoding: 'utf-8', cwd: REPO_ROOT });
}

/** Get the latest stable released version from git tags. */
export function getLatestReleasedVersion(git: GitRunner = runGit): string {
  let output: string;
  try {
    output = git('git', ['tag', '--list', `${TAG_PREFIX}*`]);
  } catch (error) {
    const detail = error instanceof Error ? `: ${error.message}` : '';
    throw new Error(`Failed to list Cua Driver release tags with git${detail}`);
  }
  const version = selectLatestReleasedVersion(output.split(/\r?\n/).filter(Boolean));
  if (!version) {
    throw new Error(
      `No stable Cua Driver release tag matching ${TAG_PREFIX}<major>.<minor>.<patch> was found`
    );
  }
  return version;
}

// ---------------------------------------------------------------- CLI

export const CLI_GROUPS: CliGroup[] = [
  {
    slug: 'mcp',
    title: 'Tools and MCP',
    summary: 'Serve MCP, print client config, and list, describe or call tools from the shell',
    commands: ['mcp', 'mcp-config', 'list-tools', 'describe', 'call', 'manifest', 'dump-docs'],
  },
  {
    slug: 'daemon',
    title: 'Daemon',
    summary: 'Run, stop and inspect the daemon, its sessions and autostart',
    commands: ['serve', 'stop', 'status', 'sessions', 'revoke', 'autostart'],
  },
  {
    slug: 'permissions',
    title: 'Permissions and diagnostics',
    summary: 'macOS permission grants, diagnostic probes and install reports',
    commands: ['permissions', 'doctor', 'diagnose'],
  },
  {
    slug: 'recording',
    title: 'Trajectory recording',
    summary: 'Record, inspect and render action trajectories',
    commands: ['recording'],
  },
  {
    slug: 'history',
    title: 'Computer History',
    summary: 'The encrypted, metadata-only Computer History preview',
    commands: ['history'],
  },
  {
    slug: 'config',
    title: 'Configuration',
    summary: 'Driver configuration, telemetry and cursor themes',
    commands: ['config', 'telemetry', 'cursor-theme'],
  },
  {
    slug: 'extensions',
    title: 'Skills and extensions',
    summary: 'The agent skill pack, signed extensions and the perception parser',
    commands: ['skills', 'extension', 'perception'],
  },
  {
    slug: 'updates',
    title: 'Updates',
    summary: 'Check for and apply updates, and choose the release channel',
    commands: ['check-update', 'update', 'channel'],
  },
];

export function cliReference(cli: CLIDocumentation): CliReference {
  return { product: 'cua-driver', cli, groups: CLI_GROUPS, generator: REGENERATE, source: 'cua-driver dump-docs --type cli' };
}

// ---------------------------------------------------------------- MCP

export const MCP_CATEGORIES: Array<McpCategory & { tools: string[] }> = [
  {
    slug: 'apps-and-windows',
    title: 'App and window tools',
    summary: 'List, launch, quit, front and arrange apps and windows',
    tools: ['list_apps', 'list_windows', 'launch_app', 'kill_app', 'bring_to_front', 'set_window_frame', 'invoke_menu', 'debug_window_info'],
  },
  {
    slug: 'window-state',
    title: 'Window state tools',
    summary: 'Screenshots, accessibility trees, verification and visual regions of a window',
    tools: ['get_window_state', 'get_accessibility_tree', 'verify_state', 'parse_visual_regions'],
  },
  {
    slug: 'screen',
    title: 'Screen tools',
    summary: 'The desktop, screen size, cursor position and zoomed crops',
    tools: ['get_desktop_state', 'get_screen_size', 'get_cursor_position', 'zoom'],
  },
  {
    slug: 'click',
    title: 'Click tool',
    summary: 'Click by element or pixel',
    tools: ['click'],
  },
  {
    slug: 'double-and-right-click',
    title: 'Double-click and right-click tools',
    summary: 'Double-click and right-click by element or pixel',
    tools: ['double_click', 'right_click'],
  },
  {
    slug: 'pointer',
    title: 'Pointer tools',
    summary: 'Drag, scroll, move the pointer, and press or release buttons',
    tools: ['drag', 'scroll', 'move_cursor', 'mouse_button_down', 'mouse_button_up', 'mouse_drag', 'parallel_mouse_drag'],
  },
  {
    slug: 'keyboard',
    title: 'Typing tools',
    summary: 'Type text into a field or window',
    tools: ['type_text', 'type_text_chars'],
  },
  {
    slug: 'keys-and-shortcuts',
    title: 'Key and shortcut tools',
    summary: 'Press keys and keyboard shortcuts',
    tools: ['press_key', 'hotkey'],
  },
  {
    slug: 'values-and-clipboard',
    title: 'Value and clipboard tools',
    summary: 'Set element values and read or write the clipboard',
    tools: ['set_value', 'clipboard_read', 'clipboard_write'],
  },
  {
    slug: 'page',
    title: 'Page tools',
    summary: 'Read and act on web pages, and prepare and navigate browsers',
    tools: ['page', 'get_browser_state', 'browser_prepare', 'browser_navigate'],
  },
  {
    slug: 'browser-input',
    title: 'Browser input tools',
    summary: 'Exact browser input over CDP: clicks, typing, pointer, dialogs, files and downloads',
    tools: ['browser_click', 'browser_type', 'browser_pointer', 'browser_dialog', 'browser_set_input_files', 'browser_download'],
  },
  {
    slug: 'sessions',
    title: 'Session tools',
    summary: 'Start, escalate, inspect and end lifecycle sessions',
    tools: ['start_session', 'escalate_session', 'get_session', 'list_sessions', 'get_session_state', 'end_session'],
  },
  {
    slug: 'agent-cursor',
    title: 'Agent cursor tools',
    summary: 'Show, animate and style the agent cursor',
    tools: ['set_agent_cursor_enabled', 'set_agent_cursor_motion', 'set_agent_cursor_theme', 'get_agent_cursor_state'],
  },
  {
    slug: 'recording',
    title: 'Recording tools',
    summary: 'Record and replay trajectories',
    tools: ['start_recording', 'stop_recording', 'get_recording_state', 'replay_trajectory', 'install_ffmpeg'],
  },
  {
    slug: 'maintenance',
    title: 'Configuration and maintenance tools',
    summary: 'Configuration, permissions, health, updates and extensions',
    tools: ['get_config', 'set_config', 'check_permissions', 'health_report', 'check_for_update', 'install_extension'],
  },
];

/** The registry fields the pages use (the per-host version is left out: it drifts every release). */
export function mcpSnapshot(mcp: MCPDocumentation): McpSnapshot {
  return {
    tools: mcp.tools.map((t) => ({
      name: t.name,
      description: t.description,
      input_schema: t.input_schema,
      read_only: Boolean(t.read_only),
      destructive: Boolean(t.destructive),
      idempotent: Boolean(t.idempotent),
    })),
  };
}

/** One entry per tool across the platforms' registries. */
export function mergeTools(snapshots: Map<ReferencePlatform, McpSnapshot>): McpToolEntry[] {
  const categoryOf = new Map<string, string>();
  for (const c of MCP_CATEGORIES) for (const t of c.tools) categoryOf.set(t, c.slug);
  const names = new Set<string>();
  for (const snap of snapshots.values()) for (const t of snap.tools) names.add(t.name);
  const uncategorized = [...names].filter((n) => !categoryOf.has(n)).sort();
  if (uncategorized.length) {
    throw new Error(
      `cua-driver MCP tools without a docs category: ${uncategorized.join(', ')}. Add them to MCP_CATEGORIES in scripts/docs-generators/cua-driver.ts.`
    );
  }
  const order = MCP_CATEGORIES.flatMap((c) => c.tools).filter((n) => names.has(n));
  return order.map((name) => {
    const present = referencePlatforms.flatMap((p) => {
      const tool = snapshots.get(p)?.tools.find((t) => t.name === name);
      return tool ? [{ platform: p, tool }] : [];
    });
    const first = present[0].tool;
    return {
      name,
      description: first.description,
      category: categoryOf.get(name)!,
      input_schema: first.input_schema,
      annotations: { read_only: first.read_only, destructive: first.destructive, idempotent: first.idempotent },
      platforms: present.map((x) => x.platform.name),
      variants: present.map(({ platform, tool }) => ({
        platform: platform.name,
        description: tool.description,
        input_schema: tool.input_schema,
      })),
    };
  });
}

export function mcpReference(snapshots: Map<ReferencePlatform, McpSnapshot>, version: string): McpReference {
  return {
    product: 'cua-driver',
    server: 'cua-driver mcp',
    generator: REGENERATE,
    source: 'cua-driver dump-docs --type mcp (macOS, Linux and Windows registries)',
    version,
    description: 'Every tool cua-driver serves over MCP on macOS, Linux and Windows, with its parameters.',
    categories: MCP_CATEGORIES,
    tools: mergeTools(snapshots),
    header: readHeader('cua-driver', 'mcp-tools'),
    platforms: referencePlatforms.map((p) => p.name),
  };
}

// ---------------------------------------------------------------- render

/** Every reference file, keyed by path relative to `reference/`. */
export function renderReference(cli: CLIDocumentation, snapshots: Map<ReferencePlatform, McpSnapshot>): Map<string, string> {
  const cliRef = cliReference(cli);
  const mcp = mcpReference(snapshots, cli.version);
  const rel = new Map<string, string>([...renderCliReference(cliRef), ...renderMcpReference(mcp)]);
  rel.set(
    'index.mdx',
    renderReferenceIndex({
      productName: 'Cua Driver',
      description: 'Every cua-driver command and every MCP tool on macOS, Linux and Windows, generated from the driver.',
      generator: REGENERATE,
      source: 'cua-driver dump-docs --type all',
      version: cli.version,
      sections: [
        { title: 'Commands', column: 'Command group', rows: cliIndexRows(cliRef) },
        { title: 'MCP tools', column: 'Tools', rows: mcpIndexRows(mcp) },
      ],
    })
  );
  rel.set('meta.json', metaJson('Reference', ['index', 'cli', 'mcp-tools']));
  return rel;
}

export function loadSnapshots(dir = SPEC_DIR): Map<ReferencePlatform, McpSnapshot> {
  const out = new Map<ReferencePlatform, McpSnapshot>();
  for (const p of referencePlatforms) {
    const file = snapshotPath(p, dir);
    if (!fs.existsSync(file)) throw new Error(`missing ${path.relative(REPO_ROOT, file)}: run the generator on ${p.name}`);
    out.set(p, JSON.parse(fs.readFileSync(file, 'utf-8')));
  }
  return out;
}

/**
 * All files for a run on `platform`: that platform's snapshot comes from
 * `docs` (the native dump), the other two from `specDir`.
 */
export function referenceFiles(
  docs: DumpDocsOutput,
  platform: ReferencePlatform,
  outDir = OUT_DIR,
  specDir = SPEC_DIR
): Map<string, string> {
  const snapshots = loadSnapshots(specDir);
  const own = mcpSnapshot(docs.mcp);
  snapshots.set(platform, own);
  const files = new Map<string, string>();
  for (const [p, content] of renderReference(docs.cli, snapshots)) files.set(path.join(outDir, p), content);
  files.set(path.join(specDir, 'cua-driver.json'), stableJson(docs.cli));
  files.set(snapshotPath(platform, specDir), stableJson(own));
  return files;
}

// ---------------------------------------------------------------- extract

type DocumentationRunner = (
  binary: string,
  args: string[],
  options: { cwd: string; encoding: 'utf-8'; env: NodeJS.ProcessEnv }
) => string;

export function extractDocumentation(
  binary: string,
  environment: NodeJS.ProcessEnv = process.env,
  run: DocumentationRunner = (command, args, options) =>
    execFileSync(command, args, { ...options, maxBuffer: 64 * 1024 * 1024 })
): DumpDocsOutput {
  // The docs describe the complete registry: drop tool policies (either case).
  const policyVariables = new Set(['CUA_DRIVER_POLICY_FILE', 'CUA_DRIVER_MANAGED_POLICY_FILE']);
  const env = Object.fromEntries(
    Object.entries(environment).filter(([key]) => !policyVariables.has(key.toUpperCase()))
  );
  return JSON.parse(
    run(binary, ['dump-docs', '--type', 'all', '--pretty'], { cwd: CUA_DRIVER_DIR, encoding: 'utf-8', env })
  );
}

/** `$CARGO_TARGET_DIR` when set (relative to the crate dir), else `target/`. */
export function targetDir(dir = CUA_DRIVER_DIR): string {
  return process.env.CARGO_TARGET_DIR ? path.resolve(dir, process.env.CARGO_TARGET_DIR) : path.join(dir, 'target');
}

export function resolveDriverBinary(): string {
  return (
    process.env.CUA_DRIVER_BINARY ||
    path.join(targetDir(), 'release', process.platform === 'win32' ? 'cua-driver.exe' : 'cua-driver')
  );
}

function resolveCargoCommand(): string {
  if (process.env.CARGO) return process.env.CARGO;
  for (const candidate of ['cargo', path.join(os.homedir() || '', '.cargo', 'bin', 'cargo'), '/opt/homebrew/bin/cargo', '/usr/local/bin/cargo']) {
    try {
      execFileSync(candidate, ['--version'], { stdio: 'ignore' });
      return candidate;
    } catch {
      // Try the next candidate.
    }
  }
  throw new Error('cargo not found on PATH; install Rust or set CARGO=/path/to/cargo');
}

function main(): void {
  const checkOnly = isCheckMode();
  const platform = referencePlatform(process.platform);
  if (!process.env.CUA_DRIVER_BINARY) {
    execFileSync(resolveCargoCommand(), ['build', '-p', 'cua-driver', '--release'], {
      cwd: CUA_DRIVER_DIR,
      stdio: 'inherit',
    });
  }
  const docs = extractDocumentation(resolveDriverBinary());
  if (!docs.cli.version) docs.cli.version = getLatestReleasedVersion();
  const owned = [OUT_DIR, path.join(OUT_DIR, 'cli'), path.join(OUT_DIR, 'mcp-tools')];
  const others = referencePlatforms.filter((p) => p !== platform);
  console.log(
    `Cua Driver: ${checkOnly ? 'verifying' : 'refreshing'} the ${platform.name} registry from a native dump; ` +
      `the ${others.map((p) => p.name).join(' and ')} snapshots are rendered as committed ` +
      `(only a run on that host verifies them).`
  );
  for (const [p, snapshot] of loadSnapshots()) {
    if (p !== platform && snapshot.provenance) {
      console.log(`note: ${path.relative(REPO_ROOT, snapshotPath(p))}: ${snapshot.provenance}`);
    }
  }
  finish(
    `Cua Driver (${platform.name} registry)`,
    syncFiles(referenceFiles(docs, platform), checkOnly, owned),
    checkOnly,
    `${REGENERATE} (on ${platform.name})`
  );
}

if (require.main === module) main();
