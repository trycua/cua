// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon } from "lucide-react";

import { useScreenPermissions } from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";

/**
 * Settings, Permissions: the first run's "Share this Mac's screen" rows
 * (Screen Recording, Accessibility), for whoever chose "Not now" there.
 * Only on a Mac that is set up and shares its screen. Allow opens the app's
 * guide to the System Settings pane; a check shows once it is granted.
 */
export function ScreenPermissionsSection() {
  const perms = useScreenPermissions();
  if (!perms) return null;
  const { view } = perms;
  return (
    <SettingsGroup title="Permissions">
      {view.rows.map((r) => (
        <SettingsRow
          key={r.id}
          label={r.title}
          description={r.note}
          control={
            r.state === "ok" ? (
              <span className="flex size-5 items-center justify-center rounded-full bg-[#1f9d55] text-white" aria-label="Allowed" data-settings-permission={r.id} data-state="ok">
                <CheckIcon className="size-3" strokeWidth={3} />
              </span>
            ) : r.action && r.settingsUrl ? (
              <Button
                size="sm"
                variant="outline"
                data-settings-permission={r.id}
                data-state={r.state}
                onClick={() => void perms.allow(r.settingsUrl!).catch(toastError("Couldn't open System Settings"))}
              >
                {r.action.label}
              </Button>
            ) : null
          }
        />
      ))}
    </SettingsGroup>
  );
}
