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
import { useT } from "@/i18n";
import { thisComputerText } from "@/lib/host-labels";

/** The host form's profile for your own connection server or Direct (`host::SELF_HOSTED_PROFILE`). */
const SELF_HOSTED = "self-hosted";

/** Experiments that stay as Labs. "Your cloud" and "Sharing" have no switch any more: the plan decides on the server. */
const LABS = new Set(["experiment:cua_volume", "experiment:ci_runners", "experiment:tcp_shares", "experiment:mobile"]);

/**
 * Settings, Advanced: the update channel (Stable or Nightly), Labs, which software runs Spaces on
 * this Mac, and your own connection server. For options most people never
 * touch; each says what it does in a line.
 */
export function AdvancedTab() {
  const t = useT();
  const { data: settings, chooseSetting, isLoading: settingsLoading } = useSettings();
  const { isLoading: aboutLoading } = useAbout();
  const { isLoading: experimentsLoading } = useExperiments();
  const page = settings?.page;
  const general = page?.sections.find((s) => s.id === "general");
  const runtimes = page?.sections.find((s) => s.id === "runtimes");
  const autoConnect = general?.rows.find((r) => r.id === "auto-connect");
  const overflow = general?.rows.find((r) => r.id === "placement-overflow");
  const choose = (row: CoreRow, option: string) =>
    void chooseSetting(row.id, option).catch(toastError(t("settings.advanced.change_failed", { setting: row.label })));

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
        <SettingsGroup title={t("settings.advanced.runtimes_title")}>
          {/* One row per kind of Space, as the core lays them out (always there, "Checking…" until the host read them). */}
          {runtimes ? <RuntimeKindRows section={runtimes} onChoose={choose} /> : null}
          {autoConnect ? <CoreSettingsRow row={{ ...autoConnect, label: t("settings.advanced.auto_connect") }} onChoose={choose} /> : null}
          {overflow ? (
            <CoreSettingsRow row={{ ...overflow, label: t("settings.advanced.overflow", { computer: thisComputerText() }) }} onChoose={choose} />
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
  const t = useT();
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
    ? {
        ...row,
        id: "update-channel-note",
        kind: "note",
        label: updates.channelNote,
        options: [],
        linkLabel: updates.downloadLabel ?? null,
        linkUrl: updates.downloadUrl ?? null,
      }
    : null;
  return (
    <SettingsGroup title={t("settings.advanced.updates")}>
      <CoreSettingsRow
        row={row}
        description={updates.channelHelp}
        onChoose={(_, option) =>
          void setAbout({ channel: option === "nightly" ? "nightly" : "stable" }).catch(toastError(t("settings.advanced.channel_failed")))
        }
      />
      {note ? <CoreSettingsRow row={note} /> : null}
    </SettingsGroup>
  );
}

/** An On/Off switch kept on this device only. */
function deviceToggleRow(id: string, label: string, on: boolean, say: ReturnType<typeof useT>): CoreRow {
  return {
    id,
    kind: "toggle",
    label,
    value: null,
    button: null,
    options: [
      { id: "on", label: say("settings.advanced.on"), active: on },
      { id: "off", label: say("settings.advanced.off"), active: !on },
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
function thisMacAskRow(asks: boolean, say: ReturnType<typeof useT>): CoreRow {
  return deviceToggleRow("ask-first:this-mac", say("settings.advanced.ask_first", { computer: thisComputerText() }), asks, say);
}

/** Home's own setting: whether a task asks before it works on this computer. */
function HomeGroup() {
  const t = useT();
  const thisMacOk = useThisMacConfirmed();
  return (
    <SettingsGroup title={t("settings.advanced.home")}>
      <CoreSettingsRow
        row={thisMacAskRow(!thisMacOk, t)}
        description={t("settings.advanced.ask_first_help", { computer: thisComputerText() })}
        onChoose={(_, option) => setThisMacConfirmed(option === "off")}
      />
    </SettingsGroup>
  );
}

/** The experiments still in Labs. Off hides; nothing set up is undone. */
function Labs() {
  const t = useT();
  const { data, chooseExperiment } = useExperiments();
  const rows = (data?.page?.sections.flatMap((s) => s.rows) ?? []).filter((r) => LABS.has(r.id));
  const note: Record<string, string> = {
    "experiment:cua_volume": t("settings.advanced.labs_cua_volume"),
    "experiment:ci_runners": t("settings.advanced.labs_ci_runners"),
    "experiment:tcp_shares": t("settings.advanced.labs_tcp_shares"),
    "experiment:mobile": t("settings.advanced.labs_mobile"),
  };
  if (!rows.length) return null;
  return (
    <SettingsGroup title={t("settings.advanced.labs")} footer={t("settings.advanced.labs_footer")}>
      {rows.map((r) => (
        <CoreSettingsRow
          key={r.id}
          row={r}
          description={note[r.id]}
          onChoose={(row, option) => void chooseExperiment(row.id, option).catch(toastError(t("settings.advanced.labs_failed")))}
        />
      ))}
    </SettingsGroup>
  );
}

/** For teams that run their own connection server (the core's "relay"), or connect directly: host setup's form, with its advanced part open. */
function OwnConnection() {
  const t = useT();
  const { data: session } = useSession();
  const { data: machines } = useMachines();
  const host = useThisMachine(session?.identity ?? null);
  const form = host.formView?.advancedOpen ? host.formView : null;
  // "Use a code from another computer", when the server wants proof it's the owner adding this computer.
  const [enterCode, setEnterCode] = useState(false);
  return (
    <SettingsGroup title={t("settings.advanced.connection")}>
      <AddComputerSheet open={enterCode} onOpenChange={setEnterCode} initialView="enter" />
      {form ? (
        <HostSetupForm
          view={form}
          host={host}
          failure={host.setupError}
          progress={machines?.find((m) => m.current)?.hostProgress}
          onRetry={() => void host.retrySetUp()}
          onUseCode={() => setEnterCode(true)}
        />
      ) : (
        <SettingRow
          data-self-hosting=""
          label={t("settings.advanced.own_server")}
          control={
            <Button size="sm" variant="outline" disabled={host.busy} onClick={() => host.openForm(SELF_HOSTED)}>
              {t("settings.advanced.own_server_set_up")}
            </Button>
          }
        />
      )}
    </SettingsGroup>
  );
}
