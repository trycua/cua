// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon } from "lucide-react";
import { useState } from "react";

import { useMachines, type Machine } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { otherMachineCommand, otherMachinePrompt, OTHER_MACHINE_PERMISSIONS, OTHER_MACHINE_RELOAD } from "@/lib/other-machine";

type Agent = "claude-code" | "codex" | "hermes" | "openclaw";
const AGENTS: { value: Agent; label: string }[] = [
  { value: "claude-code", label: "Claude Code" },
  { value: "codex", label: "Codex" },
  { value: "hermes", label: "Hermes" },
  { value: "openclaw", label: "OpenClaw" },
];

/** This machine, and the other machines of the account: what Connect names and what it did not touch. */
export function useThisAndOthers(): { here: Machine | undefined; hereName: string; others: string[] } {
  const { data: machines } = useMachines();
  const here = machines?.find((m) => m.current);
  return { here, hereName: here?.name ?? "this machine", others: (machines ?? []).filter((m) => !m.current && !m.device).map((m) => m.name) };
}

/**
 * An agent that runs on another machine uses this one: the host to name, the exact commands to run on
 * that machine, what this machine has to allow, and the reload note (CUA-2644).
 */
export function OtherMachineBlock() {
  const { here, hereName } = useThisAndOthers();
  const [agent, setAgent] = useState<Agent>("hermes");
  return (
    <div data-connect-other-machine="" className="px-4 py-3.5">
      <p className="text-xs text-muted-foreground">
        An agent that runs on a different machine can use {hereName} too. Host to name: <span className="font-medium text-foreground select-text">{hereName}</span>
        {here?.relayId ? (
          <>
            , its Space <code className="font-mono select-text">relay:{here.relayId}</code>
          </>
        ) : (
          <>. Set this machine up for access first (its page in Machines) so the agent can find it</>
        )}
        .
      </p>
      <p className="mt-3 mb-2 text-xs font-semibold">Run this on the other machine</p>
      <Segmented aria-label="Agent on the other machine" value={agent} options={AGENTS} onValueChange={setAgent} />
      <Snippet file="Terminal on the other machine" text={otherMachineCommand(agent)} />
      <p className="mt-3 mb-2 text-xs font-semibold">Then tell the agent</p>
      <Snippet file="Message to the agent" text={otherMachinePrompt(hereName, here?.relayId)} />
      <p className="mt-2 text-xs text-muted-foreground" data-connect-other-permissions="">
        {OTHER_MACHINE_PERMISSIONS}
      </p>
      <p className="mt-1 text-xs text-muted-foreground" data-connect-other-reload="">
        {OTHER_MACHINE_RELOAD}
      </p>
    </div>
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
        <span className="text-2xs text-muted-foreground">{file}</span>
        <Button size="icon-sm" variant="ghost" aria-label="Copy" onClick={copy}>
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
        </Button>
      </div>
      <pre className="overflow-x-auto px-3 py-2.5 font-mono text-xs leading-[1.55] select-text">{text}</pre>
    </div>
  );
}
