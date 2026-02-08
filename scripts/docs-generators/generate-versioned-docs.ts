#!/usr/bin/env npx tsx

/**
 * Versioned Documentation Generator
 *
 * Generates API documentation for historical versions by checking out
 * tagged versions from git and running the extraction.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/generate-versioned-docs.ts           # Generate all
 *   npx tsx scripts/docs-generators/generate-versioned-docs.ts --sdk=computer  # Specific SDK
 *   npx tsx scripts/docs-generators/generate-versioned-docs.ts --list    # List available versions
 */

import { execSync, spawnSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

// ============================================================================
// Types
// ============================================================================

type ExtractionType = 'python-griffe' | 'swift-dump-docs';

interface SDKConfig {
  name: string;
  displayName: string;
  tagPrefix: string;
  packageDir: string;
  packageName: string;
  outputDir: string;
  extractionType: ExtractionType;
  /** Skip versions below this major.minor (e.g. '0.2' skips 0.1.x) */
  minVersion?: string;
  /** Base docs directory (defaults to cua/reference under DOCS_BASE) */
  docsBasePath?: string;
}

interface VersionInfo {
  tag: string;
  version: string;
  majorMinor: string;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const DOCS_BASE = path.join(ROOT_DIR, 'docs/content/docs');
const DOCS_DIR = path.join(DOCS_BASE, 'cua/reference');
const PYTHON_SCRIPT = path.join(__dirname, 'extract_python_docs.py');

const SDK_CONFIGS: SDKConfig[] = [
  {
    name: 'computer',
    displayName: 'Computer SDK',
    tagPrefix: 'computer-v',
    packageDir: 'libs/python/computer/computer',
    packageName: 'computer',
    outputDir: 'computer-sdk',
    extractionType: 'python-griffe',
  },
  {
    name: 'agent',
    displayName: 'Agent SDK',
    tagPrefix: 'agent-v',
    packageDir: 'libs/python/agent/agent',
    packageName: 'agent',
    outputDir: 'agent-sdk',
    extractionType: 'python-griffe',
  },
  {
    name: 'bench',
    displayName: 'Cua Bench',
    tagPrefix: 'bench-v',
    packageDir: 'libs/cua-bench/cua_bench',
    packageName: 'cua_bench',
    outputDir: 'reference',
    extractionType: 'python-griffe',
    docsBasePath: 'cuabench/reference',
  },
  {
    name: 'lume',
    displayName: 'Lume',
    tagPrefix: 'lume-v',
    packageDir: 'libs/lume',
    packageName: 'lume',
    outputDir: 'lume/reference',
    extractionType: 'swift-dump-docs',
    minVersion: '0.2',
    docsBasePath: 'lume/reference',
  },
];

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const listOnly = args.includes('--list');
  const sdkArg = args.find((a) => a.startsWith('--sdk='));
  const targetSdk = sdkArg?.split('=')[1];

  console.log('📚 Versioned Documentation Generator');
  console.log('====================================\n');

  // Check for uncommitted changes
  const status = execSync('git status --porcelain', { encoding: 'utf-8', cwd: ROOT_DIR });
  if (status.trim()) {
    console.warn('⚠️  Warning: You have uncommitted changes. They will be preserved.\n');
  }

  for (const config of SDK_CONFIGS) {
    if (targetSdk && targetSdk !== config.name) {
      continue;
    }

    console.log(`\n📦 ${config.displayName}`);
    console.log('─'.repeat(40));

    // Get all version tags
    const versions = getVersions(config.tagPrefix);
    console.log(`   Found ${versions.length} version tags`);

    if (listOnly) {
      // Just list versions
      const grouped = groupByMajorMinor(versions);
      for (const [majorMinor, versionList] of Object.entries(grouped)) {
        console.log(`   v${majorMinor}.x: ${versionList.map((v) => v.version).join(', ')}`);
      }
      continue;
    }

    // Group by major.minor (we only generate one doc per major.minor)
    const grouped = groupByMajorMinor(versions);

    for (const [majorMinor, versionList] of Object.entries(grouped)) {
      // Skip versions below minVersion
      if (config.minVersion && compareVersions(majorMinor, config.minVersion) < 0) {
        console.log(`   Skipping v${majorMinor} (below minVersion ${config.minVersion})`);
        continue;
      }

      // Use the latest patch version for this major.minor
      const latestVersion = versionList[0];
      console.log(`\n   Generating v${majorMinor} (from ${latestVersion.tag})...`);

      try {
        // Generate docs for this version
        await generateVersionDocs(config, latestVersion, majorMinor);
        const files =
          config.extractionType === 'swift-dump-docs'
            ? 'cli-reference.mdx + http-api.mdx'
            : 'api.mdx';
        console.log(`   ✅ Generated v${majorMinor}/${files}`);
      } catch (error) {
        console.error(`   ❌ Failed: ${error}`);
      }
    }
  }

  console.log('\n✅ Versioned documentation generation complete!');
}

// ============================================================================
// Version Discovery
// ============================================================================

function getVersions(tagPrefix: string): VersionInfo[] {
  try {
    const output = execSync(`git tag | grep "^${tagPrefix}"`, {
      encoding: 'utf-8',
      cwd: ROOT_DIR,
    });

    return output
      .trim()
      .split('\n')
      .filter(Boolean)
      .map((tag) => {
        const version = tag.replace(tagPrefix, '');
        const parts = version.split('.');
        const majorMinor = `${parts[0]}.${parts[1] || '0'}`;
        return { tag, version, majorMinor };
      })
      .sort((a, b) => compareVersions(b.version, a.version)); // Descending
  } catch {
    return [];
  }
}

function groupByMajorMinor(versions: VersionInfo[]): Record<string, VersionInfo[]> {
  const grouped: Record<string, VersionInfo[]> = {};

  for (const v of versions) {
    if (!grouped[v.majorMinor]) {
      grouped[v.majorMinor] = [];
    }
    grouped[v.majorMinor].push(v);
  }

  // Sort keys descending
  const sortedKeys = Object.keys(grouped).sort((a, b) => compareVersions(b, a));
  const sorted: Record<string, VersionInfo[]> = {};
  for (const key of sortedKeys) {
    sorted[key] = grouped[key];
  }

  return sorted;
}

function compareVersions(a: string, b: string): number {
  const partsA = a.split('.').map((x) => parseInt(x, 10) || 0);
  const partsB = b.split('.').map((x) => parseInt(x, 10) || 0);

  for (let i = 0; i < Math.max(partsA.length, partsB.length); i++) {
    const partA = partsA[i] || 0;
    const partB = partsB[i] || 0;
    if (partA !== partB) {
      return partA - partB;
    }
  }
  return 0;
}

// ============================================================================
// Documentation Generation
// ============================================================================

async function generateVersionDocs(
  config: SDKConfig,
  versionInfo: VersionInfo,
  majorMinor: string
): Promise<void> {
  // Resolve output directory based on config
  const baseDir = config.docsBasePath
    ? path.join(DOCS_BASE, config.docsBasePath)
    : path.join(DOCS_DIR, config.outputDir);
  const outputDir = path.join(baseDir, `v${majorMinor}`);

  // Create output directory
  if (!fs.existsSync(outputDir)) {
    fs.mkdirSync(outputDir, { recursive: true });
  }

  // Checkout the package directory at the tagged version
  const packagePath = path.join(ROOT_DIR, config.packageDir);

  try {
    // Save current state
    execSync(`git stash push -m "versioned-docs-temp" -- ${config.packageDir}`, {
      cwd: ROOT_DIR,
      stdio: 'pipe',
    });
  } catch {
    // No changes to stash, continue
  }

  try {
    // Checkout tagged version of the package
    execSync(`git checkout ${versionInfo.tag} -- ${config.packageDir}`, {
      cwd: ROOT_DIR,
      stdio: 'pipe',
    });

    if (config.extractionType === 'swift-dump-docs') {
      await generateSwiftVersionDocs(config, versionInfo, majorMinor, outputDir);
    } else {
      await generatePythonVersionDocs(config, versionInfo, majorMinor, outputDir);
    }
  } finally {
    // Restore HEAD version
    execSync(`git checkout HEAD -- ${config.packageDir}`, {
      cwd: ROOT_DIR,
      stdio: 'pipe',
    });

    // Restore stashed changes if any
    try {
      execSync('git stash pop', { cwd: ROOT_DIR, stdio: 'pipe' });
    } catch {
      // No stash to pop
    }
  }
}

async function generatePythonVersionDocs(
  config: SDKConfig,
  versionInfo: VersionInfo,
  majorMinor: string,
  outputDir: string
): Promise<void> {
  const packagePath = path.join(ROOT_DIR, config.packageDir);
  const outputPath = path.join(outputDir, 'api.mdx');

  // Run extraction
  const extractOutput = execSync(
    `python3 "${PYTHON_SCRIPT}" "${packagePath}" "${config.packageName}"`,
    { encoding: 'utf-8', cwd: ROOT_DIR, maxBuffer: 10 * 1024 * 1024 }
  );

  const docs = JSON.parse(extractOutput);

  // Generate MDX
  const mdx = generateMDX(docs, config, versionInfo, majorMinor);

  // Write output
  fs.writeFileSync(outputPath, mdx);

  // Create meta.json for the version folder
  const metaPath = path.join(outputDir, 'meta.json');
  fs.writeFileSync(
    metaPath,
    JSON.stringify(
      {
        title: `v${majorMinor}`,
        description: `${config.displayName} v${majorMinor} API Reference`,
        pages: ['api'],
      },
      null,
      2
    )
  );
}

async function generateSwiftVersionDocs(
  config: SDKConfig,
  versionInfo: VersionInfo,
  majorMinor: string,
  outputDir: string
): Promise<void> {
  const lumeDir = path.join(ROOT_DIR, config.packageDir);

  // Build Lume at this version
  try {
    execSync('swift build -c release', {
      cwd: lumeDir,
      stdio: 'pipe',
      timeout: 300000, // 5 min build timeout
    });
  } catch (error) {
    throw new Error(`Swift build failed for ${versionInfo.tag}: ${error}`);
  }

  // Import Lume MDX generators
  const { generateCLIReferenceMDX, generateHTTPAPIMDX } = await import('./lume');

  // Extract CLI docs
  let cliMdx: string;
  try {
    const cliDocsJson = execSync('.build/release/lume dump-docs --type cli', {
      cwd: lumeDir,
      encoding: 'utf-8',
    });
    const cliDocs = JSON.parse(cliDocsJson);
    cliMdx = generateVersionedLumeMDX(
      generateCLIReferenceMDX(cliDocs),
      config,
      versionInfo,
      majorMinor,
      'CLI Reference'
    );
  } catch (error) {
    throw new Error(`CLI docs extraction failed for ${versionInfo.tag}: ${error}`);
  }

  // Extract API docs
  let apiMdx: string;
  try {
    const apiDocsJson = execSync('.build/release/lume dump-docs --type api', {
      cwd: lumeDir,
      encoding: 'utf-8',
    });
    const apiDocs = JSON.parse(apiDocsJson);
    apiMdx = generateVersionedLumeMDX(
      generateHTTPAPIMDX(apiDocs),
      config,
      versionInfo,
      majorMinor,
      'HTTP API Reference'
    );
  } catch (error) {
    throw new Error(`API docs extraction failed for ${versionInfo.tag}: ${error}`);
  }

  // Write outputs
  fs.writeFileSync(path.join(outputDir, 'cli-reference.mdx'), cliMdx);
  fs.writeFileSync(path.join(outputDir, 'http-api.mdx'), apiMdx);

  // Create meta.json
  fs.writeFileSync(
    path.join(outputDir, 'meta.json'),
    JSON.stringify(
      {
        title: `v${majorMinor}`,
        description: `${config.displayName} v${majorMinor} Reference`,
        pages: ['cli-reference', 'http-api'],
      },
      null,
      2
    )
  );
}

/**
 * Wrap generated Lume MDX with a version warning callout for historical versions.
 * Replaces the VersionHeader (which shows current version) with an old-version notice.
 */
function generateVersionedLumeMDX(
  currentMdx: string,
  config: SDKConfig,
  versionInfo: VersionInfo,
  majorMinor: string,
  title: string
): string {
  // Replace the VersionHeader block with an old-version callout
  const lines = currentMdx.split('\n');
  const result: string[] = [];
  let skipVersionHeader = false;

  for (const line of lines) {
    // Skip the VersionHeader block
    if (line.startsWith('<VersionHeader')) {
      skipVersionHeader = true;
      // Insert old-version callout instead
      result.push('<Callout type="warn">');
      result.push(
        `  This is documentation for **v${majorMinor}**. [View latest version](/lume/reference/cli-reference).`
      );
      result.push('</Callout>');
      result.push('');
      result.push('<div className="flex items-center gap-2 mb-6">');
      result.push(
        `  <span className="px-2 py-1 bg-amber-100 dark:bg-amber-900 text-amber-800 dark:text-amber-200 rounded text-sm font-mono">v${versionInfo.version}</span>`
      );
      result.push(
        `  <span className="text-sm text-fd-muted-foreground">curl -fsSL .../install.sh | bash</span>`
      );
      result.push('</div>');
      result.push('');
      continue;
    }
    if (skipVersionHeader) {
      if (line.startsWith('/>')) {
        skipVersionHeader = false;
        continue;
      }
      continue;
    }
    result.push(line);
  }

  return result.join('\n');
}

// ============================================================================
// MDX Generation (simplified version for historical docs)
// ============================================================================

function generateMDX(
  docs: any,
  config: SDKConfig,
  versionInfo: VersionInfo,
  majorMinor: string
): string {
  const lines: string[] = [];

  // Frontmatter
  lines.push('---');
  lines.push(`title: ${config.displayName} v${majorMinor} API Reference`);
  lines.push(`description: API reference for ${config.displayName} version ${majorMinor}`);
  lines.push('---');
  lines.push('');

  // Auto-generated notice
  lines.push(`{/*`);
  lines.push(`  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY`);
  lines.push(`  Generated by: npx tsx scripts/docs-generators/generate-versioned-docs.ts`);
  lines.push(`  Source tag: ${versionInfo.tag}`);
  lines.push(`  Version: ${versionInfo.version}`);
  lines.push(`*/}`);
  lines.push('');

  // Imports
  lines.push("import { Callout } from 'fumadocs-ui/components/callout';");
  lines.push('');

  // Version notice — link to the folder root (index.mdx is the landing page)
  const latestHref = config.docsBasePath
    ? `/${config.docsBasePath.replace(/\/$/, '')}/${config.outputDir}`
    : `/cua/reference/${config.outputDir}`;
  lines.push('<Callout type="warn">');
  lines.push(
    `  This is documentation for **v${majorMinor}**. [View latest version](${latestHref}).`
  );
  lines.push('</Callout>');
  lines.push('');

  // Version badge
  const pipName = config.packageName.replace(/_/g, '-');
  const fullPipName = pipName.startsWith('cua') ? pipName : `cua-${pipName}`;
  lines.push('<div className="flex items-center gap-2 mb-6">');
  lines.push(
    `  <span className="px-2 py-1 bg-amber-100 dark:bg-amber-900 text-amber-800 dark:text-amber-200 rounded text-sm font-mono">v${versionInfo.version}</span>`
  );
  lines.push(
    `  <span className="text-sm text-fd-muted-foreground">pip install ${fullPipName}==${versionInfo.version}</span>`
  );
  lines.push('</div>');
  lines.push('');

  // Package description
  if (docs.docstring) {
    lines.push(escapeMDX(docs.docstring));
    lines.push('');
  }

  // Classes
  if (docs.classes && docs.classes.length > 0) {
    lines.push('## Classes');
    lines.push('');
    lines.push('| Class | Description |');
    lines.push('|-------|-------------|');
    for (const cls of docs.classes) {
      if (!cls.is_private) {
        const desc = escapeMDX(cls.description?.split('\n')[0]) || 'No description';
        lines.push(`| \`${cls.name}\` | ${desc} |`);
      }
    }
    lines.push('');

    // Class details
    for (const cls of docs.classes) {
      if (!cls.is_private) {
        lines.push(`## ${cls.name}`);
        lines.push('');
        if (cls.description) {
          lines.push(escapeMDX(cls.description));
          lines.push('');
        }

        // Methods
        const publicMethods = (cls.methods || []).filter(
          (m: any) => !m.is_private && !m.is_dunder && m.name !== '__init__'
        );
        if (publicMethods.length > 0) {
          lines.push('### Methods');
          lines.push('');
          for (const method of publicMethods) {
            lines.push(`#### ${cls.name}.${method.name}`);
            lines.push('');
            lines.push('```python');
            lines.push(method.signature || `def ${method.name}(...)`);
            lines.push('```');
            lines.push('');
            if (method.description) {
              lines.push(escapeMDX(method.description));
              lines.push('');
            }
          }
        }
      }
    }
  }

  return lines.join('\n');
}

function escapeMDX(text: string): string {
  if (!text) return text;
  return text.replace(/\{/g, '\\{').replace(/\}/g, '\\}');
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
