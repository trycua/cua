// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { ArrowUpIcon, CheckIcon, ChevronDownIcon, EllipsisIcon, LaptopIcon, LoaderCircleIcon, MonitorIcon, SparklesIcon, XIcon } from "lucide-react";
import { useEffect, useRef, useState, type ReactElement, type ReactNode } from "react";

import type { AgentAccount, AgentGroup, AgentsView, Space, SpaceOs } from "@/bridge";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { OsIcon } from "@/components/os-icon";
import type { NewKind, TaskWhere } from "@/lib/ask-first";
import type { AgentSelection } from "@/lib/ask-first-model";
import type { AskRepo } from "@/lib/ask-first-repo";
import { useT, t } from "@/i18n";
import { cn } from "@/lib/utils";

import { AgentPicker } from "./agent-picker";
import { AtPicker, atPlaces, matchPlaces, type AtPlace, type NameableMachine } from "./at-picker";
import { activeMention } from "@/lib/mentions";
import { RepoChip } from "./repo-chip";
import { thisComputerLabel, thisComputerText } from "@/lib/host-labels";
import { thisMacChipText, type ThisMacReadiness } from "@/lib/this-mac-target";

export type { TaskWhere } from "@/lib/ask-first";

export { agentName, TASK_AGENTS } from "./task-agents";

const OS_WORD: Record<SpaceOs, string> = { linux: "Linux", macos: "Mac", windows: "Windows", unknown: "" };

/** Where it runs is a Space: "New Space", then quieter "Linux on This Mac". */
export function whereSpaceLabel(where: TaskWhere): { title: string; detail: string | null } {
  if (where.kind === "agent") return { title: where.reuse ? t("home.ask_box.agent_decides_same_home") : t("home.ask_box.agent_decides"), detail: null };
  if (where.kind === "machine") return { title: where.machine.name, detail: null };
  if (where.kind === "new")
    return {
      title: t("home.ask_box.new_space"),
      detail: t("home.ask_box.new_space_detail", { os: OS_WORD[where.os], computer: where.on?.name ?? thisComputerLabel() }),
    };
  return { title: where.kind === "this-mac" ? thisComputerLabel() : where.space.name, detail: null };
}

export function AppMark({ agent, size = 32 }: { agent: string; size?: number }) {
  return (
    <span className="flex shrink-0 items-center justify-center rounded-lg border bg-card dark:bg-background" style={{ width: size, height: size }}>
      <AppMarkImage agent={agent} className={size >= 40 ? "size-6" : size >= 28 ? "size-5" : "size-3.5"} />
    </span>
  );
}

/** A new Space's row: just its system ("Linux", "macOS"); the section label says it's new, on this computer. */
const NEW_KIND_WORD: Record<SpaceOs, string> = { linux: "Linux", macos: "macOS", windows: "Windows", unknown: "" };

export type { NameableMachine };

function Picker({
  trigger,
  title,
  children,
  footer,
  open,
  onOpenChange,
}: {
  trigger: ReactElement;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  return (
    <MenuPrimitive.Root {...(open === undefined ? {} : { open })} onOpenChange={(o) => onOpenChange?.(o)}>
      <MenuPrimitive.Trigger render={trigger} />
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={6} align="start" className="z-50 outline-none">
          <MenuPrimitive.Popup className="glass w-[420px] origin-(--transform-origin) rounded-xl border p-2 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0">
            <p className="px-3 pt-2 pb-1 text-[12px] font-medium text-muted-foreground">{title}</p>
            {children}
            {footer ? <div className="mt-1 border-t px-3 pt-2 pb-1 text-[12px] text-muted-foreground">{footer}</div> : null}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}

function PickerRow({ icon, title, note, selected, onSelect }: { icon: ReactNode; title: string; note?: string; selected: boolean; onSelect: () => void }) {
  return (
    <MenuPrimitive.Item
      onClick={onSelect}
      className={cn(
        "flex min-h-12 cursor-default items-center gap-3 rounded-lg px-3 py-2 outline-none data-highlighted:bg-foreground/[0.06]",
        selected && "bg-foreground/[0.06]",
      )}
    >
      {icon}
      <div className="min-w-0 flex-1">
        <div className="text-[14px] font-medium">{title}</div>
        {note ? <div className="text-[12px] text-muted-foreground">{note}</div> : null}
      </div>
      {selected ? <CheckIcon className="size-4" /> : null}
    </MenuPrimitive.Item>
  );
}

/** Where it runs, as a chip like the repo's: the place, then a switcher for your other computers and Spaces. */
const whereChip = "flex h-9 min-w-0 items-center rounded-lg bg-foreground/[0.06]";
const SectionLabel = ({ children }: { children: ReactNode }) => <p className="px-3 pt-3 pb-1 text-[12px] font-medium text-muted-foreground">{children}</p>;

const whereButton =
  "flex h-9 min-w-0 items-center gap-2 rounded-lg px-2.5 text-[13px] outline-none hover:bg-foreground/[0.05] focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07]";

/**
 * Home's box, like the first prompt of a chat app: the job in your own words,
 * then one row under it with the agent (the app and its account), an optional repo and, without an agent home, where it runs.
 * Enter starts it (Shift+Enter for a new line). The model is a line in the agent menu.
 */
export function AskBox({
  text,
  onText,
  view,
  selected,
  onPickAccount,
  onModel,
  onEffort,
  onSignIn,
  onConfigure,
  repo,
  onRepo,
  where,
  onWhere,
  whereOpen,
  onWhereOpen,
  kinds,
  spaces,
  computers = [],
  machines,
  thisMac = null,
  agentHome,
  hasHome,
  busy,
  onSubmit,
  compact,
  autoFocus,
  pickerSide = "down",
}: {
  text: string;
  onText: (t: string) => void;
  /** The coding apps and the accounts for each (`connectors.agents`); null before they are read. */
  view: AgentsView | null;
  /** The app, account and model it will use. */
  selected: AgentSelection | null;
  onPickAccount: (group: AgentGroup, account: AgentAccount) => void;
  onModel: (model: string) => void;
  onEffort?: (level: string) => void;
  /** Starts the sign-in of an app that has no account. */
  onSignIn: (group: AgentGroup) => void;
  /** Opens Settings, Connections, where the agents and their sign-ins are managed. */
  onConfigure?: () => void;
  repo: AskRepo | null;
  onRepo?: (r: AskRepo | null) => void;
  where: TaskWhere;
  onWhere: (w: TaskWhere) => void;
  /** The Where picker held open (it reopens when "Use this Mac?" is declined). */
  whereOpen?: boolean;
  onWhereOpen?: (open: boolean) => void;
  /** The kinds of new computer this Mac can make, with a line each ("about 2 min"). */
  kinds: NewKind[];
  /** Running Spaces to use instead (never this Mac's own desktop: `pickableSpaces`). */
  spaces: Space[];
  /** Your other computers that are online and share their desktop (`otherComputerDesktops`). */
  computers?: Space[];
  /** How this computer stands as a place to run, and its one Set up action (`useThisMacTarget`). */
  thisMac?: { readiness: ThisMacReadiness; busy: boolean; onSetUp: () => void } | null;
  /** With agent homes (Child Spaces on): the computers the person can name, this one first. */
  machines?: NameableMachine[];
  /** The job goes to the agent's Home Space, which makes the Spaces it needs: no where picker until a place is named. */
  agentHome?: boolean;
  /** The person already has a Home Space: the picker offers to continue in it. */
  hasHome?: boolean;
  busy: boolean;
  onSubmit: () => void;
  compact?: boolean;
  autoFocus?: boolean;
  /** Which way the `@` list opens: up for a box at the bottom of a page. */
  pickerSide?: "up" | "down";
}) {
  const t = useT();
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);
  // This computer picked but not set up yet: the chip says so, with Set up; the job can't start on it until then.
  const needsSetUp = where.kind === "this-mac" && thisMac?.readiness === "needs-setup";
  const ready = text.trim().length > 0 && !busy && selected !== null && !needsSetUp;
  const chip = thisMacChipText(thisComputerLabel());
  const label = whereSpaceLabel(where);
  // With agent homes the agent decides where, so nothing about it shows until the person names a place: with @ in the
  // text, or by picking one (the "..." button then shows).
  const placed = where.kind !== "agent" || Boolean(where.reuse);
  const naming = placed || /(^|\s)@/.test(text);
  // `@` opens your computers: Esc closes, arrows move, Enter or a click picks, and the pick is the where chip
  // (the same choice the picker on the chip makes), not text.
  const [caret, setCaret] = useState(0);
  const [index, setIndex] = useState(0);
  const [closedAt, setClosedAt] = useState<number | null>(null);
  const places = atPlaces({ agentHome, machines, computers });
  const mention = activeMention(text, caret);
  const shown = mention && closedAt !== mention.start ? matchPlaces(mention.query, places) : null;
  const open = shown !== null;
  const at = Math.min(index, Math.max(0, (shown?.length ?? 1) - 1));
  const pickPlace = (p: AtPlace) => {
    if (!mention || !p.online) return;
    const after = text.slice(caret).replace(/^[\w.-]*/, "");
    const next = (text.slice(0, mention.start) + after.replace(/^ /, "")).replace(/^\s+/, "");
    onText(next);
    onWhere(p.where);
    setCaret(Math.max(0, mention.start));
    setIndex(0);
    ref.current?.focus();
  };
  const chosen =
    agentHome && placed
      ? places.find((p) =>
          where.kind === "machine" ? p.where.kind === "machine" && p.where.machine.id === where.machine.id : where.kind === "this-mac" && p.current,
        )
      : undefined;
  return (
    <div data-ask-box="" className="relative rounded-2xl border bg-card shadow-xs dark:bg-white/[0.03]">
      {open ? <AtPicker places={shown} index={at} onIndex={setIndex} onPick={pickPlace} side={pickerSide} filtered={places.length > 0} /> : null}
      <textarea
        ref={ref}
        value={text}
        onChange={(e) => {
          onText(e.target.value);
          setCaret(e.target.selectionStart ?? e.target.value.length);
          setIndex(0);
        }}
        onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
        onKeyDown={(e) => {
          if (shown && mention && !e.nativeEvent.isComposing) {
            const n = shown.length;
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              if (n) setIndex((at + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
              return;
            }
            if (e.key === "Escape") {
              e.preventDefault();
              setClosedAt(mention.start);
              return;
            }
            if ((e.key === "Enter" || e.key === "Tab") && !e.shiftKey) {
              e.preventDefault();
              if (shown[at]) pickPlace(shown[at]);
              return;
            }
          }
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            if (ready) onSubmit();
          }
        }}
        rows={compact ? 1 : 3}
        aria-label={t("home.ask_box.label")}
        placeholder={agentHome ? t("home.ask_box.placeholder_home") : t("home.ask_box.label")}
        className={cn(
          "block w-full resize-none bg-transparent px-5 pt-4 text-[16px] leading-relaxed outline-none placeholder:text-muted-foreground",
          compact ? "min-h-14" : "min-h-24",
        )}
      />
      <div className="flex items-center gap-1 px-2 pt-1 pb-2">
        <AgentPicker
          view={view}
          selected={selected}
          onPick={onPickAccount}
          onModel={onModel}
          onEffort={onEffort}
          onSignIn={onSignIn}
          onConfigure={onConfigure}
        />
        <RepoChip repo={repo} onRepo={onRepo} />
        {placed && agentHome ? (
          <span className={cn(whereChip, "gap-1.5 pr-0.5 pl-2.5 text-[13px]")} data-ask-where-chip="at">
            {chosen ? <OsIcon os={chosen.os} className="size-3.5 text-muted-foreground" /> : null}
            <span className="max-w-64 truncate" data-ask-where="">
              {t("home.ask_box.in_place", { place: label.title })}
            </span>
            <button
              type="button"
              data-ask-where-clear=""
              aria-label={t("home.ask_box.at_clear", { place: label.title })}
              onClick={() => onWhere({ kind: "agent" })}
              className="flex size-7 items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
            >
              <XIcon className="size-3.5" />
            </button>
          </span>
        ) : null}
        {agentHome ? null : (
          <span className={whereChip} data-ask-where-chip={needsSetUp ? "needs-setup" : where.kind}>
            <WherePicker
              where={where}
              onWhere={onWhere}
              open={whereOpen}
              onOpenChange={onWhereOpen}
              kinds={kinds}
              spaces={spaces}
              computers={computers}
              trigger={
                <button
                  type="button"
                  className={whereButton}
                  data-picker="where"
                  aria-label={
                    needsSetUp
                      ? t("home.ask_box.where_label_note", { place: label.title, note: chip.note })
                      : t("home.ask_box.where_label", { place: label.title })
                  }
                >
                  {where.kind === "new" ? (
                    <OsIcon os={where.os} className="size-3.5" />
                  ) : where.kind === "this-mac" ? (
                    <LaptopIcon className="size-3.5" />
                  ) : (
                    <MonitorIcon className="size-3.5" />
                  )}
                  <span className="max-w-64 truncate" data-ask-where="">
                    {label.title}
                    {label.detail ? <span className="ml-1.5 text-muted-foreground">{label.detail}</span> : null}
                    {needsSetUp ? (
                      <span className="ml-1.5 text-muted-foreground" data-ask-where-note="">
                        {chip.note}
                      </span>
                    ) : null}
                  </span>
                  <ChevronDownIcon className="size-3.5 shrink-0 text-muted-foreground" />
                </button>
              }
            />
            {needsSetUp && thisMac ? (
              <button
                type="button"
                onClick={thisMac.onSetUp}
                disabled={thisMac.busy}
                aria-label={chip.actionLabel}
                className="mr-1 flex h-7 shrink-0 items-center gap-1.5 rounded-lg bg-background px-2.5 text-[12.5px] font-medium shadow-xs outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-70 dark:bg-white/10 dark:hover:bg-white/15"
                data-ask-where-setup=""
              >
                {thisMac.busy ? <LoaderCircleIcon aria-hidden className="size-3.5 animate-spin motion-reduce:animate-none" /> : null}
                {thisMac.busy ? chip.busy : chip.action}
              </button>
            ) : null}
          </span>
        )}
        <div className="flex-1" />
        {agentHome && naming ? (
          <WherePicker
            where={where}
            onWhere={onWhere}
            open={whereOpen}
            onOpenChange={onWhereOpen}
            kinds={kinds}
            spaces={spaces}
            machines={machines}
            agentHome
            hasHome={hasHome}
            trigger={
              <button
                type="button"
                data-picker="where"
                aria-label={t("home.ask_box.where")}
                className="flex size-9 items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-foreground/5 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07]"
              >
                <EllipsisIcon className="size-4" />
              </button>
            }
          />
        ) : null}
        <button
          type="button"
          disabled={!ready}
          onClick={onSubmit}
          aria-label={t("home.ask_box.start")}
          className={cn(
            "flex size-10 items-center justify-center rounded-full transition-colors",
            ready ? "bg-primary text-primary-foreground hover:bg-primary/90" : "bg-foreground/10 text-muted-foreground",
          )}
        >
          <ArrowUpIcon className="size-5" />
        </button>
      </div>
    </div>
  );
}

/**
 * "Where it runs": the kinds of new computer this Mac can make, then this
 * Mac itself and the running Spaces you have. Picking this Mac asks once
 * (`useWherePick`).
 */
export function WherePicker({
  where,
  onWhere,
  kinds,
  spaces,
  computers = [],
  machines,
  agentHome,
  hasHome,
  trigger,
  open,
  onOpenChange,
}: {
  where: TaskWhere | null;
  onWhere: (w: TaskWhere) => void;
  kinds: NewKind[];
  spaces: Space[];
  /** Your other computers that are online and share their desktop (without agent homes, which name `machines`). */
  computers?: Space[];
  /** With agent homes: the computers the person can name (other than this one, which has its own row). */
  machines?: NameableMachine[];
  /** Leads with "Let the agent decide". */
  agentHome?: boolean;
  /** The person already has a Home Space: offers to continue in it (a request starts a new Home otherwise). */
  hasHome?: boolean;
  trigger: ReactElement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const t = useT();
  const newRows = kinds.map((k) => (
    <PickerRow
      key={k.os}
      icon={
        <span className="flex size-7 items-center justify-center rounded-md border text-muted-foreground">
          <OsIcon os={k.os} className="size-4" />
        </span>
      }
      title={NEW_KIND_WORD[k.os]}
      note={k.blocked ?? k.note}
      selected={where?.kind === "new" && where.os === k.os}
      onSelect={() => onWhere({ kind: "new", os: k.os })}
    />
  ));
  const newSpaceLabel = t("home.ask_box.new_space_on", { computer: thisComputerText() });
  const thisRow = (
    <PickerRow
      icon={
        <span className="flex size-7 items-center justify-center rounded-md border text-muted-foreground">
          <LaptopIcon className="size-4" />
        </span>
      }
      title={thisComputerLabel()}
      note={t("home.ask_box.this_computer_note")}
      selected={where?.kind === "this-mac"}
      onSelect={() => onWhere({ kind: "this-mac" })}
    />
  );
  const spaceRows = spaces
    .slice(0, 5)
    .map((s) => (
      <PickerRow
        key={s.id}
        icon={
          <span className="flex size-7 items-center justify-center rounded-md border text-muted-foreground">
            {s.provider === "local" ? <LaptopIcon className="size-4" /> : <OsIcon os={s.os} className="size-4" />}
          </span>
        }
        title={s.name}
        note={t("home.ask_box.on_now")}
        selected={where?.kind === "space" && where.space.id === s.id}
        onSelect={() => onWhere({ kind: "space", space: s })}
      />
    ));
  return (
    <Picker title={t("home.ask_box.where")} trigger={trigger} open={open} onOpenChange={onOpenChange}>
      {agentHome ? (
        <PickerRow
          icon={
            <span className="flex size-7 items-center justify-center rounded-lg bg-muted text-muted-foreground">
              <SparklesIcon className="size-4" />
            </span>
          }
          title={t("home.ask_box.agent_decides")}
          selected={where?.kind === "agent" && !where.reuse}
          onSelect={() => onWhere({ kind: "agent" })}
        />
      ) : null}
      {agentHome && hasHome ? (
        <PickerRow
          icon={
            <span className="flex size-7 items-center justify-center rounded-lg bg-muted text-muted-foreground">
              <SparklesIcon className="size-4" />
            </span>
          }
          title={t("home.ask_box.agent_decides_same_home")}
          selected={where?.kind === "agent" && Boolean(where.reuse)}
          onSelect={() => onWhere({ kind: "agent", reuse: true })}
        />
      ) : null}
      {agentHome ? (
        <>
          <SectionLabel>{newSpaceLabel}</SectionLabel>
          {newRows}
          <SectionLabel>{t("home.ask_box.your_computers")}</SectionLabel>
          {thisRow}
          {machines
            ?.filter((m) => !m.current)
            .slice(0, 5)
            .map((m) => (
              <PickerRow
                key={m.id}
                icon={
                  <span className="flex size-7 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                    <LaptopIcon className="size-4" />
                  </span>
                }
                title={m.name}
                note={m.online ? t("home.ask_box.online") : t("home.ask_box.off")}
                selected={where?.kind === "machine" && where.machine.id === m.id}
                onSelect={() => onWhere({ kind: "machine", machine: { id: m.id, name: m.name } })}
              />
            ))}
          {spaceRows}
        </>
      ) : (
        <>
          {/* This computer already counts: your computers first, then your Spaces; a new computer is optional. */}
          <SectionLabel>{t("home.ask_box.your_computers")}</SectionLabel>
          {thisRow}
          {computers.slice(0, 5).map((c) => (
            <PickerRow
              key={c.id}
              icon={
                <span className="flex size-7 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                  <OsIcon os={c.os} className="size-4" />
                </span>
              }
              title={c.name}
              note={t("home.ask_box.online")}
              selected={where?.kind === "space" && where.space.id === c.id}
              onSelect={() => onWhere({ kind: "space", space: c })}
            />
          ))}
          {spaces.length > 0 ? <SectionLabel>{t("home.ask_box.your_spaces")}</SectionLabel> : null}
          {spaceRows}
          <SectionLabel>{newSpaceLabel}</SectionLabel>
          {newRows}
        </>
      )}
    </Picker>
  );
}
