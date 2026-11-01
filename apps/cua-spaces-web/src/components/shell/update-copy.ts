// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { UpdateState } from "@/bridge";

/** What the sidebar's update button shows for the host's updater. It is
 * quiet: idle and checking show nothing. */
export type UpdateView =
  | { kind: "downloading"; label: string; percent: number | null }
  | { kind: "ready"; label: string; detail: string }
  | { kind: "installing"; label: string }
  | { kind: "error"; label: string };

const versionText = (version: string | null) => (version ? ` ${version}` : "");

/** The button for `state` (`installing`: "Restart to update" is running);
 * null when there is nothing to show. A failed step shows a muted icon:
 * the next check tries again on its own. */
export function updateView(state: UpdateState | undefined, installing = false): UpdateView | null {
  if (!state) return null;
  switch (state.status) {
    case "available":
    case "downloading": {
      const percent = state.status === "downloading" && state.percent !== null ? Math.max(0, Math.min(100, Math.floor(state.percent))) : null;
      return { kind: "downloading", label: `Downloading update${versionText(state.version)}${percent === null ? "…" : ` (${percent}%)`}`, percent };
    }
    case "downloaded":
      if (installing) return { kind: "installing", label: `Installing update${versionText(state.version)}…` };
      return { kind: "ready", label: "Update ready", detail: state.version ? `Cua Spaces ${state.version}` : "A new version of Cua Spaces" };
    case "error":
      return { kind: "error", label: `Couldn't update${state.message ? `: ${state.message.replace(/\.$/, "")}` : ""}. Cua Spaces tries again later.` };
    default:
      return null;
  }
}

/** The button's tooltip. */
export function updateTooltip(view: UpdateView): string {
  return view.kind === "ready" ? `${view.label}: restart to install` : view.label;
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
