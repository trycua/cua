#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Builds the app core (libs/cua/crates/cua-spaces-app-core) for the web UI.
 * Reuses the Tauri app's shim crate (apps/cua-spaces/core-wasm: one
 * `call(method, args)` export) and writes wasm-bindgen's `--target web`
 * output to src/bridge/core/wasm/ (gitignored). Cargo keeps it incremental.
 *
 * Needs the wasm32-unknown-unknown target for the toolchain the libs pin
 * (`rustup target add wasm32-unknown-unknown --toolchain 1.97.1`) and
 * wasm-bindgen-cli 0.2.126 (`cargo install wasm-bindgen-cli --version
 * 0.2.126 --locked`).
 *
 * `--optional`: when the toolchain is missing, print why and exit 0, so
 * `dev` still starts (demo mode, without the core's decisions).
 */
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, realpathSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const crate = resolve(app, "..", "cua-spaces", "core-wasm")
const manifest = join(crate, "Cargo.toml")
const target = process.env.CARGO_TARGET_DIR
  ? join(resolve(process.env.CARGO_TARGET_DIR), "core-wasm")
  : join(crate, "target")
const out = join(app, "src", "bridge", "core", "wasm")
const WASM_BINDGEN = "0.2.126"
const optional = process.argv.includes("--optional")

function give_up(message) {
  console.error(`app core wasm: ${message}`)
  if (optional) {
    console.error("app core wasm: skipped; the bridge runs without the core (TypeScript fallbacks)")
    process.exit(0)
  }
  process.exit(1)
}

// rustup's proxies and cargo-installed tools first, so the toolchain the
// libs pin (apps/cua-spaces/rust-toolchain.toml, found upward from the
// crate) is used even when a Homebrew rust is earlier on PATH.
const extra = [join(homedir(), ".cargo", "bin")]
const rustup = spawnSync("sh", ["-c", "command -v rustup"], { encoding: "utf8" }).stdout?.trim()
if (rustup) extra.unshift(dirname(realpathSync(rustup)))
const env = { ...process.env, CARGO_TARGET_DIR: target, PATH: [...extra, process.env.PATH].join(":") }

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: crate, stdio: "inherit", env })
  if (r.error) give_up(`${cmd}: ${r.error.message}`)
  if (r.status !== 0) give_up(`${cmd} ${args[0]} failed (${r.status})`)
}

const version = spawnSync("wasm-bindgen", ["--version"], { encoding: "utf8", env })
if (version.error || !version.stdout.includes(WASM_BINDGEN)) {
  give_up(
    `wasm-bindgen ${WASM_BINDGEN} is required (found: ${version.stdout?.trim() || "none"}); ` +
      `cargo install wasm-bindgen-cli --version ${WASM_BINDGEN} --locked`,
  )
}

run("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", manifest])

const wasm = join(target, "wasm32-unknown-unknown", "release", "cua_spaces_core_wasm.wasm")
if (!existsSync(wasm)) give_up(`missing ${wasm}`)
mkdirSync(out, { recursive: true })
run("wasm-bindgen", ["--target", "web", "--out-dir", out, "--out-name", "core", wasm])
console.log(`app core wasm: ${out}`)
