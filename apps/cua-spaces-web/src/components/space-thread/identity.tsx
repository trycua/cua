// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Who an agent in the thread is (CUA-2262): the quiet AI tag after its
 * name, its owner after the tag when another agent uses the same name, and
 * the card that hovering or clicking the name opens. The agent sets its
 * name, icon and the harness it reports; the owner, the computer it posts
 * from and how it joined come from its member record, so the card marks
 * those two as checked by Cua.
 */

import { Popover } from "@base-ui/react/popover";
import { useNavigate } from "@tanstack/react-router";
import { ShieldCheckIcon } from "lucide-react";
import { useState, type ReactNode } from "react";

import type { MemberEvent, MemberIdentity, RightHolders } from "@/bridge";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

import { AgentTile, PersonDot, type AvatarSurface } from "./avatar";
import { harnessLabel } from "./harness-marks";
import { RIGHT_ROWS } from "./rights";
import type { Roster, RosterAgent, RosterAuthor } from "./roster";

/** After an agent's name: says it is an AI. Quiet: a 15 px fill, no outline, the one 4 px radius. */
export function AiTag({ className }: { className?: string }) {
  return (
    <span
      data-ai-tag=""
      aria-label="AI agent"
      className={cn("inline-flex h-[15px] shrink-0 items-center rounded-[4px] bg-foreground/[0.08] px-1 text-[9.5px] font-bold tracking-[0.04em] text-foreground/60", className)}
    >
      AI
    </span>
  );
}

/** One agent across Spaces (the Agents tab's row): the same owner, harness and name (`chatKey` in the bridge). */
export const identityKey = (i: MemberIdentity) => `${i.owner.id}/${i.harness.id || "other"}/${i.displayName.trim().toLowerCase()}`;

/** "(Grace)" after the tag: two agents here use the same name. */
export function OwnerSuffix({ identity }: { identity: MemberIdentity }) {
  return (
    <span data-owner-suffix={identity.owner.id} className="shrink-0 text-[12px] text-muted-foreground">
      ({identity.owner.name})
    </span>
  );
}

/** When it joined: "3 minutes ago", "Today 08:12", "Yesterday 16:05", "Tuesday 09:40", "Oct 3". */
export function joinedWhen(atMs: number, now = Date.now()): string {
  const mins = Math.round((now - atMs) / 60_000);
  if (mins < 1) return "Just now";
  if (mins < 60) return mins === 1 ? "1 minute ago" : `${mins} minutes ago`;
  const d = new Date(atMs);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(new Date(now)) - day(d)) / 86_400_000);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  if (days <= 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  if (days < 7) return `${d.toLocaleDateString([], { weekday: "long" })} ${time}`;
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

/** How it joined, in words. */
export function joinedHow(i: MemberIdentity): string {
  const by = i.joined.by?.name ?? i.owner.name;
  if (i.joined.method === "prompt") return `Connected by ${by} with a prompt`;
  if (i.joined.method === "setup") return `Set up by ${by}`;
  return "Started in this Space";
}

/** The harness ("Codex CLI 0.61.0"): as a connected agent reported it, or as Cua started it; null when a connected agent didn't say. */
export function reportedHarness(i: MemberIdentity): string | null {
  const known = i.origin === "space" || i.harness.reported;
  if (!known || !i.harness.id || i.harness.id === "other") return null;
  const name = harnessLabel(i.harness.id) ?? i.harness.id.charAt(0).toUpperCase() + i.harness.id.slice(1);
  return i.harness.version ? `${name} ${i.harness.version}` : name;
}

/** Named holders by name (the agent's owner, people the roster knows), or null when one isn't known. */
export function holderNames(ids: readonly string[], i: MemberIdentity | null, roster: Pick<Roster, "people">): string | null {
  const names = ids.map((id) => (i && id === i.owner.id ? i.owner.name : roster.people.find((p) => p.id === id)?.label));
  return names.length && names.every(Boolean) ? names.join(", ") : null;
}

/** Who holds a right, in words: a group, or the people by name. */
export function holdersName(h: RightHolders, i: MemberIdentity | null, roster: Pick<Roster, "people">): string {
  if (!Array.isArray(h)) return h === "owner" ? "Owner" : h === "admins" ? "Admins" : "Members";
  return holderNames(h, i, roster) ?? (h.length === 1 ? "1 person" : `${h.length} people`);
}

const Checked = () => <ShieldCheckIcon role="img" aria-label="Checked by Cua" className="size-3.5 shrink-0 text-success" />;

function Fact({ label, children, title }: { label: string; children: ReactNode; title?: string }) {
  return (
    <div data-card-fact={label} className="flex min-h-7 items-center justify-between gap-3 text-[12.5px]">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="flex min-w-0 items-center gap-1.5 text-right" title={title}>
        {children}
      </span>
    </div>
  );
}

/**
 * The agent's card: the owner and the computer (checked by Cua), how and
 * when it joined, its harness as it reported it, who can task, stop and
 * approve it, and Remove from Space for whoever may.
 */
export function ProvenanceCard({
  identity: i,
  agent,
  roster,
  onRemove,
  onActivity,
}: {
  identity: MemberIdentity;
  /** The member, while it is in the Space (its rights and status). */
  agent?: RosterAgent;
  roster: Roster;
  /** Removes it; absent when you can't. */
  onRemove?: () => Promise<void>;
  /** Opens its DM in the Agents tab (its tasks and posts with you). */
  onActivity?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const harness = reportedHarness(i);
  const you = roster.me !== null && i.owner.id === roster.me;
  const remove = () => {
    if (!onRemove) return;
    setBusy(true);
    onRemove().then(
      () => setBusy(false),
      (e: unknown) => {
        setBusy(false);
        toast(`Couldn't remove ${i.displayName}`, { description: e instanceof Error ? e.message : String(e), type: "error" });
      },
    );
  };
  return (
    <div data-provenance={i.handle} className="p-3.5">
      <div className="flex items-center gap-3">
        <AgentTile harness={i.harness.id} icon={i.icon} owner={i.owner} name={i.handle} px={36} surface="popover" />
        <span className="min-w-0">
          <span className="flex items-center gap-1.5">
            <span className="truncate text-[14px] font-semibold">{i.displayName}</span>
            <AiTag />
          </span>
          <span className="block truncate text-xs text-muted-foreground">@{i.handle}</span>
        </span>
      </div>
      <div className="mt-3">
        <Fact label="Owner">
          <PersonDot name={i.owner.name} px={16} />
          <span className="truncate">{you ? `${i.owner.name} (you)` : i.owner.name}</span>
          <Checked />
        </Fact>
        <Fact label="Posts from">
          <span className="truncate">{i.device?.name ?? (i.origin === "space" ? "This Space" : "Not known")}</span>
          <Checked />
        </Fact>
        <Fact label="Joined">
          <span className="truncate">{joinedHow(i)}</span>
        </Fact>
        {i.joined.atMs !== null ? (
          <Fact label="When" title={new Date(i.joined.atMs).toLocaleString()}>
            {joinedWhen(i.joined.atMs)}
          </Fact>
        ) : null}
        <Fact label="Harness" title={!harness ? `${i.displayName} didn't say which harness it runs` : i.origin === "external" ? `As ${i.displayName} reported it` : "Cua started it"}>
          {harness ? <span className="truncate">{harness}</span> : <span className="text-muted-foreground">Not reported</span>}
        </Fact>
      </div>
      {agent ? (
        <div className="mt-2.5">
          {RIGHT_ROWS.map(({ right, label }) => (
            <Fact key={right} label={label}>
              {holdersName(agent.rights[right], i, roster)}
            </Fact>
          ))}
        </div>
      ) : null}
      <p className="mt-2.5 text-[11.5px] leading-snug text-muted-foreground">
        {harness && i.origin === "external" ? "Name, icon and harness are set by the agent." : "The name and icon are set by the agent."} Owner and computer are checked by Cua.
      </p>
      {onRemove || onActivity ? (
        <div className="mt-3 flex items-center">
          {onActivity ? (
            <Button size="sm" variant="ghost" className="-ml-2" data-agent-activity={i.handle} onClick={onActivity}>
              Activity
            </Button>
          ) : null}
          <span className="flex-1" />
          {onRemove ? (
            <Button size="sm" variant="ghost" className="-mr-2 text-destructive hover:text-destructive" disabled={busy} data-remove-agent={i.handle} onClick={remove}>
              {busy ? "Removing…" : "Remove from Space"}
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/**
 * An agent's name in the thread: the name it chose, the AI tag, and its
 * owner when another agent shares the name. Hover or click the name for
 * its card. Without an identity (an older daemon) it is the name and tag.
 */
export function AgentName({ who, roster, onRemove, size = "md" }: { who: RosterAuthor; roster: Roster; onRemove?: (handle: string) => Promise<void>; size?: "md" | "sm" }) {
  const i = who.identity;
  const text = cn("min-w-0 truncate font-semibold", size === "md" ? "text-[13px]" : "text-[12.5px]");
  const may = !!onRemove && !!i && !!who.agent && roster.canRemove(i);
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      {i ? (
        <Popover.Root open={open} onOpenChange={setOpen}>
          <Popover.Trigger
            openOnHover
            delay={350}
            data-author-name={who.handle}
            className={cn(text, "cursor-default rounded-[4px] outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring/60")}
          >
            {who.label}
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Positioner side="bottom" align="start" sideOffset={6} className="z-[70]">
              <Popover.Popup data-video-occluder="" className="w-[300px] rounded-lg bg-popover text-popover-foreground shadow-float outline-none">
                <ProvenanceCard
                  identity={i}
                  agent={who.agent}
                  roster={roster}
                  onRemove={may ? () => onRemove(i.handle).then(() => setOpen(false)) : undefined}
                  onActivity={() => {
                    setOpen(false);
                    void navigate({ to: "/agents", search: { agent: identityKey(i) } });
                  }}
                />
              </Popover.Popup>
            </Popover.Positioner>
          </Popover.Portal>
        </Popover.Root>
      ) : (
        <span className={text}>{who.label}</span>
      )}
      <AiTag />
      {who.collides && i ? <OwnerSuffix identity={i} /> : null}
    </span>
  );
}

/** A member joined or left, as the thread shows it. Null for events it doesn't draw (the text shows instead). */
export function MemberEventNote({ event, text, tsMs, surface = "background" }: { event: MemberEvent; text: string; tsMs: number; surface?: AvatarSurface }) {
  const i = event.member;
  if (!i) return null;
  const time = <span className="shrink-0 tabular-nums">{new Date(tsMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>;
  if (event.kind === "member_joined") {
    const by = i.joined.by ?? i.owner;
    const from = i.device ? ` from ${i.device.name}` : "";
    return (
      <div data-member-note="joined" data-member-handle={i.handle} className="my-1 flex items-center justify-center gap-2 text-xs text-muted-foreground">
        <AgentTile harness={i.harness.id} icon={i.icon} name={i.handle} px={16} surface={surface} />
        <span className="min-w-0 truncate">
          <span className="font-medium text-foreground">{i.displayName}</span> joined. {by.name} {i.joined.method === "started" ? "started it here" : `connected it${from}`}.
        </span>
        {time}
      </div>
    );
  }
  if (event.kind === "member_removed") {
    const by = event.by;
    return (
      <p data-member-note="removed" data-member-handle={i.handle} className="my-1 truncate text-center text-xs text-muted-foreground">
        {by ? `${by.name} removed ${i.displayName}.` : `${i.displayName} left.`}
        {i.origin === "external" ? " Its key no longer works here." : ""} {time}
      </p>
    );
  }
  return (
    <p data-member-note={event.kind} className="my-1 truncate text-center text-xs text-muted-foreground">
      {text} {time}
    </p>
  );
}
