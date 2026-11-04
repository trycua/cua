// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The new first run (Labs, "Try the new setup"; CUA-3318), phase 1 of the approved storyboard
 * (CUA-3193): drawn on the spaces.cua.ai hero (its starfield, keys, Outfit title, white button and
 * the Cua lockup). The words and choices are `lib/first-run-new.ts`; this file reads the app's state
 * (sign-in, the account's computers, this computer's agents and whether it can make a separate
 * computer) and draws the screens.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronRightIcon, CopyIcon, LoaderCircleIcon, MonitorIcon, PlusIcon, ShieldIcon, UserPlusIcon } from "lucide-react";
import { useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { folderLabel, newPendingId, useMachines, useSession, useSettings, useSpaces, type SignInPhase } from "@/bridge";
import { BridgeContext } from "@/bridge/BridgeProvider";
import type { HostStatus } from "@/bridge/contracts/host";
import { readFirstRun, useFirstRunSaved, writeFirstRun } from "@/bridge/first-run";
import { hostOs } from "@/bridge/host-os";
import { useFirstSpaceOffers } from "@/bridge/new-space-hooks";
import { readSavedOnboarding, writeSavedOnboarding } from "@/bridge/onboarding";
import { extraName, spokenName } from "@/bridge/space-names";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { agoLabel, useRecentThreads } from "@/components/ask-first/pick-up";
import { CuaLogo } from "@/components/cua-logo";
import { addImportedTask, type AskTask } from "@/lib/ask-first";
import {
  agentView,
  chosenAgent,
  dotFor,
  INSTALL_PAGES,
  NEW_RUN_EMPTY,
  nextView,
  placeName,
  readyView,
  signInView,
  togglePick,
  WELCOME,
  whereView,
  type AgentView,
  type ComputerRow,
  type NewRunSaved,
  type NextView,
  type ReadyView,
  type SignInView,
  type WhereView,
} from "@/lib/first-run-new";
import { spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import { FloatKeys } from "./float-keys";
import { OsMark } from "./os-marks";

/* ---- The site's pieces ------------------------------------------------------------------ */

/** The hero's grey for second lines. */
const GREY = "text-[#8d8d8d]";
/** The site's white button (the dark hero's Download pill). */
const WHITE_BUTTON =
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  "inline-flex h-12 items-center justify-center rounded-full bg-white px-7 text-[15px] font-semibold text-[#0a152d] outline-none transition-transform hover:scale-[1.04] active:scale-[0.98] focus-visible:ring-2 focus-visible:ring-[#5ec0e8] disabled:pointer-events-none disabled:opacity-40 motion-reduce:transition-none";
/** The site's Sign up glass at the white button's height. */
const GLASS_BUTTON =
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  "inline-flex h-12 items-center justify-center rounded-full bg-white/[0.08] px-6 text-[15px] font-semibold text-white shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)] backdrop-blur-md outline-none hover:bg-white/[0.14] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]";
/** The site's Sign up chip (Skip for now, a row's Add). */
const GLASS_CHIP =
  "inline-flex items-center justify-center rounded-full bg-white/[0.08] font-semibold text-white shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)] backdrop-blur-md outline-none hover:bg-white/[0.14] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]";
/** The site's underlined text link, in white. */
// design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
const LINK = "font-semibold text-white underline decoration-white/50 underline-offset-[3px] outline-none hover:decoration-white focus-visible:ring-2 focus-visible:ring-[#5ec0e8] rounded-sm";
/** The list card: the Sign up glass with the download menu's corners. */
// design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
const CARD = "w-full max-w-[480px] rounded-[20px] bg-white/[0.08] p-1.5 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)] backdrop-blur-md";

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

/* ---- The screens (drawn from their views, so tests draw every state) --------------------- */

export function WelcomeScreen({ onAdd }: { onAdd: () => void }) {
  return (
    <div data-new-run="welcome">
      <Title>{WELCOME.title}</Title>
      <Line>{WELCOME.line}</Line>
      <div className="mt-9">
        <button type="button" className={WHITE_BUTTON} onClick={onAdd} data-onboarding-primary="">
          {WELCOME.primary}
        </button>
      </div>
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
      <div className="mt-6 flex flex-wrap items-center gap-3">
        <button type="button" className={view.tryAgainPrimary ? WHITE_BUTTON : GLASS_BUTTON} onClick={onTryAgain} data-new-run-try-again="">
          {view.tryAgain}
        </button>
        {view.useCode ? (
          <button type="button" className={GLASS_BUTTON} onClick={onUseCode} data-new-run-use-code="">
            {view.useCode}
          </button>
        ) : null}
      </div>
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
  onAddThis: () => void;
  onSignIn: () => void;
  onAddAnother: () => void;
  onSeparate: () => void;
  onContinue: () => void;
}) {
  const t = view.thisRow;
  const sep = view.separate;
  return (
    <div data-new-run="where">
      <Title>{view.title}</Title>
      <Line>{view.line}</Line>
      <div className={cn(CARD, "mt-[min(28px,3vh)]")} data-new-run-computers="">
        <Row
          icon={<Mark os="macos" />}
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
                  onAddThis();
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
              view.others.map((o) => <Row key={o.id} icon={<Mark os={o.os} />} name={o.name} sub={o.sub} picked={o.picked} disabled={o.disabled} onClick={() => onToggle(o.id)} data-new-run-computer={o.id} data-offline={o.disabled ? "" : undefined} />)
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
      <div className="mt-[min(28px,3vh)]">
        <button type="button" className={WHITE_BUTTON} disabled={!view.continueEnabled} onClick={onContinue} data-onboarding-primary="">
          Continue
        </button>
      </div>
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
            <OsMark os="macos" className="h-3 w-auto" />
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
          <div className="mt-9 flex flex-wrap items-center gap-6">
            <button type="button" className={WHITE_BUTTON} onClick={() => onInstall("claude-code")} data-new-run-install="claude-code">
              {view.install.primary}
            </button>
            <button type="button" className={cn(LINK, "text-[15px]")} onClick={() => onInstall("codex")} data-new-run-install="codex">
              {view.install.secondary}
            </button>
          </div>
          <p className={cn("mt-4 max-w-[480px] text-[13.5px] font-medium", GREY)}>{view.install.note}</p>
        </>
      ) : view.state !== "looking" ? (
        <div className="mt-[min(28px,3vh)]">
          <button type="button" className={WHITE_BUTTON} disabled={!view.continueEnabled || busy} onClick={onContinue} data-onboarding-primary="">
            {busy ? "Setting up…" : "Continue"}
          </button>
        </div>
      ) : null}
    </div>
  );
}

/**
 * "Pick up where you left off" (CUA-3319): 2 to 4 of the person's recent chats with their agents on
 * this computer, each with **Continue in Cua**, which opens it in a new Home task with what was said
 * so far. Nothing runs until they send. Hidden when there are none. Only each chat's title, folder
 * and time are read here; its messages are read when it is picked.
 */
export function PickUpWhereYouLeftOff({ onContinue, place = "this Mac", className }: { onContinue: (task: AskTask) => void; place?: string; className?: string }) {
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

export function ReadyScreen({ view, agent, onDone, onContinueChat, place }: { view: ReadyView; agent: string; onDone: () => void; onContinueChat?: (task: AskTask) => void; place?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
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
      <div className="mt-6">
        <button type="button" className={WHITE_BUTTON} onClick={onDone} data-onboarding-primary="">
          {view.done}
        </button>
      </div>
    </div>
  );
}

export function NextScreen({ view, onRow, onDone, apps }: { view: NextView; onRow: (id: NextView["rows"][number]["id"]) => void; onDone: () => void; apps?: ReactNode }) {
  const ICON = { invite: UserPlusIcon, "add-computer": MonitorIcon, apps: ShieldIcon } as const;
  return (
    <div data-new-run="next">
      <Title>{view.title}</Title>
      <Line>{view.line}</Line>
      <div className={cn(CARD, "mt-[min(28px,3vh)]")} data-new-run-next="">
        {view.rows.map((r) => {
          const Icon = ICON[r.id];
          return (
            <div key={r.id}>
              {/* design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages */}
              <button type="button" className="flex w-full items-center gap-3 rounded-[14px] px-3 py-2.5 text-left outline-none hover:bg-white/[0.06] focus-visible:ring-2 focus-visible:ring-[#5ec0e8]" onClick={() => onRow(r.id)} data-new-run-next-row={r.id}>
                <span className="flex size-7 shrink-0 items-center justify-center rounded-full text-white shadow-[inset_0_0_0_1px_rgb(255_255_255/0.28)]">
                  <Icon className="size-3.5" />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-[15px] leading-5 font-medium text-white">{r.title}</span>
                  <span className={cn("block text-[13.5px] leading-[19px] font-medium", GREY)}>{r.sub}</span>
                </span>
                <ChevronRightIcon className={cn("size-4 shrink-0", GREY)} />
              </button>
              {r.id === "apps" ? apps : null}
            </div>
          );
        })}
      </div>
      <div className="mt-[min(28px,3vh)]">
        <button type="button" className={WHITE_BUTTON} onClick={onDone} data-onboarding-primary="">
          {view.done}
        </button>
      </div>
    </div>
  );
}

/** "Let agents use apps on this Mac", opened: the two permissions, each through the app's guide (PermissionGuide). */
export function AppsPermissions({ rows, onAllow }: { rows: { id: string; title: string; granted: boolean; url: string }[]; onAllow: (url: string) => void }) {
  // design-ok: the first run is drawn from the spaces.cua.ai hero (storyboard CUA-3193), not the app's pages
  const box = "mx-3 mb-2 flex flex-col gap-1 border-t border-white/[0.14] pt-2 pl-10";
  return (
    <div className={box} data-new-run-apps="">
      {rows.map((r) => (
        <div key={r.id} className="flex min-h-9 items-center gap-3" data-new-run-permission={r.id} data-granted={String(r.granted)}>
          <span className="min-w-0 flex-1 text-[14px] font-medium text-white">{r.title}</span>
          {r.granted ? (
            <span className="flex items-center gap-1 text-[13.5px] font-medium text-white">
              <CheckIcon className="size-3.5" /> Allowed
            </span>
          ) : (
            <button type="button" className={cn(GLASS_CHIP, "h-[30px] px-3.5 text-[13.5px]")} onClick={() => onAllow(r.url)}>
              Allow
            </button>
          )}
        </div>
      ))}
      <p className={cn("pb-1 text-[12.5px] font-medium", GREY)}>Allow opens System Settings, where you turn on Cua Spaces.</p>
    </div>
  );
}

/* ---- The page ------------------------------------------------------------------------- */

/** The daemon's refusal on a Mac that can't run virtual computers (`cua_create_failure::preflight::MAC_UNSUPPORTED`). */
const CANT_RUN = /can['\u2019]t run (Spaces itself|virtual computers)/i;

const PERMISSIONS = [
  { id: "screen-recording", title: "See the screen", url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" },
  { id: "accessibility", title: "Use the mouse and keyboard", url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" },
];

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
  const save = (patch: Partial<NewRunSaved>) => writeFirstRun({ newRun: { ...NEW_RUN_EMPTY, ...(readFirstRun().newRun ?? {}), ...patch } });
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
  const [explain, setExplain] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [appsOpen, setAppsOpen] = useState(false);
  const [host, setHost] = useState<HostStatus | null>(null);

  // A new screen: focus moves to its title, so a screen reader reads it.
  useEffect(() => {
    document.querySelector<HTMLElement>("[data-onboarding-heading]")?.focus();
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
    const ok = await store.addThisComputer(session.data?.identity ?? null).catch(() => false);
    if (n !== addRun.current) return;
    setThisState(ok ? "added" : "failed");
    if (!ok) setAddError(store.thisMachine.get().setupError?.message ?? null);
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

  // Screen 6's permissions, read while they show.
  useEffect(() => {
    if (!appsOpen || !adapter) return;
    let live = true;
    const read = () =>
      void adapter
        .call("host.status", {})
        .then((h) => live && setHost(h))
        .catch(() => {});
    read();
    const t = setInterval(read, 2_500);
    window.addEventListener("focus", read);
    return () => {
      live = false;
      clearInterval(t);
      window.removeEventListener("focus", read);
    };
  }, [appsOpen, adapter]);

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
    model: machines.data?.find((m) => m.current)?.model ?? null,
    machines: machines.data ?? null,
    picks: s.picks,
    canMakeSeparate,
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
    session.cancelSignIn();
    writeSavedOnboarding({ ...readSavedOnboarding(), skipped: true });
    writeFirstRun({ finishedLater: true });
    void navigate({ to: "/spaces" });
  };
  const skip = () => {
    if (s.screen === "welcome" || s.screen === "signin") {
      session.cancelSignIn();
      go("where");
    } else leave();
  };
  const finish = async () => {
    await session.completeOnboarding(thisState === "added" && signedIn ? "host" : "client").catch(() => {});
    writeFirstRun({ finishedLater: false, todosClosed: false });
    await navigate({ to: "/home" });
  };
  // Continue in Cua (CUA-3319): setup is done and the picked chat opens as a Home task. Nothing runs until the person sends.
  const continueChat = async (task: AskTask) => {
    await session.completeOnboarding(thisState === "added" && signedIn ? "host" : "client").catch(() => {});
    writeFirstRun({ finishedLater: false, todosClosed: false });
    await navigate({ to: "/home/$taskId", params: { taskId: task.id } });
  };

  const startSignIn = () => {
    started.current = true;
    go("signin");
    void session.signIn().catch(() => {});
  };

  const accountUrl = settings.data?.accountUrl ?? "https://app.cua.ai";
  const dot = dotFor(s.screen);
  const showSkip = s.screen === "welcome" || s.screen === "signin" || s.screen === "where" || s.screen === "agent";

  let body: ReactNode = null;
  switch (s.screen) {
    case "welcome":
      body = <WelcomeScreen onAdd={startSignIn} />;
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
          onAddThis={() => (signedIn ? void addThis() : startSignIn())}
          onSignIn={startSignIn}
          onAddAnother={() => setAddOpen(true)}
          onSeparate={makeSeparate}
          onContinue={() => go("agent")}
        />
      );
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
        <ReadyScreen view={readyView({ os, agent: s.agent, computers: pickedNames() })} agent={s.agent} place={placeName(os)} onDone={() => go("next")} onContinueChat={(task) => void continueChat(task)} />
      ) : null;
      break;
    case "next": {
      const granted = (id: string) => Boolean(host?.permissions.find((p) => p.id === id)?.granted);
      body = (
        <NextScreen
          view={nextView({ os })}
          onRow={(id) => {
            if (id === "invite") open(`${accountUrl}/team`);
            else if (id === "add-computer") setAddOpen(true);
            else setAppsOpen((o) => !o);
          }}
          onDone={() => void finish()}
          apps={
            appsOpen ? (
              <AppsPermissions
                rows={PERMISSIONS.map((p) => ({ ...p, granted: granted(p.id), url: host?.permissions.find((x) => x.id === p.id)?.settingsUrl ?? p.url }))}
                onAllow={(url) => void adapter?.call("host.openSettings", { url }).catch(() => {})}
              />
            ) : null
          }
        />
      );
      break;
    }
  }

  return (
    <div className="dark relative flex h-full flex-col overflow-hidden bg-[#050607] text-white" data-new-first-run="" data-step={s.screen}>
      <FloatKeys copyRight={544} />
      <header className="relative z-30 flex shrink-0 items-center justify-between pt-[calc(var(--titlebar-height,0px)+12px)] pr-16 pb-2 pl-16">
        <span className="flex items-center gap-1.5">
          <CuaLogo className="size-5" />
          <span className="font-display text-[19px] font-medium tracking-[-0.01em] text-white">Cua</span>
        </span>
        {showSkip ? (
          <button type="button" className={cn(GLASS_CHIP, "h-9 px-4 text-[14px]")} onClick={skip} data-onboarding-later="">
            Skip for now
          </button>
        ) : null}
      </header>
      <main className="relative z-20 flex min-h-0 flex-1 flex-col overflow-y-auto px-16">
        <div aria-hidden className="min-h-0 shrink-0 grow basis-0" />
        <div className="flex w-full max-w-[600px] shrink-0 flex-col py-2">{body}</div>
        <div aria-hidden className="min-h-2 shrink-0 grow-[1.4] basis-0" />
      </main>
      {dot !== null ? (
        <ol className="relative z-20 flex shrink-0 gap-2 pt-2 pb-6 pl-16" aria-label={`Step ${dot + 1} of 3`}>
          {[0, 1, 2].map((i) => (
            <li key={i} aria-current={i === dot ? "step" : undefined} className={cn("h-1 w-7 rounded-full", i === dot ? "bg-white" : "bg-white/30")} />
          ))}
        </ol>
      ) : (
        <div className="h-8 shrink-0" />
      )}
      <AddComputerSheet open={addOpen} onOpenChange={setAddOpen} />
    </div>
  );
}

type NewRunScreenName = NewRunSaved["screen"];
