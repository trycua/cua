// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useState } from "react";

import { useAddSpaceWhenReady, useRiskyNudge, useSecondAgentNudge, type Nudge } from "@/bridge/nudges";
import { CuaKey } from "@/components/cua-key";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";

/**
 * The two This Mac prompts, in the top-right corner over whatever page is
 * open: a second agent started on this Mac's desktop, or an agent is about
 * to do something risky there. One main button and one way out; neither
 * stops an agent. The moments and words are the app core's
 * (`bridge/nudges.ts`).
 */
export function ThisMacNudges() {
  const second = useSecondAgentNudge();
  const risky = useRiskyNudge();
  const [wantSpace, setWantSpace] = useState(false);
  const started = useCallback((os: "macos" | "linux" | null) => {
    setWantSpace(false);
    if (os) toast(`Making a ${os === "macos" ? "macOS" : "Linux"} Space`, { description: "It shows in Spaces with its progress. You can keep working." });
    else toast("This Mac can't make a Space right now", { description: "Open New Space to see why." });
  }, []);

  // The risky question waits on an agent: it goes first.
  const card = risky.nudge ? (
    <NudgeCard
      nudge={risky.nudge}
      onPrimary={() => {
        risky.separate();
        setWantSpace(true);
      }}
      onSecondary={risky.here}
    />
  ) : second.nudge ? (
    <NudgeCard
      nudge={second.nudge}
      onPrimary={() => {
        second.addSpace();
        setWantSpace(true);
      }}
      onSecondary={second.keepSharing}
      onNever={second.dontAskAgain}
    />
  ) : null;

  return (
    <>
      {wantSpace ? <AddSpace onDone={started} /> : null}
      {card}
    </>
  );
}

function AddSpace({ onDone }: { onDone: (os: "macos" | "linux" | null) => void }) {
  useAddSpaceWhenReady(onDone);
  return null;
}

export function NudgeCard({ nudge, onPrimary, onSecondary, onNever }: { nudge: Nudge; onPrimary(): void; onSecondary(): void; onNever?: () => void }) {
  const titleId = `nudge-${nudge.kind}-title`;
  return (
    <div
      role="alertdialog"
      aria-modal="false"
      aria-labelledby={titleId}
      data-nudge={nudge.kind}
      className="fixed top-[calc(var(--titlebar-height,0px)+12px)] right-4 z-50 w-[392px] max-w-[calc(100vw-2rem)] rounded-lg bg-popover p-5 text-popover-foreground shadow-float"
    >
      <p className="flex items-start gap-2 text-[13px] leading-snug text-muted-foreground">
        <CuaKey size={18} />
        <span className="min-w-0">{nudge.eyebrow}</span>
      </p>
      <h2 id={titleId} className="mt-2 text-[17px] leading-snug font-semibold tracking-tight">
        {nudge.title}
      </h2>
      <p className="mt-1.5 text-sm leading-relaxed">{nudge.body}</p>
      {nudge.note ? <p className="mt-2 text-xs text-muted-foreground">{nudge.note}</p> : null}
      <div className="mt-4 flex flex-wrap items-center gap-2">
        <Button data-nudge-primary="" onClick={onPrimary}>
          {nudge.primary}
        </Button>
        <Button variant="secondary" data-nudge-secondary="" onClick={onSecondary}>
          {nudge.secondary}
        </Button>
        {nudge.never && onNever ? (
          <Button variant="ghost" size="sm" className="ml-auto text-muted-foreground" data-nudge-never="" onClick={onNever}>
            {nudge.never}
          </Button>
        ) : null}
      </div>
    </div>
  );
}
