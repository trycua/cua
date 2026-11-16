// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { defineConfig } from "vitest/config"
import react from "@vitejs/plugin-react"

// The webview UI lives in ui/; `pnpm tauri dev` serves it on 1430. The New
// Space wizard imports the shared image list from the repo
// (libs/images/sandbox-images.json), so the dev server may read the repo root.
const repoRoot = decodeURIComponent(new URL("../..", import.meta.url).pathname)

export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1430, strictPort: true, fs: { allow: [repoRoot] } },
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
  test: { include: ["src/**/*.test.{ts,tsx}"], environment: "node" },
})
