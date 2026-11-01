// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's runtimes for Settings, Advanced: an Apple silicon Mac
 * with Docker and Podman, Homebrew's Lume and Apple's container installed,
 * shaped like the SwiftUI host's `settings.get` `runtimes`
 * (`runtimes::RuntimesInput`), and its `runtimes.check` / `runtimes.browse`.
 */

import type { DetectedRuntime, RuntimeKindInput, RuntimePathCheck, RuntimesInput } from "../../contracts/host";

/** What the demo Mac has, per tool: its path and `--version`. */
const INSTALLED: Record<string, { tool: string; name: string; version: string; guests?: string; kind: string; minVersion?: string }> = {
  "/usr/local/bin/docker": { tool: "docker", name: "Docker", version: "27.3.1", kind: "container" },
  "/opt/homebrew/bin/podman": { tool: "podman", name: "Podman", version: "5.2.1", kind: "container" },
  "/opt/homebrew/bin/lume": { tool: "lume", name: "Lume", version: "0.6.1", guests: "macOS", kind: "vm", minVersion: "0.6.0" },
  "/usr/local/bin/container": { tool: "container", name: "Apple container", version: "0.6.0", guests: "Linux", kind: "microvm" },
};

/** Another binary someone might type or browse to. */
const OTHER: Record<string, (typeof INSTALLED)[string]> = {
  "/Users/demo/bin/docker": { tool: "docker", name: "Docker", version: "28.0.1", kind: "container" },
  "/opt/homebrew/Cellar/lume/0.5.3/bin/lume": { tool: "lume", name: "Lume", version: "0.5.3", guests: "macOS", kind: "vm", minVersion: "0.6.0" },
};

const EXPECTED: Record<string, string[]> = { container: ["Docker", "Podman"], vm: ["Lume"], microvm: ["Apple container"] };

/** `runtimes.check`: what `path` is for `kind` on the demo Mac. */
export function demoProbe(kind: string, path: string): RuntimePathCheck {
  const p = path.trim();
  const known = INSTALLED[p] ?? OTHER[p];
  const expected = EXPECTED[kind] ?? [];
  const base: RuntimePathCheck = { path: p, exists: false, executable: false, expected };
  if (known && known.kind === kind) {
    return { ...base, exists: true, executable: true, tool: known.tool, name: known.name, version: known.version, minVersion: known.minVersion ?? null, guests: known.guests ?? null };
  }
  // Something else that runs (`/bin/ls`), a file that doesn't (`/etc/hosts`), or nothing.
  if (known || p.startsWith("/bin/") || p.startsWith("/usr/bin/")) return { ...base, exists: true, executable: true };
  if (p.startsWith("/etc/")) return { ...base, exists: true };
  return base;
}

function detected(kind: string): DetectedRuntime[] {
  return Object.entries(INSTALLED)
    .filter(([, t]) => t.kind === kind)
    .map(([path, t]) => ({ tool: t.tool, name: t.name, path, version: t.version, guests: t.guests ?? null }));
}

export interface DemoRuntimes {
  sources: Record<string, string>;
  paths: Record<string, string | null>;
}

export const demoRuntimesState = (): DemoRuntimes => ({
  sources: { container: "auto", vm: "auto", microvm: "auto" },
  paths: { container: null, vm: null, microvm: null },
});

/** The demo host's `runtimes` for `settings.get`. */
export function demoRuntimesInput(r: DemoRuntimes): RuntimesInput {
  const kinds: RuntimeKindInput[] = ["container", "vm", "microvm"].map((kind) => {
    const found = detected(kind);
    const inUse = r.paths[kind] ?? found[0]?.path ?? null;
    return {
      kind,
      source: r.sources[kind] ?? "auto",
      path: r.paths[kind] ?? null,
      detected: found,
      check: inUse ? demoProbe(kind, inUse) : null,
      expected: EXPECTED[kind] ?? [],
    };
  });
  // The demo Mac's doctor, as New Space's (`demoMacHostOptions`): no microVMs (macOS 15).
  return { os: "macos", hostArch: "arm64", localBackends: ["lume", "qemu", "container"], localDetails: { managed: "not used: this Mac's own container engine runs Linux Spaces" }, kinds };
}

/** A runtime row's choice (`settings.choose`), as the SwiftUI host saves it
 * (`runtimes::choose`): the kind's source, or its path. */
export function demoChooseRuntime(r: DemoRuntimes, row: string, option: string): boolean {
  const [prefix, kind] = row.split(":");
  if (!kind || !(kind in r.sources)) return false;
  if (prefix === "runtime") r.sources[kind] = ["auto", "builtin", "system"].includes(option) ? option : "auto";
  else if (prefix === "runtime-path") r.paths[kind] = option.trim() || null;
  else if (prefix === "runtime-detected") r.paths[kind] = detected(kind)[0]?.path === option ? null : option;
  else return false;
  return true;
}

/** `runtimes.browse`: the file the demo's panel "picks". */
export const DEMO_BROWSED: Record<string, string> = {
  container: "/Users/demo/bin/docker",
  vm: "/opt/homebrew/Cellar/lume/0.5.3/bin/lume",
  microvm: "/usr/local/bin/container",
};
