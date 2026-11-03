#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `pnpm native [-- --target <rust-triple>] [--bindings | --check-bindings]`
 *
 * Builds the native layer the main process loads, for one Rust target (the
 * host's when none is given), into `native/<platform>-<arch>/` (gitignored;
 * electron-builder ships it as `Resources/native/`):
 *
 *  - `cua-spaces-ffi` (libs/cua/crates/cua-spaces-ffi): the cua SDK and the
 *    Spaces app core in one UniFFI library, the one the SwiftUI app links;
 *  - `cua_node_runtime.node`: the pinned uniffi-bindgen-react-native N-API
 *    runtime, rebuilt Electron-safe by libs/cua/scripts/build-node-runtime.mjs;
 *  - `cua` (`cua.exe`): the Spaces build of the CLI (`cua-spaces-cli`), which
 *    the app starts its daemon with and installs on PATH.
 *
 * `--bindings` also regenerates the TypeScript bindings of that library into
 * `src/native/generated/` (committed, like the Swift app's binding in
 * libs/spaces-app-swift); `--check-bindings` regenerates them into a
 * temporary directory and fails when the committed ones differ. Both read the
 * host's build (the bindings are the same for every target); `--skip-build`
 * reads the one already there. Windows MSVC builds link the C runtime
 * statically (no RUSTFLAGS needed).
 *
 * The UniFFI tools are the ones libs/cua/typescript pins (`npm ci` there,
 * which this runs when they are missing). macOS universal builds take both
 * `aarch64-apple-darwin` and `x86_64-apple-darwin`: electron-builder packs
 * each arch's directory and @electron/universal merges the two with lipo.
 */

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const cuaRoot = resolve(root, "../../libs/cua");
const typescriptRoot = join(cuaRoot, "typescript");
const generatedDir = join(root, "src", "native", "generated");
const INVENTORY = ".generated-files";

/** Rust target -> Node's platform and arch (the directory name). */
export const TARGETS = {
  "aarch64-apple-darwin": ["darwin", "arm64"],
  "x86_64-apple-darwin": ["darwin", "x64"],
  "x86_64-pc-windows-msvc": ["win32", "x64"],
  "aarch64-pc-windows-msvc": ["win32", "arm64"],
  "x86_64-unknown-linux-gnu": ["linux", "x64"],
  "aarch64-unknown-linux-gnu": ["linux", "arm64"],
};

const argv = process.argv.slice(2).filter((a) => a !== "--");
const flag = (name) => argv.includes(name);
const option = (name) => {
  const i = argv.indexOf(name);
  if (i < 0) return undefined;
  if (!argv[i + 1]) throw new Error(`${name} needs a value`);
  return argv[i + 1];
};

function hostTarget() {
  const found = Object.entries(TARGETS).find(([, [p, a]]) => p === process.platform && a === process.arch);
  if (!found) throw new Error(`no Rust target for this host (${process.platform}/${process.arch})`);
  return found[0];
}

const host = hostTarget();
const target = option("--target") ?? host;
if (!TARGETS[target]) throw new Error(`unknown --target ${target}; one of ${Object.keys(TARGETS).join(", ")}`);
const [platform, arch] = TARGETS[target];
const outDir = join(root, "native", `${platform}-${arch}`);
const bindings = flag("--bindings") || flag("--check-bindings");
const checkOnly = flag("--check-bindings");
if (bindings && target !== host) throw new Error(`--bindings reads the host build: ${host}, not ${target}`);

const libraryName = { darwin: "libcua_spaces_ffi.dylib", win32: "cua_spaces_ffi.dll", linux: "libcua_spaces_ffi.so" }[platform];
const cliName = platform === "win32" ? "cua.exe" : "cua";
const cargoTarget = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(cuaRoot, "target");
const releaseDir = target === host && !option("--target") ? join(cargoTarget, "release") : join(cargoTarget, target, "release");

/** A Windows `.cmd` (npm.cmd, ubrn.cmd) runs only through cmd.exe, which takes one command line. */
export function shellCommand(command, args) {
  const quote = (a) => (/[\s"&|<>^]/.test(a) ? `"${a.replaceAll('"', '""')}"` : a);
  return [command, ...args].map(quote).join(" ");
}

function run(command, args, options = {}) {
  console.log(`> ${command} ${args.join(" ")}`);
  const viaShell = process.platform === "win32" && command.endsWith(".cmd");
  const result = viaShell
    ? spawnSync(shellCommand(command, args), { stdio: "inherit", shell: true, ...options })
    : spawnSync(command, args, { stdio: "inherit", ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

/** Windows MSVC builds link the C runtime statically, so a clean Windows
 * needs no VC++ redistributable (as the Node runtime build does). */
function cargoEnv() {
  const env = { ...process.env };
  if (target.endsWith("-pc-windows-msvc")) env.RUSTFLAGS = [env.RUSTFLAGS?.trim(), "-C target-feature=+crt-static"].filter(Boolean).join(" ");
  return env;
}

function ensureUniffiTools() {
  if (existsSync(join(typescriptRoot, "node_modules", "uniffi-bindgen-react-native", "package.json"))) return;
  run(process.platform === "win32" ? "npm.cmd" : "npm", ["ci", "--ignore-scripts"], { cwd: typescriptRoot });
}

/** Local symbols out of what ships (the exported UniFFI symbols stay). */
function strip(file, kind) {
  if (platform === "win32" || process.platform !== platform) return;
  const args = platform === "darwin" ? (kind === "exe" ? [file] : ["-x", file]) : kind === "exe" ? [file] : ["--strip-unneeded", file];
  const result = spawnSync("strip", args, { stdio: "inherit" });
  if (result.status !== 0) console.warn(`note: could not strip ${file}`);
}

function buildNative() {
  const cargoArgs = ["build", "--locked", "--release", "-p", "cua-spaces-ffi", "-p", "cua-spaces-cli"];
  if (option("--target")) cargoArgs.push("--target", target);
  run("cargo", cargoArgs, { cwd: cuaRoot, env: cargoEnv() });
  ensureUniffiTools();
  mkdirSync(outDir, { recursive: true });
  const runtimeArgs = [join(cuaRoot, "scripts", "build-node-runtime.mjs"), "--output", join(outDir, "cua_node_runtime.node")];
  if (option("--target")) runtimeArgs.push("--target", target);
  run(process.execPath, runtimeArgs, { cwd: cuaRoot });
  const library = join(releaseDir, libraryName);
  const cli = join(releaseDir, platform === "win32" ? "cua-spaces-cli.exe" : "cua-spaces-cli");
  for (const f of [library, cli]) if (!existsSync(f)) throw new Error(`missing ${f}`);
  copyFileSync(library, join(outDir, libraryName));
  copyFileSync(cli, join(outDir, cliName));
  chmodSync(join(outDir, cliName), 0o755);
  strip(join(outDir, libraryName), "lib");
  strip(join(outDir, "cua_node_runtime.node"), "lib");
  strip(join(outDir, cliName), "exe");
  console.log(`native layer for ${target} in ${outDir}`);
}

/* ---- Bindings ------------------------------------------------------------- */

const FSL_HEADER = "// SPDX-License-Identifier: FSL-1.1-MIT\n// Copyright (c) 2026 Cua AI, Inc.\n";
const NAMESPACES = ["cua_sdk", "cua_spaces_ffi"];

/** heck's lowerCamelCase: a Rust variant's Swift case name. */
export function caseName(variant) {
  const words = variant.match(/[A-Z]+(?![a-z])|[A-Z]?[a-z0-9]+|[0-9]+/g) ?? [variant];
  return words.map((w, i) => (i === 0 ? w.toLowerCase() : w[0].toUpperCase() + w.slice(1).toLowerCase())).join("");
}

function normalize(source) {
  const out = source
    .replaceAll("\r\n", "\n")
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/u, ""))
    .join("\n");
  return out.endsWith("\n") ? out : `${out}\n`;
}

/**
 * Flat enums carry their Swift case names as values (`AppSpaceOs.Macos =
 * "macos"`), so a view encodes as the SwiftUI host's does without a type
 * table. The converters name the members, never their values.
 */
export function stringEnums(source) {
  return source.replace(/(export enum (\w+) \{\n)([\s\S]*?)(\n\})/g, (whole, open, name, body, close) => {
    const members = body.split("\n").filter((line) => /^\s*[A-Z]\w*/.test(line));
    if (name.endsWith("_Tags") || members.some((line) => line.includes("="))) return whole;
    const valued = body.replace(/^(\s*)([A-Z]\w*)(,?)$/gm, (_m, indent, member, comma) => `${indent}${member} = "${caseName(member)}"${comma}`);
    return `${open}${valued}${close}`;
  });
}

export function postprocess(name, source) {
  let out = normalize(source);
  if (name.endsWith("-ffi.ts")) {
    const needle = 'import lib from "@ubjs/node";';
    if (out.split(needle).length !== 2) throw new Error(`expected one Node runtime import in ${name}`);
    out = out.replace(needle, 'import lib from "../node-runtime";');
    // Every namespace is in the one library the runtime opens.
    out = out.replace(/resolveLibPath\(\{[\s\S]*?\}\)/g, "resolveLibPath()");
  } else {
    out = stringEnums(out);
  }
  // Every file here is part of this (FSL) app, so it carries the app's header
  // (scripts/spdx-headers.py --fsl --check), the cua SDK's bindings included.
  const header = name.startsWith("cua_sdk")
    ? `${FSL_HEADER}\n// Generated from the cua SDK's UniFFI namespace in libcua_spaces_ffi by\n// scripts/build-native.mjs --bindings; do not edit.\n`
    : `${FSL_HEADER}// Generated by scripts/build-native.mjs --bindings; do not edit.\n`;
  return `${header}\n${out}`;
}

function generate(into) {
  ensureUniffiTools();
  const library = join(releaseDir, libraryName);
  if (!existsSync(library)) throw new Error(`missing ${library}: run pnpm native first`);
  const raw = mkdtempSync(join(tmpdir(), "cua-spaces-bindings-"));
  try {
    const ubrn = join(typescriptRoot, "node_modules", ".bin", process.platform === "win32" ? "ubrn.cmd" : "ubrn");
    // UBRN reads `cargo metadata` in its working directory.
    run(ubrn, ["generate", "napi", "bindings", "--library", library, "--ts-dir", raw, "--lib-package-base", "@cua/cua-spaces-", "--lib-node-triple", "--no-format"], { cwd: cuaRoot });
    const names = readdirSync(raw).filter((n) => n.endsWith(".ts"));
    const foreign = names.map((n) => n.replace(/(-ffi)?\.ts$/, "")).filter((ns) => ns !== "index" && !NAMESPACES.includes(ns));
    const files = {};
    for (const name of names) {
      const source = readFileSync(join(raw, name), "utf8");
      if (name === "index.ts") {
        // The two namespaces this app uses; the others linked into the library are left out.
        const kept = source.split("\n").filter((line) => !foreign.some((ns) => line.includes(ns))).join("\n");
        files[name] = postprocess(name, kept);
      } else if (NAMESPACES.some((ns) => name === `${ns}.ts` || name === `${ns}-ffi.ts`)) {
        files[name] = postprocess(name, source);
      }
    }
    for (const ns of NAMESPACES) if (!files[`${ns}.ts`]) throw new Error(`UBRN produced no ${ns}.ts`);
    files[INVENTORY] = ["# Generated by scripts/build-native.mjs --bindings; one file per line.", ...Object.keys(files).sort(), ""].join("\n");
    mkdirSync(into, { recursive: true });
    for (const [name, contents] of Object.entries(files)) writeFileSync(join(into, name), contents);
    return files;
  } finally {
    rmSync(raw, { recursive: true, force: true });
  }
}

function writeBindings() {
  const previous = existsSync(join(generatedDir, INVENTORY))
    ? readFileSync(join(generatedDir, INVENTORY), "utf8").split("\n").filter((l) => l && !l.startsWith("#"))
    : [];
  const files = generate(generatedDir);
  for (const old of previous) if (!(old in files)) rmSync(join(generatedDir, old), { force: true });
  console.log(`bindings in ${generatedDir}`);
}

function checkBindings() {
  const scratch = mkdtempSync(join(tmpdir(), "cua-spaces-bindings-check-"));
  try {
    const files = generate(scratch);
    const stale = Object.entries(files)
      .filter(([name, contents]) => !existsSync(join(generatedDir, name)) || readFileSync(join(generatedDir, name), "utf8") !== contents)
      .map(([name]) => name);
    if (stale.length) throw new Error(`stale bindings in src/native/generated: ${stale.join(", ")} (pnpm native -- --bindings)`);
    console.log("The bindings are up to date.");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (!flag("--skip-build")) buildNative();
  if (checkOnly) checkBindings();
  else if (bindings) writeBindings();
}
