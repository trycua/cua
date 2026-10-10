#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Points electron-updater feed files (beta-mac.yml, latest.yml, ...) at the
// release that holds the installers. electron-builder writes each file's
// `url` and `path` relative to the feed; the feed lives on the rolling
// `cua-spaces-latest` release while the installers live on their own
// `cua-spaces-v<version>` release, so the CI makes them absolute before it
// uploads the feed. Absolute URLs are left as they are.
//
//   node scripts/feed-urls.mjs --base https://github.com/OWNER/REPO/releases/download/cua-spaces-vX.Y.Z/ dist/beta-mac.yml ...
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** The feed text with every relative `url:` / `path:` value under `base`. */
export function absolutize(text, base) {
  if (!/^https:\/\//.test(base)) throw new Error(`--base must be an https URL, got ${base}`);
  const root = base.endsWith("/") ? base : `${base}/`;
  return text.replace(/^(\s*(?:- )?(?:url|path): )(['"]?)([^'"\n]+)\2$/gm, (line, key, quote, value) =>
    /^[a-z]+:\/\//i.test(value) ? line : `${key}${quote}${root}${encodeURIComponent(value)}${quote}`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const at = args.indexOf("--base");
  if (at < 0 || !args[at + 1]) {
    console.error("usage: feed-urls.mjs --base URL FILE...");
    process.exit(2);
  }
  const base = args[at + 1];
  const files = args.filter((_, i) => i !== at && i !== at + 1);
  if (files.length === 0) {
    console.error("no feed files given");
    process.exit(2);
  }
  for (const file of files) {
    const before = readFileSync(file, "utf8");
    const after = absolutize(before, base);
    if (!/^\s*(?:- )?url: https:\/\//m.test(after)) throw new Error(`${file} names no installer`);
    writeFileSync(file, after);
    console.log(`${file}: ${base}`);
  }
}
