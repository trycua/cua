// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run (CUA-3318), phase 1 of the approved storyboard
 * (CUA-3193): drawn on the spaces.cua.ai hero (its starfield, keys, Outfit title, white button and
 * the Cua lockup). The words and choices are `lib/first-run-new.ts`; this file reads the app's state
 * (sign-in, adding this computer to the account, the account's ready computers and this computer's
 * agents) and draws the screens.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronDownIcon, CopyIcon, LoaderCircleIcon } from "lucide-react";
import { useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";

import { folderLabel, isNeedsProof, useMachines, useSession, useSettings, type SignInPhase } from "@/bridge";
import { BridgeContext } from "@/bridge/BridgeProvider";
import { useAppPermissions } from "@/bridge/app-permissions";
import { useLocalNetwork, type LocalNetworkState } from "@/bridge/local-network";
import { readFirstRun, useFirstRunSaved, writeFirstRun } from "@/bridge/first-run";
import { hostOs } from "@/bridge/host-os";
import { useFirstSpaceOffers } from "@/bridge/new-space-hooks";
import { onboardingInitial, onboardingTelemetry, onboardingView, readSavedOnboarding, writeSavedOnboarding } from "@/bridge/onboarding";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { starsWord } from "./stars-word";
import { AppPermissionList, LocalNetworkRow, type PermissionsView } from "@/components/app-permissions";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { agoLabel, PICK_UP_LIMIT, recentThreads, useRecentThreads } from "@/components/ask-first/pick-up";
import { CuaLogo } from "@/components/cua-logo";
import { Switch } from "@/components/ui/switch";
import { permissionsStepView, type PermissionsStepView } from "@/lib/app-permissions";
import { addImportedTask, type AskTask } from "@/lib/ask-first";
import {
  addingView,
  agentInfo,
  agentView,
  chosenAgents,
  toggleAgentOff,
  dotStep,
  furthestStep,
  INSTALL_PAGES,
  listNames,
  NEW_RUN_EMPTY,
  nextStepScreen,
  placeName,
  readyComputers,
  readyView,
  signInView,
  stepDots,
  stepOf,
  WELCOME,
  welcomeUsage,
  type AgentView,
  type NewRunSaved,
  type ReadyView,
  type RunFacts,
  type SignInView,
  type StepDot,
  type WelcomeUsage,
} from "@/lib/first-run-new";
import { cn } from "@/lib/utils";

import { FloatKeys } from "./float-keys";
import { CARD, GLASS_CHIP, GREY } from "./hero";
import { thisComputerText } from "@/lib/host-labels";

/* ---- The site's pieces ------------------------------------------------------------------ */

/** The site's white button (the dark hero's Download pill). */
const WHITE_BUTTON =
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  "inline-flex h-12 items-center justify-center rounded-full bg-white px-7 text-[15px] font-semibold text-[#0a152d] outline-none transition-transform hover:scale-[1.04] active:scale-[0.98] focus-visible:ring-2 focus-visible:ring-[#5ec0e8] disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none";
/** The site's Sign up glass at the white button's height. */
const GLASS_BUTTON =
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  "inline-flex h-12 items-center justify-center rounded-full bg-white/[0.08] px-6 text-[15px] font-semibold text-white shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)] backdrop-blur-md outline-none hover:bg-white/[0.14] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]";
/** The site's underlined text link, in white. */
const LINK =
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  "font-semibold text-white underline decoration-white/50 underline-offset-[3px] outline-none hover:decoration-white focus-visible:ring-2 focus-visible:ring-[#5ec0e8] rounded-sm";

function Title({ children }: { children: ReactNode }) {
  return (
    <h1
      tabIndex={-1}
      data-onboarding-heading=""
      className="font-display text-[clamp(34px,min(4.9vw,6.6vh),76px)] leading-[1.04] font-semibold tracking-[-0.03em] text-balance text-white outline-none"
    >
      {children}
    </h1>
  );
}

// The headline as on cua.ai: its last word in the keys' gradient (Cua Blue, the lifted navy, terracotta),
// arriving once as points of light that settle into it (stars-word.ts). Reduced motion: simply there.
const SUPER_INK_STYLE = {
  backgroundImage: "linear-gradient(90deg, #5ec0e8 0%, #6a8bb0 50%, #b5694b 100%)",
  WebkitBackgroundClip: "text",
  backgroundClip: "text",
  color: "transparent",
  WebkitTextFillColor: "transparent",
} as const;

function SuperTitle({ text }: { text: string }) {
  const word = useRef<HTMLSpanElement>(null);
  const cut = text.lastIndexOf(" ");
  useEffect(() => {
    const el = word.current;
    const host = el?.closest<HTMLElement>("[data-new-first-run]");
    if (!el || !host || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    return starsWord(el, host);
  }, []);
  return (
    <>
      {text.slice(0, cut + 1)}
      <span ref={word} className="inline-block">
        <span style={SUPER_INK_STYLE}>{text.slice(cut + 1)}</span>
      </span>
    </>
  );
}

function Line({ children }: { children: ReactNode }) {
  return <p className={cn("mt-[min(20px,2.2vh)] max-w-[430px] text-[16px] leading-[1.55] font-medium", GREY)}>{children}</p>;
}

function Check({ on }: { on: boolean }) {
  return on ? (
    <span className="flex size-[22px] shrink-0 items-center justify-center rounded-full bg-[#5ec0e8] text-[#050607]" data-check="on">
      <CheckIcon className="size-[13px]" strokeWidth={3} />
    </span>
  ) : (
    <span className="size-[22px] shrink-0 rounded-full shadow-[inset_0_0_0_1.5px_rgb(255_255_255/0.35)]" data-check="off" />
  );
}

/** A row in the card: picked rows are lit; `disabled` rows are greyed, with no check and no hover. */
function Row({
  icon,
  name,
  sub,
  picked,
  disabled,
  onClick,
  extra,
  check = true,
  ...rest
}: { icon: ReactNode; name: string; sub: ReactNode; picked?: boolean; disabled?: boolean; onClick?: () => void; extra?: ReactNode; check?: boolean } & Record<
  `data-${string}`,
  string | undefined
>) {
  const body = (
    <>
      {icon}
      <span className="min-w-0 flex-1 text-left">
        <span className={cn("block text-[15px] leading-5 font-medium transition-colors", check && !picked && onClick ? "text-white/60" : "text-white")}>
          {name}
        </span>
        <span className={cn("flex items-center gap-1.5 text-[13.5px] leading-[19px] font-medium", GREY)}>{sub}</span>
      </span>
      {extra}
      {check && !disabled ? <Check on={Boolean(picked)} /> : null}
    </>
  );
  // Picked is said by the check alone (Cua Blue) and the name at full white; no box around the row.
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  const cls = cn("flex w-full items-center gap-3 rounded-[12px] px-3 py-[9px]", disabled && "pointer-events-none cursor-not-allowed opacity-40");
  if (!onClick || disabled)
    return (
      <div className={cls} aria-disabled={disabled || undefined} {...rest}>
        {body}
      </div>
    );
  return (
    <div
      role="checkbox"
      aria-checked={Boolean(picked)}
      tabIndex={0}
      className={cn(cls, "cursor-pointer outline-none hover:bg-white/[0.06] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]")}
      onClick={onClick}
      onKeyDown={(e) => (e.key === " " || e.key === "Enter") && (e.preventDefault(), onClick())}
      {...rest}
    >
      {body}
    </div>
  );
}

/** How far above the footer the content fades out. */
const FADE_PX = 28;

/**
 * A screen's action row. It sits at the bottom of the content area on every screen and every window size, so the
 * primary button never moves between steps: short content leaves space above it (`mt-auto` in the page's full-height
 * column), long content scrolls under it (`sticky`). It has no background of its own, so the stars and keys show
 * through; the content fades out just above it instead (`useFadeUnder`).
 */
function Actions({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cn("sticky bottom-0 z-10 mt-auto pt-[min(20px,2.2vh)] pb-3", className)} data-new-run-footer="">
      {children}
    </div>
  );
}

/**
 * Fades the screen's content out just above its action row (`Actions`) while the window is too short for both, so
 * nothing runs under the buttons. A mask on each block above the row, never a background on the row: nothing covers
 * the stars or keys. One for the whole page (`scroller` is its `main`), so every screen gets it; `screen` re-reads
 * the row when the screen changes.
 */
function useFadeUnder(scroller: RefObject<HTMLElement | null>, screen: string) {
  useLayoutEffect(() => {
    const main = scroller.current;
    if (!main) return;
    let frame = 0;
    const masked = new Set<HTMLElement>();
    const clear = (el: HTMLElement) => {
      el.style.removeProperty("mask-image");
      el.style.removeProperty("-webkit-mask-image");
      delete el.dataset.faded;
      masked.delete(el);
    };
    const update = () => {
      frame = 0;
      const footer = main.querySelector<HTMLElement>("[data-new-run-footer]");
      const above = footer
        ? [...(footer.parentElement?.children ?? [])].filter(
            (el): el is HTMLElement =>
              el instanceof HTMLElement && el !== footer && Boolean(el.compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING),
          )
        : [];
      for (const el of [...masked]) if (!above.includes(el)) clear(el);
      if (!footer) return;
      const top = footer.getBoundingClientRect().top;
      for (const el of above) {
        const box = el.getBoundingClientRect();
        const cut = top - box.top;
        if (!(box.height > 0 && cut < box.height - 1)) {
          if (masked.has(el)) clear(el);
          continue;
        }
        const mask = `linear-gradient(to bottom, #000 ${Math.max(0, cut - FADE_PX)}px, transparent ${Math.max(0, cut)}px)`;
        el.style.setProperty("mask-image", mask);
        el.style.setProperty("-webkit-mask-image", mask);
        el.dataset.faded = "";
        masked.add(el);
      }
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    update();
    main.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    ro?.observe(main);
    if (main.firstElementChild) ro?.observe(main.firstElementChild);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      main.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      ro?.disconnect();
      for (const el of [...masked]) clear(el);
    };
  }, [scroller, screen]);
}

/** How far the content may run under the action row before "More below" shows, in px. */
const MORE_SLACK = 8;

/**
 * Whether the screen's content runs on below the window (a short window: a 13" MacBook with the Dock, a small
 * external display). The fade under the action row alone hid that there was more, so a row could be missed; the page
 * then shows "More below", which scrolls to it.
 */
export function useMoreBelow(scroller: RefObject<HTMLElement | null>, screen: string): boolean {
  const [more, setMore] = useState(false);
  useLayoutEffect(() => {
    const main = scroller.current;
    if (!main) return;
    const update = () => setMore(main.scrollHeight - main.scrollTop - main.clientHeight > MORE_SLACK);
    update();
    main.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    ro?.observe(main);
    if (main.firstElementChild) ro?.observe(main.firstElementChild);
    return () => {
      main.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      ro?.disconnect();
    };
  }, [scroller, screen]);
  return more;
}

/**
 * The step dots under the page. Each is a button: it opens an earlier step, or a later one already reached
 * (never one past the sign-in). The left and right arrow keys move between the steps that can be opened.
 */
export function StepDots({ dots, onGo }: { dots: StepDot[]; onGo: (i: number, viaKey: boolean) => void }) {
  return (
    <nav aria-label="Setup steps" className="relative z-20 shrink-0 pb-3.5 pl-[60px]" data-new-run-dots="">
      <ol className="flex">
        {dots.map((d, i) => (
          <li key={i}>
            <button
              type="button"
              aria-label={d.label}
              aria-current={d.current ? "step" : undefined}
              disabled={!d.to && !d.current}
              // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
              className="group flex h-6 items-center rounded-full px-1 outline-none focus-visible:ring-2 focus-visible:ring-[#5ec0e8] enabled:cursor-pointer disabled:cursor-default"
              onClick={() => d.to && onGo(i, false)}
              onKeyDown={(e) => {
                if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
                e.preventDefault();
                const to = dotStep(dots, i, e.key === "ArrowLeft" ? -1 : 1);
                if (to !== null) onGo(to, true);
              }}
              data-step-dot={i}
            >
              <span
                className={cn(
                  "block h-1 w-7 rounded-full transition-colors motion-reduce:transition-none",
                  d.current ? "bg-white" : d.to ? "bg-white/30 group-hover:bg-white/70" : "bg-white/[0.14]",
                )}
              />
            </button>
          </li>
        ))}
      </ol>
    </nav>
  );
}

/* ---- The screens (drawn from their views, so tests draw every state) --------------------- */

export function WelcomeScreen({
  onAdd,
  usage,
  onUsage,
  onUsageLink,
}: {
  onAdd: () => void;
  usage?: WelcomeUsage | null;
  onUsage?: (on: boolean) => void;
  onUsageLink?: () => void;
}) {
  return (
    <div data-new-run="welcome">
      <Title>
        <SuperTitle text={WELCOME.title} />
      </Title>
      <Line>{WELCOME.line}</Line>
      {usage ? (
        // Before anything is sent: the hosts hold every usage event until the run leaves this screen.
        <div
          className={cn("mt-6 flex max-w-[520px] items-start gap-3 text-[13px] leading-[19px] font-medium", GREY)}
          data-new-run-usage={usage.on ? "on" : "off"}
        >
          {/* The switch sits in a box one text line tall, so it centres on the label line whatever the font metrics. */}
          <span className="flex h-[19px] shrink-0 items-center">
            <Switch
              checked={usage.on}
              disabled={!usage.enabled}
              onCheckedChange={(on) => onUsage?.(on)}
              aria-label={usage.label}
              data-new-run-usage-switch=""
            />
          </span>
          <span className="min-w-0">
            <span className="block text-white">{usage.label}</span>
            <span className="block" data-new-run-usage-notice="">
              {usage.notice}{" "}
              <button type="button" className="underline underline-offset-2 hover:text-white" onClick={onUsageLink} data-new-run-usage-link="">
                {usage.linkLabel}
              </button>
            </span>
            {usage.help ? <span className="block">{usage.help}</span> : null}
          </span>
        </div>
      ) : null}
      <Actions>
        <button type="button" className={WHITE_BUTTON} onClick={onAdd} data-onboarding-primary="">
          {WELCOME.primary}
        </button>
      </Actions>
    </div>
  );
}

export function SignInScreen({
  view,
  onTryAgain,
  onUseCode,
  onOpen,
}: {
  view: SignInView;
  onTryAgain: () => void;
  onUseCode: () => void;
  onOpen: (url: string) => void;
}) {
  // The code line waits 12 s: most sign-ins finish in the browser before then.
  const [codeHint, setCodeHint] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setCodeHint(true), 12_000);
    return () => clearTimeout(t);
  }, []);
  return (
    <div data-new-run="signin" data-state={view.state}>
      <Title>{view.title}</Title>
      <Line>{view.line}</Line>
      {view.code ? (
        <div className="mt-6 flex flex-col gap-2" data-new-run-code="">
          <span className="font-mono text-[28px] font-semibold tracking-[0.12em] text-white select-text">{view.code.code}</span>
          {view.code.url ? (
            <button type="button" className={cn(LINK, "self-start text-[14px]")} onClick={() => onOpen(view.code!.url!)}>
              Open the sign-in page
            </button>
          ) : null}
        </div>
      ) : null}
      {view.waiting ? (
        <div className="mt-6 flex items-center gap-3 text-[15px] font-medium text-white" role="status" data-new-run-waiting="">
          <LoaderCircleIcon aria-hidden className="size-4 animate-spin motion-reduce:animate-none" />
          {view.waiting}
        </div>
      ) : null}
      {/* While the browser is still the way in, the code is only for a browser on another device: a quiet
          line that shows after a while, not a second button. Once sign-in failed it is a button again. */}
      {view.useCode && view.state === "waiting" && codeHint ? (
        <p className={cn("mt-4 text-[14px]", GREY)} data-new-run-code-hint="">
          Browser on another device?{" "}
          <button type="button" className="underline underline-offset-2 hover:text-white" onClick={onUseCode} data-new-run-use-code="">
            Use a code instead
          </button>
        </p>
      ) : null}
      <Actions className="flex flex-wrap items-center gap-3">
        {view.tryAgain ? (
          <button type="button" className={view.tryAgainPrimary ? WHITE_BUTTON : GLASS_BUTTON} onClick={onTryAgain} data-new-run-try-again="">
            {view.tryAgain}
          </button>
        ) : null}
        {view.useCode && (view.state === "failed" || view.state === "add-failed") ? (
          <button type="button" className={GLASS_BUTTON} onClick={onUseCode} data-new-run-use-code="">
            {view.useCode}
          </button>
        ) : null}
      </Actions>
    </div>
  );
}

export function AgentScreen({
  view,
  onPick,
  onContinue,
  onLater,
  onInstall,
  busy,
}: {
  view: AgentView;
  onPick: (id: string) => void;
  onContinue: () => void;
  onLater: () => void;
  onInstall: (agent: "claude-code" | "codex") => void;
  busy: boolean;
}) {
  const later = view.later ? (
    <button type="button" className={cn(LINK, "text-[15px]")} disabled={busy} onClick={onLater} data-new-run-agent-later="">
      {view.later}
    </button>
  ) : null;
  return (
    <div data-new-run="agent" data-state={view.state}>
      <Title>{view.title}</Title>
      {view.line ? <Line>{view.line}</Line> : null}
      {view.agents.length ? (
        <div className={cn(CARD, "mt-[min(28px,3vh)]")} data-new-run-agents="">
          {view.agents.map((a) => (
            <Row
              key={a.id}
              icon={<span className="flex size-7 shrink-0 items-center justify-center">{<AppMarkImage agent={a.id} className="size-5" />}</span>}
              name={a.name}
              sub={<span data-new-run-agent-where="">{a.where}</span>}
              picked={a.picked}
              onClick={() => onPick(a.id)}
              data-new-run-agent={a.id}
            />
          ))}
        </div>
      ) : null}
      {view.note ? (
        <p className={cn("mt-3 max-w-[480px] text-[13.5px] font-medium", GREY)} data-new-run-agent-note="">
          {view.note}
        </p>
      ) : null}
      {view.error ? (
        <p role="alert" className="mt-3 max-w-[480px] text-[13.5px] font-medium text-white" data-new-run-agent-error="">
          {view.error}
        </p>
      ) : null}
      {view.install ? (
        <>
          <p className={cn("mt-[min(28px,3vh)] max-w-[480px] text-[13.5px] font-medium", GREY)}>{view.install.note}</p>
          <Actions className="flex flex-wrap items-center gap-6">
            <button type="button" className={WHITE_BUTTON} onClick={() => onInstall("claude-code")} data-new-run-install="claude-code">
              {view.install.primary}
            </button>
            <button type="button" className={cn(LINK, "text-[15px]")} onClick={() => onInstall("codex")} data-new-run-install="codex">
              {view.install.secondary}
            </button>
            {later}
          </Actions>
        </>
      ) : (
        // Shown (greyed) while the agents are still being looked for, so it doesn't pop in once they're found.
        <Actions className="flex flex-wrap items-center gap-6">
          <button type="button" className={WHITE_BUTTON} disabled={!view.continueEnabled || busy} onClick={onContinue} data-onboarding-primary="">
            {busy ? "Setting up…" : "Continue"}
          </button>
          {later}
        </Actions>
      )}
    </div>
  );
}

/**
 * "Pick up where you left off" (CUA-3319): 2 to 4 of the person's recent chats with their agents on
 * this computer, each with **Continue in Cua**, which opens it in a new Home task with what was said
 * so far. Nothing runs until they send. Only the chats of `agents` (the agent picked in setup) are
 * listed; all three agents' when none are named. While they are read, two quiet grey rows hold the
 * place; hidden when there are none. Only each chat's title, folder and time are read here; its
 * messages are read when it is picked.
 */
export function PickUpWhereYouLeftOff({
  onContinue,
  agents,
  place = thisComputerText(),
  className,
}: {
  onContinue: (task: AskTask) => void;
  agents?: readonly string[];
  place?: string;
  className?: string;
}) {
  const threads = useRecentThreads(PICK_UP_LIMIT, agents);
  if (threads && !threads.length) return null;
  return (
    <section
      aria-labelledby="pick-up-heading"
      aria-busy={threads ? undefined : true}
      className={cn("w-full", className)}
      data-pick-up=""
      data-pick-up-loading={threads ? undefined : ""}
    >
      <h2 id="pick-up-heading" className="text-[15px] font-semibold text-white">
        Pick up where you left off
      </h2>
      <ul className={cn(CARD, "mt-2.5 max-w-[600px]")}>
        {threads === null
          ? [0, 1].map((i) => (
              <li key={i} aria-hidden className="flex items-center gap-3 px-3 py-2.5" data-pick-up-placeholder="">
                <span className="size-7 shrink-0 rounded-full bg-white/[0.06]" />
                <span className="min-w-0 flex-1 space-y-1.5 py-0.5">
                  <span className="block h-3.5 w-[55%] rounded-full bg-white/[0.08]" />
                  <span className="block h-3 w-[35%] rounded-full bg-white/[0.05]" />
                </span>
              </li>
            ))
          : null}
        {(threads ?? []).map((t) => {
          const folder = folderLabel(t.cwd);
          const when = agoLabel(t.lastActiveMs);
          return (
            <li key={`${t.agent}:${t.id}`} className="flex items-center gap-3 px-3 py-2.5" data-pick-up-row={t.agent}>
              <span className="flex size-7 shrink-0 items-center justify-center rounded-full shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)]">
                <AppMarkImage agent={t.agent} className="size-3.5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[15px] leading-5 font-medium text-white">{t.title}</span>
                <span className={cn("block truncate text-[13.5px] leading-[19px] font-medium", GREY)}>{folder ? `${folder}, ${when}` : when}</span>
              </span>
              <button
                type="button"
                className={cn(GLASS_CHIP, "h-[30px] shrink-0 px-3 text-[13.5px]")}
                onClick={() => onContinue(addImportedTask(t))}
                data-pick-up-continue=""
              >
                Continue in Cua
              </button>
            </li>
          );
        })}
      </ul>
      <p className={cn("mt-2 text-[13.5px] font-medium", GREY)}>Your chats stay on {place} until you pick one.</p>
    </section>
  );
}

export function ReadyScreen({
  view,
  agent,
  agents,
  onDone,
  onContinueChat,
  onCopied,
  place,
}: {
  view: ReadyView;
  agent: string;
  agents?: readonly string[];
  onDone: () => void;
  onContinueChat?: (task: AskTask) => void;
  onCopied?: () => void;
  place?: string;
}) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    onCopied?.();
    void navigator.clipboard?.writeText(view.command).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div data-new-run="ready">
      <Title>{view.title}</Title>
      <Line>{view.line}</Line>
      <div
        // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
        className="mt-7 flex w-full max-w-[600px] items-center gap-3 rounded-[20px] bg-white/[0.08] py-2.5 pr-2.5 pl-4 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)] backdrop-blur-md"
        data-new-run-command=""
      >
        <AppMarkImage agent={agent} className="size-4 shrink-0" />
        <code className="min-w-0 flex-1 truncate font-mono text-[14.5px] text-white select-text">
          <span className={GREY}>$ </span>
          {view.command}
        </code>
        <button type="button" className={cn(GLASS_CHIP, "h-[30px] shrink-0 gap-1.5 px-3 text-[13.5px]")} onClick={copy} data-new-run-copy="">
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
          {copied ? "Copied" : view.copy}
        </button>
      </div>
      {/* The person's own recent chats with the agent they picked (CUA-3319), hidden when there are none. */}
      {onContinueChat ? (
        <PickUpWhereYouLeftOff className="mt-[min(28px,3vh)]" agents={agents?.length ? agents : [agent]} place={place} onContinue={onContinueChat} />
      ) : null}
      <Actions>
        <button type="button" className={WHITE_BUTTON} onClick={onDone} data-onboarding-primary="">
          {view.done}
        </button>
      </Actions>
    </div>
  );
}

/**
 * 2b, on a Mac: Cua Spaces' two permissions (the same rows as the Cua Driver page), each with Allow, and Later. A Mac
 * that runs macOS Spaces also gets Local Network (`net`), asked here so the first macOS Space just starts. Never blocks
 * the run.
 */
export function PermissionsScreen({
  view,
  perms,
  net,
  appName,
  onContinue,
}: {
  view: PermissionsStepView;
  perms: PermissionsView;
  net?: LocalNetworkState | null;
  appName?: string;
  onContinue: () => void;
}) {
  return (
    <div data-new-run="permissions">
      <Title>{view.title}</Title>
      <Line>{view.line}</Line>
      <AppPermissionList
        perms={perms}
        variant="hero"
        className="mt-[min(28px,3vh)]"
        footer={view.note}
        extra={net?.available ? <LocalNetworkRow net={net} appName={appName ?? "Cua Spaces"} /> : null}
      />
      <Actions className="flex items-center gap-x-6">
        <button
          type="button"
          className={cn("shrink-0", view.primary.white ? WHITE_BUTTON : GLASS_BUTTON)}
          onClick={onContinue}
          data-onboarding-primary=""
          data-new-run-permissions-next={view.primary.label}
        >
          {view.primary.label}
        </button>
        {view.stillOff && view.missing[0] ? (
          // What is still off, by name: on a short window its row can be below the fold, so pressing this shows it.
          <button
            type="button"
            className={cn(LINK, "line-clamp-2 min-w-0 text-left text-[14px] leading-tight font-medium text-white/85")}
            onClick={() => showRow(view.missing[0]!.id)}
            data-new-run-still-off=""
          >
            {view.stillOff}
          </button>
        ) : null}
      </Actions>
    </div>
  );
}

/** Scrolls a permission row into view and puts the focus on its Allow (or the row itself once there is none). */
function showRow(id: string): void {
  const row = document.querySelector<HTMLElement>(`[data-app-permission="${id}"]`);
  if (!row) return;
  const reduce = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  row.scrollIntoView?.({ block: "center", behavior: reduce ? "auto" : "smooth" });
  const target = row.querySelector<HTMLElement>("button") ?? row;
  if (target === row && !row.hasAttribute("tabindex")) row.tabIndex = -1;
  target.focus({ preventScroll: true });
}

/* ---- The page ------------------------------------------------------------------------- */

/** The page's side padding (`px-16`) and the copy column's widest (`max-w-[600px]`), in px. */
const PAGE_PAD = 64;
const COPY_MAX = 600;
/**
 * Where the copy column ends, from the left. The keys show only on windows 860px and wider, where the column is
 * always at its widest, so the keys never cover the text, the buttons or the links (the action rows run to the
 * column's full width: "Install Claude Code", "Install Codex instead", "Install later").
 */
export const COPY_RIGHT = PAGE_PAD + COPY_MAX;

/**
 * The usage data's name for each screen (`schema::FIRST_RUN_STEPS`), so the onboarding funnel keeps its steps:
 * Ready ends the run and is counted by its finish, not as a step.
 */
const FUNNEL_STEP: Partial<Record<NewRunScreenName, string>> = {
  welcome: "welcome",
  signin: "sign_in",
  permissions: "permissions",
  agent: "connect_agent",
  ready: "ready",
};

/** The picked agent as a usage-event word (`claude-code` is sent as `claude_code`); the core drops any it doesn't know. */
const agentWord = (agent: string | null) => (agent ?? "none").replaceAll("-", "_");

/** Usage events never get in the way of the run. */
function track(send: () => void): void {
  try {
    send();
  } catch {
    // An older core that reads the state differently: no event.
  }
}

/** The new first run's whole page. */
export function NewFirstRun() {
  const navigate = useNavigate();
  const ctx = useContext(BridgeContext);
  const adapter = ctx?.adapter ?? null;
  const store = ctx?.store ?? null;
  const os = hostOs();
  const session = useSession();
  const machines = useMachines();
  const settings = useSettings();
  const offers = useFirstSpaceOffers();
  const savedAll = useFirstRunSaved();
  const s: NewRunSaved = { ...NEW_RUN_EMPTY, ...(savedAll.newRun ?? {}) };
  const save = (patch: Partial<NewRunSaved>) => {
    const prev = { ...NEW_RUN_EMPTY, ...(readFirstRun().newRun ?? {}) };
    const reachedStep = patch.screen ? furthestStep(furthestStep(prev.reachedStep, prev.screen), patch.screen) : prev.reachedStep;
    writeFirstRun({ newRun: { ...prev, ...patch, reachedStep } });
  };
  const go = (screen: NewRunScreenName) => save({ screen });

  // Which steps this run shows (`runSteps`): the step dots and every move to the next step read the same facts.
  // On a Mac, Cua Spaces' permissions (and Local Network, where macOS Spaces runs here) are read from the first step
  // on, so the dots know whether the permissions step asks anything; it is left out when every one is already on.
  // Once the run leaves the first step the answer is kept (`s.permissions`), so turning them on on that step doesn't
  // change the dots under it.
  const readPermissions = os === "macos" && (stepOf(s.screen) === "computer" || s.screen === "permissions");
  const perms = useAppPermissions(readPermissions);
  const net = useLocalNetwork(readPermissions);
  const permissionsNow: RunFacts["permissions"] = perms.loading
    ? null
    : !perms.available
      ? "skip"
      : !perms.allGranted
        ? "ask"
        : net.loading
          ? null
          : net.available && !net.granted
            ? "ask"
            : "skip";
  const facts: RunFacts = { os, permissions: stepOf(s.screen) === "computer" ? permissionsNow : (s.permissions ?? permissionsNow) };
  /** The first step is done (signed in and added) or skipped: on to the next step this run shows. */
  const leaveFirstStep = () => {
    const permissions = os === "macos" ? (permissionsNow ?? "ask") : undefined;
    const screen = nextStepScreen({ os, permissions: permissions ?? null }, "welcome") ?? "agent";
    save(permissions ? { screen, permissions } : { screen });
  };

  const signedIn = Boolean(session.data?.signedIn);
  const phase: SignInPhase = session.data?.signIn ?? { kind: "idle" };
  // Signed in, with no browser sign-in under way: only then may this computer join the account. A sign-in that
  // runs (the browser hasn't confirmed yet) is waited for, even when an older session still says signed in.
  const signingIn = phase.kind === "starting" || phase.kind === "waiting";
  const authed = signedIn && !signingIn;
  const [found, setFound] = useState<string[] | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  // This computer joining the account, once signed in: `idle` until the sign-in finishes.
  const [thisState, setThisState] = useState<"idle" | "adding" | "added" | "failed">("idle");
  const [addError, setAddError] = useState<string | null>(null);
  const [addNeedsProof, setAddNeedsProof] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  // "Use a code from another computer": the sheet opens on "Enter a code".
  const [addEnter, setAddEnter] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  useFadeUnder(mainRef, s.screen);
  const moreBelow = useMoreBelow(mainRef, s.screen);

  // The onboarding funnel (`cua_onboarding_*`): the run starts once, then each screen counts when it shows.
  const core = ctx?.core ?? null;
  const telemetry = settings.data?.telemetry ?? null;
  const funnelState = (c: NonNullable<typeof core>) => ({
    ...onboardingInitial(c, session.data?.identity ?? null),
    step: s.screen === "welcome" || s.screen === "signin" ? ("welcome" as const) : ("agents" as const),
    runCounted: true,
    telemetry: telemetry ? onboardingTelemetry(telemetry) : null,
  });
  // Screen 1's usage line, in the core's words (the same as Settings, Privacy).
  const usage =
    s.screen === "welcome" && core && telemetry
      ? welcomeUsage(onboardingView(core, { ...onboardingInitial(core, null), step: "welcome", telemetry: onboardingTelemetry(telemetry) }))
      : null;
  const runStarted = useRef(false);
  useEffect(() => {
    if (runStarted.current || !store || !core || !telemetry) return;
    runStarted.current = true;
    track(() => store.trackOnboarding({ ...onboardingInitial(core, null), telemetry: onboardingTelemetry(telemetry) }, { type: "start" }));
  }, [store, core, telemetry]);
  useEffect(() => {
    const step = FUNNEL_STEP[s.screen];
    // The Ready screen's events name the picked agent (a fixed word; the core drops anything else).
    if (store && step) track(() => store.trackFirstRunStep(step, "step_viewed", s.screen === "ready" ? agentWord(s.agent) : "none"));
  }, [store, s.screen]);
  const funnel = (action: string, detail = "none") => {
    const step = FUNNEL_STEP[s.screen];
    if (store && step) track(() => store.trackFirstRunStep(step, action, detail));
  };

  // A new screen: focus moves to its title, so a screen reader reads it. Moved with the arrow keys on the
  // step dots, focus stays on the dots instead.
  const dotFocus = useRef<number | null>(null);
  useEffect(() => {
    const keep = dotFocus.current;
    dotFocus.current = null;
    const el =
      keep !== null ? document.querySelector<HTMLElement>(`[data-step-dot="${keep}"]`) : document.querySelector<HTMLElement>("[data-onboarding-heading]");
    el?.focus();
  }, [s.screen]);

  // Sign-in: started when 1b opens, and over once the session says signed in.
  const started = useRef(false);
  useEffect(() => {
    if (s.screen !== "signin") {
      started.current = false;
      return;
    }
    if (signedIn) return;
    if (!started.current && phase.kind === "idle") {
      started.current = true;
      void session.signIn().catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, signedIn, phase.kind]);

  // Back from the browser: read the sign-in again at once, so one that finished
  // (or failed) there shows now, not on the next host push.
  const waitingOnSignIn = s.screen === "signin" && signingIn;
  const refreshSession = useRef(session.refresh);
  refreshSession.current = session.refresh;
  useEffect(() => {
    if (!waitingOnSignIn) return;
    const again = () => {
      if (document.visibilityState !== "hidden") void refreshSession.current().catch(() => {});
    };
    window.addEventListener("focus", again);
    document.addEventListener("visibilitychange", again);
    return () => {
      window.removeEventListener("focus", again);
      document.removeEventListener("visibilitychange", again);
    };
  }, [waitingOnSignIn]);

  // Signed in: this computer joins the account on the sign-in screen (the store also does it on sign-in), then setup
  // goes on. There is no step to pick where agents work: they can use every ready computer (Francesco, 8 Nov 2026).
  const addRun = useRef(0);
  const addThis = async () => {
    if (!store) return setThisState("added");
    const n = ++addRun.current;
    setThisState("adding");
    setAddError(null);
    setAddNeedsProof(false);
    const ok = await store.addThisComputer(session.data?.identity ?? null).catch(() => false);
    if (n !== addRun.current) return;
    setThisState(ok ? "added" : "failed");
    if (!ok) {
      const failure = store.thisMachine.get().setupError;
      setAddError(failure?.message ?? null);
      setAddNeedsProof(isNeedsProof(failure));
    }
  };
  useEffect(() => {
    if (s.screen !== "signin" || !authed) return;
    if (thisState === "idle") void addThis();
    else if (thisState === "added") leaveFirstStep();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, authed, thisState]);

  // A run saved on the removed "Pick where your agents work" step goes on from the step after the sign-in.
  useEffect(() => {
    if (s.screen === "where") leaveFirstStep();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen]);

  // The account's computers, read again for Ready, which names every ready one.
  useEffect(() => {
    if ((s.screen !== "agent" && s.screen !== "ready") || !signedIn) return;
    void machines.refresh().catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, signedIn]);

  // Screen 3: the coding agents on this computer (`agents.setup`: the app itself, never a file Cua wrote), read now and again.
  useEffect(() => {
    if (s.screen !== "agent" || !adapter) return;
    let live = true;
    const read = () =>
      void adapter
        .call("agents.setup", {})
        .then((rows) => live && setFound(rows.filter((r) => r.installed).map((r) => r.agent)))
        .catch(() => live && setFound((f) => f ?? []));
    read();
    const t = setInterval(read, 3_000);
    window.addEventListener("focus", read);
    return () => {
      live = false;
      clearInterval(t);
      window.removeEventListener("focus", read);
    };
  }, [s.screen, adapter]);

  // 2b: Cua Spaces' permissions (`perms`, read above). A host with none to give (an older app) has no step: it goes
  // on to the coding agent, and the dots drop the step. A Mac that runs macOS Spaces also needs Local Network (`net`):
  // asked on the same step, so the issue never shows up later on its page, and its first macOS Space just starts.
  const afterPermissions = nextStepScreen(facts, "permissions") ?? "agent";
  useEffect(() => {
    if (s.screen === "permissions" && !perms.loading && !perms.available) save({ screen: afterPermissions, permissions: "skip" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, perms.loading, perms.available]);

  // Ready's line to try asks for a separate Linux computer only where this one can make it (CUA-3150, idea D).
  // Making one lives in New Space and Home's picker, not in setup.
  const linux = offers.offers.find((o) => o.id === "linux") ?? null;
  const canMakeSeparate = !offers.known ? null : Boolean(linux && !linux.blocked);

  /** Ready's computers: this one, then every ready computer in the account (signed out: this one only). */
  const computers = signedIn ? readyComputers(machines.data ?? null) : ["this"];

  const agentsOff = s.agentsOff ?? [];
  const agentScreen = agentView({ os, found, off: agentsOff, error: connectError });
  const continueAgent = async () => {
    const agents = chosenAgents(found, agentsOff);
    if (!adapter || connecting) return;
    // Nothing ticked: the same as Choose later.
    if (!agents.length) return void agentsLater();
    setConnecting(true);
    setConnectError(null);
    // Ready's "Pick up where you left off": the picked agents' chats are read while Cua connects, so the list is
    // there when the screen shows (CUA-3319).
    void recentThreads(adapter, agents, PICK_UP_LIMIT);
    const started = performance.now();
    try {
      // Only Cua's tools in each agent's settings wait here: they are all the Ready screen needs, and take well under a
      // second. Copying an agent's sign-in can run Claude Code to refresh a sign-in that ran out (seconds, up to 30),
      // so it runs after, below.
      const results = await adapter.call("agents.connect", { agents, signIn: false }).catch(() => null);
      // A host without Connect, or an answer missing an agent: the agents' rows say.
      const missing = results ? agents.filter((id) => !results.some((r) => r.agent === id)) : agents;
      const rows = missing.length ? (results ? await adapter.call("agents.setup", {}) : await adapter.call("agents.configure", { agents })) : [];
      const failed = agents
        .map((id) => {
          const r = results?.find((x) => x.agent === id);
          if (r) return { agent: id, name: agentInfo(id).name, configured: Boolean(r.tools_ok), detail: r.tools_line ?? null };
          return rows.find((x) => x.agent === id) ?? { agent: id, name: agentInfo(id).name, configured: false, detail: null };
        })
        .filter((r) => !r.configured);
      if (failed.length) {
        const one = failed.length === 1 ? failed[0]! : null;
        setConnectError(
          one
            ? `Cua couldn't add itself to ${one.name ?? "the agent"}'s settings.${one.detail ? ` ${one.detail}` : ""}`
            : `Cua couldn't add itself to the settings of ${listNames(failed.map((r) => r.name ?? r.agent))}.`,
        );
        return;
      }
      console.info(`first run: ${agents.join(", ")} connected in ${Math.round(performance.now() - started)} ms`);
      // Ready's line to try uses the first one that has a command to type.
      const primary = agents.find((id) => agentInfo(id).command) ?? agents[0]!;
      save({ agent: primary, connected: agents, screen: nextStepScreen(facts, "agent") ?? "ready" });
      // The same connect as Settings, now with the sign-ins kept for runs in your Spaces. Nothing on Ready waits on it.
      void adapter.call("agents.connect", { agents, signIn: true }).then(
        () => console.info(`first run: sign-ins kept in ${Math.round(performance.now() - started)} ms`),
        (e: unknown) => console.warn("first run: keeping the sign-ins failed", e),
      );
    } catch (e) {
      setConnectError(e instanceof Error ? e.message : String(e));
    } finally {
      setConnecting(false);
    }
  };
  // Choose later: setup is done without connecting an agent; Settings, Connections does it any time.
  const agentsLater = () => {
    funnel("step_skipped", "later");
    void finish();
  };

  const open = (url: string) => void session.openExternal(url).catch(() => {});

  const leave = () => {
    funnel("step_skipped", "step");
    if (store && core) track(() => store.trackOnboardingSkipped(funnelState(core)));
    session.cancelSignIn();
    writeSavedOnboarding({ ...readSavedOnboarding(), skipped: true });
    writeFirstRun({ finishedLater: true });
    // Home is where the app opens (CUA-3551), so leaving setup early lands there too.
    void navigate({ to: "/home" });
  };
  const skip = () => {
    if (s.screen === "welcome" || s.screen === "signin") {
      funnel("step_skipped", "step");
      session.cancelSignIn();
      leaveFirstStep();
    } else leave();
  };
  const finished = async () => {
    if (store && core) track(() => store.trackOnboardingFinished(funnelState(core)));
    await session.completeOnboarding(thisState === "added" && signedIn ? "host" : "client").catch(() => {});
    // Setup ends on Home, ready for the first job; the rest is asked for there once that job is done (CUA-3150, C).
    const offer = readFirstRun().restOffer === "dismissed" ? ("dismissed" as const) : ("waiting" as const);
    writeFirstRun({ finishedLater: false, todosClosed: false, restOffer: offer, restSinceMs: Date.now() });
  };
  const finish = async () => {
    await finished();
    await navigate({ to: "/home" });
  };
  // Continue in Cua (CUA-3319): setup is done and the picked chat opens as a Home task. Nothing runs until the person sends.
  const continueChat = async (task: AskTask) => {
    await finished();
    await navigate({ to: "/home/$taskId", params: { taskId: task.id } });
  };
  // A run saved on the old last screen ("Now bring the rest in") finishes now: that screen lives on Home.
  const finishing = useRef(false);
  useEffect(() => {
    if (s.screen !== "next" || finishing.current) return;
    finishing.current = true;
    void finish();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen]);

  // Add this computer: the browser sign-in, waited for on 1b. Already signed in (Reset & Show, a session kept
  // from before): no second sign-in in the browser; 1b adds this computer with the session there is.
  const startSignIn = () => {
    started.current = true;
    go("signin");
    if (!signedIn) void session.signIn().catch(() => {});
  };

  const dots = stepDots(facts, s.screen, furthestStep(s.reachedStep, s.screen));
  const goDot = (i: number, viaKey: boolean) => {
    const to = dots?.[i]?.to;
    if (!to) return;
    if (s.screen === "signin") session.cancelSignIn();
    if (viaKey) dotFocus.current = i;
    go(to);
  };
  const showSkip = s.screen === "welcome" || s.screen === "signin" || s.screen === "agent";

  let body: ReactNode = null;
  switch (s.screen) {
    case "welcome":
      body = (
        <WelcomeScreen
          onAdd={startSignIn}
          usage={usage}
          onUsage={(on) => void settings.updateSetting("telemetry", on).catch(() => {})}
          onUsageLink={() => usage && open(usage.linkUrl)}
        />
      );
      break;
    case "signin":
      body = (
        <SignInScreen
          view={
            authed
              ? addingView({ os, state: thisState === "failed" ? "failed" : "adding", error: addError, needsProof: addNeedsProof })
              : signInView(phase, session.data?.signInUrl ?? null)
          }
          onTryAgain={() => {
            if (authed) return void addThis();
            session.cancelSignIn();
            void session.signIn().catch(() => {});
          }}
          onUseCode={() => {
            // Adding this computer wants a code from another computer: the sheet opens on "Enter a code".
            if (authed) {
              setAddEnter(true);
              setAddOpen(true);
              return;
            }
            session.cancelSignIn();
            void session.signIn({ code: true }).catch(() => {});
          }}
          onOpen={open}
        />
      );
      break;
    case "permissions":
      body = (
        <PermissionsScreen
          view={permissionsStepView({
            appName: perms.data?.appName ?? "Cua Spaces",
            allGranted: perms.allGranted,
            off: perms.data?.permissions.filter((p) => !p.granted).map((p) => p.id),
            localNetwork: !net.available ? "none" : net.granted ? "on" : net.denied ? "denied" : "off",
          })}
          perms={perms}
          net={net}
          appName={perms.data?.appName ?? "Cua Spaces"}
          onContinue={() => go(afterPermissions)}
        />
      );
      break;
    case "agent":
      body = (
        <AgentScreen
          view={agentScreen}
          busy={connecting}
          onPick={(id) => save({ agentsOff: toggleAgentOff(agentsOff, id) })}
          onContinue={() => void continueAgent()}
          onLater={agentsLater}
          onInstall={(a) => open(INSTALL_PAGES[a])}
        />
      );
      break;
    case "ready":
      body = s.agent ? (
        <ReadyScreen
          view={readyView({ os, agent: s.agent, agents: s.connected, computers, canMakeSeparate })}
          agent={s.agent}
          agents={s.connected}
          place={placeName(os)}
          onDone={() => {
            funnel("step_completed", agentWord(s.agent));
            void finish();
          }}
          onContinueChat={(task) => {
            funnel("step_chat_picked_up", agentWord(s.agent));
            void continueChat(task);
          }}
          onCopied={() => funnel("step_command_copied", agentWord(s.agent))}
        />
      ) : null;
      break;
  }

  return (
    <div
      className="dark relative flex h-full flex-col overflow-hidden bg-[#050607] text-white"
      data-window-surface="#050607"
      data-new-first-run=""
      data-step={s.screen}
    >
      <FloatKeys copyRight={COPY_RIGHT} />
      {/* A fixed height, with or without Skip for now, so the content under it never moves between steps. */}
      <header className="relative z-30 box-content flex h-9 shrink-0 items-center justify-between pt-[calc(var(--titlebar-height,0px)+12px)] pr-16 pb-2 pl-16">
        <span className="flex items-center gap-1.5" data-new-run-logo="">
          <CuaLogo className="size-5" />
          <span className="font-display text-[19px] font-medium tracking-[-0.01em] text-white">Cua</span>
        </span>
        {showSkip ? (
          <button type="button" className={cn(GLASS_CHIP, "h-9 px-4 text-[14px]")} onClick={skip} data-onboarding-later="">
            Skip for now
          </button>
        ) : null}
      </header>
      {/* Every screen starts at the same height under the header (never centred: screens differ in height, and a
          centred one moved the title and the button between steps). The screen fills the area, so its action row
          (`Actions`) sits at the bottom of it; content longer than the window scrolls above that row. On windows taller
          than 1000px the area stops at 860px, so the button stays near the text instead of the far bottom edge. */}
      <main ref={mainRef} className="relative z-20 flex min-h-0 flex-1 flex-col scroll-y-stable px-16" data-new-run-main="">
        {/* px-16 and max-w-[600px] are PAGE_PAD and COPY_MAX: change them together, they keep the keys off the copy. */}
        <div className="flex w-full max-w-[600px] shrink-0 grow flex-col pt-[clamp(16px,8vh,96px)] *:flex *:grow *:flex-col [@media(min-height:1000px)]:max-h-[860px]">
          {body}
        </div>
      </main>
      {/* The dots' row keeps its height on the screens without dots, so the area above it keeps its height too. */}
      <div className="relative z-20 flex h-[38px] shrink-0 flex-col justify-end" data-new-run-dots-row="">
        {dots ? <StepDots dots={dots} onGo={goDot} /> : null}
        {moreBelow ? (
          <div className="pointer-events-none absolute bottom-[3px] left-16 flex w-[min(600px,calc(100%-128px))] justify-end">
            <button
              type="button"
              className={cn(GLASS_CHIP, "pointer-events-auto h-8 gap-1 px-3 text-[13.5px]")}
              onClick={() => {
                const main = mainRef.current;
                if (!main) return;
                const reduce = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
                main.scrollBy({ top: Math.max(120, main.clientHeight * 0.6), behavior: reduce ? "auto" : "smooth" });
              }}
              data-new-run-more-below=""
            >
              More below
              <ChevronDownIcon aria-hidden className="size-3.5" />
            </button>
          </div>
        ) : null}
      </div>
      <AddComputerSheet
        open={addOpen}
        onOpenChange={(open) => {
          setAddOpen(open);
          if (open) return;
          setAddEnter(false);
          // Joined with a code: this computer is in the account now.
          if (addNeedsProof && store?.thisMachine.get().setupError === null) {
            setAddNeedsProof(false);
            setAddError(null);
            setThisState("added");
          }
        }}
        initialView={addEnter ? "enter" : "pick"}
      />
    </div>
  );
}

type NewRunScreenName = NewRunSaved["screen"];
