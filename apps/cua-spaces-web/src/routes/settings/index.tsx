// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { MonitorIcon, MoonIcon, SunIcon } from "lucide-react";

import {
  useBridge,
  useExperiments,
  useLoginItem,
  useMachines,
  useSession,
  useSettings,
  useStorageSettings,
  type SettingsPage as CorePage,
  type SettingsRow as CoreRow,
  type SettingsSection as CoreSection,
} from "@/bridge";
import { writeSavedOnboarding } from "@/bridge/onboarding";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { CoreSettingsRow, CoreSettingsSection } from "@/components/settings/core-section";
import { Button } from "@/components/ui/button";
import { Shortcut } from "@/components/ui/kbd";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { lazy, Suspense } from "react";
import { toastError } from "@/components/ui/toast";
import { showsMacShellSettings, thisComputerLabel } from "@/lib/host-labels";
import { DEFAULT_KEYBINDINGS, type KeybindingCommand } from "@/lib/keybindings";
import { useTheme, type ThemePreference } from "@/lib/theme";

export const Route = createFileRoute("/settings/")({ component: SettingsPage });

// Its miniature and setup load with the card.
const DriverSettingsCard = lazy(() => import("@/components/volume/driver-card").then((m) => ({ default: m.DriverSettingsCard })));

const COMMAND_LABEL: Record<KeybindingCommand, string> = {
  "commandPalette.toggle": "Open command palette",
  "sidebar.toggle": "Show or hide sidebar",
  "nav.spaces": "Go to Spaces",
  "nav.machines": "Go to Machines",
  "nav.agents": "Go to Agents",
  "nav.keyvault": "Go to Keyvault",
  "nav.settings": "Open Settings",
  "theme.cycle": "Cycle appearance",
};

const APPEARANCE: { value: ThemePreference; label: React.ReactNode }[] = [
  { value: "system", label: <><MonitorIcon className="size-3.5" /> System</> },
  { value: "light", label: <><SunIcon className="size-3.5" /> Light</> },
  { value: "dark", label: <><MoonIcon className="size-3.5" /> Dark</> },
];

const fail = toastError;

/** Settings, General: the core's page as the SwiftUI app draws it when the
 * core is loaded (Account with Teams, General, Runtimes, Privacy, the
 * Keyvault), with this UI's own rows (appearance, keyboard shortcuts);
 * without the core, the same settings drawn by hand. */
function SettingsPage() {
  const { data: settings } = useSettings();
  return settings?.page ? <CoreGeneralPage page={settings.page} /> : <FallbackSettingsPage />;
}

/** The rows the Keyvault section shows here (the rest needs the broker's state). */
const KEYVAULT_ROWS = new Set(["keyvault-auto-wipe", "keyvault-auto-wipe-note"]);

function CoreGeneralPage({ page }: { page: CorePage }) {
  const { data: settings, updateSetting, chooseSetting } = useSettings();
  const { signIn, signOut, cancelSignIn, openExternal } = useSession();
  const { mode } = useBridge();
  const navigate = useNavigate();
  const { preference, setPreference } = useTheme();
  const section = (id: string): CoreSection | undefined => page.sections.find((x) => x.id === id);
  const account = section("account");
  const general = section("general");
  const runtimes = section("runtimes");
  const privacy = section("privacy");
  const keyvault = section("keyvault");
  const autoWipe = keyvault && { ...keyvault, rows: keyvault.rows.filter((r) => KEYVAULT_ROWS.has(r.id)) };
  const mac = showsMacShellSettings();

  // What a row's choice changes: the settings every host has by key, the
  // host's own (runtimes, auto-connect, auto-wipe) by row.
  const choose = (row: CoreRow, option: string) => {
    const done = fail(`Couldn't change ${row.label}`);
    switch (row.id) {
      case "notch":
        return void updateSetting("menuBar", option === "hide").catch(done);
      case "telemetry":
        return void updateSetting("telemetry", option === "on").catch(done);
      case "default-location":
        return void updateSetting("defaultLocation", option).catch(done);
      default:
        return void chooseSetting(row.id, option).catch(done);
    }
  };
  // What a row's button does, as the SwiftUI app's `AppModel.press`.
  const press = (row: CoreRow) => {
    switch (row.id) {
      case "account":
        return void signOut().catch(fail("Couldn't sign out"));
      case "sign-in":
        return void signIn().catch(fail("Couldn't start sign-in"));
      case "sign-in-code":
        return cancelSignIn();
      case "teams":
      case "billing":
        return row.linkUrl ? void openExternal(row.linkUrl) : undefined;
      case "welcome":
        // The SwiftUI app shows its own welcome window; elsewhere it is
        // this UI's first run, from the start.
        if (mode === "webkit") return void chooseSetting("welcome", "show").catch(fail("Couldn't show the welcome"));
        writeSavedOnboarding(null);
        return void navigate({ to: "/onboarding" });
    }
  };
  const rows = (s: CoreSection, skip: (r: CoreRow) => boolean = () => false) =>
    s.rows.filter((r) => !skip(r)).map((r) => <CoreSettingsRow key={r.id} row={r} onChoose={choose} onPress={press} />);

  return (
    <>
      {account ? <SettingsGroup title={account.title}>{rows(account)}</SettingsGroup> : null}

      <SettingsGroup title={general?.title ?? "General"}>
        <LaunchAtLoginRows />
        {/* The notch is the macOS app's. */}
        {general ? rows(general, (r) => r.id === "notch" && !mac) : null}
        <SettingsRow label="Appearance" control={<Segmented aria-label="Appearance" value={preference} options={APPEARANCE} onValueChange={setPreference} />} />
        {mac ? <SettingsRow label="Global shortcut" control={<span className="text-[13px] text-muted-foreground">{settings?.values.hotkey ?? ""}</span>} /> : null}
      </SettingsGroup>

      <StorageSection />

      {runtimes?.rows.length ? <CoreSettingsSection section={runtimes} foldNotes onChoose={choose} /> : null}
      {privacy ? <CoreSettingsSection section={privacy} onChoose={choose} onPress={press} /> : null}
      {autoWipe?.rows.length ? <CoreSettingsSection section={autoWipe} foldNotes onChoose={choose} /> : null}

      <SettingsGroup title="AI agents">
        <Suspense fallback={<div className="h-48" />}>
          <DriverSettingsCard />
        </Suspense>
      </SettingsGroup>

      <SettingsGroup title="Keyboard shortcuts">
        {DEFAULT_KEYBINDINGS.map((b) => (
          <SettingsRow key={b.command} label={COMMAND_LABEL[b.command]} control={<Shortcut spec={b.key} />} />
        ))}
      </SettingsGroup>
    </>
  );
}

/** Without the core: the settings every host has, drawn by hand. */
function FallbackSettingsPage() {
  const { data: settings, updateSetting } = useSettings();
  const { data: machines = [] } = useMachines();
  const { data: session, signIn, signOut } = useSession();
  const { preference, setPreference } = useTheme();

  const values = settings?.values;
  const locationOptions = [
    { value: "local", label: thisComputerLabel() },
    { value: "cloud", label: "Cua Cloud" },
    ...machines.filter((m) => !m.current).map((m) => ({ value: `host:${m.id}`, label: m.name })),
  ];
  const location = values?.defaultLocation ?? "local";
  if (!locationOptions.some((o) => o.value === location)) locationOptions.push({ value: location, label: location });
  const locationLocked = settings?.defaultLocation?.source === "env";

  return (
    <>
      <SettingsGroup title="General">
        <LaunchAtLoginRows />
        <SettingsRow label="Appearance" control={<Segmented aria-label="Appearance" value={preference} options={APPEARANCE} onValueChange={setPreference} />} />
        {showsMacShellSettings() ? (
          <>
            <SettingsRow
              label="Show in menu bar"
              description="Open Cua Spaces from the menu bar instead of the notch."
              htmlFor="menuBar"
              control={
                <Switch
                  id="menuBar"
                  checked={values?.menuBar ?? false}
                  disabled={!values}
                  onCheckedChange={(on) => void updateSetting("menuBar", on).catch(fail("Couldn't change the menu bar setting"))}
                />
              }
            />
            <SettingsRow label="Global shortcut" control={<span className="text-[13px] text-muted-foreground">{values?.hotkey ?? ""}</span>} />
          </>
        ) : null}
      </SettingsGroup>

      <StorageSection />

      <SettingsGroup title="Spaces">
        <SettingsRow
          label="New Spaces start on"
          description={locationLocked ? `Set by ${settings?.defaultLocation?.env ?? "the environment"}.` : "You can pick another place when you create one."}
          control={
            <Select
              aria-label="New Spaces start on"
              value={location}
              options={locationOptions}
              onValueChange={(v) => void updateSetting("defaultLocation", v).catch(fail("Couldn't change the default location"))}
            />
          }
        />
      </SettingsGroup>

      <SettingsGroup title="AI agents">
        <Suspense fallback={<div className="h-48" />}>
          <DriverSettingsCard />
        </Suspense>
      </SettingsGroup>

      <SettingsGroup title="Account">
        <SettingsRow
          label={session?.signedIn ? (session.identity ?? "Signed in") : "Not signed in"}
          description={session?.signIn.kind === "waiting" ? "Finish signing in from your browser." : session?.signIn.kind === "failed" ? session.signIn.message : undefined}
          control={
            session?.signedIn ? (
              <Button variant="outline" size="sm" onClick={() => void signOut().catch(fail("Couldn't sign out"))}>
                Sign out
              </Button>
            ) : (
              <Button variant="outline" size="sm" disabled={!session} onClick={() => void signIn().catch(fail("Couldn't start sign-in"))}>
                Sign in
              </Button>
            )
          }
        />
      </SettingsGroup>

      <SettingsGroup title="Privacy">
        <SettingsRow
          label="Share anonymous usage data"
          description="Features used, sandbox types, durations and error categories. Never paths, names, prompts, screen content or Keyvault items."
          htmlFor="telemetry"
          control={
            <Switch
              id="telemetry"
              checked={values?.telemetry ?? false}
              disabled={!values}
              onCheckedChange={(on) => void updateSetting("telemetry", on).catch(fail("Couldn't change telemetry"))}
            />
          }
        />
      </SettingsGroup>

      <SettingsGroup title="Keyboard shortcuts">
        {DEFAULT_KEYBINDINGS.map((b) => (
          <SettingsRow key={b.command} label={COMMAND_LABEL[b.command]} control={<Shortcut spec={b.key} />} />
        ))}
      </SettingsGroup>
    </>
  );
}

/**
 * Launch at login as the app core lays it out (`login_item::rows_with`): the
 * switch shows what the system reports, never what was asked, and the line
 * under it says what turning it off stops, or where to approve it.
 */
function LaunchAtLoginRows() {
  const { data, setLaunchAtLogin, openLoginItems } = useLoginItem();
  if (!data?.rows) return null;
  return data.rows.map((row) => (
    <CoreSettingsRow
      key={row.id}
      row={row}
      onChoose={(_, option) => void setLaunchAtLogin(option === "on")}
      onPress={() => void openLoginItems().catch(fail("Couldn't open Login Items"))}
    />
  ));
}

/** Settings, Storage: after General, only with the Cua Volume experiment on (`settings.withStorage`). */
function StorageSection() {
  const { data: experiments } = useExperiments();
  if (!experiments?.experiments.cuaVolume) return null;
  return <StorageSectionBody />;
}

function StorageSectionBody() {
  const { data, press, choose, edit, save } = useStorageSettings();
  if (!data?.section) return null;
  return (
    <CoreSettingsSection
      section={data.section}
      onHeaderButton={save}
      onPress={(r) => press(r.id)}
      onChoose={(r, option) => choose(r.id, option)}
      onEdit={(r, value) => edit(r.id, value)}
    />
  );
}
