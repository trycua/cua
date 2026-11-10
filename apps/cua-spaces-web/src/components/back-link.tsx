// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { ChevronLeftIcon } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * "‹ Computers" above a page opened from the list of your computers (a
 * Space, a computer, a cloud), or "‹ This Mac" above one of its settings.
 * `inline`: in a header row, not above the page.
 */
export function BackLink({
  inline,
  computerId,
  label = "Computers",
  parent,
}: {
  inline?: boolean;
  computerId?: string;
  label?: string;
  parent?: { id: string; name: string } | null;
}) {
  const className = cn(
    "-ml-1 inline-flex h-6 shrink-0 items-center gap-0.5 rounded-lg pr-1.5 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60",
    inline ? "mr-1" : "mb-4",
  );
  const content = (
    <>
      <ChevronLeftIcon className="size-3.5" /> {parent ? parent.name : label}
    </>
  );
  // A child Space goes back to the Space that made it.
  if (parent) {
    return (
      <Link to="/spaces/$spaceId" params={{ spaceId: parent.id }} className={className} data-back-link="" data-back-to={parent.id}>
        {content}
      </Link>
    );
  }
  return computerId ? (
    <Link to="/spaces/computer/$computerId" params={{ computerId }} className={className} data-back-link="">
      {content}
    </Link>
  ) : (
    <Link to="/spaces" className={className} data-back-link="">
      {content}
    </Link>
  );
}
