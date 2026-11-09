// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space's child Spaces, in its side panel: each with its OS logo, name and where it is (running,
 * being made with its progress, failed with why), a row menu (open, stop or start, delete), how many of its
 * tree's budget they use. Deleting is per child, from its row menu, with a confirmation that names what goes.
 * Nothing shows for a Space without children.
 */

import { Link } from "@tanstack/react-router";
import { MoreHorizontalIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { useSpaces, type Space } from "@/bridge";
import { useChildren } from "@/components/children/use-children";
import { OsIcon } from "@/components/os-icon";
import { StateLabel } from "@/components/state-dot";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Menu } from "@/components/ui/menu";
import { SectionHeading } from "@/components/space-thread/section-heading";
import { toastError } from "@/components/ui/toast";
import { plural } from "@/lib/plural";
import { cn } from "@/lib/utils";
import { parentIdOf } from "@/lib/space-tree";
import { useSpaceLayoutStore } from "@/stores/space-layout";
import { createError, spaceState } from "@/lib/spaces";

/** The Spaces made under `parentId`, in the order the list has them. */
export function childrenOfSpace(spaces: readonly Space[], parentId: string, tree: Parameters<typeof parentIdOf>[1]): Space[] {
  return spaces.filter((s) => s.id !== parentId && parentIdOf(s, tree) === parentId);
}

/** How many Spaces sit under `parentId` at any depth (what deleting it would take along). */
export function descendantCount(spaces: readonly Space[], parentId: string, tree: Parameters<typeof parentIdOf>[1], seen = new Set<string>()): number {
  let n = 0;
  for (const c of childrenOfSpace(spaces, parentId, tree)) {
    if (seen.has(c.id)) continue;
    seen.add(c.id);
    n += 1 + descendantCount(spaces, c.id, tree, seen);
  }
  return n;
}

/** The confirm's words for deleting `kids` (and what is under them). */
export function deleteAllText(parentName: string, kids: readonly Pick<Space, "name">[], total: number): { title: string; description: string } {
  const names = kids.map((k) => k.name).join(", ");
  const deeper = total - kids.length;
  return {
    title: `Delete all ${plural(kids.length, "child Space")} of ${parentName}?`,
    description:
      `${names} ${kids.length === 1 ? "is" : "are"} deleted for good, with ${kids.length === 1 ? "its" : "their"} files and apps.` +
      (deeper > 0 ? ` ${plural(deeper, "Space")} made under ${kids.length === 1 ? "it" : "them"} go too.` : "") +
      ` ${parentName} stays.`,
  };
}

/**
 * What stopping or deleting a Space does to its children, to add to that confirm: deleting takes every Space under it
 * (named, with how many are deeper); stopping leaves them running. Empty when it has none.
 */
export function childrenEffect(kind: "stop" | "delete", kids: readonly Pick<Space, "name">[], total: number): string {
  if (!kids.length) return "";
  const names = kids.map((k) => k.name).join(", ");
  if (kind === "stop")
    return ` Its ${plural(kids.length, "child Space")} (${names}) keep${kids.length === 1 ? "s" : ""} running. Stop ${kids.length === 1 ? "it" : "them"} from the Child Spaces list if you want.`;
  const deeper = total - kids.length;
  return ` Its ${plural(kids.length, "child Space")} (${names}) ${kids.length === 1 ? "is" : "are"} deleted with it${deeper > 0 ? `, and so ${deeper === 1 ? "is the 1 Space" : `are the ${deeper} Spaces`} made under ${kids.length === 1 ? "it" : "them"}` : ""}.`;
}

export function ChildSpacesSection({ spaceId, spaceName }: { spaceId: string; spaceName: string }) {
  const tree = useChildren();
  const { data, startSpace, stopSpace, deleteSpaces } = useSpaces();
  const all = data ?? [];
  const kids = useMemo(() => childrenOfSpace(all, spaceId, tree), [all, spaceId, tree]);
  const total = useMemo(() => descendantCount(all, spaceId, tree), [all, spaceId, tree]);
  const [confirm, setConfirm] = useState<"all" | string | null>(null);
  const folded = useSpaceLayoutStore((st) => st.folded.children);
  if (!kids.length) return null;
  const budget = tree?.budget(spaceId) ?? null;
  const one = confirm && confirm !== "all" ? kids.find((k) => k.id === confirm) : null;
  const text = confirm === "all" ? deleteAllText(spaceName, kids, total) : one ? deleteAllText(spaceName, [one], 1 + descendantCount(all, one.id, tree)) : null;
  const remove = (ids: string[]) => void deleteSpaces(ids).catch(toastError("Couldn't delete the child Spaces"));
  return (
    <section className={cn("px-3", !folded && "pb-2")} data-child-spaces="">
      <div className={cn("-mx-3 flex h-8 items-center hover:bg-accent", !folded && "mb-1")}>
        <SectionHeading id="children" folded={folded} bodyId="section-children" inRow>
          Child Spaces
        </SectionHeading>
      </div>
      <div id="section-children" hidden={folded}>
        <ul>
          {kids.map((k) => {
            const state = spaceState(k);
            const off = state === "stopped";
            const busy = state === "creating" || state === "deleting";
            return (
              <li key={k.id} data-child-space-row={k.id} className="group/child relative">
                <Link
                  to="/spaces/$spaceId"
                  params={{ spaceId: k.id }}
                  className="flex min-h-8 w-full cursor-default items-center gap-2.5 rounded-lg py-1 pr-2 pl-2 text-left text-[13px] outline-none hover:bg-accent focus-visible:bg-accent"
                >
                  <OsIcon os={k.os} className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate">{k.name}</span>
                    {createError(k) ? <span className="block truncate text-[11px] text-destructive">{createError(k)}</span> : null}
                  </span>
                  <StateLabel state={state} space={k} className="shrink-0" />
                </Link>
                {!busy ? (
                  <span className="absolute top-1 right-1 rounded-lg bg-accent opacity-0 group-focus-within/child:opacity-100 group-hover/child:opacity-100">
                    <Menu
                      label={k.name}
                      trigger={
                        <button
                          type="button"
                          aria-label={`${k.name} actions`}
                          className="inline-flex size-6 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-background hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                        >
                          <MoreHorizontalIcon className="size-3.5" />
                        </button>
                      }
                      items={[
                        off
                          ? { label: "Start", onSelect: () => void startSpace(k.id).catch(toastError(`Couldn't start ${k.name}`)) }
                          : { label: "Stop", onSelect: () => void stopSpace(k.id).catch(toastError(`Couldn't stop ${k.name}`)) },
                        { label: "Delete", destructive: true, onSelect: () => setConfirm(k.id) },
                      ]}
                    />
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
        {budget ? (
          <p data-child-budget="" className="mt-1 px-2 text-[11px] text-muted-foreground">
            {budget.spaces} of {budget.maxSpaces} Spaces in this tree&rsquo;s budget
          </p>
        ) : null}
      </div>
      {text ? (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setConfirm(null)}
          title={text.title}
          description={text.description}
          confirmLabel="Delete"
          destructive
          cancelFirst
          onConfirm={() => remove(confirm === "all" ? kids.map((k) => k.id) : [confirm!])}
        />
      ) : null}
    </section>
  );
}
