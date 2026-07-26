#!/usr/bin/env node

import { spawnSync } from "node:child_process"

import { copyFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const driverRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const file =
  process.platform === "darwin"
    ? "libcua_driver_sdk.dylib"
    : process.platform === "win32"
      ? "cua_driver_sdk.dll"
      : "libcua_driver_sdk.so"
const source = join(driverRoot, "rust", "target", "release", file)
if (!existsSync(source)) {
  throw new Error(`missing ${source}; build cua-driver-sdk --release first`)
}

const destinations = [
  join(driverRoot, "python", "src", "cua_driver", file),
]
for (const destination of destinations) {
  mkdirSync(dirname(destination), { recursive: true })
  copyFileSync(source, destination)
  console.log(`staged ${destination}`)
}

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
const localPackage = join(
  driverRoot,
  "typescript",
  "node_modules",
  "@trycua",
  `cua-driver-${nodeTriple}`,
)
mkdirSync(localPackage, { recursive: true })
copyFileSync(source, join(localPackage, file))
const runtime = join(localPackage, "cua_driver_node_runtime.node")
const runtimeBuild = spawnSync(
  process.execPath,
  [join(driverRoot, "scripts", "build-node-runtime.mjs"), "--output", runtime],
  { stdio: "inherit" },
)
if (runtimeBuild.error) throw runtimeBuild.error
if (runtimeBuild.status !== 0) {
  throw new Error(`Node runtime build exited with status ${runtimeBuild.status}`)
}
writeFileSync(
  join(localPackage, "package.json"),
  `${JSON.stringify(
    {
      name: `@trycua/cua-driver-${nodeTriple}`,
      version: "0.0.0-local",
      private: true,
    },
    null,
    2,
  )}\n`,
)
console.log(`staged local Node platform package ${localPackage}`)
