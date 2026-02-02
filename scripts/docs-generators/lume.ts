#!/usr/bin/env npx tsx

/**
 * Lume Documentation Generator
 *
 * Generates MDX documentation files from Lume's dump-docs command output.
 * This ensures documentation stays synchronized with the source code.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/lume.ts          # Generate docs
 *   npx tsx scripts/docs-generators/lume.ts --check  # Check for drift (CI mode)
 */

import { execSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

// ============================================================================
// Types
// ============================================================================

interface CLIDocumentation {
  name: string;
  version: string;
  abstract: string;
  commands: CommandDoc[];
}

interface CommandDoc {
  name: string;
  abstract: string;
  discussion?: string;
  arguments: ArgumentDoc[];
  options: OptionDoc[];
  flags: FlagDoc[];
  subcommands: CommandDoc[];
}

interface ArgumentDoc {
  name: string;
  help: string;
  type: string;
  is_optional: boolean;
}

interface OptionDoc {
  name: string;
  short_name?: string;
  help: string;
  type: string;
  default_value?: string;
  is_optional: boolean;
}

interface FlagDoc {
  name: string;
  short_name?: string;
  help: string;
  default_value: boolean;
}

interface HTTPAPIDocumentation {
  base_path: string;
  version: string;
  description: string;
  endpoints: APIEndpointDoc[];
}

interface APIEndpointDoc {
  method: string;
  path: string;
  description: string;
  category: string;
  path_parameters: APIParameterDoc[];
  query_parameters: APIParameterDoc[];
  request_body?: APIRequestBodyDoc;
  response_body: APIResponseDoc;
  status_codes: APIStatusCodeDoc[];
}

interface APIParameterDoc {
  name: string;
  type: string;
  required: boolean;
  description: string;
}

interface APIRequestBodyDoc {
  content_type: string;
  description: string;
  fields: APIFieldDoc[];
}

interface APIResponseDoc {
  content_type: string;
  description: string;
  fields?: APIFieldDoc[];
}

interface APIFieldDoc {
  name: string;
  type: string;
  required: boolean;
  description: string;
  default_value?: string;
}

interface APIStatusCodeDoc {
  code: number;
  description: string;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const LUME_DIR = path.join(ROOT_DIR, 'libs', 'lume');
const DOCS_OUTPUT_DIR = path.join(ROOT_DIR, 'docs', 'content', 'docs', 'lume', 'reference');

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const checkOnly = args.includes('--check') || args.includes('--check-only');

  console.log('🔧 Lume Documentation Generator');
  console.log('================================\n');

  // Step 1: Build Lume if needed
  console.log('📦 Building Lume...');
  try {
    execSync('swift build -c release', {
      cwd: LUME_DIR,
      stdio: 'inherit',
    });
  } catch (error) {
    console.error('❌ Failed to build Lume');
    process.exit(1);
  }

  // Step 2: Get CLI documentation
  console.log('\n📖 Extracting CLI documentation...');
  const cliDocsJson = execSync('.build/release/lume dump-docs --type cli', {
    cwd: LUME_DIR,
    encoding: 'utf-8',
  });
  const cliDocs: CLIDocumentation = JSON.parse(cliDocsJson);
  console.log(`   Found ${cliDocs.commands.length} commands`);

  // Step 3: Get API documentation
  console.log('📖 Extracting API documentation...');
  const apiDocsJson = execSync('.build/release/lume dump-docs --type api', {
    cwd: LUME_DIR,
    encoding: 'utf-8',
  });
  const apiDocs: HTTPAPIDocumentation = JSON.parse(apiDocsJson);
  console.log(`   Found ${apiDocs.endpoints.length} endpoints`);

  // Step 4: Generate MDX files
  console.log('\n📝 Generating documentation files...');

  const cliMdx = generateCLIReferenceMDX(cliDocs);
  const apiMdx = generateHTTPAPIMDX(apiDocs);

  const cliPath = path.join(DOCS_OUTPUT_DIR, 'cli-reference.mdx');
  const apiPath = path.join(DOCS_OUTPUT_DIR, 'http-api.mdx');

  if (checkOnly) {
    // Check mode: compare with existing files
    console.log('\n🔍 Checking for documentation drift...');

    let hasDrift = false;

    if (fs.existsSync(cliPath)) {
      const existingCli = fs.readFileSync(cliPath, 'utf-8');
      if (existingCli !== cliMdx) {
        console.error('❌ cli-reference.mdx is out of sync with source code');
        hasDrift = true;
      } else {
        console.log('✅ cli-reference.mdx is up to date');
      }
    } else {
      console.error('❌ cli-reference.mdx does not exist');
      hasDrift = true;
    }

    if (fs.existsSync(apiPath)) {
      const existingApi = fs.readFileSync(apiPath, 'utf-8');
      if (existingApi !== apiMdx) {
        console.error('❌ http-api.mdx is out of sync with source code');
        hasDrift = true;
      } else {
        console.log('✅ http-api.mdx is up to date');
      }
    } else {
      console.error('❌ http-api.mdx does not exist');
      hasDrift = true;
    }

    if (hasDrift) {
      console.error("\n💡 Run 'npx tsx scripts/docs-generators/lume.ts' to update documentation");
      process.exit(1);
    }

    console.log('\n✅ All Lume documentation is up to date!');
  } else {
    // Generate mode: write files
    fs.writeFileSync(cliPath, cliMdx);
    console.log(`   ✅ Generated ${path.relative(ROOT_DIR, cliPath)}`);

    fs.writeFileSync(apiPath, apiMdx);
    console.log(`   ✅ Generated ${path.relative(ROOT_DIR, apiPath)}`);

    console.log('\n✅ Lume documentation generated successfully!');
  }
}

// ============================================================================
// CLI Reference Generator
// ============================================================================

function generateCLIReferenceMDX(docs: CLIDocumentation): string {
  const lines: string[] = [];

  // Header - frontmatter MUST be at the very beginning of the file
  lines.push('---');
  lines.push('title: Lume CLI Reference');
  lines.push('description: Command Line Interface reference for Lume');
  lines.push('---');
  lines.push('');
  lines.push(`{/*
  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY
  Generated by: npx tsx scripts/docs-generators/lume.ts
  Source: libs/lume/src/Commands/*.swift
*/}`);
  lines.push('');
  lines.push("import { Callout } from 'fumadocs-ui/components/callout';");
  lines.push('');

  // Introduction
  lines.push(`${docs.abstract}`);
  lines.push('');
  lines.push('## Quick Start');
  lines.push('');
  lines.push('```bash');
  lines.push('# Run a prebuilt macOS VM');
  lines.push('lume run macos-sequoia-vanilla:latest');
  lines.push('');
  lines.push('# Create a custom VM');
  lines.push('lume create my-vm --cpu 4 --memory 8GB --disk-size 50GB');
  lines.push('```');
  lines.push('');

  // Group commands by category
  const vmManagement = ['create', 'run', 'stop', 'delete', 'clone'];
  const vmInfo = ['ls', 'get', 'set'];
  const remoteAccess = ['ssh'];
  const imageManagement = ['images', 'pull', 'push', 'ipsw', 'prune'];
  const configuration = ['config', 'serve', 'logs', 'setup'];

  lines.push('## VM Management');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => vmManagement.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd, '###'));
  }

  lines.push('## VM Information and Configuration');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => vmInfo.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd, '###'));
  }

  lines.push('## Remote Access');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => remoteAccess.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd, '###'));
  }

  lines.push('## Image Management');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => imageManagement.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd, '###'));
  }

  lines.push('## Configuration and Server');
  lines.push('');
  for (const cmd of docs.commands.filter((c) => configuration.includes(c.name))) {
    lines.push(...generateCommandDoc(cmd, '###'));
  }

  // Global options
  lines.push('## Global Options');
  lines.push('');
  lines.push('These options are available for all commands:');
  lines.push('');
  lines.push('- `--help` - Show help information');
  lines.push('- `--version` - Show version number');
  lines.push('');

  return lines.join('\n');
}

function generateCommandDoc(cmd: CommandDoc, heading: string): string[] {
  const lines: string[] = [];

  lines.push(`${heading} lume ${cmd.name}`);
  lines.push('');
  lines.push(cmd.abstract);
  lines.push('');

  // Arguments
  if (cmd.arguments.length > 0) {
    lines.push('**Arguments:**');
    lines.push('');
    for (const arg of cmd.arguments) {
      const optional = arg.is_optional ? ' (optional)' : '';
      lines.push(`- \`<${arg.name}>\` - ${arg.help}${optional}`);
    }
    lines.push('');
  }

  // Options
  if (cmd.options.length > 0) {
    lines.push('**Options:**');
    lines.push('');
    for (const opt of cmd.options) {
      const shortFlag = opt.short_name ? `-${opt.short_name}, ` : '';
      const defaultVal = opt.default_value ? ` (default: ${opt.default_value})` : '';
      lines.push(`- \`${shortFlag}--${opt.name}\` - ${opt.help}${defaultVal}`);
    }
    lines.push('');
  }

  // Flags
  if (cmd.flags.length > 0) {
    lines.push('**Flags:**');
    lines.push('');
    for (const flag of cmd.flags) {
      const shortFlag = flag.short_name ? `-${flag.short_name}, ` : '';
      lines.push(`- \`${shortFlag}--${flag.name}\` - ${flag.help}`);
    }
    lines.push('');
  }

  // Subcommands
  if (cmd.subcommands.length > 0) {
    lines.push('**Subcommands:**');
    lines.push('');
    for (const sub of cmd.subcommands) {
      lines.push(`- \`lume ${cmd.name} ${sub.name}\` - ${sub.abstract}`);

      // Show subcommand details
      if (sub.arguments.length > 0 || sub.options.length > 0) {
        for (const arg of sub.arguments) {
          lines.push(`  - \`<${arg.name}>\` - ${arg.help}`);
        }
        for (const opt of sub.options) {
          const shortFlag = opt.short_name ? `-${opt.short_name}, ` : '';
          lines.push(`  - \`${shortFlag}--${opt.name}\` - ${opt.help}`);
        }
      }

      // Nested subcommands
      if (sub.subcommands.length > 0) {
        for (const nested of sub.subcommands) {
          lines.push(`  - \`lume ${cmd.name} ${sub.name} ${nested.name}\` - ${nested.abstract}`);
        }
      }
    }
    lines.push('');
  }

  return lines;
}

// ============================================================================
// HTTP API Reference Generator
// ============================================================================

function generateHTTPAPIMDX(docs: HTTPAPIDocumentation): string {
  const lines: string[] = [];

  // Header - frontmatter MUST be at the very beginning of the file
  lines.push('---');
  lines.push('title: Lume HTTP API Reference');
  lines.push('description: HTTP API reference for Lume server');
  lines.push('---');
  lines.push('');
  lines.push(`{/*
  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY
  Generated by: npx tsx scripts/docs-generators/lume.ts
  Source: libs/lume/src/Server/*.swift
*/}`);
  lines.push('');
  lines.push("import { Callout } from 'fumadocs-ui/components/callout';");
  lines.push("import { Tabs, Tab } from 'fumadocs-ui/components/tabs';");
  lines.push('');

  // Introduction
  lines.push(docs.description);
  lines.push('');
  lines.push('## Default URL');
  lines.push('');
  lines.push('```');
  lines.push('http://localhost:7777');
  lines.push('```');
  lines.push('');
  lines.push(
    'Start the server with `lume serve` or specify a custom port with `lume serve --port <port>`.'
  );
  lines.push('');

  // Group endpoints by category
  const categories = [...new Set(docs.endpoints.map((e) => e.category))];

  for (const category of categories) {
    lines.push(`## ${category}`);
    lines.push('');

    const categoryEndpoints = docs.endpoints.filter((e) => e.category === category);

    for (const endpoint of categoryEndpoints) {
      lines.push(...generateEndpointDoc(endpoint));
    }
  }

  return lines.join('\n');
}

function generateEndpointDoc(endpoint: APIEndpointDoc): string[] {
  const lines: string[] = [];

  // Title from description
  const title = endpoint.description.replace(
    /^Get |^List |^Create |^Delete |^Update |^Set |^Add |^Remove |^Pull |^Push |^Prune |^Stop |^Run |^Retrieve /,
    ''
  );
  lines.push(`### ${capitalizeFirst(title)}`);
  lines.push('');
  lines.push(endpoint.description);
  lines.push('');
  lines.push(`\`${endpoint.method}: ${endpoint.path}\``);
  lines.push('');

  // Parameters table (path + query)
  const allParams = [
    ...endpoint.path_parameters.map((p) => ({ ...p, location: 'path' })),
    ...endpoint.query_parameters.map((p) => ({ ...p, location: 'query' })),
  ];

  if (allParams.length > 0) {
    lines.push('#### Parameters');
    lines.push('');
    lines.push('| Name | Type | Required | Description |');
    lines.push('| ---- | ---- | -------- | ----------- |');
    for (const param of allParams) {
      const required = param.required ? 'Yes' : 'No';
      lines.push(`| ${param.name} | ${param.type} | ${required} | ${param.description} |`);
    }
    lines.push('');
  }

  // Request body
  if (endpoint.request_body) {
    lines.push('#### Request Body');
    lines.push('');
    lines.push('| Name | Type | Required | Description |');
    lines.push('| ---- | ---- | -------- | ----------- |');
    for (const field of endpoint.request_body.fields) {
      const required = field.required ? 'Yes' : 'No';
      const defaultStr = field.default_value ? ` (default: ${field.default_value})` : '';
      lines.push(
        `| ${field.name} | ${field.type} | ${required} | ${field.description}${defaultStr} |`
      );
    }
    lines.push('');
  }

  // Example request
  lines.push('#### Example Request');
  lines.push('');
  lines.push("<Tabs groupId=\"language\" persist items={['Curl', 'Python', 'TypeScript']}>");

  // Generate curl example
  lines.push('  <Tab value="Curl">');
  lines.push('```bash');
  lines.push(generateCurlExample(endpoint));
  lines.push('```');
  lines.push('  </Tab>');

  // Generate Python example
  lines.push('  <Tab value="Python">');
  lines.push('```python');
  lines.push(generatePythonExample(endpoint));
  lines.push('```');
  lines.push('  </Tab>');

  // Generate TypeScript example
  lines.push('  <Tab value="TypeScript">');
  lines.push('```typescript');
  lines.push(generateTypeScriptExample(endpoint));
  lines.push('```');
  lines.push('  </Tab>');

  lines.push('</Tabs>');
  lines.push('');

  // Status codes
  lines.push('#### Response');
  lines.push('');
  for (const status of endpoint.status_codes) {
    lines.push(`- **${status.code}**: ${status.description}`);
  }
  lines.push('');

  lines.push('---');
  lines.push('');

  return lines;
}

function generateCurlExample(endpoint: APIEndpointDoc): string {
  const path = endpoint.path.replace(/:(\w+)/g, '{$1}');
  const url = `http://localhost:7777${path}`;

  if (endpoint.method === 'GET' || endpoint.method === 'DELETE') {
    if (endpoint.method === 'DELETE') {
      return `curl -X DELETE "${url}"`;
    }
    return `curl "${url}"`;
  }

  // POST/PATCH with body
  if (endpoint.request_body && endpoint.request_body.fields.length > 0) {
    const bodyObj: Record<string, unknown> = {};
    for (const field of endpoint.request_body.fields) {
      if (field.required) {
        bodyObj[field.name] = getExampleValue(field);
      }
    }
    const bodyJson = JSON.stringify(bodyObj, null, 2);
    return `curl -X ${endpoint.method} "http://localhost:7777${path}" \\
  -H "Content-Type: application/json" \\
  -d '${bodyJson}'`;
  }

  return `curl -X ${endpoint.method} "${url}"`;
}

function generatePythonExample(endpoint: APIEndpointDoc): string {
  const path = endpoint.path.replace(/:(\w+)/g, '{$1}');
  const method = endpoint.method.toLowerCase();

  const lines: string[] = [];
  lines.push('import requests');
  lines.push('');

  if (
    endpoint.request_body &&
    endpoint.request_body.fields.length > 0 &&
    (endpoint.method === 'POST' || endpoint.method === 'PATCH')
  ) {
    lines.push('data = {');
    for (const field of endpoint.request_body.fields) {
      if (field.required) {
        const value = getExampleValuePython(field);
        lines.push(`    "${field.name}": ${value},`);
      }
    }
    lines.push('}');
    lines.push('');
    lines.push(`response = requests.${method}("http://localhost:7777${path}", json=data)`);
  } else {
    lines.push(`response = requests.${method}("http://localhost:7777${path}")`);
  }

  lines.push('print(response.json())');

  return lines.join('\n');
}

function generateTypeScriptExample(endpoint: APIEndpointDoc): string {
  const path = endpoint.path.replace(/:(\w+)/g, '${$1}');

  const lines: string[] = [];

  if (
    endpoint.request_body &&
    endpoint.request_body.fields.length > 0 &&
    (endpoint.method === 'POST' || endpoint.method === 'PATCH')
  ) {
    lines.push(`const response = await fetch(\`http://localhost:7777${path}\`, {`);
    lines.push(`  method: "${endpoint.method}",`);
    lines.push('  headers: { "Content-Type": "application/json" },');
    lines.push('  body: JSON.stringify({');
    for (const field of endpoint.request_body.fields) {
      if (field.required) {
        const value = getExampleValueTS(field);
        lines.push(`    ${field.name}: ${value},`);
      }
    }
    lines.push('  }),');
    lines.push('});');
  } else if (endpoint.method === 'DELETE') {
    lines.push(`const response = await fetch(\`http://localhost:7777${path}\`, {`);
    lines.push(`  method: "DELETE",`);
    lines.push('});');
  } else {
    lines.push(`const response = await fetch(\`http://localhost:7777${path}\`);`);
  }

  lines.push('const data = await response.json();');

  return lines.join('\n');
}

function getExampleValue(field: APIFieldDoc): unknown {
  switch (field.type) {
    case 'string':
      if (field.name === 'name') return 'my-vm';
      if (field.name === 'os') return 'macOS';
      if (field.name === 'memory') return '8GB';
      if (field.name === 'diskSize') return '50GB';
      if (field.name === 'display') return '1024x768';
      if (field.name === 'image') return 'macos-sequoia-vanilla:latest';
      if (field.name === 'path') return '/path/to/storage';
      return 'example';
    case 'integer':
      if (field.name === 'cpu') return 4;
      return 1;
    case 'boolean':
      return false;
    case 'array':
      if (field.name === 'tags') return ['latest'];
      return [];
    default:
      return 'value';
  }
}

function getExampleValuePython(field: APIFieldDoc): string {
  const val = getExampleValue(field);
  if (typeof val === 'string') return `"${val}"`;
  if (typeof val === 'boolean') return val ? 'True' : 'False';
  if (Array.isArray(val)) return JSON.stringify(val);
  return String(val);
}

function getExampleValueTS(field: APIFieldDoc): string {
  const val = getExampleValue(field);
  if (typeof val === 'string') return `"${val}"`;
  if (Array.isArray(val)) return JSON.stringify(val);
  return String(val);
}

function capitalizeFirst(str: string): string {
  return str.charAt(0).toUpperCase() + str.slice(1);
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
