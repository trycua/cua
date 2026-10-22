// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon, CopyIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { useAgentKeys, useFirstRun, type FirstRunAgentPick, type FirstRunArt, type FirstRunCredential, type FirstRunExtraView, type FirstRunHook, type FirstRunRow, type FirstRunUpsell, type FirstRunView } from "@/bridge";
import { KeySheet } from "@/components/agents/key-sheet";
import { Checkbox } from "@/components/ui/checkbox";
import { CuaKey } from "@/components/cua-key";
import { CuaLogo } from "@/components/cua-logo";
import { OsIcon } from "@/components/os-icon";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { RuntimeFixRow } from "@/components/new-space/runtime-fix";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/onboarding/")({ component: OnboardingPage });

/**
 * The first run, the same on macOS, Windows and Linux: pick any agents (or
 * none), then "Your agent can use this Mac". Every word and state is the app
 * core's (`firstRun.view`, through `useFirstRun`); this file places them
 * beside the keycap art.
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
        {view.connect ? <Connect fr={fr} view={view} /> : null}
        {view.setup ? <Setup fr={fr} view={view} onFinish={() => void finish()} /> : null}
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

/* ---- Step 1: connect your agent --------------------------------------------------- */

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
  const [moreOpen, setMoreOpen] = useState(false);
  const [before, after] = c.title.split(c.accent);
  const busy = Boolean(c.busy);
  // A ticked agent from "More agents" stays in view.
  const more = moreOpen ? c.more : [];
  return (
    <div className="my-auto flex max-w-[560px] flex-col py-8" data-first-run="connect">
      <h1
        tabIndex={-1}
        data-onboarding-heading=""
        className="font-display text-[clamp(36px,5vw,56px)] leading-[1.02] font-semibold tracking-[-0.03em] text-balance outline-none"
      >
        {before}
        {after !== undefined ? (
          <>
            <span className="font-accent font-normal tracking-normal italic">{c.accent}</span>
            {after}
          </>
        ) : null}
      </h1>
      <p className="mt-4 max-w-[520px] text-[15px] leading-relaxed text-muted-foreground">{c.lede}</p>
      <p className="mt-7 text-[14px] font-medium" data-first-run-intro="">
        {c.intro}
      </p>

      <section className="mt-3" data-first-run-agents="">
        <h2 className="mb-1.5 px-1 text-xs font-semibold text-muted-foreground">{c.agentsLabel}</h2>
        <Card className="p-1.5">
          {[...c.agents, ...more].map((p) => (
            <AgentPickRow key={p.agent} pick={p} disabled={busy} onChange={(on) => fr.toggle(p.agent, on)} />
          ))}
          {c.more.length ? (
            <button
              type="button"
              className="flex w-full items-center gap-1.5 rounded-lg px-3 py-1.5 text-left text-[13px] text-muted-foreground hover:text-foreground"
              aria-expanded={moreOpen}
              data-first-run-more=""
              onClick={() => setMoreOpen((v) => !v)}
            >
              <ChevronRightIcon className={cn("size-3.5 transition-transform", moreOpen && "rotate-90")} aria-hidden />
              {c.moreLabel} ({c.more.length})
            </button>
          ) : null}
        </Card>
        <p className="mt-2 px-1 text-xs text-muted-foreground" data-first-run-agents-note="">
          {c.agentsNote}
        </p>
      </section>

      {c.credentials.length ? (
        <section className="mt-6" data-first-run-credentials="">
          <h2 className="mb-1.5 px-1 text-xs font-semibold text-muted-foreground">{c.credentialsLabel}</h2>
          <Card className="divide-y p-0">
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
          </Card>
          <p className="mt-2 px-1 text-xs text-muted-foreground" data-first-run-credentials-note="">
            {c.credentialsNote}
          </p>
        </section>
      ) : null}

      <div className="mt-8 flex items-center gap-4">
        <Button
          size="lg"
          className="h-11 min-w-28 rounded-full px-6 text-[15px]"
          data-onboarding-primary=""
          data-first-run-continue={c.connects.join(",")}
          disabled={busy}
          autoFocus
          onClick={() => void fr.connect()}
        >
          {c.busy ?? c.primary.label}
        </Button>
      </div>
      {c.error ? (
        <p role="alert" className="mt-3 text-[13px] text-destructive">
          {c.error}
        </p>
      ) : null}
      {fr.usage ? (
        <div className="mt-8 flex max-w-[520px] flex-col gap-3 text-[13px] text-muted-foreground" data-first-run-usage="">
          <p>
            {fr.usage.notice}{" "}
            <button type="button" className="underline hover:text-foreground" onClick={fr.openUsageLink}>
              {fr.usage.linkLabel}
            </button>
          </p>
          <label className="flex items-center gap-2.5 text-foreground">
            <Switch
              checked={fr.usage.on}
              disabled={!fr.usage.enabled}
              onCheckedChange={(on) => void fr.setUsage(on).catch(() => {})}
              aria-label={fr.usage.label}
            />
            <span>{fr.usage.label}</span>
            {fr.usage.help ? <span className="text-muted-foreground">{fr.usage.help}</span> : null}
          </label>
        </div>
      ) : null}
      {keys.sheet ? <KeySheet key={keys.sheet.provider} keys={keys} provider={keys.sheet.provider} env={keys.sheet.env} /> : null}
    </div>
  );
}

/** One agent: the box toggles, and so does a click anywhere on the row. */
function AgentPickRow({ pick: p, disabled, onChange }: { pick: FirstRunAgentPick; disabled: boolean; onChange: (on: boolean) => void }) {
  const off = p.locked || disabled;
  return (
    <div
      className={cn("flex min-h-9 items-center gap-3 rounded-lg px-3 py-1.5", !off && "cursor-default hover:bg-foreground/[0.03]")}
      data-first-run-pick={p.agent}
      data-checked={String(p.checked)}
      onClick={(e) => {
        if (!off && !(e.target as HTMLElement).closest('[data-slot="checkbox"]')) onChange(!p.checked);
      }}
    >
      <Checkbox checked={p.checked} disabled={off} aria-label={p.name} onCheckedChange={(on) => onChange(Boolean(on))} />
      <AgentMark agent={p.agent} name={p.name} />
      <span className="min-w-0 flex-1 text-[14px] select-none">{p.name}</span>
      {p.note ? <span className="shrink-0 text-xs text-muted-foreground">{p.note}</span> : null}
    </div>
  );
}

function CredentialRowView({ row: r, onAct }: { row: FirstRunCredential; onAct: () => void }) {
  return (
    <div className="flex items-center gap-3 px-4 py-2.5" data-first-run-credential={r.id}>
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
        <span className={cn("shrink-0 text-xs", r.ok ? "rounded-full bg-[#1f9d55]/10 px-2 py-0.5 font-medium text-[#1f9d55]" : "text-muted-foreground")} data-first-run-credential-status="">
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

/* ---- Step 2: your agent can use this Mac ------------------------------------------- */

function Setup({ fr, view, onFinish }: { fr: FirstRunHook; view: FirstRunView; onFinish: () => void }) {
  const s = view.setup!;
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await fr.copyPrompt();
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div className="my-auto flex max-w-[600px] flex-col py-6" data-first-run="setup">
      <h1 tabIndex={-1} data-onboarding-heading="" className="font-display text-[clamp(32px,4.4vw,48px)] leading-[1.05] font-semibold tracking-[-0.03em] outline-none" role="status">
        {s.title}
      </h1>
      <Card className="mt-7 divide-y p-0">
        {s.rows.map((r) => (
          <Row key={r.id} row={r} onAct={() => void fr.act(r.action!.id, r.settingsUrl)} />
        ))}
      </Card>

      {s.prompt ? (
        <>
          <h2 className="mt-7 text-[14px] font-semibold">{s.prompt.heading}</h2>
          {s.prompt.copy ? (
            <Card className="mt-2 flex items-center justify-between gap-3 py-2.5 pr-2.5">
              <span className="min-w-0 text-[14px] select-text">“{s.prompt.text}”</span>
              <Button variant="outline" size="sm" className="shrink-0 gap-1.5 rounded-full" onClick={() => void copy()} data-first-run-copy="">
                {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
                {copied ? "Copied" : s.prompt.copy}
              </Button>
            </Card>
          ) : (
            <p className="mt-1.5 text-[14px] select-text">“{s.prompt.text}”</p>
          )}
          <p className="mt-2 text-[13px] text-muted-foreground">{s.prompt.note}</p>
        </>
      ) : null}

      <ExtraCard extra={s.extra} fr={fr} />
      {s.upsell ? <UpsellCard card={s.upsell} fr={fr} /> : null}

      <div className="mt-8 flex items-center gap-4">
        <Button
          size="lg"
          variant={s.primary.id === "done" ? "default" : "outline"}
          className="h-11 min-w-28 rounded-full px-6 text-[15px]"
          disabled={!s.primary.enabled}
          data-onboarding-primary=""
          data-first-run-primary={s.primary.id}
          onClick={onFinish}
        >
          {s.primary.label}
        </Button>
        {s.primary.note ? <span className="text-[13px] text-muted-foreground">{s.primary.note}</span> : null}
      </div>
      {view.setup && !fr.saved.usedAtMs ? (
        <button type="button" onClick={fr.back} className="mt-5 self-start text-xs text-muted-foreground hover:text-foreground">
          Change agents
        </button>
      ) : null}
    </div>
  );
}

function Card({ children, className, ...rest }: { children: ReactNode; className?: string } & Record<`data-${string}`, string | undefined>) {
  return (
    <div className={cn("rounded-2xl border bg-card px-5 py-4 shadow-xs", className)} {...rest}>
      {children}
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

function Row({ row, onAct }: { row: FirstRunRow; onAct: () => void }) {
  return (
    <div className="flex items-center gap-3.5 px-5 py-3.5" data-first-run-row={row.id} data-state={row.state}>
      <StateIcon state={row.state} />
      <div className="min-w-0 flex-1">
        <div className="text-[14px] font-medium">{row.title}</div>
        <div className="text-[13px] text-muted-foreground">{row.note}</div>
      </div>
      {row.action ? (
        <Button size="sm" variant={row.action.id === "restart-check" ? "outline" : "default"} className="shrink-0 rounded-full" onClick={onAct} data-first-run-action={row.action.id}>
          {row.action.label}
        </Button>
      ) : null}
    </div>
  );
}

function ExtraCard({ extra, fr }: { extra: FirstRunExtraView; fr: FirstRunHook }) {
  const act = (id: string) => void fr.extra(id);
  if (extra.phase === "choose") {
    return (
      <Card className="mt-6 border-dashed bg-card/60" data-first-run-extra="choose">
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
      </Card>
    );
  }
  const icon = extra.phase === "ready" ? "ok" : extra.phase === "failed" ? "fix" : extra.phase === "not-now" ? "later" : "checking";
  return (
    <Card className="mt-6" data-first-run-extra={extra.phase}>
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
    </Card>
  );
}

function UpsellCard({ card, fr }: { card: FirstRunUpsell; fr: FirstRunHook }) {
  useEffect(() => {
    // Counted once per showing.
    fr.upsell("shown");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [card.plan]);
  return (
    <Card className="mt-4" data-first-run-upsell={card.plan}>
      <div className="flex items-start gap-3">
        <CuaKey size={20} className="mt-0.5" />
        <div className="min-w-0 flex-1">
          <h2 className="text-[15px] font-semibold">{card.title}</h2>
          {card.lines.map((l) => (
            <p key={l} className="text-[13px] text-muted-foreground">
              {l}
            </p>
          ))}
          <div className="mt-3 flex items-center gap-2">
            <span className="flex-1 text-[14px] font-medium">{card.price}</span>
            <Button size="sm" className="rounded-full" onClick={() => fr.upsell("click")} data-first-run-upsell-cta="">
              {card.cta.label}
            </Button>
            <Button variant="outline" size="sm" className="rounded-full" onClick={() => fr.upsell("dismiss")} data-first-run-upsell-dismiss="">
              {card.dismiss}
            </Button>
          </div>
        </div>
      </div>
    </Card>
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
