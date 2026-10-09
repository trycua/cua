// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Performance budget: builds the app, reports JS and CSS sizes per route
// chunk and in total, measures time to first render in headless Chromium,
// and fails when anything is over the budget in perf-budget.json.
//
// Usage:
//   pnpm perf                 build, measure, check
//   pnpm perf --update        write new budgets: current sizes plus 15%
//   pnpm perf --no-build      reuse dist/ (it must have .vite/manifest.json)
//   pnpm perf --no-timing     sizes only
//
// A route's size is what opening it adds on top of the entry: its own chunk
// plus the shared chunks it imports that the entry doesn't already load.
import { spawn, spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const root = fileURLToPath(new URL("..", import.meta.url));
const dist = `${root}dist/`;
const budgetFile = `${root}perf-budget.json`;
const args = new Set(process.argv.slice(2));
const HEADROOM = 1.15;
const RUNS = 3;

if (!args.has("--no-build")) {
  const run = (cmd, argv) => {
    const r = spawnSync(cmd, argv, { cwd: root, stdio: ["ignore", "ignore", "inherit"] });
    if (r.status !== 0) process.exit(r.status ?? 1);
  };
  run("node", ["scripts/build-core-wasm.mjs"]);
  run("npx", ["vite", "build", "--manifest", "--logLevel", "warn"]);
}

/* ---- sizes ---------------------------------------------------------------- */

const manifest = JSON.parse(readFileSync(`${dist}.vite/manifest.json`, "utf8"));
const size = (file) => {
  const buf = readFileSync(dist + file);
  return { raw: buf.length, gzip: gzipSync(buf, { level: 9 }).length };
};
const add = (a, b) => ({ raw: a.raw + b.raw, gzip: a.gzip + b.gzip });
const ZERO = { raw: 0, gzip: 0 };

/** Every chunk key `key` loads up front (itself and its static imports). */
function closure(key, seen = new Set()) {
  if (seen.has(key)) return seen;
  seen.add(key);
  for (const dep of manifest[key]?.imports ?? []) closure(dep, seen);
  return seen;
}
const sum = (keys) => [...keys].reduce((acc, k) => add(acc, size(manifest[k].file)), ZERO);

const entryKey = Object.keys(manifest).find((k) => manifest[k].isEntry);
const entryKeys = closure(entryKey);
const routeName = (key) =>
  key
    .replace(/\?.*$/, "")
    .replace(/^src\/routes\//, "")
    .replace(/\/index\.tsx$|\.tsx$/, "");

const routes = Object.keys(manifest)
  .filter((k) => k.startsWith("src/routes/") && manifest[k].isDynamicEntry)
  .map((k) => ({ name: routeName(k), js: sum([...closure(k)].filter((d) => !entryKeys.has(d))) }))
  .sort((a, b) => a.name.localeCompare(b.name));

const jsFiles = new Set(Object.values(manifest).map((c) => c.file).filter((f) => f.endsWith(".js")));
const cssFiles = new Set(Object.values(manifest).flatMap((c) => c.css ?? []));
const wasmFiles = new Set(Object.values(manifest).map((c) => c.file).filter((f) => f.endsWith(".wasm")));
const filesSize = (files) => [...files].reduce((acc, f) => add(acc, size(f)), ZERO);

const sizes = {
  entry: sum(entryKeys),
  routes: Object.fromEntries(routes.map((r) => [r.name, r.js])),
  totalJs: filesSize(jsFiles),
  totalCss: filesSize(cssFiles),
  wasm: filesSize(wasmFiles),
};

/* ---- time to first render ------------------------------------------------- */

async function measureTiming() {
  const { chromium } = await import("@playwright/test");
  const port = 5176;
  const base = `http://localhost:${port}`;
  const server = spawn("npx", ["vite", "preview", "--port", String(port), "--strictPort"], { cwd: root, stdio: "ignore" });
  try {
    let up = false;
    for (let i = 0; i < 75 && !up; i++) {
      try {
        up = (await fetch(base)).ok;
      } catch {
        await new Promise((r) => setTimeout(r, 200));
      }
    }
    if (!up) throw new Error(`vite preview did not start on ${base}`);
    const browser = await chromium.launch();
    const out = {};
    try {
      for (const route of routes.map((r) => r.name).filter((n) => !n.includes("$"))) {
        const firstRender = [];
        const fcp = [];
        for (let i = 0; i < RUNS; i++) {
          // A fresh context each run, so nothing is cached.
          const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
          await context.addInitScript(() => {
            // The first time the page's <main> has content: the route rendered.
            new MutationObserver((_, observer) => {
              if (document.querySelector("main > *")) {
                window.__firstRender = performance.now();
                observer.disconnect();
              }
            }).observe(document, { childList: true, subtree: true });
          });
          const page = await context.newPage();
          await page.goto(`${base}/${route}`);
          await page.waitForFunction(() => window.__firstRender !== undefined, null, { timeout: 15_000 });
          await page.waitForFunction(() => performance.getEntriesByName("first-contentful-paint").length > 0, null, { timeout: 3_000 }).catch(() => {});
          const t = await page.evaluate(() => ({
            render: window.__firstRender,
            fcp: performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? null,
          }));
          firstRender.push(t.render);
          if (t.fcp !== null) fcp.push(t.fcp);
          await context.close();
        }
        out[route] = { firstRenderMs: Math.round(median(firstRender)), fcpMs: fcp.length ? Math.round(median(fcp)) : null };
      }
    } finally {
      await browser.close();
    }
    return out;
  } finally {
    server.kill();
  }
}

const median = (xs) => {
  const s = xs.toSorted((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
};

const timing = args.has("--no-timing") ? null : await measureTiming();

/* ---- report and check ----------------------------------------------------- */

const kb = (n) => `${(n / 1024).toFixed(1)} kB`;
const pad = (s, n) => String(s).padEnd(n);
const lpad = (s, n) => String(s).padStart(n);

if (args.has("--update")) {
  const up = (n) => Math.ceil((n * HEADROOM) / 1024) * 1024;
  const budget = {
    $comment: `Raw bytes. Budgets are the sizes on update plus ${Math.round((HEADROOM - 1) * 100)}%; run \`pnpm perf --update\` to reset them. firstRenderMs is loose because timing varies by machine.`,
    entry: up(sizes.entry.raw),
    routes: Object.fromEntries(Object.entries(sizes.routes).map(([k, v]) => [k, up(v.raw)])),
    totalJs: up(sizes.totalJs.raw),
    totalCss: up(sizes.totalCss.raw),
    wasm: up(sizes.wasm.raw),
    firstRenderMs: timing ? Math.ceil((Math.max(...Object.values(timing).map((t) => t.firstRenderMs)) * 2) / 50) * 50 : 1000,
  };
  writeFileSync(budgetFile, `${JSON.stringify(budget, null, 2)}\n`);
  console.log(`Wrote ${budgetFile}`);
}

const budget = JSON.parse(readFileSync(budgetFile, "utf8"));
const failures = [];
const warnings = [];
const rows = [];
const row = (name, s, limit) => {
  const over = limit !== undefined && s.raw > limit;
  if (over) failures.push(`${name} is ${kb(s.raw)}, over its ${kb(limit)} budget`);
  if (limit === undefined) warnings.push(`${name} has no budget; run \`pnpm perf --update\` to add one`);
  rows.push([name, kb(s.raw), kb(s.gzip), limit === undefined ? "none" : kb(limit), over ? "OVER" : limit === undefined ? "" : `${Math.round((s.raw / limit) * 100)}%`]);
};

row("entry (shell, router, bridge)", sizes.entry, budget.entry);
for (const [name, s] of Object.entries(sizes.routes)) row(`route ${name}`, s, budget.routes?.[name]);
row("total JS", sizes.totalJs, budget.totalJs);
row("total CSS", sizes.totalCss, budget.totalCss);
row("app core wasm", sizes.wasm, budget.wasm);

const widths = [34, 11, 11, 11, 6];
console.log(`\n${pad("Chunk", widths[0])}${lpad("Size", widths[1])}${lpad("Gzip", widths[2])}${lpad("Budget", widths[3])}${lpad("Used", widths[4] + 2)}`);
for (const r of rows) console.log(`${pad(r[0], widths[0])}${lpad(r[1], widths[1])}${lpad(r[2], widths[2])}${lpad(r[3], widths[3])}${lpad(r[4], widths[4] + 2)}`);

if (timing) {
  console.log(`\n${pad("Time to first render (median of " + RUNS + ")", widths[0])}${lpad("Render", widths[1])}${lpad("FCP", widths[2])}${lpad("Budget", widths[3])}`);
  for (const [route, t] of Object.entries(timing)) {
    const over = budget.firstRenderMs !== undefined && t.firstRenderMs > budget.firstRenderMs;
    if (over) failures.push(`/${route} first rendered after ${t.firstRenderMs} ms, over the ${budget.firstRenderMs} ms budget`);
    console.log(`${pad("/" + route, widths[0])}${lpad(t.firstRenderMs + " ms", widths[1])}${lpad(t.fcpMs === null ? "n/a" : t.fcpMs + " ms", widths[2])}${lpad((budget.firstRenderMs ?? "none") + " ms", widths[3])}${over ? "  OVER" : ""}`);
  }
}

for (const w of warnings) console.log(`\nNote: ${w}`);
if (failures.length) {
  console.error(`\nOver budget:\n${failures.map((f) => `  ${f}`).join("\n")}`);
  process.exit(1);
}
console.log("\nWithin budget.");
