#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Builds "Cua Spaces Notch.app", the macOS notch helper (libs/spaces-notch-swift,
// the SwiftUI app's own notch views), into native/notch/ for electron-builder
// to ship in Contents/Helpers (electron-builder.config.cjs). macOS only.
//
//   pnpm notch                          universal (arm64 + x86_64), release
//   pnpm notch -- --arch arm64          one architecture (quicker in development)
//   pnpm notch -- --jobs 2              parallel compile jobs
//
// The bundle is signed ad hoc so an unsigned build runs; a signed release
// re-signs it with the app's identity and hardened runtime (electron-builder
// signs every Mach-O and nested .app it ships). Each build runs the helper's
// `--selftest` (no window) before it is kept.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, renameSync, rmSync } from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const desktop = path.join(here, "..");
const repo = path.join(desktop, "../..");
const pkg = path.join(repo, "libs/spaces-notch-swift");
const out = path.join(desktop, "native/notch");
const APP = "Cua Spaces Notch.app";
const EXECUTABLE = "Cua Spaces Notch";
const BUNDLE_ID = "com.trycua.spaces.macos.notch";
/** The SwiftUI app's minimum (apps/cua-spaces-macos/Package.swift). */
const MIN_MACOS = "26.0";

if (process.platform !== "darwin") {
  console.error("pnpm notch builds the macOS notch helper and runs on macOS only");
  process.exit(1);
}

const args = process.argv.slice(2);
const option = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const archs = option("--arch") ? [option("--arch")] : ["arm64", "x86_64"];
const jobs = option("--jobs");

const run = (cmd, argv, opts = {}) => execFileSync(cmd, argv, { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8", ...opts });

const slices = archs.map((arch) => {
  const build = ["build", "--package-path", pkg, "-c", "release", "--triple", `${arch}-apple-macosx${MIN_MACOS}`];
  if (jobs) build.push("-j", jobs);
  console.log(`swift ${build.join(" ")} --product CuaSpacesNotch`);
  run("swift", [...build, "--product", "CuaSpacesNotch"], { stdio: "inherit" });
  return path.join(run("swift", [...build, "--show-bin-path"]).trim(), "CuaSpacesNotch");
});

// Assembled next to the old one, then swapped in.
mkdirSync(out, { recursive: true });
const staging = path.join(out, `.${APP}.new`);
rmSync(staging, { recursive: true, force: true });
const contents = path.join(staging, "Contents");
mkdirSync(path.join(contents, "MacOS"), { recursive: true });
mkdirSync(path.join(contents, "Resources"), { recursive: true });
const binary = path.join(contents, "MacOS", EXECUTABLE);
run("lipo", ["-create", ...slices, "-output", binary]);
for (const arch of archs) run("lipo", [binary, "-verify_arch", arch]);

const plist = path.join(contents, "Info.plist");
cpSync(path.join(pkg, "Support/Info.plist"), plist);
const version = JSON.parse(readFileSync(path.join(desktop, "package.json"), "utf8")).version;
run("plutil", ["-replace", "CFBundleShortVersionString", "-string", version, plist]);
run("plutil", ["-replace", "CFBundleVersion", "-string", version, plist]);
run("plutil", ["-lint", plist]);
// The notch's shapes and placement credit MIT projects (NotchDrop, CodeIsland, DynamicNotchKit).
cpSync(path.join(repo, "apps/cua-spaces-macos/THIRD_PARTY_NOTICES.md"), path.join(contents, "Resources/THIRD_PARTY_NOTICES.md"));

run("codesign", ["--force", "--sign", "-", "--identifier", BUNDLE_ID, staging]);
run("codesign", ["--verify", "--strict", staging]);
// Decodes the protocol's samples without opening a window.
process.stdout.write(run(binary, ["--selftest"], { env: { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: process.env.HOME ?? "/" } }));

const app = path.join(out, APP);
rmSync(app, { recursive: true, force: true });
renameSync(staging, app);
if (!existsSync(path.join(app, "Contents/MacOS", EXECUTABLE))) throw new Error(`no helper in ${app}`);
console.log(`built ${path.relative(desktop, app)} (${archs.join(" + ")}, macOS ${MIN_MACOS}+)`);
