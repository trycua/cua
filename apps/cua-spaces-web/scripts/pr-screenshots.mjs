// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Key screens for pull request review, at a desktop and a mobile size, from the
// demo adapter only (never a real account, host or chat). Run by
// .github/workflows/ci-pr-screenshots.yml on the PR head and its merge-base.
//
// Usage: pnpm build && node scripts/pr-screenshots.mjs
//   OUT_DIR=<dir>          where the PNGs and manifest.json go (default: pr-screenshots/)
//   BASE_URL=<url>         use a running server instead of starting `vite preview`
//   EXTRA_ROUTES=/a,/b     more routes to capture (paths only, from the PR description)
//
// A screen that cannot be reached (it does not exist at the merge-base, or its
// controls changed) is recorded in manifest.json with its error and no image;
// the comment shows "not available" for it. It never fails the run.
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const root = fileURLToPath(new URL("..", import.meta.url));
const outDir = process.env.OUT_DIR ?? `${root}pr-screenshots`;

const SIZES = [
  { id: "desktop", viewport: { width: 1440, height: 900 } },
  { id: "mobile", viewport: { width: 390, height: 844 } },
];

/** A path from the PR description is data: accept only a plain app path. */
const SAFE_ROUTE = /^\/[A-Za-z0-9/_\-.:$%=&?]*$/;
const extraRoutes = (process.env.EXTRA_ROUTES ?? "")
  .split(",")
  .map((r) => r.trim())
  .filter((r) => r && r !== "/" && SAFE_ROUTE.test(r) && !r.includes(".."))
  .slice(0, 6);

const slug = (route) => `route-${route.replace(/[^A-Za-z0-9]+/g, "-").replace(/^-|-$/g, "").toLowerCase()}`;

const onboarding = async (page, base, steps) => {
  await page.goto(`${base}/onboarding?demo=fresh`, { waitUntil: "networkidle" });
  for (const step of steps) await step(page);
};

/** The fixed list. Each screen: id, label, and how to reach it. */
const SCREENS = [
  { id: "first-run-welcome", label: "First run: welcome", go: (p, b) => onboarding(p, b, []) },
  {
    id: "first-run-sign-in",
    label: "First run: add this computer and sign in",
    go: (p, b) => onboarding(p, b, [(pg) => pg.getByRole("button", { name: "Add this computer" }).click({ timeout: 8000 })]),
  },
  { id: "home", label: "Home", go: (p, b) => p.goto(`${b}/home`, { waitUntil: "networkidle" }) },
  { id: "computers", label: "Computers", go: (p, b) => p.goto(`${b}/spaces`, { waitUntil: "networkidle" }) },
  { id: "settings", label: "Settings", go: (p, b) => p.goto(`${b}/settings`, { waitUntil: "networkidle" }) },
  ...extraRoutes.map((route) => ({
    id: slug(route),
    label: `Route ${route}`,
    go: (p, b) => p.goto(`${b}${route}`, { waitUntil: "networkidle" }),
  })),
];

let server;
let base = process.env.BASE_URL;
if (!base) {
  const port = Number(process.env.PORT ?? 5177);
  base = `http://localhost:${port}`;
  server = spawn("npx", ["vite", "preview", "--port", String(port), "--strictPort"], { cwd: root, stdio: "ignore" });
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try {
      up = (await fetch(base)).ok;
    } catch {}
    if (!up) await new Promise((r) => setTimeout(r, 200));
  }
  if (!up) {
    server.kill();
    throw new Error(`vite preview did not start on ${base}`);
  }
}

await mkdir(outDir, { recursive: true });
const browser = await chromium.launch();
const manifest = [];
try {
  for (const size of SIZES) {
    // Fixed look: light theme, reduced motion, 1x pixels, so base and head differ only by the change.
    const context = await browser.newContext({
      viewport: size.viewport,
      deviceScaleFactor: 1,
      colorScheme: "light",
      reducedMotion: "reduce",
      isMobile: size.id === "mobile",
      hasTouch: size.id === "mobile",
    });
    await context.addInitScript(() => localStorage.setItem("cua-spaces:theme", "light"));
    for (const screen of SCREENS) {
      const page = await context.newPage();
      const file = `${screen.id}-${size.id}.png`;
      const entry = { id: screen.id, label: screen.label, size: size.id, file: null, error: null };
      try {
        await screen.go(page, base);
        await page.waitForTimeout(400);
        await page.screenshot({ path: `${outDir}/${file}` });
        entry.file = file;
      } catch (e) {
        entry.error = String(e?.message ?? e).split("\n")[0].slice(0, 200);
        console.error(`${screen.id} (${size.id}): ${entry.error}`);
      }
      manifest.push(entry);
      await page.close();
    }
    await context.close();
  }
} finally {
  await browser.close();
  server?.kill();
}
await writeFile(`${outDir}/manifest.json`, JSON.stringify(manifest, null, 2));
const ok = manifest.filter((m) => m.file).length;
console.log(`Saved ${ok} of ${manifest.length} screenshots to ${outDir}`);
