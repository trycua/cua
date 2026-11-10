// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { UpdateState } from "@/bridge";
import { t } from "@/i18n";

/** What the sidebar's update button shows for the host's updater. It is
 * quiet: idle and checking show nothing. */
export type UpdateView =
  | { kind: "downloading"; label: string; percent: number | null }
  | { kind: "ready"; label: string; detail: string }
  | { kind: "installing"; label: string }
  | { kind: "error"; label: string };

/** The button for `state` (`installing`: "Restart to update" is running);
 * null when there is nothing to show. A failed step shows a muted icon:
 * the next check tries again on its own. */
export function updateView(state: UpdateState | undefined, installing = false): UpdateView | null {
  if (!state) return null;
  switch (state.status) {
    case "available":
    case "downloading": {
      const percent = state.status === "downloading" && state.percent !== null ? Math.max(0, Math.min(100, Math.floor(state.percent))) : null;
      const key = `shell.update.downloading${state.version ? "_version" : ""}${percent === null ? "" : "_percent"}`;
      return { kind: "downloading", label: t(key, { version: state.version ?? "", percent: percent ?? 0 }), percent };
    }
    case "downloaded":
      if (installing)
        return {
          kind: "installing",
          label: t(state.version ? "shell.update.installing_version" : "shell.update.installing", { version: state.version ?? "" }),
        };
      return {
        kind: "ready",
        label: t("shell.update.ready"),
        detail: state.version ? t("shell.update.ready_version", { version: state.version }) : t("shell.update.ready_new_version"),
      };
    case "error":
      return { kind: "error", label: state.message ? t("shell.update.error_reason", { reason: state.message.replace(/\.$/, "") }) : t("shell.update.error") };
    default:
      return null;
  }
}

/** The button's tooltip. */
export function updateTooltip(view: UpdateView): string {
  return view.kind === "ready" ? t("shell.update.ready_tooltip", { label: view.label }) : view.label;
}

/** Work a restart would cut short: the runs still working. */
export function runsInProgress(runs: readonly { status: string }[] | undefined): number {
  return runs?.filter((r) => r.status === "running").length ?? 0;
}

/** The question before "Restart to update" while work is running. */
export function restartWarning(running: number): { title: string; description: string; confirm: string; cancel: string } {
  return {
    title: t("shell.update.restart_title"),
    description: t("shell.update.restart_description", { count: running }),
    confirm: t("shell.update.restart_confirm"),
    cancel: t("shell.update.restart_cancel"),
  };
}
