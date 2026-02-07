#!/usr/bin/env npx tsx

/**
 * Documentation Link Checker
 *
 * Validates internal and external links across all documentation files.
 * Scans MDX content files and TSX component files for broken links.
 *
 * Usage:
 *   npx tsx scripts/docs-generators/check-links.ts                  # Check internal links only
 *   npx tsx scripts/docs-generators/check-links.ts --external       # Also check external links
 *   npx tsx scripts/docs-generators/check-links.ts --verbose        # Show all links checked
 */

import * as fs from "fs";
import * as path from "path";

// ============================================================================
// Configuration
// ============================================================================

const REPO_ROOT = path.resolve(__dirname, "../..");
const DOCS_CONTENT_DIR = path.join(REPO_ROOT, "docs/content/docs");
const DOCS_SRC_DIR = path.join(REPO_ROOT, "docs/src");

// File patterns to scan for links
const MDX_GLOB_DIRS = [DOCS_CONTENT_DIR];
const TSX_GLOB_DIRS = [DOCS_SRC_DIR];

// ============================================================================
// Types
// ============================================================================

interface LinkInfo {
  url: string;
  file: string;
  line: number;
  text: string;
}

interface CheckResult {
  link: LinkInfo;
  status: "ok" | "broken" | "warning";
  reason?: string;
}

// ============================================================================
// Helpers
// ============================================================================

function walkDir(dir: string, ext: string): string[] {
  const results: string[] = [];
  if (!fs.existsSync(dir)) return results;

  const entries = fs.readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...walkDir(fullPath, ext));
    } else if (entry.name.endsWith(ext)) {
      results.push(fullPath);
    }
  }
  return results;
}

/**
 * Build the set of valid internal doc slugs from the file system.
 * e.g., "docs/content/docs/cuabench/examples/custom-agent.mdx" → "/cuabench/examples/custom-agent"
 */
function buildValidSlugs(): Set<string> {
  const slugs = new Set<string>();
  const mdxFiles = walkDir(DOCS_CONTENT_DIR, ".mdx");

  for (const file of mdxFiles) {
    const relative = path.relative(DOCS_CONTENT_DIR, file);
    // Remove .mdx extension
    let slug = relative.replace(/\.mdx$/, "");
    // Convert index files to directory slugs
    slug = slug.replace(/\/index$/, "");
    // Normalize path separators
    slug = "/" + slug.split(path.sep).join("/");
    slugs.add(slug);
  }

  // Root index
  if (fs.existsSync(path.join(DOCS_CONTENT_DIR, "index.mdx"))) {
    slugs.add("/");
  }

  return slugs;
}

/**
 * Extract markdown links [text](url) from file content.
 */
function extractMarkdownLinks(
  content: string,
  filePath: string
): LinkInfo[] {
  const links: LinkInfo[] = [];
  const lines = content.split("\n");

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    // Match markdown links: [text](url)
    // Avoid matching image definitions like ![alt](url)
    const mdLinkRegex = /\[([^\]]*)\]\(([^)]+)\)/g;
    let match;
    while ((match = mdLinkRegex.exec(line)) !== null) {
      const url = match[2].split(/[#?]/)[0]; // Strip anchors and query params
      if (url) {
        links.push({
          url: match[2],
          file: filePath,
          line: i + 1,
          text: match[1],
        });
      }
    }
  }

  return links;
}

/**
 * Extract JSX href attributes from file content.
 */
function extractJsxLinks(content: string, filePath: string): LinkInfo[] {
  const links: LinkInfo[] = [];
  const lines = content.split("\n");

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    // Match href="..." or href='...' or href={"/..."}
    const hrefRegex = /href=["'{]+([^"'}]+)["'}]*/g;
    let match;
    while ((match = hrefRegex.exec(line)) !== null) {
      const url = match[1];
      if (url) {
        links.push({
          url,
          file: filePath,
          line: i + 1,
          text: `(JSX href)`,
        });
      }
    }
  }

  return links;
}

// Static asset extensions that should not be validated as doc pages
const STATIC_ASSET_EXTENSIONS = new Set([
  ".png",
  ".jpg",
  ".jpeg",
  ".gif",
  ".svg",
  ".ico",
  ".webp",
  ".avif",
  ".mp4",
  ".webm",
  ".pdf",
  ".css",
  ".js",
  ".woff",
  ".woff2",
  ".ttf",
  ".eot",
]);

/**
 * Classify a link as internal, external, or other.
 */
function classifyLink(url: string): "internal" | "external" | "other" {
  if (url.startsWith("http://") || url.startsWith("https://")) {
    return "external";
  }
  if (url.startsWith("#") || url.startsWith("mailto:")) {
    return "other";
  }
  if (url.startsWith("/")) {
    // Skip static assets (images, fonts, etc.)
    const pathWithoutAnchor = url.split(/[#?]/)[0];
    const ext = path.extname(pathWithoutAnchor).toLowerCase();
    if (STATIC_ASSET_EXTENSIONS.has(ext)) {
      return "other";
    }
    return "internal";
  }
  // Relative paths in MDX are resolved by Fumadocs, skip them
  return "other";
}

/**
 * Check if an internal link resolves to a valid page.
 */
function checkInternalLink(
  link: LinkInfo,
  validSlugs: Set<string>
): CheckResult {
  // Strip anchor and query params for path validation
  let urlPath = link.url.split(/[#?]/)[0];

  // Remove trailing slash
  urlPath = urlPath.replace(/\/$/, "") || "/";

  // Check exact match
  if (validSlugs.has(urlPath)) {
    return { link, status: "ok" };
  }

  // Check if it's a valid section root (directory with index.mdx)
  // Some links might reference section roots
  if (validSlugs.has(urlPath + "/index")) {
    return { link, status: "ok" };
  }

  return {
    link,
    status: "broken",
    reason: `No page found for path: ${urlPath}`,
  };
}

/**
 * Check if an external link is reachable (HTTP HEAD request).
 */
async function checkExternalLink(link: LinkInfo): Promise<CheckResult> {
  const url = link.url.split(/[#]/)[0]; // Keep query params, strip anchors
  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);

    const response = await fetch(url, {
      method: "HEAD",
      signal: controller.signal,
      redirect: "follow",
      headers: {
        "User-Agent": "CuaDocs-LinkChecker/1.0",
      },
    });
    clearTimeout(timeout);

    if (response.ok) {
      return { link, status: "ok" };
    }

    // Some servers don't support HEAD, try GET
    if (response.status === 405 || response.status === 403) {
      const getController = new AbortController();
      const getTimeout = setTimeout(() => getController.abort(), 10000);
      const getResponse = await fetch(url, {
        method: "GET",
        signal: getController.signal,
        redirect: "follow",
        headers: {
          "User-Agent": "CuaDocs-LinkChecker/1.0",
        },
      });
      clearTimeout(getTimeout);

      if (getResponse.ok) {
        return { link, status: "ok" };
      }
      return {
        link,
        status: "broken",
        reason: `HTTP ${getResponse.status}`,
      };
    }

    return {
      link,
      status: "broken",
      reason: `HTTP ${response.status}`,
    };
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err);
    // Network errors are warnings (could be transient)
    return {
      link,
      status: "warning",
      reason: `Network error: ${message}`,
    };
  }
}

// ============================================================================
// Main
// ============================================================================

async function main() {
  const args = process.argv.slice(2);
  const checkExternal = args.includes("--external");
  const verbose = args.includes("--verbose");

  console.log("🔗 Documentation Link Checker\n");

  // Build valid slug set
  const validSlugs = buildValidSlugs();
  if (verbose) {
    console.log(`Found ${validSlugs.size} valid documentation pages\n`);
  }

  // Collect all links from MDX files
  const allLinks: LinkInfo[] = [];

  for (const dir of MDX_GLOB_DIRS) {
    const mdxFiles = walkDir(dir, ".mdx");
    for (const file of mdxFiles) {
      const content = fs.readFileSync(file, "utf-8");
      allLinks.push(...extractMarkdownLinks(content, file));
      allLinks.push(...extractJsxLinks(content, file));
    }
  }

  // Collect all links from TSX files
  for (const dir of TSX_GLOB_DIRS) {
    const tsxFiles = walkDir(dir, ".tsx");
    for (const file of tsxFiles) {
      const content = fs.readFileSync(file, "utf-8");
      allLinks.push(...extractJsxLinks(content, file));
    }
  }

  // Classify links
  const internalLinks: LinkInfo[] = [];
  const externalLinks: LinkInfo[] = [];

  for (const link of allLinks) {
    const type = classifyLink(link.url);
    if (type === "internal") {
      internalLinks.push(link);
    } else if (type === "external") {
      externalLinks.push(link);
    }
  }

  console.log(`Found ${internalLinks.length} internal links`);
  console.log(`Found ${externalLinks.length} external links\n`);

  // Check internal links
  const internalResults: CheckResult[] = [];
  for (const link of internalLinks) {
    const result = checkInternalLink(link, validSlugs);
    internalResults.push(result);
  }

  // Check external links (if requested)
  const externalResults: CheckResult[] = [];
  if (checkExternal) {
    console.log("Checking external links (this may take a while)...\n");

    // Deduplicate URLs to avoid hitting the same server repeatedly
    const uniqueUrls = new Map<string, LinkInfo[]>();
    for (const link of externalLinks) {
      const baseUrl = link.url.split("#")[0];
      if (!uniqueUrls.has(baseUrl)) {
        uniqueUrls.set(baseUrl, []);
      }
      uniqueUrls.get(baseUrl)!.push(link);
    }

    // Check in batches of 5 to avoid overwhelming servers
    const urlEntries = [...uniqueUrls.entries()];
    for (let i = 0; i < urlEntries.length; i += 5) {
      const batch = urlEntries.slice(i, i + 5);
      const batchResults = await Promise.all(
        batch.map(([, links]) => checkExternalLink(links[0]))
      );

      for (let j = 0; j < batch.length; j++) {
        const [, links] = batch[j];
        const result = batchResults[j];
        // Apply result to all links with same URL
        for (const link of links) {
          externalResults.push({ ...result, link });
        }
      }
    }
  }

  // Report results
  const brokenInternal = internalResults.filter((r) => r.status === "broken");
  const brokenExternal = externalResults.filter((r) => r.status === "broken");
  const warningExternal = externalResults.filter(
    (r) => r.status === "warning"
  );

  if (brokenInternal.length > 0) {
    console.log(
      `\n❌ Found ${brokenInternal.length} broken internal link(s):\n`
    );
    for (const result of brokenInternal) {
      const relPath = path.relative(REPO_ROOT, result.link.file);
      console.log(
        `  ${relPath}:${result.link.line}` +
          `\n    Link: ${result.link.url}` +
          `\n    ${result.reason}\n`
      );
    }
  }

  if (brokenExternal.length > 0) {
    console.log(
      `\n❌ Found ${brokenExternal.length} broken external link(s):\n`
    );
    for (const result of brokenExternal) {
      const relPath = path.relative(REPO_ROOT, result.link.file);
      console.log(
        `  ${relPath}:${result.link.line}` +
          `\n    Link: ${result.link.url}` +
          `\n    ${result.reason}\n`
      );
    }
  }

  if (warningExternal.length > 0) {
    console.log(
      `\n⚠️  ${warningExternal.length} external link(s) with warnings:\n`
    );
    for (const result of warningExternal) {
      const relPath = path.relative(REPO_ROOT, result.link.file);
      console.log(
        `  ${relPath}:${result.link.line}` +
          `\n    Link: ${result.link.url}` +
          `\n    ${result.reason}\n`
      );
    }
  }

  if (verbose) {
    const okInternal = internalResults.filter((r) => r.status === "ok");
    const okExternal = externalResults.filter((r) => r.status === "ok");
    console.log(`\n✅ ${okInternal.length} internal links OK`);
    if (checkExternal) {
      console.log(`✅ ${okExternal.length} external links OK`);
    }
  }

  // Summary
  const totalBroken = brokenInternal.length + brokenExternal.length;
  if (totalBroken === 0) {
    console.log("\n✅ All links are valid!");
    process.exit(0);
  } else {
    console.log(`\n💥 ${totalBroken} broken link(s) found.`);
    process.exit(1);
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
