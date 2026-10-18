// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { UpdateState } from "@/bridge";

/** What the in-app update indicator shows for the host's updater. It is quiet:
 * only a found, a downloading or a downloaded update shows anything. */
export type UpdateView =
  | { kind: "downloading"; label: string; percent: number | null }
  | { kind: "ready"; label: string; detail: string };

const versionText = (version: string | null) => (version ? ` ${version}` : "");

/** The indicator for `state`; null when there is nothing to show (idle,
 * checking, or a failed background step: the next check tries again). */
export function updateView(state: UpdateState | undefined): UpdateView | null {
  if (!state) return null;
  switch (state.status) {
    case "available":
    case "downloading": {
      const percent = state.status === "downloading" && state.percent !== null ? Math.max(0, Math.min(100, Math.floor(state.percent))) : null;
      return { kind: "downloading", label: `Downloading update${versionText(state.version)}${percent === null ? "…" : ` (${percent}%)`}`, percent };
    }
    case "downloaded":
      return { kind: "ready", label: "Update ready", detail: state.version ? `Cua Spaces ${state.version}` : "A new version of Cua Spaces" };
    default:
      return null;
  }
}

/** Work a restart would cut short: the runs still working. */
export function runsInProgress(runs: readonly { status: string }[] | undefined): number {
  return runs?.filter((r) => r.status === "running").length ?? 0;
}

/** The question before "Restart to update" while work is running. */
export function restartWarning(running: number): { title: string; description: string; confirm: string; cancel: string } {
  return {
    title: "Restart to update?",
    description:
      running === 1
        ? "An agent is still working. Restarting Cua Spaces now stops it. You can also wait for it to finish and restart later."
        : `${running} agents are still working. Restarting Cua Spaces now stops them. You can also wait for them to finish and restart later.`,
    confirm: "Restart anyway",
    cancel: "Not now",
  };
}
