// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { useAbout, useBridge, useExperiments, useMachines, useSession, useSettings, useThisMachine, type SettingsRow as CoreRow } from "@/bridge";
import { aboutView } from "@/bridge/settings-derive";
import { SettingsGroup } from "@/components/settings-group";
import { SharedLinks } from "@/components/links/shared-links";
import { SharingSettings } from "@/components/links/sharing-settings";
import { SettingRow } from "@/components/ui/grouped";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { RuntimeKindRows } from "@/components/settings/runtime-rows";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { HostSetupForm } from "@/components/this-machine";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { setThisMacConfirmed, useThisMacConfirmed } from "@/lib/ask-first";
import { thisComputerText } from "@/lib/host-labels";

/** The host form's profile for your own connection server or Direct (`host::SELF_HOSTED_PROFILE`). */
const SELF_HOSTED = "self-hosted";

/** Experiments that stay as Labs. "Your cloud" and "Sharing" have no switch any more: the plan decides on the server. */
const LABS = new Set(["experiment:cua_volume", "experiment:secure_spaces", "experiment:child_spaces", "experiment:ask_first", "experiment:ci_runners", "experiment:tcp_shares", "experiment:mobile", "experiment:warm_pools"]);

/**
 * Settings, Advanced: the update channel (Stable or Nightly), Labs, which software runs Spaces on
 * this Mac, and your own connection server. For options most people never
 * touch; each says what it does in a line.
 */
export function AdvancedTab() {
  const { data: settings, chooseSetting, isLoading: settingsLoading } = useSettings();
  const { isLoading: aboutLoading } = useAbout();
  const { isLoading: experimentsLoading } = useExperiments();
  const page = settings?.page;
  const general = page?.sections.find((s) => s.id === "general");
  const runtimes = page?.sections.find((s) => s.id === "runtimes");
  const autoConnect = general?.rows.find((r) => r.id === "auto-connect");
  const overflow = general?.rows.find((r) => r.id === "placement-overflow");
  const choose = (row: CoreRow, option: string) => void chooseSetting(row.id, option).catch(toastError(`Couldn't change ${row.label}`));

  // Nothing is drawn until the host has answered for every group, so none of them pushes
  // another down as it arrives (a layout shift of the whole tab).
  if (settingsLoading || aboutLoading || experimentsLoading) return <div data-settings-tab="advanced" aria-busy="true" />;

  return (
    <div data-settings-tab="advanced">
      <UpdateChannel />
      <HomeGroup />
      <Labs />
      <Sharing />
      {runtimes || autoConnect || overflow ? (
        <SettingsGroup title="Which software runs Spaces on this computer">
          {/* One row per kind of Space, as the core lays them out (always there, "Checking…" until the host read them). */}
          {runtimes ? <RuntimeKindRows section={runtimes} onChoose={choose} /> : null}
          {autoConnect ? <CoreSettingsRow row={{ ...autoConnect, label: "Open a Space’s screen right away" }} onChoose={choose} /> : null}
          {overflow ? (
            <CoreSettingsRow
              row={{ ...overflow, label: `Use my own cloud when ${thisComputerText()} is busy` }}
              description={`New Spaces go to your linked cloud when ${thisComputerText()} is full.`}
              onChoose={choose}
            />
          ) : null}
        </SettingsGroup>
      ) : null}
      <OwnConnection />
    </div>
  );
}

/**
 * Updates: Stable or Nightly, as the core lays them out (`about::view`). The
 * old "Get early releases (Beta)" switch; whoever had it on is on Nightly.
 * A prerelease build back on Stable says it stays until the next stable
 * release, with a link to download Stable now.
 */
function UpdateChannel() {
  const { data, setAbout } = useAbout();
  const { mode, core } = useBridge();
  const view = data?.view;
  if (!view || !data) return null;
  // A build with no update feed (a local build) keeps the row, greyed out.
  const unavailable = !view.updates && mode === "electron" ? (aboutView(core, { ...data.input, updater: true })?.updates ?? null) : null;
  const updates = view.updates ?? unavailable;
  if (!updates) return null;
  const row: CoreRow = {
    id: "update-channel",
    kind: "choice",
    label: updates.channelLabel,
    value: null,
    button: null,
    options: updates.channels,
    enabled: data.canChange && !unavailable,
    help: null,
    linkLabel: null,
    linkUrl: null,
    placeholder: null,
  };
  const note: CoreRow | null = updates.channelNote
    ? { ...row, id: "update-channel-note", kind: "note", label: updates.channelNote, options: [], linkLabel: updates.downloadLabel ?? null, linkUrl: updates.downloadUrl ?? null }
    : null;
  return (
    <SettingsGroup title="Updates">
      <CoreSettingsRow
        row={row}
        description={updates.channelHelp}
        onChoose={(_, option) => void setAbout({ channel: option === "nightly" ? "nightly" : "stable" }).catch(toastError("Couldn't change the update channel"))}
      />
      {note ? <CoreSettingsRow row={note} /> : null}
    </SettingsGroup>
  );
}

/** An On/Off switch kept on this device only. */
function deviceToggleRow(id: string, label: string, on: boolean): CoreRow {
  return {
    id,
    kind: "toggle",
    label,
    value: null,
    button: null,
    options: [
      { id: "on", label: "On", active: on },
      { id: "off", label: "Off", active: !on },
    ],
    enabled: true,
    help: null,
    linkLabel: null,
    linkUrl: null,
    placeholder: null,
  };
}

/** Public TCP shares (Labs): how they are shared, and what is shared now. Only while the Labs switch is on. */
function Sharing() {
  const on = useExperiments().data?.experiments.tcpShares === true;
  if (!on) return null;
  return (
    <>
      <SharingSettings />
      <SharedLinks />
    </>
  );
}

/** Whether a task asks before it works on this Mac itself (the answer to "Work on this Mac?" is kept per device). */
function thisMacAskRow(asks: boolean): CoreRow {
  return deviceToggleRow("ask-first:this-mac", `Ask before a task uses ${thisComputerText()}`, asks);
}

/** Home's own setting: whether a task asks before it works on this computer. */
function HomeGroup() {
  const thisMacOk = useThisMacConfirmed();
  return (
    <SettingsGroup title="Home">
      <CoreSettingsRow
        row={thisMacAskRow(!thisMacOk)}
        description={`Before a task works directly on ${thisComputerText()}, with your files and apps. Turn it on to be asked again.`}
        onChoose={(_, option) => setThisMacConfirmed(option === "off")}
      />
    </SettingsGroup>
  );
}

/** The experiments still in Labs. Off hides; nothing set up is undone. */
function Labs() {
  const { data, chooseExperiment } = useExperiments();
  const rows = (data?.page?.sections.flatMap((s) => s.rows) ?? []).filter((r) => LABS.has(r.id));
  const note: Record<string, string> = {
    "experiment:cua_volume": "Memory and files your agents share across every Space.",
    "experiment:secure_spaces": "A Space reaches the internet and nothing else on your network unless you open it.",
    "experiment:child_spaces": "A Space can make Spaces of its own, inside a budget you set.",
    "experiment:ask_first": "An agent asks you on a card before anything leaves its Space, such as a push to GitHub.",
    "experiment:ci_runners": "Run GitHub Actions jobs in a fresh Space each, on this computer. Adds CI Runners to the sidebar.",
    "experiment:tcp_shares": "Share a game server or another TCP service in a Space at a public address.",
    "experiment:mobile": "Your computers answer the Cua Spaces phone app: start a task, follow the thread and review changes.",
    "experiment:warm_pools": "Keep one Linux Space ready on this computer, with its coding apps installed, so the next Create from Home starts at once.",
  };
  if (!rows.length) return null;
  return (
    <SettingsGroup title="Labs">
      {rows.map((r) => (
        <CoreSettingsRow
          key={r.id}
          row={r}
          description={note[r.id]}
          onChoose={(row, option) => void chooseExperiment(row.id, option).catch(toastError("Couldn't change the setting"))}
        />
      ))}
    </SettingsGroup>
  );
}

/** For teams that run their own connection server (the core's "relay"), or connect directly: host setup's form, with its advanced part open. */
function OwnConnection() {
  const { data: session } = useSession();
  const { data: machines } = useMachines();
  const host = useThisMachine(session?.identity ?? null);
  const form = host.formView?.advancedOpen ? host.formView : null;
  // "Use a code from another computer", when the server wants proof it's the owner adding this computer.
  const [enterCode, setEnterCode] = useState(false);
  return (
    <SettingsGroup title="Connection">
      <AddComputerSheet open={enterCode} onOpenChange={setEnterCode} initialView="enter" />
      {form ? (
        <HostSetupForm view={form} host={host} failure={host.setupError} progress={machines?.find((m) => m.current)?.hostProgress} onRetry={() => void host.retrySetUp()} onUseCode={() => setEnterCode(true)} />
      ) : (
        <SettingRow
          data-self-hosting=""
          label="Use your own connection server"
          help="For teams that run their own."
          control={
            <Button size="sm" variant="outline" disabled={host.busy} onClick={() => host.openForm(SELF_HOSTED)}>
              Set up…
            </Button>
          }
        />
      )}
    </SettingsGroup>
  );
}
