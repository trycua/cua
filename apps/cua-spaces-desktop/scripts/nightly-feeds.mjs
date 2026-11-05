#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The electron-updater feeds of a Windows and Linux nightly
// (nightly-cua-spaces.yml). electron-builder writes nightly.yml (one per
// Windows architecture), nightly-linux.yml and nightly-linux-arm64.yml for an
// X.Y.Z-nightly.YYYYMMDD.N version (electron-builder.config.cjs). This joins the
// Windows ones (merge-feeds.mjs), points every installer at the rolling
// `cua-spaces-nightly` release (feed-urls.mjs), and refuses anything that
// could reach a Stable app: a latest*.yml or beta*.yml, a version that is not
// a nightly, or an installer URL outside the nightly release.
//
//   node scripts/nightly-feeds.mjs --base https://github.com/OWNER/REPO/releases/download/cua-spaces-nightly \
//     --version 0.21.0-nightly.20261105.42 --out DIR PART_DIR...
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { absolutize } from "./feed-urls.mjs";
import { mergeFeeds } from "./merge-feeds.mjs";

/** The feed files a nightly may publish (Windows, Linux x64, Linux arm64). */
export const NIGHTLY_FEEDS = ["nightly.yml", "nightly-linux.yml", "nightly-linux-arm64.yml"];

// X.Y.Z-nightly.YYYYMMDD.<run number>: unique and ordered (numeric identifiers, no leading zero).
const NIGHTLY_VERSION = /^\d+\.\d+\.\d+-nightly\.\d{8}\.(?:0|[1-9]\d*)$/;

/** Why `text`, the feed `name`, must not be published on the nightly release `base` (empty: it may). */
export function nightlyFeedProblems(name, text, base, version) {
  const problems = [];
  if (!NIGHTLY_FEEDS.includes(name)) problems.push(`${name} is not a nightly feed file`);
  const root = base.endsWith("/") ? base : `${base}/`;
  if (!/\/releases\/download\/cua-spaces-nightly\/$/.test(root)) problems.push(`${base} is not the cua-spaces-nightly release`);
  const v = /^version:\s*['"]?([^'"\n]+?)['"]?\s*$/m.exec(text)?.[1];
  if (!v || !NIGHTLY_VERSION.test(v)) problems.push(`${name}: version ${v ?? "(none)"} is not X.Y.Z-nightly.YYYYMMDD.N`);
  else if (version && v !== version) problems.push(`${name}: version ${v}, want ${version}`);
  const urls = [...text.matchAll(/^\s*(?:- )?(?:url|path):\s*['"]?([^'"\n]+?)['"]?\s*$/gm)].map((m) => m[1]);
  if (urls.length === 0) problems.push(`${name} names no installer`);
  for (const url of urls) if (!url.startsWith(root)) problems.push(`${name}: ${url} is outside ${root}`);
  return problems;
}

/** Every file under `dirs` (one level deep) by name: name -> [paths]. */
function filesByName(dirs) {
  const found = new Map();
  for (const dir of dirs) {
    for (const name of readdirSync(dir)) {
      if (!found.has(name)) found.set(name, []);
      found.get(name).push(path.join(dir, name));
    }
  }
  return found;
}

/** Writes the publishable nightly feeds of `partDirs` into `out`; returns their names. */
export function prepareNightlyFeeds({ base, version, out, partDirs }) {
  const files = filesByName(partDirs);
  const stray = [...files.keys()].filter((n) => /^(latest|beta)[\w-]*\.yml$/.test(n));
  if (stray.length) throw new Error(`a nightly build wrote ${stray.join(", ")}: its channel is not nightly (electron-builder.config.cjs)`);
  mkdirSync(out, { recursive: true });
  const written = [];
  for (const name of NIGHTLY_FEEDS) {
    const sources = files.get(name) ?? [];
    if (sources.length === 0) continue;
    // Windows: one feed per architecture, joined; Linux: one per architecture already.
    const text = absolutize(sources.length > 1 ? mergeFeeds(sources.map((f) => readFileSync(f, "utf8"))) : readFileSync(sources[0], "utf8"), base);
    const problems = nightlyFeedProblems(name, text, base, version);
    if (problems.length) throw new Error(problems.join("\n"));
    writeFileSync(path.join(out, name), text);
    written.push(name);
  }
  return written;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const opt = (flag) => {
    const i = args.indexOf(flag);
    if (i < 0 || !args[i + 1]) {
      console.error("usage: nightly-feeds.mjs --base URL --version V --out DIR PART_DIR...");
      process.exit(2);
    }
    const value = args[i + 1];
    args.splice(i, 2);
    return value;
  };
  const base = opt("--base");
  const version = opt("--version");
  const out = opt("--out");
  const written = prepareNightlyFeeds({ base, version, out, partDirs: args });
  console.log(written.length ? written.join("\n") : "no nightly feed");
}
