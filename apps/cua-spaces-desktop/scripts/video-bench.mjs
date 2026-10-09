// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The video bench for both apps on one Space, measured the same way
// (docs/video.md): launches the app with its bench hooks
// (CUA_SPACES_VIDEO_BENCH: the Spaces grid plus that Space's viewer;
// CUA_SPACES_VIDEO_STATS: the streams' counts once a second), waits for the
// streams to warm up, samples CPU and memory of the app's processes over the
// window, and writes the report (src/video-report.ts) for that window.
//
//   node scripts/video-bench.mjs electron --app "<Cua Spaces.app>/Contents/MacOS/Cua Spaces" --space <id> [--seconds 30] [--warmup 12] [--out report.json]
//   node scripts/video-bench.mjs swift    --app <path>/.build/debug/CuaSpacesMac           --space <id> [...]
//
// The Swift hooks are in debug builds (`swift build` in apps/cua-spaces-macos).
// Run it as the user who uses the apps (their real Spaces, their daemon);
// quit the other app first, so only one draws video at a time.
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { videoReport } from "../src/video-report.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const kind = process.argv[2];
const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const exe = arg("app");
const space = arg("space");
const seconds = Number(arg("seconds", "30"));
const warmup = Number(arg("warmup", "12"));
const out = arg("out", `video-bench-${kind}-${process.platform}.json`);
if ((kind !== "electron" && kind !== "swift") || !exe || !space) {
  console.error("usage: node scripts/video-bench.mjs electron|swift --app <executable> --space <spaceId> [--seconds 30] [--warmup 12] [--out file]");
  process.exit(2);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dir = mkdtempSync(path.join(os.tmpdir(), "cua-video-bench-"));
const statsFile = path.join(dir, "stats.json");
const env = { ...process.env, CUA_SPACES_VIDEO_BENCH: space, CUA_SPACES_VIDEO_STATS: statsFile };
delete env.ELECTRON_RUN_AS_NODE;
// The Swift app shows the web UI window (the grid) as its main window.
if (kind === "swift") env.CUA_SPACES_START_VIEW = "webui";
const started = Date.now();
const child = spawn(exe, [], { env, stdio: "ignore" });
const read = () => {
  try {
    return JSON.parse(readFileSync(statsFile, "utf8"));
  } catch {
    return { t: 0, streams: [] };
  }
};

/** pid, start time (ms) and command of every process (macOS, Linux). */
function processes() {
  if (process.platform === "win32") return [];
  return execFileSync("ps", ["-axww", "-o", "pid=,ppid=,lstart=,comm="], { encoding: "utf8" })
    .split("\n")
    .map((l) => /^\s*(\d+)\s+(\d+)\s+(\w+\s+\w+\s+\d+\s+[\d:]+\s+\d+)\s+(.*)$/.exec(l))
    .filter(Boolean)
    .map((m) => ({ pid: Number(m[1]), ppid: Number(m[2]), start: Date.parse(m[3]), comm: m[4] }));
}

/** The app and its helpers: its descendants, and (the Swift app) the WebKit services launchd started for it. */
function appProcesses() {
  const ps = processes();
  const pids = new Set([child.pid]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const p of ps) if (!pids.has(p.pid) && pids.has(p.ppid)) (pids.add(p.pid), (grew = true));
  }
  if (kind === "swift") for (const p of ps) if (/com\.apple\.WebKit\./.test(p.comm) && p.start >= started - 1500) pids.add(p.pid);
  return [...pids];
}

/** CPU (% of one core) and memory (MB) of `pids`, once a second for `n` seconds. */
async function sampleLoad(pids, n) {
  const cpu = [];
  const memoryMB = [];
  if (process.platform === "darwin") {
    const top = execFileSync("top", ["-l", String(n + 1), "-s", "1", "-stats", "pid,cpu,mem", ...pids.flatMap((p) => ["-pid", String(p)])], { encoding: "utf8", maxBuffer: 1 << 26 });
    const toMB = (s) => (/G/.test(s) ? parseFloat(s) * 1024 : /K/.test(s) ? parseFloat(s) / 1024 : parseFloat(s));
    // One block per sample; the first one's CPU counts since launch.
    for (const block of top.split(/^Processes:/m).slice(2)) {
      let c = 0;
      let m = 0;
      for (const line of block.split("\n")) {
        const f = line.trim().split(/\s+/);
        if (pids.includes(Number(f[0]))) {
          c += parseFloat(f[1]) || 0;
          m += toMB(f[2] || "0");
        }
      }
      cpu.push(c);
      memoryMB.push(m);
    }
    return { cpu, memoryMB };
  }
  const { processTable, tree, sum } = await import(path.join(here, "video/proc-sample.mjs"));
  let last = processTable();
  for (let i = 0; i < n; i++) {
    await sleep(1000);
    const now = processTable();
    const all = tree(now, child.pid);
    cpu.push((sum(now, all, "cpuSeconds") - sum(last, all, "cpuSeconds")) * 100);
    memoryMB.push(sum(now, all, "rssBytes") / (1 << 20));
    last = now;
  }
  return { cpu, memoryMB };
}

try {
  await sleep(warmup * 1000);
  const before = read();
  const load = await sampleLoad(appProcesses(), seconds);
  const after = read();
  if (!after.streams.length) throw new Error(`no streams in ${statsFile}: did ${kind} open ${space}? (the Swift hooks need a debug build)`);
  const report = { ...videoReport(kind, before, after, load), videoDecode: after.videoDecode ?? null, platform: process.platform, arch: process.arch, space, cores: os.cpus().length, loadavg: os.loadavg() };
  writeFileSync(out, `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify(report, null, 2));
} finally {
  child.kill();
  rmSync(dir, { recursive: true, force: true });
}
