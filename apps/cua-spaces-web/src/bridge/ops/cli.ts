// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app's bundled CLI on PATH: the first run's "Install the Cua CLI"
 * screen and Settings' Command line row. Both hosts run the app core's
 * installer (`cua-spaces-app-core/src/installer.rs`, through the FFI's
 * `AppCliInstaller`): `plan` reads what installing would do and writes
 * nothing; `install` writes only when the person pressed Install. What it
 * installs is the build channel's (`cua`, or `cua-local` for Cua Spaces
 * Local), and it adds the bin folder to PATH only when it is not there.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `cli.plan` | `AppCliInstaller.plan` (`src/bridge/cli.ts`) | `AppCliInstaller.plan` (`WebUIBridge+Cli.swift`) | unsupported |
 * | `cli.install {modifyPath}` | `AppCliInstaller.install` | `AppCliInstaller.install` | unsupported |
 *
 * After an install the host's `settings.get` names the CLI as installed
 * (`SettingsSnapshot.cli`), so the pages' terminal hints show.
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";
import type { CliInstallPlan } from "../contracts/first-run";

export interface CliOps {
  /** What installing would do; null where this app carries no CLI to install. */
  "cli.plan": { args: Record<string, never>; result: CliInstallPlan | null };
  /** Installs it (after Install was pressed); `modifyPath` adds the bin folder to PATH. */
  "cli.install": { args: { modifyPath: boolean }; result: CliInstallPlan };
}

export const CLI_OPERATIONS = ["cli.plan", "cli.install"] as const satisfies readonly (keyof CliOps)[];

const TAURI = "the Tauri app installs the CLI on its own installer page (`installer_cli_plan`, `installer_install_cli`)";

export const CLI_COVERAGE = {
  "cli.plan": { webkit: { methods: ["cli.plan"] }, tauri: [], unsupported: { tauri: TAURI } },
  "cli.install": { webkit: { methods: ["cli.install"] }, tauri: [], unsupported: { tauri: TAURI } },
} as const satisfies Record<keyof CliOps, OpCoverage>;

/** A host that can't install the CLI. */
export function unsupportedCliOps(mode: BridgeMode) {
  const no = (op: keyof CliOps) => (): Promise<never> => Promise.reject(new UnsupportedOperationError(mode, op));
  return { "cli.plan": no("cli.plan"), "cli.install": no("cli.install") };
}

/** Arguments that take each down its usual path (coverage tests). */
export const CLI_ARGS = {
  "cli.plan": {},
  "cli.install": { modifyPath: true },
};
