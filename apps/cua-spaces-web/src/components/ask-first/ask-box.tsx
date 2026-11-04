// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { ArrowUpIcon, CheckIcon, ChevronDownIcon, EllipsisIcon, LaptopIcon, MonitorIcon, SparklesIcon } from "lucide-react";
import { useEffect, useRef, type ReactElement, type ReactNode } from "react";

import type { ModelsView, Space, SpaceOs } from "@/bridge";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { OsIcon } from "@/components/os-icon";
import type { NewKind, TaskWhere } from "@/lib/ask-first";
import type { ModelSelection, PickedModel } from "@/lib/ask-first-model";
import type { AskRepo } from "@/lib/ask-first-repo";
import { cn } from "@/lib/utils";

import { ModelPicker } from "./model-picker";
import { RepoChip } from "./repo-chip";

export type { TaskWhere } from "@/lib/ask-first";

export { agentName, TASK_AGENTS } from "./task-agents";

const OS_WORD: Record<SpaceOs, string> = { linux: "Linux", macos: "Mac", windows: "Windows", unknown: "" };

/** Where it runs is a Space: "New Space", then quieter "Linux on This Mac". */
export function whereSpaceLabel(where: TaskWhere): { title: string; detail: string | null } {
  if (where.kind === "agent") return { title: where.fresh ? "Agent decides, new Home" : "Agent decides", detail: null };
  if (where.kind === "machine") return { title: where.machine.name, detail: null };
  if (where.kind === "new") return { title: "New Space", detail: `${OS_WORD[where.os]} on This Mac` };
  return { title: where.kind === "this-mac" ? "This Mac" : where.space.name, detail: null };
}

export function AppMark({ agent, size = 32 }: { agent: string; size?: number }) {
  return (
    <span className="flex shrink-0 items-center justify-center rounded-lg border bg-card dark:bg-background" style={{ width: size, height: size }}>
      <AppMarkImage agent={agent} className={size >= 40 ? "size-6" : size >= 28 ? "size-5" : "size-3.5"} />
    </span>
  );
}

/** The trade-off, under the list. */
export const WHERE_FOOTER = "A new computer keeps yours untouched. This Mac works on your own files and apps.";

/** With agent homes: what the footer adds (the agent can also be told in the text with @). */
export const WHERE_FOOTER_HOME = "You can also name a Space or computer in the text with @.";

/** A computer the person can name for the agent to use (agent homes). */
export interface NameableMachine {
  id: string;
  name: string;
  current: boolean;
  online: boolean;
}

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

function PickerRow({ icon, title, note, selected, onSelect }: { icon: ReactNode; title: string; note: string; selected: boolean; onSelect: () => void }) {
  return (
    <MenuPrimitive.Item
      onClick={onSelect}
      className={cn("flex min-h-12 cursor-default items-center gap-3 rounded-lg px-3 py-2 outline-none data-highlighted:bg-foreground/[0.06]", selected && "bg-foreground/[0.06]")}
    >
      {icon}
      <div className="min-w-0 flex-1">
        <div className="text-[14px] font-medium">{title}</div>
        <div className="text-[12px] text-muted-foreground">{note}</div>
      </div>
      {selected ? <CheckIcon className="size-4" /> : null}
    </MenuPrimitive.Item>
  );
}

const whereButton = "flex h-9 items-center gap-2 rounded-lg px-2.5 text-[13px] outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07]";

/**
 * Home's box, like the first prompt of a chat app: the job in your own words,
 * then one row under it with the model, an optional repo and where it runs.
 * Enter starts it (Shift+Enter for a new line). Which coding app runs the job
 * follows the model.
 */
export function AskBox({
  text,
  onText,
  view,
  selected,
  onPickModel,
  onConnect,
  repo,
  onRepo,
  where,
  onWhere,
  whereOpen,
  onWhereOpen,
  kinds,
  spaces,
  machines,
  agentHome,
  hasHome,
  busy,
  onSubmit,
  compact,
  autoFocus,
}: {
  text: string;
  onText: (t: string) => void;
  /** The connected models (`connectors.models`); null before they are read. */
  view: ModelsView | null;
  /** The model it will use. */
  selected: ModelSelection | null;
  onPickModel: (p: PickedModel) => void;
  /** Opens Connections (to connect a provider, or reconnect the one named). */
  onConnect: (provider?: string) => void;
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
  /** With agent homes (Child Spaces on): the computers the person can name, this one first. */
  machines?: NameableMachine[];
  /** The job goes to the agent's Home Space, which makes the Spaces it needs: no where picker until a place is named. */
  agentHome?: boolean;
  /** The person already has a Home Space: the picker offers a fresh one. */
  hasHome?: boolean;
  busy: boolean;
  onSubmit: () => void;
  compact?: boolean;
  autoFocus?: boolean;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);
  const ready = text.trim().length > 0 && !busy && selected !== null;
  const label = whereSpaceLabel(where);
  // With agent homes the agent decides where, so nothing about it shows until the person names a place: with @ in the
  // text, or by picking one (the "..." button then shows).
  const placed = where.kind !== "agent" || Boolean(where.fresh);
  const naming = placed || /(^|\s)@/.test(text);
  return (
    <div data-ask-box="" className="rounded-2xl border bg-card shadow-xs dark:bg-white/[0.03]">
      <textarea
        ref={ref}
        value={text}
        onChange={(e) => onText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            if (ready) onSubmit();
          }
        }}
        rows={compact ? 1 : 3}
        aria-label="What should it do?"
        placeholder="What should it do?"
        className={cn("block w-full resize-none bg-transparent px-5 pt-4 text-[16px] leading-relaxed outline-none placeholder:text-muted-foreground", compact ? "min-h-14" : "min-h-24")}
      />
      <div className="flex items-center gap-1 px-2 pt-1 pb-2">
        <ModelPicker view={view} selected={selected} onPick={onPickModel} onConnect={onConnect} />
        <RepoChip repo={repo} onRepo={onRepo} />
        {placed && agentHome ? (
          <span data-ask-where="" className="max-w-64 truncate px-1.5 text-[13px] text-muted-foreground">
            in {label.title}
          </span>
        ) : null}
        {agentHome ? null : (
          <WherePicker
            where={where}
            onWhere={onWhere}
            open={whereOpen}
            onOpenChange={onWhereOpen}
            kinds={kinds}
            spaces={spaces}
            trigger={
              <button type="button" className={whereButton} data-picker="where">
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
                </span>
                <ChevronDownIcon className="size-3.5 text-muted-foreground" />
              </button>
            }
          />
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
                aria-label="Where it runs"
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
          aria-label="Start"
          className={cn("flex size-10 items-center justify-center rounded-full transition-colors", ready ? "bg-primary text-primary-foreground hover:bg-primary/90" : "bg-foreground/10 text-muted-foreground")}
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
  /** With agent homes: the computers the person can name (other than this one, which has its own row). */
  machines?: NameableMachine[];
  /** Leads with "Let the agent decide". */
  agentHome?: boolean;
  /** The person already has a Home Space (the next request reuses it): offers a fresh one. */
  hasHome?: boolean;
  trigger: ReactElement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  return (
    <Picker title="Where it runs" trigger={trigger} footer={agentHome ? `${WHERE_FOOTER} ${WHERE_FOOTER_HOME}` : WHERE_FOOTER} open={open} onOpenChange={onOpenChange}>
      {agentHome ? (
        <PickerRow
          icon={
            <span className="flex size-7 items-center justify-center rounded-lg bg-muted text-muted-foreground">
              <SparklesIcon className="size-4" />
            </span>
          }
          title="Let the agent decide"
          note="It picks what the job needs, within your budget"
          selected={where?.kind === "agent" && !where.fresh}
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
          title="Let the agent decide, in a new Home"
          note="Starts a fresh Home Space instead of reusing yours"
          selected={where?.kind === "agent" && Boolean(where.fresh)}
          onSelect={() => onWhere({ kind: "agent", fresh: true })}
        />
      ) : null}
      {kinds.map((k) => (
        <PickerRow
          key={k.os}
          icon={
            <span className="flex size-7 items-center justify-center rounded-md border text-muted-foreground">
              <OsIcon os={k.os} className="size-4" />
            </span>
          }
          title={`A new ${OS_WORD[k.os]} computer on this Mac`}
          note={k.blocked ?? k.note}
          selected={where?.kind === "new" && where.os === k.os}
          onSelect={() => onWhere({ kind: "new", os: k.os })}
        />
      ))}
      <p className="px-3 pt-3 pb-1 text-[12px] font-medium text-muted-foreground">Or use a computer you already have</p>
      <PickerRow
        icon={
          <span className="flex size-7 items-center justify-center rounded-md border text-muted-foreground">
            <LaptopIcon className="size-4" />
          </span>
        }
        title="This Mac"
        note="Works on your own files and apps"
        selected={where?.kind === "this-mac"}
        onSelect={() => onWhere({ kind: "this-mac" })}
      />
      {agentHome && machines?.some((m) => !m.current) ? <p className="px-3 pt-3 pb-1 text-[12px] font-medium text-muted-foreground">Your computers</p> : null}
      {agentHome
        ? machines
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
                note={m.online ? "Online" : "Off"}
                selected={where?.kind === "machine" && where.machine.id === m.id}
                onSelect={() => onWhere({ kind: "machine", machine: { id: m.id, name: m.name } })}
              />
            ))
        : null}
      {spaces.slice(0, 5).map((s) => (
        <PickerRow
          key={s.id}
          icon={
            <span className="flex size-7 items-center justify-center rounded-md border text-muted-foreground">
              {s.provider === "local" ? <LaptopIcon className="size-4" /> : <OsIcon os={s.os} className="size-4" />}
            </span>
          }
          title={s.name}
          note="On now"
          selected={where?.kind === "space" && where.space.id === s.id}
          onSelect={() => onWhere({ kind: "space", space: s })}
        />
      ))}
    </Picker>
  );
}
