// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { HardDriveIcon, LaptopIcon, MonitorIcon } from "lucide-react";
import { useSyncExternalStore } from "react";

import { OsIcon } from "@/components/os-icon";
import { avatarColor, initial } from "@/lib/spaces-page";
import type { TreeNode, TreeTone } from "@/lib/spaces-tree";
import { cn } from "@/lib/utils";

/** A row's mark: This Mac's laptop, a Mac mini's box, a PC's screen; a Space's OS. */
export function TreeIcon({ node, className }: { node: Pick<TreeNode, "kind" | "current" | "os">; className?: string }) {
  const cls = cn("size-4 shrink-0 text-muted-foreground", className);
  if (node.kind === "space") return node.os ? <OsIcon os={node.os} className={cls} /> : <MonitorIcon className={cls} strokeWidth={1.75} aria-hidden />;
  if (node.current) return <LaptopIcon className={cls} strokeWidth={1.75} aria-hidden />;
  if (node.os === "macos") return <HardDriveIcon className={cls} strokeWidth={1.75} aria-hidden />;
  return <MonitorIcon className={cls} strokeWidth={1.75} aria-hidden />;
}

const TONE: Record<TreeTone, string> = {
  online: "bg-success",
  busy: "bg-brand animate-pulse",
  offline: "bg-muted-foreground/50",
  stopped: "bg-muted-foreground/50",
};

export function TreeDot({ tone, className }: { tone: TreeTone; className?: string }) {
  return <span data-tree-dot={tone} aria-hidden className={cn("size-1.5 shrink-0 rounded-full", TONE[tone], className)} />;
}

/** Who's on it: up to three initials, then "+2". */
export function PeopleFaces({ people, className }: { people: readonly string[]; className?: string }) {
  if (people.length === 0) return null;
  const more = people.length - 3;
  return (
    <span data-people={people.length} title={people.join(", ")} className={cn("inline-flex items-center", className)}>
      {people.slice(0, 3).map((name, i) => (
        <span
          key={`${name}:${i}`}
          aria-hidden
          className={cn(
            "flex size-5 items-center justify-center rounded-full text-[10px] font-semibold text-white ring-2 ring-background",
            avatarColor(name),
            i > 0 && "-ml-1",
          )}
        >
          {initial(name)}
        </span>
      ))}
      {more > 0 ? <span className="ml-1 text-[11px] text-muted-foreground tabular-nums">+{more}</span> : null}
      <span className="sr-only">{people.join(", ")}</span>
    </span>
  );
}

/* ---- Which computers are folded (this device only) ------------------------ */

const KEY = "cua.spacesTree.collapsed";
const listeners = new Set<() => void>();
let cache: ReadonlySet<string> | null = null;

function load(): ReadonlySet<string> {
  if (cache) return cache;
  try {
    const raw = JSON.parse(globalThis.localStorage?.getItem(KEY) ?? "[]") as unknown;
    cache = new Set(Array.isArray(raw) ? raw.filter((x): x is string => typeof x === "string") : []);
  } catch {
    cache = new Set();
  }
  return cache;
}

/** Folds or unfolds a computer on the Computers page (remembered on this device). */
export function toggleCollapsed(key: string): void {
  const next = new Set(load());
  if (next.has(key)) next.delete(key);
  else next.add(key);
  cache = next;
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify([...next]));
  } catch {
    // Kept for this session only.
  }
  for (const l of [...listeners]) l();
}

export function useCollapsed(): ReadonlySet<string> {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    load,
    load,
  );
}

/** For tests: forget the cached folds. */
export function resetCollapsed(): void {
  cache = null;
}
