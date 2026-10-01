#!/usr/bin/env npx tsx

/**
 * Documentation Generator Runner
 *
 * Orchestrates documentation generation across all configured libraries.
 * Reads config.json and runs appropriate generators based on what changed.
 *
 * Usage:
 *   pnpm --dir docs docs:generate                  # Generate all enabled docs
 *   pnpm --dir docs docs:check                     # Check for drift (CI mode)
 *   pnpm --dir docs docs:check:lume                # Check one library
 *   pnpm --dir docs docs:list                      # List configured generators
 */

import { execSync, spawnSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { parseArgs } from 'node:util';

// ============================================================================
// Types
// ============================================================================

interface GeneratorOutput {
  type: string;
  outputFile: string;
  extractCommand: string | null;
}

interface GeneratorConfig {
  name: string;
  language: string;
  sourcePath: string;
  docsOutputPath: string;
  generatorScript: string;
  watchPaths: string[];
  buildCommand: string | null;
  buildDirectory: string;
  extractionMethod: string;
  outputs: GeneratorOutput[];
  enabled: boolean;
  notes?: string;
}

interface Config {
  description: string;
  generators: Record<string, GeneratorConfig>;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const CONFIG_PATH = path.join(__dirname, 'config.json');
const DOCS_TSX_PATH = path.join(ROOT_DIR, 'docs', 'node_modules', 'tsx', 'dist', 'cli.mjs');
const SHARED_GENERATOR_FILES = new Set([
  'scripts/docs-generators/runner.ts',
  // Shared rendering helpers every generator uses.
  'scripts/docs-generators/lib/mdx.ts',
  'scripts/docs-generators/lib/cli-mdx.ts',
  'scripts/docs-generators/lib/mcp-mdx.ts',
  'scripts/docs-generators/lib/headers.ts',
  'scripts/docs-generators/prose-style.ts',
  'scripts/docs-generators/config.json',
  '.github/workflows/ci-check-docs.yml',
  '.gitattributes',
  'docs/package.json',
  'docs/pnpm-lock.yaml',
]);

// ============================================================================
// Main
// ============================================================================

async function main() {
  const { values } = parseArgs({
    options: {
      help: { type: 'boolean' },
      list: { type: 'boolean' },
      library: { type: 'string' },
      check: { type: 'boolean' },
      'check-only': { type: 'boolean' },
      changed: { type: 'boolean' },
      'changed-files-file': { type: 'string' },
      'test-routing': { type: 'boolean' },
    },
  });
  if (values.help) {
    console.log(`Usage: pnpm --dir docs docs:generate [options]

  --help                       Show this help without running generators
  --list                       List configured generators
  --library <name>             Run one configured generator
  --check, --check-only         Check for documentation drift
  --changed                    Select generators from changed files
  --changed-files-file <path>   Print generators selected by a changed-file list
  --test-routing               Verify generator routing`);
    return;
  }

  const checkOnly = Boolean(values.check || values['check-only']);
  const changedFilesFile = values['changed-files-file'];
  const specificLibrary = values.library;

  // Load config
  if (!fs.existsSync(CONFIG_PATH)) {
    console.error(`❌ Config file not found: ${CONFIG_PATH}`);
    process.exit(1);
  }

  const config: Config = JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf-8'));

  if (values['test-routing']) {
    testGeneratorRouting(config);
    return;
  }

  if (changedFilesFile !== undefined) {
    if (!fs.existsSync(changedFilesFile)) {
      console.error(`Changed-files input not found: ${changedFilesFile}`);
      process.exit(1);
    }
    const changedFiles = fs.readFileSync(changedFilesFile, 'utf-8').split(/\r?\n/).filter(Boolean);
    console.log(selectGenerators(config, changedFiles).join(' '));
    return;
  }

  console.log('📚 Documentation Generator Runner');
  console.log('==================================\n');

  // List mode
  if (values.list) {
    listGenerators(config);
    return;
  }

  // Determine which generators to run
  let generatorsToRun: string[] = [];

  if (specificLibrary !== undefined) {
    if (!config.generators[specificLibrary]) {
      console.error(`❌ Unknown library: ${specificLibrary}`);
      console.log('\nAvailable libraries:', Object.keys(config.generators).join(', '));
      process.exit(1);
    }
    generatorsToRun = [specificLibrary];
  } else if (values.changed) {
    generatorsToRun = getChangedGenerators(config);
    if (generatorsToRun.length === 0) {
      console.log('✅ No documentation-related changes detected.');
      return;
    }
  } else {
    // Run all enabled generators
    generatorsToRun = Object.entries(config.generators)
      .filter(([_, cfg]) => cfg.enabled)
      .map(([key, _]) => key);
  }

  if (generatorsToRun.length === 0) {
    console.log('⚠️  No generators to run. Enable generators in config.json.');
    return;
  }

  console.log(`📋 Generators to run: ${generatorsToRun.join(', ')}\n`);

  // Run generators
  let hasErrors = false;

  for (const generatorKey of generatorsToRun) {
    const generator = config.generators[generatorKey];

    console.log(`\n${'='.repeat(50)}`);
    console.log(`📦 ${generator.name} (${generatorKey})`);
    console.log(`${'='.repeat(50)}\n`);

    if (!generator.enabled) {
      console.log(`⏭️  Skipped (disabled)`);
      if (generator.notes) {
        console.log(`   Note: ${generator.notes}`);
      }
      continue;
    }

    const success = await runGenerator(generator, generatorKey, checkOnly);
    if (!success) {
      hasErrors = true;
    }
  }

  // Summary
  console.log(`\n${'='.repeat(50)}`);
  if (hasErrors) {
    console.error('❌ Some generators failed or detected drift.');
    if (checkOnly) {
      console.log("\n💡 Run 'pnpm --dir docs docs:generate' to update documentation");
    }
    process.exit(1);
  } else {
    console.log('✅ All documentation is up to date!');
  }
}

// ============================================================================
// Generator Execution
// ============================================================================

async function runGenerator(
  generator: GeneratorConfig,
  key: string,
  checkOnly: boolean
): Promise<boolean> {
  const generatorPath = path.join(ROOT_DIR, generator.generatorScript);

  // Check if generator script exists
  if (!fs.existsSync(generatorPath)) {
    console.log(`⚠️  Generator script not found: ${generator.generatorScript}`);
    console.log(`   Create this file to enable documentation generation.`);
    return true; // Not an error, just not implemented
  }

  try {
    // Use the docs app's lockfile-installed tsx; never fall back to npx downloads.
    requireDocsTsx();
    const args = checkOnly ? ['--check'] : [];
    const result = spawnSync(process.execPath, [DOCS_TSX_PATH, generatorPath, ...args], {
      cwd: ROOT_DIR,
      stdio: 'inherit',
      encoding: 'utf-8',
    });

    return result.status === 0;
  } catch (error) {
    console.error(`❌ Error running generator for ${key}:`, error);
    return false;
  }
}

// ============================================================================
// Changed Files Detection (for CI)
// ============================================================================

function requireDocsTsx(): string {
  if (!fs.existsSync(DOCS_TSX_PATH)) {
    throw new Error(
      `Pinned tsx executable not found at ${DOCS_TSX_PATH}. Run pnpm --dir docs install --frozen-lockfile.`
    );
  }

  const packagePath = path.join(ROOT_DIR, 'docs', 'node_modules', 'tsx', 'package.json');
  const lockfilePath = path.join(ROOT_DIR, 'docs', 'pnpm-lock.yaml');
  const installedVersion = JSON.parse(fs.readFileSync(packagePath, 'utf-8')).version as string;
  const lockfile = fs.readFileSync(lockfilePath, 'utf-8');

  if (!lockfile.includes(`tsx@${installedVersion}:`)) {
    throw new Error(`Installed tsx ${installedVersion} is not pinned by docs/pnpm-lock.yaml`);
  }

  return installedVersion;
}

function globToRegExp(glob: string): RegExp {
  let pattern = '';

  for (let index = 0; index < glob.length; index += 1) {
    const character = glob[index];

    if (character === '*' && glob[index + 1] === '*') {
      if (glob[index + 2] === '/') {
        pattern += '(?:.*/)?';
        index += 2;
      } else {
        pattern += '.*';
        index += 1;
      }
    } else if (character === '*') {
      pattern += '[^/]*';
    } else {
      pattern += character.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    }
  }

  return new RegExp(`^${pattern}$`);
}

function selectGenerators(config: Config, changedFiles: readonly string[]): string[] {
  const enabledGenerators = Object.entries(config.generators).filter(([_, cfg]) => cfg.enabled);
  const normalizedFiles = changedFiles.map((file) => file.replace(/\\/g, '/'));

  if (normalizedFiles.some((file) => SHARED_GENERATOR_FILES.has(file))) {
    return enabledGenerators.map(([key]) => key);
  }

  return enabledGenerators.flatMap(([key, generator]) => {
    const ownedFiles = new Set([
      generator.generatorScript,
      ...generator.outputs.map((output) =>
        path.posix.join(generator.docsOutputPath, output.outputFile)
      ),
    ]);
    const watchPatterns = generator.watchPaths.map(globToRegExp);
    const selected = normalizedFiles.some(
      (file) =>
        file.startsWith(`${generator.sourcePath}/`) ||
        ownedFiles.has(file) ||
        watchPatterns.some((pattern) => pattern.test(file))
    );

    return selected ? [key] : [];
  });
}

function assertSelection(
  config: Config,
  changedFiles: readonly string[],
  expected: readonly string[]
): void {
  const actual = selectGenerators(config, changedFiles);
  if (actual.join('\n') !== expected.join('\n')) {
    throw new Error(
      `Routing assertion failed for ${changedFiles.join(', ')}: expected ${expected.join(', ')}, got ${actual.join(', ')}`
    );
  }
}

function testGeneratorRouting(config: Config): void {
  const tsxVersion = requireDocsTsx();

  assertSelection(config, ['docs/content/docs/cua-driver/guides/connect-your-agent.mdx'], []);
  assertSelection(
    config,
    [
      'libs/cua-driver/rust/crates/cua-driver/src/main.rs',
      'docs/content/docs/cua-driver/reference/cli/daemon.mdx',
      'scripts/docs-generators/cua-driver.ts',
    ],
    ['cua-driver']
  );
  // Every platform's registry snapshot and every generated page routes to it.
  for (const file of [
    'scripts/docs-generators/cli-specs/cua-driver-mcp-windows.json',
    'scripts/docs-generators/headers/cua-driver/mcp-tools.md',
    'docs/content/docs/cua-driver/reference/mcp-tools/click.mdx',
    'docs/content/docs/cua-driver/reference/index.mdx',
  ]) {
    assertSelection(config, [file], ['cua-driver']);
  }
  assertSelection(
    config,
    [
      'libs/cua-bench/cua_bench/cli/commands/run.py',
      'libs/cua-bench/cua_bench/results.py',
      'docs/content/docs/cua-bench/reference/cli/run.mdx',
      'scripts/docs-generators/extract_cb_docs.py',
    ],
    ['cua-bench']
  );
  assertSelection(config, ['docs/content/docs/cua-bench/guides/benchmarks.mdx'], []);
  assertSelection(
    config,
    [
      'libs/lume/src/Commands/List.swift',
      'docs/content/docs/lume/reference/http-api.mdx',
      'scripts/docs-generators/lume.ts',
    ],
    ['lume']
  );
  assertSelection(
    config,
    [
      'libs/python/cua-sandbox/cua_sandbox/image.py',
      'docs/content/docs/cua-sdk/reference/os-image-catalog.mdx',
      'scripts/docs-generators/sandbox-facts.json',
    ],
    ['sandbox', 'cua-sdk-python']
  );
  assertSelection(
    config,
    [
      'libs/cua/crates/cua-cli/src/main.rs',
      'docs/content/docs/cua-cli/reference/mcp-tools/computer.mdx',
      'scripts/docs-generators/cua-cli.ts',
    ],
    ['cua-cli']
  );
  assertSelection(config, ['libs/cua/spaces-contract/manifest.json'], ['cua-cli', 'spaces']);
  assertSelection(
    config,
    ['libs/cua/crates/cua-spaces-contract/src/lib.rs', 'docs/content/docs/spaces/reference/agents.mdx', 'scripts/docs-generators/spaces.ts'],
    ['cua-cli', 'spaces']
  );
  assertSelection(
    config,
    ['libs/fleet/backend/docs/swagger.json', 'docs/content/docs/fleets/reference/pool.mdx', 'scripts/docs-generators/fleet.ts'],
    ['fleet']
  );
  assertSelection(config, ['libs/cua/crates/cua-fleet/src/lib.rs'], ['cua-rust', 'fleet']);
  assertSelection(config, ['docs/content/docs/cua-cli/guides/mcp-server.mdx'], []);
  assertSelection(config, ['docs/content/docs/lume/reference/cli/vms.mdx'], ['lume']);
  assertSelection(
    config,
    [
      'libs/cua/crates/cua-sdk/src/native/sandbox.rs',
      'libs/cua/crates/cua-bindgen/src/docs.rs',
      'docs/content/docs/cua-sdk/reference/sandbox/index.mdx',
    ],
    ['cua-sdk', 'cua-rust']
  );
  assertSelection(config, ['libs/cua/crates/cua-sdk/uniffi.toml'], ['cua-sdk', 'cua-rust']);
  // The reference index is the object generator's; the Rust pages read its anchors.
  assertSelection(config, ['docs/content/docs/cua-sdk/reference/index.mdx'], ['cua-sdk', 'cua-rust']);
  // Curated headers and tested examples route to the generator that embeds them.
  assertSelection(config, ['scripts/docs-generators/examples/cua-sdk/Sandboxes.create.py'], ['cua-sdk']);
  assertSelection(config, ['scripts/docs-generators/headers/cua-sdk/python/index.md'], ['cua-sdk-python']);
  assertSelection(
    config,
    [
      'libs/cua/python/src/cua/images.py',
      'docs/content/docs/cua-sdk/reference/python/index.mdx',
      'docs/content/docs/cua-sdk/reference/python/interfaces.mdx',
      'scripts/docs-generators/extract_python_docs.py',
    ],
    ['cua-sdk-python']
  );
  assertSelection(
    config,
    [
      'libs/cua/typescript/src/spaces/thread.ts',
      'libs/cua/typescript/package-lock.json',
      'docs/content/docs/cua-sdk/reference/typescript/spaces.mdx',
    ],
    ['cua-sdk-ts']
  );
  assertSelection(
    config,
    [
      'libs/cua/crates/cua-spaces/src/client/model.rs',
      'scripts/docs-generators/rustdoc-json/src/main.rs',
      'docs/content/docs/cua-sdk/reference/rust/cua-fleet.mdx',
    ],
    ['cua-rust']
  );
  // The Rust pages link UniFFI-exported items to the object pages.
  assertSelection(config, ['docs/content/docs/cua-sdk/reference/spacesd/files.mdx'], ['cua-sdk', 'cua-rust']);
  // Unrelated crates do not trigger it.
  assertSelection(config, ['libs/cua/crates/cua-vmm/src/lib.rs'], []);
  assertSelection(
    config,
    [
      'libs/cua/proto/cua/env/v1/process.proto',
      'libs/cua/crates/cua-proto/src/lib.rs',
      'docs/content/docs/cua-sdk/reference/protocol/env-process.mdx',
    ],
    ['cua-proto']
  );
  assertSelection(config, ['scripts/docs-generators/headers/cua-sdk/protocol/index.md'], ['cua-proto']);
  // A shared file routes to every enabled generator.
  const enabled = Object.entries(config.generators)
    .filter(([, generator]) => generator.enabled)
    .map(([key]) => key);
  assertSelection(config, ['scripts/docs-generators/runner.ts'], enabled);
  assertSelection(config, ['scripts/docs-generators/config.json'], enabled);
  assertSelection(config, ['scripts/docs-generators/lib/mdx.ts'], enabled);
  assertSelection(config, ['scripts/docs-generators/lib/cli-mdx.ts'], enabled);
  assertSelection(config, ['scripts/docs-generators/lib/mcp-mdx.ts'], enabled);

  console.log(`Generator routing assertions passed with pinned tsx ${tsxVersion}`);
}

function getChangedGenerators(config: Config): string[] {
  try {
    // Get changed files from git
    // This works for both PRs (comparing to base) and pushes
    const gitDiff = execSync(
      'git diff --name-only HEAD~1 HEAD 2>/dev/null || git diff --name-only HEAD',
      { cwd: ROOT_DIR, encoding: 'utf-8' }
    );

    const changedFiles = gitDiff.split('\n').filter(Boolean);

    console.log(`📝 Changed files: ${changedFiles.length}`);

    const changedGenerators = selectGenerators(config, changedFiles);
    for (const key of changedGenerators) console.log(`   📌 ${key}: changes detected`);
    return changedGenerators;
  } catch (error) {
    console.warn('⚠️  Could not detect changed files, running all generators');
    return Object.entries(config.generators)
      .filter(([_, cfg]) => cfg.enabled)
      .map(([key, _]) => key);
  }
}

// ============================================================================
// List Generators
// ============================================================================

function listGenerators(config: Config): void {
  console.log('Configured Documentation Generators:\n');

  const maxKeyLen = Math.max(...Object.keys(config.generators).map((k) => k.length));

  for (const [key, generator] of Object.entries(config.generators)) {
    const status = generator.enabled ? '✅' : '⏸️ ';
    const paddedKey = key.padEnd(maxKeyLen);
    console.log(`${status} ${paddedKey}  ${generator.name} (${generator.language})`);

    if (!generator.enabled && generator.notes) {
      console.log(`   ${''.padEnd(maxKeyLen)}  └─ ${generator.notes}`);
    }
  }

  console.log('\n');
  console.log('Legend:');
  console.log('  ✅ = Enabled (will run)');
  console.log('  ⏸️  = Disabled (not yet implemented)');
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
