// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Link } from "@tanstack/react-router";
import { LoaderCircleIcon } from "lucide-react";

import { AUTOMATION_PANE, FULL_DISK_ACCESS_PANE, type KvReadiness, type TeleportSetup, type TeleportStore } from "@/bridge";
import { Button } from "@/components/ui/button";
import { GroupedSection, SettingRow } from "@/components/ui/grouped";

/**
 * Teleport setup: one sheet, shown before a browser's sign-ins are read, that
 * says what a teleport needs and what is missing, in plain words, in place
 * of the raw refusals a first teleport used to end in. Keyvault, Full Disk
 * Access, Automation, the browser profile. Nothing is read to draw it and no
 * Touch ID is asked; Cua cannot turn any of these on (macOS only lets you),
 * so each row says what to do and a button takes you there.
 */
export function TeleportSetupSheet({ setup, teleport }: { setup: TeleportSetup; teleport: TeleportStore }) {
  const r = setup.readiness;
  if (!r) {
    return (
      <div className="flex h-full items-center justify-center gap-2 text-[13px] text-muted-foreground" data-teleport-setup="checking">
        <LoaderCircleIcon className="size-4 animate-spin" /> Checking
      </div>
    );
  }
  const name = r.app_display;
  const denied = r.files.state === "denied";
  return (
    <div className="space-y-4 px-5 pb-4" data-teleport-setup={setup.blocked ? "blocked" : "ready"} aria-busy={setup.checking || undefined}>
      <p className="text-[13px] text-muted-foreground" data-setup-intro>
        {setup.blocked
          ? `${name} cannot be teleported yet. Here is what is missing.`
          : `Choose the ${name} profile to teleport. Everything it needs is ready.`}
      </p>
      <GroupedSection>
        <SettingRow
          data-setup-row="keyvault"
          label="Keyvault"
          help={r.keyvault_initialized ? null : "Teleport keeps what it moves in your Keyvault. Create it first."}
          value={state(r.keyvault_initialized ? "Ready" : "Not set up")}
          control={
            r.keyvault_initialized ? null : (
              <Button size="sm" variant="secondary" render={<Link to="/keyvault" onClick={() => teleport.close()} />}>
                Set up
              </Button>
            )
          }
        />
        <SettingRow
          data-setup-row="files"
          label="Full Disk Access"
          help={
            denied ? (
              <span data-setup-say="files">{r.files.say}</span>
            ) : r.profile_empty ? null : (
              `So Cua can read ${name}'s sign-ins when you approve.`
            )
          }
          value={state(denied ? "Needed" : "Allowed")}
          control={
            denied ? (
              <Button size="sm" variant="secondary" onClick={() => void teleport.openPane(FULL_DISK_ACCESS_PANE)}>
                Open Settings
              </Button>
            ) : null
          }
        />
        {denied ? (
          <SettingRow
            data-setup-row="tabs-only"
            label="Without it"
            help="Open tabs can still be teleported, signed out."
            control={
              <Button size="sm" variant="secondary" onClick={() => teleport.useTabsOnly()}>
                Send open tabs only
              </Button>
            }
          />
        ) : null}
        <SettingRow
          data-setup-row="tabs"
          label="Automation"
          help={r.tabs.state === "denied" ? <span data-setup-say="tabs">{r.tabs.say}</span> : "macOS asks the first time open tabs are read."}
          value={state(r.tabs.state === "denied" ? "Needed" : r.tabs.state === "granted" ? "Allowed" : "Asked when needed")}
          control={
            r.tabs.state === "denied" ? (
              <Button size="sm" variant="secondary" onClick={() => void teleport.openPane(AUTOMATION_PANE)}>
                Open Settings
              </Button>
            ) : null
          }
        />
      </GroupedSection>
      <Profiles readiness={r} chosen={setup.profile} onChoose={(dir) => void teleport.chooseProfile(dir)} />
      {setup.error ? (
        <p className="text-xs text-muted-foreground" data-setup-error>
          {setup.error}
        </p>
      ) : null}
    </div>
  );
}

const state = (word: string) => <span data-setup-state={word}>{word}</span>;

/** The browser profile to read: name and account, the last used one marked. */
function Profiles({ readiness: r, chosen, onChoose }: { readiness: KvReadiness; chosen: string | null; onChoose: (dir: string) => void }) {
  if (!r.profiles.length) return null;
  return (
    <GroupedSection title="Profile" data-setup-profiles footer={r.profile_empty ? <span data-setup-say="profile">{r.files.say}</span> : null}>
      <RadioGroup value={chosen ?? ""} onValueChange={(v) => onChoose(String(v))} aria-label="Profile">
        {r.profiles.map((p) => (
          <label key={p.dir} className="flex min-h-11 cursor-default items-center gap-3 px-4 py-2 text-[13px]" data-setup-profile={p.dir} data-last-used={p.last_used || undefined}>
            <Radio.Root
              value={p.dir}
              className="flex size-4 shrink-0 items-center justify-center rounded-full border border-input bg-card shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/60 data-checked:border-brand data-checked:bg-brand dark:bg-input/30" // design-ok: radio control ring, not a content box
            >
              <Radio.Indicator className="size-1.5 rounded-full bg-white" />
            </Radio.Root>
            <span className="min-w-0 flex-1">
              <span className="block truncate">{p.name}</span>
              {p.account ? <span className="block truncate text-xs text-muted-foreground">{p.account}</span> : null}
            </span>
            {p.last_used ? <span className="shrink-0 text-xs text-muted-foreground">Used last</span> : null}
          </label>
        ))}
      </RadioGroup>
    </GroupedSection>
  );
}
