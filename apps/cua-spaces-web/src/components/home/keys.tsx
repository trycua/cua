// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceOs } from "@/bridge";
import { cn } from "@/lib/utils";

/** The spaces.cua.ai product keys, one per kind of computer (public/onboarding/float, the site's own renders). */
const KEY: Record<Exclude<SpaceOs, "unknown">, string> = {
  macos: "/onboarding/float/product-2.webp",
  linux: "/onboarding/float/product-3.webp",
  windows: "/onboarding/float/product-1.webp",
};

/** A computer as its keycap: dark for a Mac, warm for Linux, blue for Windows. Faded while it's offline. */
export function ComputerKey({ os, size = 44, dim = false, className }: { os: SpaceOs | null; size?: number; dim?: boolean; className?: string }) {
  return (
    <img
      src={KEY[os && os !== "unknown" ? os : "macos"]}
      alt=""
      aria-hidden
      draggable={false}
      width={size}
      height={size}
      data-computer-key={os ?? "macos"}
      className={cn("shrink-0 object-contain select-none", dim && "opacity-45 grayscale", className)}
    />
  );
}

/** The three keys over "Add a computer". */
export function KeyTrio({ size = 44 }: { size?: number }) {
  return (
    <span aria-hidden className="flex items-end -space-x-1">
      <ComputerKey os="macos" size={size} className="-rotate-6" />
      <ComputerKey os="linux" size={size} />
      <ComputerKey os="windows" size={size} className="rotate-6" />
    </span>
  );
}

const AGENT: Record<string, string> = {
  "claude-code": "/onboarding/agent-claude.webp",
  "openai-codex": "/onboarding/agent-codex.webp",
  codex: "/onboarding/agent-codex.webp",
  hermes: "/onboarding/agent-hermes.webp",
  openclaw: "/onboarding/agent-openclaw.webp",
};

/** A coding agent as its keycap (the first run's), or the plain one for any other agent. */
export function AgentKey({ harness, size = 32, className }: { harness: string; size?: number; className?: string }) {
  return (
    <img
      src={AGENT[harness] ?? "/onboarding/agent-other.webp"}
      alt=""
      aria-hidden
      draggable={false}
      width={size}
      height={size}
      className={cn("shrink-0 object-contain select-none", className)}
    />
  );
}

/** Words with a dot between each ("3 computers · 40 cores · 96 GB"). */
export function Dots({ parts, className, keep = false }: { parts: readonly string[]; className?: string; keep?: boolean }) {
  return (
    <span className={className}>
      {parts.map((p, i) => {
        const last = i === parts.length - 1;
        return (
          // A no-break space keeps each dot with the word before it; a line may break after it.
          <span key={`${i}:${p}`}>
            <span className={keep ? "inline-block whitespace-nowrap" : undefined}>
              {p}
              {last ? null : "\u00a0·"}
            </span>
            {last ? null : " "}
          </span>
        );
      })}
    </span>
  );
}
