// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ArrowDownCircleIcon, EllipsisIcon, LoaderCircleIcon, PictureInPicture2Icon, PowerIcon, UserPlusIcon } from "lucide-react";
import type { ReactNode } from "react";

import type { SpaceOs } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { Button } from "@/components/ui/button";
import { Menu, type MenuItem } from "@/components/ui/menu";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export interface HeaderAction {
  label: string;
  help?: string;
  enabled: boolean;
  onClick: () => void;
}

/** The power icon: on (green, its label turns it off), off (gray, its
 * label turns it on), or pending (a start or stop under way; the label is
 * what is happening, "Starting…"). */
export interface HeaderPower {
  state: "on" | "off" | "pending";
  label: string;
  onClick: () => void;
}

export interface SpaceHeaderProps {
  back: ReactNode;
  os: SpaceOs;
  title: string;
  place: ReactNode;
  power?: HeaderPower;
  /** The desktop into its floating window. */
  pip?: HeaderAction & { id: string; active: boolean };
  teleport?: HeaderAction;
  share?: HeaderAction;
  remove?: HeaderAction;
  /** A failed create: take it off the list. */
  dismiss?: HeaderAction;
}

// The app's one radius, tonal fills, no outlines.
const filled = "h-8 border-transparent px-3 shadow-none";
const tonal = "bg-secondary text-secondary-foreground hover:bg-accent";
const accent = "bg-brand-surface text-foreground hover:bg-brand-surface/70 [&_svg]:text-brand-strong";
const bare = "text-muted-foreground hover:text-foreground [&_svg:not([class*='size-'])]:size-[18px]";

function PowerButton({ power }: { power: HeaderPower }) {
  const pending = power.state === "pending";
  return (
    <Tooltip content={power.label}>
      <Button
        variant="ghost"
        size="icon"
        aria-label={power.label}
        data-header-power={power.state}
        aria-busy={pending || undefined}
        // A pending change can't be pressed again; it stays hoverable for its tooltip.
        aria-disabled={pending || undefined}
        onClick={pending ? undefined : power.onClick}
        className={cn(bare, power.state === "on" && "text-power-on hover:text-power-on", pending && "opacity-60")}
      >
        {pending ? <LoaderCircleIcon className="size-[18px] animate-spin" /> : <PowerIcon />}
      </Button>
    </Tooltip>
  );
}

/** The Space page header: back on its own row, then the Space (a bare OS
 * mark, its name and where it runs), its icon actions, then Teleport and
 * Share. A hairline under it spans the page. */
export function SpaceHeader(p: SpaceHeaderProps) {
  const more: MenuItem[] = [
    ...(p.dismiss ? [{ label: p.dismiss.label, onSelect: p.dismiss.onClick, destructive: true }] : []),
    ...(p.remove ? [{ label: `${p.remove.label}…`, onSelect: p.remove.onClick, destructive: true }] : []),
  ];
  return (
    <header data-call-header="" className="border-b px-5 pt-2.5 pb-3.5">
      <div className="mb-2">{p.back}</div>
      <div className="flex items-center gap-3">
        <OsIcon os={p.os} className="size-9 shrink-0 text-foreground/80" />
        <div className="min-w-0 flex-1">
          <h1 data-detail-title className="truncate text-[20px] leading-tight font-semibold tracking-[-0.01em]">
            {p.title}
          </h1>
          <div className="[&>p]:mt-0.5">{p.place}</div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {p.power ? <PowerButton power={p.power} /> : null}
          {p.pip ? (
            <Tooltip content={p.pip.help ?? p.pip.label}>
              <Button
                variant="ghost"
                size="icon"
                data-pip={p.pip.id}
                aria-label={p.pip.label}
                aria-pressed={p.pip.active}
                disabled={!p.pip.enabled}
                onClick={p.pip.onClick}
                className={cn(bare, p.pip.active && "text-brand-strong")}
              >
                <PictureInPicture2Icon />
              </Button>
            </Tooltip>
          ) : null}
          {more.length ? (
            <Menu
              items={more}
              trigger={
                <Button variant="ghost" size="icon" aria-label="More actions" data-header-more="" className={bare}>
                  <EllipsisIcon />
                </Button>
              }
            />
          ) : null}
          {p.teleport || p.share ? <span className="w-1.5" /> : null}
          {p.teleport ? (
            <Button variant="secondary" className={cn(filled, tonal)} disabled={!p.teleport.enabled} title={p.teleport.help} onClick={p.teleport.onClick}>
              <ArrowDownCircleIcon className="size-3.5" /> {p.teleport.label}
            </Button>
          ) : null}
          {p.share ? (
            <Button variant="secondary" data-share-space="" className={cn(filled, accent)} disabled={!p.share.enabled} title={p.share.help} onClick={p.share.onClick}>
              <UserPlusIcon className="size-3.5" /> {p.share.label}
            </Button>
          ) : null}
        </div>
      </div>
    </header>
  );
}
