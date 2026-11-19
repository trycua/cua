// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, CopyIcon, Loader2Icon, PlugIcon, RotateCcwIcon } from "lucide-react";
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { Link } from "@tanstack/react-router";

import { computerSignInAgentOf, connectPlan, useAgentKeys, useBridge, type ThreadFailure } from "@/bridge";
import { Button } from "@/components/ui/button";
import { useConnectors } from "@/hooks/use-connectors";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

import { ComputerSignIn } from "./computer-sign-in";

/** The Connections app that gives a harness its sign-in. */
const HARNESS_APP: Record<string, string> = { "claude-code": "claude-code", "openai-codex": "codex", hermes: "hermes" };
const PROVIDER_APP: Record<string, string> = { anthropic: "claude-code", openai: "codex" };

/** The Connections app a failed agent's Connect is for: its own harness first, else the provider's. */
export function failureApp(harness: string, provider: string | null): string | null {
  return HARNESS_APP[harness] ?? (provider ? (PROVIDER_APP[provider] ?? null) : null);
}

/** Where the agent runs: a Space (including a local VM) has its own sign-in unless sharing is enabled. */
export interface FailureComputer {
  space: string;
  name: string;
  thisMac: boolean;
}

/** The computer a thread's agents run on, for the failures in it (`ThreadView` provides it). */
export const FailureComputerContext = createContext<FailureComputer | undefined>(undefined);

/** A Space's sign-in is separate from the host's Connected badge, even for a local VM. */
export function FailureSummary({ failure, harness, children }: { failure: ThreadFailure; harness: string; children: ReactNode }) {
  const computer = useContext(FailureComputerContext);
  const t = useT();
  const agent = computerSignInAgentOf(harness);
  if (failure.class !== "needs_connect" || !computer || computer.thisMac || !agent) return children;
  return (
    <p data-failure-summary="" className="text-[13px] leading-[1.55] break-words text-foreground select-text">
      {t("spaces.sign_in.failure", { agent: agent === "claude-code" ? "Claude Code" : "Codex" })}
    </p>
  );
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
  computer,
}: {
  failure: ThreadFailure;
  /** Where the agent runs (absent: unknown, Connect as before). */
  computer?: FailureComputer;
  /** The failed agent's harness id. */
  harness: string;
  /** The message the agent was asked (re-sent by Retry). */
  taskText: string | null;
  onRetry?: (text: string) => Promise<void>;
}) {
  const needsConnect = failure.class === "needs_connect";
  const fromThread = useContext(FailureComputerContext);
  computer ??= fromThread;
  return (
    <div data-failure={failure.class} className="mt-1.5">
      <div className="flex flex-wrap items-center gap-2">
        {needsConnect ? (
          <ConnectActions harness={harness} provider={failure.provider} taskText={taskText} onRetry={onRetry} computer={computer} />
        ) : (
          <RetryButton taskText={taskText} onRetry={failure.retryable ? onRetry : undefined} enabled />
        )}
        <FailureDetails message={failure.message} />
      </div>
    </div>
  );
}

function ConnectActions({
  harness,
  provider,
  taskText,
  onRetry,
  computer,
}: {
  harness: string;
  provider: string | null;
  taskText: string | null;
  onRetry?: (text: string) => Promise<void>;
  computer?: FailureComputer;
}) {
  const t = useT();
  const c = useConnectors();
  const { core } = useBridge();
  // The agent was refused its credential: Settings reads the saved list again, which says it needs a reconnect.
  const keys = useAgentKeys();
  useEffect(() => {
    void keys.refresh();
    // Once, when the failure appears.
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const app = failureApp(harness, provider);
  const row = app ? c.view?.apps.find((a) => a.agent === app) : undefined;
  const providerRow = provider ? [...(c.view?.providers ?? []), ...(c.view?.moreProviders ?? [])].find((p) => p.id === provider) : undefined;
  const connected = row?.state === "connected" && providerRow?.state !== "not-connected";
  // The same word as the Connections row: Reconnect when the saved sign-in or key ran out or was refused.
  const label = row?.button?.label === "Reconnect" || providerRow?.status === "Needs reconnect" ? "Reconnect" : "Connect";
  // The same sheet as Settings, on the provider that refused it: an app with a sign-in of its own
  // (Claude Code, Codex) goes through its own Connect; any other app opens the provider's sheet.
  const connect = () => {
    const plan = app ? connectPlan(core, c.input, app) : null;
    if (app && plan && (plan.appToken || plan.device)) void c.connectApp(app);
    else c.openProvider(provider ?? plan?.provider ?? null, app);
  };
  // A Space (local VM or remote) can sign in itself, use an API key, or receive a saved sign-in with the sharing opt-in.
  const signInAgent = computer && !computer.thisMac ? computerSignInAgentOf(harness) : null;
  if (computer && signInAgent)
    return (
      <>
        {keys.report?.available && keys.report.copySignIn === false ? (
          <div data-failure-sign-in-sharing="" className="basis-full text-xs text-muted-foreground">
            <p>{t("spaces.sign_in.sharing_off", { setting: t("settings.connections.page.copy_sign_in") })}</p>
            <Link to="/settings/connections" hash="sign-ins-in-spaces" className="inline-block mt-1 text-foreground underline underline-offset-2">
              {t("spaces.sign_in.sharing_settings")}
            </Link>
          </div>
        ) : null}
        <ComputerSignIn
          agent={signInAgent}
          space={computer.space}
          computerName={computer.name}
          onUseKey={() => c.openProvider(provider ?? (signInAgent === "claude-code" ? "anthropic" : "openai"), app)}
          onSignedIn={() => {
            if (taskText && onRetry) void onRetry(taskText).catch(() => {});
          }}
        />
        <RetryButton taskText={taskText} onRetry={onRetry} enabled />
      </>
    );
  return (
    <>
      <Button size="sm" variant="outline" data-failure-connect="" onClick={connect}>
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
