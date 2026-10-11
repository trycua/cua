// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { BotIcon, KeyRoundIcon, PlugIcon } from "lucide-react";
import { useEffect, useState } from "react";

import {
  isUnsupported,
  useAgents,
  useAgentTimeline,
  useSession,
  type AgentRunSummary,
  type AgentsData,
  type AgentSummary,
  type AgentTimeline,
} from "@/bridge";
import { ConnectAgentDialog } from "@/components/agents/connect-dialog";
import { Timeline } from "@/components/agents/timeline";
import { EmptyState, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import {
  AGENT_STATE_LABEL,
  RUN_STATUS_LABEL,
  agentTone,
  monogram,
  resolveSelection,
  runTone,
  shortAge,
  spaceLabel,
  type AgentsSelection,
  type Tone,
} from "@/lib/agents";
import { needsAgentKey } from "@/lib/agent-keys";
import { relativeTime, cn } from "@/lib/utils";

export const Route = createFileRoute("/agents/")({
  component: AgentsPage,
  validateSearch: (s: Record<string, unknown>): AgentsSelection => ({
    agent: typeof s.agent === "string" ? s.agent : undefined,
    space: typeof s.space === "string" ? s.space : undefined,
    run: typeof s.run === "string" ? s.run : undefined,
  }),
});

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

function AgentsPage() {
  const search = Route.useSearch();
  const navigate = useNavigate({ from: "/agents/" });
  const { data, isLoading, error } = useAgents();
  const [connectOpen, setConnectOpen] = useState(false);
  const selected = resolveSelection(data, search);
  const select = (next: AgentsSelection) => void navigate({ search: next, replace: true });
  const empty = data !== undefined && data.agents.length === 0 && data.runs.length === 0;

  return (
    <div className="flex h-full flex-col">
      <div className="px-8 pt-6">
        <PageHeader
          title="Agents"
          description="Persistent agents and the runs in your Spaces."
          actions={
            <Button variant="outline" onClick={() => setConnectOpen(true)}>
              <PlugIcon /> Connect an agent
            </Button>
          }
        />
      </div>
      <div className="min-h-0 flex-1 px-8 pb-6">
        {error ? (
          <EmptyState
            icon={<BotIcon />}
            title={isUnsupported(error) ? "Agents aren't available here yet" : "Couldn't load agents"}
            action={
              <div className="flex gap-2">
                <Button variant="outline" onClick={() => setConnectOpen(true)}>
                  Connect an agent
                </Button>
                <AgentKeysLink />
              </div>
            }
          >
            {isUnsupported(error) ? "This version of the app can't list agents. The Cua Spaces app on macOS shows them in its own window." : error.message}
          </EmptyState>
        ) : isLoading && !data ? null : empty ? (
          <EmptyState
            icon={<BotIcon />}
            title="No agents yet"
            action={
              <div className="flex gap-2">
                <Button variant="outline" onClick={() => setConnectOpen(true)}>
                  <PlugIcon /> Connect an agent
                </Button>
                <AgentKeysLink />
              </div>
            }
          >
            Connect a coding agent to Cua, then ask it to work in a Space. Its runs and persistent agents show up here. Agents need a provider key to run: add one in Settings → Agents.
          </EmptyState>
        ) : data ? (
          <div className="flex h-full overflow-hidden rounded-xl border bg-card shadow-xs">
            <AgentList data={data} selected={selected} onSelect={select} />
            <div className="flex min-w-0 flex-1 flex-col border-l bg-background">
              {selected.ref || selected.agent ? (
                <AgentDetail
                  key={selected.ref ? `${selected.ref.spaceId}/${selected.ref.runId}` : selected.agent?.name}
                  agent={selected.agent}
                  run={selected.run}
                  runRef={selected.ref}
                />
              ) : null}
            </div>
          </div>
        ) : null}
      </div>
      <ConnectAgentDialog open={connectOpen} onOpenChange={setConnectOpen} />
    </div>
  );
}

/* ---- The list -------------------------------------------------------------- */

function AgentList({
  data,
  selected,
  onSelect,
}: {
  data: AgentsData;
  selected: ReturnType<typeof resolveSelection>;
  onSelect: (s: AgentsSelection) => void;
}) {
  const now = useNow();
  const isRunSelected = (r: AgentRunSummary) => !selected.agent && selected.ref?.spaceId === r.spaceId && selected.ref.runId === r.runId;
  return (
    <nav aria-label="Agents and runs" className="flex w-[320px] shrink-0 flex-col overflow-y-auto py-2">
      {data.canList ? (
        <Section title="Persistent agents" count={data.agents.length}>
          {data.agents.length === 0 ? (
            <p className="px-4 py-2 text-xs text-muted-foreground">None yet. Ask an agent to create one, or run cua agent create.</p>
          ) : (
            data.agents.map((a) => (
              <ListRow
                key={a.name}
                selected={selected.agent?.name === a.name}
                onClick={() => onSelect({ agent: a.name })}
                agent={a.name}
                mark={monogram(a.harnessName)}
                title={a.name}
                subtitle={a.detail}
                tone={agentTone(a.state)}
                state={AGENT_STATE_LABEL[a.state]}
                age={a.state === "running" ? "now" : shortAge(a.lastActivityMs, now)}
                ageTitle={a.lastActivityMs ? `Last saved ${relativeTime(a.lastActivityMs, now)}` : undefined}
              />
            ))
          )}
        </Section>
      ) : null}
      <Section title="Recent runs" count={data.runs.length}>
        {data.runs.length === 0 ? (
          <p className="px-4 py-2 text-xs text-muted-foreground">No runs in your running Spaces.</p>
        ) : (
          data.runs.map((r) => (
            <ListRow
              key={`${r.spaceId}/${r.runId}`}
              selected={isRunSelected(r)}
              onClick={() => onSelect({ space: r.spaceId, run: r.runId })}
              mark={monogram(r.harnessName)}
              title={r.summary || r.harnessName}
              subtitle={`${r.agentName ? `${r.agentName}, ` : ""}${r.harnessName} in ${spaceLabel(r.spaceName)}`}
              tone={runTone(r.status)}
              state={RUN_STATUS_LABEL[r.status]}
              age={shortAge(r.startedMs, now)}
              ageTitle={r.startedMs ? `Started ${relativeTime(r.startedMs, now)}` : undefined}
            />
          ))
        )}
      </Section>
      {data.unread.length ? (
        <div className="mt-auto space-y-1 border-t px-4 pt-3 pb-1">
          {data.unread.map((u) => (
            <p key={u.spaceId} className="text-2xs text-muted-foreground">
              Couldn't read runs in {spaceLabel(u.spaceName)}: {u.message}
            </p>
          ))}
        </div>
      ) : null}
    </nav>
  );
}

function Section({ title, count, children }: { title: string; count: number; children: React.ReactNode }) {
  return (
    <section className="mb-2">
      <h2 className="flex h-8 items-center gap-2 px-4 text-xs font-semibold text-muted-foreground">
        {title}
        <span className="font-normal tabular-nums text-muted-foreground/70">{count}</span>
      </h2>
      <ul className="space-y-px px-2">{children}</ul>
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

function ListRow(props: {
  /** A persistent agent's row (the parity harness reads it). */
  agent?: string;
  selected: boolean;
  onClick: () => void;
  mark: string;
  title: string;
  subtitle: string;
  tone: Tone;
  state: string;
  age: string;
  ageTitle?: string;
}) {
  return (
    <li>
      <button
        type="button"
        aria-current={props.selected ? "true" : undefined}
        data-agent-row={props.agent}
        onClick={props.onClick}
        className={cn(
          "flex w-full cursor-default items-start gap-2.5 rounded-lg px-2 py-2 text-left outline-none transition-colors hover:bg-foreground/[0.04] focus-visible:ring-2 focus-visible:ring-ring/60",
          props.selected && "bg-foreground/[0.06] hover:bg-foreground/[0.06] dark:bg-white/[0.07]",
        )}
      >
        <HarnessMark mark={props.mark} />
        <span className="min-w-0 flex-1">
          <span className="flex items-baseline gap-2">
            <span data-row-title className="min-w-0 flex-1 truncate text-[13px] font-medium">
              {props.title}
            </span>
            <span className="shrink-0 text-2xs tabular-nums text-muted-foreground" title={props.ageTitle}>
              {props.age}
            </span>
          </span>
          <span className="mt-0.5 flex items-center gap-2">
            <span data-row-detail className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
              {props.subtitle}
            </span>
            <span data-row-state className="inline-flex shrink-0 items-center gap-1.5 text-2xs text-muted-foreground">
              <StatusDot tone={props.tone} />
              {props.state}
            </span>
          </span>
        </span>
      </button>
    </li>
  );
}

function HarnessMark({ mark, large }: { mark: string; large?: boolean }) {
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center rounded-md border bg-background font-semibold text-muted-foreground",
        large ? "size-8 text-[13px]" : "mt-px size-6 text-2xs",
      )}
      aria-hidden
    >
      {mark}
    </span>
  );
}

/* ---- The detail ------------------------------------------------------------- */

function AgentDetail({ agent, run, runRef }: { agent: AgentSummary | null; run: AgentRunSummary | null; runRef: { spaceId: string; runId: string } | null }) {
  const { pauseAgent, resumeAgent } = useAgents();
  const { openExternal } = useSession();
  const timeline = useAgentTimeline(runRef);
  const now = useNow();
  const [busy, setBusy] = useState(false);

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
        <HarnessMark mark={monogram(harness)} large />
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
      <DetailBody agent={agent} run={run} hasRun={runRef !== null} timeline={timeline} onOpenLink={(url) => void openExternal(url)} />
    </>
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
          ? `Its home is saved and its routines are on hold. Resume it to let it work in ${spaceLabel(agent.spaceName)} again.`
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
