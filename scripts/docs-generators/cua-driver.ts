#!/usr/bin/env npx tsx

/**
 * Cua Driver Documentation Generator
 *
 * Generates MDX documentation files from the cua-driver `dump-docs` command output.
 * This ensures documentation stays synchronized with the source code.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/cua-driver.ts          # Generate docs
 *   npx tsx scripts/docs-generators/cua-driver.ts --check  # Check for drift (CI mode)
 */

import { execFileSync, execSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

// ============================================================================
// Types
// ============================================================================

export interface CLIDocumentation {
  name: string;
  version: string;
  abstract: string;
  commands: CommandDoc[];
}

export interface CommandDoc {
  name: string;
  abstract: string;
  discussion?: string;
  arguments: ArgumentDoc[];
  options: OptionDoc[];
  flags: FlagDoc[];
  subcommands: CommandDoc[];
}

export interface ArgumentDoc {
  name: string;
  help: string;
  type: string;
  is_optional: boolean;
}

export interface OptionDoc {
  name: string;
  short_name?: string | null;
  help: string;
  type: string;
  default_value?: string | null;
  is_optional: boolean;
}

export interface FlagDoc {
  name: string;
  short_name?: string | null;
  help: string;
  default_value: boolean;
}

export interface MCPToolDoc {
  name: string;
  description: string;
  input_schema: MCPInputSchema;
}

export interface MCPInputSchema {
  type: string;
  required?: string[];
  properties?: Record<string, MCPPropertyDoc>;
}

export interface MCPPropertyDoc {
  type: string;
  description: string;
  items?: { type: string };
}

export interface MCPDocumentation {
  version: string;
  tools: MCPToolDoc[];
}

export interface DumpDocsOutput {
  cli: CLIDocumentation;
  mcp: MCPDocumentation;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const CUA_DRIVER_DIR = path.join(ROOT_DIR, 'libs', 'cua-driver', 'rust');
const CUA_DRIVER_BIN = path.join(
  CUA_DRIVER_DIR,
  'target',
  'release',
  process.platform === 'win32' ? 'cua-driver.exe' : 'cua-driver'
);
const DOCS_OUTPUT_DIR = path.join(ROOT_DIR, 'docs', 'content', 'docs', 'reference', 'cua-driver');
const TAG_PREFIX = 'cua-driver-rs-v';

// ============================================================================
// Version Discovery
// ============================================================================

function resolveCargoCommand(): string {
  if (process.env.CARGO) {
    return process.env.CARGO;
  }

  const candidates = [
    'cargo',
    path.join(os.homedir() || '', '.cargo', 'bin', 'cargo'),
    '/opt/homebrew/bin/cargo',
    '/usr/local/bin/cargo',
  ];

  for (const candidate of candidates) {
    try {
      execFileSync(candidate, ['--version'], { stdio: 'ignore' });
      return candidate;
    } catch {
      // Try the next candidate.
    }
  }

  throw new Error('cargo not found on PATH; install Rust or set CARGO=/path/to/cargo');
}

/**
 * Get the latest released version from git tags.
 */
export function getLatestReleasedVersion(): string {
  try {
    const output = execSync(`git tag | grep "^${TAG_PREFIX}" | sort -V | tail -1`, {
      encoding: 'utf-8',
      cwd: ROOT_DIR,
    }).trim();
    if (output) {
      return output.replace(TAG_PREFIX, '');
    }
  } catch {
    // Fall through
  }
  return '0.0.0';
}

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const checkOnly = args.includes('--check') || args.includes('--check-only');

  console.log('Cua Driver Documentation Generator');
  console.log('===================================\n');

  // Step 1: Build cua-driver
  console.log('Building cua-driver...');
  try {
    const cargo = resolveCargoCommand();
    execFileSync(cargo, ['build', '-p', 'cua-driver', '--release'], {
      cwd: CUA_DRIVER_DIR,
      stdio: 'inherit',
    });
  } catch (error) {
    console.error('Failed to build cua-driver');
    process.exit(1);
  }

  // Step 2: Extract all docs in a single invocation
  console.log('\nExtracting documentation...');
  const binary = process.env.CUA_DRIVER_BINARY || CUA_DRIVER_BIN;
  const dumpDocsJson = execFileSync(binary, ['dump-docs', '--type', 'all', '--pretty'], {
    cwd: CUA_DRIVER_DIR,
    encoding: 'utf-8',
  });
  const dumpDocs: DumpDocsOutput = JSON.parse(dumpDocsJson);
  console.log(`   Found ${dumpDocs.cli.commands.length} CLI commands`);
  console.log(`   Found ${dumpDocs.mcp.tools.length} MCP tools`);

  // Step 3: Generate MDX files
  console.log('\nGenerating documentation files...');

  const releasedVersion = getLatestReleasedVersion();
  // Use the version embedded in the binary itself as the canonical current
  // version — it's always correct even on unreleased branches where no git
  // tag exists yet. Fall back to the latest released tag only when the
  // binary reports an empty/missing version.
  const currentVersion = dumpDocs.cli.version || dumpDocs.mcp.version || releasedVersion;

  const cliMdx = generateCLIReferenceMDX(dumpDocs.cli, currentVersion);
  const mcpMdx = generateMCPToolsMDX(dumpDocs.mcp, currentVersion);

  const cliPath = path.join(DOCS_OUTPUT_DIR, 'cli-reference.mdx');
  const mcpPath = path.join(DOCS_OUTPUT_DIR, 'mcp-tools.mdx');

  if (checkOnly) {
    // Check mode: compare with existing files
    console.log('\nChecking for documentation drift...');

    let hasDrift = false;

    if (fs.existsSync(cliPath)) {
      const existingCli = fs.readFileSync(cliPath, 'utf-8');
      if (existingCli !== cliMdx) {
        console.error('cli-reference.mdx is out of sync with source code');
        hasDrift = true;
      } else {
        console.log('cli-reference.mdx is up to date');
      }
    } else {
      console.error('cli-reference.mdx does not exist');
      hasDrift = true;
    }

    if (fs.existsSync(mcpPath)) {
      const existingMcp = fs.readFileSync(mcpPath, 'utf-8');
      if (existingMcp !== mcpMdx) {
        console.error('mcp-tools.mdx is out of sync with source code');
        hasDrift = true;
      } else {
        console.log('mcp-tools.mdx is up to date');
      }
    } else {
      console.error('mcp-tools.mdx does not exist');
      hasDrift = true;
    }

    if (hasDrift) {
      console.error(
        "\nRun 'npx tsx scripts/docs-generators/cua-driver.ts' to update documentation"
      );
      process.exit(1);
    }

    console.log('\nAll cua-driver documentation is up to date!');
  } else {
    // Generate mode: write files
    fs.mkdirSync(DOCS_OUTPUT_DIR, { recursive: true });

    fs.writeFileSync(cliPath, cliMdx);
    console.log(`   Generated ${path.relative(ROOT_DIR, cliPath)}`);

    fs.writeFileSync(mcpPath, mcpMdx);
    console.log(`   Generated ${path.relative(ROOT_DIR, mcpPath)}`);

    console.log('\ncua-driver documentation generated successfully!');
  }
}

// ============================================================================
// CLI Reference Generator
// ============================================================================

export function generateCLIReferenceMDX(docs: CLIDocumentation, releasedVersion: string): string {
  const lines: string[] = [];

  // Frontmatter — must be at the very beginning of the file
  lines.push('---');
  lines.push('title: CLI Reference');
  lines.push('description: Command-line interface specification for Cua Driver');
  lines.push('---');
  lines.push('');
  lines.push(`{/*
  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY
  Generated by: npx tsx scripts/docs-generators/cua-driver.ts
  Source: cua-driver dump-docs
  Version: ${releasedVersion}
*/}`);
  lines.push('');

  // Introduction
  lines.push(escapeMdxText(docs.abstract) + ' Install via the official script:');
  lines.push('');
  lines.push('```sh');
  lines.push(
    'curl -fsSL https://cua.ai/driver/install.sh | bash'
  );
  lines.push('```');
  lines.push('');
  lines.push(
    `Documented against Cua Driver **${releasedVersion}**. Run \`cua-driver --version\` for your installed version.`
  );
  lines.push('');
  lines.push(
    'The macOS-only `cua-driver permissions` command is documented separately in [macOS permissions](/reference/cua-driver/macos-permissions).'
  );
  lines.push('');
  lines.push('---');
  lines.push('');

  // Group commands by category
  const toolDispatch = ['call', 'list-tools', 'describe'];
  const daemonManagement = ['serve', 'stop', 'status', 'mcp', 'mcp-config'];
  const trajectoryRecording = ['recording'];
  const configuration = ['config'];
  const diagnostics = ['diagnose', 'update', 'check-update', 'doctor'];

  lines.push('## Tool dispatch');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => toolDispatch.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd));
  }

  lines.push('## Daemon management');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => daemonManagement.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd));
  }

  lines.push('## Trajectory recording');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => trajectoryRecording.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd));
  }

  lines.push('## Configuration');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => configuration.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd));
  }

  lines.push('## Diagnostics');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => diagnostics.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd));
  }

  // Any commands not yet categorised above
  const allCategorised = [
    ...toolDispatch,
    ...daemonManagement,
    ...trajectoryRecording,
    ...configuration,
    ...diagnostics,
  ];
  // Documented on their own pages, so keep them out of this reference.
  const excludedFromReference = ['permissions']; // -> reference/cua-driver/macos-permissions
  const uncategorised = docs.commands.filter(
    (c) => !allCategorised.includes(c.name) && !excludedFromReference.includes(c.name)
  );
  if (uncategorised.length > 0) {
    lines.push('## Other commands');
    lines.push('');
    for (const cmd of uncategorised) {
      lines.push(...generateCommandDoc(cmd));
    }
  }

  // Global options footer
  lines.push('## Global options');
  lines.push('');
  lines.push('Available on all commands:');
  lines.push('');
  lines.push('- `--help` — Show help information.');
  lines.push('- `--version` — Show version number.');
  lines.push('');

  return lines.join('\n');
}

export function generateCommandDoc(cmd: CommandDoc): string[] {
  const lines: string[] = [];

  lines.push(`### \`cua-driver ${cmd.name}\``);
  lines.push('');
  lines.push(escapeMdxText(cmd.abstract));
  lines.push('');

  if (cmd.discussion) {
    lines.push(escapeMdxText(cmd.discussion));
    lines.push('');
  }

  // Arguments table
  if (cmd.arguments.length > 0) {
    lines.push('**Arguments:**');
    lines.push('');
    lines.push('| Name | Type | Required | Description |');
    lines.push('| ---- | ---- | -------- | ----------- |');
    for (const arg of cmd.arguments) {
      const required = arg.is_optional ? 'No' : 'Yes';
      lines.push(
        `| \`<${arg.name}>\` | ${escapeTableCell(arg.type)} | ${required} | ${escapeTableCell(arg.help)} |`
      );
    }
    lines.push('');
  }

  // Options table
  if (cmd.options.length > 0) {
    lines.push('**Options:**');
    lines.push('');
    lines.push('| Name | Type | Default | Description |');
    lines.push('| ---- | ---- | ------- | ----------- |');
    for (const opt of cmd.options) {
      const nameCell = opt.short_name
        ? `\`-${opt.short_name}\`, \`--${opt.name}\``
        : `\`--${opt.name}\``;
      const defaultVal = opt.default_value != null ? opt.default_value : '—';
      lines.push(
        `| ${nameCell} | ${escapeTableCell(opt.type)} | ${escapeTableCell(defaultVal)} | ${escapeTableCell(opt.help)} |`
      );
    }
    lines.push('');
  }

  // Flags table
  if (cmd.flags.length > 0) {
    lines.push('**Flags:**');
    lines.push('');
    lines.push('| Name | Description |');
    lines.push('| ---- | ----------- |');
    for (const flag of cmd.flags) {
      const nameCell = flag.short_name
        ? `\`-${flag.short_name}\`, \`--${flag.name}\``
        : `\`--${flag.name}\``;
      lines.push(`| ${nameCell} | ${escapeTableCell(flag.help)} |`);
    }
    lines.push('');
  }

  // Subcommands
  if (cmd.subcommands.length > 0) {
    for (const sub of cmd.subcommands) {
      lines.push(`#### \`cua-driver ${cmd.name} ${sub.name}\``);
      lines.push('');
      lines.push(escapeMdxText(sub.abstract));
      lines.push('');

      if (sub.discussion) {
        lines.push(escapeMdxText(sub.discussion));
        lines.push('');
      }

      if (sub.arguments.length > 0) {
        lines.push('**Arguments:**');
        lines.push('');
        lines.push('| Name | Type | Required | Description |');
        lines.push('| ---- | ---- | -------- | ----------- |');
        for (const arg of sub.arguments) {
          const required = arg.is_optional ? 'No' : 'Yes';
          lines.push(
            `| \`<${arg.name}>\` | ${escapeTableCell(arg.type)} | ${required} | ${escapeTableCell(arg.help)} |`
          );
        }
        lines.push('');
      }

      if (sub.options.length > 0) {
        lines.push('**Options:**');
        lines.push('');
        lines.push('| Name | Type | Default | Description |');
        lines.push('| ---- | ---- | ------- | ----------- |');
        for (const opt of sub.options) {
          const nameCell = opt.short_name
            ? `\`-${opt.short_name}\`, \`--${opt.name}\``
            : `\`--${opt.name}\``;
          const defaultVal = opt.default_value != null ? opt.default_value : '—';
          lines.push(
            `| ${nameCell} | ${escapeTableCell(opt.type)} | ${escapeTableCell(defaultVal)} | ${escapeTableCell(opt.help)} |`
          );
        }
        lines.push('');
      }

      if (sub.flags.length > 0) {
        lines.push('**Flags:**');
        lines.push('');
        lines.push('| Name | Description |');
        lines.push('| ---- | ----------- |');
        for (const flag of sub.flags) {
          const nameCell = flag.short_name
            ? `\`-${flag.short_name}\`, \`--${flag.name}\``
            : `\`--${flag.name}\``;
          lines.push(`| ${nameCell} | ${escapeTableCell(flag.help)} |`);
        }
        lines.push('');
      }

      // Nested subcommands
      if (sub.subcommands.length > 0) {
        for (const nested of sub.subcommands) {
          lines.push(`##### \`cua-driver ${cmd.name} ${sub.name} ${nested.name}\``);
          lines.push('');
          lines.push(escapeMdxText(nested.abstract));
          lines.push('');

          if (nested.discussion) {
            lines.push(escapeMdxText(nested.discussion));
            lines.push('');
          }

          if (nested.arguments.length > 0) {
            lines.push('**Arguments:**');
            lines.push('');
            lines.push('| Name | Type | Required | Description |');
            lines.push('| ---- | ---- | -------- | ----------- |');
            for (const arg of nested.arguments) {
              const required = arg.is_optional ? 'No' : 'Yes';
              lines.push(
                `| \`<${arg.name}>\` | ${escapeTableCell(arg.type)} | ${required} | ${escapeTableCell(arg.help)} |`
              );
            }
            lines.push('');
          }

          if (nested.options.length > 0) {
            lines.push('**Options:**');
            lines.push('');
            lines.push('| Name | Type | Default | Description |');
            lines.push('| ---- | ---- | ------- | ----------- |');
            for (const opt of nested.options) {
              const nameCell = opt.short_name
                ? `\`-${opt.short_name}\`, \`--${opt.name}\``
                : `\`--${opt.name}\``;
              const defaultVal = opt.default_value != null ? opt.default_value : '—';
              lines.push(
                `| ${nameCell} | ${escapeTableCell(opt.type)} | ${escapeTableCell(defaultVal)} | ${escapeTableCell(opt.help)} |`
              );
            }
            lines.push('');
          }

          if (nested.flags.length > 0) {
            lines.push('**Flags:**');
            lines.push('');
            lines.push('| Name | Description |');
            lines.push('| ---- | ----------- |');
            for (const flag of nested.flags) {
              const nameCell = flag.short_name
                ? `\`-${flag.short_name}\`, \`--${flag.name}\``
                : `\`--${flag.name}\``;
              lines.push(`| ${nameCell} | ${escapeTableCell(flag.help)} |`);
            }
            lines.push('');
          }
        }
      }
    }
  }

  return lines;
}

// ============================================================================
// MCP Tools Generator
// ============================================================================

export function generateMCPToolsMDX(docs: MCPDocumentation, releasedVersion: string): string {
  const lines: string[] = [];

  // Frontmatter — must be at the very beginning of the file
  lines.push('---');
  lines.push('title: MCP Tools');
  lines.push('description: Reference for every MCP tool Cua Driver exposes');
  lines.push('---');
  lines.push('');
  lines.push(`{/*
  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY
  Generated by: npx tsx scripts/docs-generators/cua-driver.ts
  Source: cua-driver dump-docs
  Version: ${releasedVersion}
*/}`);
  lines.push('');
  lines.push("import { Callout } from 'fumadocs-ui/components/callout';");
  lines.push('');

  // Introduction — mirror the existing hand-written header prose
  lines.push(
    `\`cua-driver\` exposes ${docs.tools.length} MCP tools through a single stdio server (\`cua-driver mcp\`). Every tool is also callable from the shell as \`cua-driver <name> '<JSON-args>'\`.`
  );
  lines.push('');
  lines.push(
    'Tool names are `snake_case`. Responses are MCP `CallTool.Result` envelopes: a text content block prefixed with a `✅` summary (or the error reason on failure), plus optional image or structured-content blocks on tools that produce them. See the [CLI reference](/reference/cua-driver/cli-reference) for CLI-specific options like `--socket` and `--screenshot-out-file`.'
  );
  lines.push('');
  lines.push(
    'For the cross-cutting parameter contract (shared parameters, required-parameter rules, platform-specific parameters) and the action response shape, see [MCP tool notes](/reference/cua-driver/mcp-tool-notes).'
  );
  lines.push('');
  lines.push('<Callout type="info">');
  lines.push(
    '  Tool names here match the CLI form exactly. `cua-driver list_apps` and the MCP `list_apps` tool run the same code path.'
  );
  lines.push('</Callout>');
  lines.push('');
  lines.push('<Callout type="info">');
  lines.push(
    "  **TCC auto-delegation.** When an MCP client spawns `cua-driver mcp` from an IDE terminal (Claude Code, Cursor, VS Code, Warp), macOS attributes the subprocess to the parent terminal — not `CuaDriver.app` — so AX probes fail against the wrong bundle id. `mcp` detects this and auto-launches a `cua-driver serve` daemon via `open -n -g -a CuaDriver --args serve`, then proxies every tool call through the daemon's Unix socket. Tool semantics are identical to the in-process path; no Python bridge is needed. Pass `--no-daemon-relaunch` (or set `CUA_DRIVER_MCP_NO_RELAUNCH=1`) to force in-process execution. See the [process model](/reference/cua-driver/process-model) for the full lifecycle, failure modes, and wrapper-author guidance."
  );
  lines.push('</Callout>');
  lines.push('');

  const categories: Array<{ title: string; tools: string[] }> = [
    {
      title: 'Inspection tools',
      tools: [
        'list_apps',
        'list_windows',
        'get_window_state',
        'get_accessibility_tree',
        'get_desktop_state',
        'get_screen_size',
        'get_cursor_position',
        'get_config',
        'get_recording_state',
        'get_agent_cursor_state',
      ],
    },
    {
      title: 'Action tools',
      tools: [
        'launch_app',
        'kill_app',
        'bring_to_front',
        'click',
        'double_click',
        'right_click',
        'drag',
        'type_text',
        'press_key',
        'hotkey',
        'set_value',
        'scroll',
        'move_cursor',
        'zoom',
      ],
    },
    {
      title: 'Browser tools',
      tools: ['page'],
    },
    {
      title: 'Recording tools',
      tools: ['start_recording', 'stop_recording', 'replay_trajectory'],
    },
    {
      title: 'Configuration tools',
      tools: [
        'set_config',
        'start_session',
        'end_session',
        'set_agent_cursor_enabled',
        'set_agent_cursor_motion',
        'set_agent_cursor_style',
      ],
    },
    {
      title: 'Maintenance tools',
      tools: [
        'check_permissions',
        'health_report',
        'check_for_update',
        'install_ffmpeg',
      ],
    },
  ];

  const byName = new Map(docs.tools.map((tool) => [tool.name, tool]));
  const emitted = new Set<string>();

  for (const category of categories) {
    const tools = category.tools
      .map((name) => byName.get(name))
      .filter((tool): tool is MCPToolDoc => Boolean(tool));
    if (tools.length === 0) continue;

    lines.push(`## ${category.title}`);
    lines.push('');
    for (const tool of tools) {
      emitted.add(tool.name);
      lines.push(...generateMCPToolDoc(tool));
    }
  }

  const uncategorised = docs.tools.filter((tool) => !emitted.has(tool.name));
  if (uncategorised.length > 0) {
    lines.push('## Other tools');
    lines.push('');
    for (const tool of uncategorised) {
      lines.push(...generateMCPToolDoc(tool));
    }
  }

  return lines.join('\n');
}

export function generateMCPToolDoc(tool: MCPToolDoc): string[] {
  const lines: string[] = [];

  lines.push(`### \`${tool.name}\``);
  lines.push('');
  lines.push(escapeMdxText(tool.description));
  lines.push('');

  const properties = tool.input_schema.properties ?? {};
  const required = new Set(tool.input_schema.required ?? []);
  const propertyNames = Object.keys(properties);

  if (propertyNames.length === 0) {
    lines.push('**Arguments:** none.');
    lines.push('');
  } else {
    lines.push('**Arguments:**');
    lines.push('');
    for (const propName of propertyNames) {
      const prop = properties[propName];
      const isRequired = required.has(propName);
      const requiredLabel = isRequired ? 'required' : 'optional';
      const typeLabel = formatPropertyType(prop);
      const description = escapeMdxText(prop.description ?? '');
      const suffix = description ? `: ${description}` : '';
      lines.push(`- \`${propName}\` (${typeLabel}, ${requiredLabel})${suffix}`);
    }
    lines.push('');
  }

  // Synthesize a minimal JSON example from required params only.
  // Skip the example entirely when there are no required params (tools
  // like check_permissions where everything is optional) or when the
  // required set is empty after filtering — avoid emitting {} for tools
  // that are only valid with at least one of several mutually-optional groups.
  const exampleObj: Record<string, unknown> = {};
  for (const propName of propertyNames) {
    if (required.has(propName)) {
      exampleObj[propName] = syntheticExampleValue(propName, properties[propName]);
    }
  }

  if (Object.keys(exampleObj).length > 0) {
    lines.push('```json');
    lines.push(JSON.stringify(exampleObj));
    lines.push('```');
    lines.push('');
  }

  return lines;
}

function escapeTableCell(value: string): string {
  return escapeMdxText(value.replace(/\n/g, ' ')).replace(/\|/g, '\\|');
}

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

function formatPropertyType(prop: MCPPropertyDoc): string {
  if (prop.type === 'array') {
    const itemType = prop.items?.type ?? 'unknown';
    return `array of ${itemType}`;
  }
  return prop.type;
}

function syntheticExampleValue(name: string, prop: MCPPropertyDoc): unknown {
  switch (prop.type) {
    case 'integer':
      if (name === 'pid') return 844;
      if (name === 'window_id') return 10725;
      if (name === 'element_index') return 14;
      if (name === 'x' || name === 'x1' || name === 'x2' || name === 'from_x' || name === 'to_x')
        return 100;
      if (name === 'y' || name === 'y1' || name === 'y2' || name === 'from_y' || name === 'to_y')
        return 200;
      return 1;
    case 'number':
      if (name === 'x' || name === 'x1' || name === 'x2' || name === 'from_x' || name === 'to_x')
        return 100;
      if (name === 'y' || name === 'y1' || name === 'y2' || name === 'from_y' || name === 'to_y')
        return 200;
      return 0.5;
    case 'boolean':
      return false;
    case 'array':
      if (name === 'keys') return ['cmd', 'c'];
      if (name === 'modifiers') return ['cmd'];
      return [];
    case 'string':
      if (name === 'text') return 'hello';
      if (name === 'key') return 'return';
      if (name === 'direction') return 'down';
      if (name === 'action') return 'get_text';
      if (name === 'bundle_id') return 'com.apple.finder';
      if (name === 'value') return '42';
      if (name === 'key_path' || name === 'key') return 'capture_mode';
      if (name === 'dir' || name === 'output_dir') return '~/cua-trajectories/demo1';
      return 'example';
    default:
      return 'value';
  }
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
