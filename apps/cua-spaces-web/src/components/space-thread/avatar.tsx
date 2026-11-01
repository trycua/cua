// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { MemberIdentity, MemberRef } from "@/bridge";
import { cn } from "@/lib/utils";

import { HarnessMark } from "./harness-marks";

/** People: a hue from the name, so each keeps the same colour everywhere. */
const PERSON_HUES = [45, 305, 265, 155, 20, 200] as const;
/** Agents: the harness's own colour where it has one. Only the fallback now: a harness's mark fills the tile where there is one. */
const AGENT_HUES: Record<string, number> = { "claude-code": 190, hermes: 330, "openai-codex": 250, codex: 250, openclaw: 25, "gemini-cli": 240, goose: 60 };

function hash(s: string): number {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h;
}

export function personHue(name: string): number {
  return PERSON_HUES[hash(name.toLowerCase()) % PERSON_HUES.length]!;
}

export function agentHue(harness: string, name: string): number {
  return AGENT_HUES[harness] ?? (hash(name) % 360);
}

/** What an agent's owner badge is cut out of: the surface the avatar sits on. */
export type AvatarSurface = "background" | "chrome" | "popover" | "muted";

const SURFACE_RING: Record<AvatarSurface, string> = {
  background: "border-background",
  chrome: "border-chrome",
  popover: "border-popover",
  // An approval's card: `bg-muted/50` over the page.
  muted: "border-[color-mix(in_oklab,var(--muted)_50%,var(--background))]",
};

/** A person's round initial, on their hue. */
export function PersonDot({ name, px, className }: { name: string; px: number; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn("inline-flex shrink-0 items-center justify-center rounded-full font-semibold text-white select-none", className)}
      style={{ width: px, height: px, fontSize: Math.max(7, Math.round(px * 0.5)), backgroundColor: `oklch(0.62 0.14 ${personHue(name)})` }}
    >
      {(name.trim().charAt(0) || "?").toUpperCase()}
    </span>
  );
}

/** Whose agent it is: the owner's initial at half the avatar's size, in its corner, cut out of the surface. Drawn from the member's record; an agent can't set or hide it. */
export function OwnerBadge({ owner, size, surface = "background" }: { owner: MemberRef; size: number; surface?: AvatarSurface }) {
  const px = Math.round(size / 2);
  return (
    <span data-owner-badge={owner.id} title={`${owner.name}'s agent`} className="absolute" style={{ right: -Math.round(px / 3), bottom: -Math.round(px / 3) }}>
      <PersonDot name={owner.name} px={px + 3} className={cn("border-[1.5px]", SURFACE_RING[surface])} />
    </span>
  );
}

/**
 * An agent's tile: its harness's mark (or the icon it set, or the bot glyph
 * on its colour), with its owner's badge in the corner when it has an
 * identity. `dim` draws an asleep or paused agent quieter; the badge stays.
 */
export function AgentTile({
  harness,
  icon,
  owner,
  name,
  px,
  dim,
  surface,
  className,
}: {
  harness: string;
  icon?: string | null;
  owner?: MemberRef | null;
  /** For the fallback colour. */
  name: string;
  px: number;
  dim?: boolean;
  surface?: AvatarSurface;
  className?: string;
}) {
  return (
    <span data-avatar="agent" aria-hidden className={cn("relative inline-flex shrink-0 select-none", className)} style={{ width: px, height: px }}>
      <span data-agent-tile="" className={cn("block overflow-hidden", px >= 24 ? "rounded-lg" : "rounded-[5px]", dim && "opacity-45")}>
        <HarnessMark harness={harness} icon={icon} px={px} hue={agentHue(harness, name)} />
      </span>
      {owner ? <OwnerBadge owner={owner} size={px} surface={surface} /> : null}
    </span>
  );
}

/**
 * A member's mark: a person's initial in a round badge; an agent's tile
 * (`AgentTile`). `dim` draws an asleep or paused agent quieter.
 * `presence` (P2) adds the online dot; nothing draws one without it.
 */
export function MemberAvatar({
  kind,
  name,
  harness = "",
  identity,
  surface,
  size = 28,
  dim,
  presence,
  className,
}: {
  kind: "person" | "agent";
  name: string;
  harness?: string;
  /** An agent's identity: its icon and its owner's badge. */
  identity?: MemberIdentity | null;
  surface?: AvatarSurface;
  size?: 20 | 24 | 28;
  dim?: boolean;
  presence?: "online" | null;
  className?: string;
}) {
  const dot = presence === "online" ? <span data-presence="" className="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-background bg-success" /> : null;
  if (kind === "agent") {
    return (
      <span className={cn("relative inline-flex shrink-0", className)}>
        <AgentTile harness={identity?.harness.id || harness} icon={identity?.icon} owner={identity?.owner} name={name} px={size} dim={dim} surface={surface} />
        {identity ? null : dot}
      </span>
    );
  }
  const box = size === 20 ? "size-5 text-[10px]" : size === 24 ? "size-6 text-[11px]" : "size-7 text-xs";
  return (
    <span
      data-avatar={kind}
      aria-hidden
      className={cn("relative inline-flex shrink-0 items-center justify-center rounded-full font-semibold text-white select-none", box, className)}
      style={{ backgroundColor: `oklch(0.62 0.14 ${personHue(name)})` }}
    >
      {(name.trim().charAt(0) || "?").toUpperCase()}
      {dot}
    </span>
  );
}
