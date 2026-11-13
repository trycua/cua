// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceOs } from "@/bridge";
import { cn } from "@/lib/utils";
import { APPLE_PATH, COMPUTER_PATH, LINUX_PATH, WINDOWS_PATH } from "./os-paths";

const PATHS: Record<SpaceOs, string> = { macos: APPLE_PATH, linux: LINUX_PATH, windows: WINDOWS_PATH, unknown: COMPUTER_PATH };

export const OS_NAME: Record<SpaceOs, string> = { macos: "macOS", linux: "Linux", windows: "Windows", unknown: "Unknown" };

export function OsIcon({ os, className }: { os: SpaceOs; className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" role="img" aria-label={OS_NAME[os]} className={cn("size-4", className)}>
      <path d={PATHS[os]} />
    </svg>
  );
}
