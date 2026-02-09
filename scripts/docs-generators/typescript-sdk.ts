#!/usr/bin/env npx tsx

/**
 * TypeScript SDK Documentation Generator
 *
 * Generates MDX API reference documentation from TypeScript source code.
 * Uses regex-based parsing to extract exports and JSDoc comments (no TS compiler dependency).
 *
 * Usage:
 *   npx tsx scripts/docs-generators/typescript-sdk.ts              # Generate all
 *   npx tsx scripts/docs-generators/typescript-sdk.ts --sdk=cuabot  # Generate specific
 */

import * as fs from 'fs';
import * as path from 'path';
import { execSync } from 'child_process';

// ============================================================================
// Types
// ============================================================================

interface ExtractedClass {
  name: string;
  description: string;
  constructorSig: string | null;
  constructorParams: ParamInfo[];
  methods: MethodInfo[];
  properties: PropInfo[];
}

interface ExtractedInterface {
  name: string;
  description: string;
  properties: PropInfo[];
}

interface ExtractedFunction {
  name: string;
  description: string;
  signature: string;
  params: ParamInfo[];
  returnType: string;
  isAsync: boolean;
}

interface ExtractedConst {
  name: string;
  type: string;
  description: string;
}

interface MethodInfo {
  name: string;
  description: string;
  signature: string;
  params: ParamInfo[];
  returnType: string;
  isAsync: boolean;
}

interface ParamInfo {
  name: string;
  type: string;
  description: string;
  defaultValue: string | null;
  isOptional: boolean;
}

interface PropInfo {
  name: string;
  type: string;
  description: string;
  isOptional: boolean;
}

interface ModuleDoc {
  name: string;
  description: string;
  classes: ExtractedClass[];
  interfaces: ExtractedInterface[];
  functions: ExtractedFunction[];
  constants: ExtractedConst[];
}

interface SDKConfig {
  packageDir: string;
  packageName: string;
  outputPath: string;
  displayName: string;
  description: string;
  outputDir: string;
  tagPrefix: string;
  docsBaseDir?: string;
  hrefBase?: string;
  pageTitle?: string;
  includeFiles?: string[];
  installCommand?: string;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');

const SDK_CONFIGS: Record<string, SDKConfig> = {
  cuabot: {
    packageDir: 'libs/cuabot/src',
    packageName: 'cuabot',
    outputPath: 'docs/content/docs/cuabot/reference/index.mdx',
    displayName: 'Cua-Bot',
    description: 'TypeScript API reference for the Cua-Bot sandboxed agent framework',
    outputDir: 'reference',
    tagPrefix: 'cuabot-v',
    docsBaseDir: 'docs/content/docs/cuabot',
    hrefBase: '/cuabot',
    pageTitle: 'API Reference',
    includeFiles: ['client.ts', 'settings.ts'],
    installCommand: 'npm install -g cuabot',
  },
};

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const sdkArg = args.find((a) => a.startsWith('--sdk='));
  const targetSdk = sdkArg?.split('=')[1];

  console.log('📦 TypeScript SDK Documentation Generator');
  console.log('==========================================\n');

  let hasErrors = false;

  for (const [sdkName, config] of Object.entries(SDK_CONFIGS)) {
    if (targetSdk && targetSdk !== sdkName) continue;

    console.log(`📖 Processing ${config.displayName}...`);

    const packagePath = path.join(ROOT_DIR, config.packageDir);
    if (!fs.existsSync(packagePath)) {
      console.error(`   ❌ Package not found: ${config.packageDir}`);
      hasErrors = true;
      continue;
    }

    const modules = extractDocs(packagePath, config);
    console.log(
      `   Found ${modules.reduce((n, m) => n + m.classes.length, 0)} classes, ` +
        `${modules.reduce((n, m) => n + m.functions.length, 0)} functions, ` +
        `${modules.reduce((n, m) => n + m.interfaces.length, 0)} interfaces`
    );

    const mdx = generateMDX(modules, config);

    const outputPath = path.join(ROOT_DIR, config.outputPath);
    const outputDir = path.dirname(outputPath);
    if (!fs.existsSync(outputDir)) {
      fs.mkdirSync(outputDir, { recursive: true });
    }

    fs.writeFileSync(outputPath, mdx);
    console.log(`   ✅ Generated ${path.relative(ROOT_DIR, outputPath)}`);
  }

  if (hasErrors) process.exit(1);
  console.log('\n✅ TypeScript SDK documentation generation complete!');
}

// ============================================================================
// Regex-based Extraction
// ============================================================================

function extractDocs(packagePath: string, config: SDKConfig): ModuleDoc[] {
  const fileNames = config.includeFiles
    ? config.includeFiles
    : fs.readdirSync(packagePath).filter((f) => f.endsWith('.ts') && !f.endsWith('.d.ts'));

  const modules: ModuleDoc[] = [];

  for (const fileName of fileNames) {
    const filePath = path.join(packagePath, fileName);
    if (!fs.existsSync(filePath)) continue;
    const source = fs.readFileSync(filePath, 'utf-8');
    const moduleName = path.basename(fileName, '.ts');

    const mod: ModuleDoc = {
      name: moduleName,
      description: extractFileDescription(source),
      classes: extractClasses(source),
      interfaces: extractInterfaces(source),
      functions: extractFunctions(source),
      constants: extractConstants(source),
    };

    if (mod.classes.length > 0 || mod.interfaces.length > 0 || mod.functions.length > 0 || mod.constants.length > 0) {
      modules.push(mod);
    }
  }

  return modules;
}

function extractFileDescription(source: string): string {
  const match = source.match(/^\/\*\*\s*\n([\s\S]*?)\*\//);
  if (!match) return '';
  return match[1]
    .split('\n')
    .map((l) => l.replace(/^\s*\*\s?/, '').trim())
    .filter(Boolean)
    .join('\n');
}

/**
 * Get the JSDoc comment immediately preceding a position in source.
 */
function getJSDocBefore(source: string, pos: number): string {
  const before = source.substring(0, pos);
  const match = before.match(/\/\*\*([\s\S]*?)\*\/\s*$/);
  if (!match) return '';
  return match[1]
    .split('\n')
    .map((l) => l.replace(/^\s*\*\s?/, ''))
    .filter((l) => !l.startsWith('@'))
    .join('\n')
    .trim();
}

function extractClasses(source: string): ExtractedClass[] {
  const classes: ExtractedClass[] = [];
  const classRegex = /export\s+class\s+(\w+)(?:\s+extends\s+[\w.]+)?\s*\{/g;

  let match;
  while ((match = classRegex.exec(source)) !== null) {
    const name = match[1];
    const description = getJSDocBefore(source, match.index);
    const classBodyStart = match.index + match[0].length;
    const classBody = extractBraceBlock(source, classBodyStart - 1);

    const cls: ExtractedClass = {
      name,
      description,
      constructorSig: null,
      constructorParams: [],
      methods: [],
      properties: [],
    };

    // Extract constructor
    const ctorMatch = classBody.match(/constructor\s*\(([\s\S]*?)\)\s*\{/);
    if (ctorMatch) {
      cls.constructorParams = parseParams(ctorMatch[1]);
      cls.constructorSig = `constructor(${ctorMatch[1].trim()})`;
    }

    // Extract methods (async or not, excluding private)
    const methodRegex = /(\/\*\*[\s\S]*?\*\/\s*)?(async\s+)?(\w+)\s*\(([\s\S]*?)\)\s*:\s*([\w<>\[\]|, ]+)\s*\{/g;
    let mMatch;
    while ((mMatch = methodRegex.exec(classBody)) !== null) {
      const methodName = mMatch[3];
      if (methodName === 'constructor' || methodName.startsWith('_') || methodName === 'private') continue;

      const isAsync = !!mMatch[2];
      const params = parseParams(mMatch[4]);
      const returnType = mMatch[5].trim();
      const jsdoc = mMatch[1] ? parseJSDocBlock(mMatch[1]) : '';

      // Get param descriptions from JSDoc
      if (mMatch[1]) {
        const paramDescs = parseJSDocParams(mMatch[1]);
        for (const p of params) {
          if (paramDescs[p.name]) p.description = paramDescs[p.name];
        }
      }

      cls.methods.push({
        name: methodName,
        description: jsdoc,
        signature: `${isAsync ? 'async ' : ''}${methodName}(${params.map((p) => formatParam(p)).join(', ')}): ${returnType}`,
        params: params.filter((p) => p.name !== 'this'),
        returnType,
        isAsync,
      });
    }

    classes.push(cls);
  }

  return classes;
}

function extractInterfaces(source: string): ExtractedInterface[] {
  const interfaces: ExtractedInterface[] = [];
  const ifaceRegex = /export\s+interface\s+(\w+)\s*\{/g;

  let match;
  while ((match = ifaceRegex.exec(source)) !== null) {
    const name = match[1];
    const description = getJSDocBefore(source, match.index);
    const bodyStart = match.index + match[0].length;
    const body = extractBraceBlock(source, bodyStart - 1);

    const properties: PropInfo[] = [];
    const propRegex = /(\w+)(\?)?\s*:\s*([^;\n]+)/g;
    let pMatch;
    while ((pMatch = propRegex.exec(body)) !== null) {
      properties.push({
        name: pMatch[1],
        type: pMatch[3].trim().replace(/;$/, ''),
        description: '',
        isOptional: !!pMatch[2],
      });
    }

    interfaces.push({ name, description, properties });
  }

  return interfaces;
}

function extractFunctions(source: string): ExtractedFunction[] {
  const functions: ExtractedFunction[] = [];
  const fnRegex = /export\s+(async\s+)?function\s+(\w+)\s*\(([\s\S]*?)\)\s*:\s*([\w<>\[\]|, {}:]+)\s*\{/g;

  let match;
  while ((match = fnRegex.exec(source)) !== null) {
    const isAsync = !!match[1];
    const name = match[2];
    if (name.startsWith('_')) continue;

    const params = parseParams(match[3]);
    const returnType = match[4].trim();
    const description = getJSDocBefore(source, match.index);

    // Get param descriptions from JSDoc
    const jsdocBlock = source.substring(Math.max(0, match.index - 500), match.index);
    const jsdocMatch = jsdocBlock.match(/\/\*\*([\s\S]*?)\*\/\s*$/);
    if (jsdocMatch) {
      const paramDescs = parseJSDocParams(jsdocMatch[0]);
      for (const p of params) {
        if (paramDescs[p.name]) p.description = paramDescs[p.name];
      }
    }

    functions.push({
      name,
      description,
      signature: `${isAsync ? 'async ' : ''}function ${name}(${params.map((p) => formatParam(p)).join(', ')}): ${returnType}`,
      params,
      returnType,
      isAsync,
    });
  }

  return functions;
}

function extractConstants(source: string): ExtractedConst[] {
  const constants: ExtractedConst[] = [];
  const constRegex = /export\s+const\s+(\w+)(?:\s*:\s*([^=]+))?\s*=/g;

  let match;
  while ((match = constRegex.exec(source)) !== null) {
    const name = match[1];
    if (name.startsWith('_')) continue;
    const type = match[2]?.trim() || 'const';
    const description = getJSDocBefore(source, match.index);
    constants.push({ name, type, description });
  }

  return constants;
}

// ============================================================================
// Helpers
// ============================================================================

function extractBraceBlock(source: string, openBracePos: number): string {
  let depth = 0;
  let start = openBracePos;
  for (let i = openBracePos; i < source.length; i++) {
    if (source[i] === '{') depth++;
    else if (source[i] === '}') {
      depth--;
      if (depth === 0) return source.substring(start + 1, i);
    }
  }
  return source.substring(start + 1);
}

function parseParams(paramStr: string): ParamInfo[] {
  if (!paramStr.trim()) return [];

  const params: ParamInfo[] = [];
  let depth = 0;
  let current = '';

  for (const char of paramStr) {
    if (char === '(' || char === '<' || char === '{' || char === '[') depth++;
    else if (char === ')' || char === '>' || char === '}' || char === ']') depth--;

    if (char === ',' && depth === 0) {
      const p = parseSingleParam(current.trim());
      if (p) params.push(p);
      current = '';
    } else {
      current += char;
    }
  }
  if (current.trim()) {
    const p = parseSingleParam(current.trim());
    if (p) params.push(p);
  }

  return params;
}

function parseSingleParam(param: string): ParamInfo | null {
  if (!param) return null;

  // Match: name?: type = default
  const match = param.match(/^(\w+)(\?)?\s*(?::\s*([\s\S]+?))?(?:\s*=\s*([\s\S]+))?$/);
  if (!match) return null;

  return {
    name: match[1],
    type: match[3]?.trim().replace(/\s*=\s*[\s\S]*$/, '') || 'any',
    description: '',
    defaultValue: match[4]?.trim() || null,
    isOptional: !!match[2] || !!match[4],
  };
}

function formatParam(p: ParamInfo): string {
  const opt = p.isOptional && !p.defaultValue ? '?' : '';
  const def = p.defaultValue ? ` = ${p.defaultValue}` : '';
  return `${p.name}${opt}: ${p.type}${def}`;
}

function parseJSDocBlock(block: string): string {
  return block
    .replace(/^\/\*\*\s*/, '')
    .replace(/\s*\*\/\s*$/, '')
    .split('\n')
    .map((l) => l.replace(/^\s*\*\s?/, ''))
    .filter((l) => !l.startsWith('@'))
    .join('\n')
    .trim();
}

function parseJSDocParams(block: string): Record<string, string> {
  const params: Record<string, string> = {};
  const lines = block.split('\n');
  for (const line of lines) {
    const match = line.match(/@param\s+(\w+)\s+(.*)/);
    if (match) params[match[1]] = match[2].trim();
  }
  return params;
}

// ============================================================================
// Version Discovery
// ============================================================================

interface VersionInfo {
  version: string;
  href: string;
  isCurrent: boolean;
}

function getLatestReleasedVersion(config: SDKConfig, fallback: string): string {
  try {
    const output = execSync(`git tag | grep "^${config.tagPrefix}" | sort -V | tail -1`, {
      encoding: 'utf-8',
      cwd: ROOT_DIR,
    }).trim();
    if (output) return output.replace(config.tagPrefix, '');
  } catch {
    // fall through
  }
  return fallback;
}

function discoverVersions(config: SDKConfig, currentVersion: string): VersionInfo[] {
  const baseDir = config.docsBaseDir
    ? path.join(ROOT_DIR, config.docsBaseDir)
    : path.join(ROOT_DIR, 'docs/content/docs/cuabot');
  const docsDir = path.join(baseDir, config.outputDir);
  const hrefBase = config.hrefBase ?? '/cuabot';
  const versions: VersionInfo[] = [];

  const currentMM = currentVersion.split('.').slice(0, 2).join('.');
  versions.push({ version: currentMM, href: `${hrefBase}/${config.outputDir}`, isCurrent: true });

  if (fs.existsSync(docsDir)) {
    for (const entry of fs.readdirSync(docsDir, { withFileTypes: true })) {
      if (entry.isDirectory() && entry.name.startsWith('v')) {
        const v = entry.name.substring(1);
        if (v === currentMM) continue;
        versions.push({ version: v, href: `${hrefBase}/${config.outputDir}/${entry.name}/api`, isCurrent: false });
      }
    }
  }

  versions.sort((a, b) => {
    const pa = a.version.split('.').map(Number);
    const pb = b.version.split('.').map(Number);
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      if ((pa[i] || 0) !== (pb[i] || 0)) return (pb[i] || 0) - (pa[i] || 0);
    }
    return 0;
  });

  return versions;
}

function getPackageVersion(config: SDKConfig): string {
  const pkgJsonPath = path.join(ROOT_DIR, path.dirname(config.packageDir), 'package.json');
  try {
    return JSON.parse(fs.readFileSync(pkgJsonPath, 'utf-8')).version || '0.0.0';
  } catch {
    return '0.0.0';
  }
}

// ============================================================================
// MDX Generation
// ============================================================================

function escapeMDX(text: string): string {
  if (!text) return text;
  return text
    .replace(/\{/g, '\\{')
    .replace(/\}/g, '\\}')
    .replace(/<(?!\/?(?:Callout|Tab|Tabs|VersionHeader|div|span|a|code|pre|br|hr)\b)/g, '&lt;');
}

function generateMDX(modules: ModuleDoc[], config: SDKConfig): string {
  const lines: string[] = [];
  const pkgVersion = getPackageVersion(config);
  const releasedVersion = getLatestReleasedVersion(config, pkgVersion);
  const pageTitle = config.pageTitle ?? `${config.displayName} API Reference`;

  // Frontmatter
  lines.push('---');
  lines.push(`title: ${pageTitle}`);
  lines.push(`description: ${config.description}`);
  lines.push('---');
  lines.push('');
  lines.push(`{/*`);
  lines.push(`  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY`);
  lines.push(`  Generated by: npx tsx scripts/docs-generators/typescript-sdk.ts`);
  lines.push(`  Source: ${config.packageDir}`);
  lines.push(`  Version: ${releasedVersion}`);
  lines.push(`*/}`);
  lines.push('');

  // Imports
  lines.push("import { Callout } from 'fumadocs-ui/components/callout';");
  lines.push("import { VersionHeader } from '@/components/version-selector';");
  lines.push('');

  // Version header
  const versions = discoverVersions(config, releasedVersion);
  const currentMM = releasedVersion.split('.').slice(0, 2).join('.');
  lines.push('<VersionHeader');
  lines.push(`  versions={${JSON.stringify(versions)}}`);
  lines.push(`  currentVersion="${currentMM}"`);
  lines.push(`  fullVersion="${releasedVersion}"`);
  lines.push(`  packageName="${config.packageName}"`);
  if (config.installCommand) {
    lines.push(`  installCommand="${config.installCommand}"`);
  }
  lines.push('/>');
  lines.push('');

  for (const mod of modules) {
    lines.push('---');
    lines.push('');
    lines.push(`## ${mod.name}`);
    lines.push('');
    if (mod.description) {
      lines.push(escapeMDX(mod.description));
      lines.push('');
    }

    // Interfaces
    for (const iface of mod.interfaces) {
      lines.push(`### ${iface.name}`);
      lines.push('');
      if (iface.description) {
        lines.push(escapeMDX(iface.description));
        lines.push('');
      }
      lines.push('```typescript');
      lines.push(`interface ${iface.name} {`);
      for (const prop of iface.properties) {
        const opt = prop.isOptional ? '?' : '';
        lines.push(`  ${prop.name}${opt}: ${prop.type};`);
      }
      lines.push('}');
      lines.push('```');
      lines.push('');
      if (iface.properties.length > 0) {
        lines.push('| Property | Type | Description |');
        lines.push('|----------|------|-------------|');
        for (const prop of iface.properties) {
          const opt = prop.isOptional ? ' *(optional)*' : '';
          lines.push(`| \`${prop.name}\` | \`${escapeMDX(prop.type)}\` | ${opt}${escapeMDX(prop.description)} |`);
        }
        lines.push('');
      }
    }

    // Constants
    for (const c of mod.constants) {
      lines.push(`### ${c.name}`);
      lines.push('');
      lines.push('```typescript');
      lines.push(`const ${c.name}: ${escapeMDX(c.type)}`);
      lines.push('```');
      lines.push('');
      if (c.description) {
        lines.push(escapeMDX(c.description));
        lines.push('');
      }
    }

    // Classes
    for (const cls of mod.classes) {
      lines.push(`### ${cls.name}`);
      lines.push('');
      if (cls.description) {
        lines.push(escapeMDX(cls.description));
        lines.push('');
      }

      if (cls.constructorSig) {
        lines.push('#### Constructor');
        lines.push('');
        lines.push('```typescript');
        lines.push(`new ${cls.name}(${cls.constructorParams.map((p) => formatParam(p)).join(', ')})`);
        lines.push('```');
        lines.push('');
        if (cls.constructorParams.length > 0) {
          lines.push(...generateParamsTable(cls.constructorParams));
        }
      }

      if (cls.methods.length > 0) {
        lines.push('#### Methods');
        lines.push('');
        for (const method of cls.methods) {
          lines.push(`##### ${cls.name}.${method.name}`);
          lines.push('');
          lines.push('```typescript');
          lines.push(method.signature);
          lines.push('```');
          lines.push('');
          if (method.description) {
            lines.push(escapeMDX(method.description));
            lines.push('');
          }
          if (method.params.length > 0) {
            lines.push(...generateParamsTable(method.params));
          }
          if (method.returnType && method.returnType !== 'void' && method.returnType !== 'Promise<void>') {
            lines.push(`**Returns:** \`${escapeMDX(method.returnType)}\``);
            lines.push('');
          }
        }
      }
    }

    // Functions
    for (const fn of mod.functions) {
      lines.push(`### ${fn.name}`);
      lines.push('');
      lines.push('```typescript');
      lines.push(fn.signature);
      lines.push('```');
      lines.push('');
      if (fn.description) {
        lines.push(escapeMDX(fn.description));
        lines.push('');
      }
      if (fn.params.length > 0) {
        lines.push(...generateParamsTable(fn.params));
      }
      if (fn.returnType && fn.returnType !== 'void') {
        lines.push(`**Returns:** \`${escapeMDX(fn.returnType)}\``);
        lines.push('');
      }
    }
  }

  return lines.join('\n');
}

function generateParamsTable(params: ParamInfo[]): string[] {
  const lines: string[] = [];
  lines.push('**Parameters:**');
  lines.push('');
  lines.push('| Name | Type | Description |');
  lines.push('|------|------|-------------|');
  for (const p of params) {
    const def = p.defaultValue ? ` (default: \`${p.defaultValue}\`)` : '';
    const opt = p.isOptional ? ' *(optional)*' : '';
    lines.push(`| \`${p.name}\` | \`${escapeMDX(p.type)}\` | ${escapeMDX(p.description)}${opt}${def} |`);
  }
  lines.push('');
  return lines;
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
