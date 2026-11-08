// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { PlusIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { agentsView, useAgentKeys, useBridge, type AgentAccount, type AgentGroup } from "@/bridge";
import { SettingsGroup } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { GroupedRow } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import type { Connectors } from "@/hooks/use-connectors";

/** What the list can do with an app's sign-ins: the calls it makes against the host. */
export interface AccountActions {
  /** Makes `account` the one the app runs with. */
  onUse(group: AgentGroup, account: AgentAccount): void;
  /** Takes `account` off the app. */
  onRemove(group: AgentGroup, account: AgentAccount): void;
  /** Gives `account` a name. */
  onRename(group: AgentGroup, account: AgentAccount, name: string): void;
  /** Adds another sign-in (a token or key, named) to the app; rejects with the host's words. */
  onAdd(group: AgentGroup, name: string, value: string): Promise<void>;
}

const same = (a: AgentAccount, b: AgentAccount) => a.env === b.env && (a.slot ?? null) === (b.slot ?? null);

/** What the group says under it: why an agent signs in at all. */
export const SIGN_INS_WHY = "Sign in once so your agents can run on your other computers without asking again.";

/**
 * One coding app's sign-ins on its Settings page (Settings, Connections, the app): which one it runs with, and
 * make default, rename, remove and add. They used to sit in Home's agent menu, where nobody could tell what
 * they were for; the menu now only picks the agent for a job and links here.
 */
export function AgentSignIns({ agent, c }: { agent: string; c: Pick<Connectors, "input" | "useCredential"> }) {
  const { core } = useBridge();
  const keys = useAgentKeys();
  const group = useMemo(() => (core ? (agentsView(core, c.input)?.groups.find((g) => g.app === agent) ?? null) : null), [core, c.input, agent]);
  if (!group) return null;
  const actions: AccountActions = {
    onUse: (g, a) => void c.useCredential(g.app, { env: a.env, slot: a.slot }),
    onRemove: (_g, a) => void (a.slot ? keys.remove(a.env, { slot: a.slot }) : keys.remove(a.env, { keepOthers: true })).catch(() => {}),
    onRename: (_g, a, name) => void keys.rename(a.env, a.slot, name).catch(() => {}),
    // Claude Code's takes a token, the others the key of their provider.
    onAdd: (g, name, value) => keys.save(g.accounts[0]?.provider ?? (g.app === "claude-code" ? "anthropic" : g.app === "codex" ? "openai" : "anthropic"), value, null, {}, { add: true, ...(name ? { label: name } : {}) }),
  };
  return (
    <SettingsGroup title="Saved sign-ins" footer={SIGN_INS_WHY} data-agent-sign-ins={agent}>
      <SignInList group={group} actions={actions} />
    </SettingsGroup>
  );
}

/** The rows of `AgentSignIns`, drawn from the app's group (`connectors.agents`). */
export function SignInList({ group, actions }: { group: AgentGroup; actions: AccountActions }) {
  const [renaming, setRenaming] = useState<AgentAccount | null>(null);
  const [name, setName] = useState("");
  const [removing, setRemoving] = useState<AgentAccount | null>(null);
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await actions.onAdd(group, newName.trim(), value.trim());
      setAdding(false);
      setNewName("");
      setValue("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      {group.accounts.length === 0 ? <GroupedRow className="text-muted-foreground">No sign-in yet.</GroupedRow> : null}
      {group.accounts.map((a) => (
        <GroupedRow key={`${a.env}/${a.slot ?? ""}`} data-manage-account={a.slot ?? a.env} className="flex-wrap">
          {renaming && same(renaming, a) ? (
            <Input
              autoFocus
              value={name}
              aria-label="Name"
              className="min-w-0 flex-1"
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && name.trim()) {
                  actions.onRename(group, a, name.trim());
                  setRenaming(null);
                } else if (e.key === "Escape") {
                  e.stopPropagation();
                  setRenaming(null);
                }
              }}
            />
          ) : (
            <span className="min-w-0 flex-1 truncate">
              {a.line}
              {a.selected ? (
                <span data-manage-default="" className="ml-2 text-xs text-muted-foreground">
                  In use
                </span>
              ) : null}
            </span>
          )}
          {removing && same(removing, a) ? (
            <span className="flex items-center gap-1">
              <span className="text-xs text-muted-foreground">Remove this sign-in?</span>
              <Button
                size="sm"
                variant="ghost"
                className="text-destructive"
                data-manage-confirm-remove=""
                onClick={() => {
                  actions.onRemove(group, a);
                  setRemoving(null);
                }}
              >
                Remove
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setRemoving(null)}>
                Cancel
              </Button>
            </span>
          ) : renaming && same(renaming, a) ? null : (
            <span className="flex items-center gap-1">
              {a.selected ? null : (
                <Button size="sm" variant="ghost" data-manage-use="" onClick={() => actions.onUse(group, a)}>
                  Use this one
                </Button>
              )}
              <Button
                size="sm"
                variant="ghost"
                data-manage-rename=""
                onClick={() => {
                  setRenaming(a);
                  setName(a.line);
                }}
              >
                Rename
              </Button>
              <Button size="sm" variant="ghost" data-manage-remove="" onClick={() => setRemoving(a)}>
                Remove
              </Button>
            </span>
          )}
        </GroupedRow>
      ))}
      {adding ? (
        <GroupedRow data-manage-add-form="" className="flex-col items-stretch gap-2">
          <Input value={newName} maxLength={40} placeholder="Name (optional)" aria-label="Name" autoComplete="off" onChange={(e) => setNewName(e.target.value)} />
          <Input
            type="password"
            value={value}
            placeholder="Token or key"
            aria-label="Token or key"
            autoComplete="off"
            spellCheck={false}
            className="font-mono"
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && value.trim() && !busy) void add();
            }}
          />
          {error ? <p className="text-xs text-destructive">{error}</p> : null}
          <span className="flex items-center justify-end gap-1">
            <Button size="sm" variant="ghost" onClick={() => setAdding(false)}>
              Cancel
            </Button>
            <Button size="sm" variant="outline" data-manage-add-save="" disabled={busy || !value.trim()} onClick={() => void add()}>
              Add
            </Button>
          </span>
        </GroupedRow>
      ) : (
        <GroupedRow interactive className="cursor-default p-0">
          <button type="button" data-manage-add="" onClick={() => setAdding(true)} className="flex min-h-11 w-full items-center gap-2 px-4 text-left text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60">
            <PlusIcon className="size-3.5" />
            Add a sign-in
          </button>
        </GroupedRow>
      )}
    </>
  );
}
