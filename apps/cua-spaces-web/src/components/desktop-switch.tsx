// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";

import { useMachines, useSession, useThisMachine, type MachinesViewToggle } from "@/bridge";
import { SettingRow } from "@/components/ui/grouped";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { showDesktop } from "@/lib/hidden-desktops";

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * This Mac's "Let other machines control this desktop" switch, as the machine page draws it
 * (the core's `machines.view` toggle), for the page of its desktop row. Turning it on also
 * brings a hidden desktop row back. Before this Mac is set up, it opens the setup on Machines.
 */
export function useDesktopSwitch(): { toggle: MachinesViewToggle | undefined; busy: boolean; flip: () => void } {
  const { view } = useMachines();
  const { data: session } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const toggle = view?.machines.find((m) => m.current)?.toggles.find((t) => t.id === "desktop");
  const flip = () => {
    if (!toggle) return;
    if (toggle.setup) {
      host.openForm(toggle.setup);
      void navigate({ to: "/machines" });
      return;
    }
    const turningOn = !toggle.on;
    setBusy(true);
    void (async () => {
      try {
        for (const a of toggle.actions) await host.run(a);
        if (turningOn) showDesktop("this-mac");
      } catch (e) {
        toast(`Couldn't change ${toggle.label}`, { description: errorText(e), type: "error" });
      } finally {
        setBusy(false);
      }
    })();
  };
  return { toggle, busy: busy || host.busy, flip };
}

/** The switch with its label, as a row of a grouped section. */
export function DesktopSwitchRow() {
  const { toggle, busy, flip } = useDesktopSwitch();
  if (!toggle) return null;
  return (
    <SettingRow
      data-desktop-switch=""
      label={toggle.label}
      labelId="desktop-switch-label"
      control={<Switch checked={toggle.on} disabled={!toggle.editable || !toggle.enabled || busy} aria-labelledby="desktop-switch-label" onCheckedChange={flip} />}
    />
  );
}
