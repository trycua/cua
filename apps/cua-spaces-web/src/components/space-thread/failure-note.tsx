// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, CopyIcon, Loader2Icon, PlugIcon, RotateCcwIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Link } from "@tanstack/react-router";

import { useAgentKeys, type ThreadFailure } from "@/bridge";
import { Button } from "@/components/ui/button";
import { useConnectors } from "@/hooks/use-connectors";
import { cn } from "@/lib/utils";

/** The Connectors app page that gives a harness its sign-in. */
const HARNESS_APP: Record<string, string> = { "claude-code": "claude-code", "openai-codex": "codex", hermes: "hermes" };
const PROVIDER_APP: Record<string, string> = { anthropic: "claude-code", openai: "codex" };

/** The Connectors app a failed agent's Connect opens: its own harness first, else the provider's. */
export function failureApp(harness: string, provider: string | null): string | null {
  return HARNESS_APP[harness] ?? (provider ? (PROVIDER_APP[provider] ?? null) : null);
}

/**
 * What sits under an agent's failed reply (the reply's own text is the plain
 * words): Connect and Retry when it needs a sign-in, Retry for any other
 * failure, and Show details with the complete error, selectable, with Copy.
 * The raw error is never the summary and is never cut.
 */
export function FailureNote({
  failure,
  harness,
  taskText,
  onRetry,
}: {
  failure: ThreadFailure;
  /** The failed agent's harness id. */
  harness: string;
  /** The message the agent was asked (re-sent by Retry). */
  taskText: string | null;
  onRetry?: (text: string) => Promise<void>;
}) {
  const needsConnect = failure.class === "needs_connect";
  return (
    <div data-failure={failure.class} className="mt-1.5">
      <div className="flex flex-wrap items-center gap-2">
        {needsConnect ? <ConnectActions harness={harness} provider={failure.provider} taskText={taskText} onRetry={onRetry} /> : <RetryButton taskText={taskText} onRetry={failure.retryable ? onRetry : undefined} enabled />}
        <FailureDetails message={failure.message} />
      </div>
    </div>
  );
}

function ConnectActions({ harness, provider, taskText, onRetry }: { harness: string; provider: string | null; taskText: string | null; onRetry?: (text: string) => Promise<void> }) {
  const c = useConnectors();
  // The agent was refused its credential: Settings reads the saved list again, which says it needs a reconnect.
  const keys = useAgentKeys();
  useEffect(() => {
    void keys.refresh();
    // Once, when the failure appears.
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const app = failureApp(harness, provider);
  const row = app ? c.view?.apps.find((a) => a.agent === app) : undefined;
  const connected = row?.state === "connected";
  // The same word as the Connectors row: Reconnect when the saved sign-in ran out or was refused.
  const label = row?.button?.label === "Reconnect" ? "Reconnect" : "Connect";
  return (
    <>
      <Button size="sm" variant="outline" data-failure-connect="" render={app ? <Link to="/settings/connectors/$app" params={{ app }} /> : <Link to="/settings/connectors" />}>
        <PlugIcon /> {label}
      </Button>
      <RetryButton taskText={taskText} onRetry={onRetry} enabled={connected} />
    </>
  );
}

/** Sends the same message again, once. */
function RetryButton({ taskText, onRetry, enabled }: { taskText: string | null; onRetry?: (text: string) => Promise<void>; enabled: boolean }) {
  const [state, setState] = useState<"idle" | "sending" | "sent">("idle");
  const [error, setError] = useState<string | null>(null);
  if (!onRetry || !taskText) return null;
  if (state === "sent")
    return (
      <span data-failure-retried="" className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
        <CheckIcon className="size-3.5 text-success" /> Sent again
      </span>
    );
  return (
    <>
      <Button
        size="sm"
        variant="outline"
        data-failure-retry=""
        disabled={!enabled || state === "sending"}
        onClick={() => {
          setState("sending");
          setError(null);
          onRetry(taskText).then(
            () => setState("sent"),
            (e: unknown) => {
              setState("idle");
              setError(e instanceof Error ? e.message : String(e));
            },
          );
        }}
      >
        {state === "sending" ? <Loader2Icon className="animate-spin" /> : <RotateCcwIcon />} Retry
      </Button>
      {error ? <span className="text-xs text-destructive">{error}</span> : null}
    </>
  );
}

/** "Show details": the complete error, selectable, with Copy. */
export function FailureDetails({ message }: { message: string }) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  if (!message) return null;
  return (
    <>
      <button
        type="button"
        aria-expanded={open}
        data-failure-details=""
        onClick={() => setOpen(!open)}
        className="flex cursor-default items-center gap-1 rounded-lg px-1 py-0.5 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <ChevronRightIcon className={cn("size-3 transition-transform", open && "rotate-90")} />
        {open ? "Hide details" : "Show details"}
      </button>
      {open ? (
        <div data-failure-details-open="" className="basis-full rounded-lg bg-muted/60 px-3 py-2">
          <pre data-failure-message="" className="font-mono text-[11.5px] leading-[1.5] break-words whitespace-pre-wrap text-foreground select-text">
            {message}
          </pre>
          <Button
            size="sm"
            variant="ghost"
            data-failure-copy=""
            className="mt-1 -ml-2"
            onClick={() => {
              void navigator.clipboard?.writeText(message).catch(() => undefined);
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            }}
          >
            {copied ? <CheckIcon /> : <CopyIcon />} {copied ? "Copied" : "Copy"}
          </Button>
        </div>
      ) : null}
    </>
  );
}
