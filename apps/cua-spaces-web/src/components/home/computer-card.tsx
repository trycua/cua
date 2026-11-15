// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, useNavigate } from "@tanstack/react-router";
import { ChevronRightIcon, LoaderCircleIcon, WifiOffIcon } from "lucide-react";

import type { AgentChat } from "@/bridge";
import { Bar } from "@/components/resources/meters";
import { Button } from "@/components/ui/button";
import { AgentTile } from "@/components/space-thread/avatar";
import { useT } from "@/i18n";
import { useAskTasks, type AskTask } from "@/lib/ask-first";
import { thisComputerLabel } from "@/lib/host-labels";
import { computerParam, presenceLabel } from "@/lib/machines";
import { computerSpec, freeText, type Free, type HomeComputer } from "@/lib/pool";
import { osName } from "@/lib/machines";
import { cn } from "@/lib/utils";

import { ComputerKey, Dots, KeyTrio } from "./keys";

/** "14 min" since a job last moved. */
function since(ms: number, now: number, t: ReturnType<typeof useT>): string | null {
  if (!ms) return null;
  const m = Math.max(0, Math.round((now - ms) / 60_000));
  if (m < 1) return t("home.sections.ago_now");
  if (m < 60) return t("home.pool.minutes", { count: m });
  const h = Math.round(m / 60);
  return h < 24 ? t("home.sections.ago_hours", { count: h }) : t("home.sections.ago_days", { count: Math.round(h / 24) });
}

/** The Home task a job came from, when it was started on this device: its project and where it moved from. */
export function taskOf(chat: AgentChat, tasks: readonly AskTask[]): AskTask | undefined {
  const place = chat.places[0];
  if (!place) return undefined;
  return tasks.find((t) => t.spaceId === place.spaceId && !t.ending) ?? (chat.attempt ? tasks.find((t) => t.id === chat.attempt) : undefined);
}

/** Where a job's page is: its Space's thread, or for a job on this computer itself, its task's page. */
export function jobLink(
  chat: AgentChat,
  task: AskTask | undefined,
): { to: "/home/$taskId"; params: { taskId: string } } | { to: "/spaces/$spaceId"; params: { spaceId: string } } | null {
  if (task?.onHost) return { to: "/home/$taskId", params: { taskId: task.id } };
  const place = chat.places[0];
  return place ? { to: "/spaces/$spaceId", params: { spaceId: place.spaceId } } : null;
}

function JobRow({ chat, task }: { chat: AgentChat; task: AskTask | undefined }) {
  const t = useT();
  const navigate = useNavigate();
  const link = jobLink(chat, task);
  const open = () => link && void navigate(link);
  const meta = [
    task?.repo?.label,
    since(chat.lastMs, Date.now(), t),
    task?.movedFrom ? t("home.pool.moved_here", { computer: task.movedFrom.name }) : chat.line,
  ].filter((x): x is string => Boolean(x));
  return (
    <li>
      <button
        type="button"
        data-pool-job={chat.key}
        data-chat-tone={chat.tone}
        onClick={open}
        className="flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left outline-none hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <AgentTile harness={chat.harness} icon={chat.identity?.icon} owner={chat.identity?.owner} name={chat.label} px={32} surface="background" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[14px] font-medium">{chat.job ?? chat.label}</span>
          <Dots parts={meta} className="block truncate text-[13px] text-muted-foreground" />
        </span>
        {chat.tone === "working" || chat.tone === "creating" ? (
          <LoaderCircleIcon aria-label={t("home.pool.job_running")} className="size-4 shrink-0 animate-spin text-brand-strong" />
        ) : (
          <span aria-hidden className={cn("size-2 shrink-0 rounded-full", chat.tone === "failed" ? "bg-destructive" : "bg-warning")} />
        )}
        <ChevronRightIcon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
      </button>
    </li>
  );
}

/** One computer: online or not, how much room it has free now and the jobs on it. Its name opens its own page. */
export function ComputerCard({
  computer,
  free,
  only,
  onSetUp,
  settingUp = false,
}: {
  computer: HomeComputer;
  free: Free | null;
  only: boolean;
  /** This computer isn't set up to take jobs yet: Set up does it. */
  onSetUp?: () => void;
  settingUp?: boolean;
}) {
  const t = useT();
  const tasks = useAskTasks();
  const { node, machine, resources, jobs } = computer;
  const online = node.online;
  const label = thisComputerLabel();
  const model = machine.model?.trim() || osName(machine);
  const r = resources
    ? { cpuCount: resources.cpuCount, cpuPercent: resources.cpuPercent, memoryTotal: resources.memoryTotal, memoryUsed: resources.memoryUsed }
    : null;
  const freeParts = online ? freeText(free, r) : [];
  const empty = !online
    ? t("home.pool.offline_note")
    : computer.needsSetup
      ? t("home.pool.needs_setup", { computer: label })
      : machine.current
        ? only
          ? t("home.pool.no_jobs_only")
          : t("home.pool.no_jobs_here")
        : t("home.pool.no_jobs");
  return (
    <article
      data-pool-computer={machine.id}
      data-online={online ? "true" : "false"}
      data-current={machine.current ? "true" : undefined}
      className="flex min-h-[220px] min-w-0 flex-col rounded-lg bg-tile p-5 sm:p-6"
    >
      <div className="flex items-start gap-3">
        <Link
          to="/spaces/computer/$computerId"
          params={{ computerId: computerParam(machine) }}
          className="flex min-w-0 flex-1 items-center gap-3 rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
          data-pool-computer-link=""
        >
          <ComputerKey os={node.os} size={44} dim={!online} />
          <span className="min-w-0">
            <span className="flex items-baseline gap-2">
              <span className={cn("truncate text-[17px] font-semibold", !online && "text-muted-foreground")}>{machine.name}</span>
              {machine.current && machine.name !== label ? <span className="shrink-0 text-[13px] text-muted-foreground">{label}</span> : null}
            </span>
            <Dots parts={computerSpec(model, r)} className="block truncate text-[13px] text-muted-foreground" />
          </span>
        </Link>
        <span data-pool-presence="" className="flex shrink-0 items-center gap-1.5 pt-1 text-[13px] text-muted-foreground">
          <span aria-hidden className={cn("size-2 rounded-full", online ? "bg-success" : "border border-muted-foreground/60")} />
          {presenceLabel(machine, Date.now())}
        </span>
      </div>

      <div className="mt-5" data-pool-free={free ? Math.round(free.fraction * 100) : "none"}>
        <div className="flex items-baseline justify-between gap-3 text-[13px]">
          <span className="text-muted-foreground">{t("home.pool.free_now")}</span>
          {freeParts.length ? <Dots parts={freeParts} className="truncate font-medium tabular-nums" /> : <span className="text-muted-foreground">—</span>}
        </div>
        <Bar fill={online && free ? free.fraction : 0} className="mt-2 h-1.5" />
      </div>

      <div className="mt-4 flex-1">
        {jobs.length ? (
          <ul data-pool-jobs="" className="-mx-2 flex flex-col">
            {jobs.map((chat) => (
              <JobRow key={chat.key} chat={chat} task={taskOf(chat, tasks)} />
            ))}
          </ul>
        ) : (
          <p data-pool-empty="" className="flex items-start gap-2 text-[14px] text-muted-foreground">
            {!online ? <WifiOffIcon aria-hidden className="mt-0.5 size-4 shrink-0" /> : null}
            {empty}
          </p>
        )}
        {onSetUp ? (
          <Button size="sm" variant="outline" className="mt-3" onClick={onSetUp} disabled={settingUp} data-pool-set-up="">
            {settingUp ? t("home.pool.setting_up") : t("home.pool.set_up")}
          </Button>
        ) : null}
      </div>
    </article>
  );
}

/** The big dashed "Add a computer" tile; with only this computer, it says in one line why you'd add one. */
export function AddComputerTile({ onAdd, only }: { onAdd: () => void; only: boolean }) {
  const t = useT();
  return (
    <button
      type="button"
      onClick={onAdd}
      data-pool-add-tile=""
      // design-ok: the dashed tile is the page's biggest target for adding a computer (Home option A, 10 Nov 2026).
      className="flex min-h-[220px] flex-col items-center justify-center gap-3 rounded-lg border border-dashed border-foreground/20 px-6 py-8 text-center outline-none transition-colors hover:bg-accent/40 focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      <KeyTrio size={44} />
      <span className="text-[17px] font-semibold">{t("home.pool.add_title")}</span>
      <span className="max-w-[360px] text-[14px] text-muted-foreground">{only ? t("home.pool.add_why_only") : t("home.pool.add_why")}</span>
    </button>
  );
}
