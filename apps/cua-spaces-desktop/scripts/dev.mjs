// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `pnpm dev`: builds the main process and preload in watch mode, then runs
// Electron against the web dev server (http://localhost:5174 by default).
// If nothing is listening there and apps/cua-spaces-web exists, it starts
// that dev server too; otherwise the shell shows its placeholder page.
import { spawn } from "node:child_process";
import { existsSync, watch } from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import electronPath from "electron";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const webDir = path.resolve(root, "../cua-spaces-web");
const devUrl = process.env.CUA_SPACES_DEV_URL ?? "http://localhost:5174";
const children = new Set();

function run(cmd, args, opts = {}) {
  const child = spawn(cmd, args, { stdio: "inherit", ...opts });
  children.add(child);
  child.on("exit", () => children.delete(child));
  return child;
}

async function reachable(url) {
  try {
    await fetch(url, { signal: AbortSignal.timeout(1000) });
    return true;
  } catch {
    return false;
  }
}

async function waitFor(url, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await reachable(url)) return true;
    await new Promise((r) => setTimeout(r, 250));
  }
  return false;
}

if (!(await reachable(devUrl)) && existsSync(path.join(webDir, "package.json"))) {
  const port = new URL(devUrl).port || "5174";
  console.log(`[dev] starting the web dev server in ${webDir}`);
  run("pnpm", ["--dir", webDir, "exec", "vite", "--port", port, "--strictPort"], { cwd: webDir });
  if (!(await waitFor(devUrl, 30_000))) console.warn(`[dev] ${devUrl} did not come up; using the placeholder`);
} else if (!(await reachable(devUrl))) {
  console.log(`[dev] nothing at ${devUrl} and no apps/cua-spaces-web; using the placeholder page`);
}

await new Promise((resolve, reject) => {
  run("pnpm", ["exec", "tsdown"], { cwd: root }).on("exit", (code) =>
    code === 0 ? resolve() : reject(new Error("tsdown failed")),
  );
});
run("pnpm", ["exec", "tsdown", "--watch", "--no-clean", "--log-level", "warn"], { cwd: root });

// A parent that is itself Electron (an editor, an agent host) may leak
// ELECTRON_RUN_AS_NODE, which would start our app as plain Node.
function electronEnv(extra) {
  const env = { ...process.env, ...extra };
  delete env.ELECTRON_RUN_AS_NODE;
  return env;
}

let electron;
let restarting = false;
function startElectron() {
  electron = run(electronPath, [root], {
    cwd: root,
    env: electronEnv({ CUA_SPACES_DEV_URL: devUrl, CUA_SPACES_LOG_STARTUP: "1" }),
  });
  electron.on("exit", () => {
    if (!restarting) shutdown();
  });
}

let timer;
// The watcher's own first build lands right after launch; ignore it.
const watchFrom = Date.now() + 3000;
watch(path.join(root, "dist-electron"), (_event, file) => {
  if (!file?.endsWith(".cjs") || Date.now() < watchFrom) return;
  clearTimeout(timer);
  timer = setTimeout(() => {
    restarting = true;
    electron.once("exit", () => {
      restarting = false;
      startElectron();
    });
    electron.kill();
  }, 300);
});

function shutdown() {
  for (const c of children) c.kill();
  process.exit(0);
}
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

startElectron();
