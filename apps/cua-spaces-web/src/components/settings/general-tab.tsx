// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { useLoginItem, useSession, useSettings, type GlobalShortcut, type SettingsRow as CoreRow } from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { globalShortcutLine, showsMacShellSettings } from "@/lib/host-labels";
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
 * Everything else moved: Advanced, Connected apps, Trusted devices, the Volume
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

  return (
    <div data-settings-tab="general">
      <SettingsGroup title="Everyday">
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
      {dataLink?.linkLabel && dataLink.linkUrl ? (
        <button
          type="button"
          data-telemetry-link=""
          className="-mt-4 px-1 text-[13px] text-brand-strong outline-none hover:underline focus-visible:underline"
          onClick={() => void openExternal(dataLink.linkUrl!).catch(fail("Couldn't open the page"))}
        >
          {dataLink.linkLabel}
        </button>
      ) : null}
    </div>
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
