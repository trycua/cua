// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { HostLimitRow, HostLimits } from "@/bridge/contracts/host";

import {
  chooseSettings,
  useBridge,
  useLoginItem,
  useMachines,
  useSession,
  useSettings,
  useThisMachine,
  useTips,
  type GlobalShortcut,
  type SettingsRow as CoreRow,
} from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { CliSettingsSection } from "@/components/settings/cli-section";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { DefaultSpaceGroup } from "@/components/settings/default-space-group";
import { LanguageRow } from "@/components/settings/language-row";
import { RepoCacheGroup } from "@/components/settings/repo-cache-group";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { useT } from "@/i18n";
import { globalShortcutLine, showsMacShellSettings, thisComputerLabel, thisComputerText } from "@/lib/host-labels";
import { launchHelp, launchLabel } from "@/lib/settings-copy";
import { acceleratorOf } from "@/lib/shortcut-keys";
import { useTheme, type ThemePreference } from "@/lib/theme";

const fail = toastError;

type Say = ReturnType<typeof useT>;

const appearance = (say: Say): { value: ThemePreference; label: React.ReactNode }[] => [
  {
    value: "system",
    label: (
      <>
        <MonitorIcon className="size-3.5" /> {say("settings.general.appearance_system")}
      </>
    ),
  },
  {
    value: "light",
    label: (
      <>
        <SunIcon className="size-3.5" /> {say("settings.general.appearance_light")}
      </>
    ),
  },
  {
    value: "dark",
    label: (
      <>
        <MoonIcon className="size-3.5" /> {say("settings.general.appearance_dark")}
      </>
    ),
  },
];

/** The core's notch row in plain words: "Show Cua in", Notch and menu bar or Menu bar only. */
export const showCuaIn = (row: CoreRow, say: Say): CoreRow => ({
  ...row,
  label: say("settings.general.show_in"),
  options: row.options.map((o) => ({ ...o, label: o.id === "hide" ? say("settings.general.show_in_menu_bar") : say("settings.general.show_in_notch") })),
});

/**
 * Settings, General: the few things a person sets every day. Start at login,
 * where Cua lives (Mac), appearance, the shortcut (Mac) and usage data.
 * Everything else moved: Advanced, Connectors, the Volume
 * and Keyvault pages.
 */
export function GeneralTab() {
  const t = useT();
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
  // "When I close the window" and its notice: the Mac app's background service (the host lays the rows out).
  const closeRows = (general?.rows ?? []).filter((r) => r.id === "close-window" || r.id === "close-notice");
  const tipRows = (general?.rows ?? []).filter((r) => r.id === "tips" || r.id === "reset-tips");
  const chooseTips = (row: string, option: string) => tips.save(chooseSettings(core, { tips: tips.state }, row, option).tips);

  return (
    <div data-settings-tab="general">
      <SettingsGroup
        title={t("settings.general.everyday")}
        footer={
          dataLink?.linkLabel && dataLink.linkUrl ? (
            <button
              type="button"
              data-telemetry-link=""
              className="text-brand-strong outline-none hover:underline focus-visible:underline"
              onClick={() => void openExternal(dataLink.linkUrl!).catch(fail(t("settings.general.open_failed")))}
            >
              {dataLink.linkLabel}
            </button>
          ) : null
        }
      >
        <LaunchAtLogin />
        {mac && notch ? (
          <CoreSettingsRow
            row={showCuaIn(notch, t)}
            onChoose={(_, option) => void updateSetting("menuBar", option === "hide").catch(fail(t("settings.general.show_in_failed")))}
          />
        ) : mac && values ? (
          <SettingsRow
            label={t("settings.general.show_in")}
            control={
              <Segmented
                aria-label={t("settings.general.show_in")}
                value={values.menuBar ? "hide" : "show"}
                options={[
                  { value: "show", label: t("settings.general.show_in_notch") },
                  { value: "hide", label: t("settings.general.show_in_menu_bar") },
                ]}
                onValueChange={(v) => void updateSetting("menuBar", v === "hide").catch(fail(t("settings.general.show_in_failed")))}
              />
            }
          />
        ) : null}
        {closeRows.map((row) => (
          <CoreSettingsRow
            key={row.id}
            row={row}
            asSelect
            onChoose={(_, option) => void chooseSetting(row.id, option).catch(fail(t("settings.general.close_failed")))}
          />
        ))}
        {tipRows.map((row) => (
          <CoreSettingsRow
            key={row.id}
            row={row}
            onChoose={(_, option) => void chooseTips(row.id, option).catch(fail(t("settings.general.tips_failed")))}
            onPress={() => void chooseTips(row.id, "press").catch(fail(t("settings.general.tips_reset_failed")))}
          />
        ))}
        <SettingsRow
          label={t("settings.general.appearance")}
          control={<Segmented aria-label={t("settings.general.appearance")} value={preference} options={appearance(t)} onValueChange={setPreference} />}
        />
        <LanguageRow />
        <ShortcutRow shortcut={settings?.globalShortcut} onChange={(accelerator) => chooseSetting("global-shortcut", accelerator)} />
        {telemetryRow ? (
          <CoreSettingsRow
            row={{ ...telemetryRow, kind: "toggle" }}
            description={t("settings.general.telemetry_help")}
            onChoose={(_, option) => void updateSetting("telemetry", option === "on").catch(fail(t("settings.general.telemetry_failed")))}
          />
        ) : values ? (
          <SettingsRow
            label={t("settings.general.telemetry")}
            description={t("settings.general.telemetry_help")}
            htmlFor="telemetry"
            control={
              <Switch
                id="telemetry"
                checked={values.telemetry}
                onCheckedChange={(on) => void updateSetting("telemetry", on).catch(fail(t("settings.general.telemetry_failed")))}
              />
            }
          />
        ) : null}
      </SettingsGroup>
      <CliSettingsSection />
      <SharingGroup />
      <RepoCacheGroup />
      {/* Last: it shows once your computers have loaded, so nothing above it moves when it does. */}
      <DefaultSpaceGroup />
    </div>
  );
}

/** What the switch says before this Mac is set up (the core's words once it is): `settings.general.sharing_switch`. */
/**
 * Sharing, for this Mac: "Let other machines create Spaces here", on by default. Before this Mac is set
 * up, turning it on sets it up with the defaults (its desktop stays private; that switch is on its page
 * under Machines). With both switches off this Mac simply is not shared, with no error.
 */
function SharingGroup() {
  const t = useT();
  const { data: machines } = useMachines();
  const { data: session } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  const panel = machines?.find((m) => m.current && !m.device)?.panel;
  if (!panel) return null;
  const toggle = panel.toggles?.find((x) => x.id === "spaces");
  const resume = panel.actions.some((a) => a.id === "resume-sharing" && a.enabled);
  const on = panel.configured && Boolean(toggle?.on);
  const flip = (next: boolean) => {
    void (async () => {
      try {
        if (!panel.configured) {
          if (next && !(await host.quickSetUp()))
            throw new Error(host.setupError?.message ?? t("settings.general.set_up_failed", { computer: thisComputerText() }));
          return;
        }
        if (!toggle) return;
        await host.run(toggle.action);
        if (next && resume) await host.run("resume-sharing");
      } catch (e) {
        fail(t("settings.general.sharing_failed"))(e);
      }
    })();
  };
  return (
    <SettingsGroup title={t("settings.general.sharing")}>
      <SettingsRow
        label={toggle?.label ?? t("settings.general.sharing_switch")}
        description={toggle?.help || undefined}
        htmlFor="share-spaces"
        control={
          <Switch
            id="share-spaces"
            data-setting-row="share-spaces"
            checked={on}
            disabled={host.busy || (panel.configured && !toggle?.enabled)}
            onCheckedChange={flip}
          />
        }
      />
      {on
        ? (panel.limitRows ?? []).map((row) => (
            <LimitRow
              key={row.id}
              row={row}
              busy={host.busy}
              onChange={(limits) =>
                host.setLimits(limits).catch((e) => {
                  fail(t("settings.general.limit_failed"))(e);
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
  const t = useT();
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
  const total = row.total && row.total > 0 ? t("settings.general.limit_total", { computer: thisComputerLabel(), total: String(row.total) }) : null;
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
              {row.over ? <span className="ml-1.5 font-medium text-destructive">{row.overText ?? t("settings.general.limit_over")}</span> : null}
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
            placeholder={t("settings.general.limit_none")}
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
  const t = useT();
  const { data, setLaunchAtLogin, openLoginItems } = useLoginItem();
  if (!data?.rows) return null;
  return data.rows
    .filter((row) => row.id !== "launch-at-login-note")
    .map((row) =>
      row.id === "launch-at-login" ? (
        <CoreSettingsRow
          key={row.id}
          row={{ ...row, label: launchLabel() }}
          description={launchHelp()}
          onChoose={(_, option) => void setLaunchAtLogin(option === "on")}
        />
      ) : (
        <CoreSettingsRow key={row.id} row={row} onPress={() => void openLoginItems().catch(fail(t("settings.general.login_items_failed")))} />
      ),
    );
}

/**
 * The key that brings Cua Spaces forward from any app (the Mac hosts register
 * one). Change waits for the next keys pressed (Esc cancels); the host says
 * whether the system gave them, and a taken shortcut shows as the old line.
 */
function ShortcutRow({ shortcut, onChange }: { shortcut: GlobalShortcut | null | undefined; onChange: (accelerator: string) => Promise<void> }) {
  const t = useT();
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
      void onChange(accelerator).catch(fail(t("settings.general.shortcut_failed")));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, onChange, t]);
  if (!shortcut) return null;
  return (
    <div data-global-shortcut={shortcut.registered ? "registered" : "taken"}>
      <SettingsRow
        label={t("settings.general.shortcut")}
        description={recording ? t("settings.general.shortcut_recording") : (globalShortcutLine(shortcut) ?? undefined)}
        control={
          <div className="flex items-center gap-2">
            <span className="text-[13px] text-muted-foreground" data-shortcut-label="">
              {recording ? "…" : shortcut.label}
            </span>
            <Button variant="outline" size="sm" data-shortcut-change="" onClick={() => setRecording((r) => !r)}>
              {recording ? t("settings.general.shortcut_cancel") : t("settings.general.shortcut_change")}
            </Button>
          </div>
        }
      />
    </div>
  );
}
