#!/usr/bin/env npx tsx

/**
 * Documentation Link Checker
 *
 * Uses next-validate-link (by the Fumadocs author) to validate all internal
 * links across MDX documentation files. Builds the valid URL map from the
 * content directory and validates links using remark-based MDX parsing.
 *
 * Usage:
 *   pnpm docs:check-links                # Check internal links
 *   pnpm docs:check-links:external       # Also check external links
 */

import { readFiles, validateFiles, printErrors, type ScanResult } from 'next-validate-link';
import * as path from 'path';
import fg from 'fast-glob';

const DOCS_DIR = path.resolve(__dirname, '..');
const CONTENT_DIR = path.join(DOCS_DIR, 'content/docs');

/**
 * Build the ScanResult (valid URL map) directly from content files.
 * This is more reliable than scanURLs for catch-all routes.
 */
function buildScannedUrls(): ScanResult {
  const urls = new Map<string, {}>();
  const mdxFiles = fg.sync('**/*.mdx', { cwd: CONTENT_DIR });

  for (const file of mdxFiles) {
    const slug = file.replace(/\.mdx$/, '').replace(/\/index$/, '');
    urls.set(`/${slug}`, {});
  }

  // Also add the root
  urls.set('/', {});

  return { urls, fallbackUrls: [] };
}

function pathToUrl(filePath: string): string | undefined {
  const slug = filePath
    .replace(/^content\/docs\//, '')
    .replace(/\.mdx$/, '')
    .replace(/\/index$/, '');
  return `/${slug}`;
}

async function main() {
  const args = process.argv.slice(2);
  const checkExternal = args.includes('--external');

  const scanned = buildScannedUrls();

  // Read all MDX content files
  const files = await readFiles('content/docs/**/*.mdx', { pathToUrl });

  // Validate all links in the MDX files
  const results = await validateFiles(files, {
    scanned,
    checkExternal,
    // Fumadocs resolves relative MDX links at runtime, skip them here
    checkRelativePaths: false,
    checkRelativeUrls: false,
    pathToUrl,
    whitelist: (url: string) => {
      const pathname = url.split(/[#?]/)[0];
      const ext = path.extname(pathname).toLowerCase();
      const staticExts = new Set([
        '.png',
        '.jpg',
        '.jpeg',
        '.gif',
        '.svg',
        '.ico',
        '.webp',
        '.avif',
        '.mp4',
        '.webm',
        '.pdf',
        '.css',
        '.js',
        '.woff',
        '.woff2',
        '.ttf',
        '.eot',
      ]);
      return staticExts.has(ext);
    },
  });

  printErrors(results, true);
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
