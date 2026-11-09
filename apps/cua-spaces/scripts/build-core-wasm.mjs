#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Builds the app core (libs/cua/crates/cua-spaces-app-core) for the webview:
 * core-wasm (one `call(method, args)` export) to wasm32, then wasm-bindgen
 * (--target web) into src/core/wasm/ (gitignored). Run by `pnpm dev`,
 * `pnpm build` and `pnpm test`; cargo keeps it incremental.
 *
 * Needs the wasm32-unknown-unknown target (`rustup target add
 * wasm32-unknown-unknown`) and wasm-bindgen-cli 0.2.126 (`cargo install
 * wasm-bindgen-cli --version 0.2.126 --locked`).
 */
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const manifest = join(app, "core-wasm", "Cargo.toml")
const target = process.env.CARGO_TARGET_DIR
  ? join(resolve(process.env.CARGO_TARGET_DIR), "core-wasm")
  : join(app, "core-wasm", "target")
const out = join(app, "src", "core", "wasm")
const WASM_BINDGEN = "0.2.126"

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: app, stdio: "inherit", env: { ...process.env, CARGO_TARGET_DIR: target } })
  if (r.error) throw r.error
  if (r.status !== 0) process.exit(r.status ?? 1)
}

const version = spawnSync("wasm-bindgen", ["--version"], { encoding: "utf8" })
if (version.error || !version.stdout.includes(WASM_BINDGEN)) {
  console.error(
    `wasm-bindgen ${WASM_BINDGEN} is required (found: ${version.stdout?.trim() || "none"}); ` +
      `cargo install wasm-bindgen-cli --version ${WASM_BINDGEN} --locked`,
  )
  process.exit(1)
}

run("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", manifest])
const wasm = join(target, "wasm32-unknown-unknown", "release", "cua_spaces_core_wasm.wasm")
if (!existsSync(wasm)) {
  console.error(`missing ${wasm}`)
  process.exit(1)
}
mkdirSync(out, { recursive: true })
run("wasm-bindgen", ["--target", "web", "--out-dir", out, "--out-name", "core", wasm])
console.log(`app core wasm: ${out}`)
