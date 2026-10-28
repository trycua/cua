// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect, useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon, CheckIcon, CopyIcon, ExternalLinkIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { useAgentKeys, useBridge, useFirstRun, useSpaces, type FirstRunArt, type FirstRunCredential, type FirstRunExtraView, type FirstRunHook, type FirstRunRow, type FirstRunView } from "@/bridge";
import { readSavedOnboarding } from "@/bridge/onboarding";
import { AddAgentSheet } from "@/components/add-agent/add-agent-sheet";
import { APP_MARKS, AppMarkImage } from "@/components/agents/harness-mark";
import { CuaLogo } from "@/components/cua-logo";
import { OsIcon } from "@/components/os-icon";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { RuntimeFixRow } from "@/components/new-space/runtime-fix";
import { Snippet } from "@/components/settings/snippet";
import { KeySheet } from "@/components/agents/key-sheet";
import { setConnectedAppsState, useConnectedAppsState } from "@/hooks/use-connected-apps";
import { BY_HAND, failureText, getItUrl } from "@/lib/connected-apps";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/onboarding/")({
  // `?due=1` is a host opening the first run because it is not done (the Mac
  // app at launch, or when its window opens again). The page keeps to what
  // the person chose: after "Skip for now" it goes to Spaces instead, as the
  // root route does for a host that opens the app at `/`. Every other way in
  // (the sidebar's "Finish setting up", Settings, the command palette) has
  // no `due` and always shows the first run.
  validateSearch: (s: Record<string, unknown>): { due?: true } => (s.due === 1 || s.due === "1" || s.due === true ? { due: true as const } : {}),
  beforeLoad: ({ search }) => {
    if (search.due && readSavedOnboarding().skipped) throw redirect({ to: "/spaces" });
  },
  component: OnboardingPage,
});

/**
 * The first run, the same on macOS, Windows and Linux: Welcome, Pick your
 * agent, Install the Cua CLI (only while the app's CLI is not on this
 * computer), Add your other computers (optional), Share this Mac's screen
 * (a signed-in Mac that shares it), Try it in your agent. Every
 * word, the agents' order and which button does what are the app core's
 * (`firstRun.view`, through `useFirstRun`); this file places them beside the
 * keycap art, as plain rows.
 */
function OnboardingPage() {
  const fr = useFirstRun();
  const navigate = useNavigate();
  const { view } = fr;

  const leave = () => {
    fr.later();
    void navigate({ to: "/spaces" });
  };
  const finish = async () => {
    try {
      await fr.finish();
      await navigate({ to: "/spaces" });
    } catch (e) {
      toast("Couldn't finish setup", { description: e instanceof Error ? e.message : String(e) });
    }
  };

  // A new screen: focus moves to its heading, so a screen reader reads it.
  useEffect(() => {
    document.querySelector<HTMLElement>("[data-onboarding-heading]")?.focus();
  }, [view.step]);

  // Esc goes back a step, like the "Back" link, on every step that has one.
  // An open sheet or menu keeps its own Esc.
  const canGoBack = view.step !== "welcome";
  useEffect(() => {
    if (!canGoBack) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || e.isComposing) return;
      if (document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]')) return;
      e.preventDefault();
      fr.back();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canGoBack, fr.back]);

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-[#F4F5F7] [--ob-x:clamp(40px,8vw,96px)] [--ob-art-h:min(58vh,560px)] dark:bg-[#07080a]" data-step={view.step}>
      <Backdrop />
      <Art art={view.art} step={view.step} />
      <header className="relative z-30 flex shrink-0 items-center justify-between pr-10 pl-[var(--ob-x)] pt-[calc(var(--titlebar-height)+12px)] pb-2">
        <CuaWordmark />
        <button type="button" onClick={leave} className={SKIP_PILL} data-onboarding-later="">
          {view.later}
        </button>
      </header>
      <div className="relative z-10 flex min-h-0 flex-1 flex-col overflow-y-auto pr-10 pb-6 pl-[var(--ob-x)] [@media(min-width:1000px)]:pr-[46%]">
        {/* Centre the step on the window, a little above true centre (about 45% from the top). The min heights keep a gap under the logo bar and above the progress dots; tall steps scroll. */}
        <div aria-hidden data-onboarding-spacer-top="" className="min-h-[clamp(8px,2vh,32px)] shrink-0 grow-[33] basis-0 [@media(max-width:999px)]:grow-[50]" />
        <div className="flex w-full max-w-[520px] shrink-0 flex-col">
          {view.welcome ? <Welcome fr={fr} view={view} /> : null}
          {view.connect ? <Connect fr={fr} view={view} /> : null}
          {view.cli ? <CommandLine fr={fr} view={view} /> : null}
          {view.devices ? <Devices fr={fr} view={view} /> : null}
          {view.permissions ? <Permissions fr={fr} view={view} /> : null}
          {view.setup ? <Setup fr={fr} view={view} onFinish={() => void finish()} /> : null}
        </div>
        <div aria-hidden data-onboarding-spacer-bottom="" className="min-h-6 shrink-0 grow-[67] basis-0 [@media(max-width:999px)]:grow-[50]" />
      </div>
      <Dots view={view} />
    </div>
  );
}

/** "Skip for now": a ghost pill, readable on the starfield, quieter than Continue. */
const SKIP_PILL =
  "rounded-full border border-foreground/30 px-4 py-1.5 text-[13px] text-foreground/90 outline-none transition-colors hover:border-foreground/50 hover:bg-foreground/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60";

function CuaWordmark() {
  return (
    <span className="flex items-center gap-1.5">
      <CuaLogo className="size-5" />
      <span className="font-display text-[17px] font-medium tracking-[-0.01em]">Cua</span>
    </span>
  );
}

/* ---- Pieces every screen uses ------------------------------------------------------ */

function Heading({ children }: { children: ReactNode }) {
  return (
    <h1 tabIndex={-1} data-onboarding-heading="" className="font-display text-[clamp(32px,4.4vw,48px)] leading-[1.05] font-semibold tracking-[-0.03em] text-balance outline-none">
      {children}
    </h1>
  );
}

function Lede({ children }: { children: ReactNode }) {
  return <p className="mt-3 text-[15px] leading-relaxed text-foreground/75">{children}</p>;
}

/** Plain rows between hairlines: no boxes, no filled panels. */
function Rows({ children, className, ...rest }: { children: ReactNode; className?: string } & Record<`data-${string}`, string | undefined>) {
  return (
    <div className={cn("flex flex-col divide-y divide-foreground/10 border-y border-foreground/10", className)} {...rest}>
      {children}
    </div>
  );
}

function Primary({ label, onClick, ...rest }: { label: string; onClick: () => void } & Record<`data-${string}`, string | undefined>) {
  return (
    <Button size="lg" className="h-11 min-w-28 rounded-full px-6 text-[15px]" data-onboarding-primary="" onClick={onClick} {...rest}>
      {label}
    </Button>
  );
}

/** The quieter choice beside the primary: the same pill, outlined, lower contrast. */
function Secondary({ label, onClick, describedBy, ...rest }: { label: string; onClick: () => void; describedBy?: string } & Record<`data-${string}`, string | undefined>) {
  return (
    <Button
      size="lg"
      variant="ghost"
      className="h-11 rounded-full border-foreground/20 px-6 text-[15px] font-normal text-foreground/75 hover:bg-foreground/[0.06] hover:text-foreground"
      aria-describedby={describedBy}
      data-first-run-secondary=""
      onClick={onClick}
      {...rest}
    >
      {label}
    </Button>
  );
}

const QUIET_LINK = "-mx-1.5 rounded-md px-1.5 py-0.5 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60";

/**
 * A step after Welcome: "Back" sits above the heading, apart from the
 * step's own choices. It comes last in the page's order, so Tab goes
 * primary, then secondary, then Back; Esc does the same as Back.
 */
function Step({ id, onBack, children }: { id: string; onBack: () => void; children: ReactNode }) {
  return (
    <div data-first-run={id} className="flex flex-col">
      {children}
      <button type="button" onClick={onBack} aria-keyshortcuts="Escape" className={cn(QUIET_LINK, "order-first mb-4 inline-flex h-7 items-center gap-1.5 self-start text-[13px]")} data-first-run-back="">
        <ArrowLeftIcon aria-hidden className="size-3.5" />
        Back
      </button>
    </div>
  );
}

/** A line with a state: a check, a spinner, or a fix with its button. */
function StatusLine({ row, onAct }: { row: FirstRunRow; onAct?: () => void }) {
  return (
    <div className="flex min-h-16 items-center gap-3 py-2" data-first-run-status={row.id} data-state={row.state}>
      <StateIcon state={row.state} />
      <div className="min-w-0 flex-1">
        <div className="text-[14px]">{row.title}</div>
        {row.note ? <div className="text-[13px] text-muted-foreground">{row.note}</div> : null}
      </div>
      {row.action && onAct ? (
        <Button variant="outline" className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={onAct} data-first-run-action={row.action.id}>
          {row.action.label}
        </Button>
      ) : null}
    </div>
  );
}

/* ---- 1 · Welcome ------------------------------------------------------------------ */

function Welcome({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const w = view.welcome!;
  return (
    <div data-first-run="welcome">
      {/* The row Back takes on the other steps, so the heading stays put. */}
      <div aria-hidden className="mb-4 h-7" />
      <Heading>{w.title}</Heading>
      <Lede>{w.lede}</Lede>
      <div className="mt-8">
        <Primary label={w.primary.label} onClick={() => void fr.act(w.primary.id)} />
      </div>
    </div>
  );
}

/* ---- 2 · Pick your agent ------------------------------------------------------------ */

function AgentMark({ agent, name, className }: { agent: string; name?: string; className?: string }) {
  if (!(agent in APP_MARKS))
    return (
      <span aria-hidden className={cn("flex size-4 shrink-0 items-center justify-center rounded-[4px] bg-foreground/10 text-[9px] font-semibold text-muted-foreground", className)}>
        {(name ?? agent).trim().charAt(0).toUpperCase()}
      </span>
    );
  return <AppMarkImage agent={agent} className={cn("size-4", className)} />;
}

/** The prompt "done" hands over: a real job where a fresh computer matters. */
/** First letter up: "your 5 coding apps can use Cua." as a sentence. */
/** "Codex", "Codex and Cursor", "Hermes, Codex and Cursor". */
const listNames = (names: string[]): string => (names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);

const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

const FIRST_PROMPT = "Reproduce the crash from issue #4821 on a fresh Windows 11 Space and show me a recording.";

/** The coding apps named when none is installed, in this order. */
const HEADLINE_APPS = [
  { agent: "claude-code", name: "Claude Code" },
  { agent: "codex", name: "OpenAI Codex" },
  { agent: "cursor", name: "Cursor" },
];

/**
 * "An agent on another machine": the other choice beside the coding apps found here. It opens the
 * join sheet for this machine's own Space: one line to paste into whichever agent runs elsewhere.
 * Not offered before the machine has a Space to join.
 */
function OtherMachineRow() {
  const { data: spaces } = useSpaces();
  const [open, setOpen] = useState(false);
  const space = spaces?.find((s) => s.id === "this-mac") ?? spaces?.find((s) => s.provider === "local");
  if (!space) return null;
  return (
    <div className="flex min-h-12 items-center gap-3 py-2" data-first-run-other-machine="">
      <div className="min-w-0 flex-1">
        <div className="text-[14px]">An agent on another machine</div>
        <div className="text-[13px] text-muted-foreground">Copy one line to paste into it.</div>
      </div>
      <Button variant="outline" className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={() => setOpen(true)} data-first-run-other-machine-open="">
        Connect it
      </Button>
      <AddAgentSheet open={open} onOpenChange={setOpen} space={{ id: space.id, name: space.name }} role={null} initialStage="connect" />
    </div>
  );
}

/**
 * 2 · Let your coding apps use Cua: one question, asked once. It names the
 * coding apps found on this computer; Set up adds Cua to all of them, Not now
 * moves on. Later installs are set up the same way, with a notice and Undo
 * (Settings, Connectors). Which apps there are, and what Connect does for
 * each, is still the app core's (`firstRun.view`).
 */
function Connect({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const c = view.connect!;
  const { data } = useBridge();
  const remembered = useConnectedAppsState();
  const [tried, setTried] = useState(false);
  const [busy, setBusy] = useState(false);
  const [byHand, setByHand] = useState(false);
  const [copied, setCopied] = useState(false);
  const rows = c.agents;
  const looking = rows.length === 0 && /^Looking/.test(c.lede);
  const none = rows.length === 0 && !looking;
  const pending = rows.filter((a) => !a.connected);
  const ready = rows.filter((a) => a.connected);
  const working = busy || rows.some((a) => a.note === "Connecting\u2026");
  const failed = tried && !working ? pending : [];
  const why = (agent: string, name: string) =>
    failureText(name, fr.agents.find((r) => r.agent === agent)?.detail || (c.error ?? "").replace(/^Couldn't connect Cua:\s*/, ""));
  const ids = rows.map((a) => a.agent);
  // Advanced (the core's mode: its link then offers Simple) shows each app on its own row.
  const advanced = c.mode?.id === "simple";

  const setUp = async () => {
    setTried(true);
    setBusy(true);
    // One yes covers these apps and the ones installed later (each with a notice and Undo).
    setConnectedAppsState({ ...remembered, known: [...new Set([...(remembered.known ?? []), ...ids])], auto: true });
    try {
      for (const a of pending) await fr.connect(a.agent);
      // One consent: they may also use the computer in the background, where Cua Driver is here.
      if (data) await data.call("agents.setupDriver", { agents: pending.map((a) => a.agent) }).catch(() => {});
    } finally {
      setBusy(false);
    }
  };
  const notNow = () => {
    setConnectedAppsState({ ...remembered, known: [...new Set([...(remembered.known ?? []), ...ids])], auto: false });
    void fr.act("skip");
  };
  const retry = async (agent: string) => {
    setBusy(true);
    try {
      await fr.connect(agent);
    } finally {
      setBusy(false);
    }
  };
  const copy = () => {
    void navigator.clipboard?.writeText(FIRST_PROMPT).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  const usage = fr.usage ? (
    <label className="mt-5 flex items-center gap-2.5 text-[13px] text-muted-foreground" data-first-run-usage="">
      <Switch checked={fr.usage.on} disabled={!fr.usage.enabled} onCheckedChange={(on) => void fr.setUsage(on).catch(() => {})} aria-label={fr.usage.label} />
      <span>
        {fr.usage.label}.{" "}
        <button type="button" className="underline hover:text-foreground" onClick={fr.openUsageLink}>
          {fr.usage.linkLabel}
        </button>
      </span>
    </label>
  ) : null;

  if (advanced) return <AdvancedConnect fr={fr} view={view} />;

  if (looking) {
    return (
      <Step id="connect" onBack={fr.back}>
        <Heading>Looking for your coding apps…</Heading>
      </Step>
    );
  }

  if (none) {
    return (
      <Step id="connect" onBack={fr.back}>
        <div data-first-run-connect-state="none">
          <Heading>We didn’t find a coding app yet.</Heading>
          <Lede>Cua works with coding apps such as Claude Code, Codex and Cursor. Install one and Cua will notice.</Lede>
          <Rows className="mt-8" data-first-run-agents="">
            {HEADLINE_APPS.map((a) => {
              const url = getItUrl(a.agent);
              return (
                <div key={a.agent} className="flex min-h-12 items-center gap-3 py-2" data-first-run-missing={a.agent}>
                  <AgentMark agent={a.agent} name={a.name} />
                  <span className="min-w-0 flex-1 text-[14px]">{a.name}</span>
                  {url ? (
                    <Button variant="outline" className="h-9 shrink-0 gap-1.5 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={() => void fr.openUrl(url)}>
                      Download <ExternalLinkIcon className="size-3.5" />
                    </Button>
                  ) : null}
                </div>
              );
            })}
            <OtherMachineRow />
          </Rows>
          <div className="mt-8 flex flex-wrap items-center gap-3">
            <Primary label="Check again" onClick={() => void fr.act("recheck")} data-first-run-recheck="" />
            <Secondary label="Continue without one" onClick={() => void fr.act("skip")} />
          </div>
          <p className="mt-4 text-[13px] text-muted-foreground">Spaces work without a coding app. You can set one up later in Settings.</p>
          {usage}
        </div>
      </Step>
    );
  }

  const all = pending.length === 0;
  // Up to three names read well in a heading; more are "your 5 coding apps" and the rows name them.
  const phrase = (list: { name: string }[]) => (list.length <= 3 ? listNames(list.map((a) => a.name)) : `your ${list.length} coding apps`);
  const title = all
    ? sentence(`${phrase(rows)} can use Cua.`)
    : failed.length
      ? `${ready.length ? sentence(`${phrase(ready)} can use Cua.`) + " " : ""}${sentence(`${phrase(failed)} ${failed.length === 1 ? "needs" : "need"} a hand.`)}`
      : `Let ${phrase(rows)} use Cua?`;

  return (
    <Step id="connect" onBack={fr.back}>
      <div data-first-run-connect-state={all ? "done" : failed.length ? "failed" : "ask"}>
        <Heading>{title}</Heading>
        <Lede>
          {all
            ? "If one of them is open, restart it first. Then try this:"
            : failed.length
              ? failed.length === 1
                ? `Cua couldn\u2019t change ${failed[0]!.name}\u2019s settings, so ${failed[0]!.name} can\u2019t use Cua yet.`
                : `Cua couldn\u2019t change their settings, so ${listNames(failed.map((a) => a.name))} can\u2019t use Cua yet.`
              : "Then you can ask them for things like \u201cReproduce this bug on a fresh Windows machine.\u201d They create the Space, work in it and tell you what they found."}
        </Lede>
        {all ? (
          <div className="mt-6 flex items-start gap-3 rounded-2xl border border-foreground/15 px-5 py-4" data-first-run-prompt="">
            <p className="min-w-0 flex-1 text-[15px] leading-relaxed select-text">{FIRST_PROMPT}</p>
            <button type="button" aria-label="Copy the prompt" onClick={copy} className="mt-0.5 text-muted-foreground hover:text-foreground">
              {copied ? <CheckIcon className="size-4" /> : <CopyIcon className="size-4" />}
            </button>
          </div>
        ) : null}
        <Rows className="mt-8" data-first-run-agents="">
          {rows.map((a) => {
            const bad = failed.some((f) => f.agent === a.agent);
            return (
              <div key={a.agent} className="flex min-h-12 items-center gap-3 py-2" data-first-run-agent={a.agent} data-connected={String(a.connected)} data-failed={bad ? "" : undefined}>
                <AgentMark agent={a.agent} name={a.name} />
                <div className="min-w-0 flex-1">
                  <div className="text-[14px]">{a.name}</div>
                  {bad ? <div className="text-[13px] text-destructive">{why(a.agent, a.name)}</div> : null}
                </div>
                {a.connected ? (
                  <span className="flex shrink-0 items-center gap-1 text-[13px] text-[#1f9d55] dark:text-[#4ade80]">
                    <CheckIcon className="size-3.5" /> Ready
                  </span>
                ) : bad ? (
                  <Button variant="outline" disabled={working} className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={() => void retry(a.agent)} data-first-run-retry={a.agent}>
                    Try again
                  </Button>
                ) : working ? (
                  <span className="shrink-0 text-[13px] text-muted-foreground">Setting up…</span>
                ) : null}
              </div>
            );
          })}
          <OtherMachineRow />
        </Rows>
        <div className="mt-8 flex flex-wrap items-center gap-3">
          {all || failed.length ? (
            <>
              <Primary label="Continue" onClick={() => void fr.act(c.primary.id)} />
              {all ? <Secondary label={copied ? "Copied" : "Copy the prompt"} onClick={copy} /> : <Secondary label="Add it by hand" onClick={() => setByHand(!byHand)} />}
            </>
          ) : (
            <>
              <Primary label="Set up" onClick={() => void setUp()} data-first-run-set-up="" />
              <Secondary label="Not now" onClick={notNow} />
            </>
          )}
          {c.mode ? (
            <button
              type="button"
              className="ml-auto text-[13px] text-muted-foreground outline-none hover:text-foreground focus-visible:underline"
              onClick={() => void fr.act(c.mode!.id)}
              data-first-run-mode={c.mode.id}
            >
              {c.mode.label}
            </button>
          ) : null}
        </div>
        {byHand && failed.length ? (
          <div className="mt-4 space-y-3" data-first-run-by-hand="">
            {failed.map((a) =>
              BY_HAND[a.agent] ? (
                <div key={a.agent}>
                  <Snippet file={BY_HAND[a.agent]!.file} text={BY_HAND[a.agent]!.snippet} />
                  <p className="mt-2 text-xs text-muted-foreground">Paste this into the file, then restart {a.name}.</p>
                </div>
              ) : null,
            )}
          </div>
        ) : null}
        <p className="mt-4 max-w-[460px] text-[13px] text-muted-foreground" data-first-run-consent="">
          {all
            ? "Cua added itself to each app\u2019s settings and left everything else alone."
            : failed.length
              ? `${listNames(ready.map((a) => a.name))} ${ready.length === 1 ? "is" : "are"} done. ${listNames(failed.map((a) => a.name))} can wait: Settings has the same button.`
              : "Cua adds itself to each app\u2019s settings and leaves everything else alone. Apps you install later are set up the same way, with a notice and Undo. You can turn it off any time in Settings."}
        </p>
        {usage}
      </div>
    </Step>
  );
}

/**
 * 2 · Advanced: each coding app on its own row with the file Connect changes,
 * Connect (Cua's tools, and the sign-in copied to Keyvault) or Disconnect for
 * one whose files are gone, and the sign-ins agents in your Spaces use.
 * Simple is the one question above (`Connect`).
 */
function AdvancedConnect({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const c = view.connect!;
  const keys = useAgentKeys();
  const [copied, setCopied] = useState(false);
  return (
    <Step id="connect" onBack={fr.back}>
      <Heading>{c.title}</Heading>
      <Lede>{c.lede}</Lede>
      {c.agents.length ? (
        <Rows className="mt-8" data-first-run-agents="">
          {c.agents.map((a) => (
            <div key={a.agent} className="flex min-h-12 items-center gap-3 py-2" data-first-run-agent={a.agent} data-connected={String(a.connected)}>
              <AgentMark agent={a.agent} name={a.name} />
              <div className="min-w-0 flex-1">
                <span className="text-[14px]">{a.name}</span>
                {a.file ? <span className="ml-2 font-mono text-[12px] text-muted-foreground">{a.file}</span> : null}
                {/* A problem's line goes under the name; it is too long for the right. */}
                {a.note && !a.connected && a.action ? <span className="block text-[12px] text-muted-foreground" data-first-run-agent-note="">{a.note}</span> : null}
              </div>
              {a.note && (a.connected || !a.action) ? (
                <span className={cn("shrink-0 text-[13px]", a.connected ? "text-[#1f9d55]" : "text-muted-foreground")}>{a.note}</span>
              ) : null}
              {a.action ? (
                <Button
                  variant="outline"
                  className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium"
                  onClick={() => void (a.action!.id === "disconnect" ? fr.disconnect(a.agent) : fr.connect(a.agent))}
                  data-first-run-connect={a.agent}
                  data-first-run-action-id={a.action.id}
                >
                  {a.action.label}
                </Button>
              ) : null}
            </div>
          ))}
        </Rows>
      ) : null}
      {c.alternatives.length ? (
        <Rows className="mt-6" data-first-run-alternatives="">
          {c.alternatives.map((alt) => (
            <div key={alt.id} className="flex min-h-16 items-center gap-3 py-2" data-first-run-alternative={alt.id}>
              <div className="min-w-0 flex-1">
                <div className="text-[14px]">{alt.title}</div>
                <div className="text-[13px] text-muted-foreground">{alt.note}</div>
                {alt.command ? <code className="mt-1 block font-mono text-[12.5px]">{alt.command}</code> : null}
              </div>
              {alt.url ? (
                <Button variant="outline" className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={() => void fr.openUrl(alt.url!)}>
                  Open
                </Button>
              ) : alt.command ? (
                <Button
                  variant="outline"
                  className="h-9 shrink-0 gap-1.5 rounded-full border-foreground/25 px-4 text-[13px] font-medium"
                  onClick={() => {
                    void navigator.clipboard?.writeText(alt.command!).catch(() => {});
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  }}
                >
                  {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
                  {copied ? "Copied" : "Copy"}
                </Button>
              ) : null}
            </div>
          ))}
        </Rows>
      ) : null}
      {c.credentials.length ? (
        <>
          <Rows className="mt-6" data-first-run-credentials="">
            {c.credentials.map((r) => (
              <CredentialRowView
                key={r.id}
                row={r}
                onAct={() => {
                  if (r.action?.id === "agent-sign-in" && r.agent) void fr.signIn(r.agent);
                  else if (r.action?.id === "add-key" && (r.provider === "anthropic" || r.provider === "openai")) keys.openSheet(r.provider);
                }}
              />
            ))}
          </Rows>
          <p className="mt-3 text-xs text-muted-foreground" data-first-run-credentials-note="">
            {c.credentialsNote}
          </p>
        </>
      ) : null}
      {c.error ? (
        <p role="alert" className="mt-3 text-[13px] text-destructive">
          {c.error}
        </p>
      ) : null}
      {c.consent ? (
        <p className="mt-3 text-xs text-muted-foreground" data-first-run-consent="">
          {c.consent}
        </p>
      ) : null}
      <div className="mt-8 flex flex-wrap items-center gap-3">
        <Primary label={c.primary.label} onClick={() => void fr.act(c.primary.id)} />
        {c.secondary ? <Secondary label={c.secondary.label} onClick={() => void fr.act(c.secondary!.id)} /> : null}
        {c.mode ? (
          <button
            type="button"
            className="ml-auto text-[13px] text-muted-foreground outline-none hover:text-foreground focus-visible:underline"
            onClick={() => void fr.act(c.mode!.id)}
            data-first-run-mode={c.mode.id}
          >
            {c.mode.label}
          </button>
        ) : null}
      </div>
      {fr.usage ? (
        <label className="mt-5 flex items-center gap-2.5 text-[13px] text-muted-foreground" data-first-run-usage="">
          <Switch checked={fr.usage.on} disabled={!fr.usage.enabled} onCheckedChange={(on) => void fr.setUsage(on).catch(() => {})} aria-label={fr.usage.label} />
          <span>
            {fr.usage.label}.{" "}
            <button type="button" className="underline hover:text-foreground" onClick={fr.openUsageLink}>
              {fr.usage.linkLabel}
            </button>
          </span>
        </label>
      ) : null}
      {keys.sheet ? <KeySheet key={keys.sheet.provider} keys={keys} provider={keys.sheet.provider} env={keys.sheet.env} /> : null}
    </Step>
  );
}


function CredentialRowView({ row: r, onAct }: { row: FirstRunCredential; onAct: () => void }) {
  return (
    <div className="flex min-h-16 items-center gap-3 py-2" data-first-run-credential={r.id}>
      <div className="min-w-0 flex-1">
        <div className="text-[14px]">{r.title}</div>
        <div className="text-xs text-muted-foreground">{r.note}</div>
        {r.error ? (
          <div role="alert" className="mt-0.5 text-xs text-destructive">
            {r.error}
          </div>
        ) : null}
      </div>
      {r.status ? (
        <span className={cn("shrink-0 text-[13px]", r.ok ? "text-[#1f9d55]" : "text-muted-foreground")} data-first-run-credential-status="">
          {r.status}
        </span>
      ) : null}
      {r.action ? (
        <Button variant="outline" className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={onAct} data-first-run-credential-action={r.action.id}>
          {r.action.label}
        </Button>
      ) : null}
    </div>
  );
}

/* ---- 3 · Add your other computers -------------------------------------------------- */


/* ---- 3 · Add your other computers -------------------------------------------------- */

function Devices({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const d = view.devices!;
  return (
    <Step id="devices" onBack={fr.back}>
      <Heading>{d.title}</Heading>
      <Lede>{d.lede}</Lede>
      <Rows className="mt-8" data-first-run-devices="">
        {d.rows.map((r) => (
          <div key={r.title} className="flex min-h-12 items-center gap-3 py-2 text-[14px]">
            <span className="w-48 shrink-0">{r.title}</span>
            <span className="text-[13px] text-muted-foreground">{r.note}</span>
          </div>
        ))}
        {d.status ? <StatusLine row={d.status} onAct={() => void fr.act(d.status!.action?.id ?? "")} /> : null}
      </Rows>
      <div className="mt-8 flex flex-wrap items-center gap-3">
        <Primary label={d.primary.label} onClick={() => void fr.act(d.primary.id)} data-first-run-devices-primary={d.primary.id} />
        {d.secondary ? <Secondary label={d.secondary.label} onClick={() => void fr.act(d.secondary!.id)} describedBy={d.note ? "first-run-devices-note" : undefined} /> : null}
      </div>
      {d.note ? (
        <p id="first-run-devices-note" className="mt-4 text-[13px] text-muted-foreground" data-first-run-devices-note="">
          {d.note}
        </p>
      ) : null}
    </Step>
  );
}

/* ---- Install the Cua CLI (only while it is missing) ----------------------------------- */

/** What Install writes (the exact file, and the PATH line when one is
 * needed), Install or Not now, and the result with a command to try. */
function CommandLine({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const c = view.cli!;
  return (
    <Step id="cli" onBack={fr.back}>
      <Heading>{c.title}</Heading>
      <Lede>{c.lede}</Lede>
      <Rows className="mt-8" data-first-run-cli="">
        {c.facts.map((f) => (
          <div key={f.label} className="flex min-h-12 items-center gap-3 py-2 text-[14px]" data-first-run-cli-fact={f.label}>
            <span className="w-48 shrink-0">{f.label}</span>
            <code className="min-w-0 font-mono text-[13px] break-all text-muted-foreground">{f.value}</code>
          </div>
        ))}
        {c.installed && c.tryCommand ? (
          <div className="flex min-h-12 items-center gap-3 py-2" data-first-run-cli-done="">
            <StateIcon state="ok" />
            <div className="min-w-0 flex-1 text-[14px]">
              Open a new Terminal window and try <code className="font-mono text-[13px]">{c.tryCommand}</code>
            </div>
          </div>
        ) : null}
      </Rows>
      {c.error ? (
        <p role="alert" className="mt-3 text-[13px] text-destructive" data-first-run-cli-error="">
          {c.error}
        </p>
      ) : null}
      <div className="mt-8 flex flex-wrap items-center gap-3">
        {c.primary ? (
          <Primary label={c.busy ? "Installing…" : c.primary.label} onClick={() => !c.busy && void fr.act(c.primary!.id)} data-first-run-cli-primary={c.primary.id} />
        ) : null}
        {c.secondary && !c.busy ? <Secondary label={c.secondary.label} onClick={() => void fr.act(c.secondary!.id)} /> : null}
      </div>
    </Step>
  );
}

/* ---- 4 · Share this Mac's screen (a signed-in Mac that shares it) ------------------- */

function Permissions({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const p = view.permissions!;
  return (
    <Step id="permissions" onBack={fr.back}>
      <Heading>{p.title}</Heading>
      <Lede>{p.lede}</Lede>
      <Rows className="mt-8" data-first-run-permissions="">
        {p.rows.map((r) => (
          <StatusLine key={r.id} row={r} onAct={r.action ? () => void fr.act(r.action!.id, r.settingsUrl) : undefined} />
        ))}
      </Rows>
      <p className="mt-4 text-[13px] text-muted-foreground" data-first-run-permissions-note="">
        {p.note}
      </p>
      <div className="mt-8 flex flex-wrap items-center gap-3">
        {p.primary ? <Primary label={p.primary.label} onClick={() => void fr.act(p.primary!.id)} data-first-run-permissions-primary={p.primary.id} /> : null}
        {p.secondary ? <Secondary label={p.secondary.label} onClick={() => void fr.act(p.secondary!.id)} data-first-run-permissions-secondary={p.secondary.id} /> : null}
      </div>
    </Step>
  );
}

/* ---- 5 · Try it ---------------------------------------------------------------------- */

function Setup({ fr, view, onFinish }: { fr: FirstRunHook; view: FirstRunView; onFinish: () => void }) {
  const s = view.setup!;
  const [copied, setCopied] = useState(false);
  const [tab, setTab] = useState<string | null>(null);
  const tabs = s.prompt?.tabs ?? [];
  const shownTab = tabs.find((t) => t.id === (tab ?? s.prompt?.selected));
  const promptText = shownTab?.text ?? s.prompt?.text ?? "";
  const copy = async () => {
    await fr.copyPrompt(promptText);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <Step id="setup" onBack={fr.back}>
      <Heading>{s.title}</Heading>
      <Lede>{s.lede}</Lede>
      {s.prompt && tabs.length > 1 ? (
        <div className="mt-8" data-first-run-prompt-tabs="">
          <Segmented
            aria-label="Operating system"
            className="flex h-9 w-full [&>button]:h-8 [&>button]:flex-1 [&>button]:justify-center [&>button]:text-[13px]"
            value={shownTab?.id ?? tabs[0]!.id}
            options={tabs.map((t) => ({ value: t.id, label: t.label }))}
            onValueChange={(v) => {
              setTab(v);
              setCopied(false);
            }}
          />
        </div>
      ) : null}
      {s.prompt ? (
        <div className={cn("relative", tabs.length > 1 ? "mt-2" : "mt-8")}>
          <textarea
            readOnly
            rows={3}
            value={promptText}
            aria-label={s.prompt.heading}
            onFocus={(e) => e.currentTarget.select()}
            className="block w-full resize-none rounded-lg border border-foreground/15 bg-transparent py-3 pr-24 pl-4 font-mono text-[13px] leading-relaxed outline-none select-text focus-visible:ring-2 focus-visible:ring-ring/60"
            data-first-run-prompt=""
          />
          {s.prompt.copy ? (
            <Button variant="outline" className="absolute top-2 right-2 h-8 gap-1.5 rounded-full border-foreground/25 bg-background/60 px-3 text-[13px] font-medium" onClick={() => void copy()} data-first-run-copy="">
              {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
              {copied ? "Copied" : s.prompt.copy}
            </Button>
          ) : null}
        </div>
      ) : null}
      {s.status ? (
        <div className="mt-4">
          <StatusLine row={s.status} />
        </div>
      ) : null}
      {s.extra ? <ExtraCard extra={s.extra} fr={fr} /> : null}
      <div className="mt-8 flex items-center gap-3">
        <Button size="lg" className="h-11 min-w-28 rounded-full px-6 text-[15px]" disabled={!s.primary.enabled} data-onboarding-primary="" data-first-run-primary={s.primary.id} onClick={onFinish}>
          {s.primary.label}
        </Button>
      </div>
    </Step>
  );
}

function StateIcon({ state }: { state: FirstRunRow["state"] | "later" }) {
  if (state === "ok")
    return (
      <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-[#1f9d55] text-white" aria-label="Done">
        <CheckIcon className="size-3" strokeWidth={3} />
      </span>
    );
  if (state === "fix") return <span className="size-5 shrink-0 rounded-full border-2 border-foreground/30" aria-label="Needs attention" />;
  if (state === "later") return <span className="size-5 shrink-0 rounded-full border border-dashed border-foreground/40" aria-hidden />;
  return <span className="size-5 shrink-0 animate-spin rounded-full border-2 border-foreground/15 border-t-foreground/60" aria-label="Checking" />;
}

function ExtraCard({ extra, fr }: { extra: FirstRunExtraView; fr: FirstRunHook }) {
  const act = (id: string) => void fr.extra(id);
  if (extra.phase === "choose") {
    return (
      <div className="mt-8" data-first-run-extra="choose">
        <div className="flex items-baseline gap-2">
          <h2 className="text-[15px] font-semibold">{extra.title}</h2>
          {extra.tag ? <span className="text-[13px] text-muted-foreground">{extra.tag}</span> : null}
        </div>
        <p className="mt-1 text-[13px] text-muted-foreground">{extra.note}</p>
        <div className="mt-3 flex flex-wrap items-stretch gap-2.5">
          {extra.choices.map((c) => (
            <button
              key={c.os}
              type="button"
              disabled={c.blocked !== null}
              title={c.blocked ?? undefined}
              onClick={() => act(c.os)}
              data-first-run-extra-choice={c.os}
              className="flex min-w-44 items-start gap-2.5 rounded-xl border px-3.5 py-2.5 text-left hover:bg-foreground/[0.03] disabled:cursor-not-allowed disabled:opacity-60"
            >
              <OsIcon os={c.os as "macos" | "linux"} className="mt-0.5 size-4 shrink-0" />
              <span className="min-w-0">
                <span className="block text-[14px] font-medium">{c.label}</span>
                <span className="block text-xs text-muted-foreground">{c.blocked ?? c.detail}</span>
              </span>
            </button>
          ))}
          {extra.action ? (
            <button type="button" className="rounded-md px-2 text-[14px] font-medium underline decoration-foreground/25 underline-offset-4 outline-none focus-visible:ring-2 focus-visible:ring-ring/60" onClick={() => act(extra.action!.id)}>
              {extra.action.label}
            </button>
          ) : null}
        </div>
        {/* Windows without Docker Desktop: install or start it from here, and the choices follow. */}
        {extra.choices.some((c) => c.blocked !== null) ? <RuntimeFixRow /> : null}
      </div>
    );
  }
  const icon = extra.phase === "ready" ? "ok" : extra.phase === "failed" ? "fix" : extra.phase === "not-now" ? "later" : "checking";
  return (
    <div className="mt-8 border-y border-foreground/10 py-3" data-first-run-extra={extra.phase}>
      <div className="flex items-start gap-3.5">
        <span className="mt-0.5 flex">
          <StateIcon state={icon} />
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="text-[15px] font-semibold">{extra.title}</h2>
          <p className={cn("text-[13px] text-muted-foreground", extra.phase === "failed" && "text-destructive")}>{extra.note}</p>
          {extra.phase === "creating" ? (
            <div
              className="mt-2 h-1 max-w-[400px] overflow-hidden rounded-full bg-foreground/10"
              role="progressbar"
              aria-label={extra.title}
              aria-valuenow={extra.fraction === null ? undefined : Math.round(extra.fraction * 100)}
            >
              <div className={cn("h-full rounded-full bg-brand", extra.fraction === null && "w-1/4 animate-pulse")} style={extra.fraction === null ? undefined : { width: `${Math.max(2, extra.fraction * 100)}%` }} />
            </div>
          ) : null}
          {extra.footnote ? <p className="mt-2.5 text-[13px] text-muted-foreground">{extra.footnote}</p> : null}
        </div>
        {extra.action ? (
          <Button variant="outline" className="h-9 shrink-0 rounded-full border-foreground/25 px-4 text-[13px] font-medium" onClick={() => act(extra.action!.id)} data-first-run-extra-action={extra.action.id}>
            {extra.action.label}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/* ---- Art and chrome ------------------------------------------------------------------- */

/** Light: a plain field with far keys. Dark: the starfield, darker on the text side. */
function Backdrop() {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0">
      <div className="absolute inset-0 hidden bg-[url(/onboarding/sky.jpg)] bg-cover bg-center brightness-[1.7] contrast-[1.08] dark:block" />
      <div className="absolute inset-0 hidden bg-gradient-to-r from-black/70 via-black/35 to-transparent dark:block" />
      <img src="/onboarding/depth.webp" alt="" className="absolute inset-0 size-full object-cover opacity-90" draggable={false} />
    </div>
  );
}

/**
 * The keys beside the page: the agent's key, the blue key for this
 * computer (faded while it is checked or needs a fix) and, on step 2, a
 * small dashed outline for the optional extra Space that fills in while
 * one is being made.
 */
function Art({ art, step }: { art: FirstRunArt; step: FirstRunView["step"] }) {
  const agent = ["claude-code", "codex", "hermes", "openclaw"].includes(art.agent) ? art.agent.replace("claude-code", "claude") : "other";
  const connect = step === "connect";
  // One fixed-ratio box, centred on the heading block and clamped to sit fully
  // inside the window, so the dashed outline is never cut at any size.
  return (
    <div aria-hidden className="pointer-events-none absolute inset-y-0 right-0 hidden w-[46%] [@media(min-width:1000px)]:block" data-first-run-art={`${art.agent} ${art.computer} ${art.extra}`}>
      <div
        className="absolute left-1/2 aspect-[6/7] -translate-x-1/2"
        style={{ height: "var(--ob-art-h)", top: "clamp(64px, calc(45% - var(--ob-art-h) / 2), calc(100% - var(--ob-art-h) - 24px))" }}
      >
        <img
          src={`/onboarding/agent-${agent}.webp`}
          alt=""
          draggable={false}
          className={cn("absolute w-[62%] transition-all duration-500", connect ? "top-[32%] left-[2%]" : "top-[36%] left-[6%]")}
        />
        <img
          src="/onboarding/cua.webp"
          alt=""
          draggable={false}
          className={cn("absolute top-[4%] right-[2%] w-[44%] transition-all duration-500", art.computer === "faded" && "opacity-40 blur-[1px]")}
        />
        {art.extra !== "none" ? (
          <div className="absolute right-[8%] bottom-[2%] w-[27%]">
            {art.extra === "outline" ? (
              <SlotOutline />
            ) : (
              <img src="/onboarding/extra.webp" alt="" draggable={false} className={cn("w-full transition-opacity", art.extra === "filling" ? "animate-pulse opacity-40" : "opacity-95")} />
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** The Spaces key's outline, dashed: an extra Space that doesn't exist yet. */
function SlotOutline() {
  return (
    <svg viewBox="0 0 486 470" className="w-full -rotate-6 text-foreground/35 dark:text-white/35">
      <polygon
        points="110,65 309,8 327,10 341,20 359,52 414,165 445,238 461,280 476,326 476,345 465,357 421,375 328,406 187,449 143,461 125,461 116,456 109,447 84,388 19,218 8,184 11,164 27,140 51,109 82,78 104,67"
        fill="currentColor"
        fillOpacity={0.05}
        stroke="currentColor"
        strokeWidth={4}
        strokeDasharray="14 16"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function Dots({ view }: { view: FirstRunView }) {
  return (
    <ol className="relative z-10 flex shrink-0 justify-start gap-2 pt-4 pb-6 pl-[var(--ob-x)]" aria-label={`Step ${view.current + 1} of ${view.dots}`}>
      {Array.from({ length: view.dots }, (_, i) => (
        <li key={i} aria-current={i === view.current ? "step" : undefined} className={cn("h-1 w-7 rounded-full bg-foreground/30", i <= view.current && "bg-foreground")} />
      ))}
    </ol>
  );
}
