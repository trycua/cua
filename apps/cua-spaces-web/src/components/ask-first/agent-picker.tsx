// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon, PlusIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { AgentAccount, AgentGroup, AgentsView } from "@/bridge";
import { HarnessMark } from "@/components/agents/harness-mark";
import { Popover, PopoverDivider } from "@/components/ui/popover";
import { agentLine, type AgentSelection } from "@/lib/ask-first-model";
import { cn } from "@/lib/utils";

/** What the menu can do with the accounts of an app: the calls Home makes against the host. */
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

/**
 * The agent menu of the ask box: the coding apps the person has connected
 * (Codex, Hermes, Claude Code and OpenCode first), each with its accounts as
 * rows to pick from. An app with no account has a Sign in row. The chevron on
 * an app's header opens that app's sign-ins in place: which is the default,
 * rename, remove and add. The model is a quiet line at the bottom, only for an
 * account that offers a choice. The chip shows the app and the account.
 */
export function AgentPicker({
  view,
  selected,
  onPick,
  onModel,
  onSignIn,
  onConfigure,
  accounts,
}: {
  view: AgentsView | null;
  selected: AgentSelection | null;
  onPick: (group: AgentGroup, account: AgentAccount) => void;
  onModel: (model: string) => void;
  /** Starts the sign-in of an app that has no account. */
  onSignIn: (group: AgentGroup) => void;
  /** Opens where the menu's apps are chosen (Settings, Connections). */
  onConfigure?: () => void;
  accounts: AccountActions;
}) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<{ kind: "manage"; app: string } | { kind: "model" } | null>(null);
  const focus = useRef<HTMLButtonElement>(null);
  const groups = view?.groups ?? [];
  useEffect(() => {
    if (!open) setPage(null);
  }, [open]);
  const manageGroup = page?.kind === "manage" ? groups.find((g) => g.app === page.app) : undefined;
  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      width={340}
      initialFocus={focus}
      popupProps={{ "data-agent-menu": "" } as never}
      trigger={
        <button
          type="button"
          data-picker="agent"
          className={cn(
            "flex h-9 max-w-[22rem] items-center gap-2 rounded-lg px-2.5 text-[13px] outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07]",
            !selected && "text-brand-strong",
          )}
          aria-label={selected ? `Agent: ${agentLine(selected)}, ${selected.account.line}` : "Sign in to an agent"}
        >
          {selected ? (
            <>
              <HarnessMark agent={selected.group.app} name={selected.group.label} plain />
              <span data-ask-agent="" className="truncate font-medium">
                {agentLine(selected)}
              </span>
            </>
          ) : (
            <span data-ask-agent="">Sign in to an agent</span>
          )}
          <ChevronDownIcon className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      }
    >
      <button ref={focus} aria-hidden tabIndex={-1} className="sr-only" />
      {manageGroup ? (
        <ManagePage group={manageGroup} actions={accounts} onBack={() => setPage(null)} />
      ) : page?.kind === "model" && selected ? (
        <ModelPage
          selected={selected}
          onBack={() => setPage(null)}
          onPick={(m) => {
            onModel(m);
            setOpen(false);
          }}
        />
      ) : (
        <div role="listbox" aria-label="Agents" className="max-h-[420px] overflow-y-auto">
          {groups.map((g) => (
            <div key={g.app} data-agent-group={g.app} data-state={g.state} className="py-0.5">
              <div className="flex items-center gap-2 pr-1 pl-2.5">
                <HarnessMark agent={g.app} name={g.label} plain />
                <span className="min-w-0 flex-1 truncate py-1.5 text-[12px] font-medium text-muted-foreground">{g.label}</span>
                <button
                  type="button"
                  data-agent-manage={g.app}
                  aria-label={`${g.label}: manage sign-ins`}
                  onClick={() => setPage({ kind: "manage", app: g.app })}
                  className="flex size-7 shrink-0 items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                >
                  <ChevronRightIcon className="size-3.5" />
                </button>
              </div>
              {g.accounts.length ? (
                g.accounts.map((a) => {
                  const on = selected?.harness === g.harness && same(selected.account, a);
                  return (
                    <button
                      type="button"
                      role="option"
                      aria-selected={on}
                      key={`${a.env}/${a.slot ?? ""}`}
                      data-agent-account={a.slot ?? a.env}
                      onClick={() => {
                        onPick(g, a);
                        setOpen(false);
                      }}
                      className="flex w-full items-center gap-2 rounded-lg py-1.5 pr-2.5 pl-9 text-left text-[13px] outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60"
                    >
                      <span className="min-w-0 flex-1 truncate">{a.line}</span>
                      {on ? <CheckIcon className="size-3.5 shrink-0" /> : null}
                    </button>
                  );
                })
              ) : (
                <button
                  type="button"
                  data-agent-sign-in={g.app}
                  onClick={() => {
                    setOpen(false);
                    onSignIn(g);
                  }}
                  className="flex w-full items-center rounded-lg py-1.5 pr-2.5 pl-9 text-left text-[13px] text-brand-strong outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60"
                >
                  {g.signInLabel}
                </button>
              )}
            </div>
          ))}
          {selected && selected.account.models.length > 1 ? (
            <>
              <PopoverDivider />
              <button
                type="button"
                data-agent-model-row=""
                onClick={() => setPage({ kind: "model" })}
                className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                <span className="text-muted-foreground">Model</span>
                <span data-ask-model="" className="min-w-0 flex-1 truncate text-right">
                  {selected.model.label}
                </span>
                <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
              </button>
            </>
          ) : null}
          {onConfigure ? (
            <>
              {selected && selected.account.models.length > 1 ? null : <PopoverDivider />}
              <button
                type="button"
                data-agent-configure=""
                onClick={() => {
                  setOpen(false);
                  onConfigure();
                }}
                className="flex w-full items-center rounded-lg px-2.5 py-1.5 text-left text-[13px] text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                Configure agents
              </button>
            </>
          ) : null}
        </div>
      )}
    </Popover>
  );
}

function Back({ onBack, title }: { onBack: () => void; title: string }) {
  return (
    <div className="flex items-center gap-1 pb-1">
      <button type="button" data-agent-back="" aria-label="Back" onClick={onBack} className="flex size-7 items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60">
        <ChevronLeftIcon className="size-3.5" />
      </button>
      <span className="text-[13px] font-medium">{title}</span>
    </div>
  );
}

const small = "rounded-lg px-1.5 py-0.5 text-[12px] text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60";

/** An app's sign-ins: the default, rename, remove and add. */
function ManagePage({ group, actions, onBack }: { group: AgentGroup; actions: AccountActions; onBack: () => void }) {
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
    <div data-agent-manage-page={group.app}>
      <Back onBack={onBack} title={`${group.label} sign-ins`} />
      {group.accounts.length === 0 ? <p className="px-2.5 py-2 text-[13px] text-muted-foreground">No sign-in yet.</p> : null}
      {group.accounts.map((a) => (
        <div key={`${a.env}/${a.slot ?? ""}`} data-manage-account={a.slot ?? a.env} className="rounded-lg px-2.5 py-1.5">
          {renaming && same(renaming, a) ? (
            <input
              autoFocus
              value={name}
              aria-label="Name"
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
              className="w-full rounded-lg bg-foreground/[0.05] px-2.5 py-1.5 text-[13px] outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
            />
          ) : (
            <div className="flex items-center gap-2 text-[13px]">
              <span className="min-w-0 flex-1 truncate">{a.line}</span>
              {a.selected ? (
                <span data-manage-default="" className="text-[12px] text-muted-foreground">
                  Default
                </span>
              ) : null}
            </div>
          )}
          {removing && same(removing, a) ? (
            <div className="mt-1 flex items-center gap-1 text-[12px]">
              <span className="text-muted-foreground">Remove this sign-in?</span>
              <button
                type="button"
                data-manage-confirm-remove=""
                onClick={() => {
                  actions.onRemove(group, a);
                  setRemoving(null);
                }}
                className={cn(small, "text-destructive hover:text-destructive")}
              >
                Remove
              </button>
              <button type="button" onClick={() => setRemoving(null)} className={small}>
                Cancel
              </button>
            </div>
          ) : (
            <div className="mt-0.5 flex items-center gap-1">
              {a.selected ? null : (
                <button type="button" data-manage-use="" onClick={() => actions.onUse(group, a)} className={small}>
                  Make default
                </button>
              )}
              <button
                type="button"
                data-manage-rename=""
                onClick={() => {
                  setRenaming(a);
                  setName(a.line);
                }}
                className={small}
              >
                Rename
              </button>
              <button type="button" data-manage-remove="" onClick={() => setRemoving(a)} className={small}>
                Remove
              </button>
            </div>
          )}
        </div>
      ))}
      <PopoverDivider />
      {adding ? (
        <div data-manage-add-form="" className="flex flex-col gap-1.5 px-2.5 py-1.5">
          <input value={newName} maxLength={40} placeholder="Name (optional)" aria-label="Name" autoComplete="off" onChange={(e) => setNewName(e.target.value)} className="w-full rounded-lg bg-foreground/[0.05] px-2.5 py-1.5 text-[13px] outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/60" />
          <input
            type="password"
            value={value}
            placeholder="Token or key"
            aria-label="Token or key"
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && value.trim() && !busy) void add();
            }}
            className="w-full rounded-lg bg-foreground/[0.05] px-2.5 py-1.5 font-mono text-[13px] outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
          />
          {error ? <p className="text-[12px] text-destructive">{error}</p> : null}
          <div className="flex items-center justify-end gap-1">
            <button type="button" onClick={() => setAdding(false)} className={small}>
              Cancel
            </button>
            <button type="button" data-manage-add-save="" disabled={busy || !value.trim()} onClick={() => void add()} className={cn(small, "text-foreground disabled:opacity-50")}>
              Add
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          data-manage-add=""
          onClick={() => setAdding(true)}
          className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <PlusIcon className="size-3.5" />
          Add a sign-in
        </button>
      )}
    </div>
  );
}

/** The models of the account in use. */
function ModelPage({ selected, onBack, onPick }: { selected: AgentSelection; onBack: () => void; onPick: (model: string) => void }) {
  return (
    <div data-agent-model-page="">
      <Back onBack={onBack} title="Model" />
      <div role="listbox" aria-label="Models" className="max-h-[320px] overflow-y-auto">
        {selected.account.models.filter((m) => !m.legacy || m.id === selected.model.id).map((m) => (
          <button
            type="button"
            role="option"
            aria-selected={m.id === selected.model.id}
            key={m.id}
            data-model-row={m.id}
            onClick={() => onPick(m.id)}
            className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60"
          >
            <span className="min-w-0 flex-1 truncate">{m.label}</span>
            {m.fast ? <span className="text-[11px] text-muted-foreground">Fast</span> : null}
            {m.id === selected.model.id ? <CheckIcon className="size-3.5 shrink-0" /> : null}
          </button>
        ))}
      </div>
    </div>
  );
}
