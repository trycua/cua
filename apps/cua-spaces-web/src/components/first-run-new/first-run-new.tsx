// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run (CUA-3318), phase 1 of the approved storyboard
 * (CUA-3193): drawn on the spaces.cua.ai hero (its starfield, keys, Outfit title, white button and
 * the Cua lockup). The words and choices are `lib/first-run-new.ts`; this file reads the app's state
 * (sign-in, the account's computers, this computer's agents and whether it can make a separate
 * computer) and draws the screens.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon, CopyIcon, LoaderCircleIcon, MonitorIcon, PlusIcon } from "lucide-react";
import { useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";

import { folderLabel, isNeedsProof, newPendingId, useMachines, useSession, useSettings, useSpaces, type SignInPhase } from "@/bridge";
import { BridgeContext } from "@/bridge/BridgeProvider";
import { useAppPermissions } from "@/bridge/app-permissions";
import { readFirstRun, useFirstRunSaved, writeFirstRun } from "@/bridge/first-run";
import { hostOs } from "@/bridge/host-os";
import { useFirstSpaceOffers } from "@/bridge/new-space-hooks";
import { onboardingInitial, onboardingTelemetry, onboardingView, readSavedOnboarding, writeSavedOnboarding } from "@/bridge/onboarding";
import { extraName, spokenName } from "@/bridge/space-names";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { AppPermissionList, type PermissionsView } from "@/components/app-permissions";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { agoLabel, useRecentThreads } from "@/components/ask-first/pick-up";
import { CuaLogo } from "@/components/cua-logo";
import { Switch } from "@/components/ui/switch";
import { permissionsStepView, type PermissionsStepView } from "@/lib/app-permissions";
import { addImportedTask, type AskTask } from "@/lib/ask-first";
import {
  afterWhere,
  agentView,
  chosenAgent,
  dotStep,
  INSTALL_PAGES,
  NEW_RUN_EMPTY,
  placeName,
  reachedAfter,
  readyView,
  signInView,
  stepDots,
  togglePick,
  WELCOME,
  welcomeUsage,
  whereView,
  type AgentView,
  type ComputerRow,
  type NewRunSaved,
  type ReadyView,
  type SignInView,
  type StepDot,
  type WelcomeUsage,
  type WhereView,
} from "@/lib/first-run-new";
import { spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import { FloatKeys } from "./float-keys";
import { CARD, GLASS_CHIP, GREY } from "./hero";
import { OsMark } from "./os-marks";
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
// design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
const LINK = "font-semibold text-white underline decoration-white/50 underline-offset-[3px] outline-none hover:decoration-white focus-visible:ring-2 focus-visible:ring-[#5ec0e8] rounded-sm";

function Title({ children }: { children: ReactNode }) {
  return (
    <h1 tabIndex={-1} data-onboarding-heading="" className="font-display text-[clamp(34px,min(4.9vw,6.6vh),76px)] leading-[0.98] font-semibold tracking-[-0.045em] text-balance text-white outline-none">
      {children}
    </h1>
  );
}

function Line({ children }: { children: ReactNode }) {
  return <p className={cn("mt-[min(20px,2.2vh)] max-w-[430px] text-[16px] leading-[1.55] font-medium", GREY)}>{children}</p>;
}

function Check({ on }: { on: boolean }) {
  return on ? (
    <span className="flex size-[22px] shrink-0 items-center justify-center rounded-full bg-white text-[#0a152d]" data-check="on">
      <CheckIcon className="size-[13px]" strokeWidth={3} />
    </span>
  ) : (
    <span className="size-[22px] shrink-0 rounded-full shadow-[inset_0_0_0_1.5px_rgb(255_255_255/0.35)]" data-check="off" />
  );
}

function Mark({ os }: { os: ComputerRow["os"] }) {
  return <span className="flex size-7 shrink-0 items-center justify-center text-white">{os ? <OsMark os={os} className="h-[17px] w-auto" /> : <MonitorIcon className="size-4" />}</span>;
}

/** A row in the card: picked rows are lit; `disabled` rows are greyed, with no check and no hover. */
function Row({ icon, name, sub, picked, disabled, onClick, extra, check = true, ...rest }: { icon: ReactNode; name: string; sub: ReactNode; picked?: boolean; disabled?: boolean; onClick?: () => void; extra?: ReactNode; check?: boolean } & Record<`data-${string}`, string | undefined>) {
  const body = (
    <>
      {icon}
      <span className="min-w-0 flex-1 text-left">
        <span className="block text-[15px] leading-5 font-medium text-white">{name}</span>
        <span className={cn("flex items-center gap-1.5 text-[13.5px] leading-[19px] font-medium", GREY)}>{sub}</span>
      </span>
      {extra}
      {check && !disabled ? <Check on={Boolean(picked)} /> : null}
    </>
  );
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  const cls = cn("flex w-full items-center gap-3 rounded-[14px] px-3 py-[7px]", picked && "bg-white/10 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.5)]", disabled && "pointer-events-none cursor-not-allowed opacity-40");
  if (!onClick || disabled)
    return (
      <div className={cls} aria-disabled={disabled || undefined} {...rest}>
        {body}
      </div>
    );
  return (
    <div role="checkbox" aria-checked={Boolean(picked)} tabIndex={0} className={cn(cls, "cursor-pointer outline-none hover:bg-white/[0.06] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]")} onClick={onClick} onKeyDown={(e) => (e.key === " " || e.key === "Enter") && (e.preventDefault(), onClick())} {...rest}>
      {body}
    </div>
  );
}

const Divider = () => <div className="mx-3 my-1.5 h-px bg-white/[0.14]" />;
const Label = ({ children }: { children: ReactNode }) => <div className={cn("px-3 pt-3 pb-1 text-[12.5px] font-semibold tracking-[0.02em]", GREY)}>{children}</div>;

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
      const above = footer ? [...(footer.parentElement?.children ?? [])].filter((el): el is HTMLElement => el instanceof HTMLElement && el !== footer && Boolean(el.compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING)) : [];
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

export function WelcomeScreen({ onAdd, usage, onUsage, onUsageLink }: { onAdd: () => void; usage?: WelcomeUsage | null; onUsage?: (on: boolean) => void; onUsageLink?: () => void }) {
  return (
    <div data-new-run="welcome">
      <Title>{WELCOME.title}</Title>
      <Line>{WELCOME.line}</Line>
      {usage ? (
        // Before anything is sent: the hosts hold every usage event until the run leaves this screen.
        <div className={cn("mt-6 flex max-w-[520px] items-start gap-3 text-[13px] leading-[19px] font-medium", GREY)} data-new-run-usage={usage.on ? "on" : "off"}>
          <Switch className="mt-px shrink-0" checked={usage.on} disabled={!usage.enabled} onCheckedChange={(on) => onUsage?.(on)} aria-label={usage.label} data-new-run-usage-switch="" />
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

export function SignInScreen({ view, onTryAgain, onUseCode, onOpen }: { view: SignInView; onTryAgain: () => void; onUseCode: () => void; onOpen: (url: string) => void }) {
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
      <Actions className="flex flex-wrap items-center gap-3">
        <button type="button" className={view.tryAgainPrimary ? WHITE_BUTTON : GLASS_BUTTON} onClick={onTryAgain} data-new-run-try-again="">
          {view.tryAgain}
        </button>
        {view.useCode ? (
          <button type="button" className={GLASS_BUTTON} onClick={onUseCode} data-new-run-use-code="">
            {view.useCode}
          </button>
        ) : null}
      </Actions>
    </div>
  );
}

export function WhereScreen({
  view,
  onToggle,
  onAddThis,
  onSignIn,
  onAddAnother,
  onSeparate,
  onContinue,
}: {
  view: WhereView;
  onToggle: (id: string) => void;
  onAddThis: (action: "add" | "retry-add" | "use-code") => void;
  onSignIn: () => void;
  onAddAnother: () => void;
  onSeparate: () => void;
  onContinue: () => void;
}) {
  const t = view.thisRow;
  const sep = view.separate;
  const fold = view.offlineFold;
  const [showOffline, setShowOffline] = useState(false);
  const other = (o: ComputerRow) => <Row key={o.id} icon={<Mark os={o.os} />} name={o.name} sub={o.sub} picked={o.picked} disabled={o.disabled} onClick={() => onToggle(o.id)} data-new-run-computer={o.id} data-offline={o.disabled ? "" : undefined} />;
  return (
    <div data-new-run="where">
      <div data-new-run-content="">
        <Title>{view.title}</Title>
        <Line>{view.line}</Line>
        <div className={cn(CARD, "mt-[min(28px,3vh)]")} data-new-run-computers="">
          <Row
            icon={<Mark os={hostOs()} />}
            name={t.name}
            sub={
              <>
                {t.model && t.modelFirst ? <span>{t.model} ·</span> : null}
                {t.tick ? <CheckIcon aria-hidden className="size-[13px] text-white" strokeWidth={2.6} /> : null}
                <span className={t.tick ? "text-white" : undefined} data-new-run-this-status="">
                  {t.status}
                </span>
                {t.model && !t.modelFirst ? <span>· {t.model}</span> : null}
              </>
            }
            picked={t.picked}
            onClick={() => onToggle("this")}
            extra={
              t.action ? (
                <button
                  type="button"
                  className={cn(GLASS_CHIP, "mr-1 h-[30px] px-3.5 text-[13.5px]")}
                  onClick={(e) => {
                    e.stopPropagation();
                    onAddThis(t.action!.id);
                  }}
                  data-new-run-add-this={t.action.id}
                >
                  {t.action.label}
                </button>
              ) : null
            }
            data-new-run-computer="this"
          />
          {view.others ? (
            <>
              <Label>{view.othersLabel}</Label>
              {view.others.length ? (
                <>
                  {(fold ? view.others.filter((o) => !o.offline) : view.others).map(other)}
                  {fold ? (
                    <>
                      <button
                        type="button"
                        aria-expanded={showOffline}
                        // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
                        className="flex w-full items-center gap-3 rounded-[14px] px-3 py-[7px] text-left outline-none hover:bg-white/[0.06] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]"
                        onClick={() => setShowOffline((v) => !v)}
                        data-new-run-offline-fold={fold.rows.length}
                      >
                        <span className="flex size-7 shrink-0 items-center justify-center text-white">
                          <ChevronRightIcon className={cn("size-4 transition-transform motion-reduce:transition-none", showOffline && "rotate-90")} strokeWidth={2.2} />
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block text-[15px] leading-5 font-medium text-white">{fold.label}</span>
                          <span className={cn("block text-[13.5px] leading-[19px] font-medium", GREY)}>{fold.sub}</span>
                        </span>
                      </button>
                      {showOffline ? fold.rows.map(other) : null}
                    </>
                  ) : null}
                </>
              ) : (
                <p className={cn("px-3 pt-1 pb-2 text-[13.5px] leading-[19px] font-medium", GREY)} data-new-run-none="">
                  {view.emptyLine}
                </p>
              )}
            </>
          ) : null}
          {view.addAnother ? (
            <>
              <Divider />
              {/* design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages */}
              <button type="button" className="flex w-full items-center gap-3 rounded-[14px] px-3 py-2 text-left outline-none hover:bg-white/[0.06] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]" onClick={onAddAnother} data-new-run-add-another="">
                <span className="flex size-7 shrink-0 items-center justify-center rounded-full text-white shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)]">
                  <PlusIcon className="size-4" strokeWidth={2.2} />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-[15px] leading-5 font-medium text-white">{view.addAnother.title}</span>
                  <span className={cn("block text-[13.5px] leading-[19px] font-medium", GREY)}>{view.addAnother.sub}</span>
                </span>
              </button>
            </>
          ) : null}
          {view.signInLine ? (
            <>
              <Divider />
              <p className={cn("px-3 pt-2.5 pb-3 text-[14px] leading-5 font-medium", GREY)} data-new-run-sign-in-line="">
                <button type="button" className={LINK} onClick={onSignIn}>
                  {view.signInLine.link}
                </button>
                {view.signInLine.rest}
              </p>
            </>
          ) : null}
        </div>
        {/* Only where this computer can make one (CUA-3150, idea D). */}
        {sep ? (
        <div className="mt-[min(18px,2vh)] max-w-[480px] px-[18px]" data-new-run-separate={sep.state} data-disabled={sep.disabled ? "" : undefined}>
          <div className={cn("flex items-center gap-2 text-[14px] leading-5 font-semibold", sep.disabled ? GREY : "text-white")}>
            <OsMark os="linux" className="h-[15px] w-4 shrink-0" />
            {sep.disabled || sep.state !== "offer" ? (
              <span>{sep.title}</span>
            ) : (
              <button type="button" className={LINK} onClick={onSeparate} data-new-run-separate-open="">
                {sep.title}
              </button>
            )}
          </div>
          <p className={cn("mt-[3px] pl-6 text-[13.5px] leading-[19px] font-medium", GREY)}>{sep.sub}</p>
          {sep.action ? (
            <div className="mt-2 pl-6">
              <button type="button" className={cn(GLASS_CHIP, "h-[30px] px-3.5 text-[13.5px]")} onClick={onSeparate} data-new-run-separate-action="">
                {sep.action}
              </button>
            </div>
          ) : null}
        </div>
        ) : null}
      </div>
      <Actions>
        <button type="button" className={WHITE_BUTTON} disabled={!view.continueEnabled} onClick={onContinue} data-onboarding-primary="">
          Continue
        </button>
      </Actions>
    </div>
  );
}

export function AgentScreen({ view, onPick, onContinue, onInstall, busy }: { view: AgentView; onPick: (id: string) => void; onContinue: () => void; onInstall: (agent: "claude-code" | "codex") => void; busy: boolean }) {
  return (
    <div data-new-run="agent" data-state={view.state}>
      <Title>{view.title}</Title>
      {view.line ? <Line>{view.line}</Line> : null}
      {view.agents.length ? (
        <div className={cn(CARD, "mt-[min(28px,3vh)]")} data-new-run-agents="">
          <div className={cn("flex items-center gap-2 px-3 pt-2 pb-2 text-[12.5px] font-semibold tracking-[0.02em]", GREY)}>
            <OsMark os={hostOs()} className="h-3 w-auto" />
            {view.label}
          </div>
          {view.agents.map((a) => (
            <Row
              key={a.id}
              icon={<span className="flex size-7 shrink-0 items-center justify-center">{<AppMarkImage agent={a.id} className="size-5" />}</span>}
              name={a.name}
              sub={a.maker}
              picked={a.picked}
              onClick={view.state === "several" ? () => onPick(a.id) : undefined}
              data-new-run-agent={a.id}
            />
          ))}
        </div>
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
          </Actions>
        </>
      ) : (
        // Shown (greyed) while the agents are still being looked for, so it doesn't pop in once they're found.
        <Actions>
          <button type="button" className={WHITE_BUTTON} disabled={!view.continueEnabled || busy} onClick={onContinue} data-onboarding-primary="">
            {busy ? "Setting up…" : "Continue"}
          </button>
        </Actions>
      )}
    </div>
  );
}

/**
 * "Pick up where you left off" (CUA-3319): 2 to 4 of the person's recent chats with their agents on
 * this computer, each with **Continue in Cua**, which opens it in a new Home task with what was said
 * so far. Nothing runs until they send. Hidden when there are none. Only each chat's title, folder
 * and time are read here; its messages are read when it is picked.
 */
export function PickUpWhereYouLeftOff({ onContinue, place = thisComputerText(), className }: { onContinue: (task: AskTask) => void; place?: string; className?: string }) {
  const threads = useRecentThreads();
  if (!threads?.length) return null;
  return (
    <section aria-labelledby="pick-up-heading" className={cn("w-full", className)} data-pick-up="">
      <h2 id="pick-up-heading" className="text-[15px] font-semibold text-white">
        Pick up where you left off
      </h2>
      <ul className={cn(CARD, "mt-2.5 max-w-[600px]")}>
        {threads.map((t) => {
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
              <button type="button" className={cn(GLASS_CHIP, "h-[30px] shrink-0 px-3 text-[13.5px]")} onClick={() => onContinue(addImportedTask(t))} data-pick-up-continue="">
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

export function ReadyScreen({ view, agent, onDone, onContinueChat, onCopied, place }: { view: ReadyView; agent: string; onDone: () => void; onContinueChat?: (task: AskTask) => void; onCopied?: () => void; place?: string }) {
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
      {/* design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages */}
      <div className="mt-7 flex w-full max-w-[600px] items-center gap-3 rounded-[20px] bg-white/[0.08] py-2.5 pr-2.5 pl-4 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)] backdrop-blur-md" data-new-run-command="">
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
      {/* The person's own recent chats on this computer (CUA-3319), hidden when there are none. */}
      {onContinueChat ? <PickUpWhereYouLeftOff className="mt-[min(28px,3vh)]" place={place} onContinue={onContinueChat} /> : null}
      <Actions>
        <button type="button" className={WHITE_BUTTON} onClick={onDone} data-onboarding-primary="">
          {view.done}
        </button>
      </Actions>
    </div>
  );
}

/** 2b, on a Mac: Cua Spaces' two permissions (the same rows as the Cua Driver page), each with Allow, and Later. Never blocks the run. */
export function PermissionsScreen({ view, perms, onContinue }: { view: PermissionsStepView; perms: PermissionsView; onContinue: () => void }) {
  return (
    <div data-new-run="permissions">
      <Title>{view.title}</Title>
      <Line>{view.line}</Line>
      <AppPermissionList perms={perms} variant="hero" className="mt-[min(28px,3vh)]" footer={view.note} />
      <Actions>
        <button type="button" className={view.primary.white ? WHITE_BUTTON : GLASS_BUTTON} onClick={onContinue} data-onboarding-primary="" data-new-run-permissions-next={view.primary.label}>
          {view.primary.label}
        </button>
      </Actions>
    </div>
  );
}

/* ---- The page ------------------------------------------------------------------------- */

/** The daemon's refusal on a Mac that can't run virtual computers (`cua_create_failure::preflight::MAC_UNSUPPORTED`). */
const CANT_RUN = /can['\u2019]t run (Spaces itself|virtual computers)/i;

/**
 * The usage data's name for each screen (`schema::FIRST_RUN_STEPS`), so the onboarding funnel keeps its steps:
 * Ready ends the run and is counted by its finish, not as a step.
 */
const FUNNEL_STEP: Partial<Record<NewRunScreenName, string>> = {
  welcome: "welcome",
  signin: "sign_in",
  where: "create_space",
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
  const spaces = useSpaces();
  const offers = useFirstSpaceOffers();
  const savedAll = useFirstRunSaved();
  const s: NewRunSaved = { ...NEW_RUN_EMPTY, ...(savedAll.newRun ?? {}) };
  const save = (patch: Partial<NewRunSaved>) => {
    const prev = { ...NEW_RUN_EMPTY, ...(readFirstRun().newRun ?? {}) };
    const reached = patch.screen ? reachedAfter(reachedAfter(prev.reached, prev.screen), patch.screen) : prev.reached;
    writeFirstRun({ newRun: { ...prev, ...patch, reached } });
  };
  const go = (screen: NewRunScreenName) => save({ screen });

  const signedIn = Boolean(session.data?.signedIn);
  const phase: SignInPhase = session.data?.signIn ?? { kind: "idle" };
  const [now, setNow] = useState(() => Date.now());
  const [found, setFound] = useState<string[] | null>(null);
  const [agentPick, setAgentPick] = useState<string | null>(s.agent);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [thisState, setThisState] = useState<"adding" | "added" | "failed">("adding");
  const [addError, setAddError] = useState<string | null>(null);
  const [addNeedsProof, setAddNeedsProof] = useState(false);
  const [explain, setExplain] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  // "Use a code from another computer": the sheet opens on "Enter a code".
  const [addEnter, setAddEnter] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  useFadeUnder(mainRef, s.screen);

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
  const usage = s.screen === "welcome" && core && telemetry ? welcomeUsage(onboardingView(core, { ...onboardingInitial(core, null), step: "welcome", telemetry: onboardingTelemetry(telemetry) })) : null;
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
    const el = keep !== null ? document.querySelector<HTMLElement>(`[data-step-dot="${keep}"]`) : document.querySelector<HTMLElement>("[data-onboarding-heading]");
    el?.focus();
  }, [s.screen]);

  // Sign-in: started when 1b opens, and over once the session says signed in.
  const started = useRef(false);
  useEffect(() => {
    if (s.screen !== "signin") {
      started.current = false;
      return;
    }
    if (signedIn) {
      save({ screen: "where", addedInRun: true });
      return;
    }
    if (!started.current && phase.kind === "idle") {
      started.current = true;
      void session.signIn().catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, signedIn, phase.kind]);

  // Back from the browser: read the sign-in again at once, so one that finished
  // (or failed) there shows now, not on the next host push.
  const waitingOnSignIn = s.screen === "signin" && (phase.kind === "starting" || phase.kind === "waiting");
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

  // Screen 2, signed in: this computer joins the account (once; the store also does it on sign-in).
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
    if (s.screen === "where" && signedIn) void addThis();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, signedIn]);

  // The account's computers, read again while screen 2 shows.
  useEffect(() => {
    if (s.screen !== "where" || !signedIn) return;
    void machines.refresh().catch(() => {});
    const t = setInterval(() => {
      setNow(Date.now());
      void machines.refresh().catch(() => {});
    }, 10_000);
    return () => clearInterval(t);
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

  // 2b: Cua Spaces' permissions, read while the step shows. A host with none to give (not a Mac, an older app) has
  // no step: it goes on to the coding agent.
  const perms = useAppPermissions(s.screen === "permissions");
  useEffect(() => {
    if (s.screen === "permissions" && !perms.loading && !perms.available) go("agent");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.screen, perms.loading, perms.available]);

  // The separate computer: this page's create, followed in the Spaces list.
  const linux = offers.offers.find((o) => o.id === "linux") ?? null;
  const made = useMemo(() => {
    if (explain) return { phase: "explain" as const, name: null, error: null, id: null };
    if (!s.separatePendingId) return { phase: "none" as const, name: null, error: null, id: null };
    const id = store?.createdId(s.separatePendingId) ?? s.separatePendingId;
    const row = spaces.data?.find((x) => x.id === id);
    if (!row) return { phase: "making" as const, name: null, error: null, id: null };
    const st = spaceState(row);
    const name = spokenName(row.name);
    if (st === "failed") return { phase: "failed" as const, name, error: row.progress?.error ?? null, id: row.id };
    return { phase: st === "creating" ? ("making" as const) : ("ready" as const), name, error: null, id: row.id };
  }, [explain, s.separatePendingId, spaces.data, store]);
  // A Mac that can't run virtual computers is only known when the daemon refuses the create (#6323): then
  // the option turns grey with the plain sentence, instead of a failure.
  const cantRun = made.phase === "failed" && CANT_RUN.test(made.error ?? "");
  const canMakeSeparate = cantRun ? false : !offers.known ? null : Boolean(linux && !linux.blocked);
  const separate = cantRun ? { phase: "none" as const, name: null, error: null, id: null } : made;

  const makeSeparate = () => {
    if (!linux || linux.blocked) return;
    // On a Mac, our one line first; the ask itself is the daemon's, before the create (#6009).
    if (os === "macos" && !explain && separate.phase !== "failed") return setExplain(true);
    setExplain(false);
    const pendingId = newPendingId();
    save({ separatePendingId: pendingId, picks: [...new Set([...s.picks, `pending:${pendingId}`])] });
    spaces.createSpace({ ...linux.args, name: extraName("linux", spaces.data ?? []), os: linux.os, notifyWhenReady: true }, pendingId).catch(() => {
      // The Spaces list shows the failure; the line reads it from there.
    });
  };

  const whereInput = {
    os,
    signedIn,
    thisState: signedIn ? thisState : ("signed-out" as const),
    addedInRun: s.addedInRun,
    addError,
    addNeedsProof,
    model: machines.data?.find((m) => m.current)?.model ?? null,
    machines: machines.data ?? null,
    picks: s.picks,
    canMakeSeparate,
    separateRefused: cantRun,
    separateBlocked: linux?.blocked ?? null,
    separate,
    now,
  };
  const where = whereView(whereInput);

  /** The picked computers' names for screen 4: this computer, the others, then the separate computer. */
  const pickedNames = (): string[] => {
    const out: string[] = [];
    for (const p of s.picks) {
      if (p === "this") out.push("this");
      else if (p.startsWith("pending:")) {
        if (separate.name) out.push(separate.name);
      } else {
        const m = machines.data?.find((x) => x.id === p);
        if (m?.online) out.push(m.name);
      }
    }
    return out.length ? out : ["this"];
  };

  const agent = chosenAgent(found, agentPick);
  const agentScreen = agentView({ os, found, picked: agentPick, error: connectError });
  const continueAgent = async () => {
    if (!agent || !adapter || connecting) return;
    setConnecting(true);
    setConnectError(null);
    try {
      // The same connect as Settings and the old first run: Cua's tools in the agent's settings, and its sign-in kept.
      const viaConnect = await adapter.call("agents.connect", { agents: [agent], signIn: true }).then(
        () => true,
        () => false,
      );
      const rows = viaConnect ? await adapter.call("agents.setup", {}) : await adapter.call("agents.configure", { agents: [agent] });
      const row = rows.find((r) => r.agent === agent);
      if (!row?.configured) {
        setConnectError(`Cua couldn't add itself to ${row?.name ?? "the agent"}'s settings.${row?.detail ? ` ${row.detail}` : ""}`);
        return;
      }
      save({ agent, screen: "ready" });
    } catch (e) {
      setConnectError(e instanceof Error ? e.message : String(e));
    } finally {
      setConnecting(false);
    }
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
      go("where");
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

  const startSignIn = () => {
    started.current = true;
    go("signin");
    void session.signIn().catch(() => {});
  };

  const dots = stepDots(s.screen, reachedAfter(s.reached, s.screen));
  const goDot = (i: number, viaKey: boolean) => {
    const to = dots?.[i]?.to;
    if (!to) return;
    if (s.screen === "signin") session.cancelSignIn();
    if (viaKey) dotFocus.current = i;
    go(to);
  };
  const showSkip = s.screen === "welcome" || s.screen === "signin" || s.screen === "where" || s.screen === "agent";

  let body: ReactNode = null;
  switch (s.screen) {
    case "welcome":
      body = <WelcomeScreen onAdd={startSignIn} usage={usage} onUsage={(on) => void settings.updateSetting("telemetry", on).catch(() => {})} onUsageLink={() => usage && open(usage.linkUrl)} />;
      break;
    case "signin":
      body = (
        <SignInScreen
          view={signInView(phase, session.data?.signInUrl ?? null)}
          onTryAgain={() => {
            session.cancelSignIn();
            void session.signIn().catch(() => {});
          }}
          onUseCode={() => {
            session.cancelSignIn();
            void session.signIn({ code: true }).catch(() => {});
          }}
          onOpen={open}
        />
      );
      break;
    case "where":
      body = (
        <WhereScreen
          view={where}
          onToggle={(id) => save({ picks: togglePick(s.picks, id, where.others ?? []) })}
          onAddThis={(action) => {
            if (action === "use-code") {
              setAddEnter(true);
              setAddOpen(true);
            } else if (signedIn) void addThis();
            else startSignIn();
          }}
          onSignIn={startSignIn}
          onAddAnother={() => setAddOpen(true)}
          onSeparate={makeSeparate}
          onContinue={() => go(afterWhere(os))}
        />
      );
      break;
    case "permissions":
      body = <PermissionsScreen view={permissionsStepView({ appName: perms.data?.appName ?? "Cua Spaces", allGranted: perms.allGranted })} perms={perms} onContinue={() => go("agent")} />;
      break;
    case "agent":
      body = (
        <AgentScreen
          view={agentScreen}
          busy={connecting}
          onPick={(id) => {
            setAgentPick(id);
            save({ agent: id });
          }}
          onContinue={() => void continueAgent()}
          onInstall={(a) => open(INSTALL_PAGES[a])}
        />
      );
      break;
    case "ready":
      body = s.agent ? (
        <ReadyScreen
          view={readyView({ os, agent: s.agent, computers: pickedNames(), canMakeSeparate })}
          agent={s.agent}
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
    <div className="dark relative flex h-full flex-col overflow-hidden bg-[#050607] text-white" data-window-surface="#050607" data-new-first-run="" data-step={s.screen}>
      <FloatKeys copyRight={544} />
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
          (`Actions`) sits at the bottom of it; content longer than the window scrolls above that row. */}
      <main ref={mainRef} className="relative z-20 flex min-h-0 flex-1 flex-col overflow-y-auto px-16" data-new-run-main="">
        <div className="flex w-full max-w-[600px] shrink-0 grow flex-col pt-[clamp(16px,8vh,96px)] *:flex *:grow *:flex-col">{body}</div>
      </main>
      {/* The dots' row keeps its height on the screens without dots, so the area above it keeps its height too. */}
      <div className="relative z-20 flex h-[38px] shrink-0 flex-col justify-end" data-new-run-dots-row="">
        {dots ? <StepDots dots={dots} onGo={goDot} /> : null}
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
