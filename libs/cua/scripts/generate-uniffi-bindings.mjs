#!/usr/bin/env node

/**
 * Generate the cua SDK's UniFFI bindings (Python, Swift, Kotlin,
 * TypeScript/Node) from the compiled `cua-sdk` cdylib, and fail rather than
 * rot when they drift.
 *
 * The same shape as libs/cua-driver/scripts/generate-uniffi-bindings.mjs:
 *
 *  - both generators read the *compiled library*, so a binding can never
 *    describe an API the library does not export;
 *  - both are pinned: the UniFFI CLI is `cua-bindgen` from this workspace
 *    (same UniFFI as the cdylib) and UBRN is a pinned devDependency of
 *    libs/cua/typescript;
 *  - every language root carries a `.cua-uniffi-generated-files` inventory
 *    listing each generated path, so renamed bindings are pruned;
 *  - `--check` regenerates into a temporary directory and compares.
 *
 * Only the `cua_sdk` namespace is generated.
 *
 * The library is built with the dev profile. UniFFI reads the interface
 * from metadata the proc macros embed, which does not depend on the
 * profile, so the bindings are identical. A cold release build alone took
 * most of the CI drift job's time limit and shared nothing with the job's
 * other (dev profile) cargo steps. Pass `--release` to read
 * `target/release` instead, e.g. with `--skip-build` after a release build.
 *
 * The library is built without the `byoc` feature: it only forwards to the
 * daemon's cloud providers (the AWS SDK alone is minutes of compile time)
 * and adds nothing to the UniFFI interface.
 *
 * Avoiding the build altogether:
 *
 *  - `--library=PATH` (or `CUA_SDK_LIBRARY=PATH`) reads an already built,
 *    unstripped library (UniFFI reads the interface from its symbol table,
 *    so CI's stripped release artifacts do not work);
 *  - otherwise the generated files are cached by a fingerprint of everything
 *    the interface is made from (the cua-sdk crate, VERSION, the pinned UniFFI
 *    and UBRN, these scripts). A later run with the same fingerprint, from
 *    any worktree, writes the cached files without compiling anything.
 *    The cache lives in `$CUA_BUILD_CACHE_DIR/uniffi` (default
 *    `~/.cache/cua-build`); `--no-cache` ignores it, and CI never uses it.
 *
 * Usage: node generate-uniffi-bindings.mjs [--check] [--skip-build]
 *        [--release] [--only=python,swift,kotlin,typescript]
 *        [--library=PATH] [--no-cache]
 */

import { spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { homedir, tmpdir } from "node:os"
import { dirname, isAbsolute, join, relative, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { postprocessPython } from "./uniffi-python-postprocess.mjs"

const scriptDirectory = dirname(fileURLToPath(import.meta.url))
const cuaRoot = resolve(scriptDirectory, "..")
const pythonRoot = join(cuaRoot, "python")
const swiftRoot = join(cuaRoot, "swift")
const kotlinRoot = join(cuaRoot, "kotlin")
const typescriptRoot = join(cuaRoot, "typescript")
const check = process.argv.includes("--check")
const skipBuild = process.argv.includes("--skip-build")
const release = process.argv.includes("--release")
const onlyArgument = process.argv.find((argument) => argument.startsWith("--only="))
const languages = new Set(
  (onlyArgument ? onlyArgument.slice("--only=".length) : "python,swift,kotlin,typescript")
    .split(",")
    .map((language) => language.trim())
    .filter(Boolean),
)
const libraryArgument = process.argv.find((argument) => argument.startsWith("--library="))
const useCache = !process.argv.includes("--no-cache") && !process.env.CI
const INVENTORY = ".cua-uniffi-generated-files"
const CRATE = "cua_sdk"

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" })
  if (result.error) throw result.error
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`)
  }
}

function nativeLibraryPath() {
  const file =
    process.platform === "darwin"
      ? "libcua_sdk.dylib"
      : process.platform === "win32"
        ? "cua_sdk.dll"
        : "libcua_sdk.so"
  const targetDirectory = process.env.CARGO_TARGET_DIR
    ? resolve(process.env.CARGO_TARGET_DIR)
    : join(cuaRoot, "target")
  return join(targetDirectory, release ? "release" : "debug", file)
}

function normalizeWhitespace(source) {
  const output = source
    .replaceAll("\r\n", "\n")
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/u, ""))
    .join("\n")
  return output.endsWith("\n") ? output : `${output}\n`
}

function normalizeTypeScript(name, source) {
  let output = normalizeWhitespace(source)
  // NodeNext requires emitted relative ESM imports to carry their .js suffix.
  output = output.replace(/(from\s+["']\.\/[A-Za-z0-9_-]+)(["'])/g, "$1.js$2")
  if (name.endsWith("-ffi.ts")) {
    const needle = 'import lib from "@ubjs/node";'
    const matches = output.split(needle).length - 1
    if (matches !== 1) {
      throw new Error(`expected one Node runtime import in ${name}, found ${matches}`)
    }
    output = output.replace(needle, 'import lib from "./node-runtime.js";')
  }
  return output
}

function inventoryContents(paths) {
  return [
    "# Generated by libs/cua/scripts/generate-uniffi-bindings.mjs; one owned path per line.",
    ...paths.slice().sort(),
    "",
  ].join("\n")
}

function assertOwnedPath(path) {
  const parts = path.split(/[\\/]/)
  if (isAbsolute(path) || parts.some((part) => part === "" || part === "." || part === "..")) {
    throw new Error(`invalid generated ownership path: ${path}`)
  }
  return path
}

// Every input the generated files are a function of. The interface is the
// metadata UniFFI's proc macros embed in cua-sdk (the only crate of this
// library that uses UniFFI), so other crates cannot change it.
function fingerprint() {
  const hash = createHash("sha256")
  const add = (path) => {
    hash.update(relative(cuaRoot, path))
    hash.update("\0")
    hash.update(readFileSync(path, "utf8").replaceAll("\r\n", "\n"))
    hash.update("\0")
  }
  const walk = (directory) => {
    for (const name of readdirSync(directory).sort()) {
      const path = join(directory, name)
      if (lstatSync(path).isDirectory()) walk(path)
      else add(path)
    }
  }
  walk(join(cuaRoot, "crates/cua-sdk"))
  walk(join(cuaRoot, "crates/cua-bindgen"))
  for (const path of [
    "VERSION",
    "scripts/generate-uniffi-bindings.mjs",
    "scripts/uniffi-python-postprocess.mjs",
    "typescript/package-lock.json",
  ]) {
    add(join(cuaRoot, path))
  }
  // The pinned UniFFI crates (the rest of Cargo.lock does not matter).
  const lock = readFileSync(join(cuaRoot, "Cargo.lock"), "utf8").replaceAll("\r\n", "\n")
  for (const entry of lock.split("\n[[package]]\n")) {
    if (/^name = "(uniffi[^"]*|ubrn[^"]*)"/m.test(entry)) hash.update(entry)
  }
  hash.update(`${[...languages].sort().join(",")}\0${process.env.CUA_SDK_RELEASE_VERSION ?? ""}`)
  return hash.digest("hex").slice(0, 32)
}

function cacheDirectory(key) {
  const base =
    process.env.CUA_BUILD_CACHE_DIR ??
    join(process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache"), "cua-build")
  return join(base, "uniffi", key)
}

// The generated files of every group, in generation order, for the cache.
const generated = []
const stale = []

function applyGroup(root, files) {
  generated.push({ root: relative(cuaRoot, root), files })
  const inventory = join(root, INVENTORY)
  const previous = existsSync(inventory)
    ? readFileSync(inventory, "utf8")
        .split("\n")
        .filter((line) => line && !line.startsWith("#"))
        .map(assertOwnedPath)
    : []
  const expected = Object.keys(files).map(assertOwnedPath).sort()
  const planned = { ...files, [INVENTORY]: inventoryContents(expected) }

  if (check) {
    const differences = previous.filter((path) => !expected.includes(path))
    for (const [path, contents] of Object.entries(planned)) {
      const destination = join(root, path)
      if (!existsSync(destination) || readFileSync(destination, "utf8") !== contents) {
        differences.push(path)
      }
    }
    if (differences.length) stale.push(`${root}: ${differences.join(", ")}`)
    return
  }

  for (const path of previous.filter((path) => !expected.includes(path))) {
    const destination = join(root, path)
    if (existsSync(destination)) {
      if (lstatSync(destination).isSymbolicLink()) {
        throw new Error(`refusing to prune generated symlink ${destination}`)
      }
      unlinkSync(destination)
    }
  }
  for (const [path, contents] of Object.entries(planned)) {
    const destination = join(root, path)
    if (existsSync(destination) && lstatSync(destination).isSymbolicLink()) {
      throw new Error(`refusing to overwrite generated symlink ${destination}`)
    }
    mkdirSync(dirname(destination), { recursive: true })
    const temporary = `${destination}.tmp.${process.pid}`
    writeFileSync(temporary, contents)
    renameSync(temporary, destination)
  }
}

function bindgen(library, language, outDir) {
  run(
    "cargo",
    [
      "run",
      "--locked",
      "-q",
      "-p",
      "cua-bindgen",
      "--",
      "generate",
      library,
      "--crate",
      CRATE,
      "--language",
      language,
      "--out-dir",
      outDir,
      "--no-format",
    ],
    cuaRoot,
  )
}

// cua-sdk's default features without `byoc` (see the header comment).
function buildFeatures() {
  const manifest = readFileSync(join(cuaRoot, "crates/cua-sdk/Cargo.toml"), "utf8")
  const match = manifest.match(/^default\s*=\s*\[([^\]]*)\]/m)
  if (!match) throw new Error("no default features in crates/cua-sdk/Cargo.toml")
  return [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]).filter((f) => f !== "byoc")
}

const external = Boolean(libraryArgument || process.env.CUA_SDK_LIBRARY)
// A library someone else built (or an old one) may not match the sources:
// only a library built here fills the cache.
const key = useCache && !skipBuild && !external ? fingerprint() : null
const cached = key ? join(cacheDirectory(key), "files.json") : null
const temporaryRoot = mkdtempSync(join(tmpdir(), "cua-uniffi-"))
try {
  if (cached && existsSync(cached)) {
    for (const group of JSON.parse(readFileSync(cached, "utf8"))) {
      applyGroup(join(cuaRoot, group.root), group.files)
    }
    console.log(`UniFFI interface unchanged (cache ${key}); nothing compiled.`)
  } else {
    let library = libraryArgument
      ? resolve(libraryArgument.slice("--library=".length))
      : process.env.CUA_SDK_LIBRARY
        ? resolve(process.env.CUA_SDK_LIBRARY)
        : null
      if (!library && !skipBuild) {
      run(
        "cargo",
        [
          "build",
          "--locked",
          ...(release ? ["--release"] : []),
          "-p",
          "cua-sdk",
          "--no-default-features",
          "--features",
          buildFeatures().join(","),
        ],
        cuaRoot,
      )
    }
    library ??= nativeLibraryPath()
    if (!existsSync(library)) {
      throw new Error(
        `missing built SDK library ${library}` +
          (release ? "" : " (pass --release to use a release build)"),
      )
    }

    if (languages.has("python")) {
      const out = join(temporaryRoot, "python")
      bindgen(library, "python", out)
      applyGroup(pythonRoot, {
        "src/cua/_native.py": postprocessPython(
          normalizeWhitespace(readFileSync(join(out, "cua_sdk.py"), "utf8")),
        ),
      })
    }

    if (languages.has("swift")) {
      const out = join(temporaryRoot, "swift")
      bindgen(library, "swift", out)
      applyGroup(swiftRoot, {
        "Sources/CuaSDK/CuaSDK.swift": normalizeWhitespace(
          readFileSync(join(out, "CuaSDK.swift"), "utf8"),
        ),
        "Sources/cua_sdkFFI/include/cua_sdkFFI.h": normalizeWhitespace(
          readFileSync(join(out, "cua_sdkFFI.h"), "utf8"),
        ),
        // SwiftPM requires the map to be named `module.modulemap` and to point
        // at the header by its path inside the system-library target. This is
        // the one place generated output is edited.
        "Sources/cua_sdkFFI/module.modulemap": normalizeWhitespace(
          readFileSync(join(out, "cua_sdkFFI.modulemap"), "utf8").replace(
            'header "cua_sdkFFI.h"',
            'header "include/cua_sdkFFI.h"',
          ),
        ),
      })
    }

    if (languages.has("kotlin")) {
      const out = join(temporaryRoot, "kotlin")
      bindgen(library, "kotlin", out)
      const relative = "ai/cua/sdk/cua_sdk.kt"
      applyGroup(kotlinRoot, {
        [`src/main/kotlin/${relative}`]: normalizeWhitespace(
          readFileSync(join(out, relative), "utf8"),
        ),
      })
    }

    if (languages.has("typescript")) {
      const out = join(temporaryRoot, "typescript")
      const ubrn = join(
        typescriptRoot,
        "node_modules",
        ".bin",
        process.platform === "win32" ? "ubrn.cmd" : "ubrn",
      )
      if (!existsSync(ubrn)) {
        throw new Error("missing pinned UBRN generator; run npm ci in libs/cua/typescript")
      }
      run(
        ubrn,
        [
          "generate",
          "napi",
          "bindings",
          "--library",
          library,
          "--ts-dir",
          out,
          "--lib-package-base",
          "@trycua/cua",
          "--lib-node-triple",
          "--no-format",
        ],
        // UBRN shells out to `cargo metadata` in its working directory.
        cuaRoot,
      )
      const files = {}
      const names = readdirSync(out).filter((name) => name.endsWith(".ts"))
      for (const name of names) {
        if (name === "index.ts") {
          // UBRN's initializer.
          files["src/native/index.ts"] = normalizeTypeScript(name, readFileSync(join(out, name), "utf8"))
          continue
        }
        // Only the cua_sdk namespace (see the header comment).
        if (!name.startsWith(`${CRATE}.`) && !name.startsWith(`${CRATE}-`)) continue
        files[`src/native/${name}`] = normalizeTypeScript(name, readFileSync(join(out, name), "utf8"))
      }
      if (!files[`src/native/${CRATE}.ts`]) throw new Error("UBRN produced no cua_sdk.ts")
      applyGroup(typescriptRoot, files)
    }

    if (cached) {
      mkdirSync(dirname(cached), { recursive: true })
      const temporary = `${cached}.tmp.${process.pid}`
      writeFileSync(temporary, JSON.stringify(generated))
      renameSync(temporary, cached)
    }
  }

  if (languages.has("typescript")) {
    // The shared presence cursor art, generated from the same source as the
    // Rust SDK's copy.
    run("node", [join(scriptDirectory, "generate-presence-art.mjs"), ...(check ? ["--check"] : [])], cuaRoot)
  }

  if (stale.length) {
    throw new Error(`stale UniFFI generated files:\n  ${stale.join("\n  ")}`)
  }
  console.log(check ? "UniFFI bindings are up to date." : "Generated UniFFI bindings.")
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true })
}
