// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fileURLToPath, URL } from "node:url";

import babel from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const src = (path: string) => fileURLToPath(new URL(`./src/${path}`, import.meta.url));
// The cua SDK's browser-safe presence model and cursor art, from source, as
// the shared presence layer (libs/cua/crates/cua-spacesd-html5) imports it.
const presence = fileURLToPath(new URL("../../libs/cua/typescript/src/spaces/presence.ts", import.meta.url));

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
      { find: /^@trycua\/cua\/spaces\/presence$/, replacement: presence },
    ],
  },
  // Scan every source file up front: routes are code-split, so otherwise their
  // dependencies are found on first navigation and the dev server reloads the
  // page mid-run (parity flows then fail with "execution context destroyed").
  optimizeDeps: { entries: ["index.html", "src/**/*.{ts,tsx}", "!src/**/*.d.ts", "!src/**/__tests__/**", "!src/**/*.test.{ts,tsx}"] },
  server: { port: 5174, strictPort: true },
  preview: { port: 5174 },
  // Source maps: shown in a plain build (`pnpm dev`, local checks). Release CD
  // sets CUA_SOURCEMAPS=1 to emit them hidden (no sourceMappingURL comment) in
  // dist/**/*.map, uploads them to Sentry, then deletes them: the app never
  // ships them (electron-builder's extraResources filter drops *.map too).
  build: { target: "es2023", sourcemap: process.env.CUA_SOURCEMAPS === "1" ? "hidden" : false },
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
