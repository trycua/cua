#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Reads the Electron fuses back from packaged builds and checks them against
// packaging/fuses.cjs. Node built-ins only, so it also runs on a test machine
// next to an installed app (copy this file and packaging/fuses.cjs, keeping
// the layout).
//
//   node scripts/check-fuses.mjs                 every build under dist/
//   node scripts/check-fuses.mjs <app> [<app>…]  a .app, an .exe or the Linux binary
//
// Exits 1 when a build has a fuse in the wrong state, or no fuse wire.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const { RELEASE_FUSES } = createRequire(import.meta.url)("../packaging/fuses.cjs");

// @electron/fuses' sentinel; the wire follows it: version, length, one byte per fuse.
const SENTINEL = Buffer.from("dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX");
const STATE = { 0x30: "off", 0x31: "on", 0x72: "removed", 0x90: "inherit" };

/** The file that carries the fuse wire. */
export function fuseFile(app) {
  if (app.endsWith(".app")) return path.join(app, "Contents/Frameworks/Electron Framework.framework/Electron Framework");
  return app;
}

/** The fuse wire in `buf`: `{ version, states }` (one word per fuse), or null. */
export function readWire(buf) {
  const at = buf.indexOf(SENTINEL);
  if (at < 0) return null;
  const version = buf[at + SENTINEL.length];
  const length = buf[at + SENTINEL.length + 1];
  const start = at + SENTINEL.length + 2;
  const states = [...buf.subarray(start, start + length)].map((b) => STATE[b] ?? `0x${b.toString(16)}`);
  return { version, states };
}

/** Problems with one build's wire (empty when it is as packaging/fuses.cjs says). */
export function checkWire(wire) {
  if (!wire) return ["no fuse wire found"];
  if (wire.version !== 1) return [`fuse wire version ${wire.version}, expected 1`];
  const problems = [];
  for (const [name, { index, on }] of Object.entries(RELEASE_FUSES)) {
    const want = on ? "on" : "off";
    const got = wire.states[index] ?? "missing";
    if (got !== want) problems.push(`${name}: ${got}, expected ${want}`);
  }
  return problems;
}

/** Packaged apps under dist/: mac*\/*.app, win*-unpacked/*.exe, linux*-unpacked/<executable>. */
function findBuilds(dist) {
  if (!existsSync(dist)) return [];
  const out = [];
  for (const dir of readdirSync(dist)) {
    const full = path.join(dist, dir);
    if (!statSync(full).isDirectory()) continue;
    if (dir.startsWith("mac")) out.push(...readdirSync(full).filter((f) => f.endsWith(".app")).map((f) => path.join(full, f)));
    else if (dir.startsWith("win") && dir.endsWith("-unpacked")) out.push(...readdirSync(full).filter((f) => f.endsWith(".exe") && !/^Uninstall/i.test(f)).map((f) => path.join(full, f)));
    else if (dir.startsWith("linux") && dir.endsWith("-unpacked") && existsSync(path.join(full, "cua-spaces"))) out.push(path.join(full, "cua-spaces"));
  }
  return out;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const apps = process.argv.length > 2 ? process.argv.slice(2) : findBuilds(path.join(here, "../dist"));
  if (apps.length === 0) {
    console.error("check-fuses: no packaged build found (run a dist script first, or pass a path)");
    process.exit(1);
  }
  let failed = false;
  for (const app of apps) {
    const wire = readWire(readFileSync(fuseFile(app)));
    const problems = checkWire(wire);
    const names = Object.entries(RELEASE_FUSES).map(([n, { index }]) => `${n}=${wire?.states[index] ?? "?"}`);
    console.log(`${problems.length ? "FAIL" : "ok  "} ${app}\n     ${names.join(" ")}`);
    for (const p of problems) console.log(`     ${p}`);
    failed ||= problems.length > 0;
  }
  process.exit(failed ? 1 : 0);
}
