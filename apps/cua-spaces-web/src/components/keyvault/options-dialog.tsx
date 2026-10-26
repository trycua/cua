// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { useSettings, type SettingsRow } from "@/bridge";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toastError } from "@/components/ui/toast";
import { keyvaultRows } from "@/lib/keyvault";

/** The Keyvault's options in plain words (they used to be in Settings, General). */
const COPY: Record<string, { label: string; help?: string }> = {
  "keyvault-auto-wipe": { label: "Remove saved sign-ins from Spaces after an hour", help: "When off, sign-ins stay in a Space until you remove them." },
  "keyvault-unlock-prompt": { label: "Show the reminder before an agent uses a saved sign-in", help: "Unlocking a login always asks for Touch ID first." },
  "keyvault-site-icons": { label: "Show site logos", help: "Asks Google for each logo and sends only the site address." },
};

/**
 * Keyvault, Options: the switches that were in Settings, General. A Mac
 * setting: the dialog's button shows only where the core reports them.
 */
export function KeyvaultOptions() {
  const [open, setOpen] = useState(false);
  const { data: settings, chooseSetting } = useSettings();
  const section = settings?.page?.sections.find((s) => s.id === "keyvault");
  if (!section) return null;
  const all = keyvaultRows(section.rows, settings?.hostSettings);
  const rows = all.filter((r) => r.id in COPY);
  const facts = all.filter((r) => r.id.startsWith("keyvault-protection:"));
  if (rows.length === 0) return null;
  const choose = (row: SettingsRow, option: string) => void chooseSetting(row.id, option).catch(toastError("Couldn't change the setting"));
  return (
    <>
      <Button variant="outline" onClick={() => setOpen(true)} data-keyvault-options="">
        Options
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogPopup data-keyvault-options-dialog="">
          <div className="px-5 pt-5 pb-3">
            <DialogTitle className="text-[15px] font-semibold">Keyvault options</DialogTitle>
          </div>
          <div className="divide-y border-y">
            {rows.map((r) => (
              <CoreSettingsRow key={r.id} row={{ ...r, label: COPY[r.id]!.label }} description={COPY[r.id]!.help} onChoose={choose} />
            ))}
            {facts.map((r) => (
              <CoreSettingsRow key={r.id} row={r} />
            ))}
          </div>
          <div className="flex justify-end px-5 py-3">
            <Button variant="outline" onClick={() => setOpen(false)}>
              Done
            </Button>
          </div>
        </DialogPopup>
      </Dialog>
    </>
  );
}
