#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Joins the electron-updater feed files of the per-architecture Windows
// builds. electron-builder lists every installer of one build in one
// latest.yml / beta.yml (`files:`); the release packs x64 and arm64 in
// separate jobs, so each wrote a feed naming its own installer. The result is
// the first feed (its top-level `path`, `sha512`, `releaseDate`) with the
// `files:` entries of all of them, in the order given.
//
//   node scripts/merge-feeds.mjs --out latest.yml x64/latest.yml arm64/latest.yml
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const FILES_BLOCK = /^files:[ \t]*\n((?:[ \t]+.*\n|[ \t]*\n)*)/m;

/** The `files:` entries of a feed (its indented lines, each entry starting at `- `). */
export function entries(text) {
  const normalized = text.endsWith("\n") ? text : `${text}\n`;
  const match = FILES_BLOCK.exec(normalized);
  if (!match) throw new Error("no files: block in the feed");
  const items = [];
  for (const line of match[1].split("\n")) {
    if (!line.trim()) continue;
    if (/^\s*- /.test(line) || items.length === 0) items.push([line]);
    else items[items.length - 1].push(line);
  }
  return items.map((lines) => lines.join("\n"));
}

/** The first feed with the `files:` entries of every feed (duplicates by url left out). */
export function mergeFeeds(texts) {
  if (texts.length === 0) throw new Error("no feeds to merge");
  const first = texts[0].endsWith("\n") ? texts[0] : `${texts[0]}\n`;
  const seen = new Set();
  const merged = [];
  for (const text of texts) {
    for (const entry of entries(text)) {
      const url = /url:\s*(\S+)/.exec(entry)?.[1];
      if (!url) throw new Error(`a files: entry has no url:\n${entry}`);
      if (seen.has(url)) continue;
      seen.add(url);
      merged.push(entry);
    }
  }
  return first.replace(FILES_BLOCK, () => `files:\n${merged.join("\n")}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const at = args.indexOf("--out");
  if (at < 0 || !args[at + 1]) {
    console.error("usage: merge-feeds.mjs --out FILE FEED...");
    process.exit(2);
  }
  const out = args[at + 1];
  const feeds = args.filter((_, i) => i !== at && i !== at + 1);
  if (feeds.length === 0) {
    console.error("no feed files given");
    process.exit(2);
  }
  writeFileSync(out, mergeFeeds(feeds.map((file) => readFileSync(file, "utf8"))));
  console.log(`${out}: ${feeds.length} feeds`);
}
