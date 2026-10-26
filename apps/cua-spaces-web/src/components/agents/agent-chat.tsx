// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Agents tab as a chat (CUA-2266, agents-tab storyboard pp. 15–25).
 * Left: Needs you pinned, then every agent with its live status line,
 * owner badge and unread count. Middle: the DM, the part of the agent's
 * Space thread(s) between you and it; sending is an @mention task in that
 * Space, and the composer says which Space and that members see it.
 */

import { Link, useNavigate } from "@tanstack/react-router";
import { ArrowUpRightIcon, EyeIcon, InfoIcon, LinkIcon, LockIcon, MonitorPlayIcon, PlayIcon, PlusIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { useAgentDm, useAgents, useBridge, type AgentChat, type AgentChatSpace, type AgentSetupRow, type ChatNeed, type DmItem, type ThreadAttachment, type ThreadEntry } from "@/bridge";
import { AddAgentSheet, type AddAgentStage } from "@/components/add-agent/add-agent-sheet";
import { HarnessMark as AppMark } from "@/components/agents/harness-mark";
import { OsIcon } from "@/components/os-icon";
import { AgentTile } from "@/components/space-thread/avatar";
import { Composer } from "@/components/space-thread/composer";
import { AiTag, holdersName, ProvenanceCard } from "@/components/space-thread/identity";
import { HARNESSES } from "@/components/space-thread/harness-marks";
import { buildRoster, type Roster } from "@/components/space-thread/roster";
import { ThreadItemView } from "@/components/space-thread/thread-view";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

const TONE: Record<AgentChat["tone"], string> = {
  you: "bg-warning",
  working: "bg-success",
  failed: "bg-destructive",
  idle: "bg-muted-foreground/40",
  asleep: "bg-muted-foreground/40",
};

/** The owner as the list says it: nothing for your own agents, "Grace's" for a teammate's. */
export function ownerWord(chat: AgentChat, me: string | null): string | null {
  const o = chat.identity?.owner;
  if (!o) return null;
  return me && o.id === me ? null : `${o.name}'s`;
}

const ago = (ms: number, now: number) => {
  if (!ms) return "";
  const m = Math.max(0, Math.round((now - ms) / 60_000));
  if (m < 60) return `${Math.max(1, m)}m`;
  const h = Math.round(m / 60);
  return h < 24 ? `${h}h` : `${Math.round(h / 24)}d`;
};

/* ------------------------------------------------------------------ list */

/** "@ada which branch…" reads "Which branch…" in Needs you: the mention is you. */
export function needText(t: string): string {
  const s = t.replace(/^(\s*@[\w.-]+[,:]?\s+)+/, "").replace(/`/g, "");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function AgentList({
  chats,
  me,
  selected,
  onSelect,
  onNew,
  now,
}: {
  chats: AgentChat[];
  me: string | null;
  selected: string | null;
  onSelect: (key: string, space?: string) => void;
  onNew: () => void;
  now: number;
}) {
  const needs = chats.flatMap((c) => c.needsYou.map((n) => ({ n, c }))).sort((a, b) => b.n.tsMs - a.n.tsMs);
  const running = chats.filter((c) => c.tone === "you" || c.tone === "working");
  const others = chats.filter((c) => c.tone !== "you" && c.tone !== "working");
  return (
    <aside data-agent-list="" className="flex w-72 shrink-0 flex-col bg-chrome">
      <div className="flex h-14 items-center justify-between pr-3 pl-5">
        <h1 className="text-[17px] font-semibold">Agents</h1>
        <Button size="sm" variant="outline" data-new-agent="" onClick={onNew}>
          <PlusIcon className="size-3.5" />
          New agent
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        {chats.length === 0 ? <p className="px-2 text-xs text-muted-foreground">No agents yet</p> : null}
        {needs.length ? (
          <Section title="Needs you" count={needs.length}>
            {needs.map(({ n, c }) => (
              <button
                key={`${n.spaceId}#${n.seq}`}
                type="button"
                data-needs-you={n.kind}
                onClick={() => onSelect(c.key, c.spaces.length > 1 ? n.spaceId : undefined)}
                className="flex w-full cursor-default items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-accent focus-visible:bg-accent"
              >
                <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-warning" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px]">{needText(n.text)}</span>
                  <span className="block truncate text-[11px] text-muted-foreground">{c.label}</span>
                </span>
                <span className="shrink-0 text-[11px] text-muted-foreground tabular-nums">{ago(n.tsMs, now)}</span>
              </button>
            ))}
          </Section>
        ) : null}
        {running.length ? (
          <Section title="Running" count={running.length}>
            {running.map((c) => (
              <AgentRow key={c.key} chat={c} me={me} selected={selected === c.key} onSelect={() => onSelect(c.key)} now={now} />
            ))}
          </Section>
        ) : null}
        {others.length ? (
          <Section title={running.length ? "Others" : "Agents"} count={others.length}>
            {others.map((c) => (
              <AgentRow key={c.key} chat={c} me={me} selected={selected === c.key} onSelect={() => onSelect(c.key)} now={now} />
            ))}
          </Section>
        ) : null}
      </div>
      <div className="shrink-0 px-5 pb-3">
        <Link to="/agents/all" data-agents-show-all="" className="text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:underline">
          All agents and runs
        </Link>
      </div>
    </aside>
  );
}

function Section({ title, count, children }: { title: string; count: number; children: ReactNode }) {
  return (
    <section className="mt-2 first:mt-0" data-agent-section={title}>
      <h2 className="flex items-center gap-2 px-2 pt-2 pb-1 text-xs font-medium text-muted-foreground">
        {title} <span className="tabular-nums">{count}</span>
      </h2>
      {children}
    </section>
  );
}

function AgentRow({ chat: c, me, selected, onSelect, now }: { chat: AgentChat; me: string | null; selected: boolean; onSelect: () => void; now: number }) {
  const owner = ownerWord(c, me);
  return (
    <button
      type="button"
      data-chat={c.key}
      data-chat-tone={c.tone}
      aria-current={selected ? "true" : undefined}
      onClick={onSelect}
      className={cn("flex w-full cursor-default items-center gap-2.5 rounded-lg px-2 py-2 text-left outline-none hover:bg-accent focus-visible:bg-accent", selected && "bg-accent")}
    >
      <AgentTile harness={c.harness} icon={c.identity?.icon} owner={c.identity?.owner} name={c.label} px={28} dim={c.tone === "asleep"} surface="chrome" />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="truncate text-[13px] font-semibold">{c.label}</span>
          <AiTag />
          {owner ? <span className="shrink-0 truncate text-[11.5px] text-muted-foreground">{owner}</span> : null}
          <span className="ml-auto shrink-0 text-[11px] text-muted-foreground tabular-nums">{ago(c.lastMs, now)}</span>
        </span>
        <span className="flex items-center gap-1.5">
          <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", TONE[c.tone])} />
          <span data-chat-line="" className="min-w-0 flex-1 truncate text-[11.5px] text-muted-foreground">
            {c.line}
          </span>
          {c.unread ? (
            <span data-chat-unread={c.unread} className="inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-brand px-1 text-[10px] font-semibold text-white tabular-nums">
              {c.unread}
            </span>
          ) : null}
        </span>
      </span>
    </button>
  );
}

/* -------------------------------------------------------------------- DM */

/** The roster of one Space's DM (its authors and @handles). */
function useRosters(chat: AgentChat, entries: Record<string, ThreadEntry[]>, me: string | null, identity: string | null) {
  return useMemo(() => {
    const out: Record<string, Roster> = {};
    for (const s of chat.spaces)
      out[s.spaceId] = buildRoster({
        members: [s.member],
        shares: [],
        entries: entries[s.spaceId] ?? [],
        me,
        identity,
        role: s.role,
        harnessName: (h) => HARNESSES.find((x) => x.id === h)?.name ?? h,
      });
    return out;
  }, [chat, entries, me, identity]);
}

export function AgentDmView({ chat, space, onSpace, identity, onInfo, me }: { chat: AgentChat; space: string | null; onSpace: (space: string | null) => void; identity: string | null; onInfo: () => void; me: string | null }) {
  const { data } = useBridge();
  const navigate = useNavigate();
  const dm = useAgentDm(chat, space);
  const rosters = useRosters(chat, dm.entries, dm.me ?? me, identity);
  const shown = space ? chat.spaces.filter((s) => s.spaceId === space) : chat.spaces;
  // Where a message goes: the Space shown, or where it waits for you, or where it works, or its first.
  const [target, setTarget] = useState<string>(() => space ?? chat.needsYou[0]?.spaceId ?? chat.spaces.find((s) => s.member.status === "working")?.spaceId ?? chat.spaces[0]!.spaceId);
  useEffect(() => {
    if (space) setTarget(space);
    else if (!chat.spaces.some((s) => s.spaceId === target)) setTarget(chat.spaces[0]!.spaceId);
  }, [space, chat, target]);
  const to = chat.spaces.find((s) => s.spaceId === target) ?? chat.spaces[0]!;
  const working = shown.find((s) => s.member.status === "working" || s.member.status === "claimed");
  const owner = ownerWord(chat, dm.me ?? me);
  const openSpace = (id: string) => void navigate({ to: "/spaces/$spaceId", params: { spaceId: id } });
  const openAttachment = (a: ThreadAttachment) =>
    void data?.call("volume.reveal", { path: a.path }).catch((e: unknown) => toast(`Couldn't show ${a.name}`, { description: e instanceof Error ? e.message : String(e), type: "error" }));
  const multi = chat.spaces.length > 1;
  const ref = useRef<HTMLDivElement | null>(null);
  const lastKey = dm.items.length ? `${dm.items[dm.items.length - 1]!.spaceId}${dm.items[dm.items.length - 1]!.item.key}` : "";
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lastKey, chat.key, space]);
  const asleepHere = space && shown[0]?.member.status === "asleep";
  return (
    <section data-dm={chat.key} data-dm-space={space ?? "all"} className="flex min-w-0 flex-1 flex-col">
      <header className="flex h-16 shrink-0 items-center gap-3 px-6">
        <AgentTile harness={chat.harness} icon={chat.identity?.icon} owner={chat.identity?.owner} name={chat.label} px={32} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="truncate text-[15px] font-semibold">{chat.label}</span>
            <AiTag />
            <span className="text-[12px] text-muted-foreground">{owner ?? "Yours"}</span>
          </div>
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <span aria-hidden className={cn("size-1.5 rounded-full", TONE[chat.tone])} />
            <span className="truncate">{chat.line}</span>
          </div>
        </div>
        {multi ? (
          <Segmented<string>
            aria-label="Space"
            value={space ?? "all"}
            options={[{ value: "all", label: "All" }, ...chat.spaces.map((s) => ({ value: s.spaceId, label: s.spaceName }))]}
            onValueChange={(v) => onSpace(v === "all" ? null : v)}
          />
        ) : (
          <Button size="sm" variant="outline" data-dm-open-space="" onClick={() => openSpace(chat.spaces[0]!.spaceId)}>
            {chat.spaces[0]!.spaceName}
            <ArrowUpRightIcon className="size-3.5" />
          </Button>
        )}
        <button
          type="button"
          aria-label={`About ${chat.label}`}
          data-dm-info=""
          onClick={onInfo}
          className="inline-flex size-7 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <InfoIcon className="size-4" />
        </button>
      </header>
      <div ref={ref} data-dm-log="" role="log" aria-label={`Messages with ${chat.label}`} className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex min-h-full flex-col justify-end gap-1 px-6 pt-4 pb-3">
          {dm.status === "loading" && !dm.items.length ? null : dm.items.length === 0 ? (
            <p data-dm-empty="" className="text-center text-xs text-muted-foreground">
              Nothing between you and {chat.label} yet. A message here is a task for it in {to.spaceName}.
            </p>
          ) : (
            <>
              {asleepHere ? <p className="my-1 text-center text-xs text-muted-foreground">{chat.label} is asleep here. A message wakes it.</p> : null}
              {dm.items.map((d) => (
                <DmRow key={`${d.spaceId}${d.item.key}`} d={d} roster={rosters[d.spaceId]!} spaces={chat.spaces} multi={multi && !space} onAnswer={(seq, ok) => dm.answer(d.spaceId, seq, ok)} onOpenAttachment={openAttachment} />
              ))}
            </>
          )}
          {dm.error ? <p className="text-center text-xs text-destructive">Couldn't read the messages: {dm.error}</p> : null}
        </div>
      </div>
      <div className="shrink-0 px-6 pb-4">
        {working ? (
          <div data-dm-working="" className="mb-2 flex items-center gap-2 px-1 text-xs">
            <span aria-hidden className="size-1.5 rounded-full bg-success" />
            <span className="min-w-0 flex-1 truncate">
              {needText(working.member.activity ?? "Working")} <span className="text-muted-foreground">in {working.spaceName}</span>
            </span>
            <Button size="sm" variant="ghost" data-dm-watch="" onClick={() => openSpace(working.spaceId)}>
              <MonitorPlayIcon className="size-3.5" />
              Watch
            </Button>
          </div>
        ) : null}
        <DmComposer chat={chat} to={to} onTarget={setTarget} onSend={(text) => dm.send(to.spaceId, text)} onOpenSpace={() => openSpace(to.spaceId)} rosters={rosters} spaces={space ? shown : chat.spaces} />
      </div>
    </section>
  );
}

function DmRow({ d, roster, spaces, multi, onAnswer, onOpenAttachment }: { d: DmItem; roster: Roster; spaces: AgentChatSpace[]; multi: boolean; onAnswer: (seq: number, ok: boolean) => Promise<void>; onOpenAttachment: (a: ThreadAttachment) => void }) {
  const s = spaces.find((x) => x.spaceId === d.spaceId);
  // Every message says its Space when the DM shows several.
  const label = multi && s ? <SpaceLabel name={s.spaceName} os={s.os} /> : undefined;
  return <ThreadItemView item={d.item} roster={roster} onAnswer={onAnswer} onOpenAttachment={onOpenAttachment} label={label} />;
}

function SpaceLabel({ name, os }: { name: string; os: AgentChatSpace["os"] }) {
  return (
    <span data-dm-space-label={name} className="inline-flex items-center gap-1 text-[11px] text-muted-foreground">
      <OsIcon os={os} className="size-3" />
      {name}
    </span>
  );
}

/** Sending is posting: an @mention task for the agent in the Space chosen here. Members of that Space see it. */
function DmComposer({
  chat,
  to,
  spaces,
  onTarget,
  onSend,
  onOpenSpace,
  rosters,
}: {
  chat: AgentChat;
  to: AgentChatSpace;
  spaces: AgentChatSpace[];
  onTarget: (space: string) => void;
  onSend: (text: string) => Promise<void>;
  onOpenSpace: () => void;
  rosters: Record<string, Roster>;
}) {
  const youCan = to.member.youCan;
  const guest = to.role === "guest";
  if (guest || (youCan && !youCan.task)) {
    const who = guest ? null : holdersName(to.member.rights?.task ?? "members", to.member.identity, rosters[to.spaceId] ?? { people: [] });
    return (
      <div data-dm-locked="" className="flex items-center gap-2.5 rounded-lg bg-muted py-1.5 pr-1.5 pl-3 text-[13px] text-muted-foreground">
        <LockIcon className="size-3.5 shrink-0" />
        <span className="min-w-0 flex-1 truncate">{guest ? `You can read ${to.spaceName} but not post there.` : `Only ${who === "Owner" ? "the Space's owner" : who} can give ${chat.label} tasks.`}</span>
        <Button size="sm" variant="ghost" data-dm-open-space="" onClick={onOpenSpace}>
          Open {to.spaceName}
        </Button>
      </div>
    );
  }
  return (
    <Composer
      placeholder={spaces.length > 1 ? `Message ${chat.label}` : `Message ${chat.label} in ${to.spaceName}`}
      candidates={[]}
      handles={new Set([to.handle])}
      agentHarness={() => chat.harness}
      onSend={onSend}
      footer={
        <span className="flex items-center gap-2 text-xs text-muted-foreground">
          <span className="inline-flex items-center gap-1.5">
            In
            {spaces.length > 1 ? (
              <Select<string>
                aria-label="Send in"
                value={to.spaceId}
                options={spaces.map((s) => ({ value: s.spaceId, label: s.spaceName }))}
                onValueChange={onTarget}
                className="h-6 min-w-0 border-0 bg-background/60 px-2 text-xs shadow-none dark:bg-background/40"
              />
            ) : (
              <span data-dm-target={to.spaceId} className="inline-flex h-6 items-center gap-1 rounded-lg bg-background/60 px-2 text-foreground dark:bg-background/40">
                <OsIcon os={to.os} className="size-3" />
                {to.spaceName}
              </span>
            )}
          </span>
          <span className="inline-flex items-center gap-1" data-dm-members-see="">
            <EyeIcon className="size-3.5" />
            Members see it
          </span>
        </span>
      }
    />
  );
}

/* --------------------------------------------------------------- empty */

/** No agents yet: one question, three ways in (p. 23). */
export function AgentsEmpty({ onStart, onConnect }: { onStart: () => void; onConnect: () => void }) {
  const { agentSetup } = useAgents();
  const navigate = useNavigate();
  const [found, setFound] = useState<AgentSetupRow[] | null>(null);
  useEffect(() => {
    let live = true;
    agentSetup().then(
      (r) => live && setFound(r.filter((x) => x.installed && !x.configured)),
      () => live && setFound([]),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Connecting an app is its switch in Settings, Connectors: this opens it at that app.
  const connect = (agent: string) => void navigate({ to: "/settings/connectors", search: { app: agent } });
  return (
    <div data-agents-empty="" className="flex min-w-0 flex-1 items-center justify-center">
      <div className="w-[min(520px,100%)] px-6">
        <h2 className="text-center text-[17px] font-semibold">Who should work for you?</h2>
        <p className="mt-1 text-center text-[13px] text-muted-foreground">Add an agent, then message it here.</p>
        <div className="mt-5">
          {(found ?? []).slice(0, 3).map((f) => (
            <EmptyRow key={f.agent} data={`connect-mac-${f.agent}`} icon={<AppMark agent={f.agent} name={f.name} />} title={`Connect ${f.name}`} detail="On This Mac" onClick={() => connect(f.agent)} />
          ))}
          <EmptyRow data="start" icon={<PlayIcon className="size-3.5" />} title="Start one in a Space" detail="Runs inside the Space" onClick={onStart} />
          <EmptyRow data="connect" icon={<LinkIcon className="size-3.5" />} title="Connect your own agent" detail="With a prompt" onClick={onConnect} />
        </div>
      </div>
    </div>
  );
}

function EmptyRow({ icon, title, detail, onClick, data }: { icon: ReactNode; title: string; detail: string; onClick: () => void; data: string }) {
  return (
    <button type="button" data-agents-empty-row={data} onClick={onClick} className="flex h-11 w-full cursor-default items-center gap-3 rounded-lg px-3 text-left outline-none hover:bg-accent focus-visible:bg-accent">
      <span className="flex w-14 shrink-0 items-center justify-end text-muted-foreground">{icon}</span>
      <span className="w-52 shrink-0 text-[13px]">{title}</span>
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{detail}</span>
    </button>
  );
}

/* ---------------------------------------------------------- agent page */

/** ⓘ: who the agent is in each of its Spaces, what it may do, and where to go for more. */
export function AgentPage({ chat, open, onOpenChange, me, identity }: { chat: AgentChat; open: boolean; onOpenChange: (open: boolean) => void; me: string | null; identity: string | null }) {
  const navigate = useNavigate();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup data-agent-page={chat.key} className="top-[10vh] flex max-h-[80vh] w-[min(420px,calc(100vw-2rem))] flex-col bg-popover">
        <DialogTitle className="sr-only">About {chat.label}</DialogTitle>
        <div className="min-h-0 overflow-y-auto">
          {chat.spaces.map((s) => {
            const roster = buildRoster({ members: [s.member], shares: [], entries: [], me, identity, role: s.role, harnessName: (h) => h });
            return (
              <div key={s.spaceId} data-agent-page-space={s.spaceId}>
                {chat.spaces.length > 1 ? (
                  <p className="px-3.5 pt-3 text-xs text-muted-foreground">
                    <SpaceLabel name={s.spaceName} os={s.os} />
                  </p>
                ) : null}
                {s.member.identity ? (
                  <ProvenanceCard identity={s.member.identity} agent={roster.agents[0]} roster={roster} />
                ) : (
                  <p className="p-3.5 text-[13px]">
                    @{s.handle} in {s.spaceName}
                  </p>
                )}
              </div>
            );
          })}
        </div>
        <div className="flex shrink-0 items-center justify-end gap-2 px-3.5 pt-1 pb-3.5">
          <Button
            size="sm"
            variant="outline"
            data-agent-page-all=""
            onClick={() => {
              onOpenChange(false);
              void navigate({ to: "/agents/all", search: { agent: chat.spaces[0]!.handle } });
            }}
          >
            Memory, routines and access
          </Button>
          <Button size="sm" variant="outline" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

/** New agent / the empty state's rows: the approved sheet, with the Space picked in its title. */
export function NewAgentSheet({
  open,
  onOpenChange,
  stage,
  spaces,
  taken,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  stage?: AddAgentStage;
  spaces: { id: string; name: string }[];
  taken: (spaceId: string) => string[];
}) {
  const [space, setSpace] = useState<string | null>(null);
  const picked = spaces.find((s) => s.id === space) ?? spaces[0] ?? null;
  return (
    <AddAgentSheet
      open={open}
      onOpenChange={onOpenChange}
      space={picked}
      spaces={spaces}
      onSpaceChange={setSpace}
      role={null}
      taken={picked ? taken(picked.id) : []}
      initialStage={stage}
    />
  );
}

export type { ChatNeed };
