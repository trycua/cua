// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { agentKeyForm, type AgentKeyProvider, type AgentKeysHook } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

/** Add or replace a key. The key stays in this field until Save, then the sheet closes and it is gone. */
export function KeySheet({ keys, provider, env }: { keys: AgentKeysHook; provider: AgentKeyProvider; env: string | null }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const v = agentKeyForm(keys.core, keys.input, { provider, env, name, hasValue: value.trim().length > 0 });

  const save = async () => {
    if (!v?.canSave || busy) return;
    setBusy(true);
    setError(null);
    try {
      await keys.save(v.provider, value, v.env);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  if (!v) return null;
  return (
    <Dialog open onOpenChange={(o) => !o && keys.closeSheet()}>
      <DialogPopup className="w-[min(440px,calc(100vw-2rem))]" data-agent-key-sheet>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <div className="px-5 pt-5 pb-4">
            <DialogTitle className="text-[15px] font-semibold" data-agent-key-sheet-title>
              {v.title}
            </DialogTitle>
            <p className="mt-1.5 text-[13px] text-muted-foreground" data-agent-key-sheet-lede>
              {v.lede}
            </p>
            {v.nameLabel ? (
              <label className="mt-4 block">
                <span className="mb-1 block text-xs font-medium">{v.nameLabel}</span>
                <Input
                  data-agent-key-name
                  value={name}
                  placeholder={v.namePlaceholder ?? ""}
                  autoComplete="off"
                  spellCheck={false}
                  aria-invalid={v.nameError ? true : undefined}
                  className="font-mono"
                  onChange={(e) => setName(e.currentTarget.value)}
                />
                {v.nameError ? (
                  <span className="mt-1 block text-xs text-destructive" data-agent-key-name-error>
                    {v.nameError}
                  </span>
                ) : null}
              </label>
            ) : null}
            <label className="mt-4 block">
              <span className="mb-1 block text-xs font-medium">{v.valueLabel}</span>
              <Input
                data-agent-key-value
                type="password"
                value={value}
                placeholder={v.valuePlaceholder}
                autoComplete="off"
                spellCheck={false}
                autoFocus={!v.nameLabel}
                onChange={(e) => setValue(e.currentTarget.value)}
              />
              <span className="mt-1 block text-xs text-muted-foreground" data-agent-key-value-help>
                {v.valueHelp}
              </span>
            </label>
            {error ? (
              <p className="mt-3 text-[13px] text-destructive" data-agent-key-error>
                {error}
              </p>
            ) : null}
          </div>
          <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
            <Button type="button" variant="outline" onClick={() => keys.closeSheet()} data-agent-key-cancel>
              {v.cancelLabel}
            </Button>
            <Button type="submit" disabled={!v.canSave || busy} data-agent-key-save>
              {busy ? "Saving…" : v.saveLabel}
            </Button>
          </div>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
