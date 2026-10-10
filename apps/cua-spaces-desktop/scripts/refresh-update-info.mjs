#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `node scripts/refresh-update-info.mjs <file>...`
 *
 * After an installer electron-builder already made is changed (the release
 * signs, notarizes and staples the macOS disk images, which changes their
 * bytes), rebuilds each file's blockmap (`<file>.blockmap`, electron-builder's
 * own `buildBlockMap`) and updates its `sha512` and `size` (and
 * `blockMapSize`) in the update feed files next to it (dist/*.yml), so the
 * feed describes the file that is published. Fails when a file is not named
 * by any feed file.
 */
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, join } from "node:path";

const require = createRequire(import.meta.url);
/** app-builder-lib (electron-builder's), which this project does not depend on directly. */
function builderLib(subpath) {
  const builder = require.resolve("electron-builder/package.json");
  const lib = dirname(require.resolve("app-builder-lib/package.json", { paths: [dirname(builder)] }));
  return require(join(lib, subpath));
}

export async function refresh(files, { buildBlockMap = builderLib("out/targets/blockmap/blockmap.js").buildBlockMap, yaml = require(require.resolve("js-yaml", { paths: [dirname(require.resolve("electron-builder/package.json"))] })) } = {}) {
  for (const file of files) {
    const info = await buildBlockMap(file, "gzip", `${file}.blockmap`);
    const name = basename(file);
    const dir = dirname(file);
    let found = false;
    for (const feed of readdirSync(dir).filter((f) => f.endsWith(".yml"))) {
      const path = join(dir, feed);
      const doc = yaml.load(readFileSync(path, "utf8"));
      let changed = false;
      for (const entry of doc?.files ?? []) {
        if (entry.url !== name) continue;
        entry.sha512 = info.sha512;
        entry.size = info.size;
        if (entry.blockMapSize !== undefined || info.blockMapSize !== undefined) entry.blockMapSize = info.blockMapSize;
        changed = true;
      }
      if (doc?.path === name) {
        doc.sha512 = info.sha512;
        changed = true;
      }
      if (changed) {
        writeFileSync(path, yaml.dump(doc, { lineWidth: 8000 }));
        found = true;
        console.log(`${feed}: ${name} sha512 ${info.sha512.slice(0, 16)}… size ${info.size}`);
      }
    }
    if (!found) throw new Error(`no update feed file in ${dir} names ${name}`);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const files = process.argv.slice(2);
  if (!files.length) {
    console.error("usage: refresh-update-info.mjs <file>...");
    process.exit(2);
  }
  await refresh(files);
}
