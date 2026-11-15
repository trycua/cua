// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceOs } from "@/bridge";
import { cn } from "@/lib/utils";
import { OsIcon } from "./os-icon";

/** A graphics card with two fans: the mark of a GPU job's row, drawn like the other sidebar icons (24 px grid, `currentColor`, sized by the caller). */
export function GpuIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      role="img"
      aria-label="GPU"
      data-gpu-icon=""
      className={cn("size-4", className)}
    >
      <rect x="3" y="7" width="18" height="9" rx="1.5" />
      <path d="M3 6v11" />
      <circle cx="9" cy="11.5" r="2.5" />
      <circle cx="16" cy="11.5" r="2.5" />
      <path d="M7 16v2M10 16v2M13 16v2M16 16v2" />
    </svg>
  );
}

/** A child Space's icon: the GPU card for a GPU job, the OS logo for every other Space (a job without a GPU too). */
export function SpaceIcon({ os, gpu, className }: { os: SpaceOs; gpu?: boolean; className?: string }) {
  return gpu ? <GpuIcon className={className} /> : <OsIcon os={os} className={className} />;
}
