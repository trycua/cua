// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cn } from "@/lib/utils";

/** The official Cua mark, unmodified. Swaps the black and white files with the theme. */
export function CuaLogo({ className }: { className?: string }) {
  return (
    <span className={cn("relative inline-block size-5", className)} aria-label="Cua" role="img">
      <img src="/cua_logo_black_new.svg" alt="" className="absolute inset-0 size-full dark:hidden" draggable={false} />
      <img src="/cua_logo_white_new.svg" alt="" className="absolute inset-0 hidden size-full dark:block" draggable={false} />
    </span>
  );
}
