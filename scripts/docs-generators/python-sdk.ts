#!/usr/bin/env npx tsx

/**
 * Python SDK Documentation Generator
 *
 * Generates MDX API reference documentation from Python source code docstrings.
 * Uses griffe to extract documentation without importing packages.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/python-sdk.ts                    # Generate all
 *   npx tsx scripts/docs-generators/python-sdk.ts --sdk=computer     # Generate specific SDK
 *   npx tsx scripts/docs-generators/python-sdk.ts --check            # Check for drift (CI mode)
 */

import { execSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

// ============================================================================
// Types
// ============================================================================

interface PythonPackage {
  name: string;
  version: string;
  docstring: string;
  exports: string[] | null;
  classes: ClassDoc[];
  functions: FunctionDoc[];
  submodules: ModuleDoc[];
  error?: string;
}

interface ModuleDoc {
  name: string;
  version: string;
  docstring: string;
  exports: string[] | null;
  classes: ClassDoc[];
  functions: FunctionDoc[];
}

interface ClassDoc {
  name: string;
  description: string;
  bases: string[];
  methods: FunctionDoc[];
  attributes: AttributeDoc[];
  is_private: boolean;
}

interface FunctionDoc {
  name: string;
  signature: string;
  is_async: boolean;
  is_method: boolean;
  description: string;
  parameters: ParameterDoc[];
  returns: ReturnDoc | null;
  raises: RaiseDoc[];
  examples: string[];
  is_private: boolean;
  is_dunder: boolean;
}

interface ParameterDoc {
  name: string;
  type: string;
  description: string;
  default: string | null;
}

interface ReturnDoc {
  type: string;
  description: string;
}

interface RaiseDoc {
  type: string;
  description: string;
}

interface AttributeDoc {
  name: string;
  type: string;
  description: string;
  default: string | null;
  is_private: boolean;
}

interface SDKConfig {
  packageDir: string;
  packageName: string;
  outputPath: string;
  displayName: string;
  description: string;
  outputDir: string; // For version discovery (relative to docsBaseDir)
  tagPrefix: string; // Git tag prefix for version discovery
  /** Absolute docs dir for this SDK (defaults to docs/content/docs/cua/reference) */
  docsBaseDir?: string;
  /** URL base path for hrefs (defaults to /cua/reference) */
  hrefBase?: string;
  /** Submodules to include in docs (if set, only these are included; if unset, all are included) */
  includeSubmodules?: string[];
  /** Override the page title (defaults to "${displayName} API Reference") */
  pageTitle?: string;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const PYTHON_SCRIPT = path.join(__dirname, 'extract_python_docs.py');

const SDK_CONFIGS: Record<string, SDKConfig> = {
  computer: {
    packageDir: 'libs/python/computer/computer',
    packageName: 'computer',
    outputPath: 'docs/content/docs/cua/reference/computer-sdk/index.mdx',
    displayName: 'Computer SDK',
    description: 'Python API reference for controlling virtual machines and computer interfaces',
    outputDir: 'computer-sdk',
    tagPrefix: 'computer-v',
    includeSubmodules: ['interface', 'models', 'tracing', 'helpers', 'diorama_computer'],
  },
  agent: {
    packageDir: 'libs/python/agent/agent',
    packageName: 'agent',
    outputPath: 'docs/content/docs/cua/reference/agent-sdk/index.mdx',
    displayName: 'Agent SDK',
    description: 'Python API reference for building computer-use agents',
    outputDir: 'agent-sdk',
    tagPrefix: 'agent-v',
    includeSubmodules: ['callbacks', 'tools', 'types'],
  },
  cli: {
    packageDir: 'libs/python/cua-cli/cua_cli',
    packageName: 'cua_cli',
    outputPath: 'docs/content/docs/cua/reference/cli/index.mdx',
    displayName: 'Cua CLI',
    description: 'Python API reference for the Cua command-line interface',
    outputDir: 'cli',
    tagPrefix: 'cli-v',
  },
  bench: {
    packageDir: 'libs/cua-bench/cua_bench',
    packageName: 'cua_bench',
    outputPath: 'docs/content/docs/cuabench/reference/api.mdx',
    displayName: 'Cua Bench',
    description: 'Python API reference for the desktop automation benchmarking framework',
    outputDir: 'reference',
    tagPrefix: 'bench-v',
    docsBaseDir: 'docs/content/docs/cuabench',
    hrefBase: '/cuabench',
    pageTitle: 'API Reference',
  },
};

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const checkOnly = args.includes('--check') || args.includes('--check-only');
  const sdkArg = args.find((a) => a.startsWith('--sdk='));
  const targetSdk = sdkArg?.split('=')[1];

  console.log('🐍 Python SDK Documentation Generator');
  console.log('=====================================\n');

  // Check if Python script exists
  if (!fs.existsSync(PYTHON_SCRIPT)) {
    console.error(`❌ Python extraction script not found: ${PYTHON_SCRIPT}`);
    process.exit(1);
  }

  let hasErrors = false;

  for (const [sdkName, config] of Object.entries(SDK_CONFIGS)) {
    // Skip if targeting specific SDK
    if (targetSdk && targetSdk !== sdkName) {
      continue;
    }

    console.log(`📖 Processing ${config.displayName}...`);

    // Check if package exists
    const packagePath = path.join(ROOT_DIR, config.packageDir);
    if (!fs.existsSync(packagePath)) {
      console.error(`   ❌ Package not found: ${config.packageDir}`);
      hasErrors = true;
      continue;
    }

    // Extract documentation using Python script
    console.log(`   Extracting documentation from ${config.packageDir}...`);
    let docs: PythonPackage;
    try {
      // Prefer uv run --with griffe python (works cross-platform), fall back to python3
      const pythonCmd =
        process.platform === 'win32'
          ? `uv run --with griffe python`
          : `python3`;
      const output = execSync(
        `${pythonCmd} "${PYTHON_SCRIPT}" "${packagePath}" "${config.packageName}"`,
        {
          encoding: 'utf-8',
          cwd: ROOT_DIR,
          timeout: 60000,
        }
      );
      docs = JSON.parse(output);
    } catch (error) {
      console.error(`   ❌ Failed to extract documentation: ${error}`);
      hasErrors = true;
      continue;
    }

    if (docs.error) {
      console.error(`   ❌ Extraction error: ${docs.error}`);
      hasErrors = true;
      continue;
    }

    console.log(`   Found ${docs.classes.length} classes, ${docs.functions.length} functions`);

    // Generate MDX
    console.log(`   Generating MDX...`);
    const mdx = generateMDX(docs, config);

    // Ensure output directory exists
    const outputPath = path.join(ROOT_DIR, config.outputPath);
    const outputDir = path.dirname(outputPath);
    if (!fs.existsSync(outputDir)) {
      fs.mkdirSync(outputDir, { recursive: true });
    }

    if (checkOnly) {
      // Check mode: compare with existing file
      if (fs.existsSync(outputPath)) {
        const existing = fs.readFileSync(outputPath, 'utf-8');
        if (existing !== mdx) {
          console.error(`   ❌ ${path.basename(outputPath)} is out of sync with source code`);
          hasErrors = true;
        } else {
          console.log(`   ✅ ${path.basename(outputPath)} is up to date`);
        }
      } else {
        console.error(`   ❌ ${path.basename(outputPath)} does not exist`);
        hasErrors = true;
      }
    } else {
      // Generate mode: write file
      fs.writeFileSync(outputPath, mdx);
      console.log(`   ✅ Generated ${path.relative(ROOT_DIR, outputPath)}`);
    }
  }

  if (hasErrors) {
    if (checkOnly) {
      console.error(
        "\n💡 Run 'npx tsx scripts/docs-generators/python-sdk.ts' to update documentation"
      );
    }
    process.exit(1);
  }

  console.log('\n✅ Python SDK documentation generation complete!');
}

// ============================================================================
// Version Discovery
// ============================================================================

interface VersionInfo {
  version: string;
  href: string;
  isCurrent: boolean;
}

/**
 * Get the latest released version from git tags.
 * Falls back to the version from source if no tags found.
 */
function getLatestReleasedVersion(config: SDKConfig, fallbackVersion: string): string {
  try {
    const output = execSync(`git tag | grep "^${config.tagPrefix}" | sort -V | tail -1`, {
      encoding: 'utf-8',
      cwd: ROOT_DIR,
    }).trim();
    if (output) {
      return output.replace(config.tagPrefix, '');
    }
  } catch {
    // Fall through to fallback
  }
  return fallbackVersion;
}

function discoverVersions(config: SDKConfig, currentVersion: string): VersionInfo[] {
  const baseDir = config.docsBaseDir
    ? path.join(ROOT_DIR, config.docsBaseDir)
    : path.join(ROOT_DIR, 'docs/content/docs/cua/reference');
  const docsDir = path.join(baseDir, config.outputDir);
  const hrefBase = config.hrefBase ?? '/cua/reference';
  const versions: VersionInfo[] = [];

  // Add current version (latest) — points to the index page (folder root)
  const currentMajorMinor = currentVersion.split('.').slice(0, 2).join('.');
  versions.push({
    version: currentMajorMinor,
    href: `${hrefBase}/${config.outputDir}`,
    isCurrent: true,
  });

  // Discover versioned folders (v0.5, v0.4, etc.)
  if (fs.existsSync(docsDir)) {
    const entries = fs.readdirSync(docsDir, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.isDirectory() && entry.name.startsWith('v')) {
        const version = entry.name.substring(1); // Remove 'v' prefix
        // Skip if this is the current version
        if (version === currentMajorMinor) continue;

        versions.push({
          version,
          href: `${hrefBase}/${config.outputDir}/${entry.name}/api`,
          isCurrent: false,
        });
      }
    }
  }

  // Sort versions descending
  versions.sort((a, b) => {
    const partsA = a.version.split('.').map((x) => parseInt(x, 10) || 0);
    const partsB = b.version.split('.').map((x) => parseInt(x, 10) || 0);
    for (let i = 0; i < Math.max(partsA.length, partsB.length); i++) {
      const partA = partsA[i] || 0;
      const partB = partsB[i] || 0;
      if (partA !== partB) return partB - partA;
    }
    return 0;
  });

  return versions;
}

// ============================================================================
// MDX Generation
// ============================================================================

function generateMDX(docs: PythonPackage, config: SDKConfig): string {
  const lines: string[] = [];

  // Get the actual latest released version from git tags
  const releasedVersion = getLatestReleasedVersion(config, docs.version);

  // Frontmatter
  const pageTitle = config.pageTitle ?? `${config.displayName} API Reference`;
  lines.push('---');
  lines.push(`title: ${pageTitle}`);
  lines.push(`description: ${config.description}`);
  lines.push('---');
  lines.push('');

  // Auto-generated notice
  lines.push(`{/*`);
  lines.push(`  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY`);
  lines.push(`  Generated by: npx tsx scripts/docs-generators/python-sdk.ts`);
  lines.push(`  Source: ${config.packageDir}`);
  lines.push(`  Version: ${releasedVersion}`);
  lines.push(`*/}`);
  lines.push('');

  // Imports
  lines.push("import { Callout } from 'fumadocs-ui/components/callout';");
  lines.push("import { Tabs, Tab } from 'fumadocs-ui/components/tabs';");
  lines.push("import { VersionHeader } from '@/components/version-selector';");
  lines.push('');

  // Discover available versions using the released version
  const versions = discoverVersions(config, releasedVersion);
  const currentMajorMinor = releasedVersion.split('.').slice(0, 2).join('.');

  // Version selector and badge
  lines.push('<VersionHeader');
  lines.push(`  versions={${JSON.stringify(versions)}}`);
  lines.push(`  currentVersion="${currentMajorMinor}"`);
  lines.push(`  fullVersion="${releasedVersion}"`);
  // Use pip-style package name (underscores → hyphens)
  // If it already starts with 'cua', don't add prefix
  const pipName = config.packageName.replace(/_/g, '-');
  const fullPipName = pipName.startsWith('cua') ? pipName : `cua-${pipName}`;
  lines.push(`  packageName="${fullPipName}"`);
  lines.push('/>');
  lines.push('');

  // Package description
  if (docs.docstring) {
    lines.push(docs.docstring);
    lines.push('');
  }

  // Table of contents for classes
  if (docs.classes.length > 0) {
    lines.push('## Classes');
    lines.push('');
    lines.push('| Class | Description |');
    lines.push('|-------|-------------|');
    for (const cls of docs.classes) {
      if (!cls.is_private) {
        const desc = escapeMDX(cls.description.split('\n')[0]) || 'No description';
        lines.push(`| [\`${cls.name}\`](#${cls.name.toLowerCase()}) | ${desc} |`);
      }
    }
    lines.push('');
  }

  // Table of contents for functions
  if (docs.functions.length > 0) {
    lines.push('## Functions');
    lines.push('');
    lines.push('| Function | Description |');
    lines.push('|----------|-------------|');
    for (const fn of docs.functions) {
      if (!fn.is_private) {
        const desc = escapeMDX(fn.description.split('\n')[0]) || 'No description';
        lines.push(`| [\`${fn.name}\`](#${fn.name.toLowerCase()}) | ${desc} |`);
      }
    }
    lines.push('');
  }

  // Detailed class documentation
  for (const cls of docs.classes) {
    if (!cls.is_private) {
      lines.push(...generateClassDoc(cls));
    }
  }

  // Detailed function documentation
  for (const fn of docs.functions) {
    if (!fn.is_private) {
      lines.push(...generateFunctionDoc(fn, '##'));
    }
  }

  // Submodules (for packages that expose API through submodules)
  if (docs.submodules && docs.submodules.length > 0) {
    let publicSubmodules = docs.submodules.filter(
      (m) => !m.name.startsWith('_') && (m.classes.length > 0 || m.functions.length > 0)
    );

    // Filter to only included submodules if configured
    if (config.includeSubmodules) {
      publicSubmodules = publicSubmodules.filter((m) => config.includeSubmodules!.includes(m.name));
    }

    for (const mod of publicSubmodules) {
      const publicClasses = mod.classes.filter((c) => !c.is_private);
      const publicFunctions = mod.functions.filter((f) => !f.is_private && !f.is_dunder);

      if (publicClasses.length === 0 && publicFunctions.length === 0) continue;

      lines.push('---');
      lines.push('');
      lines.push(`## ${mod.name}`);
      lines.push('');
      if (mod.docstring) {
        lines.push(escapeMDX(mod.docstring));
        lines.push('');
      }

      for (const cls of publicClasses) {
        lines.push(...generateClassDoc(cls));
      }

      for (const fn of publicFunctions) {
        lines.push(...generateFunctionDoc(fn, '###'));
      }
    }
  }

  return lines.join('\n');
}

function generateClassDoc(cls: ClassDoc): string[] {
  const lines: string[] = [];

  lines.push('---');
  lines.push('');
  lines.push(`## ${cls.name}`);
  lines.push('');

  // Base classes
  if (cls.bases.length > 0) {
    const bases = cls.bases.filter((b) => b !== 'object').join(', ');
    if (bases) {
      lines.push(`*Inherits from: ${bases}*`);
      lines.push('');
    }
  }

  // Description
  if (cls.description) {
    lines.push(escapeMDX(cls.description));
    lines.push('');
  }

  // Constructor (__init__)
  const initMethod = cls.methods.find((m) => m.name === '__init__');
  if (initMethod) {
    lines.push('### Constructor');
    lines.push('');
    lines.push('```python');
    lines.push(formatSignature(initMethod.signature, cls.name));
    lines.push('```');
    lines.push('');

    if (initMethod.parameters.length > 0) {
      lines.push(...generateParametersTable(initMethod.parameters));
    }
  }

  // Attributes
  if (cls.attributes.length > 0) {
    lines.push('### Attributes');
    lines.push('');
    lines.push('| Name | Type | Description |');
    lines.push('|------|------|-------------|');
    for (const attr of cls.attributes) {
      const type = attr.type || 'Any';
      // Strip docstring sections and collapse to single line for table cells
      const desc = escapeMDX(stripDocstringSections(attr.description)) || '';
      lines.push(`| \`${attr.name}\` | \`${type}\` | ${desc} |`);
    }
    lines.push('');
  }

  // Methods (excluding __init__ and private)
  const publicMethods = cls.methods.filter(
    (m) => !m.is_private && !m.is_dunder && m.name !== '__init__'
  );

  if (publicMethods.length > 0) {
    lines.push('### Methods');
    lines.push('');

    for (const method of publicMethods) {
      lines.push(...generateMethodDoc(method, cls.name));
    }
  }

  return lines;
}

function generateMethodDoc(method: FunctionDoc, className: string): string[] {
  const lines: string[] = [];

  lines.push(`#### ${className}.${method.name}`);
  lines.push('');
  lines.push('```python');
  lines.push(formatSignature(method.signature));
  lines.push('```');
  lines.push('');

  // Use structured data flags to avoid duplicating info from docstring
  const hasStructuredParams = method.parameters.filter((p) => p.name !== 'self').length > 0;
  const hasStructuredReturns = !!method.returns;
  const hasStructuredRaises = method.raises.length > 0;

  if (method.description) {
    lines.push(
      ...formatDocstringLines(
        method.description,
        hasStructuredParams,
        hasStructuredReturns,
        hasStructuredRaises
      )
    );
  }

  // Structured parameters (from parsed signature)
  if (hasStructuredParams) {
    const params = method.parameters.filter((p) => p.name !== 'self');
    lines.push(...generateParametersTable(params));
  }

  // Structured returns
  if (method.returns) {
    lines.push('**Returns:**');
    lines.push('');
    const returnType = method.returns.type || 'None';
    const returnDesc = escapeMDX(method.returns.description) || '';
    lines.push(`- \`${returnType}\` - ${returnDesc}`);
    lines.push('');
  }

  // Structured raises
  if (method.raises.length > 0) {
    lines.push('**Raises:**');
    lines.push('');
    for (const exc of method.raises) {
      lines.push(`- \`${exc.type}\` - ${escapeMDX(exc.description)}`);
    }
    lines.push('');
  }

  return lines;
}

function generateFunctionDoc(fn: FunctionDoc, heading: string): string[] {
  const lines: string[] = [];

  lines.push(`${heading} ${fn.name}`);
  lines.push('');
  lines.push('```python');
  lines.push(formatSignature(fn.signature));
  lines.push('```');
  lines.push('');

  const hasStructuredParams = fn.parameters.length > 0;
  const hasStructuredReturns = !!fn.returns;
  const hasStructuredRaises = fn.raises.length > 0;

  if (fn.description) {
    lines.push(
      ...formatDocstringLines(
        fn.description,
        hasStructuredParams,
        hasStructuredReturns,
        hasStructuredRaises
      )
    );
  }

  // Structured parameters
  if (hasStructuredParams) {
    lines.push(...generateParametersTable(fn.parameters));
  }

  // Structured returns
  if (fn.returns) {
    lines.push('**Returns:**');
    lines.push('');
    const returnType = fn.returns.type || 'None';
    const returnDesc = escapeMDX(fn.returns.description) || '';
    lines.push(`- \`${returnType}\` - ${returnDesc}`);
    lines.push('');
  }

  // Structured raises
  if (fn.raises.length > 0) {
    lines.push('**Raises:**');
    lines.push('');
    for (const exc of fn.raises) {
      lines.push(`- \`${exc.type}\` - ${escapeMDX(exc.description)}`);
    }
    lines.push('');
  }

  // Structured examples (from parsed data)
  if (fn.examples.length > 0) {
    lines.push('**Example:**');
    lines.push('');
    lines.push('```python');
    for (const example of fn.examples) {
      lines.push(example);
    }
    lines.push('```');
    lines.push('');
  }

  return lines;
}

function generateParametersTable(params: ParameterDoc[]): string[] {
  const lines: string[] = [];

  lines.push('**Parameters:**');
  lines.push('');
  lines.push('| Name | Type | Description |');
  lines.push('|------|------|-------------|');

  for (const param of params) {
    const type = param.type || 'Any';
    const desc = escapeMDX(param.description) || '';
    const defaultVal = param.default ? ` (default: \`${param.default}\`)` : '';
    lines.push(`| \`${param.name}\` | \`${type}\` | ${desc}${defaultVal} |`);
  }

  lines.push('');
  return lines;
}

function formatSignature(signature: string, className?: string): string {
  // Replace __init__ with class name for constructors
  if (className && signature.includes('__init__')) {
    return signature.replace('def __init__', className);
  }
  return signature;
}

/**
 * Escape special MDX characters in text content.
 * Curly braces and HTML-like tags must be escaped outside of code blocks.
 */
function escapeMDX(text: string): string {
  if (!text) return text;
  return (
    text
      .replace(/\{/g, '\\{')
      .replace(/\}/g, '\\}')
      // Escape HTML-like tags that would be interpreted as JSX components
      // but preserve markdown links []() and code backticks
      .replace(/<(?!\/?(?:Callout|Tab|Tabs|VersionHeader|div|span|a|code|pre|br|hr)\b)/g, '&lt;')
  );
}

/**
 * Parse Google-style docstring sections (Args, Returns, Raises, Examples)
 * and return the description text with those sections stripped,
 * plus the parsed sections as structured data.
 */
interface ParsedDocstring {
  description: string;
  args: { name: string; type: string; description: string }[];
  returns: string;
  raises: { type: string; description: string }[];
  examples: string[];
}

function parseDocstring(text: string): ParsedDocstring {
  if (!text) return { description: '', args: [], returns: '', raises: [], examples: [] };

  const lines = text.split('\n');
  const result: ParsedDocstring = {
    description: '',
    args: [],
    returns: '',
    raises: [],
    examples: [],
  };

  type Section = 'description' | 'args' | 'returns' | 'raises' | 'examples';
  let currentSection: Section = 'description';
  const descLines: string[] = [];
  const returnsLines: string[] = [];
  const exampleLines: string[] = [];
  let currentArg: { name: string; type: string; description: string } | null = null;
  let currentRaise: { type: string; description: string } | null = null;

  for (const line of lines) {
    const trimmed = line.trim();

    // Detect section headers
    if (/^Args?\s*:/.test(trimmed) || /^Parameters?\s*:/.test(trimmed)) {
      currentSection = 'args';
      continue;
    }
    if (/^Returns?\s*:/.test(trimmed)) {
      // Check if it's a one-liner like "Returns: something"
      const inlineReturn = trimmed.replace(/^Returns?\s*:\s*/, '');
      if (inlineReturn) {
        returnsLines.push(inlineReturn);
      }
      currentSection = 'returns';
      continue;
    }
    if (/^Raises?\s*:/.test(trimmed)) {
      currentSection = 'raises';
      continue;
    }
    if (/^Examples?\s*:/.test(trimmed)) {
      currentSection = 'examples';
      continue;
    }

    switch (currentSection) {
      case 'description':
        descLines.push(line);
        break;

      case 'args': {
        // Match "param_name (type): description" or "param_name: description"
        const argMatch = trimmed.match(/^(\w+)\s*(?:\(([^)]+)\))?\s*:\s*(.*)$/);
        if (argMatch && !line.startsWith('        ')) {
          if (currentArg) result.args.push(currentArg);
          currentArg = {
            name: argMatch[1],
            type: argMatch[2] || '',
            description: argMatch[3],
          };
        } else if (currentArg && trimmed) {
          // Continuation line for current arg
          currentArg.description += ' ' + trimmed;
        }
        break;
      }

      case 'returns':
        if (trimmed) returnsLines.push(trimmed);
        break;

      case 'raises': {
        const raiseMatch = trimmed.match(/^(\w+)\s*:\s*(.*)$/);
        if (raiseMatch && !line.startsWith('        ')) {
          if (currentRaise) result.raises.push(currentRaise);
          currentRaise = { type: raiseMatch[1], description: raiseMatch[2] };
        } else if (currentRaise && trimmed) {
          currentRaise.description += ' ' + trimmed;
        }
        break;
      }

      case 'examples':
        exampleLines.push(line);
        break;
    }
  }

  // Flush remaining items
  if (currentArg) result.args.push(currentArg);
  if (currentRaise) result.raises.push(currentRaise);

  result.description = descLines.join('\n').trim();
  result.returns = returnsLines.join(' ').trim();
  result.examples = exampleLines;

  return result;
}

/**
 * Format a parsed docstring into markdown lines.
 * Only outputs sections not already covered by structured data.
 */
function formatDocstringLines(
  text: string,
  hasStructuredParams: boolean,
  hasStructuredReturns: boolean,
  hasStructuredRaises: boolean
): string[] {
  const parsed = parseDocstring(text);
  const lines: string[] = [];

  // Description (always output)
  if (parsed.description) {
    lines.push(escapeMDX(parsed.description));
    lines.push('');
  }

  // Args (only if no structured params)
  if (!hasStructuredParams && parsed.args.length > 0) {
    lines.push('**Parameters:**');
    lines.push('');
    lines.push('| Name | Type | Description |');
    lines.push('|------|------|-------------|');
    for (const arg of parsed.args) {
      const type = arg.type || 'Any';
      lines.push(`| \`${arg.name}\` | \`${escapeMDX(type)}\` | ${escapeMDX(arg.description)} |`);
    }
    lines.push('');
  }

  // Returns (only if no structured returns)
  if (!hasStructuredReturns && parsed.returns) {
    lines.push(`**Returns:** ${escapeMDX(parsed.returns)}`);
    lines.push('');
  }

  // Raises (only if no structured raises)
  if (!hasStructuredRaises && parsed.raises.length > 0) {
    lines.push('**Raises:**');
    lines.push('');
    for (const r of parsed.raises) {
      lines.push(`- \`${r.type}\` - ${escapeMDX(r.description)}`);
    }
    lines.push('');
  }

  // Examples
  if (parsed.examples.length > 0) {
    // Dedent example lines by removing common leading whitespace
    const nonEmptyLines = parsed.examples.filter((l) => l.trim().length > 0);
    if (nonEmptyLines.length > 0) {
      const minIndent = Math.min(...nonEmptyLines.map((l) => l.match(/^(\s*)/)?.[1].length ?? 0));
      const dedented = parsed.examples
        .map((l) => (l.trim().length > 0 ? l.substring(minIndent) : ''))
        .join('\n')
        .trim();
      if (dedented) {
        lines.push('**Example:**');
        lines.push('');
        lines.push('```python');
        lines.push(dedented);
        lines.push('```');
        lines.push('');
      }
    }
  }

  return lines;
}

/**
 * Strip docstring sections from text for use in single-line contexts (e.g. table cells).
 */
function stripDocstringSections(text: string): string {
  if (!text) return text;
  const parsed = parseDocstring(text);
  // Return only the first line of the description
  return parsed.description.split('\n')[0].trim();
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
