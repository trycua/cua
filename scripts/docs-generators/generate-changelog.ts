#!/usr/bin/env npx tsx

/**
 * Changelog Generator
 *
 * Generates changelog MDX files for each SDK by parsing GitHub releases.
 * Fetches all releases, cleans up noisy auto-generated body content, and
 * produces clean per-SDK changelogs grouped by major.minor version.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/generate-changelog.ts           # Generate all
 *   npx tsx scripts/docs-generators/generate-changelog.ts --check   # Check for drift
 *   npx tsx scripts/docs-generators/generate-changelog.ts --sdk=computer  # Specific SDK
 */

import { execSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

// ============================================================================
// Types
// ============================================================================

interface GitHubRelease {
  tagName: string;
  name: string;
  body: string;
  publishedAt: string;
  isDraft: boolean;
  isPrerelease: boolean;
}

interface SDKConfig {
  name: string;
  displayName: string;
  tagPrefix: string;
  outputPath: string;
}

// ============================================================================
// Configuration
// ============================================================================

const ROOT_DIR = path.resolve(__dirname, '../..');
const DOCS_BASE = path.join(ROOT_DIR, 'docs/content/docs');
const GITHUB_REPO = 'trycua/cua';

const SDK_CONFIGS: SDKConfig[] = [
  {
    name: 'computer',
    displayName: 'Computer SDK',
    tagPrefix: 'computer-v',
    outputPath: 'cua/reference/computer-sdk/changelog.mdx',
  },
  {
    name: 'agent',
    displayName: 'Agent SDK',
    tagPrefix: 'agent-v',
    outputPath: 'cua/reference/agent-sdk/changelog.mdx',
  },
  {
    name: 'computer-server',
    displayName: 'Desktop Sandbox',
    tagPrefix: 'computer-server-v',
    outputPath: 'cua/reference/desktop-sandbox/changelog.mdx',
  },
  {
    name: 'lume',
    displayName: 'Lume',
    tagPrefix: 'lume-v',
    outputPath: 'lume/reference/changelog.mdx',
  },
];

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const checkOnly = args.includes('--check');
  const sdkArg = args.find((a) => a.startsWith('--sdk='));
  const targetSdk = sdkArg?.split('=')[1];

  console.log('Changelog Generator');
  console.log('======================\n');

  // Fetch ALL releases once (across all SDKs)
  console.log('Fetching all GitHub releases...');
  const allReleases = fetchAllReleases();
  console.log(`Found ${allReleases.length} total releases\n`);

  let hasErrors = false;

  for (const config of SDK_CONFIGS) {
    if (targetSdk && targetSdk !== config.name) {
      continue;
    }

    console.log(`Processing ${config.displayName}...`);

    // Filter releases for this SDK
    const sdkReleases = allReleases.filter(
      (r) => r.tagName.startsWith(config.tagPrefix) && !r.isDraft
    );

    // Sort by version descending
    sdkReleases.sort((a, b) => {
      const versionA = a.tagName.replace(config.tagPrefix, '');
      const versionB = b.tagName.replace(config.tagPrefix, '');
      return compareVersions(versionB, versionA);
    });

    console.log(`   Found ${sdkReleases.length} releases`);

    if (sdkReleases.length === 0) {
      console.log(`   No releases found for ${config.displayName}`);
      continue;
    }

    // Fetch body for each release
    const releasesWithBody = fetchBodies(sdkReleases);

    // Generate MDX
    const mdx = generateChangelogMDX(config, releasesWithBody);

    // Output path
    const outputPath = path.join(DOCS_BASE, config.outputPath);
    const outputDir = path.dirname(outputPath);

    if (!fs.existsSync(outputDir)) {
      fs.mkdirSync(outputDir, { recursive: true });
    }

    if (checkOnly) {
      if (fs.existsSync(outputPath)) {
        const existing = fs.readFileSync(outputPath, 'utf-8');
        if (existing !== mdx) {
          console.error(`   ${config.name} changelog is out of sync`);
          hasErrors = true;
        } else {
          console.log(`   ${config.name} changelog is up to date`);
        }
      } else {
        console.error(`   ${config.name} changelog does not exist`);
        hasErrors = true;
      }
    } else {
      fs.writeFileSync(outputPath, mdx);
      console.log(`   Generated ${path.relative(ROOT_DIR, outputPath)}`);
    }
  }

  if (hasErrors) {
    if (checkOnly) {
      console.error("\nRun 'npx tsx scripts/docs-generators/generate-changelog.ts' to update");
    }
    process.exit(1);
  }

  console.log('\nChangelog generation complete!');
}

// ============================================================================
// GitHub Release Fetching
// ============================================================================

/**
 * Fetch all releases at once (without body) to avoid per-SDK limit issues.
 * Uses --limit 500 to get all releases across the entire repo.
 */
function fetchAllReleases(): GitHubRelease[] {
  try {
    const output = execSync(
      'gh release list --limit 500 --json tagName,name,publishedAt,isDraft,isPrerelease',
      { encoding: 'utf-8', cwd: ROOT_DIR, maxBuffer: 10 * 1024 * 1024 }
    );
    return JSON.parse(output);
  } catch (error) {
    console.error(`Failed to fetch releases: ${error}`);
    return [];
  }
}

/**
 * Fetch release bodies individually for a filtered set of releases.
 */
function fetchBodies(releases: GitHubRelease[]): GitHubRelease[] {
  const results: GitHubRelease[] = [];

  for (const release of releases) {
    try {
      const output = execSync(`gh release view "${release.tagName}" --json body`, {
        encoding: 'utf-8',
        cwd: ROOT_DIR,
        maxBuffer: 1024 * 1024,
      });
      const data = JSON.parse(output);
      results.push({
        ...release,
        body: data.body || '',
      });
    } catch {
      results.push({ ...release, body: '' });
    }
  }

  return results;
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
// MDX Generation
// ============================================================================

function generateChangelogMDX(config: SDKConfig, releases: GitHubRelease[]): string {
  const lines: string[] = [];

  // Frontmatter
  lines.push('---');
  lines.push(`title: Changelog`);
  lines.push(`description: Release history for ${config.displayName}`);
  lines.push('---');
  lines.push('');

  // Auto-generated notice
  lines.push(`{/*`);
  lines.push(`  AUTO-GENERATED FILE - DO NOT EDIT DIRECTLY`);
  lines.push(`  Generated by: npx tsx scripts/docs-generators/generate-changelog.ts`);
  lines.push(`  Last updated: ${new Date().toISOString().split('T')[0]}`);
  lines.push(`*/}`);
  lines.push('');

  // Introduction
  lines.push(`# ${config.displayName} Changelog`);
  lines.push('');
  lines.push(`All notable changes to the ${config.displayName} are documented here.`);
  lines.push('');

  // Group releases by major.minor version
  const grouped = groupByMajorMinor(releases, config.tagPrefix);

  for (const [majorMinor, versionReleases] of Object.entries(grouped)) {
    lines.push(`## ${majorMinor}.x`);
    lines.push('');

    for (const release of versionReleases) {
      const version = release.tagName.replace(config.tagPrefix, '');
      const date = formatDate(release.publishedAt);

      lines.push(`### v${version} (${date})`);
      lines.push('');

      // Process release body
      const body = processReleaseBody(release.body);
      if (body) {
        lines.push(body);
        lines.push('');
      } else {
        lines.push('Maintenance release.');
        lines.push('');
      }
    }
  }

  return lines.join('\n');
}

function groupByMajorMinor(
  releases: GitHubRelease[],
  tagPrefix: string
): Record<string, GitHubRelease[]> {
  const grouped: Record<string, GitHubRelease[]> = {};

  for (const release of releases) {
    const version = release.tagName.replace(tagPrefix, '');
    const parts = version.split('.');
    const majorMinor = `${parts[0]}.${parts[1] || '0'}`;

    if (!grouped[majorMinor]) {
      grouped[majorMinor] = [];
    }
    grouped[majorMinor].push(release);
  }

  // Sort keys by version descending
  const sortedKeys = Object.keys(grouped).sort((a, b) => compareVersions(b, a));
  const sortedGrouped: Record<string, GitHubRelease[]> = {};
  for (const key of sortedKeys) {
    sortedGrouped[key] = grouped[key];
  }

  return sortedGrouped;
}

function formatDate(isoDate: string): string {
  const date = new Date(isoDate);
  return date.toISOString().split('T')[0];
}

// ============================================================================
// Release Body Processing
// ============================================================================

/**
 * Clean up a GitHub release body to produce focused changelog content.
 *
 * Strips:
 * - "## What's Changed" header
 * - "## Installation" / "## Installation Options" sections
 * - "## Usage" / "## Quick Start" / "## Claude Desktop Integration" sections
 * - "## Dependencies" section (condensed to one line if present)
 * - Package description boilerplate
 * - "**Full Changelog**" links
 * - Heading markers ## / ### (to avoid hierarchy conflicts)
 * - Commit SHAs like (abc1234)
 *
 * Adds:
 * - Linked PR numbers: (#789) -> ([#789](https://github.com/trycua/cua/pull/789))
 *
 * Detects:
 * - Bot-only releases (all entries are "Bump cua-X") -> "Maintenance release."
 */
function processReleaseBody(body: string): string {
  if (!body || body.trim() === '') {
    return '';
  }

  let processed = body;

  // Remove "**Full Changelog**" links
  processed = processed.replace(/\*\*Full Changelog\*\*:.*$/gm, '');

  // Remove package description boilerplate headers
  processed = processed.replace(
    /^##\s*(Computer control library|Computer Server|Base package|MCP Server|Computer Vision|Toolkit for computer-use|Lightweight webUI|Unified CLI).*$/gm,
    ''
  );

  // Remove standalone description lines that match boilerplate patterns
  processed = processed.replace(
    /^(A FastAPI-based server implementation|This package provides (MCP|enhanced UI)|Manage cloud sandboxes).*$/gm,
    ''
  );

  // Remove raw checksum lines (sha256 hex + filename)
  processed = processed.replace(/^[a-f0-9]{64}\s+\S+.*$/gm, '');
  // Remove markdown table rows that look like checksum tables
  processed = processed.replace(/^\|?\s*`?[a-f0-9]{64}`?\s*\|.*$/gm, '');
  // Remove checksum table headers
  processed = processed.replace(/^\|?\s*SHA256\s*\|.*$/gim, '');
  processed = processed.replace(/^\|?\s*-+\s*\|.*$/gm, '');

  // Extract dependencies from "## Dependencies" section before removing sections
  let depsLine = '';
  const depsContent = extractSection(processed, 'Dependencies');
  if (depsContent !== null) {
    const items = depsContent
      .split('\n')
      .filter((l) => l.trim().startsWith('*'))
      .map((l) => l.replace(/^\*\s*/, '').trim());
    if (items.length > 0) {
      depsLine = `**Dependencies:** ${items.join(', ')}`;
    }
    processed = removeSection(processed, 'Dependencies');
  }

  // Remove boilerplate sections (BEFORE removing "What's Changed" heading,
  // so section boundaries are intact for removeSection to work correctly)
  const sectionsToRemove = [
    'Installation Options',
    'Installation with script',
    'Installation',
    'Usage',
    'Claude Desktop Integration',
    'Quick Start',
  ];
  for (const section of sectionsToRemove) {
    processed = removeSection(processed, section);
  }

  // NOW remove "What's Changed" / "Whats Changed" header line (after sections are removed)
  processed = processed.replace(/^#{1,3}\s*What'?s Changed\s*$/gm, '');

  // Handle single-line "**Dependencies:**" already in the body (from new workflow format)
  const inlineDeps = processed.match(/^\*\*Dependencies:\*\*.*$/m);
  if (inlineDeps) {
    depsLine = inlineDeps[0].trim();
    processed = processed.replace(/^\*\*Dependencies:\*\*.*$/m, '');
  }

  // Remove release title headings like "# cua-computer v0.4.11"
  processed = processed.replace(/^#{1,3}\s*cua-\S+\s+v[\d.]+\s*$/gm, '');

  // Strip heading markers (## / ###) from remaining content to avoid hierarchy issues
  processed = processed.replace(/^#{1,3}\s+/gm, '');

  // Link PR numbers: (#123) -> ([#123](https://github.com/trycua/cua/pull/123))
  processed = processed.replace(
    /\(#(\d+)\)/g,
    `([#$1](https://github.com/${GITHUB_REPO}/pull/$1))`
  );

  // Strip commit SHAs: (7-8 char hex in parens) from entries
  processed = processed.replace(/\s*\([a-f0-9]{7,8}\)/g, '');

  // Clean up excessive blank lines
  processed = processed.replace(/\n{3,}/g, '\n\n').trim();

  // Check if all remaining bullet lines are bot bumps
  const contentLines = processed
    .split('\n')
    .filter((l) => l.trim().startsWith('*') || l.trim().startsWith('-'));
  const allBotBumps =
    contentLines.length > 0 && contentLines.every((l) => /Bump (cua-|lume)/i.test(l));

  if (allBotBumps && !depsLine) {
    return 'Maintenance release.';
  }

  // Re-add dependencies line at the top if present
  if (depsLine) {
    processed = depsLine + (processed ? '\n\n' + processed : '');
  }

  // Final check: if nothing meaningful remains
  if (!processed.trim()) {
    return '';
  }

  // Escape curly braces outside of code blocks for MDX compatibility
  processed = escapeMDXOutsideCode(processed);

  return processed;
}

/**
 * Extract the content of a markdown section (any heading level) up to the next
 * same-or-higher-level heading or end of text.
 * Returns null if the section is not found.
 */
function extractSection(text: string, sectionTitle: string): string | null {
  const lines = text.split('\n');
  let inSection = false;
  let sectionLevel = 0;
  let content: string[] = [];

  for (const line of lines) {
    const headerMatch = line.match(
      new RegExp(`^(#{1,6})\\s*${escapeRegex(sectionTitle)}\\s*$`, 'i')
    );
    if (!inSection && headerMatch) {
      inSection = true;
      sectionLevel = headerMatch[1].length;
      continue;
    }
    if (inSection) {
      // Stop at next heading of same or higher level
      const headingMatch = line.match(/^(#{1,6})\s/);
      if (headingMatch && headingMatch[1].length <= sectionLevel) {
        break;
      }
      content.push(line);
    }
  }

  return inSection ? content.join('\n') : null;
}

/**
 * Remove a markdown section (any heading level) and everything until the next
 * same-or-higher-level heading or end of text.
 */
function removeSection(text: string, sectionTitle: string): string {
  const lines = text.split('\n');
  const result: string[] = [];
  let inSection = false;
  let sectionLevel = 0;

  for (const line of lines) {
    const headerMatch =
      !inSection && line.match(new RegExp(`^(#{1,6})\\s*${escapeRegex(sectionTitle)}\\s*$`, 'i'));
    if (headerMatch) {
      inSection = true;
      sectionLevel = headerMatch[1].length;
      continue;
    }
    if (inSection) {
      const headingMatch = line.match(/^(#{1,6})\s/);
      if (headingMatch && headingMatch[1].length <= sectionLevel) {
        inSection = false;
        result.push(line);
      }
      continue;
    }
    result.push(line);
  }

  return result.join('\n');
}

function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function escapeMDXOutsideCode(text: string): string {
  const codeBlockRegex = /(```[\s\S]*?```|`[^`]+`)/g;
  const parts = text.split(codeBlockRegex);

  return parts
    .map((part) => {
      if (part.startsWith('```') || part.startsWith('`')) {
        return part;
      }
      return part.replace(/\{/g, '\\{').replace(/\}/g, '\\}');
    })
    .join('');
}

// ============================================================================
// Run
// ============================================================================

main().catch((error) => {
  console.error('Error:', error);
  process.exit(1);
});
