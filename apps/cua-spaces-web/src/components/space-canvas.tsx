// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode, Ref } from "react";

import { cn } from "@/lib/utils";

/** The gridded surface Space tiles draw on: under the Space's latest
 * thumbnail (`spaces.thumbnail`) when the host has one, and its live video. */
export function SpaceCanvas({
  dim,
  className,
  children,
  ref,
  streamState,
}: {
  dim?: boolean;
  className?: string;
  children?: ReactNode;
  ref?: Ref<HTMLDivElement>;
  /** Live video over it (`data-stream-state`), when the host draws it. */
  streamState?: string;
}) {
  return (
    <div
      ref={ref}
      data-stream-state={streamState}
      className={cn(
        "relative flex aspect-[16/10] items-center justify-center overflow-hidden rounded-lg bg-thumb",
        "bg-[linear-gradient(var(--thumb-line)_1px,transparent_1px),linear-gradient(90deg,var(--thumb-line)_1px,transparent_1px)] bg-[size:24px_24px] bg-center",
        dim && "opacity-70",
        className,
      )}
    >
      {children}
    </div>
  );
}
