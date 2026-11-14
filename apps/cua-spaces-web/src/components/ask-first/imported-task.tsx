// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { Link } from "@tanstack/react-router";
import { ArrowLeftIcon, ArrowUpIcon, CheckIcon, ChevronDownIcon, LaptopIcon, Loader2Icon, MonitorIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  folderLabel,
  resumeContext,
  useAgents,
  useAgentTimeline,
  useBridge,
  useMachines,
  type AgentTimeline,
  type HostStatus,
  type PastThread,
  type ThreadAgent,
  type TranscriptItem,
} from "@/bridge";
import { Timeline } from "@/components/agents/timeline";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { HOST_NOT_SHARED, hostSpaceId, updateAskTask, type AskTask } from "@/lib/ask-first";
import { chatComputers } from "@/lib/machines";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

import { agentName, AppMark } from "./ask-box";
import { agoLabel } from "./pick-up";
import { StartFailed } from "./task-view";
import { thisComputerLabel, thisComputerText } from "@/lib/host-labels";

/** The earlier messages as the run's timeline rows, before the run's own. */
export function earlierItems(history: PastThread | null): TranscriptItem[] {
  return (history?.messages ?? []).map((m, i) => ({
    id: `earlier:${m.key || i}`,
    kind: m.role === "person" ? "user" : "message",
    turn: -1,
    text: m.text,
    steps: [],
    startedMs: m.tsMs ?? 0,
  }));
}

/**
 * A chat picked from "Pick up where you left off" (CUA-3319, step 5): its
 * title, agent and folder, what was said so far under "Earlier on this Mac"
 * (read again from the agent's own store), and the box with **Continue on**
 * beside it. Nothing runs until the person sends; then the agent resumes its
 * own session on this Mac, in the chat's folder, and the run's turns follow
 * the earlier ones. The chat stays private: nothing goes to a Space thread.
 */
/** The newest messages a chat opens with: a long chat is never read or drawn whole. */
export const FIRST_VIEW_MESSAGES = 30;
/** Each "Show earlier messages" adds this many. */
export const MORE_MESSAGES = 100;

export function ImportedTaskView({ task }: { task: AskTask & { imported: NonNullable<AskTask["imported"]> } }) {
  const t = useT();
  const { data } = useBridge();
  const { startRun, sendMessage } = useAgents();
  const chat = task.imported;
  const name = agentName(task.agent);
  const [history, setHistory] = useState<PastThread | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);
  // Only the newest messages are read at first (the host reads the end of the chat's file, never all
  // of it); Show earlier messages asks for more. The page never waits on more than it shows.
  const [limit, setLimit] = useState(FIRST_VIEW_MESSAGES);
  const [loadingMore, setLoadingMore] = useState(false);
  useEffect(() => {
    let live = true;
    data
      ?.call("agents.threadHistory", { agent: task.agent as ThreadAgent, id: chat.sessionId, limit })
      .then((h) => live && setHistory(h))
      .catch((e: unknown) => live && setHistoryError(e instanceof Error ? e.message : String(e)))
      .finally(() => live && setLoadingMore(false));
    return () => {
      live = false;
    };
  }, [data, task.agent, chat.sessionId, limit]);

  const ref = task.runId && task.spaceId ? { spaceId: task.spaceId, runId: task.runId } : null;
  const live = useAgentTimeline(ref);
  const timeline = useMemo<AgentTimeline>(() => ({ ...live, items: [...earlierItems(history), ...live.items] }), [live, history]);
  const [reply, setReply] = useState("");
  const [starting, setStarting] = useState(false);
  const lastSent = useRef("");

  const start = async (text: string) => {
    setStarting(true);
    lastSent.current = text;
    try {
      const status = (await data?.call("host.status", {} as never).catch(() => null)) as HostStatus | null;
      const spaceId = hostSpaceId(status);
      if (!spaceId) throw new Error(HOST_NOT_SHARED);
      updateAskTask(task.id, { spaceId, onHost: true, prompt: text, error: null });
      const started = await startRun({
        spaceId,
        agent: task.agent,
        prompt: text,
        ...(chat.cwd ? { cwd: chat.cwd } : {}),
        resumeSession: chat.sessionId,
        ...(history?.messages.length ? { resumeContext: resumeContext(history.messages) } : {}),
      });
      updateAskTask(task.id, { runId: started.run_id });
    } catch (e) {
      updateAskTask(task.id, { error: e instanceof Error ? e.message : String(e) });
    } finally {
      setStarting(false);
    }
  };
  const send = async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed || starting) return;
    setReply("");
    if (!ref) return start(trimmed);
    await sendMessage(ref, trimmed).catch(toastError(t("home.task.imported_send_failed", { name })));
  };

  const folder = folderLabel(chat.cwd);
  const loading = !history && !historyError;
  const waiting = !ref && !task.error;
  return (
    <div className="flex h-full flex-col" data-ask-first-task={task.id} data-imported-chat={task.agent}>
      <header /* design-ok: page-wide header rule, as the task view's */ className="flex h-16 shrink-0 items-center gap-3 border-b px-8">
        <Button variant="ghost" className="-ml-2 h-9" render={<Link to="/home" />}>
          <ArrowLeftIcon /> {t("home.task.home")}
        </Button>
        <AppMark agent={task.agent} size={32} />
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-[15px] font-semibold">{chat.title}</h2>
          <p className="truncate text-[13px] text-muted-foreground">
            {folder
              ? t("home.task.imported_detail_folder", { name, folder, ago: agoLabel(chat.lastActiveMs) })
              : t("home.task.imported_detail", { name, ago: agoLabel(chat.lastActiveMs) })}
          </p>
        </div>
      </header>
      <div className="flex min-h-0 flex-1 flex-col">
        <p className="mx-auto w-full max-w-[720px] shrink-0 px-8 pt-6 text-[12px] font-medium text-muted-foreground" data-earlier-heading="">
          {history?.earlierOmitted
            ? t("home.task.earlier_omitted", { computer: thisComputerText() })
            : t("home.task.earlier", { computer: thisComputerText() })}
          {history?.earlierOmitted ? (
            <button
              type="button"
              disabled={loadingMore}
              onClick={() => {
                setLoadingMore(true);
                setLimit((n) => n + MORE_MESSAGES);
              }}
              className="ml-2 underline underline-offset-2 hover:text-foreground disabled:opacity-60"
              data-earlier-more=""
            >
              {loadingMore ? t("home.task.loading") : t("home.task.show_earlier")}
            </button>
          ) : null}
        </p>
        {loading ? (
          <div className="flex flex-1 items-center justify-center text-muted-foreground">
            <Loader2Icon className="size-5 animate-spin" />
          </div>
        ) : historyError && !timeline.items.length ? (
          <div className="mx-auto w-full max-w-[720px] flex-1 px-8 pt-4 text-[14px] text-muted-foreground" data-earlier-error="">
            {t("home.task.earlier_error", { name })}
          </div>
        ) : (
          <Timeline
            className="flex-1"
            timeline={timeline}
            footer={
              task.error && !ref ? (
                <div className="pb-4">
                  <StartFailed error={task.error} name={name} agent={task.agent} host onRetry={() => void start(lastSent.current || task.prompt)} />
                </div>
              ) : undefined
            }
          />
        )}
        <div className="shrink-0 px-8 pb-6">
          <div className="mx-auto flex max-w-[720px] flex-col gap-2">
            <div
              /* design-ok: the message box, as Home's and the task view's */ className="flex min-h-14 items-end gap-2 rounded-2xl border bg-card py-1.5 pr-1.5 pl-5 shadow-xs dark:bg-white/[0.03]"
            >
              <textarea
                value={reply}
                rows={1}
                autoFocus
                disabled={starting}
                onChange={(e) => setReply(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    void send(reply);
                  }
                }}
                aria-label={t("home.task.next_message", { name })}
                placeholder={waiting ? t("home.task.next_message", { name }) : t("home.task.reply_to", { name })}
                className="min-h-11 flex-1 resize-none self-center bg-transparent py-2.5 text-[14px] outline-none placeholder:text-muted-foreground disabled:opacity-60"
                data-imported-reply=""
              />
              <ContinueOn />
              <button
                type="button"
                aria-label={t("home.task.send")}
                disabled={starting || !reply.trim()}
                onClick={() => void send(reply)}
                className={cn(
                  "flex size-11 shrink-0 items-center justify-center rounded-full",
                  reply.trim() && !starting ? "bg-primary text-primary-foreground" : "bg-foreground/10 text-muted-foreground",
                )}
              >
                {starting ? <Loader2Icon className="size-5 animate-spin" /> : <ArrowUpIcon className="size-5" />}
              </button>
            </div>
            {waiting ? (
              <p className="px-1 text-[12px] text-muted-foreground" data-imported-note="">
                {t("home.task.imported_note", { name, computer: thisComputerText() })}
              </p>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * **Continue on**: where the chat carries on. This Mac today; your other
 * computers that could run it (online, each once, no browsers or phones) are
 * listed but can't be picked yet, because the agent's session lives on this
 * Mac (continuing elsewhere is CUA-3331). The menu opens above or below the
 * box, whichever has room, and scrolls rather than leave the window.
 */
export function ContinueOn() {
  const t = useT();
  const { data: machines } = useMachines();
  const { computers: others, offline } = chatComputers(machines ?? []);
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        render={
          <button
            type="button"
            className="flex h-11 shrink-0 items-center gap-1.5 rounded-lg px-3 text-[13px] outline-none hover:bg-foreground/5 data-popup-open:bg-foreground/[0.07]"
            data-continue-on=""
          >
            <span className="text-muted-foreground">{t("home.task.continue_on_label")}</span> {thisComputerLabel()}{" "}
            <ChevronDownIcon className="size-3.5 text-muted-foreground" />
          </button>
        }
      />
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner side="top" sideOffset={6} align="end" collisionPadding={12} className="z-50 outline-none">
          <MenuPrimitive.Popup
            /* design-ok: a menu's popup, as Home's pickers */
            className="glass max-h-[min(480px,var(--available-height))] w-[340px] origin-(--transform-origin) overflow-y-auto overscroll-contain rounded-xl border p-2 text-popover-foreground shadow-float outline-none"
            data-continue-on-menu=""
          >
            <p className="px-3 pt-2 pb-1 text-[12px] font-medium text-muted-foreground">{t("home.task.continue_on")}</p>
            <MenuPrimitive.Item className="flex min-h-12 cursor-default items-center gap-3 rounded-lg bg-foreground/[0.06] px-3 py-2 outline-none">
              <LaptopIcon className="size-4" />
              <div className="min-w-0 flex-1">
                <div className="text-[14px] font-medium">{thisComputerLabel()}</div>
                <div className="text-[12px] text-muted-foreground">{t("home.task.session_here")}</div>
              </div>
              <CheckIcon className="size-4" />
            </MenuPrimitive.Item>
            {others.map((m) => (
              <MenuPrimitive.Item
                key={m.id}
                disabled
                className="flex min-h-12 cursor-default items-center gap-3 rounded-lg px-3 py-2 opacity-50 outline-none"
                data-continue-on-other={m.id}
              >
                <MonitorIcon className="size-4" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[14px] font-medium">{m.name}</div>
                  <div className="text-[12px] text-muted-foreground">{t("home.task.session_not_yet", { computer: thisComputerText() })}</div>
                </div>
              </MenuPrimitive.Item>
            ))}
            {offline > 0 ? (
              <p className="px-3 pt-2 pb-1 text-[12px] text-muted-foreground" data-continue-on-offline={offline}>
                {t("home.task.offline", { count: offline })}
              </p>
            ) : null}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
