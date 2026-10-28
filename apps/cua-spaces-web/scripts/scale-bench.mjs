// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Spaces page at scale: the production build in headless Chromium with
// the demo host listing N more Spaces (`?demo=scale-N`). For each N it
// reports, as the median of RUNS loads:
//   firstTile   navigation start to the first Space tile drawn
//   filterKey   one character typed in the filter to the grid redrawn
//   selectAll   ⌘A (select every Space shown) to the selection bar drawn
//   pickOne     one Space's checkbox clicked to the selection bar drawn
//   longTasks   main-thread tasks over 50 ms while the page settles, total ms
//   drawn       tiles in the page: one screen of rows once the list is long (windowing)
// and fails when a number is over its budget in scale-budget.json.
//
// Usage:
//   node scripts/scale-bench.mjs              build, measure, check
//   node scripts/scale-bench.mjs --no-build   reuse dist/
//   node scripts/scale-bench.mjs --sizes 100,1000
//   node scripts/scale-bench.mjs --update     write budgets: current numbers plus 50%
import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const argv = process.argv.slice(2);
const has = (f) => argv.includes(f);
const opt = (f, d) => (argv.includes(f) ? argv[argv.indexOf(f) + 1] : d);
const SIZES = opt("--sizes", "100,500,1000").split(",").map(Number);
const RUNS = Number(opt("--runs", "3"));
const PORT = Number(opt("--port", "5299"));
const budgetFile = `${root}scale-budget.json`;

if (!has("--no-build")) {
  for (const [cmd, a] of [
    ["node", ["scripts/build-core-wasm.mjs"]],
    ["npx", ["vite", "build", "--logLevel", "warn"]],
  ]) {
    const r = spawnSync(cmd, a, { cwd: root, stdio: ["ignore", "ignore", "inherit"], env: process.env });
    if (r.status !== 0) process.exit(r.status ?? 1);
  }
}

const server = spawn("npx", ["vite", "preview", "--port", String(PORT), "--strictPort"], { cwd: root, stdio: "ignore" });
const stop = () => server.kill("SIGTERM");
process.on("exit", stop);
for (let i = 0; i < 100; i++) {
  try {
    if ((await fetch(`http://localhost:${PORT}/`)).ok) break;
  } catch {}
  await new Promise((r) => setTimeout(r, 200));
}

const { chromium } = await import("@playwright/test");
const browser = await chromium.launch();
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];

async function once(n) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  await page.addInitScript(() => {
    window.__long = 0;
    new PerformanceObserver((l) => l.getEntries().forEach((e) => (window.__long += e.duration))).observe({ type: "longtask", buffered: true });
  });
  await page.goto(`http://localhost:${PORT}/spaces?bridge=demo&demo=scale-${n}`);
  await page.locator("[data-space-id]").first().waitFor({ timeout: 60_000 });
  const firstTile = await page.evaluate(() => performance.now());
  // Settled: no long task for a second.
  await page.waitForTimeout(1500);
  const longTasks = await page.evaluate(() => window.__long);
  // Tiles drawn: all of them for a short list, one screen of rows for a long one.
  const total = await page.locator("[data-space-id]").count();

  // One character in the filter, to the grid redrawn with fewer tiles.
  const filter = page.locator("[data-spaces-filter]").first();
  await filter.click();
  // One Space's name: the list drops to that one, so the first tile and the count change.
  const name = `scale-${String(n - 1).padStart(4, "0")}`;
  const filterKey = await page.evaluate(
    (name) =>
      new Promise((resolve) => {
        // The grid draws only what is on screen, so the number of tiles may not change: the
        // first tile and the "N Spaces" line do.
        const sig = () => `${document.querySelector("[data-space-id]")?.getAttribute("data-space-id")}|${document.querySelectorAll("[data-space-id]").length}|${document.querySelector("h1")?.parentElement?.textContent}`;
        const before = sig();
        const input = document.activeElement;
        const t0 = performance.now();
        const obs = new MutationObserver(() => {
          if (sig() !== before) {
            obs.disconnect();
            requestAnimationFrame(() => resolve(performance.now() - t0));
          }
        });
        obs.observe(document.body, { childList: true, subtree: true });
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
        setter.call(input, name);
        input.dispatchEvent(new Event("input", { bubbles: true }));
        setTimeout(() => resolve(-1), 10_000);
      }),
    name,
  );
  await filter.fill("");
  await page.waitForTimeout(500);

  // ⌘A on the grid: every Space shown picked, to the selection bar drawn.
  await page.locator("main").first().click({ position: { x: 5, y: 5 } }).catch(() => {});
  const t0 = await page.evaluate(() => performance.now());
  await page.keyboard.press("Meta+a");
  await page.locator("[data-selection-bar], [data-picked-count]").first().waitFor({ timeout: 10_000 }).catch(() => {});
  const selectAll = (await page.evaluate(() => performance.now())) - t0;
  await page.keyboard.press("Escape");
  await page.waitForTimeout(500);

  // Picking one Space (its checkbox), to the selection bar drawn.
  const pickOne = await page.evaluate(
    () =>
      new Promise((resolve) => {
        const box = document.querySelectorAll("[data-select]")[5];
        const t0 = performance.now();
        const obs = new MutationObserver(() => {
          if (document.querySelector("[data-selection-bar]")) {
            obs.disconnect();
            requestAnimationFrame(() => resolve(performance.now() - t0));
          }
        });
        obs.observe(document.body, { childList: true, subtree: true });
        box.dispatchEvent(new MouseEvent("click", { bubbles: true }));
        setTimeout(() => resolve(-1), 10_000);
      }),
  );
  await page.close();
  return { firstTile, filterKey, selectAll, pickOne, longTasks, drawn: total, total };
}

const results = {};
for (const n of SIZES) {
  const runs = [];
  for (let i = 0; i < RUNS; i++) runs.push(await once(n));
  results[n] = Object.fromEntries(["firstTile", "filterKey", "selectAll", "pickOne", "longTasks", "drawn"].map((k) => [k, Math.round(median(runs.map((r) => r[k])))]));
  results[n].tiles = runs[0].total;
  console.log(`N=${n}`, JSON.stringify(results[n]));
}
await browser.close();
stop();

if (has("--update")) {
  const budget = Object.fromEntries(Object.entries(results).map(([n, r]) => [n, Object.fromEntries(Object.entries(r).filter(([k]) => k !== "tiles").map(([k, v]) => [k, Math.max(k === "drawn" ? 20 : 50, Math.round(v * 1.5))]))]));
  writeFileSync(budgetFile, JSON.stringify(budget, null, 2) + "\n");
  console.log(`wrote ${budgetFile}`);
} else if (existsSync(budgetFile)) {
  const budget = JSON.parse(readFileSync(budgetFile, "utf8"));
  const over = [];
  for (const [n, r] of Object.entries(results))
    for (const [k, v] of Object.entries(budget[n] ?? {})) {
      // -1: the redraw never came (a measurement that timed out is a failure, not a fast run).
      if (r[k] < 0) over.push(`N=${n} ${k} was not measured`);
      else if (r[k] > v) over.push(`N=${n} ${k} ${r[k]} > ${v}`);
    }
  if (over.length) {
    console.error(`over budget:\n  ${over.join("\n  ")}`);
    process.exit(1);
  }
  console.log("within budget");
}
