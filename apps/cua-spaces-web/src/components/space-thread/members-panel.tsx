// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronRightIcon, PanelRightCloseIcon, PanelRightOpenIcon, PictureInPicture2Icon, PlusIcon } from "lucide-react";
import { useState, type ReactNode } from "react";

import type { AgentRights, SpaceFact, StreamRow } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { Select } from "@/components/ui/select";
import { toast } from "@/components/ui/toast";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { MemberAvatar } from "./avatar";
import { AiTag, OwnerSuffix } from "./identity";
import { RAIL_WINDOWS, railStreams } from "./rail";
import { holderChoice, holdersLabel, RIGHT_ROWS, type HolderChoice } from "./rights";
import type { Roster, RosterAgent } from "./roster";

/** Who is here right now (P2: relay presence and session states). Without
 * it no dot is drawn: P1 never guesses. */
export type MemberPresence = Record<string, "online">;

/**
 * The right side of the call layout: "In this Space" (people, then agents
 * with what they are doing) and the Space's details below. Folded, it is a
 * narrow rail of marks.
 */
export function MembersPanel({
  roster,
  presence,
  facts,
  onMore,
  windows,
  desktop,
  selected,
  onSelect,
  onStream,
  folded,
  onFold,
  onSetRights,
  onRemoveAgent,
  onAddAgent,
}: {
  roster: Roster;
  /** Opens Add an agent (absent for guests). */
  onAddAgent?: () => void;
  /** Removes an agent member (absent: the host can't); shown only to who may. */
  onRemoveAgent?: (name: string) => Promise<void>;
  /** Changes an agent's rights; absent when you can't (only the owner and admins can). */
  onSetRights?: (name: string, rights: Partial<AgentRights>) => Promise<void>;
  presence?: MemberPresence;
  /** The first few of the core's facts; More opens the rest. */
  facts: SpaceFact[];
  onMore: () => void;
  /** The Space's windows (the core's Stream rows without the desktop); the first few show here. */
  windows: StreamRow[];
  /** The Desktop row, first in the list (the default view). */
  desktop?: StreamRow | null;
  /** The row the main view shows (the desktop when none). */
  selected?: string | null;
  /** Shows that row in the main view. */
  onSelect?: (row: StreamRow) => void;
  /** Opens that row in its own window (picture in picture where the host has it). */
  onStream: (row: StreamRow) => void;
  folded: boolean;
  onFold: (folded: boolean) => void;
}) {
  const [opened, setOpened] = useState<string | null>(null);
  if (folded) {
    return (
      <aside data-members-panel="folded" className="flex w-12 shrink-0 flex-col items-center gap-2.5 bg-chrome py-3">
        <FoldButton folded onFold={onFold} />
        {roster.people.map((p) => (
          <Tooltip key={p.id} content={p.label} side="left">
            <span>
              <MemberAvatar kind="person" name={p.label} size={24} presence={presence?.[p.id] ?? null} />
            </span>
          </Tooltip>
        ))}
        {roster.agents.map((a) => (
          <Tooltip key={a.id} content={a.caption ? `${a.label}: ${a.caption}` : a.label} side="left">
            <span>
              <MemberAvatar kind="agent" name={a.name} harness={a.harness} identity={a.identity} surface="chrome" size={24} dim={a.dim} presence={presence?.[a.id] ?? null} />
            </span>
          </Tooltip>
        ))}
      </aside>
    );
  }
  return (
    <aside data-members-panel="" className="flex w-72 shrink-0 flex-col bg-chrome">
      <div className="flex h-10 items-center justify-between pr-2 pl-5">
        <h2 className="text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">In this Space</h2>
        <span className="flex items-center">
          {onAddAgent ? (
            <Tooltip content="Add an agent" side="left">
              <button
                type="button"
                aria-label="Add an agent"
                data-add-agent-open=""
                onClick={onAddAgent}
                className="inline-flex size-7 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                <PlusIcon className="size-4" />
              </button>
            </Tooltip>
          ) : null}
          <FoldButton folded={false} onFold={onFold} />
        </span>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto px-3 pb-3" data-members="">
        {roster.people.map((p) => (
          <Row key={p.id} member={p.id} mark={<MemberAvatar kind="person" name={p.label} size={24} presence={presence?.[p.id] ?? null} />} label={p.label} caption={p.me ? "You" : null} />
        ))}
        {roster.agents.map((a) => (
          <Row
            key={a.id}
            member={a.id}
            mark={<MemberAvatar kind="agent" name={a.name} harness={a.harness} identity={a.identity} surface="chrome" size={24} dim={a.dim} presence={presence?.[a.id] ?? null} />}
            label={
              <span className="flex min-w-0 items-center gap-1.5">
                <span className="truncate">{a.label}</span>
                <AiTag />
                {a.collides && a.identity ? <OwnerSuffix identity={a.identity} /> : null}
              </span>
            }
            caption={a.caption}
            dim={a.dim}
            status={a.status}
            open={opened === a.id}
            onToggle={() => setOpened(opened === a.id ? null : a.id)}
          >
            {opened === a.id ? <AgentRightsRows agent={a} onSetRights={onSetRights} onRemove={onRemoveAgent && roster.canRemove(a) ? () => onRemoveAgent(a.name) : undefined} /> : null}
          </Row>
        ))}
      </ul>
      {windows.length || desktop ? (
        <section className="px-3 pt-2 pb-2" data-call-stream="">
          <h2 className="mb-1 px-2 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">Stream</h2>
          <ul>
            {[...(desktop ? [desktop] : []), ...railStreams(windows, selected ?? null)].map((w) => {
              const pip = w.actions.find((a) => a.id === "pip");
              const isDesktop = w.kind === "desktop";
              const current = isDesktop ? !selected || selected === w.id : selected === w.id;
              const name = isDesktop ? "Desktop" : (appOf(w) ?? w.label);
              return (
                <li key={w.id} className="group/stream relative">
                  <button
                    type="button"
                    {...(isDesktop ? { "data-stream-desktop": "" } : { "data-stream-window": w.id })}
                    aria-current={current ? "true" : undefined}
                    title={w.help}
                    onClick={() => (onSelect ? onSelect(w) : onStream(w))}
                    className={cn(
                      "flex h-8 w-full cursor-default items-center gap-2.5 rounded-lg pr-8 pl-2 text-left text-[13px] outline-none hover:bg-accent focus-visible:bg-accent",
                      current && "bg-accent font-medium",
                    )}
                  >
                    <WindowMark row={w} />
                    <span className="min-w-0 flex-1 truncate">
                      <span>{name}</span>
                      {!isDesktop && appOf(w) && w.label && w.label !== appOf(w) ? <span className="ml-1.5 font-normal text-muted-foreground">{w.label}</span> : null}
                    </span>
                  </button>
                  {pip ? (
                    <Tooltip content={pip.active ? pip.help : "Open in its own window"}>
                      <button
                        type="button"
                        data-stream-popout={w.id}
                        aria-pressed={pip.active}
                        aria-label={pip.active ? pip.help : `Open ${name} in its own window`}
                        onClick={() => onStream(w)}
                        className={cn(
                          "absolute top-1 right-1 inline-flex size-6 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-background hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60",
                          pip.active ? "text-brand-strong opacity-100" : "opacity-0 group-hover/stream:opacity-100",
                        )}
                      >
                        <PictureInPicture2Icon className="size-3.5" />
                      </button>
                    </Tooltip>
                  ) : null}
                </li>
              );
            })}
          </ul>
          {windows.length > RAIL_WINDOWS ? (
            <button
              type="button"
              data-stream-show-all=""
              onClick={onMore}
              className="mt-0.5 ml-2 cursor-default text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:underline"
            >
              Show all {windows.length}
            </button>
          ) : null}
        </section>
      ) : null}
      <section className="px-3 pt-2 pb-3" data-call-details="">
        <h2 className="mb-1 px-2 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">Details</h2>
        {facts.map((f) => (
          <div key={f.label} data-rail-fact={f.label} className="flex h-8 items-center justify-between gap-4 px-2 text-[13px]">
            <span className="shrink-0 text-muted-foreground">{f.label}</span>
            <span data-rail-fact-value="" className="min-w-0 truncate text-right" title={f.help ?? f.value}>
              {f.value}
            </span>
          </div>
        ))}
        <button
          type="button"
          data-call-more=""
          onClick={onMore}
          className="flex h-8 w-full cursor-default items-center justify-between rounded-lg px-2 text-[13px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:bg-accent"
        >
          More
          <ChevronRightIcon className="size-3.5" />
        </button>
      </section>
    </aside>
  );
}

/** A window's app name, when the row has one. */
const appOf = (w: StreamRow) => (w.icon.kind === "app" ? w.icon.appName || null : null);

/** The OS a core OS icon id names (`os-macos`, `os-ubuntu`, `os-windows`, ...). */
export const osOfIcon = (id: string): "macos" | "windows" | "linux" => (/mac/i.test(id) ? "macos" : /win/i.test(id) ? "windows" : "linux");

/** The window's app: its initial (the Space's own icons are not read yet), no frame. */
function WindowMark({ row }: { row: StreamRow }) {
  return (
    <span aria-hidden className="flex size-5 shrink-0 items-center justify-center rounded-[5px] bg-muted text-[10px] font-semibold text-muted-foreground">
      {row.icon.kind === "app" ? (row.icon.appName || row.icon.appId).trim().charAt(0).toUpperCase() || "?" : <OsIcon os={osOfIcon(row.icon.id)} className="size-3" />}
    </span>
  );
}

function Row({
  member,
  mark,
  label,
  caption,
  dim,
  status,
  open,
  onToggle,
  children,
}: {
  member: string;
  mark: ReactNode;
  label: ReactNode;
  caption: string | null;
  dim?: boolean;
  status?: string;
  /** An agent's row opens to its rights. */
  open?: boolean;
  onToggle?: () => void;
  children?: ReactNode;
}) {
  const body = (
    <>
      {mark}
      <span className="min-w-0 flex-1">
        <span className={dim ? "block truncate text-[13px] text-muted-foreground" : "block truncate text-[13px]"}>{label}</span>
        {caption ? <span className="block truncate text-[11px] text-muted-foreground">{caption}</span> : null}
      </span>
    </>
  );
  if (!onToggle) {
    return (
      <li data-member={member} data-member-status={status} className="flex min-h-10 items-center gap-2.5 rounded-lg px-2 py-1.5">
        {body}
      </li>
    );
  }
  return (
    <li data-member={member} data-member-status={status}>
      <button
        type="button"
        aria-expanded={open}
        data-member-toggle=""
        onClick={onToggle}
        className="flex min-h-10 w-full cursor-default items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none hover:bg-accent focus-visible:bg-accent"
      >
        {body}
        <ChevronRightIcon className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-90")} />
      </button>
      {children}
    </li>
  );
}

const CHOICES: readonly { value: HolderChoice; label: string }[] = [
  { value: "owner", label: "Owner" },
  { value: "admins", label: "Admins" },
  { value: "members", label: "Members" },
];

/** Who can task, stop and approve an agent: three plain rows under its name.
 * The owner and admins change them; everyone else reads them. */
function AgentRightsRows({ agent, onSetRights, onRemove }: { agent: RosterAgent; onSetRights?: (name: string, rights: Partial<AgentRights>) => Promise<void>; onRemove?: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const remove = () => {
    if (!onRemove) return;
    setBusy(true);
    onRemove().then(
      () => setBusy(false),
      (e: unknown) => {
        setBusy(false);
        toast(`Couldn't remove ${agent.label}`, { description: e instanceof Error ? e.message : String(e), type: "error" });
      },
    );
  };
  return (
    <div data-agent-rights={agent.name} className="mb-1 ml-9 divide-y divide-border/60 border-y border-border/60">
      {RIGHT_ROWS.map(({ right, label }) => {
        const value = agent.rights[right];
        const choice = holderChoice(value);
        return (
          <div key={right} data-agent-right={right} className="flex h-8 items-center justify-between gap-3 pr-1 pl-1 text-[12.5px]">
            <span className="text-muted-foreground">{label}</span>
            {onSetRights ? (
              <Select<HolderChoice>
                aria-label={`${label}: ${agent.label}`}
                value={choice}
                disabled={busy}
                options={choice === "named" ? [...CHOICES, { value: "named", label: holdersLabel(value) }] : CHOICES}
                onValueChange={(v) => {
                  if (v === "named" || v === choice) return;
                  setBusy(true);
                  onSetRights(agent.name, { [right]: v }).then(
                    () => setBusy(false),
                    (e: unknown) => {
                      setBusy(false);
                      toast(`Couldn't change who ${label.toLowerCase()} ${agent.label}`, { description: e instanceof Error ? e.message : String(e), type: "error" });
                    },
                  );
                }}
                className="h-7 min-w-0 border-0 bg-transparent px-1.5 text-[12.5px] shadow-none hover:bg-accent dark:bg-transparent"
              />
            ) : (
              <span data-agent-right-value="" className="px-1.5">
                {holdersLabel(value)}
              </span>
            )}
          </div>
        );
      })}
      {onRemove ? (
        <div className="flex h-8 items-center justify-end pr-1">
          <button
            type="button"
            data-remove-agent={agent.name}
            disabled={busy}
            onClick={remove}
            className="cursor-default rounded-lg px-1.5 text-[12.5px] text-destructive outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-50"
          >
            Remove from Space
          </button>
        </div>
      ) : null}
    </div>
  );
}

function FoldButton({ folded, onFold }: { folded: boolean; onFold: (folded: boolean) => void }) {
  const label = folded ? "Show people" : "Hide people";
  return (
    <Tooltip content={label} side="left">
      <button
        type="button"
        aria-label={label}
        data-members-fold=""
        onClick={() => onFold(!folded)}
        className="inline-flex size-7 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        {folded ? <PanelRightOpenIcon className="size-4" /> : <PanelRightCloseIcon className="size-4" />}
      </button>
    </Tooltip>
  );
}
