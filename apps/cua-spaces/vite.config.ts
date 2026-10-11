// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The rcdp wire v2 client core is shared with the cua-spacesd HTML5 viewer.
const html5Core = decodeURIComponent(new URL("../../libs/cua/crates/cua-spacesd-html5/web/src/core", import.meta.url).pathname);
// The cua SDK's browser-safe teleport UX core (`@trycua/cua/teleport`), used
// from source so the app needs no prebuilt SDK package.
const teleportCore = decodeURIComponent(
  new URL("../../libs/cua/typescript/src/teleport", import.meta.url).pathname,
);
// The SDK's browser-safe presence model and shared cursor art
// (`@trycua/cua/spaces/presence`), from source like the teleport core.
const spacesCore = decodeURIComponent(new URL("../../libs/cua/typescript/src/spaces", import.meta.url).pathname);

// Tauri expects a fixed dev-server port (see src-tauri/tauri.conf.json).
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@cua/spacesd-html5/core": html5Core,
      "@trycua/cua/teleport": `${teleportCore}/index.ts`,
      "@trycua/cua/spaces/presence": `${spacesCore}/presence.ts`,
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    fs: { allow: [".", html5Core, teleportCore, spacesCore] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  // Tauri's WKWebView/WebView2 are evergreen, so es2022 is safe.
  build: {
    target: "es2022",
    sourcemap: false,
    // Never inline JS assets as data: URLs: the audio playout AudioWorklet
    // (src/viewer/audioWorklet.js) must load same-origin under the CSP's
    // default-src 'self'.
    assetsInlineLimit: (file: string) => (file.endsWith(".js") ? false : undefined),
  },
  esbuild: { target: "es2022" },
  optimizeDeps: {
    // Dep pre-bundling has its own esbuild target (default es2020); keep it
    // in step with the build target.
    esbuildOptions: { target: "es2022" },
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
  },
});
