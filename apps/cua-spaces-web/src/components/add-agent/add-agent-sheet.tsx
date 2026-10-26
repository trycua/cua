// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Add an agent" (CUA-2264): two rows. Start an agent here runs one inside
 * the Space (`agent_add`; it sleeps until someone @mentions it). Connect
 * your own agent gives one line to paste into the agent you already use:
 * `Join my Cua Space "<name>": https://cua.ai/join/<code>`. Its code works
 * once, for this Space only, and expires; the sheet waits for the agent and
 * closes by itself when it joins, and offers a new code when it expires.
 *
 * Reusable: "+" in In this Space opens it for that Space; the Agents tab's
 * New agent opens it with `spaces`, and the title becomes a Space picker.
 */

import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, LinkIcon, Loader2Icon, PlayIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { threadOpAvailable, useBridge, type ThreadRole } from "@/bridge";
import { HarnessMark, HARNESSES } from "@/components/space-thread/harness-marks";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { PromptBox } from "@/components/ui/prompt-box";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

import { useJoinCode, type JoinSettings } from "./use-join-code";

export interface AddAgentSpace {
  id: string;
  name: string;
}

type Stage = "pick" | "start" | "connect";
export type AddAgentStage = Stage;

/** Start an agent here lists these, in this order. */
export const START_HARNESSES = ["hermes", "claude-code", "codex", "cursor", "openclaw"] as const;
const START_NAMES: Record<string, string> = { hermes: "Hermes Agent", "claude-code": "Claude Code", codex: "Codex CLI", cursor: "Cursor CLI", openclaw: "OpenClaw" };
const SHORT: Record<string, string> = { hermes: "Hermes", "claude-code": "Claude Code", codex: "Codex", cursor: "Cursor", openclaw: "OpenClaw" };
const HANDLE: Record<string, string> = { hermes: "hermes", "claude-code": "claude", codex: "codex", cursor: "cursor", openclaw: "openclaw" };

/** A free handle for a new member: `hermes`, else `hermes-2`, ... */
export function freeHandle(base: string, taken: readonly string[]): string {
  const set = new Set(taken.map((t) => t.toLowerCase()));
  if (!set.has(base)) return base;
  for (let n = 2; ; n++) if (!set.has(`${base}-${n}`)) return `${base}-${n}`;
}

const EXPIRES = [
  { value: 900, label: "In 15 minutes" },
  { value: 3600, label: "In 1 hour" },
  { value: 86_400, label: "In 1 day" },
];
const TASKS: { value: JoinSettings["canTask"]; label: string }[] = [
  { value: "members", label: "Members" },
  { value: "owner", label: "Only me" },
];

export function AddAgentSheet({
  open,
  onOpenChange,
  space,
  spaces,
  onSpaceChange,
  role,
  taken = [],
  initialStage = "pick",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The Space the agent joins (null: none picked yet). */
  space: AddAgentSpace | null;
  /** With these, the title is a Space picker (the Agents tab's New agent). */
  spaces?: readonly AddAgentSpace[];
  onSpaceChange?: (id: string) => void;
  /** Your role in the Space (null: a daemon that doesn't say, the owner's). */
  role: ThreadRole | null;
  /** Handles already used in the Space (Start picks a free one). */
  taken?: readonly string[];
  /** Open straight on a row (the Agents tab's empty state). */
  initialStage?: Stage;
}) {
  const { data } = useBridge();
  const [stage, setStage] = useState<Stage>("pick");
  useEffect(() => {
    if (open) setStage(initialStage);
  }, [open, initialStage]);
  const canStart = !!data && threadOpAvailable(data.mode, "thread.agentAdd");
  const canConnect = !!data && threadOpAvailable(data.mode, "thread.joinCodeCreate");
  const close = () => onOpenChange(false);

  const title = (text: string, back?: boolean) => (
    <DialogTitle className="flex items-center gap-1.5 text-[15px] font-semibold">
      {back ? (
        <button
          type="button"
          aria-label="Back"
          data-add-agent-back=""
          onClick={() => setStage("pick")}
          className="-ml-1.5 inline-flex size-6 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <ChevronLeftIcon className="size-4" />
        </button>
      ) : null}
      {text}
    </DialogTitle>
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup data-add-agent={stage} className="top-[12vh] w-[min(600px,calc(100vw-2rem))] bg-popover">
        <div className="px-6 pt-5 pb-5">
          {stage === "pick" ? (
            <>
              <DialogTitle className="flex items-center gap-1.5 text-[15px] font-semibold">
                Add an agent to{" "}
                {spaces && onSpaceChange ? (
                  <Select<string>
                    aria-label="Space"
                    value={space?.id ?? ""}
                    options={spaces.map((s) => ({ value: s.id, label: s.name }))}
                    onValueChange={onSpaceChange}
                    className="h-7 min-w-0 border-0 bg-muted px-2 text-[15px] font-semibold shadow-none dark:bg-muted"
                  />
                ) : (
                  <span>{space?.name}</span>
                )}
              </DialogTitle>
              <DialogDescription className="mt-1 text-[13px] text-muted-foreground">It joins the thread, reads @mentions and takes tasks.</DialogDescription>
              <div className="mt-3">
                <Row
                  data="start"
                  icon={<PlayIcon className="size-3.5" />}
                  title="Start an agent here"
                  detail={canStart ? "Runs inside this Space" : "Comes with the next Cua update"}
                  disabled={!canStart || !space}
                  onClick={() => setStage("start")}
                />
                <Row
                  data="connect"
                  icon={<LinkIcon className="size-3.5" />}
                  title="Connect your own agent"
                  detail={canConnect ? "From your computer, with a prompt" : "Comes with the next Cua update"}
                  disabled={!canConnect || !space}
                  onClick={() => setStage("connect")}
                />
              </div>
              <div className="mt-4 flex items-center justify-between gap-4">
                <span className="flex items-center gap-1" data-add-agent-marks="">
                  {HARNESSES.map((h) => (
                    <span key={h.id} title={h.name} className="block overflow-hidden rounded-[5px]">
                      <HarnessMark harness={h.id} px={18} />
                    </span>
                  ))}
                </span>
                <span className="text-xs text-muted-foreground">Any agent that can run a command</span>
              </div>
            </>
          ) : stage === "start" && space ? (
            <StartAgent space={space} taken={taken} title={title("Start an agent here", true)} onDone={close} />
          ) : stage === "connect" && space ? (
            <ConnectAgent space={space} role={role} open={open} title={title("Connect your own agent", true)} onDone={close} />
          ) : null}
        </div>
      </DialogPopup>
    </Dialog>
  );
}

function Row({ icon, title, detail, onClick, disabled, data }: { icon: ReactNode; title: string; detail: string; onClick: () => void; disabled?: boolean; data: string }) {
  return (
    <button
      type="button"
      data-add-row={data}
      disabled={disabled}
      onClick={onClick}
      className="-mx-2 flex h-11 w-[calc(100%+1rem)] cursor-default items-center gap-3 rounded-lg px-2 text-left outline-none hover:bg-accent focus-visible:bg-accent disabled:opacity-50 disabled:hover:bg-transparent"
    >
      <span className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">{icon}</span>
      <span className="w-44 shrink-0 text-[13px]">{title}</span>
      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{detail}</span>
      <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
    </button>
  );
}

function Setting({ label, children, htmlFor }: { label: string; children: ReactNode; htmlFor?: string }) {
  return (
    <div className="flex h-10 items-center justify-between gap-4">
      <label htmlFor={htmlFor} className="text-[13px]">
        {label}
      </label>
      {children}
    </div>
  );
}

const PICK = "h-7 min-w-0 border-0 bg-muted px-2.5 text-[12.5px] shadow-none dark:bg-muted";

/** Start an agent here: today's agent_add, in the Space on your keys. */
function StartAgent({ space, taken, title, onDone }: { space: AddAgentSpace; taken: readonly string[]; title: ReactNode; onDone: () => void }) {
  const { data } = useBridge();
  const [harness, setHarness] = useState<string>(START_HARNESSES[0]);
  const [name, setName] = useState(() => freeHandle(HANDLE[START_HARNESSES[0]]!, taken));
  const [edited, setEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pick = (h: string) => {
    setHarness(h);
    setError(null);
    if (!edited) setName(freeHandle(HANDLE[h]!, taken));
  };
  const start = () => {
    if (!data || !name.trim()) return;
    setBusy(true);
    setError(null);
    data.call("thread.agentAdd", { spaceId: space.id, name: name.trim().toLowerCase(), harness }).then(
      () => {
        setBusy(false);
        toast(`${SHORT[harness]} is in ${space.name}`, { description: `@mention @${name.trim().toLowerCase()} to give it a task.` });
        onDone();
      },
      (e: unknown) => {
        setBusy(false);
        setError(e instanceof Error ? e.message : String(e));
      },
    );
  };
  return (
    <>
      {title}
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">It runs inside {space.name} on your keys, and sleeps until someone @mentions it.</DialogDescription>
      <div className="mt-3" role="radiogroup" aria-label="Agent">
        {START_HARNESSES.map((h) => (
          <button
            key={h}
            type="button"
            role="radio"
            aria-checked={harness === h}
            data-start-harness={h}
            onClick={() => pick(h)}
            className={cn("-mx-2 flex h-11 w-[calc(100%+1rem)] cursor-default items-center gap-3 rounded-lg px-2 text-left outline-none hover:bg-accent focus-visible:bg-accent", harness === h && "bg-accent")}
          >
            <span className="block overflow-hidden rounded-lg">
              <HarnessMark harness={h} px={24} />
            </span>
            <span className="text-[13px]">{START_NAMES[h]}</span>
            <span className="flex-1 text-xs text-muted-foreground">{HARNESSES.find((x) => x.id === h)?.maker}</span>
            {harness === h ? <CheckIcon className="size-4 text-brand" /> : null}
          </button>
        ))}
      </div>
      <Setting label="Name" htmlFor="add-agent-name">
        <Input
          id="add-agent-name"
          data-start-name=""
          value={name}
          onChange={(e) => {
            setEdited(true);
            setError(null);
            setName(e.target.value);
          }}
          onKeyDown={(e) => e.key === "Enter" && start()}
          className="h-7 w-40 border-0 bg-muted text-[12.5px] shadow-none dark:bg-muted"
        />
      </Setting>
      {error ? (
        <p data-start-error="" className="mt-1 text-xs text-destructive">
          {error}
        </p>
      ) : null}
      <div className="mt-4 flex justify-end gap-2">
        <Button variant="secondary" onClick={onDone}>
          Cancel
        </Button>
        <Button data-start-agent="" disabled={busy || !name.trim()} onClick={start}>
          {busy ? "Starting…" : `Start ${SHORT[harness]}`}
        </Button>
      </div>
    </>
  );
}

/** Connect your own agent: the one line with Copy, its settings, and the wait for the agent. */
function ConnectAgent({ space, role, open, title, onDone }: { space: AddAgentSpace; role: ThreadRole | null; open: boolean; title: ReactNode; onDone: () => void }) {
  const { data } = useBridge();
  const own = role === null || role === "owner";
  // Your own agent in your own Space joins without asking; in someone else's, the owner lets it in.
  const [settings, setSettings] = useState<JoinSettings>({ ttlS: 3600, canTask: "members", askBeforeJoin: !own });
  const stable = useMemo(() => settings, [settings]);
  const { watch, copied, renew, cancel } = useJoinCode(space.id, stable, open);
  const code = "code" in watch ? watch.code : null;

  // It joined (or asks to, in the thread): the sheet's work is done, once.
  const finished = useRef(false);
  useEffect(() => {
    if (watch.kind !== "joined" || finished.current) return;
    finished.current = true;
    if (watch.state === "pending_approval") toast("Your agent is asking to join", { description: "The owner or an admin lets it in from the thread." });
    onDone();
  }, [watch, onDone]);

  const waiting = watch.kind === "waiting";
  const set = (patch: Partial<JoinSettings>) => setSettings((s) => ({ ...s, ...patch }));
  return (
    <>
      {title}
      <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
        Paste this line into Hermes, Claude Code, Codex, Cursor or any other agent. The link tells it what to do.
      </DialogDescription>
      <PromptBox
        className="mt-4"
        aria-label="Line to paste into your agent"
        data-join-line={code?.code ?? ""}
        value={code ? code.line : watch.kind === "failed" ? "" : "Making a code…"}
        disabled={!code || watch.kind === "expired"}
        copied={waiting}
        onCopy={copied}
      />
      <p className="mt-1.5 text-xs text-muted-foreground">
        Works once, for this Space only.{" "}
        {code ? (
          <button
            type="button"
            data-join-doc={code.url}
            onClick={() => void data?.call("session.openExternal", { url: code.url })}
            className="cursor-default text-brand-strong outline-none hover:underline focus-visible:underline"
          >
            What the agent reads
          </button>
        ) : null}
      </p>
      <div className="mt-3">
        <Setting label="Code expires">
          <Select<number> aria-label="Code expires" value={settings.ttlS} options={EXPIRES} onValueChange={(v) => set({ ttlS: v })} className={PICK} />
        </Setting>
        <Setting label="It can take tasks from">
          <Select<JoinSettings["canTask"]> aria-label="It can take tasks from" value={settings.canTask} options={TASKS} onValueChange={(v) => set({ canTask: v })} className={PICK} />
        </Setting>
        <Setting label="Ask me before it joins" htmlFor="add-agent-ask">
          <Switch id="add-agent-ask" data-join-ask="" checked={settings.askBeforeJoin} onCheckedChange={(v) => set({ askBeforeJoin: v })} />
        </Setting>
      </div>
      {waiting ? (
        <p data-join-waiting="" className="mt-3 flex items-center gap-2 text-[13px] text-muted-foreground">
          <Loader2Icon className="size-3.5 animate-spin" />
          Waiting for your agent to run the code…
        </p>
      ) : watch.kind === "expired" ? (
        <p data-join-expired="" className="mt-3 flex items-center gap-3 text-[13px] text-muted-foreground">
          This code expired before an agent used it.
          <Button size="sm" variant="secondary" data-join-renew="" onClick={renew}>
            New code
          </Button>
        </p>
      ) : watch.kind === "failed" ? (
        <p data-join-failed="" className="mt-3 flex items-center gap-3 text-[13px] text-destructive">
          Couldn't make a code: {watch.error}
          <Button size="sm" variant="secondary" onClick={renew}>
            Try again
          </Button>
        </p>
      ) : null}
      <div className="mt-4 flex items-center gap-2">
        <span className="text-xs text-muted-foreground">Only this Space. You can remove it any time.</span>
        <span className="flex-1" />
        {waiting ? (
          <Button variant="secondary" data-join-done="" onClick={onDone}>
            Done
          </Button>
        ) : (
          <Button
            variant="secondary"
            data-join-cancel=""
            onClick={() => {
              cancel();
              onDone();
            }}
          >
            Cancel
          </Button>
        )}
      </div>
    </>
  );
}
