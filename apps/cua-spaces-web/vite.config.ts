// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fileURLToPath, URL } from "node:url";

import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const src = (path: string) => fileURLToPath(new URL(`./src/${path}`, import.meta.url));

export default defineConfig({
  plugins: [
    tanstackRouter({
      target: "react",
      autoCodeSplitting: true,
      // The generated route tree is source-available like the rest of the
      // app, so it carries the same header (scripts/spdx-headers.py checks it).
      routeTreeFileHeader: [
        "// SPDX-License-Identifier: FSL-1.1-MIT\n// Copyright (c) 2026 Cua AI, Inc.",
        "/* eslint-disable */",
        "// @ts-nocheck",
        "// noinspection JSUnusedGlobalSymbols",
      ],
    }),
    react(),
    babel({
      parserOpts: { plugins: ["typescript", "jsx"] },
      presets: [reactCompilerPreset()],
    }),
    tailwindcss(),
  ],
  resolve: {
    alias: [
      { find: /^@\//, replacement: `${src("")}` },
    ],
  },
  // Scan every source file up front: routes are code-split, so otherwise their
  // dependencies are found on first navigation and the dev server reloads the
  // page mid-run (parity flows then fail with "execution context destroyed").
  optimizeDeps: { entries: ["index.html", "src/**/*.{ts,tsx}", "!src/**/*.d.ts", "!src/**/__tests__/**", "!src/**/*.test.{ts,tsx}"] },
  server: { port: 5174, strictPort: true },
  preview: { port: 5174 },
  build: { target: "es2023", sourcemap: true },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    // The app-level tests mount the real routes on the wasm core; a CI
    // runner under the whole suite's load takes seconds where a laptop
    // takes milliseconds (see src/test-setup.ts).
    setupFiles: ["src/test-setup.ts"],
    testTimeout: 15_000,
  },
});
