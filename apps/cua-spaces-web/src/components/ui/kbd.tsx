// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";
import { shortcutLabel } from "@/lib/keybindings";

export function Kbd({ className, ...props }: ComponentProps<"kbd">) {
  return (
    <kbd
      className={cn(
        "inline-flex h-5 min-w-5 items-center justify-center rounded-[5px] border border-border bg-muted px-1 font-sans text-2xs font-medium text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

export function Shortcut({ spec, className }: { spec: string; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-0.5", className)}>
      {shortcutLabel(spec).map((token) => (
        <Kbd key={token}>{token}</Kbd>
      ))}
    </span>
  );
}
