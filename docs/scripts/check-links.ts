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

import {
  readFiles,
  validateFiles,
  printErrors,
  type FileObject,
  type ScanResult,
} from 'next-validate-link';
import * as path from 'path';
import { pathToFileURL } from 'node:url';
import fg from 'fast-glob';

const DOCS_DIR = path.resolve(__dirname, '..');
const CONTENT_DIR = path.join(DOCS_DIR, 'content/docs');
const BLOG_DIR = path.join(DOCS_DIR, '../blog');

/**
 * Build the ScanResult (valid URL map) directly from content files.
 * This is more reliable than scanURLs for catch-all routes.
 */
export async function buildScannedUrls(files: FileObject[]): Promise<ScanResult> {
  // Fumadocs exposes this plugin through an import-only package export.
  const { remarkHeading } = await import('fumadocs-core/mdx-plugins/remark-heading');
  const urls: ScanResult['urls'] = new Map();

  for (const file of files) {
    if (!file.url) continue;
    const hashes: string[] = [];
    // Reuse the validator's MDX parser and the renderer's heading plugin so
    // inline markup, duplicate headings, and [#custom-id] use Fumadocs IDs.
    await validateFiles([file], {
      scanned: { urls: new Map(), fallbackUrls: [] },
      markdown: {
        remarkPlugins: [[remarkHeading, { generateToc: false }]],
        onNode(node) {
          if (node.type === 'heading') {
            const id = node.data?.hProperties?.id;
            if (typeof id === 'string') hashes.push(id);
          } else if (node.type === 'mdxJsxFlowElement' || node.type === 'mdxJsxTextElement') {
            for (const attribute of node.attributes) {
              if (
                attribute.type === 'mdxJsxAttribute' &&
                attribute.name === 'id' &&
                typeof attribute.value === 'string'
              ) {
                hashes.push(attribute.value);
              }
            }
          }
          // This pass only inventories anchors. Validate links after every
          // destination has been indexed, including pages without headings.
          return { hrefs: [] };
        },
      },
    });
    urls.set(file.url, { hashes });
  }

  // Blog posts share the cua.ai origin with the docs app, so root-relative
  // links from docs pages should be checked against the repository's posts.
  const blogFiles = fg.sync('**/*.md', { cwd: BLOG_DIR });
  for (const file of blogFiles) {
    const slug = file.replace(/\.md$/, '').replace(/\/index$/, '');
    urls.set(`/blog/${slug}`, {});
  }

  // Preserve root-page anchors when the content includes an index page.
  if (!urls.has('/')) urls.set('/', {});

  return { urls, fallbackUrls: [] };
}

export function pathToUrl(filePath: string): string {
  const slug = path
    .relative(CONTENT_DIR, path.resolve(DOCS_DIR, filePath))
    .split(path.sep)
    .join('/')
    .replace(/\.mdx$/, '')
    .replace(/(^|\/)index$/, '');
  return `/${slug}`;
}

export async function checkLinks(files: FileObject[], scanned: ScanResult, checkExternal = false) {
  const results = await Promise.all(
    files.map((file) =>
      validateFiles([file], {
        scanned,
        checkExternal,
        // Fumadocs resolves relative MDX links at runtime, skip them here
        checkRelativePaths: false,
        checkRelativeUrls: false,
        pathToUrl,
        markdown: {
          onNode(node) {
            if (node.type !== 'link') return { hrefs: [] };
            // next-validate-link skips bare fragments; resolve them to this page
            // so they go through the same fragment check as cross-page links.
            return {
              hrefs: [node.url.startsWith('#') && file.url ? `${file.url}${node.url}` : node.url],
            };
          },
        },
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
      })
    )
  );
  return results.flat();
}

async function main() {
  const checkExternal = process.argv.slice(2).includes('--external');
  const files = await readFiles(path.join(CONTENT_DIR, '**/*.mdx'), { pathToUrl });
  const scanned = await buildScannedUrls(files);
  const results = await checkLinks(files, scanned, checkExternal);

  printErrors(results, true);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch((err) => {
    console.error('Fatal error:', err);
    process.exit(1);
  });
}
