// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * `contained`: the page does not scroll as a whole. The header stays put and
 * the page's own panes (a list and its detail) scroll on their own, each at
 * the full height that is left.
 */
export function Page({ children, className, contained }: { children: ReactNode; className?: string; contained?: boolean }) {
  if (contained) {
    return (
      <div className="h-full overflow-hidden" data-page-contained="">
        <div className={cn("mx-auto flex h-full w-full max-w-5xl flex-col px-8 pt-6", className)}>{children}</div>
      </div>
    );
  }
  // The gutter is always reserved: otherwise a page that scrolls (Settings >
  // General) is narrower than one that doesn't, and the centred column jumps
  // sideways between them.
  return (
    <div className="h-full overflow-y-auto [scrollbar-gutter:stable]">
      <div className={cn("mx-auto w-full max-w-5xl px-8 pt-6 pb-16", className)}>{children}</div>
    </div>
  );
}

export function PageHeader({ title, description, actions }: { title: string; description?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="mb-6 flex items-end justify-between gap-4">
      <div className="min-w-0">
        <h1 className="text-[22px] font-semibold tracking-[-0.01em]">{title}</h1>
        {description ? <p className="mt-1 text-[13px] text-muted-foreground">{description}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 items-center gap-2">{actions}</div> : null}
    </header>
  );
}

export function EmptyState({ icon, title, children, action }: { icon: ReactNode; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center rounded-lg px-8 py-20 text-center">
      <div className="mb-4 flex size-10 items-center justify-center rounded-lg bg-muted text-muted-foreground [&_svg]:size-5">{icon}</div>
      <h2 className="text-[15px] font-semibold">{title}</h2>
      {children ? <p className="mt-1.5 max-w-sm text-[13px] text-muted-foreground">{children}</p> : null}
      {action ? <div className="mt-5">{action}</div> : null}
    </div>
  );
}
