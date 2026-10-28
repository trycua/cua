// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { getRouteApi, Link, useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon, BotIcon, KeyRoundIcon, SettingsIcon } from "lucide-react";
import { useEffect, useState } from "react";

import {
  isUnsupported,
  useAgentDetail,
  useAgents,
  useAgentTimeline,
  useFeatureRecorded,
  useSession,
  type AgentRunSummary,
  type AgentsData,
  type AgentSummary,
  type AgentTab,
  type AgentTimeline,
} from "@/bridge";
import { AgentPanel } from "@/components/agents/agent-panels";
import { AgentsEmpty } from "@/legacy/components/agents/agents-empty";
import { HarnessMark } from "@/components/agents/harness-mark";
import { Timeline } from "@/components/agents/timeline";
import { EmptyState, PageHeader } from "@/legacy/components/page";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import {
  AGENT_STATE_LABEL,
  RUN_STATUS_LABEL,
  agentTone,
  isWaiting,
  resolveSelection,
  runTone,
  spaceLabel,
  type AgentsSelection,
  type Tone,
} from "@/lib/agents";
import { needsAgentKey } from "@/lib/agent-keys";
import { relativeTime, cn } from "@/lib/utils";

const Route = getRouteApi("/agents/");

/** Public's page before the redesign (`spaces_redesign` off); its search is the real route's. */
export const LegacyAgentsPage = AgentsPage;

/** Settings → Agents, where agents get their provider keys. */
function AgentKeysLink({ size }: { size?: "sm" }) {
  return (
    <Button variant="outline" size={size} render={<Link to="/settings/agents" />} data-agent-keys-link>
      <KeyRoundIcon /> Add a provider key
    </Button>
  );
}

/** Re-renders every `ms` so ages stay current. */
function useNow(ms = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

/** Settings, Connected apps: where Cua is switched on for each coding app. */
function ConnectedAppsLink() {
  const navigate = useNavigate();
  return (
    <Button variant="outline" onClick={() => void navigate({ to: "/settings/connectors", search: {} })} data-connected-apps-link>
      <SettingsIcon /> Connected apps
    </Button>
  );
}

function AgentsPage() {
  const search = Route.useSearch();
  const navigate = useNavigate({ from: "/agents/" });
  const { data, isLoading, error } = useAgents();
  useFeatureRecorded("agents_page_open");
  // The page opens on the overview; a run or an agent opens its own view.
  const opened = Boolean(search.agent || (search.run && search.space));
  const selected = resolveSelection(data, search);
  const select = (next: AgentsSelection) => void navigate({ search: next, replace: false });
  const empty = data !== undefined && data.agents.length === 0 && data.runs.length === 0;
  const detail = opened && (selected.ref || selected.agent);

  return (
    <div className="h-full overflow-y-auto [scrollbar-gutter:stable]">
      <div className="mx-auto flex min-h-full w-full max-w-5xl flex-col px-8 pt-8 pb-16">
        {detail ? (
          <div className="flex min-h-0 flex-1 flex-col" data-agent-view>
            <Button variant="ghost" size="sm" className="mb-4 -ml-2 self-start" onClick={() => select({})} data-agents-back>
              <ArrowLeftIcon /> Agents
            </Button>
            <div className="flex min-h-[28rem] flex-1 flex-col overflow-hidden rounded-xl border bg-card shadow-xs">
              <AgentDetail
                key={selected.ref ? `${selected.ref.spaceId}/${selected.ref.runId}` : selected.agent?.name}
                agent={selected.agent}
                run={selected.run}
                runRef={selected.ref}
              />
            </div>
          </div>
        ) : (
          <>
            <PageHeader title="Agents" description="What your agents have done in your Spaces." actions={<ConnectedAppsLink />} />
            {error ? (
              <EmptyState
                icon={<BotIcon />}
                title={isUnsupported(error) ? "Agents aren't available here yet" : "Couldn't load agents"}
                action={<AgentKeysLink />}
              >
                {isUnsupported(error) ? "This version of the app can't list agents. The Cua Spaces app on macOS shows them in its own window." : error.message}
              </EmptyState>
            ) : isLoading && !data ? null : empty ? (
              <AgentsEmpty />
            ) : data ? (
              <Overview data={data} onSelect={select} />
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}

/* ---- The overview ---------------------------------------------------------- */

function Overview({ data, onSelect }: { data: AgentsData; onSelect: (s: AgentsSelection) => void }) {
  const now = useNow();
  const navigate = useNavigate();
  const waiting = data.runs.filter(isWaiting);
  const openSpace = (spaceId: string) => void navigate({ to: "/spaces/$spaceId", params: { spaceId } });
  return (
    <div className="flex flex-col gap-8">
      {waiting.length > 0 ? (
        <Block title="Waiting for you" count={waiting.length} id="waiting">
          {waiting.map((r) => (
            <div
              key={`${r.spaceId}/${r.runId}`}
              data-waiting-run={r.runId}
              className="flex min-h-12 items-center gap-3 px-4 py-3"
            >
              <HarnessMark agent={r.agent} name={r.harnessName} />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[14px] font-medium">{r.needsYou!.title}</div>
                <div className="truncate text-[13px] text-muted-foreground">{r.needsYou!.detail}</div>
              </div>
              <Button onClick={() => openSpace(r.spaceId)} data-waiting-open>
                Open Space
              </Button>
            </div>
          ))}
        </Block>
      ) : null}

      {data.canList && data.agents.length > 0 ? (
        <Block title="Running" count={data.agents.length} id="running">
          {data.agents.map((a) => (
            <button
              key={a.name}
              type="button"
              data-agent-row={a.name}
              onClick={() => onSelect({ agent: a.name })}
              className="flex min-h-12 w-full cursor-default items-center gap-3 px-4 py-3 text-left outline-none transition-colors hover:bg-foreground/[0.04] focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-inset"
            >
              <HarnessMark agent={a.harness} name={a.harnessName} />
              <span className="min-w-0 flex-1">
                <span data-row-title className="block truncate text-[14px] font-medium">
                  {a.name}
                </span>
                <span data-row-detail className="block truncate text-[13px] text-muted-foreground">
                  {a.detail}
                </span>
              </span>
              <span data-row-state className="inline-flex shrink-0 items-center gap-1.5 text-[13px] text-muted-foreground">
                <StatusDot tone={agentTone(a.state)} />
                {AGENT_STATE_LABEL[a.state]}
              </span>
              <span className="w-24 shrink-0 text-right text-[13px] text-muted-foreground" title={a.lastActivityMs ? `Last saved ${relativeTime(a.lastActivityMs, now)}` : undefined}>
                {a.state === "running" ? "Now" : a.lastActivityMs ? relativeTime(a.lastActivityMs, now) : ""}
              </span>
            </button>
          ))}
        </Block>
      ) : null}

      <Block title="Recent runs" count={data.runs.length} id="runs" empty={data.runs.length === 0 ? "No runs yet in your running Spaces." : null}>
        {data.runs.length > 0 ? (
          <div role="table" aria-label="Recent runs">
            <div role="row" className="grid min-h-12 grid-cols-[minmax(0,3fr)_minmax(0,2fr)_8rem_7rem_6rem] items-center gap-4 px-4 text-[13px] text-muted-foreground">
              <span role="columnheader">What it did</span>
              <span role="columnheader">Space</span>
              <span role="columnheader">Status</span>
              <span role="columnheader">Started</span>
              <span role="columnheader" />
            </div>
            {data.runs.map((r) => {
              const live = r.status === "running";
              const needs = isWaiting(r);
              return (
                <div
                  key={`${r.spaceId}/${r.runId}`}
                  role="row"
                  data-run-row={r.runId}
                  className="grid min-h-12 grid-cols-[minmax(0,3fr)_minmax(0,2fr)_8rem_7rem_6rem] items-center gap-4 border-t px-4 py-2"
                >
                  <div role="cell" className="flex min-w-0 items-center gap-3">
                    <HarnessMark agent={r.agent} name={r.harnessName} />
                    <span className="min-w-0">
                      <span data-row-title className="block truncate text-[14px] font-medium">
                        {r.summary || r.harnessName}
                      </span>
                      <span data-row-detail className="block truncate text-[13px] text-muted-foreground">
                        {r.agentName ? `${r.agentName}, ${r.harnessName}` : r.harnessName}
                      </span>
                    </span>
                  </div>
                  <div role="cell" className="min-w-0 truncate text-[13px]">
                    {spaceLabel(r.spaceName)}
                  </div>
                  <div role="cell" data-row-state className="inline-flex items-center gap-1.5 text-[13px]">
                    <StatusDot tone={needs ? "paused" : runTone(r.status)} />
                    {needs ? "Needs you" : RUN_STATUS_LABEL[r.status]}
                  </div>
                  <div role="cell" className="text-[13px] text-muted-foreground" title={r.startedMs ? relativeTime(r.startedMs, now) : undefined}>
                    {live ? "Now" : r.startedMs ? relativeTime(r.startedMs, now) : ""}
                  </div>
                  <div role="cell" className="flex justify-end">
                    {needs ? (
                      <Button size="sm" onClick={() => openSpace(r.spaceId)}>
                        Open Space
                      </Button>
                    ) : (
                      <Button size="sm" variant="outline" data-run-open onClick={() => onSelect({ space: r.spaceId, run: r.runId })}>
                        {live ? "Watch" : "Replay"}
                      </Button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        ) : null}
      </Block>
      {data.unread.length ? (
        <div className="space-y-1">
          {data.unread.map((u) => (
            <p key={u.spaceId} className="text-xs text-muted-foreground">
              Couldn't read runs in {spaceLabel(u.spaceName)}: {u.message}
            </p>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** A titled card: 32px between blocks, rows of at least 48px inside. */
function Block({ title, count, id, children, empty }: { title: string; count: number; id: string; children?: React.ReactNode; empty?: string | null }) {
  return (
    <section data-agents-section={id}>
      <h2 className="mb-3 flex items-center gap-2 text-[15px] font-semibold">
        {title}
        <span className="text-[13px] font-normal tabular-nums text-muted-foreground">{count}</span>
      </h2>
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
        {empty ? <p className="px-4 py-3 text-[13px] text-muted-foreground">{empty}</p> : children}
      </div>
    </section>
  );
}

const TONE_DOT: Record<Tone, string> = {
  live: "bg-success",
  quiet: "bg-muted-foreground/40",
  paused: "bg-warning",
  problem: "bg-destructive",
};

function StatusDot({ tone, className }: { tone: Tone; className?: string }) {
  return (
    <span className={cn("relative inline-flex size-1.5 shrink-0 rounded-full", TONE_DOT[tone], className)} aria-hidden>
      {tone === "live" ? <span className="absolute inset-0 animate-ping rounded-full bg-success/60" /> : null}
    </span>
  );
}

/* ---- The detail ------------------------------------------------------------- */

function AgentDetail({ agent, run, runRef }: { agent: AgentSummary | null; run: AgentRunSummary | null; runRef: { spaceId: string; runId: string } | null }) {
  const { pauseAgent, resumeAgent } = useAgents();
  const { openExternal } = useSession();
  const timeline = useAgentTimeline(runRef);
  const detail = useAgentDetail(agent?.record ?? null);
  const now = useNow();
  const [busy, setBusy] = useState(false);
  // The run's conversation, or one of the core's tabs (Memory, Routines, Access).
  const [pane, setPane] = useState<"run" | AgentTab>("run");

  const harness = agent?.harnessName ?? run?.harnessName ?? "";
  const space = spaceLabel(agent?.spaceName ?? run?.spaceName ?? "");
  const status = timeline.status ?? run?.status ?? null;
  const title = agent?.name ?? run?.summary ?? harness;
  const turn = Math.max(run?.turn ?? 0, timeline.items[timeline.items.length - 1]?.turn ?? 0);
  const subtitle = [
    `${harness} in ${space}`,
    agent && run?.summary ? run.summary : null,
    turn ? `Turn ${turn}` : null,
    run?.startedMs ? `started ${relativeTime(run.startedMs, now)}` : null,
  ]
    .filter(Boolean)
    .join(" · ");

  const act = () => {
    if (!agent) return;
    setBusy(true);
    (agent.state === "paused" ? resumeAgent(agent.name) : pauseAgent(agent.name))
      .catch((e: unknown) => toast(`Couldn't ${agent.actionLabel.toLowerCase()} ${agent.name}`, { description: e instanceof Error ? e.message : String(e) }))
      .finally(() => setBusy(false));
  };

  return (
    <>
      <header className="flex items-center gap-3 border-b bg-card px-5 py-3">
        <HarnessMark agent={agent?.harness ?? run?.agent ?? ""} name={harness} large />
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-[14px] font-semibold">{title}</h2>
          <p className="truncate text-xs text-muted-foreground" title={subtitle}>
            {subtitle}
          </p>
        </div>
        {agent ? (
          <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
            <StatusDot tone={agentTone(agent.state)} />
            {AGENT_STATE_LABEL[agent.state]}
          </span>
        ) : status ? (
          <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
            <StatusDot tone={runTone(status)} />
            {RUN_STATUS_LABEL[status]}
          </span>
        ) : null}
        {agent ? (
          <Button size="sm" variant="outline" disabled={busy} onClick={act}>
            {busy ? (agent.state === "paused" ? "Resuming…" : "Pausing…") : agent.actionLabel}
          </Button>
        ) : null}
      </header>
      {agent?.lastError ? (
        <div className="flex items-center gap-3 border-b bg-destructive/5 px-5 py-2" data-agent-last-error>
          <p className="min-w-0 flex-1 text-xs text-destructive">{agent.lastError}</p>
          {needsAgentKey(agent.lastError) ? <AgentKeysLink size="sm" /> : null}
        </div>
      ) : null}
      {detail.view ? (
        <AgentTabs
          tabs={[{ tab: "run", label: "Conversation" }, ...detail.view.tabs]}
          selected={pane}
          onSelect={(tab) => {
            setPane(tab);
            if (tab !== "run") detail.act({ type: "set-tab", tab });
          }}
        />
      ) : null}
      {pane !== "run" && detail.view ? (
        <AgentPanel tab={pane} detail={detail} />
      ) : (
        <DetailBody agent={agent} run={run} hasRun={runRef !== null} timeline={timeline} onOpenLink={(url) => void openExternal(url)} />
      )}
    </>
  );
}

function AgentTabs({
  tabs,
  selected,
  onSelect,
}: {
  tabs: { tab: "run" | AgentTab; label: string }[];
  selected: "run" | AgentTab;
  onSelect: (tab: "run" | AgentTab) => void;
}) {
  return (
    <div role="tablist" aria-label="Agent" className="flex shrink-0 gap-1 border-b bg-card px-4">
      {tabs.map((t) => (
        <button
          key={t.tab}
          type="button"
          role="tab"
          aria-selected={selected === t.tab}
          onClick={() => onSelect(t.tab)}
          data-agent-tab={t.tab}
          className={cn(
            "-mb-px cursor-default border-b-2 border-transparent px-2 py-2 text-xs font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:text-foreground",
            selected === t.tab && "border-foreground text-foreground",
          )}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

function DetailBody({
  agent,
  run,
  hasRun,
  timeline,
  onOpenLink,
}: {
  agent: AgentSummary | null;
  run: AgentRunSummary | null;
  hasRun: boolean;
  timeline: AgentTimeline;
  onOpenLink: (url: string) => void;
}) {
  if (!hasRun) {
    return (
      <Centered title={agent?.state === "paused" ? `${agent.name} is paused` : "No runs yet"}>
        {agent?.state === "paused"
          ? `Its home is saved. Resume it to let it work in ${spaceLabel(agent.spaceName)} again.`
          : `Runs show up here once ${agent?.name ?? "this agent"} starts working.`}
      </Centered>
    );
  }
  if (timeline.unsupported) {
    return (
      <Centered title="The conversation isn't available here">
        This version of the app can list runs but can't read what they wrote. {run?.reason ? `The run is ${run.status}: ${run.reason}.` : null}
      </Centered>
    );
  }
  if (timeline.error && timeline.items.length === 0) {
    return <Centered title="Couldn't read this run">{timeline.error.message}</Centered>;
  }
  if (timeline.isLoading && timeline.items.length === 0) return <div className="flex-1" />;
  return <Timeline className="flex-1" timeline={timeline} onOpenLink={onOpenLink} footer={<RunFooter timeline={timeline} run={run} />} />;
}

function RunFooter({ timeline, run }: { timeline: AgentTimeline; run: AgentRunSummary | null }) {
  const status = timeline.status ?? run?.status;
  if (status === "running") {
    const doing = timeline.phase === "responding" ? "Writing" : timeline.phase === "tool" ? "Using tools" : "Working";
    return (
      <p className="flex items-center gap-2 text-xs text-muted-foreground" role="status">
        <StatusDot tone="live" />
        {doing}
      </p>
    );
  }
  if (status === "failed" || status === "crashed") {
    return (
      <p className="text-xs text-destructive" role="status">
        {RUN_STATUS_LABEL[status]}
        {run?.reason ? `: ${run.reason}` : null}
      </p>
    );
  }
  if (status === "idle") {
    return <p className="text-xs text-muted-foreground">Waiting for a follow-up.</p>;
  }
  return null;
}

function Centered({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
      <h3 className="text-[14px] font-semibold">{title}</h3>
      <p className="mt-1.5 max-w-sm text-[13px] text-muted-foreground">{children}</p>
    </div>
  );
}
