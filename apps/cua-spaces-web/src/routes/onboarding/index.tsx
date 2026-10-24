// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { useAgentKeys, useFirstRun, type FirstRunArt, type FirstRunCredential, type FirstRunExtraView, type FirstRunHook, type FirstRunRow, type FirstRunView } from "@/bridge";
import { KeySheet } from "@/components/agents/key-sheet";
import { CuaLogo } from "@/components/cua-logo";
import { OsIcon } from "@/components/os-icon";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { RuntimeFixRow } from "@/components/new-space/runtime-fix";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/onboarding/")({ component: OnboardingPage });

/**
 * The first run, the same on macOS, Windows and Linux: Welcome, Pick your
 * agent, Connect your other devices (optional), Try it in your agent. Every
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

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-[#F4F5F7] dark:bg-[#07080a]" data-step={view.step}>
      <Backdrop />
      <Art art={view.art} step={view.step} />
      <header className="relative z-30 flex shrink-0 items-center justify-between px-10 pt-[calc(var(--titlebar-height)+12px)] pb-2">
        <CuaWordmark />
        <button type="button" onClick={leave} className="text-[13px] text-muted-foreground hover:text-foreground" data-onboarding-later="">
          {view.later}
        </button>
      </header>
      <div className="relative z-10 flex min-h-0 flex-1 flex-col overflow-y-auto px-10 pb-6 [@media(min-width:1000px)]:pr-[42%]">
        <div className="my-auto flex max-w-[600px] flex-col py-6">
          {view.welcome ? <Welcome fr={fr} view={view} /> : null}
          {view.connect ? <Connect fr={fr} view={view} /> : null}
          {view.devices ? <Devices fr={fr} view={view} /> : null}
          {view.setup ? <Setup fr={fr} view={view} onFinish={() => void finish()} /> : null}
        </div>
      </div>
      <Dots view={view} />
    </div>
  );
}

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
  return <p className="mt-3 text-[15px] leading-relaxed text-muted-foreground">{children}</p>;
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

function Quiet({ label, onClick, ...rest }: { label: string; onClick: () => void } & Record<`data-${string}`, string | undefined>) {
  return (
    <button type="button" className="text-[13px] text-muted-foreground hover:text-foreground" onClick={onClick} {...rest}>
      {label}
    </button>
  );
}

/** A line with a state: a check, a spinner, or a fix with its button. */
function StatusLine({ row, onAct }: { row: FirstRunRow; onAct?: () => void }) {
  return (
    <div className="flex items-center gap-3 py-2.5" data-first-run-status={row.id} data-state={row.state}>
      <StateIcon state={row.state} />
      <div className="min-w-0 flex-1">
        <div className="text-[14px]">{row.title}</div>
        {row.note ? <div className="text-[13px] text-muted-foreground">{row.note}</div> : null}
      </div>
      {row.action && onAct ? (
        <Button size="sm" variant="outline" className="shrink-0 rounded-full" onClick={onAct} data-first-run-action={row.action.id}>
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
      <Heading>{w.title}</Heading>
      <Lede>{w.lede}</Lede>
      <div className="mt-8">
        <Primary label={w.primary.label} onClick={() => void fr.act(w.primary.id)} />
      </div>
    </div>
  );
}

/* ---- 2 · Pick your agent ------------------------------------------------------------ */

const MARKS: Record<string, string> = {
  "claude-code": "/onboarding/marks/mark-claude.svg",
  codex: "/onboarding/marks/mark-openai.svg",
  hermes: "/onboarding/marks/mark-hermes.svg",
  openclaw: "/onboarding/marks/mark-openclaw.svg",
};

function AgentMark({ agent, name, className }: { agent: string; name?: string; className?: string }) {
  const src = MARKS[agent];
  if (!src)
    return (
      <span aria-hidden className={cn("flex size-4 shrink-0 items-center justify-center rounded-[4px] bg-foreground/10 text-[9px] font-semibold text-muted-foreground", className)}>
        {(name ?? agent).trim().charAt(0).toUpperCase()}
      </span>
    );
  return <img src={src} alt="" aria-hidden className={cn("size-4 shrink-0 object-contain", agent !== "claude-code" && agent !== "openclaw" && "dark:invert", className)} draggable={false} />;
}

function Connect({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const c = view.connect!;
  const keys = useAgentKeys();
  const [copied, setCopied] = useState(false);
  return (
    <div data-first-run="connect">
      <Heading>{c.title}</Heading>
      <Lede>{c.lede}</Lede>
      {c.agents.length ? (
        <Rows className="mt-6" data-first-run-agents="">
          {c.agents.map((a) => (
            <div key={a.agent} className="flex min-h-11 items-center gap-3 py-2" data-first-run-agent={a.agent} data-connected={String(a.connected)}>
              <AgentMark agent={a.agent} name={a.name} />
              <div className="min-w-0 flex-1">
                <span className="text-[14px]">{a.name}</span>
                {a.file ? <span className="ml-2 font-mono text-[12px] text-muted-foreground">{a.file}</span> : null}
              </div>
              {a.note ? (
                <span className={cn("shrink-0 text-[13px]", a.connected ? "text-[#1f9d55]" : "text-muted-foreground")}>{a.note}</span>
              ) : null}
              {a.action ? (
                <Button size="sm" variant="outline" className="shrink-0 rounded-full" onClick={() => void fr.connect(a.agent)} data-first-run-connect={a.agent}>
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
            <div key={alt.id} className="flex items-center gap-3 py-2.5" data-first-run-alternative={alt.id}>
              <div className="min-w-0 flex-1">
                <div className="text-[14px]">{alt.title}</div>
                <div className="text-[13px] text-muted-foreground">{alt.note}</div>
                {alt.command ? <code className="mt-1 block font-mono text-[12.5px]">{alt.command}</code> : null}
              </div>
              {alt.url ? (
                <Button size="sm" variant="outline" className="shrink-0 rounded-full" onClick={() => void fr.openUrl(alt.url!)}>
                  Open
                </Button>
              ) : alt.command ? (
                <Button
                  size="sm"
                  variant="outline"
                  className="shrink-0 gap-1.5 rounded-full"
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
          <Rows className="mt-4" data-first-run-credentials="">
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
          <p className="mt-2 text-xs text-muted-foreground" data-first-run-credentials-note="">
            {c.credentialsNote}
          </p>
        </>
      ) : null}
      {c.error ? (
        <p role="alert" className="mt-3 text-[13px] text-destructive">
          {c.error}
        </p>
      ) : null}
      <div className="mt-6 flex items-center gap-4">
        <Primary label={c.primary.label} onClick={() => void fr.act(c.primary.id)} />
        {c.secondary ? <Quiet label={c.secondary.label} onClick={() => void fr.act(c.secondary!.id)} data-first-run-secondary="" /> : null}
        <Quiet label="Back" onClick={fr.back} data-first-run-back="" />
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
    </div>
  );
}

function CredentialRowView({ row: r, onAct }: { row: FirstRunCredential; onAct: () => void }) {
  return (
    <div className="flex items-center gap-3 py-2.5" data-first-run-credential={r.id}>
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
        <Button size="sm" variant="outline" className="shrink-0 rounded-full" onClick={onAct} data-first-run-credential-action={r.action.id}>
          {r.action.label}
        </Button>
      ) : null}
    </div>
  );
}

/* ---- 3 · Connect your other devices -------------------------------------------------- */

function Devices({ fr, view }: { fr: FirstRunHook; view: FirstRunView }) {
  const d = view.devices!;
  return (
    <div data-first-run="devices">
      <Heading>{d.title}</Heading>
      <Lede>{d.lede}</Lede>
      <Rows className="mt-6" data-first-run-devices="">
        {d.rows.map((r) => (
          <div key={r.title} className="flex items-baseline gap-3 py-2.5 text-[14px]">
            <span className="w-48 shrink-0">{r.title}</span>
            <span className="text-[13px] text-muted-foreground">{r.note}</span>
          </div>
        ))}
        {d.status ? <StatusLine row={d.status} onAct={() => void fr.act(d.status!.action?.id ?? "")} /> : null}
      </Rows>
      <div className="mt-7 flex items-center gap-4">
        <Primary label={d.primary.label} onClick={() => void fr.act(d.primary.id)} data-first-run-devices-primary={d.primary.id} />
        {d.secondary ? <Quiet label={d.secondary.label} onClick={() => void fr.act(d.secondary!.id)} data-first-run-secondary="" /> : null}
        <Quiet label="Back" onClick={fr.back} data-first-run-back="" />
      </div>
      {d.note ? <p className="mt-4 text-[13px] text-muted-foreground">{d.note}</p> : null}
    </div>
  );
}

/* ---- 4 · Try it ---------------------------------------------------------------------- */

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
    <div data-first-run="setup">
      <Heading>{s.title}</Heading>
      <Lede>{s.lede}</Lede>
      {s.prompt && tabs.length > 1 ? (
        <div className="mt-6" data-first-run-prompt-tabs="">
          <Segmented
            aria-label="System"
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
        <div className={cn("flex items-start gap-3", tabs.length > 1 ? "mt-3" : "mt-6")}>
          <textarea
            readOnly
            rows={2}
            value={promptText}
            aria-label={s.prompt.heading}
            onFocus={(e) => e.currentTarget.select()}
            className="min-w-0 flex-1 resize-none rounded-lg border border-foreground/15 bg-transparent px-3 py-2 font-mono text-[13px] leading-relaxed outline-none select-text focus-visible:ring-2 focus-visible:ring-ring/60"
            data-first-run-prompt=""
          />
          {s.prompt.copy ? (
            <Button variant="outline" size="sm" className="shrink-0 gap-1.5 rounded-full" onClick={() => void copy()} data-first-run-copy="">
              {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
              {copied ? "Copied" : s.prompt.copy}
            </Button>
          ) : null}
        </div>
      ) : null}
      {s.status ? (
        <div className="mt-3">
          <StatusLine row={s.status} />
        </div>
      ) : null}
      {s.extra ? <ExtraCard extra={s.extra} fr={fr} /> : null}
      <div className="mt-8 flex items-center gap-4">
        <Button size="lg" className="h-11 min-w-28 rounded-full px-6 text-[15px]" disabled={!s.primary.enabled} data-onboarding-primary="" data-first-run-primary={s.primary.id} onClick={onFinish}>
          {s.primary.label}
        </Button>
        <Quiet label="Back" onClick={fr.back} data-first-run-back="" />
      </div>
    </div>
  );
}

function StateIcon({ state }: { state: FirstRunRow["state"] | "later" }) {
  if (state === "ok")
    return (
      <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-[#1f9d55] text-white" aria-label="Done">
        <CheckIcon className="size-3" strokeWidth={3} />
      </span>
    );
  if (state === "fix") return <span className="size-5 shrink-0 rounded-full border-[5px] border-[#d18a12]" aria-label="Needs a fix" />;
  if (state === "later") return <span className="size-5 shrink-0 rounded-full border border-dashed border-foreground/40" aria-hidden />;
  return <span className="size-5 shrink-0 animate-spin rounded-full border-2 border-foreground/15 border-t-foreground/60" aria-label="Checking" />;
}

function ExtraCard({ extra, fr }: { extra: FirstRunExtraView; fr: FirstRunHook }) {
  const act = (id: string) => void fr.extra(id);
  if (extra.phase === "choose") {
    return (
      <div className="mt-6" data-first-run-extra="choose">
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
            <button type="button" className="px-2 text-[14px] font-medium underline decoration-foreground/25 underline-offset-4" onClick={() => act(extra.action!.id)}>
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
    <div className="mt-6 border-y border-foreground/10 py-3" data-first-run-extra={extra.phase}>
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
          <Button variant="outline" size="sm" className="shrink-0 rounded-full" onClick={() => act(extra.action!.id)} data-first-run-extra-action={extra.action.id}>
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
  return (
    <div aria-hidden className="pointer-events-none absolute inset-y-0 right-0 hidden w-[46%] [@media(min-width:1000px)]:block" data-first-run-art={`${art.agent} ${art.computer} ${art.extra}`}>
      <img
        src={`/onboarding/agent-${agent}.webp`}
        alt=""
        draggable={false}
        className={cn("absolute w-[56%] max-w-[420px] transition-all duration-500", connect ? "top-[30%] left-[4%]" : "top-[34%] left-[10%]")}
      />
      <img
        src="/onboarding/cua.webp"
        alt=""
        draggable={false}
        className={cn("absolute top-[12%] right-[6%] w-[40%] max-w-[300px] transition-all duration-500", art.computer === "faded" && "opacity-40 blur-[1px]")}
      />
      {art.extra !== "none" ? (
        <div className="absolute right-[12%] bottom-[10%] w-[24%] max-w-[190px]">
          {art.extra === "outline" ? (
            <SlotOutline />
          ) : (
            <img src="/onboarding/extra.webp" alt="" draggable={false} className={cn("w-full transition-opacity", art.extra === "filling" ? "animate-pulse opacity-40" : "opacity-95")} />
          )}
        </div>
      ) : null}
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
    <ol className="relative z-10 flex shrink-0 justify-center gap-2 pb-5" aria-label={`Step ${view.current + 1} of ${view.dots}`}>
      {Array.from({ length: view.dots }, (_, i) => (
        <li key={i} aria-current={i === view.current ? "step" : undefined} className={cn("h-0.5 w-6 rounded-full bg-foreground/20", i <= view.current && "bg-foreground/80")} />
      ))}
    </ol>
  );
}
