/**
 * Vitest configuration for cua-driver TypeScript example tests.
 *
 * Parallelism strategy:
 * - pool: 'forks' — each test FILE runs in its own child_process fork.
 *   This is required for Node.js process APIs (child_process.spawn) used
 *   by DriverClient; 'threads' would share the process and cause issues.
 * - singleFork: false — each file gets its OWN fork (true = all files
 *   share one fork = serial).
 *
 * Test files:
 *   calculator.test.ts  — addition + multiplication tests against Calculator
 *   safari-form.test.ts — form-fill tests against Safari
 *
 * These two files use DIFFERENT macOS apps so they can safely run in
 * parallel.  Never run two files that target the same single-instance app
 * (e.g. Calculator) concurrently — they will interfere.
 *
 * Run:
 *   cd sdk/examples/typescript && npx vitest run
 *   cd sdk/examples/typescript && npx vitest run calculator.test.ts
 */

import { defineConfig } from 'vitest/config'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

export default defineConfig({
  resolve: {
    alias: {
      // Resolve @cua/driver-sdk directly to TS source (no build step needed)
      '@cua/driver-sdk': path.resolve(__dirname, '../../typescript/src/index.ts'),
    },
  },
  test: {
    // Only run the canonical merged test files.
    // calculator-addition.test.ts and calculator-multiplication.test.ts are
    // kept as standalone examples but excluded here — running them alongside
    // calculator.test.ts would launch three Calculator instances which conflict.
    include: ['calculator.test.ts', 'safari-form.test.ts'],
    // Each file in its own child-process fork → isolated cua-driver process
    pool: 'forks',
    poolOptions: {
      forks: {
        singleFork: false,   // true = all files share one fork (serial)
      },
    },
    // Run files sequentially to avoid macOS accessibility API contention.
    // Individual files can run in parallel if targeting completely independent
    // apps, but cua-driver startup + AX tree queries can conflict when
    // multiple driver processes run simultaneously on the same machine.
    fileParallelism: false,
    // Per-test timeout: UI interactions can take several seconds each
    testTimeout: 60_000,
    // Per-hook timeout: beforeAll kills + opens Safari (may need ~30s), afterAll force-kills
    hookTimeout: 60_000,
    // Verbose output
    reporters: ['verbose'],
    // Do NOT use threads — DriverClient.spawn() needs full process APIs
    // pool: 'threads' would break readline / child_process
  },
})
