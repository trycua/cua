#!/usr/bin/env node

/**
 * Stage the host-built cua-sdk library where each language's loader looks
 * for it during development and tests:
 *
 *  - Python: next to `src/cua/_native.py`;
 *  - Swift: `swift/lib/` (the dev Package.swift links it from there);
 *  - Node: a local `@trycua/cua-<triple>` platform package in
 *    `typescript/node_modules`, with the copy-based N-API runtime.
 *
 * Usage: node stage-uniffi-library.mjs [--only=python,swift,node]
 */

import { spawnSync } from "node:child_process"
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const cuaRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const onlyArgument = process.argv.find((a) => a.startsWith("--only="))
const targets = new Set(
  (onlyArgument ? onlyArgument.slice(7) : "python,swift,node").split(",").filter(Boolean),
)
const file =
  process.platform === "darwin"
    ? "libcua_sdk.dylib"
    : process.platform === "win32"
      ? "cua_sdk.dll"
      : "libcua_sdk.so"
const targetDirectory = process.env.CARGO_TARGET_DIR
  ? resolve(process.env.CARGO_TARGET_DIR)
  : join(cuaRoot, "target")
const source = join(targetDirectory, "release", file)
if (!existsSync(source)) {
  throw new Error(`missing ${source}; run cargo build --release -p cua-sdk first`)
}

function stage(destination) {
  mkdirSync(dirname(destination), { recursive: true })
  copyFileSync(source, destination)
  console.log(`staged ${destination}`)
}

if (targets.has("python")) stage(join(cuaRoot, "python", "src", "cua", file))
if (targets.has("swift")) stage(join(cuaRoot, "swift", "lib", file))

if (targets.has("node")) {
  const nodeTriple = (() => {
    if (process.platform === "darwin" && ["arm64", "x64"].includes(process.arch))
      return `darwin-${process.arch}`
    if (process.platform === "win32" && ["arm64", "x64"].includes(process.arch))
      return `win32-${process.arch}-msvc`
    if (process.platform === "linux" && ["arm64", "x64"].includes(process.arch)) {
      const gnu = process.report?.getReport()?.header?.glibcVersionRuntime !== undefined
      return `linux-${process.arch}-${gnu ? "gnu" : "musl"}`
    }
    throw new Error(`unsupported Node platform ${process.platform}/${process.arch}`)
  })()
  const localPackage = join(cuaRoot, "typescript", "node_modules", "@trycua", `cua-${nodeTriple}`)
  stage(join(localPackage, file))
  const runtime = join(localPackage, "cua_node_runtime.node")
  if (!existsSync(runtime) || process.argv.includes("--rebuild-runtime")) {
    const build = spawnSync(
      process.execPath,
      [join(cuaRoot, "scripts", "build-node-runtime.mjs"), "--output", runtime],
      { stdio: "inherit" },
    )
    if (build.error) throw build.error
    if (build.status !== 0) throw new Error(`Node runtime build exited with ${build.status}`)
  }
  writeFileSync(
    join(localPackage, "package.json"),
    `${JSON.stringify({ name: `@trycua/cua-${nodeTriple}`, version: "0.0.0-local", private: true }, null, 2)}\n`,
  )
  console.log(`staged local Node platform package ${localPackage}`)
}
