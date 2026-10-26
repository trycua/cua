// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { ArrowUpIcon, CheckIcon, ChevronDownIcon, LaptopIcon, MonitorIcon } from "lucide-react";
import { useEffect, useRef, type ReactElement, type ReactNode } from "react";

import type { Space, SpaceOs } from "@/bridge";
import { AppMarkImage } from "@/components/agents/harness-mark";
import { OsIcon } from "@/components/os-icon";
import type { NewKind, TaskWhere } from "@/lib/ask-first";
import { cn } from "@/lib/utils";

export type { TaskWhere } from "@/lib/ask-first";

/** The coding apps a task can run with (`agent_start`'s harnesses), in the picker's order. */
export const TASK_AGENTS: { id: string; name: string; setupId: string }[] = [
  { id: "claude-code", name: "Claude Code", setupId: "claude-code" },
  { id: "openai-codex", name: "Codex", setupId: "codex" },
  { id: "gemini-cli", name: "Gemini CLI", setupId: "gemini-cli" },
  { id: "opencode", name: "OpenCode", setupId: "opencode" },
  { id: "goose", name: "Goose", setupId: "goose" },
  { id: "hermes", name: "Hermes", setupId: "hermes" },
];

export const agentName = (id: string) => TASK_AGENTS.find((a) => a.id === id)?.name ?? id;

const OS_WORD: Record<SpaceOs, string> = { linux: "Linux", macos: "Mac", windows: "Windows", unknown: "" };

export function whereLabel(where: TaskWhere): string {
  return where.kind === "new" ? `A new ${OS_WORD[where.os]} computer · this Mac` : where.space.name;
}

export function AppMark({ agent, size = 32 }: { agent: string; size?: number }) {
  return (
    <span className="flex shrink-0 items-center justify-center rounded-lg border bg-card dark:bg-background" style={{ width: size, height: size }}>
      <AppMarkImage agent={agent} className={size >= 40 ? "size-6" : size >= 28 ? "size-5" : "size-3.5"} />
    </span>
  );
}

/** Why this Mac itself isn't in the list. */
export const WHERE_FOOTER = "Agents don't work on this Mac itself. Each job gets its own computer, so yours stays untouched.";

function Picker({ trigger, title, children, footer }: { trigger: ReactElement; title: string; children: ReactNode; footer?: ReactNode }) {
  return (
    <MenuPrimitive.Root>
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

const pickerButton = "flex h-12 items-center gap-2.5 rounded-lg px-3 text-[14px] outline-none hover:bg-foreground/5 data-popup-open:bg-foreground/[0.07]";

/**
 * Home's box: the job in your own words, who does it and where it runs.
 * Enter starts it (Shift+Enter for a new line).
 */
export function AskBox({
  text,
  onText,
  agent,
  onAgent,
  installed,
  mostUsed,
  where,
  onWhere,
  kinds,
  spaces,
  busy,
  onSubmit,
  compact,
  autoFocus,
}: {
  text: string;
  onText: (t: string) => void;
  agent: string;
  onAgent: (a: string) => void;
  /** Harness ids found on this computer. */
  installed: ReadonlySet<string>;
  mostUsed: string | null;
  where: TaskWhere;
  onWhere: (w: TaskWhere) => void;
  /** The kinds of new computer this Mac can make, with a line each ("about 2 min"). */
  kinds: NewKind[];
  /** Running Spaces to use instead (never this Mac's own desktop: `pickableSpaces`). */
  spaces: Space[];
  busy: boolean;
  onSubmit: () => void;
  compact?: boolean;
  autoFocus?: boolean;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);
  const ready = text.trim().length > 0 && !busy;
  const agentNote = (id: string) => (id === mostUsed ? "You use it most" : installed.has(id) ? "On this Mac" : "Runs on the new computer with your saved key");
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
        aria-label="What do you want to do?"
        placeholder="Describe the job in your own words, or paste a link to an issue or pull request"
        className={cn("block w-full resize-none bg-transparent px-5 pt-4 text-[16px] leading-relaxed outline-none placeholder:text-muted-foreground", compact ? "min-h-14" : "min-h-24")}
      />
      <div className="flex items-center gap-1 px-2 pt-1 pb-2">
        <Picker
          title="Who does it"
          trigger={
            <button type="button" className={pickerButton} data-picker="who">
              <AppMark agent={agent} size={24} />
              <span>{agentName(agent)}</span>
              <ChevronDownIcon className="size-4 text-muted-foreground" />
            </button>
          }
          footer="It works on the new computer, signed in with the key you saved in Settings → Connected apps."
        >
          {TASK_AGENTS.map((a) => (
            <PickerRow key={a.id} icon={<AppMark agent={a.id} size={28} />} title={a.name} note={agentNote(a.id)} selected={a.id === agent} onSelect={() => onAgent(a.id)} />
          ))}
        </Picker>
        <span className="px-1 text-[14px] text-muted-foreground">on</span>
        <WherePicker
          where={where}
          onWhere={onWhere}
          kinds={kinds}
          spaces={spaces}
          trigger={
            <button type="button" className={pickerButton} data-picker="where">
              {where.kind === "new" ? <OsIcon os={where.os} className="size-4 text-muted-foreground" /> : <MonitorIcon className="size-4 text-muted-foreground" />}
              <span className="max-w-72 truncate">{whereLabel(where)}</span>
              <ChevronDownIcon className="size-4 text-muted-foreground" />
            </button>
          }
        />
        <div className="flex-1" />
        <button
          type="button"
          disabled={!ready}
          onClick={onSubmit}
          aria-label="Start"
          className={cn("flex size-12 items-center justify-center rounded-full transition-colors", ready ? "bg-primary text-primary-foreground hover:bg-primary/90" : "bg-foreground/10 text-muted-foreground")}
        >
          <ArrowUpIcon className="size-5" />
        </button>
      </div>
    </div>
  );
}

/**
 * "Where it runs": the kinds of new computer this Mac can make, then the
 * running Spaces you have. This Mac's own desktop is never offered (the
 * footer says why): `agent_start` runs only in a Space.
 */
export function WherePicker({
  where,
  onWhere,
  kinds,
  spaces,
  trigger,
}: {
  where: TaskWhere | null;
  onWhere: (w: TaskWhere) => void;
  kinds: NewKind[];
  spaces: Space[];
  trigger: ReactElement;
}) {
  return (
    <Picker title="Where it runs" trigger={trigger} footer={WHERE_FOOTER}>
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
      {spaces.length ? <p className="px-3 pt-3 pb-1 text-[12px] font-medium text-muted-foreground">Or use a computer you already have</p> : null}
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
