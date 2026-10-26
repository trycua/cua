// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { setFlagOverride, useAbout, useBridge, useExperiments, useFlag, useMachines, useSession, useSettings, useThisMachine, type SettingsOption, type SettingsRow as CoreRow } from "@/bridge";
import { aboutView } from "@/bridge/settings-derive";
import { SettingsGroup } from "@/components/settings-group";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { HostSetupForm } from "@/components/this-machine";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { setThisMacConfirmed, useThisMacConfirmed } from "@/lib/ask-first";

/** The host form's profile for your own connection server or Direct (`host::SELF_HOSTED_PROFILE`). */
const SELF_HOSTED = "self-hosted";

const BETA_HELP = "New features sooner, with rough edges. Back on Stable, this version stays until a newer stable release.";

/** Experiments that stay as Labs. "Your cloud" and "Sharing" have no switch any more: the plan decides on the server. */
const LABS = new Set(["experiment:cua_volume"]);

/** The software choices in plain words (the core says Built-in and System). */
const RUNTIME_LABEL: Record<string, string> = { "macos-runtime": "Mac computers", "linux-runtime": "Linux computers" };
const RUNTIME_OPTION: Record<string, string> = { auto: "Automatic", builtin: "Cua’s own", system: "Already installed" };

const runtimeRow = (row: CoreRow): CoreRow => ({
  ...row,
  label: RUNTIME_LABEL[row.id] ?? row.label,
  options: row.options.map((o: SettingsOption) => ({ ...o, label: RUNTIME_OPTION[o.id] ?? o.label })),
});

/**
 * Settings, Advanced: early releases, Labs, which software runs Spaces on
 * this Mac, and your own connection server. For options most people never
 * touch; each says what it does in a line.
 */
export function AdvancedTab() {
  const { data: settings, chooseSetting } = useSettings();
  const page = settings?.page;
  const general = page?.sections.find((s) => s.id === "general");
  const runtimes = page?.sections.find((s) => s.id === "runtimes");
  const autoConnect = general?.rows.find((r) => r.id === "auto-connect");
  const overflow = general?.rows.find((r) => r.id === "placement-overflow");
  const choose = (row: CoreRow, option: string) => void chooseSetting(row.id, option).catch(toastError(`Couldn't change ${row.label}`));

  return (
    <div data-settings-tab="advanced">
      <EarlyReleases />
      <Labs />
      {runtimes || autoConnect || overflow ? (
        <SettingsGroup title="Which software runs Spaces on this Mac">
          {runtimes?.rows.filter((r) => r.kind === "choice").map((r) => <CoreSettingsRow key={r.id} row={runtimeRow(r)} asSelect onChoose={choose} />)}
          {autoConnect ? <CoreSettingsRow row={{ ...autoConnect, label: "Open a Space’s screen right away" }} onChoose={choose} /> : null}
          {overflow ? (
            <CoreSettingsRow
              row={{ ...overflow, label: "Use my own cloud when this Mac is busy" }}
              description="New Spaces go to your linked cloud when this Mac is full."
              onChoose={choose}
            />
          ) : null}
        </SettingsGroup>
      ) : null}
      <OwnConnection />
    </div>
  );
}

/** "Get early releases (Beta)": the update channel, moved here from About. */
function EarlyReleases() {
  const { data, setAbout } = useAbout();
  const { mode, core } = useBridge();
  const view = data?.view;
  if (!view || !data) return null;
  // A build with no update feed (a local build) keeps the row, greyed out.
  const unavailable = !view.updates && mode === "electron" ? (aboutView(core, { ...data.input, updater: true })?.updates ?? null) : null;
  const updates = view.updates ?? unavailable;
  if (!updates) return null;
  const beta = updates.channels.find((c) => c.id === "beta")?.active ?? false;
  const row: CoreRow = {
    id: "update-channel",
    kind: "toggle",
    label: "Get early releases (Beta)",
    value: null,
    button: null,
    options: [
      { id: "on", label: "On", active: beta },
      { id: "off", label: "Off", active: !beta },
    ],
    enabled: data.canChange && !unavailable,
    help: null,
    linkLabel: null,
    linkUrl: null,
    placeholder: null,
  };
  return (
    <SettingsGroup title="Updates">
      <CoreSettingsRow
        row={row}
        description={BETA_HELP}
        onChoose={(_, option) => void setAbout({ channel: option === "on" ? "beta" : "stable" }).catch(toastError("Couldn't change the update channel"))}
      />
    </SettingsGroup>
  );
}

/** The "Ask first" Home (`spaces_ask_first_home`): a switch for this device only. */
function askFirstRow(on: boolean): CoreRow {
  return {
    id: "flag:spaces_ask_first_home",
    kind: "toggle",
    label: "Try the new Home",
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

/** Whether a task asks before it works on this Mac itself (the answer to "Work on this Mac?" is kept per device). */
function thisMacAskRow(asks: boolean): CoreRow {
  return { ...askFirstRow(asks), id: "ask-first:this-mac", label: "Ask before a task uses this Mac", options: [
    { id: "on", label: "On", active: asks },
    { id: "off", label: "Off", active: !asks },
  ] };
}

/** Cua Volume and the new Home. Off hides; nothing set up is undone. */
function Labs() {
  const { data, chooseExperiment } = useExperiments();
  const askFirst = useFlag("spaces_ask_first_home");
  const thisMacOk = useThisMacConfirmed();
  const rows = (data?.page?.sections.flatMap((s) => s.rows) ?? []).filter((r) => LABS.has(r.id));
  const note: Record<string, string> = {
    "experiment:cua_volume": "Memory and files your agents share across every Space.",
  };
  return (
    <SettingsGroup title="Labs">
      <CoreSettingsRow
        row={askFirstRow(askFirst)}
        description="Home asks what you want done, then does it on a fresh computer you can watch. On this device only."
        onChoose={(_, option) => setFlagOverride("spaces_ask_first_home", option === "on")}
      />
      {askFirst ? (
        <CoreSettingsRow
          row={thisMacAskRow(!thisMacOk)}
          description="Before a task works directly on this Mac, with your files and apps. Turn it on to be asked again."
          onChoose={(_, option) => setThisMacConfirmed(option === "off")}
        />
      ) : null}
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
  return (
    <SettingsGroup title="Connection">
      {form ? (
        <HostSetupForm view={form} host={host} failure={host.setupError} progress={machines?.find((m) => m.current)?.hostProgress} onRetry={() => void host.retrySetUp()} />
      ) : (
        <div className="flex min-h-12 items-center justify-between gap-6 px-4 py-2.5" data-self-hosting="">
          <div className="min-w-0">
            <div className="text-[13px]">Use your own connection server</div>
            <p className="mt-0.5 text-xs text-muted-foreground">For teams that run their own.</p>
          </div>
          <Button size="sm" variant="outline" disabled={host.busy} onClick={() => host.openForm(SELF_HOSTED)}>
            Set up…
          </Button>
        </div>
      )}
    </SettingsGroup>
  );
}
