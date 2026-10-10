// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { KeyRoundIcon, TriangleAlertIcon } from "lucide-react";
import { useState } from "react";

import { agentKeyForm, useAgentKeys, type AgentKeyProvider, type AgentKeyRowView, type AgentKeysHook } from "@/bridge";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { toastError } from "@/components/ui/toast";
import { addedText } from "@/lib/agent-keys";

export const Route = createFileRoute("/settings/agents")({ component: AgentKeysPage });

/**
 * Settings, Agents: the provider keys agents in your Spaces get. Anthropic
 * and OpenAI always have a row, other keys follow by variable name; Add or
 * Replace opens a sheet with a secure field (the key is never shown again),
 * Remove asks first. Every word is the app core's `agent_keys` view.
 */
function AgentKeysPage() {
  // The keys also change outside the app (`cua agent keys`): read them again when this page shows
  // and whenever the window gets focus.
  const keys = useAgentKeys({ watch: true });
  const { view } = keys;
  if (keys.unsupported) return <p className="px-1 text-[13px] text-muted-foreground">Agent keys are not available in this app yet.</p>;
  if (keys.isLoading) return <p className="px-1 text-[13px] text-muted-foreground" data-agent-keys-loading>Loading agent keys…</p>;
  if (!view) return <p className="px-1 text-[13px] text-muted-foreground">Agent keys need the app core, which isn't loaded.</p>;
  const confirm = keys.removing ? keys.removeConfirmOf(keys.removing) : null;

  return (
    <div data-agent-keys>
      <p className="mb-4 px-1 text-[13px] text-muted-foreground" data-agent-keys-intro>
        {view.intro}
      </p>
      {view.notice ? (
        <div data-agent-keys-notice className="mb-5 flex items-start gap-2.5 rounded-xl border border-warning/30 bg-warning/5 px-4 py-2.5 text-[13px]">
          <TriangleAlertIcon className="mt-0.5 size-4 shrink-0 text-warning" />
          <span className="min-w-0 flex-1">{view.notice}</span>
          {keys.error ? (
            <Button size="sm" variant="outline" onClick={() => void keys.refresh()}>
              Try again
            </Button>
          ) : null}
        </div>
      ) : null}

      <section className="mb-7">
        <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground" data-agent-keys-title>
          {view.title}
        </h2>
        <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
          {view.rows.map((r) => (
            <KeyRow key={`${r.provider}:${r.env}`} row={r} addedLabel={view.addedLabel} canEdit={view.canEdit} onEdit={() => keys.openSheet(r.provider, r.provider === "other" ? r.env : null)} onRemove={() => keys.askRemove(r.env)} />
          ))}
        </div>
        <div className="mt-3 flex items-start gap-3 px-1">
          <Button size="sm" variant="outline" disabled={!view.canEdit} onClick={() => keys.openSheet("other")} data-agent-key-add-other>
            {view.addOtherLabel}
          </Button>
          <p className="pt-1 text-xs text-muted-foreground" data-agent-key-other-help>
            {view.otherHelp}
          </p>
        </div>
      </section>

      {keys.sheet ? <KeySheet key={`${keys.sheet.provider}:${keys.sheet.env ?? ""}`} keys={keys} provider={keys.sheet.provider} env={keys.sheet.env} /> : null}
      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => !o && keys.askRemove(null)}
        title={confirm?.title ?? ""}
        description={confirm?.message ?? ""}
        confirmLabel={confirm?.confirmLabel ?? "Remove"}
        cancelLabel={confirm?.cancelLabel}
        destructive
        onConfirm={() => {
          const env = keys.removing;
          if (env) void keys.remove(env).catch(toastError("Couldn't remove the key"));
        }}
      />
    </div>
  );
}

function KeyRow({ row: r, addedLabel, canEdit, onEdit, onRemove }: { row: AgentKeyRowView; addedLabel: string; canEdit: boolean; onEdit: () => void; onRemove: () => void }) {
  return (
    <div data-agent-key-row={r.env} data-agent-key-provider={r.provider} data-set={String(r.set)} className="flex min-h-14 items-center gap-3 px-4 py-2.5">
      <KeyRoundIcon className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.75} />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="truncate text-[13px]" data-agent-key-title>
            {r.title}
          </span>
          {r.title !== r.env ? (
            <span className="truncate font-mono text-[11px] text-muted-foreground" data-agent-key-env>
              {r.env}
            </span>
          ) : null}
        </div>
        <div className="truncate text-xs text-muted-foreground" data-agent-key-detail>
          {r.detail}
        </div>
      </div>
      <div className="flex shrink-0 flex-col items-end">
        <span className={r.set ? "font-mono text-[12px]" : "text-[13px] text-muted-foreground"} data-agent-key-status>
          {r.status}
        </span>
        {r.addedMs !== null ? (
          <span className="text-[11px] text-muted-foreground" data-agent-key-added>
            {addedText(addedLabel, r.addedMs)}
          </span>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button size="sm" variant="outline" disabled={!canEdit} onClick={onEdit} data-agent-key-action>
          {r.actionLabel}
        </Button>
        {r.removeLabel ? (
          <Button size="sm" variant="ghost" disabled={!canEdit} onClick={onRemove} data-agent-key-remove>
            {r.removeLabel}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/** Add or replace a key. The key stays in this field until Save, then the sheet closes and it is gone. */
function KeySheet({ keys, provider, env }: { keys: AgentKeysHook; provider: AgentKeyProvider; env: string | null }) {
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
