// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { LaptopIcon } from "lucide-react";

import type { Space, SpaceOs } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { t } from "@/i18n";
import type { TaskWhere } from "@/lib/ask-first";
import { thisComputerLabel } from "@/lib/host-labels";
import { cn } from "@/lib/utils";

/** A computer `@` can pick in the prompt box: where the job then runs. */
export interface AtPlace {
  id: string;
  name: string;
  os: SpaceOs;
  online: boolean;
  /** This computer. */
  current: boolean;
  where: TaskWhere;
}

/** A computer the person can name for the agent to use (agent homes). */
export interface NameableMachine {
  id: string;
  name: string;
  current: boolean;
  online: boolean;
  os?: SpaceOs;
}

/** What the `@` picker offers: with agent homes your named computers, else this computer and your other online ones. */
export function atPlaces({
  agentHome,
  machines = [],
  computers = [],
}: {
  agentHome?: boolean;
  machines?: readonly NameableMachine[];
  computers?: readonly Space[];
}): AtPlace[] {
  const list: AtPlace[] = agentHome
    ? machines.map((m) => ({
        id: m.id,
        name: m.current ? thisComputerLabel() : m.name,
        os: m.os ?? "unknown",
        online: m.online,
        current: m.current,
        where: m.current ? { kind: "this-mac" } : { kind: "machine", machine: { id: m.id, name: m.name } },
      }))
    : [
        { id: "this-mac", name: thisComputerLabel(), os: "macos", online: true, current: true, where: { kind: "this-mac" } },
        ...computers.map((c): AtPlace => ({ id: c.id, name: c.name, os: c.os, online: true, current: false, where: { kind: "space", space: c } })),
      ];
  // Online first, this computer leading, the order given otherwise.
  return [...list.filter((p) => p.online), ...list.filter((p) => !p.online)].sort(
    (a, b) => Number(b.current) - Number(a.current) || Number(b.online) - Number(a.online),
  );
}

const slug = (name: string) =>
  name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "computer";

/** The places whose name starts with (or has a word starting with) what follows `@`. */
export function matchPlaces(query: string, places: readonly AtPlace[]): AtPlace[] {
  const q = query.toLowerCase();
  if (!q) return [...places];
  return places.filter(
    (p) =>
      p.name
        .toLowerCase()
        .split(/[\s-]+/)
        .some((w) => w.startsWith(q)) || slug(p.name).startsWith(q),
  );
}

/** The small list under `@`: your computers, online first, offline greyed, this one labelled. */
export function AtPicker({
  places,
  index,
  onIndex,
  onPick,
  side,
  filtered,
}: {
  places: readonly AtPlace[];
  index: number;
  onIndex: (i: number) => void;
  onPick: (p: AtPlace) => void;
  side: "up" | "down";
  /** Nothing matched what was typed (rather than no computers at all). */
  filtered: boolean;
}) {
  return (
    <div
      role="listbox"
      aria-label={t("home.ask_box.at_label")}
      data-at-picker=""
      className={cn(
        "absolute left-4 z-20 w-72 overflow-hidden rounded-lg bg-popover p-1 text-popover-foreground shadow-float ring-1 ring-foreground/10",
        side === "up" ? "bottom-full mb-2" : "top-full mt-2",
      )}
    >
      {places.length === 0 ? (
        <p data-at-none="" className="px-2 py-2 text-[13px] text-muted-foreground">
          {filtered ? t("home.ask_box.at_none_match") : t("home.ask_box.at_none")}
        </p>
      ) : (
        places.map((p, i) => (
          <div
            key={p.id}
            role="option"
            aria-selected={i === index}
            aria-disabled={!p.online || undefined}
            data-at-option={p.id}
            data-at-offline={p.online ? undefined : ""}
            onMouseDown={(e) => {
              e.preventDefault();
              if (p.online) onPick(p);
            }}
            onMouseEnter={() => onIndex(i)}
            className={cn("flex h-9 cursor-default items-center gap-2.5 rounded-lg px-2 text-[13px]", i === index && "bg-accent", !p.online && "opacity-50")}
          >
            {p.current ? (
              <LaptopIcon className="size-4 shrink-0 text-muted-foreground" />
            ) : (
              <OsIcon os={p.os} className="size-4 shrink-0 text-muted-foreground" />
            )}
            <span className="min-w-0 flex-1 truncate">{p.name}</span>
            {!p.online ? <span className="text-xs text-muted-foreground">{t("home.ask_box.off")}</span> : null}
          </div>
        ))
      )}
    </div>
  );
}
