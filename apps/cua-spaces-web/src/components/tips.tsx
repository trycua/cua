// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Tips (CUA-2204): one quiet tip at a time, where it matters, in place of
 * the sidebar's "Get started" and the agent banner. Which tip shows, its
 * words and its action are the core's (`tips.view`); what the page knows
 * (`TipFacts`) comes from real data here: the Spaces, the agents set up on
 * this computer, the computers online, This Mac's disk. Once done, skipped
 * or closed a tip never comes back; "Don't show tips" turns them all off,
 * and Settings, General turns them back on or starts them over.
 *
 * Places: the Spaces page (above the computer alert and the toolbar), a
 * Space's page (top of its content) and the sidebar, where "Get started"
 * was, only while no page shows one.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, CopyIcon, LightbulbIcon, XIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";

import {
  hostResources,
  tipsView,
  useAgents,
  useBridge,
  useMachines,
  useResources,
  useSession,
  useSpaces,
  useTips,
  type AgentSetupRow,
  type Space,
  type Tip,
  type TipFacts,
  type TipPlace,
} from "@/bridge";
import { Button } from "@/components/ui/button";
import { canDelete, canPower, realSpaces, spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

/** Where Select stopped goes: the list for This Mac, biggest first, the
 * stopped ones picked. The list view (`/spaces?view=list`) reads these:
 * `view=list` the list (not tiles), `on=this-mac` only This Mac's Spaces,
 * `sort=size` biggest on disk first, `select=stopped` the stopped ones you
 * may delete selected. */
export const SELECT_STOPPED_SEARCH = { view: "list", on: "this-mac", sort: "size", select: "stopped" } as const;

/** Whether a page shows a tip now (the sidebar's waits until none does). */
const usePageTip = create<{ shown: number; add(): () => void }>((set) => ({
  shown: 0,
  add: () => {
    set((s) => ({ shown: s.shown + 1 }));
    return () => set((s) => ({ shown: s.shown - 1 }));
  },
}));

/** The sharing facts a Space carries once the host reports them (`Space.access`). */
type Access = { kind?: string; people?: unknown[]; canDelete?: boolean; canPower?: boolean };
const accessOf = (s: Space): Access | undefined => (s as Space & { access?: Access }).access;
const mine = (s: Space) => (accessOf(s)?.kind ?? "mine") === "mine";
const deletable = (s: Space) => canDelete(s) && (accessOf(s)?.canDelete ?? mine(s));
const powerable = (s: Space) => canPower(s) && (accessOf(s)?.canPower ?? true);

/** The newest running Space: started last (else used last). */
export function newestRunning(spaces: readonly Space[]): Space | undefined {
  const when = (s: Space) => s.startedAt ?? s.lastUsedAt;
  return spaces.filter((s) => spaceState(s) === "running").reduce<Space | undefined>((a, s) => (!a || when(s) > when(a) ? s : a), undefined);
}

/** The coding agents on this computer, read once per page load. */
const agentRows = new WeakMap<object, Promise<AgentSetupRow[]>>();
/** Once read: drawn at once by a tip mounted later (no tip popping in a frame after the page). */
const agentRowsRead = new WeakMap<object, AgentSetupRow[]>();

/** The agents set up here (null: not read yet). */
function useAgentSetup(): AgentSetupRow[] | null {
  const { data: host } = useBridge();
  const { agentSetup } = useAgents();
  const [rows, setRows] = useState<AgentSetupRow[] | null>(() => (host ? (agentRowsRead.get(host) ?? null) : null));
  useEffect(() => {
    if (!host) return;
    let live = true;
    let p = agentRows.get(host);
    if (!p) {
      p = agentSetup()
        .catch(() => [])
        .then((r) => (agentRowsRead.set(host, r), r));
      agentRows.set(host, p);
    }
    void p.then((r) => live && setRows(r));
    return () => {
      live = false;
    };
    // Once per host.
  }, [host]);
  return rows;
}

/** The tips can be chosen (the agents here are read): a page draws its cards
 * together with its tip, not the tip a moment later (CUA-1951). */
export function useTipsLoaded(): boolean {
  return useAgentSetup() !== null;
}

/** What the tips read, from the page's data. `space`: the Space page's Space. */
export function useTipFacts(space?: Space): TipFacts | null {
  const { data: all = [] } = useSpaces();
  const { data: machines = [] } = useMachines();
  const resources = useResources();
  const agents = useAgentSetup();
  const pageTipShown = usePageTip((s) => s.shown > 0);
  return useMemo(() => {
    // Until the agents are read the try-agent tip can't name yours: no tip for that moment.
    if (!agents) return null;
    const spaces = realSpaces(all);
    const stopped = spaces.filter((s) => spaceState(s) === "stopped");
    const here = machines.find((m) => m.current);
    const disk = hostResources(resources, here)?.warning.level;
    return {
      newestRunning: newestRunning(spaces)?.name ?? null,
      spacePage: space && spaceState(space) === "running" ? space.name : null,
      agentsConnected: agents.filter((a) => a.configured).map((a) => a.agent),
      agentsInstalled: agents.filter((a) => a.installed).map((a) => a.agent),
      hasStopped: stopped.some(powerable),
      deletableStopped: stopped.filter(deletable).length,
      diskNearlyFull: disk === "low" || disk === "critical",
      secondComputerOnline: machines.filter((m) => m.online).length >= 2,
      sharedAny: spaces.some((s) => mine(s) && (accessOf(s)?.people?.length ?? 0) > 0),
      pageTipShown,
    };
  }, [all, machines, resources, agents, space, pageTipShown]);
}

/** The tip for `place` now (null: none, tips off, or no core). */
export function useTip(place: TipPlace, space?: Space): Tip | null {
  const { core } = useBridge();
  const { state } = useTips();
  const facts = useTipFacts(space);
  return useMemo(() => (core.status === "ready" && facts ? tipsView(core, place, facts, state) : null), [core, place, facts, state]);
}

/** Copy, which says "Copied" in place for a moment. */
function CopyButton({ text, label, doneLabel, iconOnly = false }: { text: string; label: string; doneLabel: string; iconOnly?: boolean }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <Button
      size={iconOnly ? "icon-sm" : "sm"}
      variant="ghost"
      aria-label={iconOnly ? (copied ? doneLabel : label) : undefined}
      data-tip-copy={copied ? "copied" : ""}
      className="text-muted-foreground hover:text-foreground"
      onClick={() => {
        void navigator.clipboard?.writeText(text).catch(() => undefined);
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 1600);
      }}
    >
      {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
      {iconOnly ? null : copied ? doneLabel : label}
    </Button>
  );
}

/** The tip marker: a quiet lightbulb, no box. */
const Bulb = ({ className }: { className?: string }) => (
  <LightbulbIcon aria-hidden data-tip-mark="" className={cn("size-4 shrink-0 text-muted-foreground", className)} strokeWidth={1.75} />
);

/**
 * A page's tip (`place` spaces or space): one line, its action, Next tip,
 * Don't show tips and ×; the try-agent request under it, read-only, with
 * Copy (and Open in Cursor where the core gives it).
 */
export function PageTip({ place, space, onShare, className }: { place: "spaces" | "space"; space?: Space; onShare?: () => void; className?: string }) {
  const tip = useTip(place, space);
  const { send } = useTips();
  const { openExternal } = useSession();
  const navigate = useNavigate();
  const add = usePageTip((s) => s.add);
  useEffect(() => (tip ? add() : undefined), [tip, add]);
  if (!tip) return null;
  const close = () => void send({ type: "close", id: tip.id });
  const { action } = tip;
  const button =
    action.kind === "selectStopped" ? (
      <Button size="sm" variant="secondary" data-tip-action="select-stopped" onClick={() => void navigate({ to: "/spaces", search: { ...SELECT_STOPPED_SEARCH } })}>
        {action.label}
      </Button>
    ) : action.kind === "share" && onShare ? (
      <Button size="sm" variant="secondary" data-tip-action="share" onClick={onShare}>
        {action.label}
      </Button>
    ) : null;
  return (
    <section data-tip={tip.id} aria-label={tip.text} className={cn("mb-5 rounded-lg bg-foreground/[0.04] px-4 py-2.5 dark:bg-white/[0.05]", className)}>
      <div className="flex min-h-7 items-center gap-3">
        <Bulb />
        <p className="min-w-0 flex-1 truncate text-[13px] font-medium" data-tip-text="">
          {tip.text}
        </p>
        {button}
        {tip.next ? (
          <Button size="sm" variant="ghost" data-tip-next="" className="text-muted-foreground" onClick={() => void send({ type: "next", id: tip.id })}>
            {tip.next}
          </Button>
        ) : null}
        {tip.dontShow ? (
          <Button size="sm" variant="ghost" data-tip-off="" className="text-muted-foreground" onClick={() => void send({ type: "dont-show" })}>
            {tip.dontShow}
          </Button>
        ) : null}
        <Button size="icon-sm" variant="ghost" aria-label={tip.close} data-tip-close="" className="-mr-1.5 text-muted-foreground" onClick={close}>
          <XIcon className="size-3.5" />
        </Button>
      </div>
      {tip.code ? (
        <div className="mt-1.5 mb-1 flex items-center gap-2 pl-7">
          <textarea
            readOnly
            rows={1}
            value={tip.code}
            data-tip-code=""
            aria-label={tip.text}
            onFocus={(e) => e.currentTarget.select()}
            className="h-8 min-w-0 flex-1 resize-none overflow-hidden rounded-lg bg-background px-3 py-[7px] font-mono text-xs leading-[18px] whitespace-nowrap outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
          />
          {action.value && action.label ? <CopyButton text={action.value} label={action.label} doneLabel={action.doneLabel ?? action.label} /> : null}
          {tip.openIn ? (
            <Button size="sm" variant="ghost" data-tip-open-in="" className="text-muted-foreground hover:text-foreground" onClick={() => void openExternal(tip.openIn!.url).catch(() => {})}>
              {tip.openIn.label}
            </Button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

/** The sidebar's tip, where "Get started" was: a tip that belongs to no
 * page, only while no page shows one. */
export function SidebarTip() {
  const tip = useTip("sidebar");
  const { send } = useTips();
  if (!tip) return null;
  return (
    <div data-tip={tip.id} className="mx-2.5 mb-2 rounded-lg bg-foreground/[0.04] p-2.5 dark:bg-white/[0.05]">
      <div className="flex items-start gap-2">
        <Bulb className="mt-px size-3.5" />
        <p className="min-w-0 flex-1 text-xs leading-[1.45]" data-tip-text="">
          {tip.text}
        </p>
        <button
          type="button"
          aria-label={tip.close}
          data-tip-close=""
          onClick={() => void send({ type: "close", id: tip.id })}
          className="-mt-0.5 -mr-1 rounded-md p-0.5 text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <XIcon className="size-3.5" />
        </button>
      </div>
      {tip.code ? (
        <div className="mt-2 flex items-center gap-1.5">
          <code data-tip-code="" className="min-w-0 flex-1 truncate rounded-lg bg-background px-2 py-1 font-mono text-[11px] leading-5 select-all">
            {tip.code}
          </code>
          {tip.action.value ? <CopyButton iconOnly text={tip.action.value} label={tip.action.label || "Copy"} doneLabel={tip.action.doneLabel ?? "Copied"} /> : null}
        </div>
      ) : null}
    </div>
  );
}
