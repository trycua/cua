// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Checks that the built shell carries the web bridge's Electron contract:
// the preload exposes the one bridge channel and the one event channel
// (apps/cua-spaces-web/src/bridge/electron-channels.ts) and no other, and
// the main bundle routes every method of src/bridge/methods.ts (the SwiftUI
// host's `WebUIBridge.methods` plus this host's own). That the list matches
// the Swift host and the web contract, and that the registry answers each
// one, is test/bridge-registry.test.ts's. Run after `tsdown` (`pnpm build` does).
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const quoted = (block) => [...block.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
const read = (file) => readFileSync(path.join(root, file), "utf8");

const contract = readFileSync(path.resolve(root, "../cua-spaces-web/src/bridge/electron-channels.ts"), "utf8");
const constant = (name) => new RegExp(`export const ${name} = "([^"]+)"`).exec(contract)?.[1];
const bridgeChannel = constant("ELECTRON_BRIDGE_CHANNEL");
const eventChannel = constant("ELECTRON_EVENT_CHANNEL");

const methodsSrc = read("src/bridge/methods.ts");
const methods = [
  ...quoted(/export const METHODS = \[([\s\S]*?)\] as const/.exec(methodsSrc)?.[1] ?? ""),
  ...quoted(/export const ELECTRON_METHODS = \[([\s\S]*?)\] as const/.exec(methodsSrc)?.[1] ?? ""),
];

const problems = [];
if (!bridgeChannel || !eventChannel) problems.push("electron-channels.ts names no bridge or event channel");
if (methods.length < 50) problems.push(`src/bridge/methods.ts lists only ${methods.length} methods`);
const preload = read("dist-electron/preload.cjs");
const main = read("dist-electron/main.cjs");
for (const channel of [bridgeChannel, eventChannel]) {
  if (channel && !preload.includes(`"${channel}"`)) problems.push(`preload.cjs lacks ${channel}`);
  if (channel && !main.includes(`"${channel}"`)) problems.push(`main.cjs lacks ${channel}`);
}
// The old per-operation channels are gone: the preload allows the bridge channel only.
for (const m of preload.matchAll(/"(cua:[\w.]+)"/g)) {
  if (m[1] !== bridgeChannel && m[1] !== eventChannel) problems.push(`preload.cjs names another channel: ${m[1]}`);
}
for (const method of methods) if (!main.includes(`"${method}"`)) problems.push(`main.cjs routes no ${method}`);

if (problems.length) {
  console.error(`channel check failed:\n  ${problems.join("\n  ")}`);
  process.exit(1);
}
console.log(`channel check: ${bridgeChannel} and ${eventChannel}, ${methods.length} methods routed`);
