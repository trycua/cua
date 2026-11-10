// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { CheckIcon, PlayIcon, XIcon } from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";

import { agentsView, useBridge, type AgentsView } from "@/bridge";
import { useWherePick } from "@/components/ask-first/this-mac-confirm";
import { RepoChip } from "@/components/ask-first/repo-chip";
import { agentName } from "@/components/ask-first/task-agents";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { toast } from "@/components/ui/toast";
import { useAgentMenu } from "@/hooks/use-agent-menu";
import { connectFailureLine, useConnectors } from "@/hooks/use-connectors";
import { useT } from "@/i18n";
import { rememberAgent, resolveAgent, usePickedAgent } from "@/lib/ask-first-model";
import { useRepos } from "@/lib/ask-first-repo";
import { thisComputerLabel } from "@/lib/host-labels";
import { placeJob, reasonText, type HomeComputer, type PlaceRow, type PoolComputer } from "@/lib/pool";
import { cn } from "@/lib/utils";

import { AgentKey, ComputerKey } from "./keys";

/** What the sheet asks to run: the agent's harness, account and model are the agent menu's own pick. */
export interface RunRequest {
  prompt: string;
  harness: string;
  model: string;
  /** The computer picked in "Where", or null to let the pool pick. */
  picked: string | null;
}

/** The two coding agents the sheet offers first; every other app of the menu is in More options. */
const SHEET_AGENTS = ["claude-code", "openai-codex"];

/** The agents the sheet offers: Claude Code and Codex when the menu has them, else the menu's own apps. */
export function agentChoices(view: AgentsView | null) {
  const groups = view?.groups ?? [];
  const two = SHEET_AGENTS.map((h) => groups.find((g) => g.harness === h)).filter((g): g is NonNullable<typeof g> => Boolean(g));
  return two.length ? two : groups;
}

/**
 * "Run something": pick the agent, the project and the task; the computer with the most room is picked for
 * you (`pool.place`) and you can change it. The job starts where it is placed and shows on that computer's card.
 */
export function RunSomethingSheet({
  open,
  onOpenChange,
  computers,
  pool,
  onRun,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  computers: readonly HomeComputer[];
  pool: readonly PoolComputer[];
  /** Starts the job; `started` is called once it has (not when This Mac's confirm is declined), and clears the task. */
  onRun: (req: RunRequest, started: () => void) => void;
}) {
  const t = useT();
  const taskId = useId();
  const { core } = useBridge();
  const connectors = useConnectors();
  const menu = useAgentMenu(connectors);
  const view = useMemo(() => (core ? agentsView(core, connectors.input, menu.prefs) : null), [core, connectors.input, menu.prefs]);
  const remembered = usePickedAgent();
  const offered = agentChoices(view);
  const rememberedOffered = remembered && offered.some((g) => g.harness === remembered.harness) ? remembered : null;
  const firstConnected = offered.find((g) => g.state === "connected");
  const selected = resolveAgent(view, rememberedOffered ?? (firstConnected ? { harness: firstConnected.harness } : null));
  const { current: repo } = useRepos();
  const [text, setText] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  useEffect(() => {
    if (!open) setPicked(null);
  }, [open]);
  const placement = useMemo(() => placeJob(core, { computers: [...pool], picked }), [core, pool, picked]);
  const byId = new Map(computers.map((c) => [c.machine.id, c] as const));
  const chosen = placement.chosen ? byId.get(placement.chosen) : undefined;
  const ready = Boolean(text.trim() && selected && chosen);

  const signIn = async (app: string, label: string) => {
    if ((await connectors.connectApp(app)) === "failed")
      toast(t("home.ask_box.sign_in_failed", { label }), {
        description: connectFailureLine(app) || t("home.ask_box.sign_in_failed_hint"),
        type: "error",
        group: `sign-in:${app}`,
      });
  };

  const submit = () => {
    if (!ready || !selected) return;
    // The task stays until the job has started: This Mac's confirm can still say "Pick another".
    onRun({ prompt: text.trim(), harness: selected.harness, model: selected.model.id, picked: placement.chosen }, () => setText(""));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[8vh] max-h-[84vh] w-[min(720px,calc(100vw-2rem))] overflow-y-auto bg-popover p-6 sm:p-8" data-run-sheet="">
        <div className="flex items-center justify-between gap-4">
          <DialogTitle className="text-[20px] font-semibold">{t("home.run.title")}</DialogTitle>
          <Button variant="ghost" size="icon" aria-label={t("home.run.close")} onClick={() => onOpenChange(false)}>
            <XIcon />
          </Button>
        </div>

        <p className="mt-5 text-[13px] text-muted-foreground">{t("home.run.agent")}</p>
        <div className="mt-2 grid gap-3 sm:grid-cols-2" role="radiogroup" aria-label={t("home.run.agent")}>
          {offered.map((g) => {
            const on = selected?.harness === g.harness;
            const connected = g.state === "connected";
            return (
              <button
                key={g.harness}
                type="button"
                role="radio"
                aria-checked={on}
                data-run-agent={g.harness}
                onClick={() => (connected ? rememberAgent({ harness: g.harness }) : void signIn(g.app, g.label))}
                className={cn(
                  "flex items-center gap-3 rounded-lg bg-tile px-4 py-3 text-left outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60",
                  on && "ring-2 ring-foreground",
                )}
              >
                <AgentKey harness={g.harness} size={36} />
                <span className="min-w-0">
                  <span className="block truncate text-[15px] font-medium">{g.label}</span>
                  <span className="block truncate text-[13px] text-muted-foreground">
                    {connected ? t("home.run.signed_in") : g.state === "needs-reconnect" ? t("home.run.sign_in_again") : g.signInLabel}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
        {/* Said under the agents, where it is fixed: below the Run button it sat off screen in a short window. */}
        {!selected && view ? (
          <p className="mt-2 text-[13px] text-muted-foreground" data-run-sign-in-first="">
            {t("home.run.sign_in_first")}
          </p>
        ) : null}

        <p className="mt-5 text-[13px] text-muted-foreground">{t("home.run.project")}</p>
        <div className="mt-2">
          <RepoChip repo={repo} />
        </div>

        <label htmlFor={taskId} className="mt-5 block text-[13px] text-muted-foreground">
          {t("home.run.task")}
        </label>
        <textarea
          id={taskId}
          data-run-task=""
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
          }}
          rows={3}
          placeholder={t("home.run.task_placeholder")}
          className="mt-2 w-full resize-none rounded-lg bg-tile px-4 py-3 text-[15px] outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        />

        <p className="mt-5 text-[13px] text-muted-foreground">{t("home.run.where")}</p>
        <ul className="mt-2 flex flex-col gap-1" role="radiogroup" aria-label={t("home.run.where_label")}>
          {placement.rows.map((row) => {
            const c = byId.get(row.id);
            if (!c) return null;
            return <WhereRow key={row.id} row={row} computer={c} chosen={row.id === placement.chosen} onPick={() => setPicked(row.id)} />;
          })}
        </ul>

        <div className="mt-6 flex flex-wrap items-center justify-end gap-3">
          <p className="mr-auto max-w-[320px] text-[13px] text-muted-foreground">
            {t("home.run.watch")}{" "}
            <Link to="/home/ask" onClick={() => onOpenChange(false)} className="text-foreground underline-offset-2 hover:underline" data-run-more="">
              {t("home.run.more_options")}
            </Link>
          </p>
          <Button variant="outline" size="lg" onClick={() => onOpenChange(false)}>
            {t("home.run.cancel")}
          </Button>
          <Button size="lg" disabled={!ready} onClick={submit} data-run-submit="">
            <PlayIcon />
            {chosen ? t("home.run.run_on", { computer: chosen.machine.name }) : t("home.run.no_computer")}
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

function WhereRow({ row, computer, chosen, onPick }: { row: PlaceRow; computer: HomeComputer; chosen: boolean; onPick: () => void }) {
  const t = useT();
  const note = computer.machine.current && row.reason === "not-sharing" ? t("home.run.reason_set_up", { computer: thisComputerLabel() }) : reasonText(row);
  return (
    <li>
      <button
        type="button"
        role="radio"
        aria-checked={chosen}
        disabled={!row.eligible}
        onClick={onPick}
        data-run-where={row.id}
        data-reason={row.reason}
        className={cn(
          "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60 enabled:hover:bg-accent",
          chosen && "bg-tile",
          !row.eligible && "opacity-60",
        )}
      >
        <ComputerKey os={computer.node.os} size={28} dim={!row.eligible} />
        <span className="shrink-0 text-[15px] font-medium">{computer.machine.name}</span>
        <span className="min-w-0 flex-1 truncate text-[13px] text-muted-foreground">{note}</span>
        {chosen ? <CheckIcon aria-hidden className="size-4 shrink-0" /> : null}
      </button>
    </li>
  );
}

/** The confirm This Mac asks once before an agent works on your own files, for the sheet's agent. */
export const useRunPick = (harness: string) => useWherePick(agentName(harness));
