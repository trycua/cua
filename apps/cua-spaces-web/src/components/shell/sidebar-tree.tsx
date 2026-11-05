// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { ChevronRightIcon } from "lucide-react";

import { TreeDot, TreeIcon, toggleCollapsed, useCollapsed } from "@/components/spaces/tree-parts";
import type { SpacesTree, TreeNode } from "@/lib/spaces-tree";
import { cn } from "@/lib/utils";

const row =
  "group flex h-7 min-w-0 flex-1 items-center gap-2 rounded-lg px-2 text-[13px] outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-[status=active]:bg-foreground/[0.07] data-[status=active]:font-medium dark:data-[status=active]:bg-white/[0.08]";

function NodeLink({ node, depth }: { node: TreeNode; depth: 0 | 1 }) {
  const body = (
    <>
      <TreeIcon node={node} className={cn(node.dim && "opacity-50")} />
      <span className={cn("min-w-0 flex-1 truncate", node.dim && "text-muted-foreground")}>{node.name}</span>
      {node.isDefault ? (
        <span data-tree-default="" className="shrink-0 text-2xs text-muted-foreground">
          Default
        </span>
      ) : null}
      <TreeDot tone={node.tone} />
    </>
  );
  const cls = cn(row, depth === 1 && "pl-7");
  // A computer that shares no desktop has no page of its own: its Machines page.
  return node.space ? (
    <Link to="/spaces/$spaceId" params={{ spaceId: node.space.id }} className={cls} data-sidebar-tree-row={node.key}>
      {body}
    </Link>
  ) : (
    <Link to="/machines" className={cls} data-sidebar-tree-row={node.key}>
      {body}
    </Link>
  );
}

/**
 * The sidebar's Spaces as a tree (Labs, CUA-3413): each computer, with the
 * Spaces made on it under it. A computer with Spaces folds and unfolds.
 */
export function SidebarTree({ tree }: { tree: SpacesTree }) {
  const collapsed = useCollapsed();
  if (tree.roots.length === 0) return null;
  return (
    <ul className="flex flex-col gap-px pb-1" data-sidebar-tree="" aria-label="Your Spaces">
      {tree.roots.map((node) => {
        const folded = collapsed.has(node.key);
        return (
          <li key={node.key}>
            <div className="relative flex items-center">
              <NodeLink node={node} depth={0} />
              {node.children.length > 0 ? (
                <button
                  type="button"
                  aria-label={folded ? `Show the Spaces on ${node.name}` : `Hide the Spaces on ${node.name}`}
                  aria-expanded={!folded}
                  data-sidebar-tree-fold={node.key}
                  onClick={() => toggleCollapsed(node.key)}
                  className="absolute -left-2 flex size-4 items-center justify-center rounded-lg text-muted-foreground opacity-0 outline-none hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 [li:hover_&]:opacity-100"
                >
                  <ChevronRightIcon className={cn("size-3 transition-transform", !folded && "rotate-90")} />
                </button>
              ) : null}
            </div>
            {node.children.length > 0 && !folded ? (
              <ul className="flex flex-col gap-px">
                {node.children.map((c) => (
                  <li key={c.key} className="flex">
                    <NodeLink node={c} depth={1} />
                  </li>
                ))}
              </ul>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
