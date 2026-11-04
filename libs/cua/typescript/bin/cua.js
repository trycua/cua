#!/usr/bin/env node
// The `cua` command: runs the Rust CLI shipped in the matching
// @trycua/cua-<platform> package.
import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"
import { createRequire } from "node:module"
import { dirname, join } from "node:path"

const arch = process.arch
const triple =
  process.platform === "linux"
    ? `linux-${arch}-gnu`
    : process.platform === "win32"
      ? `win32-${arch}-msvc`
      : `${process.platform}-${arch}`
const exe = process.platform === "win32" ? "cua.exe" : "cua"
const require = createRequire(import.meta.url)

let binary
try {
  binary = join(dirname(require.resolve(`@trycua/cua-${triple}/package.json`)), exe)
} catch {
  binary = undefined
}
if (!binary || !existsSync(binary)) {
  console.error(
    `cua: no CLI binary for ${triple}; install @trycua/cua on a supported platform, ` +
      "or build it with `cargo install --locked --path libs/cua/crates/cua-cli`.",
  )
  process.exit(127)
}
const r = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" })
if (r.error) {
  console.error(`cua: ${r.error.message}`)
  process.exit(127)
}
process.exit(r.status ?? 1)
