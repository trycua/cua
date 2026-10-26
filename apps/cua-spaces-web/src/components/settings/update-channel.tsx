// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { useAbout, useAgents, useBridge, useSession, useUpdate, type AboutUpdates, type SettingsRow as CoreRow, type UpdateState } from "@/bridge";
import { aboutView } from "@/bridge/settings-derive";
import { SettingsGroup } from "@/components/settings-group";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { restartWarning, runsInProgress } from "@/components/shell/update-copy";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { toast, toastError } from "@/components/ui/toast";

/** What the updater is doing, in a line; null when it is idle. */
export function channelStatus(state: UpdateState | undefined, checking: boolean): { text: string; ready: boolean } | null {
  const version = state?.version ? ` ${state.version}` : "";
  switch (state?.status) {
    case "available":
    case "downloading": {
      const percent = state.status === "downloading" && state.percent !== null ? ` (${Math.max(0, Math.min(100, Math.floor(state.percent)))}%)` : "";
      return { text: `Downloading Cua Spaces${version}${percent}…`, ready: false };
    }
    case "downloaded":
      return { text: `Cua Spaces${version} is ready. Restart to start using it.`, ready: true };
    case "error":
      return { text: state.message ? `Couldn't update: ${state.message}` : "Couldn't update. Cua Spaces tries again later.", ready: false };
    case "checking":
      return { text: "Checking for updates…", ready: false };
    default:
      return checking ? { text: "Checking for updates…", ready: false } : null;
  }
}

/** The picker's row for the core's channels: Stable, Beta and, on the Mac app, Nightly. */
function channelRow(u: AboutUpdates, enabled: boolean): CoreRow {
  return {
    id: "update-channel",
    kind: "choice",
    label: u.channelLabel,
    value: null,
    button: null,
    options: u.channels,
    enabled,
    help: null,
    linkLabel: null,
    linkUrl: null,
    placeholder: null,
  };
}

/**
 * Settings, Advanced, Updates: where updates come from (Stable, Beta or
 * Nightly). Choosing a channel saves it and the host checks that channel at
 * once; what it finds downloads on its own and is offered here (and in the
 * sidebar) as "Restart to update". A nightly build back on Stable says it
 * stays until the next Stable release, with a download to go back now.
 */
export function UpdateChannelGroup() {
  const { data, setAbout } = useAbout();
  const { data: update, install } = useUpdate();
  const { data: agents } = useAgents();
  const { openExternal } = useSession();
  const { mode, core } = useBridge();
  const [asking, setAsking] = useState(false);
  const view = data?.view;
  if (!view || !data) return null;
  // A build with no update feed (a local build) keeps the row, greyed out.
  const unavailable = !view.updates && mode === "electron" ? (aboutView(core, { ...data.input, updater: true })?.updates ?? null) : null;
  const updates = view.updates ?? unavailable;
  if (!updates) return null;

  const status = unavailable ? null : channelStatus(update?.state, data.input.checking);
  const busy = status !== null && status.text !== "Checking for updates…" && update?.state.status !== "error";
  const running = runsInProgress(agents?.runs);
  const restartNow = () => void install().catch((e: unknown) => toast("Couldn't restart to update", { description: e instanceof Error ? e.message : String(e) }));
  const restart = () => (running > 0 ? setAsking(true) : restartNow());
  const warning = restartWarning(running);
  // Once something newer is on its way, the note about staying on this build no longer holds.
  const note = busy ? null : updates.channelNote;
  const download = !busy && updates.downloadUrl ? { label: updates.downloadLabel ?? "Download", url: updates.downloadUrl } : null;

  return (
    <SettingsGroup title="Updates">
      <CoreSettingsRow
        row={channelRow(updates, data.canChange && !unavailable)}
        description={updates.channelHelp}
        onChoose={(_, option) => void setAbout({ channel: option as "stable" | "beta" | "nightly" }).catch(toastError("Couldn't change where updates come from"))}
      />
      {status ? (
        <div className="flex min-h-12 items-center justify-between gap-6 px-4 py-2.5" data-update-channel-status={update?.state.status ?? "checking"}>
          <p className="min-w-0 text-[13px]" aria-live="polite">
            {status.text}
          </p>
          {status.ready ? (
            <Button size="sm" onClick={restart} disabled={update?.installing ?? false} data-update-channel-restart="">
              Restart to update
            </Button>
          ) : null}
        </div>
      ) : null}
      {note || download ? (
        <div className="flex items-baseline justify-between gap-6 px-4 py-2.5 text-xs text-muted-foreground" data-update-channel-note="">
          <span>{note}</span>
          {download ? (
            <Button size="sm" variant="outline" className="shrink-0" onClick={() => void openExternal(download.url).catch(toastError(`Couldn't open ${download.label}`))} data-update-channel-download="">
              {download.label}
            </Button>
          ) : null}
        </div>
      ) : null}
      <ConfirmDialog open={asking} onOpenChange={setAsking} title={warning.title} description={warning.description} confirmLabel={warning.confirm} cancelLabel={warning.cancel} destructive onConfirm={restartNow} />
    </SettingsGroup>
  );
}
