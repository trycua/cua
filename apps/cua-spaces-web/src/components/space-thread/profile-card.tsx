// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The card a person or an agent opens to in a Space (redesign): their name
 * or avatar in the thread, an @mention, a row in "In this space". One calm
 * shape for both, like a chat app's profile card: who, their access or what
 * runs them, their status, the few actions that apply, and for an agent the
 * rest folded under Details. Every word is the core's (`profileCard.*`).
 */

import { Popover } from "@base-ui/react/popover";
import { Link, useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon, CopyIcon, Loader2Icon, PlugIcon } from "lucide-react";
import { createContext, useContext, useState, type ReactNode } from "react";

import { agentCard, personCard, useBridge, type AgentMember, type AgentRights, type MemberIdentity, type ShareEntry, type ThreadRole } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/select";
import { toast } from "@/components/ui/toast";
import { useConnectors } from "@/hooks/use-connectors";
import { cn } from "@/lib/utils";

import { AgentTile, PersonDot } from "./avatar";
import { holdersName, identityKey, joinedHow, reportedHarness } from "./identity";
import { RightsEditor } from "./members-panel";
import { RIGHT_ROWS } from "./rights";
import { personLabel, type Roster, type RosterAgent, type RosterPerson } from "./roster";

/** What a Space's cards read: who it's shared with, your role, its agents, and what you may change. */
export interface ProfileScope {
  spaceId: string;
  roster: Roster;
  shares: readonly ShareEntry[];
  members: readonly AgentMember[];
  role: ThreadRole | null;
  /** Your sign-in (`ada@example.com`), for your own card's email. */
  identity: string | null;
  onRemoveAgent?: (name: string) => Promise<void>;
  /** Changes who may task, stop and approve an agent (owners and admins). */
  onSetRights?: (name: string, rights: Partial<AgentRights>) => Promise<void>;
}

const Scope = createContext<ProfileScope | null>(null);
export const ProfileScopeProvider = Scope.Provider;
export const useProfileScope = () => useContext(Scope);

const canManage = (role: ThreadRole | null) => role === null || role === "owner" || role === "admin";

/* ---- The shape both share ---------------------------------------------------- */

function CardShell({
  mark,
  title,
  badge,
  sub,
  children,
  actions,
  data,
}: {
  mark: ReactNode;
  title: ReactNode;
  badge?: ReactNode;
  sub?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  data: Record<string, string>;
}) {
  return (
    <div className="w-[300px] p-4" {...data}>
      <div className="flex items-center gap-3">
        {mark}
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="truncate text-[14px] font-semibold" data-card-title="">
              {title}
            </span>
            {badge}
          </div>
          {sub ? <div className="truncate text-xs text-muted-foreground">{sub}</div> : null}
        </div>
      </div>
      {children ? <div className="mt-3 space-y-1.5 text-[12.5px]">{children}</div> : null}
      {actions ? <div className="-mx-2 mt-3 flex flex-wrap items-center gap-1">{actions}</div> : null}
    </div>
  );
}

const Line = ({ children, muted, data }: { children: ReactNode; muted?: boolean; data?: string }) => (
  <p data-card-line={data} className={cn("leading-snug", muted && "text-muted-foreground")}>
    {children}
  </p>
);

/* ---- People ------------------------------------------------------------------ */

export function PersonCard({ person, onDone }: { person: RosterPerson; onDone?: () => void }) {
  const { core, data } = useBridge();
  const scope = useProfileScope();
  const share = scope?.shares.find((s) => !s.who.startsWith("team:") && personLabel(s.who).toLowerCase() === person.label.toLowerCase());
  const myRole = scope?.role ?? null;
  const v = personCard(core, {
    name: person.label,
    who: person.me ? (scope?.identity ?? null) : (share?.who ?? null),
    me: person.me,
    access: person.me ? (myRole === null || myRole === "owner" ? "owner" : myRole === "admin" ? "editor" : myRole === "guest" ? "viewer" : "editor") : (share?.role ?? null),
    // Which team someone is on isn't known to this device yet (only that a team is shared with): none shown.
    team: null,
    active: person.me || Boolean(share?.connected),
    lastSeenMs: null,
    nowMs: Date.now(),
    canManage: canManage(myRole),
  });
  const [busy, setBusy] = useState(false);
  if (!v) return null;
  const act = (op: () => Promise<unknown>, failed: string) => {
    setBusy(true);
    op().then(
      () => (setBusy(false), onDone?.()),
      (e: unknown) => (setBusy(false), toast(failed, { description: e instanceof Error ? e.message : String(e), type: "error" })),
    );
  };
  return (
    <CardShell
      data={{ "data-profile-card": "person", "data-profile": person.label }}
      mark={<PersonDot name={person.label} px={40} />}
      title={v.name}
      badge={v.youLabel ? <span className="text-xs text-muted-foreground">{v.youLabel}</span> : null}
      sub={v.email}
      actions={
        v.removeLabel ? (
          <>
            <span className="flex-1" />
            {v.removeLabel && share && scope && data ? (
              <Button
                size="sm"
                variant="ghost"
                className="text-destructive hover:text-destructive"
                disabled={busy}
                data-card-remove=""
                onClick={() => act(() => data.call("sharing.unshare", { spaceId: scope.spaceId, who: share.who }), `Couldn't remove ${v.name}`)}
              >
                {v.removeLabel}
              </Button>
            ) : null}
          </>
        ) : null
      }
    >
      {v.access ? (
        <div className="flex min-h-7 items-center justify-between gap-3" data-card-access="">
          <span className="text-muted-foreground">Access</span>
          {v.changeAccessLabel && share && scope && data ? (
            <Select<string>
              aria-label={v.changeAccessLabel}
              value={v.accessValue ?? "viewer"}
              disabled={busy}
              options={v.accessChoices}
              onValueChange={(role) => act(() => data.call("sharing.share", { spaceId: scope.spaceId, who: share.who, role }), `Couldn't change ${v.name}'s access`)}
              className="h-7 min-w-0 border-0 bg-muted px-2 text-[12.5px] shadow-none dark:bg-muted"
            />
          ) : (
            <span>{v.access}</span>
          )}
        </div>
      ) : null}
      {v.team ? <Line muted>{v.team}</Line> : null}
      {v.presence ? (
        <p className="flex items-center gap-1.5" data-card-presence="">
          <span className={cn("size-1.5 rounded-full", v.active ? "bg-success" : "bg-muted-foreground/40")} />
          <span className={v.active ? "" : "text-muted-foreground"}>{v.presence}</span>
        </p>
      ) : null}
    </CardShell>
  );
}

/* ---- Agents ------------------------------------------------------------------ */

export function AgentCard({ identity: i, agent, roster, onRemove, onDone }: { identity: MemberIdentity; agent?: RosterAgent; roster: Roster; onRemove?: () => Promise<void>; onDone?: () => void }) {
  const { core } = useBridge();
  const scope = useProfileScope();
  const c = useConnectors();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const member = scope?.members.find((m) => m.name === i.handle);
  const harness = reportedHarness(i);
  const v = agentCard(core, {
    displayName: i.displayName,
    handle: i.handle,
    harness: i.harness.id,
    harnessReported: harness,
    ownerName: i.owner.name,
    ownerId: i.owner.id,
    ownerIsMe: roster.me !== null && i.owner.id === roster.me,
    device: i.device?.name ?? null,
    origin: i.origin,
    status: member?.status ?? agent?.status ?? "",
    activity: member?.activity ?? null,
    joinedHow: joinedHow(i),
    joinedAtMs: i.joined.atMs,
    nowMs: Date.now(),
    rights: agent
      ? RIGHT_ROWS.map(({ right, label }) => ({
          label: RIGHT_WORDS[right] ?? label,
          holders: holdersName(agent.rights[right], i, roster),
        }))
      : [],
    canRemove: Boolean(onRemove),
    connectors: c.input,
  });
  if (!v) return null;
  const connecting = v.app ? c.input.busy.some((b) => b.id === `app:${v.app}`) : false;
  const remove = () => {
    if (!onRemove) return;
    setBusy(true);
    onRemove().then(
      () => (setBusy(false), onDone?.()),
      (e: unknown) => (
        setBusy(false),
        toast(`Couldn't remove ${v.name}`, {
          description: e instanceof Error ? e.message : String(e),
          type: "error",
        })
      ),
    );
  };
  return (
    <CardShell
      data={{ "data-profile-card": "agent", "data-profile": i.handle, "data-provenance": i.handle }}
      mark={<AgentTile harness={i.harness.id} icon={i.icon} owner={i.owner} name={i.handle} px={40} surface="popover" />}
      title={v.name}
      badge={
        <span data-card-badge="" className="rounded-lg bg-muted px-1.5 py-px text-[10.5px] font-medium text-muted-foreground">
          {v.badge}
        </span>
      }
      actions={
        <>
          {v.connectLabel && v.app ? (
            <Button size="sm" variant="outline" className="ml-2" disabled={connecting} data-card-connect="" onClick={() => void c.connectApp(v.app!)}>
              {connecting ? <Loader2Icon className="animate-spin" /> : <PlugIcon />}
              {v.connectLabel}
            </Button>
          ) : null}
          {v.settingsLabel && v.app ? (
            <Button size="sm" variant="ghost" data-card-settings="" render={<Link to="/settings/connectors/$app" params={{ app: v.app }} />}>
              {v.settingsLabel}
            </Button>
          ) : null}
          <span className="flex-1" />
          {v.removeLabel ? (
            <Button size="sm" variant="ghost" className="text-destructive hover:text-destructive" disabled={busy} data-remove-agent={i.handle} onClick={remove}>
              {busy ? "Removing…" : v.removeLabel}
            </Button>
          ) : null}
        </>
      }
    >
      <Line data="who">{v.line}</Line>
      <p className="flex items-center gap-1.5" data-card-status={v.status}>
        <span className={cn("size-1.5 shrink-0 rounded-full", v.status === "working" ? "bg-success" : v.status === "not-connected" ? "bg-warning" : "bg-muted-foreground/40")} />
        <span className={cn("truncate", v.status === "idle" && "text-muted-foreground")}>{v.statusText}</span>
      </p>
      <div className="pt-1">
        <button
          type="button"
          aria-expanded={open}
          data-card-details=""
          onClick={() => setOpen(!open)}
          className="-ml-1 flex cursor-default items-center gap-1 rounded-lg px-1 py-0.5 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <ChevronRightIcon className={cn("size-3 transition-transform", open && "rotate-90")} /> {v.detailsLabel}
        </button>
        {open ? (
          <dl className="mt-1.5 space-y-1" data-card-details-open="">
            {v.details
              .filter((d) => !(scope?.onSetRights && agent && canManage(scope.role) && RIGHT_LABELS.has(d.label)))
              .map((d) => (
                <div key={d.label} className="flex min-h-6 items-center justify-between gap-3 text-xs">
                  <dt className="shrink-0 text-muted-foreground">{d.label}</dt>
                  <dd className="flex min-w-0 items-center gap-1">
                    <span className={cn("truncate", d.copy && "font-mono text-[11px]")} title={d.value}>
                      {d.value}
                    </span>
                    {d.copy ? <IdCopy value={d.value} /> : null}
                  </dd>
                </div>
              ))}
            {scope?.onSetRights && agent && canManage(scope.role) ? <RightsEditor agent={agent} onSetRights={scope.onSetRights} /> : null}
            {/* Its tasks and posts with you, in the Agents tab. */}
            <Button
              size="sm"
              variant="ghost"
              className="-ml-2"
              data-agent-activity={i.handle}
              onClick={() => {
                onDone?.();
                void navigate({ to: "/agents", search: { agent: identityKey(i) } });
              }}
            >
              Activity
            </Button>
          </dl>
        ) : null}
      </div>
    </CardShell>
  );
}

/** The rights, in the plain words the card's Details uses. */
const RIGHT_WORDS: Record<string, string> = {
  task: "Can give it tasks",
  control: "Can stop it",
  approve: "Can approve its steps",
};
const RIGHT_LABELS = new Set(Object.values(RIGHT_WORDS));

function IdCopy({ value }: { value: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      aria-label={done ? "Copied" : `Copy ${value}`}
      data-card-id-copy=""
      onClick={() => {
        void navigator.clipboard?.writeText(value.replace(/^@/, "")).catch(() => undefined);
        setDone(true);
        setTimeout(() => setDone(false), 1200);
      }}
      className="inline-flex size-5 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      {done ? <CheckIcon className="size-3" /> : <CopyIcon className="size-3" />}
    </button>
  );
}

/* ---- Where the cards open ------------------------------------------------------ */

/**
 * Opens the card of `person` or `agent` on click. Without a Space's scope
 * (outside the redesigned Space page) a person has no card: the children
 * draw as they are.
 */
export function ProfileTrigger({
  person,
  agent,
  identity,
  children,
  className,
  side = "bottom",
  data,
  roster: ownRoster,
  onRemove,
}: {
  /** Where the page has no Space scope (the Agents tab): its roster and Remove. */
  roster?: Roster;
  onRemove?: (handle: string) => Promise<void>;
  person?: RosterPerson;
  agent?: RosterAgent;
  identity?: MemberIdentity | null;
  children: ReactNode;
  className?: string;
  side?: "bottom" | "left" | "top";
  data?: Record<string, string>;
}) {
  const scope = useProfileScope();
  const [open, setOpen] = useState(false);
  const who = identity ?? agent?.identity ?? null;
  if ((person && !scope) || (!person && !who)) return <>{children}</>;
  const roster = scope?.roster ?? ownRoster;
  const removeFn = scope?.onRemoveAgent ?? onRemove;
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger
        data-profile-trigger={person ? person.label : who!.handle}
        className={cn("cursor-default rounded-lg text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60", className)}
        {...data}
      >
        {children}
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Positioner side={side} align="start" sideOffset={6} className="z-[70]">
          <Popover.Popup data-video-occluder="" className="rounded-lg bg-popover text-popover-foreground shadow-float outline-none">
            {person ? (
              <PersonCard person={person} onDone={() => setOpen(false)} />
            ) : roster ? (
              <AgentCard
                identity={who!}
                agent={agent ?? roster.agents.find((a) => a.name === who!.handle)}
                roster={roster}
                onRemove={removeFn && roster.canRemove(who!) ? () => removeFn(who!.handle) : undefined}
                onDone={() => setOpen(false)}
              />
            ) : null}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** An @mention chip that opens its member's card (a plain chip when it is nobody here yet). */
export function MentionChip({ handle, children }: { handle: string; children: ReactNode }) {
  const scope = useProfileScope();
  const chip = (
    <span data-mention={handle} className="font-medium text-brand-strong">
      {children}
    </span>
  );
  if (!scope) return chip;
  const agent = scope.roster.agents.find((a) => a.name.toLowerCase() === handle);
  const person = agent ? undefined : scope.roster.people.find((p) => p.label.toLowerCase().replace(/\s+/g, "") === handle);
  if (!agent?.identity && !person) return chip;
  return (
    <ProfileTrigger agent={agent} person={person} className="hover:underline">
      {chip}
    </ProfileTrigger>
  );
}
