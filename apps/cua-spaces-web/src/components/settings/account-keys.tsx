// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { TriangleAlertIcon } from "lucide-react";

import { useAgentKeys, type AgentKeyRowView } from "@/bridge";
import { KeySheet } from "@/components/agents/key-sheet";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { SettingsGroup } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { GroupedRow, SettingRow } from "@/components/ui/grouped";
import { toastError } from "@/components/ui/toast";
import { addedText } from "@/lib/agent-keys";
import { KEYS_ADD_OTHER_BUTTON, KEYS_ADD_OTHER_LABEL, KEYS_OTHER_HELP, KEYS_TITLE, plainKeysIntro } from "@/lib/settings-copy";

/**
 * Settings, Connectors, Account keys: the provider keys agents in your Spaces get. Anthropic
 * and OpenAI always have a row, other keys follow by variable name; Add or
 * Replace opens a sheet with a secure field (the key is never shown again),
 * Remove asks first. Every word is the app core's `agent_keys` view.
 */
export function AccountKeysSection() {
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
      {view.notice ? (
        <div data-agent-keys-notice className="mb-4 flex items-start gap-2.5 rounded-lg bg-warning/10 px-4 py-2.5 text-[13px]">
          <TriangleAlertIcon className="mt-0.5 size-4 shrink-0 text-warning" />
          <span className="min-w-0 flex-1">{view.notice}</span>
          {keys.error ? (
            <Button size="sm" variant="outline" onClick={() => void keys.refresh()}>
              Try again
            </Button>
          ) : null}
        </div>
      ) : null}

      <SettingsGroup title={<span data-agent-keys-title>{KEYS_TITLE}</span>} footer={<span data-agent-keys-intro>{plainKeysIntro(view.intro)}</span>}>
        {view.rows.map((r) => (
          <KeyRow key={`${r.provider}:${r.env}`} row={r} addedLabel={view.addedLabel} canEdit={view.canEdit} onEdit={() => keys.openSheet(r.provider, r.provider === "other" ? r.env : null)} onRemove={() => keys.askRemove(r.env)} />
        ))}
        <SettingRow
          label={KEYS_ADD_OTHER_LABEL}
          help={<span data-agent-key-other-help>{KEYS_OTHER_HELP}</span>}
          control={
            <Button size="sm" variant="outline" disabled={!view.canEdit} onClick={() => keys.openSheet("other")} data-agent-key-add-other>
              {KEYS_ADD_OTHER_BUTTON}
            </Button>
          }
        />
      </SettingsGroup>

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
    <GroupedRow data-agent-key-row={r.env} data-agent-key-provider={r.provider} data-set={String(r.set)}>
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
    </GroupedRow>
  );
}
