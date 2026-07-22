#!/usr/bin/env node

/** Assemble the public TypeScript package and its optional native packages.
 *
 * The generated UniFFI loader resolves the SDK cdylib from an OS/architecture
 * package. Keeping native payloads separate lets `@trycua/cua-driver` remain
 * one stable public package while npm installs only the matching binary.
 */

import { spawnSync } from "node:child_process"
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDirectory = dirname(fileURLToPath(import.meta.url))
const driverRoot = resolve(scriptDirectory, "..")
const typescriptRoot = join(driverRoot, "typescript")

function valueAfter(flag) {
  const index = process.argv.indexOf(flag)
  if (index < 0 || !process.argv[index + 1]) throw new Error(`missing ${flag}`)
  return process.argv[index + 1]
}

const version = valueAfter("--version")
const nativeRoot = resolve(valueAfter("--native-dir"))
const outputRoot = resolve(valueAfter("--output-dir"))
const allowPartial = process.argv.includes("--allow-partial")
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(version)) {
  throw new Error(`invalid release version: ${version}`)
}

const platforms = [
  { triple: "darwin-arm64", os: "darwin", cpu: "arm64", file: "libcua_driver_sdk.dylib" },
  { triple: "darwin-x64", os: "darwin", cpu: "x64", file: "libcua_driver_sdk.dylib" },
  { triple: "linux-arm64-gnu", os: "linux", cpu: "arm64", libc: "glibc", file: "libcua_driver_sdk.so" },
  { triple: "linux-x64-gnu", os: "linux", cpu: "x64", libc: "glibc", file: "libcua_driver_sdk.so" },
  { triple: "win32-arm64-msvc", os: "win32", cpu: "arm64", file: "cua_driver_sdk.dll" },
  { triple: "win32-x64-msvc", os: "win32", cpu: "x64", file: "cua_driver_sdk.dll" },
]

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`)
}

function pack(directory) {
  const result = spawnSync(
    "npm",
    ["pack", directory, "--pack-destination", outputRoot, "--json"],
    { cwd: typescriptRoot, encoding: "utf8" },
  )
  if (result.status !== 0) {
    throw new Error(`npm pack failed for ${directory}:\n${result.stderr || result.stdout}`)
  }
}

mkdirSync(outputRoot, { recursive: true })
const stagingRoot = mkdtempSync(join(outputRoot, ".cua-driver-npm-staging-"))

const optionalDependencies = {}
let nativeCount = 0
for (const platform of platforms) {
  const name = `@trycua/cua-driver-${platform.triple}`
  optionalDependencies[name] = version
  const source = join(nativeRoot, platform.triple, platform.file)
  if (!existsSync(source)) {
    if (allowPartial) continue
    throw new Error(`missing native SDK library ${source}`)
  }
  nativeCount += 1
  const destination = join(stagingRoot, platform.triple)
  mkdirSync(destination, { recursive: true })
  cpSync(source, join(destination, platform.file))
  const manifest = {
    name,
    version,
    description: `Native Cua Driver SDK library for ${platform.triple}`,
    license: "MIT",
    repository: { type: "git", url: "git+https://github.com/trycua/cua.git" },
    os: [platform.os],
    cpu: [platform.cpu],
    files: [platform.file],
    publishConfig: { access: "public" },
  }
  if (platform.libc) manifest.libc = [platform.libc]
  writeJson(join(destination, "package.json"), manifest)
  pack(destination)
}
if (nativeCount === 0) throw new Error(`no native SDK libraries found under ${nativeRoot}`)

const sourceManifest = JSON.parse(readFileSync(join(typescriptRoot, "package.json"), "utf8"))
if (sourceManifest.version !== version) {
  throw new Error(
    `TypeScript package version ${sourceManifest.version} does not match release ${version}`,
  )
}
const rootStage = join(stagingRoot, "root")
mkdirSync(rootStage, { recursive: true })
cpSync(join(typescriptRoot, "dist"), join(rootStage, "dist"), { recursive: true })
for (const nativeFile of [
  "libcua_driver_sdk.dylib",
  "libcua_driver_sdk.so",
  "cua_driver_sdk.dll",
]) {
  const staleColocatedLibrary = join(rootStage, "dist", "native", nativeFile)
  if (existsSync(staleColocatedLibrary)) unlinkSync(staleColocatedLibrary)
}
cpSync(join(typescriptRoot, "README.md"), join(rootStage, "README.md"))
writeJson(join(rootStage, "package.json"), {
  ...sourceManifest,
  scripts: undefined,
  devDependencies: undefined,
  optionalDependencies,
})
pack(rootStage)

rmSync(stagingRoot, { recursive: true })
console.log(`Packed @trycua/cua-driver ${version} with ${nativeCount} native package(s).`)
