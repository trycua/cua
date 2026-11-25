// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Home's sections: "Your agents" (the Agents page's own
 * list, `agentList.view`: agents in your Spaces and this device's runs that
 * haven't started, with Connect or Try again) and "Your Spaces" (the Spaces
 * page's tiles). One row each, and See all.
 */

import { Link, useNavigate } from "@tanstack/react-router";
import { ChevronRightIcon } from "lucide-react";
import { useMemo, type KeyboardEvent, type MouseEvent } from "react";

import { spaceDiskBytes, useMachines, useResources, useSpaces, useSpaceThumbnail, type AgentChat } from "@/bridge";
import { BulkConfirmDialog, useBulkRemove } from "@/components/spaces/bulk-remove";
import { desktopOf, type SpaceItem } from "@/components/spaces/model";
import { SpaceMoreMenu, type MenuActions } from "@/components/spaces/space-menu";
import { SpaceTile } from "@/components/spaces/space-tile";
import { AgentTile } from "@/components/space-thread/avatar";
import { Button } from "@/components/ui/button";
import { GroupedSection } from "@/components/ui/grouped";
import { toastError } from "@/components/ui/toast";
import { useAttemptAction } from "@/hooks/use-agent-attempts";
import { accessOf, isMine } from "@/lib/space-access";
import { useListedSpaces } from "@/components/spaces/listed";
import { useT, t as tr } from "@/i18n";
import { useHiddenDesktops } from "@/lib/hidden-desktops";
import { machineName, orderSpaces, spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

/** Agents shown on Home before See all. */
const HOME_AGENTS = 3;
/** Spaces shown on Home before See all: one row of tiles. */
const HOME_SPACES = 4;

const TONE: Record<AgentChat["tone"], string> = {
  you: "bg-warning",
  working: "bg-success",
  failed: "bg-destructive",
  idle: "bg-muted-foreground/40",
  asleep: "bg-muted-foreground/40",
  "not-connected": "bg-warning",
  creating: "bg-brand",
  stopped: "bg-muted-foreground/40",
};

function ago(ms: number, now: number): string {
  if (!ms) return "";
  const m = Math.max(0, Math.round((now - ms) / 60_000));
  if (m < 1) return tr("home.sections.ago_now");
  if (m < 60) return tr("home.sections.ago_minutes", { count: m });
  const h = Math.round(m / 60);
  return h < 24 ? tr("home.sections.ago_hours", { count: h }) : tr("home.sections.ago_days", { count: Math.round(h / 24) });
}

function SectionHead({ title, note, to, all }: { title: string; note?: string | null; to: "/agents" | "/spaces"; all: string }) {
  return (
    <div className="mb-3 flex items-center justify-between">
      <h2 className="flex items-baseline gap-2 text-[17px] font-semibold">
        {title} {note ? <span className="text-[14px] font-normal text-muted-foreground">{note}</span> : null}
      </h2>
      <Link to={to} data-home-see-all={to} className="flex items-center gap-1 text-[13px] text-muted-foreground hover:text-foreground">
        {all} <ChevronRightIcon className="size-4" />
      </Link>
    </div>
  );
}

/** "Your agents": the first rows of the one agent list; a row opens its Space's thread. */
export function YourAgents({ chats }: { chats: AgentChat[] | null | undefined }) {
  const t = useT();
  const act = useAttemptAction();
  if (!chats || chats.length === 0) return null;
  const working = chats.filter((c) => c.tone === "working").length;
  return (
    <section data-home-agents="">
      <SectionHead
        title={t("home.sections.your_agents")}
        note={working ? t("home.sections.working", { count: working }) : null}
        to="/agents"
        all={t("home.sections.see_all", { count: chats.length })}
      />
      <GroupedSection>
        {chats.slice(0, HOME_AGENTS).map((c) => (
          <AgentRunRow key={c.key} chat={c} onAction={() => act(c)} />
        ))}
      </GroupedSection>
    </section>
  );
}

function AgentRunRow({ chat: c, onAction }: { chat: AgentChat; onAction: () => void }) {
  const navigate = useNavigate();
  const { data: spaces } = useSpaces();
  const place = c.places[0];
  const space = spaces?.find((s) => s.id === place?.spaceId);
  const thumb = useSpaceThumbnail(place?.spaceId ?? "", Boolean(space) && spaceState(space!) === "running");
  const open = () => place && void navigate({ to: "/spaces/$spaceId", params: { spaceId: place.spaceId } });
  const stop = (e: MouseEvent | KeyboardEvent) => e.stopPropagation();
  return (
    <div
      role="link"
      tabIndex={0}
      data-home-agent={c.key}
      data-chat-tone={c.tone}
      onClick={open}
      onKeyDown={(e) => (e.key === "Enter" ? open() : undefined)}
      className="flex min-h-[64px] cursor-default items-center gap-4 px-4 py-3 outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
    >
      <AgentTile harness={c.harness} icon={c.identity?.icon} owner={c.identity?.owner} name={c.label} px={32} dim={c.tone === "asleep"} surface="background" />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-medium">{c.job ?? c.label}</div>
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", TONE[c.tone])} />
          <span className="truncate" data-home-agent-line="">
            {c.line}
          </span>
        </div>
      </div>
      {c.action ? (
        <Button size="sm" variant="outline" data-home-agent-action={c.action.id} onClick={(e) => (stop(e), onAction())} onKeyDown={stop}>
          {c.action.label}
        </Button>
      ) : null}
      <span className="hidden w-36 truncate text-right text-xs text-muted-foreground md:block">{place?.spaceName}</span>
      <span className="w-10 text-right text-xs text-muted-foreground tabular-nums">{ago(c.lastMs, Date.now())}</span>
      <span className="hidden w-20 lg:block">
        {thumb ? (
          <img src={thumb} alt="" className="aspect-[16/10] w-20 rounded-lg object-cover" />
        ) : (
          <span className="block aspect-[16/10] w-20 rounded-lg bg-muted" />
        )}
      </span>
    </div>
  );
}

/** "Your Spaces": the Spaces page's tiles, one row, with power and ⋯ on hover. */
export function YourSpaces() {
  const t = useT();
  const { data, startSpace, stopSpace } = useSpaces();
  const { data: machines = [] } = useMachines();
  const resources = useResources();
  // Running Spaces first, then the most recently used; This Mac's unshared desktop never counts.
  const { listed, counted, hiddenCount } = useListedSpaces(data);
  const desktops = useHiddenDesktops();
  const spaces = useMemo(() => orderSpaces(listed), [listed]);
  const items = useMemo(
    () =>
      spaces.map((space): SpaceItem => {
        const machine = machineName(machines, space);
        const mine = isMine(space);
        return {
          space,
          state: spaceState(space),
          access: accessOf(space, machine),
          mine,
          bytes: mine ? spaceDiskBytes(resources, space.id) : null,
          machine,
          desktop: desktopOf(space),
        };
      }),
    [spaces, machines, resources],
  );
  const present = useMemo(() => new Set(spaces.map((s) => s.id)), [spaces]);
  const bulk = useBulkRemove(present, () => {});
  // With everything hidden, the way back still shows.
  if (counted.length === 0 && hiddenCount === 0) return null;
  const running = items.filter((it) => it.state === "running" && it.desktop !== "off").length;
  const actions: MenuActions = {
    hide: (it) => desktops.hide(it.space.id),
    power: (it, on) =>
      void (on ? startSpace : stopSpace)(it.space.id).catch(toastError(t("home.sections.turn_failed", { state: on ? "on" : "off", name: it.space.name }))),
    remove: (it) => bulk.ask([it]),
  };
  return (
    <section data-home-spaces="">
      <SectionHead
        title={t("home.sections.your_spaces")}
        note={running ? t("home.sections.running", { count: running }) : null}
        to="/spaces"
        all={t("home.sections.see_all", { count: counted.length })}
      />
      <ul className="grid grid-cols-2 gap-5 md:grid-cols-4">
        {items.slice(0, HOME_SPACES).map((it) => (
          <li key={it.space.id}>
            <SpaceTile
              item={it}
              hide={it.desktop ? () => actions.hide?.(it) : undefined}
              more={it.desktop ? undefined : <SpaceMoreMenu item={it} actions={actions} />}
            />
          </li>
        ))}
      </ul>
      {hiddenCount > 0 ? (
        <p className="mt-3 flex items-center gap-1.5 text-[13px] text-muted-foreground" data-home-hidden={hiddenCount}>
          {t("home.sections.hidden", { count: hiddenCount })}
          <span aria-hidden>·</span>
          <button
            type="button"
            onClick={desktops.showAll}
            className="text-foreground underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring/60"
            data-home-show-hidden=""
          >
            {t("home.sections.show")}
          </button>
        </p>
      ) : null}
      {bulk.view ? <BulkConfirmDialog view={bulk.view} items={bulk.asked} onCancel={bulk.cancel} onConfirm={() => void bulk.confirm()} /> : null}
    </section>
  );
}
