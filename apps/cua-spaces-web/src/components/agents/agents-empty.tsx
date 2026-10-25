// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { useAgents, type AgentSetupRow } from "@/bridge";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";

const MAIN_PROMPT = "Reproduce the crash from issue #4821 on a fresh Windows 11 Space and show me a recording.";
const MORE_PROMPTS = ["Run our tests on Ubuntu and macOS at the same time and tell me which ones fail.", "Check that the installer works on a clean Windows machine."];

/** "Claude Code, Codex and Cursor". */
export function nameList(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/**
 * The Agents page before any run. It says what to do next, by what is true
 * on this computer: coding apps found but Cua not in them yet (one Set up
 * button, asked once, in place); Cua already in them (the first prompts to
 * try); or no coding app found.
 */
export function AgentsEmpty() {
  const { agentSetup, configureAgents } = useAgents();
  const navigate = useNavigate();
  const [rows, setRows] = useState<AgentSetupRow[] | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    agentSetup().then(
      (r) => live && setRows(r),
      () => live && setRows([]),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per showing
  }, []);

  if (rows === null) return <div className="min-h-64" />;
  const installed = rows.filter((r) => r.installed);
  const ready = installed.filter((r) => r.configured);
  const todo = installed.filter((r) => !r.configured);
  const state = installed.length === 0 ? "none" : ready.length === 0 ? "setup" : "ready";

  const setUp = () => {
    setBusy(true);
    configureAgents(todo.map((r) => r.agent))
      .then(setRows)
      .catch(toastError("Couldn't set up your apps"))
      .finally(() => setBusy(false));
  };

  return (
    <div className="mx-auto flex w-full max-w-xl flex-col items-center pt-8 text-center" data-agents-empty={state}>
      <img src="/cua-key-128.png" alt="" className="size-24" />
      <h2 className="mt-6 text-[22px] font-semibold tracking-[-0.01em]">
        {state === "ready" ? "Ask an agent to work in a Space" : state === "setup" ? "Your agents can't use Cua yet" : "No coding app found yet"}
      </h2>
      <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">
        {state === "ready"
          ? "Open Claude Code, Codex or Cursor and ask it for something that needs a clean computer. It makes the Space, works in it and shows you the result here."
          : state === "setup"
            ? `${nameList(todo.map((r) => r.name))} ${todo.length === 1 ? "is" : "are"} on this Mac. Let ${todo.length === 1 ? "it" : "them"} use Cua, then ask one to work in a Space.`
            : "Install Claude Code, Codex or Cursor and Cua will notice. Spaces work without one."}
      </p>
      {state === "setup" ? (
        <Button size="lg" className="mt-6" disabled={busy} onClick={setUp} data-agents-setup>
          {busy ? "Setting up…" : "Set up"}
        </Button>
      ) : null}
      {state !== "none" ? (
        <>
          <div className="mt-8 w-full">
            {state === "setup" ? <p className="mb-3 text-[13px] text-muted-foreground">Then try</p> : null}
            <CopyPrompt text={MAIN_PROMPT} large={state === "ready"} />
          </div>
          {state === "ready" ? (
            <div className="mt-6 flex w-full flex-col gap-3">
              <p className="text-[13px] text-muted-foreground">Or try</p>
              {MORE_PROMPTS.map((p) => (
                <CopyPrompt key={p} text={p} />
              ))}
            </div>
          ) : null}
          <p className="mt-8 max-w-sm text-[13px] text-muted-foreground">
            {state === "ready" ? (
              <>
                {nameList(ready.map((r) => r.name))} can use Cua.{" "}
                <button type="button" className="text-brand-strong hover:underline" onClick={() => void navigate({ to: "/settings/connected-apps" as "/settings" })}>
                  Manage
                </button>
              </>
            ) : (
              "Cua adds itself to each app's settings and leaves the rest alone. You can turn it off in Settings."
            )}
          </p>
        </>
      ) : null}
    </div>
  );
}

function CopyPrompt({ text, large }: { text: string; large?: boolean }) {
  const [copied, setCopied] = useState(false);
  return (
    <div data-agent-prompt="" className="flex min-h-12 items-center gap-3 rounded-xl border bg-card px-4 py-3 text-left shadow-xs">
      <p className={large ? "min-w-0 flex-1 text-[15px] leading-relaxed" : "min-w-0 flex-1 text-[13px] text-muted-foreground"}>{text}</p>
      <Button
        size="icon"
        variant="ghost"
        aria-label={copied ? "Copied" : "Copy this prompt"}
        onClick={() => {
          void navigator.clipboard?.writeText(text).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? <CheckIcon /> : <CopyIcon />}
      </Button>
    </div>
  );
}
