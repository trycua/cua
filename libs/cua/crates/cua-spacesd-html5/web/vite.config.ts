// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { defineConfig } from "vitest/config";

// The cua SDK's browser-safe presence model and shared cursor art, from
// source (the same module the Cua Spaces app aliases).
const presence = decodeURIComponent(new URL("../../../typescript/src/spaces/presence.ts", import.meta.url).pathname);

// Deterministic output (no content hashes) straight into the crate, which
// embeds it: `viewer.js`, `viewer.css`, the worklets, `index.html`.
export default defineConfig({
  base: "./",
  resolve: { alias: { "@trycua/cua/spaces/presence": presence } },
  build: {
    outDir: "../assets",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false,
    // Worklets must load same-origin (never inlined as data: URLs).
    assetsInlineLimit: 0,
    modulePreload: false,
    rollupOptions: {
      output: {
        entryFileNames: "viewer.js",
        chunkFileNames: "[name].js",
        assetFileNames: "[name][extname]",
      },
    },
  },
  test: {
    environment: "jsdom",
    css: false,
  },
});
