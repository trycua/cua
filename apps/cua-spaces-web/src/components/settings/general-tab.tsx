// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { HostLimitRow, HostLimits } from "@/bridge/contracts/host";

import { chooseSettings, useBridge, useLoginItem, useMachines, useSession, useSettings, useThisMachine, useTips, type GlobalShortcut, type SettingsRow as CoreRow } from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { globalShortcutLine, showsMacShellSettings, thisComputerLabel } from "@/lib/host-labels";
import { LAUNCH_HELP, LAUNCH_LABEL } from "@/lib/settings-copy";
import { acceleratorOf } from "@/lib/shortcut-keys";
import { useTheme, type ThemePreference } from "@/lib/theme";

const fail = toastError;

const APPEARANCE: { value: ThemePreference; label: React.ReactNode }[] = [
  { value: "system", label: <><MonitorIcon className="size-3.5" /> System</> },
  { value: "light", label: <><SunIcon className="size-3.5" /> Light</> },
  { value: "dark", label: <><MoonIcon className="size-3.5" /> Dark</> },
];

const TELEMETRY_HELP = "Which features you use and when something goes wrong. Never your files, what you type or your screen.";

/** The core's notch row in plain words: "Show Cua in", Notch and menu bar or Menu bar only. */
export const showCuaIn = (row: CoreRow): CoreRow => ({
  ...row,
  label: "Show Cua in",
  options: row.options.map((o) => ({ ...o, label: o.id === "hide" ? "Menu bar only" : "Notch and menu bar" })),
});

/**
 * Settings, General: the few things a person sets every day. Start at login,
 * where Cua lives (Mac), appearance, the shortcut (Mac) and usage data.
 * Everything else moved: Advanced, Connectors, Trusted machines, the Volume
 * and Keyvault pages.
 */
export function GeneralTab() {
  const { data: settings, updateSetting, chooseSetting } = useSettings();
  const { preference, setPreference } = useTheme();
  const mac = showsMacShellSettings();
  const page = settings?.page;
  const general = page?.sections.find((s) => s.id === "general");
  const telemetry = page?.sections.find((s) => s.id === "privacy");
  const notch = general?.rows.find((r) => r.id === "notch");
  const telemetryRow = telemetry?.rows.find((r) => r.id === "telemetry");
  const dataLink = telemetry?.rows.find((r) => r.id === "telemetry-note");
  const values = settings?.values;
  const { openExternal } = useSession();
  const { core } = useBridge();
  const tips = useTips();
  // Show tips and Reset tips: what the core makes of the row (`settings.choose`), saved with the settings.
  const tipRows = (general?.rows ?? []).filter((r) => r.id === "tips" || r.id === "reset-tips");
  const chooseTips = (row: string, option: string) => tips.save(chooseSettings(core, { tips: tips.state }, row, option).tips);

  return (
    <div data-settings-tab="general">
      <SettingsGroup
        title="Everyday"
        footer={
          dataLink?.linkLabel && dataLink.linkUrl ? (
            <button
              type="button"
              data-telemetry-link=""
              className="text-brand-strong outline-none hover:underline focus-visible:underline"
              onClick={() => void openExternal(dataLink.linkUrl!).catch(fail("Couldn't open the page"))}
            >
              {dataLink.linkLabel}
            </button>
          ) : null
        }
      >
        <LaunchAtLogin />
        {mac && notch ? (
          <CoreSettingsRow
            row={showCuaIn(notch)}
            description="Pick where to find Cua Spaces. Mac only."
            onChoose={(_, option) => void updateSetting("menuBar", option === "hide").catch(fail("Couldn't change where Cua shows"))}
          />
        ) : mac && values ? (
          <SettingsRow
            label="Show Cua in"
            description="Pick where to find Cua Spaces. Mac only."
            control={
              <Segmented
                aria-label="Show Cua in"
                value={values.menuBar ? "hide" : "show"}
                options={[
                  { value: "show", label: "Notch and menu bar" },
                  { value: "hide", label: "Menu bar only" },
                ]}
                onValueChange={(v) => void updateSetting("menuBar", v === "hide").catch(fail("Couldn't change where Cua shows"))}
              />
            }
          />
        ) : null}
        {tipRows.map((row) => (
          <CoreSettingsRow
            key={row.id}
            row={row}
            onChoose={(_, option) => void chooseTips(row.id, option).catch(fail("Couldn't change tips"))}
            onPress={() => void chooseTips(row.id, "press").catch(fail("Couldn't reset tips"))}
          />
        ))}
        <SettingsRow label="Appearance" control={<Segmented aria-label="Appearance" value={preference} options={APPEARANCE} onValueChange={setPreference} />} />
        <ShortcutRow shortcut={settings?.globalShortcut} onChange={(accelerator) => chooseSetting("global-shortcut", accelerator)} />
        {telemetryRow ? (
          <CoreSettingsRow
            row={{ ...telemetryRow, kind: "toggle" }}
            description={TELEMETRY_HELP}
            onChoose={(_, option) => void updateSetting("telemetry", option === "on").catch(fail("Couldn't change usage data"))}
          />
        ) : values ? (
          <SettingsRow
            label="Share usage data to help improve Cua"
            description={TELEMETRY_HELP}
            htmlFor="telemetry"
            control={<Switch id="telemetry" checked={values.telemetry} onCheckedChange={(on) => void updateSetting("telemetry", on).catch(fail("Couldn't change usage data"))} />}
          />
        ) : null}
      </SettingsGroup>
      <SharingGroup />
    </div>
  );
}

/** What the switch says before this Mac is set up (the core's words once it is). */
const SPACES_SWITCH = "Let other machines create Spaces here";
const SPACES_HELP = "Your other machines can create Spaces here.";

/**
 * Sharing, for this Mac: "Let other machines create Spaces here", on by default. Before this Mac is set
 * up, turning it on sets it up with the defaults (its desktop stays private; that switch is on its page
 * under Machines). With both switches off this Mac simply is not shared, with no error.
 */
function SharingGroup() {
  const { data: machines } = useMachines();
  const { data: session } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  const panel = machines?.find((m) => m.current && !m.device)?.panel;
  if (!panel) return null;
  const toggle = panel.toggles?.find((t) => t.id === "spaces");
  const resume = panel.actions.some((a) => a.id === "resume-sharing" && a.enabled);
  const on = panel.configured && Boolean(toggle?.on);
  const flip = (next: boolean) => {
    void (async () => {
      try {
        if (!panel.configured) {
          if (next && !(await host.quickSetUp())) throw new Error(host.setupError?.message ?? "Couldn't set this Mac up.");
          return;
        }
        if (!toggle) return;
        await host.run(toggle.action);
        if (next && resume) await host.run("resume-sharing");
      } catch (e) {
        fail("Couldn't change sharing")(e);
      }
    })();
  };
  return (
    <SettingsGroup title="Sharing">
      <SettingsRow
        label={toggle?.label ?? SPACES_SWITCH}
        description={toggle?.help ?? SPACES_HELP}
        htmlFor="share-spaces"
        control={<Switch id="share-spaces" data-setting-row="share-spaces" checked={on} disabled={host.busy || (panel.configured && !toggle?.enabled)} onCheckedChange={flip} />}
      />
      {on
        ? (panel.limitRows ?? []).map((row) => (
            <LimitRow
              key={row.id}
              row={row}
              busy={host.busy}
              onChange={(limits) =>
                host.setLimits(limits).catch((e) => {
                  fail("Couldn't change the limit")(e);
                  throw e;
                })
              }
            />
          ))
        : null}
    </SettingsGroup>
  );
}

const LIMIT_FIELD: Record<string, keyof HostLimits> = {
  "max-spaces": "maxSpaces",
  "max-memory": "maxMemoryGb",
  "max-disk": "maxDiskGb",
  "max-run": "maxRunMinutes",
};

/**
 * One limit on the Spaces other machines create here: a number (empty is no limit), what is in use
 * next to it, and "Over limit" when it was lowered below that (nothing is stopped; new Spaces are refused).
 * Saved when the field is left or Enter is pressed.
 */
function LimitRow({ row, busy, onChange }: { row: HostLimitRow; busy: boolean; onChange: (limits: HostLimits) => Promise<unknown> }) {
  const shown = row.value > 0 ? String(row.value) : "";
  const [draft, setDraft] = useState(shown);
  const latest = useRef(shown);
  latest.current = shown;
  useEffect(() => setDraft(shown), [shown]);
  const field = LIMIT_FIELD[row.id];
  if (!field) return null;
  const save = () => {
    const n = draft.trim() === "" ? 0 : Math.floor(Number(draft));
    if (!Number.isFinite(n) || n < 0) return setDraft(shown);
    if (n === row.value) return setDraft(shown);
    // A change the host refused shows the number it still has.
    onChange({ [field]: n }).catch(() => setDraft(latest.current));
  };
  const total = row.total && row.total > 0 ? `${thisComputerLabel()} has ${row.total} GB` : null;
  return (
    <SettingsRow
      label={row.label}
      htmlFor={`limit-${row.id}`}
      description={
        <>
          {row.help}
          {row.useText ? (
            <span className="block" data-limit-use={row.id}>
              {row.useText}
              {row.over ? <span className="ml-1.5 font-medium text-destructive">{row.overText ?? "Over limit"}</span> : null}
              {total ? `. ${total}` : null}
            </span>
          ) : null}
        </>
      }
      control={
        <span className="flex items-center gap-2">
          <Input
            id={`limit-${row.id}`}
            data-setting-row={`limit-${row.id}`}
            type="number"
            inputMode="numeric"
            min={0}
            className="w-28 text-right"
            placeholder="No limit"
            value={draft}
            disabled={busy}
            onValueChange={setDraft}
            onBlur={save}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            }}
          />
          <span className="w-12 text-[13px] text-muted-foreground">{row.unit === "Spaces" ? "Spaces" : row.unit}</span>
        </span>
      }
    />
  );
}

/** Launch at login as the app core lays it out; the switch shows what the system reports, never what was asked. */
function LaunchAtLogin() {
  const { data, setLaunchAtLogin, openLoginItems } = useLoginItem();
  if (!data?.rows) return null;
  return data.rows
    .filter((row) => row.id !== "launch-at-login-note")
    .map((row) =>
      row.id === "launch-at-login" ? (
        <CoreSettingsRow
          key={row.id}
          row={{ ...row, label: LAUNCH_LABEL }}
          description={LAUNCH_HELP}
          onChoose={(_, option) => void setLaunchAtLogin(option === "on")}
        />
      ) : (
        <CoreSettingsRow key={row.id} row={row} onPress={() => void openLoginItems().catch(fail("Couldn't open Login Items"))} />
      ),
    );
}

/**
 * The key that brings Cua Spaces forward from any app (the Mac hosts register
 * one). Change waits for the next keys pressed (Esc cancels); the host says
 * whether the system gave them, and a taken shortcut shows as the old line.
 */
function ShortcutRow({ shortcut, onChange }: { shortcut: GlobalShortcut | null | undefined; onChange: (accelerator: string) => Promise<void> }) {
  const [recording, setRecording] = useState(false);
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") return setRecording(false);
      const accelerator = acceleratorOf(e);
      if (!accelerator) return;
      setRecording(false);
      void onChange(accelerator).catch(fail("Couldn't change the shortcut"));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, onChange]);
  if (!shortcut) return null;
  return (
    <div data-global-shortcut={shortcut.registered ? "registered" : "taken"}>
      <SettingsRow
        label="Open Cua Spaces from any app"
        description={recording ? "Press the keys you want. Esc cancels." : shortcut.registered ? "Press this to bring Cua Spaces to the front." : globalShortcutLine(shortcut)}
        control={
          <div className="flex items-center gap-2">
            <span className="text-[13px] text-muted-foreground" data-shortcut-label="">
              {recording ? "…" : shortcut.label}
            </span>
            <Button variant="outline" size="sm" data-shortcut-change="" onClick={() => setRecording((r) => !r)}>
              {recording ? "Cancel" : "Change"}
            </Button>
          </div>
        }
      />
    </div>
  );
}
