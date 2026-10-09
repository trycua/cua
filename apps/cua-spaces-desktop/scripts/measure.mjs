// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Rough startup numbers for the packaged macOS app: wall time from spawn to
// `ready-to-show`, over a few launches. Run after `pnpm dist:mac:dir`.
//   node scripts/measure.mjs [runs]
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const bin = path.join(root, "dist/mac-arm64/Cua Spaces.app/Contents/MacOS/Cua Spaces");
const runs = Number(process.argv[2] ?? 5);

function launch() {
  const userData = mkdtempSync(path.join(os.tmpdir(), "cua-spaces-measure-"));
  const env = { ...process.env, CUA_SPACES_LOG_STARTUP: "1", CUA_SPACES_QUIT_AFTER_LOAD: "1", CUA_SPACES_USER_DATA: userData };
  delete env.ELECTRON_RUN_AS_NODE;
  const t0 = performance.now();
  return new Promise((resolve, reject) => {
    const child = spawn(bin, [], { env });
    let wall = null;
    let inProcess = null;
    child.stdout.on("data", (d) => {
      const m = /ready-to-show (\d+)ms/.exec(String(d));
      if (m && wall === null) {
        wall = performance.now() - t0;
        inProcess = Number(m[1]);
      }
    });
    child.on("exit", () => {
      rmSync(userData, { recursive: true, force: true });
      wall === null ? reject(new Error("no ready-to-show line")) : resolve({ wall, inProcess });
    });
  });
}

const results = [];
for (let i = 0; i < runs; i++) results.push(await launch());
for (const [i, r] of results.entries()) {
  console.log(`run ${i + 1}: ${r.wall.toFixed(0)} ms wall, ${r.inProcess} ms in process`);
}
const sorted = results.map((r) => r.wall).sort((a, b) => a - b);
console.log(`median wall-to-ready-to-show: ${sorted[Math.floor(sorted.length / 2)].toFixed(0)} ms`);
