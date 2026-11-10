// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";

import { agentsView, useAgentChats, useBridge, useMachines, useSpaces, type AgentAccount, type AgentGroup, type AgentRunSummary } from "@/bridge";
import { useChildren } from "@/components/children/use-children";
import { useAgentAttempts } from "@/hooks/use-agent-attempts";
import { useAgentMenu } from "@/hooks/use-agent-menu";
import { toast } from "@/components/ui/toast";
import { connectFailureLine, useConnectors } from "@/hooks/use-connectors";
import { OsIcon } from "@/components/os-icon";
import { currentWhere, SUGGESTIONS, thisMacConfirmed, useAskTasks, type AskTask, type TaskState } from "@/lib/ask-first";
import { pickAccount, pickOf, rememberAgent, resolveAgent, usePickedAgent } from "@/lib/ask-first-model";
import { reusableHome } from "@/lib/agent-home/home-space";
import { useRepos } from "@/lib/ask-first-repo";
import { useListedSpaces } from "@/components/spaces/listed";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

import { AskBox, type TaskWhere } from "./ask-box";
import { BringTheRest } from "./bring-the-rest";
import { agentName } from "./task-agents";
import { AgentStatus } from "./agent-status";
import { useAgentReadiness } from "./use-agent-readiness";
import { useKeepHomeWarm } from "./use-keep-home-warm";
import { YourAgents, YourSpaces as YourSpacesRow } from "./home-sections";
import { useWherePick } from "./this-mac-confirm";
import { useStartTask } from "./use-start-task";
import { useThisMacTarget } from "./use-this-mac-target";
import { useWhereOptions } from "./where-options";
import { homeDefaultWhere } from "@/lib/this-mac-target";
import { PrepareLine } from "@/components/prepare/prepare-line";
import { usePrepareAction } from "@/components/prepare/use-prepare-action";
import { usePrepare } from "@/lib/prepare/store";

/** The run a task follows, from the runs in your running Spaces. */
export function taskRun(runs: readonly AgentRunSummary[] | undefined, task: AskTask): AgentRunSummary | undefined {
  return task.runId ? runs?.find((r) => r.runId === task.runId && r.spaceId === task.spaceId) : undefined;
}

export function StateDot({ state }: { state: TaskState }) {
  const tone =
    state === "working"
      ? "bg-success"
      : state === "needs-you" || state === "stuck"
        ? "bg-warning"
        : state === "failed" || state === "didnt-start"
          ? "bg-destructive"
          : state === "done"
            ? "bg-success"
            : "bg-muted-foreground/40";
  return (
    <span className={cn("relative inline-flex size-2 shrink-0 rounded-full", tone)}>
      {state === "working" ? <span className="absolute inset-0 animate-ping rounded-full bg-success/60" /> : null}
    </span>
  );
}

/**
 * The "Ask first" Home (CUA-2346): one question and one box. With no tasks
 * yet, three real jobs sit under it; afterwards, the tasks read like
 * conversations (the one that needs you first), and your Spaces underneath.
 */
export function AskFirstHome() {
  const t = useT();
  const tasks = useAskTasks();
  const { data: spaces } = useSpaces();
  const listed = useListedSpaces(spaces);
  const { kinds, spaces: pickable, computers } = useWhereOptions();
  // This computer, once it's in the account, is where a job goes unless you pick (no Space needed first).
  const thisMac = useThisMacTarget();
  const { start } = useStartTask();
  const { core } = useBridge();
  // The agent: the last app, account and model picked while they are still there, else the first connected app. The
  // account is the app's own choice (kept by the host), so a run uses the one shown.
  const connectors = useConnectors();
  // Which apps the menu lists is the person's choice (Settings, Connections); an app they connect shows again.
  const menu = useAgentMenu(connectors);
  const navigate = useNavigate();
  const agents = useMemo(() => (core ? agentsView(core, connectors.input, menu.prefs) : null), [core, connectors.input, menu.prefs]);
  const picked = usePickedAgent();
  const selected = resolveAgent(agents, picked);
  const { current: repo } = useRepos();
  const [text, setText] = useState("");
  // With Child Spaces on (and this Home, the Ask first experiment), the job goes to the agent's Home Space, which
  // makes the Spaces it needs on your computers when asked in plain words.
  const children = useChildren();
  const agentHome = Boolean(children);
  const chosenAgent = selected?.harness ?? "claude-code";
  const { data: machineRows } = useMachines();
  const machines = useMemo(() => (machineRows ?? []).map((m) => ({ id: m.id, name: m.name, current: m.current, online: m.online })), [machineRows]);
  useKeepHomeWarm(chosenAgent, agentHome);
  const readiness = useAgentReadiness(chosenAgent, agentHome, agents, selected);
  // This computer once it's in the account, else a new Linux computer, unless you pick; a pick that's no longer
  // offered falls back to it.
  const [pickedWhere, setWhere] = useState<TaskWhere | null>(null);
  // The first Space being prepared after the first run: silent unless it fails.
  const prep = usePrepare();
  const onPrepareAction = usePrepareAction();
  const [whereOpen, setWhereOpen] = useState(false);
  const pick = useWherePick(agentName(chosenAgent));
  const where = currentWhere(pickedWhere ?? (agentHome ? { kind: "agent" } : homeDefaultWhere(thisMac.readiness, kinds)), kinds, [...pickable, ...computers]);
  const submit = (prompt = text, w = where) => {
    if (!prompt.trim() || !selected) return;
    // The default can be This Mac itself: it asks once, as picking it does.
    if (w.kind === "this-mac" && thisMac.readiness === "needs-setup") return;
    if (w.kind === "this-mac" && !thisMacConfirmed()) {
      pick.choose(
        w,
        (ok) => submit(prompt, ok),
        () => setWhereOpen(true),
      );
      return;
    }
    // The run uses the app's own choice of account: make it the one shown.
    if (!selected.account.selected) void connectors.useCredential(selected.group.app, { env: selected.account.env, slot: selected.account.slot });
    start({ prompt, agent: chosenAgent, where: w, model: selected.model.id, repo });
    setText("");
  };
  // A sign-in that did not work says so, in plain words, where the person clicked.
  const signInTo = async (app: string, label: string) => {
    if ((await connectors.connectApp(app)) === "failed") {
      toast(t("home.ask_box.sign_in_failed", { label }), {
        description: connectFailureLine(app) || t("home.ask_box.sign_in_failed_hint"),
        type: "error",
        group: `sign-in:${app}`,
      });
    }
  };
  const useAccount = (group: AgentGroup, account: AgentAccount) => {
    rememberAgent(pickAccount(group, account, picked));
    if (!account.selected) void connectors.useCredential(group.app, { env: account.env, slot: account.slot });
  };

  // Home lists agents and Spaces, not tasks: with neither (a first run,
  // or tasks whose Spaces are gone), Home is the welcome again.
  const attempts = useAgentAttempts();
  const { chats, loading: chatsLoading } = useAgentChats(attempts);
  // The home being prepared since launch, and the agent waiting in it, are not "yours" yet: until a job
  // is asked for, the welcome and its suggestions stay.
  const prepared = (id: string) => Boolean(prep.state?.wanted && (id === prep.state.homeSpaceId || id === prep.state.pendingId));
  const first = prep.state?.wanted ? tasks.length === 0 && listed.counted.every((c) => prepared(c.id)) : !chats?.length && listed.counted.length === 0;
  // Nothing is drawn until the Spaces have loaded and, when there are some, their agents have answered (at most
  // 0.8 s): otherwise the welcome turns into the lists, or the agents push the Spaces down, as the page loads.
  // Once drawn, it stays drawn: a later read of the agents never blanks the page again.
  const waited = useElapsed(800);
  const shown = useRef(false);
  shown.current ||= spaces !== undefined && (waited || listed.counted.length === 0 || (chats !== null && chatsLoading === 0));
  if (!shown.current) return <div className="h-full" data-ask-first-home="" aria-busy="true" />;

  return (
    <div className="h-full scroll-y-stable" data-ask-first-home="">
      <div className={cn("mx-auto flex w-full flex-col gap-8 px-8 pb-16", first ? "max-w-[880px] pt-24" : "max-w-[1040px] pt-10")}>
        <AskBox
          text={text}
          onText={setText}
          view={agents}
          selected={selected}
          onPickAccount={useAccount}
          onModel={(model) => selected && rememberAgent({ ...pickOf(selected), model })}
          onSignIn={(g) => void signInTo(g.app, g.label)}
          onConfigure={() => void navigate({ to: "/settings/connections" })}
          repo={repo}
          where={where}
          onWhere={(w) => pick.choose(w, setWhere, () => setWhereOpen(true))}
          whereOpen={whereOpen}
          onWhereOpen={setWhereOpen}
          kinds={kinds}
          spaces={pickable}
          computers={computers}
          thisMac={{ readiness: thisMac.readiness, busy: thisMac.busy, onSetUp: thisMac.setUp }}
          machines={machines}
          agentHome={agentHome}
          hasHome={agentHome && reusableHome(chosenAgent, spaces, false, children?.homesOf(chosenAgent)) !== null}
          busy={false}
          onSubmit={() => submit()}
          compact={!first}
          autoFocus
        />
        <AgentStatus state={agentHome ? readiness : "none"} onReconnect={() => selected && void signInTo(selected.group.app, selected.group.label)} />
        {/* Preparing is silent: only a failed prepare says so, with Try again. */}
        <PrepareLine view={prep.view} onRetry={onPrepareAction} />
        {/* After the first job: the rest of setup, once (CUA-3150, idea C). On a Mac with a permission missing, the apps row at every launch (CUA-3551). */}
        <BringTheRest />
        {first ? (
          <>
            <div className="grid gap-4 sm:grid-cols-3">
              {SUGGESTIONS.filter((s) => agentHome || kinds.some((k) => k.os === s.os) || s.os === "linux").map((s) => (
                <button
                  type="button"
                  key={s.title}
                  data-home-card=""
                  onClick={() => {
                    setText(s.prompt);
                    // Without an agent home the suggestion's kind of computer is made for it.
                    if (!agentHome) setWhere({ kind: "new", os: kinds.some((k) => k.os === s.os) ? s.os : "linux" });
                  }}
                  className="flex flex-col gap-2 rounded-xl border bg-card/60 px-5 py-4 text-left transition-colors hover:bg-accent dark:bg-white/[0.02]"
                >
                  <OsIcon os={s.os} className="size-4 text-muted-foreground" />
                  <span className="text-[14px] leading-snug font-medium">{s.title}</span>
                  {s.detail ? <span className="text-[13px] leading-snug text-muted-foreground">{s.detail}</span> : null}
                </button>
              ))}
            </div>
          </>
        ) : null}
        {/* The Agents page's own list, then the Spaces page's tiles. */}
        <YourAgents chats={chats} />
        <YourSpacesRow />
      </div>
      {pick.dialog}
    </div>
  );
}

/** True once `ms` have passed since the first render. */
function useElapsed(ms: number): boolean {
  const [done, setDone] = useState(false);
  useEffect(() => {
    const timer = setTimeout(() => setDone(true), ms);
    return () => clearTimeout(timer);
  }, [ms]);
  return done;
}
