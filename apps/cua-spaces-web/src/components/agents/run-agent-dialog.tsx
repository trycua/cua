// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { useAgents, type AgentSetupRow, type Space } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { PromptBox } from "@/components/ui/prompt-box";
import { toast } from "@/components/ui/toast";

/** What to ask the agent: one request that works in a Space with nothing else set up. */
export const runAgentPrompt = (spaceName: string) => `Open the Cua Space "${spaceName}" and tell me what's on its screen.`;

/**
 * "Run an agent here": the shortest way from a ready Space to an agent
 * working in it. If a coding agent on this computer already works with Cua,
 * it shows the one request to paste into it. If not, it sets one up here
 * first (one button) and then shows the request. The agent's work shows up
 * on the Agents page.
 */
export function RunAgentDialog({ open, space, onOpenChange, onManual, onConnected, prompt: asked }: { open: boolean; space: Space; onOpenChange: (open: boolean) => void; onManual?: () => void; onConnected?: () => void; /** The request to show; the default only looks at the screen. */ prompt?: string }) {
  const { agentSetup, configureAgents } = useAgents();
  const navigate = useNavigate();
  const [rows, setRows] = useState<AgentSetupRow[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!open) return;
    let live = true;
    setRows(null);
    agentSetup().then(
      (r) => live && setRows(r),
      () => live && setRows([]),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const ready = (rows ?? []).filter((r) => r.configured);
  const pending = (rows ?? []).filter((r) => r.installed && !r.configured);
  const prompt = asked ?? runAgentPrompt(space.name);
  const first = ready[0];
  const toSet = pending[0];

  const setUp = () => {
    setBusy(true);
    configureAgents()
      .then((r) => {
        setRows(r);
        if (r.some((x) => x.configured)) onConnected?.();
      })
      .catch((e: unknown) => toast("Couldn't connect your agent", { description: e instanceof Error ? e.message : String(e) }))
      .finally(() => setBusy(false));
  };
  // The prompt box copies; this is what follows a copy (and "Copy and see Agents" copies too).
  const copy = () => {
    onConnected?.();
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  const copyPrompt = () => void navigator.clipboard?.writeText(prompt).then(copy, copy);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup data-run-agent={rows === null ? "loading" : ready.length ? "ready" : "setup"} className="top-[12vh] w-[min(520px,calc(100vw-2rem))] bg-popover">
        <div className="px-6 pt-5 pb-6">
          <DialogTitle className="text-[15px] font-semibold">Run an agent in {space.name}</DialogTitle>
          {rows === null ? (
            <DialogDescription className="mt-3 text-[13px] text-muted-foreground">Looking for your agents…</DialogDescription>
          ) : first ? (
            <>
              <DialogDescription className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">
                {first.name} is connected. Open it, paste this, and watch it work in {space.name}.
              </DialogDescription>
              <PromptBox className="mt-4" id="run-agent-prompt" aria-label="Prompt" data-run-agent-prompt="" value={prompt} copied={copied} onCopy={copy} />
              <p className="mt-3 text-xs text-muted-foreground">If {first.name} was already open, restart it first so it can see Cua.</p>
              <div className="mt-5 flex justify-end gap-2">
                <Button variant="secondary" onClick={() => onOpenChange(false)}>
                  Close
                </Button>
                <Button
                  data-run-agent-watch=""
                  onClick={() => {
                    copyPrompt();
                    onOpenChange(false);
                    void navigate({ to: "/agents/all" });
                  }}
                >
                  Copy and see Agents
                </Button>
              </div>
            </>
          ) : toSet ? (
            <>
              <DialogDescription className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">
                To let an agent work in {space.name}, connect it to Cua first. This takes a few seconds and changes nothing else about {toSet.name}.
              </DialogDescription>
              <ul className="mt-4 divide-y rounded-lg border">
                {pending.map((r) => (
                  <li key={r.agent} className="px-3 py-2 text-[13px]">
                    {r.name}
                  </li>
                ))}
              </ul>
              <div className="mt-5 flex justify-end gap-2">
                <Button variant="secondary" onClick={() => onOpenChange(false)}>
                  Not now
                </Button>
                <Button data-run-agent-setup="" disabled={busy} onClick={setUp}>
                  {busy ? "Connecting…" : pending.length === 1 ? `Connect ${toSet.name}` : "Connect these agents"}
                </Button>
              </div>
            </>
          ) : (
            <>
              <DialogDescription className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">
                We couldn't find an agent such as Claude Code or Codex on this computer. Install one, then come back here. Or connect one by hand.
              </DialogDescription>
              <div className="mt-5 flex justify-end gap-2">
                <Button variant="secondary" onClick={() => onOpenChange(false)}>
                  Close
                </Button>
                {onManual ? (
                  <Button
                    onClick={() => {
                      onOpenChange(false);
                      onManual();
                    }}
                  >
                    Show me how
                  </Button>
                ) : null}
              </div>
            </>
          )}
        </div>
      </DialogPopup>
    </Dialog>
  );
}
