#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# The native video harness (docs/native-video.md): the debug app on fixture
# Spaces with synthetic H.264 streams, a grid of tiles in the New UI window
# and one Space viewer in a second window.
#
#   scripts/native-video-harness.sh check [shots-dir]   # drive it, print results JSON
#   scripts/native-video-harness.sh measure [seconds]   # CPU / memory / fps, JSON
#
# measure options (environment): TILES=9 (tiles besides the fixtures'),
# TILE=tile30|tile10 (1280x800@30 tiles, or the production tile tier's
# 960x600@10), VIDEO=0 (the same windows with native video off:
# the baseline the video adds to).
#
# Needs: `swift build` (debug), `pnpm build` in apps/cua-spaces-web, and the
# streams from apps/cua-spaces-desktop/scripts/video/encode.sh. Runs in a
# throwaway HOME; touches no daemon and no real Space.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
streams="${STREAMS:-$here/../cua-spaces-desktop/scripts/video/.cache}"
tiles="${TILES:-9}"
bin="$here/.build/debug/CuaSpacesMac"
[ -x "$bin" ] || { echo "build first: swift build" >&2; exit 1; }
[ -f "$streams/tile30.h264" ] && [ -f "$streams/full60.h264" ] || { echo "missing streams in $streams (encode.sh)" >&2; exit 1; }
home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
mode="${1:-check}"

video=()
[ "${VIDEO:-1}" = 0 ] || video=(CUA_SPACES_SYNTHETIC_VIDEO="$streams" CUA_SPACES_SYNTHETIC_TILE="${TILE:-tile30}")

launch() {
  env HOME="$home" CUA_SPACES_FIXTURES=1 CUA_SPACES_FIXTURE_SPACES="$tiles" \
    ${video[@]+"${video[@]}"} CUA_SPACES_START_VIEW=webui \
    CUA_SPACES_VIDEO_BENCH=local:bench-1 "$@" "$bin" >"$home/app.log" 2>&1 &
  echo $!
}

case "$mode" in
check)
  out="$home/check.json"
  pid=$(launch CUA_SPACES_VIDEO_CHECK="$out" ${2:+CUA_SPACES_VIDEO_SHOT="$2"})
  for _ in $(seq 1 120); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
  kill "$pid" 2>/dev/null || true
  cat "$out"
  ;;
measure)
  seconds="${2:-30}"
  stats="$home/stats.json"
  pid=$(launch CUA_SPACES_VIDEO_STATS="$stats")
  sleep 12 # load, open every stream, warm up
  node - "$pid" "$seconds" "$stats" <<'JS'
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const [pid, seconds, statsFile] = process.argv.slice(2);
const read = () => { try { return JSON.parse(fs.readFileSync(statsFile, "utf8")); } catch { return { t: 0, streams: [] }; } };
// The app and the WebKit services it started: WebKit's XPC services are
// launchd's children, so take those that started after the app did.
function procs() {
  const ps = execFileSync("ps", ["-axww", "-o", "pid=,lstart=,comm="], { encoding: "utf8" }).split("\n").map((l) => {
    const m = l.trim().match(/^(\d+)\s+(\w+\s+\w+\s+\d+\s+[\d:]+\s+\d+)\s+(.*)$/);
    return m ? { pid: m[1], start: Date.parse(m[2]), comm: m[3] } : null;
  }).filter(Boolean);
  const app = ps.find((p) => p.pid === pid);
  const webkit = ps.filter((p) => /com\.apple\.WebKit\./.test(p.comm) && p.start >= (app?.start ?? 0) - 1500).map((p) => p.pid);
  return [pid, ...webkit];
}
const list = procs();
const before = read();
const t0 = Date.now();
const top = execFileSync("top", ["-l", String(Number(seconds) + 1), "-s", "1", "-stats", "pid,cpu,mem,command", ...list.flatMap((p) => ["-pid", p])], { encoding: "utf8", maxBuffer: 1 << 26 });
const after = read();
const dt = (Date.now() - t0) / 1000;
// top: one block per sample; skip the first (its CPU is since launch).
const blocks = top.split(/^Processes:/m).slice(2);
const cpu = [], mem = [];
const per = new Map();
const toMB = (s) => { const n = parseFloat(s); return /G/.test(s) ? n * 1024 : /K/.test(s) ? n / 1024 : n; };
for (const b of blocks) {
  let c = 0, m = 0;
  for (const line of b.split("\n")) {
    const f = line.trim().split(/\s+/);
    if (list.includes(f[0])) {
      c += parseFloat(f[1]) || 0; m += toMB(f[2] || "0");
      const name = f.slice(3).join(" ").replace(/^com\.apple\.WebKit\./, "WebKit.");
      const e = per.get(f[0]) ?? { name, cpu: [], mem: [] };
      e.cpu.push(parseFloat(f[1]) || 0); e.mem.push(toMB(f[2] || "0")); per.set(f[0], e);
    }
  }
  cpu.push(c); mem.push(m);
}
const avg = (a) => a.reduce((x, y) => x + y, 0) / Math.max(1, a.length);
const fps = after.streams.map((s) => {
  const b = before.streams.find((x) => x.spaceId === s.spaceId && x.tier === s.tier);
  return { spaceId: s.spaceId, tier: s.tier, fps: +(((s.presented - (b?.presented ?? 0)) / (after.t - before.t)) || 0).toFixed(1), decoded: s.decoded - (b?.decoded ?? 0) };
});
const load = execFileSync("sysctl", ["-n", "vm.loadavg"], { encoding: "utf8" }).trim();
console.log(JSON.stringify({ seconds: dt, processes: list.length, cpuPercent: +avg(cpu).toFixed(1), cpuMax: +Math.max(...cpu).toFixed(1), memoryMB: +avg(mem).toFixed(0), loadavg: load,
  perProcess: [...per.values()].map((e) => ({ name: e.name, cpu: +avg(e.cpu).toFixed(1), memoryMB: +avg(e.mem).toFixed(0) })),
  tiles: fps.filter((f) => f.tier === "tile"), full: fps.filter((f) => f.tier === "full") }, null, 2));
JS
  kill "$pid" 2>/dev/null || true
  ;;
*) echo "usage: $0 check [shots-dir] | measure [seconds]" >&2; exit 2 ;;
esac
