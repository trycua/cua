// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { isUnsupported, useAgents, type AgentSetupRow } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Segmented } from "@/components/ui/segmented";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

/**
 * What the cua MCP server is and how an agent gets it: the agents on this
 * machine (`agents.setup`, the SDK's agent setup), one button that sets up
 * every installed one (`agents.configure`, the same as `cua agents setup`),
 * and the config entry for doing it by hand.
 */

type Manual = "claude-code" | "codex" | "hermes" | "openclaw";

const MANUAL: Record<Manual, { label: string; file: string; snippet: string }> = {
  "claude-code": {
    label: "Claude Code",
    file: "Terminal (writes ~/.claude.json)",
    snippet: "claude mcp add --scope user cua -- cua mcp",
  },
  codex: {
    label: "Codex",
    file: "~/.codex/config.toml",
    snippet: '[mcp_servers.cua]\ncommand = "cua"\nargs = ["mcp"]',
  },
  hermes: {
    label: "Hermes",
    file: "~/.hermes/config.yaml",
    snippet: "mcp_servers:\n  cua:\n    command: cua\n    args: [mcp]",
  },
  openclaw: {
    label: "OpenClaw",
    file: "~/.openclaw/openclaw.json",
    snippet: '{\n  "mcp": {\n    "servers": {\n      "cua": { "command": "cua", "args": ["mcp"] }\n    }\n  }\n}',
  },
};

const MANUAL_OPTIONS = (Object.keys(MANUAL) as Manual[]).map((value) => ({ value, label: MANUAL[value].label }));

export function ConnectAgentDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { agentSetup, configureAgents } = useAgents();
  const [rows, setRows] = useState<AgentSetupRow[] | null>(null);
  const [detectError, setDetectError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [manual, setManual] = useState<Manual>("claude-code");

  useEffect(() => {
    if (!open) return;
    let live = true;
    setDetectError(null);
    agentSetup().then(
      (r) => live && setRows(r),
      (e: unknown) => {
        if (!live) return;
        setRows([]);
        setDetectError(isUnsupported(e) ? null : e instanceof Error ? e.message : String(e));
      },
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const installed = (rows ?? []).filter((r) => r.installed);
  const pending = installed.filter((r) => !r.configured);
  const setUpAll = () => {
    setBusy(true);
    configureAgents()
      .then((r) => {
        setRows(r);
        toast("Agents set up", { description: "Restart an agent that is already open so it loads the server." });
      })
      .catch((e: unknown) => toast("Couldn't set up agents", { description: e instanceof Error ? e.message : String(e) }))
      .finally(() => setBusy(false));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[8vh] w-[min(600px,calc(100vw-2rem))] bg-popover">
        <div className="max-h-[84vh] overflow-y-auto px-6 pt-5 pb-6">
          <DialogTitle className="text-[15px] font-semibold">Connect an agent</DialogTitle>
          <DialogDescription className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">
            Coding agents reach Cua through the cua MCP server (<code className="font-mono text-xs">cua mcp</code>) and the cua skills.
            Once an agent has them, it can create Spaces, work inside them and start persistent agents. Its runs then show up on this page.
          </DialogDescription>

          <section className="mt-5">
            <div className="mb-2 flex items-center justify-between gap-3">
              <h3 className="text-xs font-semibold">On this machine</h3>
              {rows && rows.length > 0 ? (
                <Button size="sm" variant={pending.length ? "default" : "outline"} disabled={busy || pending.length === 0} onClick={setUpAll}>
                  {busy ? "Setting up…" : pending.length ? `Set up ${pending.length === 1 ? "1 agent" : `${pending.length} agents`}` : "All set up"}
                </Button>
              ) : null}
            </div>
            <div className="overflow-hidden rounded-lg border">
              {rows === null ? (
                <p className="px-3 py-3 text-xs text-muted-foreground">Looking for agents…</p>
              ) : rows.length === 0 ? (
                <p className="px-3 py-3 text-xs text-muted-foreground">
                  {detectError ?? "This app can't look for agents here. Run cua agents setup in a terminal, or add the server by hand below."}
                </p>
              ) : (
                <ul className="divide-y">
                  {rows.map((r) => (
                    <SetupRow key={r.agent} row={r} />
                  ))}
                </ul>
              )}
            </div>
            <p className="mt-2 text-xs text-muted-foreground">
              Setting up edits each agent's own config and keeps everything else in it. <code className="font-mono">cua agents setup</code> in a terminal does the
              same, and <code className="font-mono">cua agents remove</code> undoes it.
            </p>
          </section>

          <section className="mt-6">
            <h3 className="mb-2 text-xs font-semibold">Add it by hand</h3>
            <Segmented aria-label="Agent" value={manual} options={MANUAL_OPTIONS} onValueChange={setManual} />
            <Snippet file={MANUAL[manual].file} text={MANUAL[manual].snippet} />
            <p className="mt-2 text-xs text-muted-foreground">
              Use the full path to <code className="font-mono">cua</code> if the agent runs without your shell's PATH, as apps opened from the Dock do.
            </p>
          </section>

          <div className="mt-6 flex justify-end">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              Done
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

function SetupRow({ row }: { row: AgentSetupRow }) {
  return (
    <li className="flex items-center gap-3 px-3 py-2">
      <div className="min-w-0 flex-1">
        <div className={cn("text-[13px]", !row.installed && "text-muted-foreground")}>{row.name}</div>
        {row.installed && row.mcpConfig ? <div className="truncate font-mono text-2xs text-muted-foreground">{row.mcpConfig}</div> : null}
      </div>
      <span className={cn("inline-flex shrink-0 items-center gap-1 text-xs", row.configured ? "text-foreground" : "text-muted-foreground")}>
        {row.configured ? <CheckIcon className="size-3.5 text-success" aria-hidden /> : null}
        {!row.installed ? "Not installed" : row.configured ? "Set up" : "Not set up"}
      </span>
    </li>
  );
}

function Snippet({ file, text }: { file: string; text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };
  return (
    <div className="mt-2.5 overflow-hidden rounded-lg border bg-muted/40">
      <div className="flex h-8 items-center justify-between border-b pr-1 pl-3">
        <span className="font-mono text-2xs text-muted-foreground">{file}</span>
        <Button size="icon-sm" variant="ghost" aria-label="Copy" onClick={copy}>
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
        </Button>
      </div>
      <pre className="overflow-x-auto px-3 py-2.5 font-mono text-xs leading-[1.55]">{text}</pre>
    </div>
  );
}
