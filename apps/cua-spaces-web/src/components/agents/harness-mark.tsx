// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cn } from "@/lib/utils";

/** The marks the app already ships for the apps it has them for. Others get their first letter. */
const MARKS: Record<string, string> = {
  "claude-code": "/onboarding/marks/mark-claude.svg",
  codex: "/onboarding/marks/mark-openai.svg",
  "openai-codex": "/onboarding/marks/mark-openai.svg",
  hermes: "/onboarding/marks/mark-hermes.svg",
  openclaw: "/onboarding/marks/mark-openclaw.svg",
};

/** A coding app's logo in a small tile; a neutral initial tile when there is no logo. */
export function HarnessMark({ agent, name, large }: { agent: string; name: string; large?: boolean }) {
  const src = MARKS[agent];
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center rounded-lg border bg-background font-semibold text-muted-foreground",
        large ? "size-10 text-[14px]" : "size-8 text-xs",
      )}
      data-harness-mark={agent}
      aria-hidden
    >
      {src ? <img src={src} alt="" className={large ? "size-6" : "size-5"} /> : name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}
