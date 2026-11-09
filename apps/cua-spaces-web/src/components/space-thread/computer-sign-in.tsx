// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ExternalLinkIcon, KeyRoundIcon, Loader2Icon, LogInIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useBridge, type ComputerSignInAgent, type ComputerSignInState } from "@/bridge";
import { CopyButton } from "@/components/prompt-line";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/** What each agent is called in the sign-in's words. */
const AGENT_NAME: Record<ComputerSignInAgent, string> = { "claude-code": "Claude Code", "openai-codex": "Codex" };
/** Who you sign in with. */
const ACCOUNT: Record<ComputerSignInAgent, string> = { "claude-code": "Claude", "openai-codex": "ChatGPT" };

/** How often the page asks whether the sign-in finished. */
export const SIGN_IN_POLL_MS = 2000;

/**
 * A coding agent on another computer that isn't signed in (CUA-3907): Cua starts
 * the agent's own sign-in on that computer and shows the link (and code) to
 * finish it in your own browser, then sends the message again. Cua never copies
 * your sign-in there. "or use an API key" opens the provider's key sheet.
 */
export function ComputerSignIn({
  agent,
  space,
  computerName,
  onUseKey,
  onSignedIn,
}: {
  agent: ComputerSignInAgent;
  space: string;
  computerName: string;
  /** Opens the API key sheet for this agent's provider. */
  onUseKey: () => void;
  /** The agent is signed in there now (sends the message again). */
  onSignedIn: () => void;
}) {
  const { data } = useBridge();
  const [s, setS] = useState<ComputerSignInState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [unsupported, setUnsupported] = useState(false);
  const told = useRef(false);
  /** The person started it here (a sign-in that was already there needs no resend). */
  const started = useRef(false);
  const name = AGENT_NAME[agent];

  const call = async (action: "status" | "start" | "code" | "cancel", extra: { code?: string } = {}) => {
    if (!data) return null;
    try {
      // The run that failed was refused its sign-in: one that is there has run out.
      const next = await data.call("agents.computerSignIn", { space, agent, action, afterRefusal: true, ...extra });
      setS(next);
      setError(null);
      return next;
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (/unsupported|not supported/i.test(msg)) setUnsupported(true);
      else setError(msg);
      return null;
    }
  };

  // Where it stands, once, when the note appears.
  useEffect(() => {
    void call("status");
    // Once per note.
  }, [space, agent]); // eslint-disable-line react-hooks/exhaustive-deps

  // While it's going, ask again until it finishes.
  const going = s?.state === "signing_in";
  useEffect(() => {
    if (!going) return;
    const t = setInterval(() => void call("status"), SIGN_IN_POLL_MS);
    return () => clearInterval(t);
  }, [going]); // eslint-disable-line react-hooks/exhaustive-deps

  // Signed in after it was offered: send the message again, once.
  useEffect(() => {
    if (s?.state === "signed_in" && !told.current && started.current) {
      told.current = true;
      onSignedIn();
    }
  }, [s?.state]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await fn();
    } finally {
      setBusy(false);
    }
  };
  const start = () => {
    started.current = true;
    void run(() => call("start"));
  };

  if (unsupported) return null;

  const keyLink = (
    <button
      type="button"
      data-signin-use-key=""
      onClick={onUseKey}
      className="inline-flex cursor-default items-center gap-1 text-xs text-muted-foreground outline-none hover:text-foreground hover:underline focus-visible:underline"
    >
      <KeyRoundIcon className="size-3" /> or use an API key
    </button>
  );

  if (s?.state === "signed_in")
    return (
      <span data-signin-state="signed_in" className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
        <CheckIcon className="size-3.5 text-success" /> {name} is signed in on {computerName}
      </span>
    );

  if (s?.state === "signing_in")
    return (
      <div data-signin-state="signing_in" className="basis-full rounded-lg bg-muted/60 px-3 py-2.5 text-[13px]">
        <p className="font-medium">
          Sign in to {name} on {computerName}
        </p>
        {s.url ? (
          <>
            <ol className="mt-1.5 list-decimal space-y-1 pl-4 text-muted-foreground">
              <li>
                Open the sign-in page on your phone or computer and sign in with {ACCOUNT[agent]}.{" "}
                <button
                  type="button"
                  data-signin-open={s.url}
                  onClick={() => void data?.call("session.openExternal", { url: s.url! }).catch(() => {})}
                  className="inline-flex cursor-default items-center gap-1 text-brand-strong outline-none hover:underline focus-visible:underline"
                >
                  Open sign-in page <ExternalLinkIcon className="size-3" />
                </button>
              </li>
              {s.code ? (
                <li>
                  Type this code on that page:
                  <span className="mt-1 flex items-center gap-3" data-signin-code={s.code}>
                    <code className="font-mono text-[20px] font-medium tracking-[0.08em] text-foreground tabular-nums">{s.code}</code>
                    <CopyButton text={s.code} label="Copy code" />
                  </span>
                </li>
              ) : null}
              {s.pasteCode ? <li>Copy the code the page shows you and paste it here.</li> : null}
            </ol>
            {s.pasteCode ? (
              <form
                className="mt-2 flex items-center gap-2"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (code.trim()) void run(() => call("code", { code: code.trim() }));
                }}
              >
                <Input
                  data-signin-paste=""
                  aria-label="Code from the sign-in page"
                  placeholder="Paste the code"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  className="h-8 max-w-72 font-mono text-xs"
                />
                <Button size="sm" type="submit" disabled={!code.trim() || busy}>
                  {busy ? <Loader2Icon className="animate-spin" /> : null} Finish
                </Button>
              </form>
            ) : (
              <p className="mt-2 inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <Loader2Icon className="size-3 animate-spin" /> Waiting for you to finish. The message is sent again once {name} is signed in.
              </p>
            )}
          </>
        ) : (
          <p className="mt-1 inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <Loader2Icon className="size-3 animate-spin" /> Starting {name}'s sign-in on {computerName}…
          </p>
        )}
        <div className="mt-2 flex items-center gap-3">
          <button
            type="button"
            data-signin-cancel=""
            onClick={() => void run(() => call("cancel"))}
            className="cursor-default text-xs text-muted-foreground outline-none hover:text-foreground hover:underline focus-visible:underline"
          >
            Cancel
          </button>
          {keyLink}
        </div>
        {error ? <p className="mt-1 text-xs text-destructive">{error}</p> : null}
      </div>
    );

  const again = s?.state === "expired" || s?.state === "failed";
  return (
    <>
      <Button size="sm" variant="outline" data-signin-start="" data-signin-state={s?.state ?? "loading"} disabled={busy || !s} onClick={start}>
        {busy ? <Loader2Icon className="animate-spin" /> : <LogInIcon />} {again ? "Sign in again" : "Sign in"} to {name} on {computerName}
      </Button>
      {keyLink}
      {s?.state === "expired" ? (
        <span className="basis-full text-xs text-muted-foreground">
          {name}'s sign-in on {computerName} has run out.
        </span>
      ) : null}
      {s?.state === "failed" ? (
        <span data-signin-failed="" className="basis-full text-xs text-destructive">
          {s.reason}
        </span>
      ) : null}
      {error ? <span className="basis-full text-xs text-destructive">{error}</span> : null}
    </>
  );
}
