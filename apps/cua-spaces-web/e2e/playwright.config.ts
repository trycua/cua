// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The web parity harness: `pnpm parity` (builds the wasm core first), in
// the browser against the dev server; `pnpm parity:electron` runs the same
// specs in the built Electron shell (`host.ts`).
import { defineConfig, devices } from "@playwright/test";

const ELECTRON = process.env.PARITY_HOST === "electron";

// PARITY_PORT: parallel worktrees each need their own server (a reused one
// on the default port may be another checkout's build).
const PORT = Number(process.env.PARITY_PORT ?? 5176);

export default defineConfig({
  testDir: ".",
  // Electron: fetch the binary once, before the workers (see host.ts).
  globalSetup: ELECTRON ? "./global-setup.ts" : undefined,
  outputDir: ELECTRON ? "../test-results/parity-electron" : "../test-results/parity",
  fullyParallel: true,
  // One shell per test; each is a full Electron app.
  workers: ELECTRON ? Number(process.env.PARITY_WORKERS ?? 2) : undefined,
  timeout: ELECTRON ? 90_000 : undefined,
  // One retry in CI. A test that passes only on it is "flaky" in Playwright's
  // report; CI lists it and opens an issue (.github/scripts/flaky/flaky.py).
  retries: process.env.CI ? 1 : 0,
  reporter: [
    ["list"],
    ["./summary-reporter.ts"],
    ...(process.env.FLAKY_RESULTS_OUT ? [["json", { outputFile: process.env.FLAKY_RESULTS_OUT }] as const] : []),
  ],
  use: {
    baseURL: `http://localhost:${PORT}`,
    viewport: { width: 1440, height: 900 },
    trace: "retain-on-failure",
  },
  projects: ELECTRON
    ? [{ name: "electron" }]
    : [{ name: "chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } }],
  webServer: ELECTRON ? undefined : {
    command: `npx vite --port ${PORT} --strictPort`,
    cwd: "..",
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    // The flows run on the new design (`spaces_redesign`); the classic pages
    // keep their own tests in src/legacy/__tests__.
    env: { VITE_CUA_FLAG_SPACES_REDESIGN: process.env.VITE_CUA_FLAG_SPACES_REDESIGN ?? "1" },
    timeout: 60_000,
  },
});
