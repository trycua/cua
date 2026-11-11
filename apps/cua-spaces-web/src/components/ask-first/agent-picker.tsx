// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { AgentAccount, AgentGroup, AgentsView } from "@/bridge";
import { HarnessMark } from "@/components/agents/harness-mark";
import { Popover, PopoverDivider } from "@/components/ui/popover";
import { chipLine, type AgentSelection } from "@/lib/ask-first-model";
import { effortKey, effortLevels } from "@/lib/model-effort";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

const same = (a: AgentAccount, b: AgentAccount) => a.env === b.env && (a.slot ?? null) === (b.slot ?? null);

/**
 * The agent menu of the ask box: the coding apps the person has connected
 * (Codex, Hermes, Claude Code and OpenCode first), each with its accounts as
 * rows to pick from. An app with no account has a Sign in row, and one line at
 * the top says why: an agent signs in once, so it can run on your other
 * computers without asking again. Adding, renaming and removing sign-ins is
 * Settings' job (each app's page under Connections), not the job box's. The
 * model is a quiet line at the bottom, only for an account that offers a
 * choice. The chip shows the app and the account.
 */
export function AgentPicker({
  view,
  selected,
  onPick,
  onModel,
  onEffort,
  onSignIn,
  onConfigure,
}: {
  view: AgentsView | null;
  selected: AgentSelection | null;
  onPick: (group: AgentGroup, account: AgentAccount) => void;
  onModel: (model: string) => void;
  /** Sets the reasoning level of the model in use. */
  onEffort?: (level: string) => void;
  /** Starts the sign-in of an app that has no account. */
  onSignIn: (group: AgentGroup) => void;
  /** Opens where the agents and their sign-ins are managed (Settings, Connections). */
  onConfigure?: () => void;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<{ kind: "model" } | null>(null);
  const focus = useRef<HTMLButtonElement>(null);
  const chip = selected ? chipLine(selected, t) : null;
  const groups = view?.groups ?? [];
  useEffect(() => {
    if (!open) setPage(null);
  }, [open]);
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
          aria-label={
            selected
              ? t("home.agent_picker.trigger_label", { agent: `${chip!.lead} ${chip!.trail}`, account: selected.account.line })
              : t("home.agent_picker.choose")
          }
        >
          {selected ? (
            <>
              <HarnessMark agent={selected.group.app} name={selected.group.label} plain />
              <span data-ask-agent="" className="truncate">
                <span data-chip-lead="" className="font-medium">
                  {chip!.lead}
                </span>
                <span data-chip-trail="" className="ml-1.5 text-muted-foreground">
                  {chip!.trail}
                </span>
              </span>
            </>
          ) : (
            <span data-ask-agent="">{t("home.agent_picker.choose")}</span>
          )}
          <ChevronDownIcon className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      }
    >
      <button ref={focus} aria-hidden tabIndex={-1} className="sr-only" />
      {page?.kind === "model" && selected ? (
        <ModelPage
          selected={selected}
          onBack={() => setPage(null)}
          onPick={(m) => {
            onModel(m);
            setOpen(false);
          }}
        />
      ) : (
        <div role="listbox" aria-label={t("home.agent_picker.agents")} className="max-h-[420px] overflow-y-auto">
          <p data-agent-menu-why="" className="px-2.5 pt-1 pb-1.5 text-[12px] leading-snug text-muted-foreground">
            {t("home.agent_picker.why")}
          </p>
          {groups.map((g) => (
            <div key={g.app} data-agent-group={g.app} data-state={g.state} className="py-0.5">
              <div className="flex items-center gap-2 pr-1 pl-2.5">
                <HarnessMark agent={g.app} name={g.label} plain />
                <span className="min-w-0 flex-1 truncate py-1.5 text-[12px] font-medium text-muted-foreground">{g.label}</span>
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
          {selected && (selected.account.models.length > 1 || selected.effort) ? <PopoverDivider /> : null}
          {selected && selected.account.models.length > 1 ? (
            <>
              <button
                type="button"
                data-agent-model-row=""
                onClick={() => setPage({ kind: "model" })}
                className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                <span className="text-muted-foreground">{t("home.agent_picker.model")}</span>
                <span data-ask-model="" className="min-w-0 flex-1 truncate text-right">
                  {selected.model.label}
                </span>
                <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
              </button>
            </>
          ) : null}
          {selected ? <EffortRow selected={selected} onEffort={onEffort} /> : null}
          {onConfigure ? (
            <>
              {selected && (selected.account.models.length > 1 || selected.effort) ? null : <PopoverDivider />}
              <button
                type="button"
                data-agent-configure=""
                onClick={() => {
                  setOpen(false);
                  onConfigure();
                }}
                className="flex w-full items-center rounded-lg px-2.5 py-1.5 text-left text-[13px] text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                {t("home.agent_picker.manage")}
              </button>
            </>
          ) : null}
        </div>
      )}
    </Popover>
  );
}

/** The reasoning levels of the model in use, all on one line; nothing when the model has none. */
function EffortRow({ selected, onEffort }: { selected: AgentSelection; onEffort?: (level: string) => void }) {
  const t = useT();
  const e = effortLevels(selected.harness, selected.model.id);
  if (!e || !onEffort) return null;
  return (
    <div data-agent-effort-row="" className="flex items-center gap-2 px-2.5 py-1.5 text-[13px]">
      <span className="text-muted-foreground">{t("home.agent_picker.reasoning")}</span>
      <div role="radiogroup" aria-label={t("home.agent_picker.reasoning")} className="ml-auto flex items-center gap-0.5 rounded-lg bg-foreground/[0.05] p-0.5">
        {e.levels.map((l) => (
          <button
            type="button"
            role="radio"
            key={l}
            aria-checked={l === selected.effort}
            data-effort-option={l}
            onClick={() => onEffort(l)}
            className={cn(
              "rounded-lg px-1.5 py-0.5 text-[12px] outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
              l === selected.effort ? "bg-background font-medium text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
            )}
          >
            {t(effortKey(l, true))}
          </button>
        ))}
      </div>
    </div>
  );
}

function Back({ onBack, title }: { onBack: () => void; title: string }) {
  const t = useT();
  return (
    <div className="flex items-center gap-1 pb-1">
      <button
        type="button"
        data-agent-back=""
        aria-label={t("home.agent_picker.back")}
        onClick={onBack}
        className="flex size-7 items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <ChevronLeftIcon className="size-3.5" />
      </button>
      <span className="text-[13px] font-medium">{title}</span>
    </div>
  );
}

/** The models of the account in use. */
function ModelPage({ selected, onBack, onPick }: { selected: AgentSelection; onBack: () => void; onPick: (model: string) => void }) {
  const t = useT();
  return (
    <div data-agent-model-page="">
      <Back onBack={onBack} title={t("home.agent_picker.model")} />
      <div role="listbox" aria-label={t("home.agent_picker.models")} className="max-h-[320px] overflow-y-auto">
        {selected.account.models
          .filter((m) => !m.legacy || m.id === selected.model.id)
          .map((m) => (
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
              {m.fast ? <span className="text-[11px] text-muted-foreground">{t("home.agent_picker.fast")}</span> : null}
              {m.id === selected.model.id ? <CheckIcon className="size-3.5 shrink-0" /> : null}
            </button>
          ))}
      </div>
    </div>
  );
}
