// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The bundled CLI on PATH (`cli.plan`, `cli.install`; the web bridge's
// ops/cli.ts): the first run's "Install the Cua CLI" screen and Settings'
// Command line row, as the SwiftUI host answers them
// (`WebUIBridge+Cli.swift`). Both run the app core's installer
// (`AppCliInstaller`): the plan writes
// nothing; Install writes the channel's CLI (`cua`, or `cua-local`) and adds
// its folder to PATH only when asked. `settings.get`'s `cli` names it from
// the last plan read.
import path from "node:path";

import type { AppCliInstallPlan } from "../native/generated/index";
import type { AppModel } from "../model/app-model";
import { recordCliInstall } from "../model/cli-record";
import { bool } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { encode } from "./value";

/** The installer for this app's bundled `cua` (it finds the CLI next to the "executable" it is given). */
export function cliInstaller({ model, supervisor }: Pick<BridgeContext, "model" | "supervisor">) {
  if (!supervisor) return null;
  return model.native.AppCliInstaller.forExecutable(path.join(path.dirname(supervisor.cua), "cua-spaces"));
}

/** `settings.get`'s `cli`: the plan's file name and whether it is on this computer; null until read. */
export function cliSetting(model: AppModel): { name: string; installed: boolean } | null {
  const plan = model.cliPlan;
  if (!plan) return null;
  const file = path.basename(plan.target.replaceAll("\\", "/"));
  return { name: file.replace(/\.exe$/i, ""), installed: plan.upToDate || Boolean(plan.found) };
}

/** Keeps `plan` as the last read and tells the page when `settings.get`'s `cli` changed. */
function notePlan(ctx: Pick<BridgeContext, "model" | "events">, plan: AppCliInstallPlan) {
  const before = JSON.stringify(cliSetting(ctx.model));
  ctx.model.cliPlan = plan;
  if (JSON.stringify(cliSetting(ctx.model)) !== before) ctx.events.emit("settings.changed");
  return encode(plan);
}

/** Reads the plan once (at launch), so the pages know whether the CLI is here. */
export async function readCliPlan(ctx: BridgeContext): Promise<void> {
  const installer = cliInstaller(ctx);
  if (installer) notePlan(ctx, await installer.plan());
}

export function cliMethods(ctx: BridgeContext): Handlers {
  const note = (plan: AppCliInstallPlan) => notePlan(ctx, plan);
  return {
    "cli.plan": async () => {
      const installer = cliInstaller(ctx);
      return installer ? note(await installer.plan()) : null;
    },
    "cli.install": async (args) => {
      const modifyPath = bool(args, "modifyPath");
      const installer = cliInstaller(ctx);
      if (!installer) throw Failure.unsupported("This build has no bundled cua to install.");
      const planned = await installer.plan();
      const installed = await installer.install({ modifyPath });
      // Windows: the uninstaller removes what this install added (the PATH entry only when it was added now).
      await recordCliInstall(installed.target, modifyPath && !planned.onPath, ctx.platform).catch((error) =>
        console.warn(`[cua-spaces] could not record the cua command for the uninstaller: ${String(error)}`),
      );
      return note(installed);
    },
  };
}
