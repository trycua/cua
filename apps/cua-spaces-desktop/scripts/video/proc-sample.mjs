// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CPU time and memory of a process tree on Linux (/proc) and Windows
// (Win32_Process), for scripts/video-bench.mjs.
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";

/** pid → {ppid, cpuSeconds, rssBytes} for every process. */
export function processTable() {
  const table = new Map();
  if (process.platform === "linux") {
    const tick = 100;
    const page = 4096;
    for (const name of readdirSync("/proc")) {
      if (!/^\d+$/.test(name)) continue;
      try {
        const stat = readFileSync(`/proc/${name}/stat`, "utf8");
        // comm may hold spaces and parentheses; fields resume after the last ")".
        const f = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
        table.set(Number(name), { ppid: Number(f[1]), cpuSeconds: (Number(f[11]) + Number(f[12])) / tick, rssBytes: Number(f[21]) * page });
      } catch {
        // gone
      }
    }
  } else if (process.platform === "win32") {
    const out = execFileSync(
      "powershell",
      ["-NoProfile", "-Command", "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,KernelModeTime,UserModeTime,WorkingSetSize | ConvertTo-Json -Compress"],
      { encoding: "utf8", maxBuffer: 64 << 20 },
    );
    for (const p of JSON.parse(out)) {
      table.set(p.ProcessId, { ppid: p.ParentProcessId, cpuSeconds: (Number(p.KernelModeTime) + Number(p.UserModeTime)) / 1e7, rssBytes: Number(p.WorkingSetSize) });
    }
  } else {
    throw new Error("macOS: video-bench.mjs samples with top");
  }
  return table;
}

/** The root and all its descendants. */
export function tree(table, root) {
  const pids = new Set([root]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const [pid, p] of table) if (!pids.has(pid) && pids.has(p.ppid)) (pids.add(pid), (grew = true));
  }
  return [...pids].filter((pid) => table.has(pid));
}

export const sum = (table, pids, key) => pids.reduce((n, pid) => n + (table.get(pid)?.[key] ?? 0), 0);
