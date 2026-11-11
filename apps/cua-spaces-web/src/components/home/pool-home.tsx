// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, useNavigate } from "@tanstack/react-router";
import { PlayIcon, PlusIcon, ShuffleIcon, XIcon } from "lucide-react";
import { useMemo, useRef, useState } from "react";

import { useBridge, useResources } from "@/bridge";
import { BringTheRest } from "@/components/ask-first/bring-the-rest";
import { useStartTask } from "@/components/ask-first/use-start-task";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { PrepareLine } from "@/components/prepare/prepare-line";
import { usePrepareAction } from "@/components/prepare/use-prepare-action";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";
import { askTaskTitle, useAskTasks, type AskTask } from "@/lib/ask-first";
import { THIS_COMPUTER } from "@/lib/machines";
import { placeJob, poolHeadline, poolLine, whereOn, type HomeComputer } from "@/lib/pool";
import { usePrepare } from "@/lib/prepare/store";
import { thisComputerText } from "@/lib/host-labels";

import { AddComputerTile, ComputerCard } from "./computer-card";
import { ComputerWarnings } from "./computer-warnings";
import { Dots } from "./keys";
import { RunSomethingSheet, useRunPick, type RunRequest } from "./run-something";
import { useHomeComputers } from "./use-home-computers";

/** How long Home says a job moved. */
const MOVED_SHOWN_MS = 24 * 60 * 60_000;
const MOVED_SEEN_KEY = "cua.home.movedSeen";

/** The newest job that moved off a computer it couldn't reach, until it's dismissed or a day old. */
export function movedTask(tasks: readonly AskTask[], seen: string | null, now: number): AskTask | null {
  const t = tasks.find((x) => x.movedFrom && x.computer && !x.ending);
  return t && t.id !== seen && now - t.createdMs < MOVED_SHOWN_MS ? t : null;
}

function readSeen(): string | null {
  try {
    return globalThis.localStorage?.getItem(MOVED_SEEN_KEY) ?? null;
  } catch {
    return null;
  }
}

/**
 * Home, option A ("Machine grid", 10 Nov 2026): your computers as one supercomputer. The total at the top with
 * Add a computer and Run something, then one card per computer (online or not, its free room and its jobs) and a
 * big Add a computer tile. Chat lives in a job's own page, one click from its card.
 */
export function PoolHome() {
  const t = useT();
  const navigate = useNavigate();
  const { core } = useBridge();
  const resources = useResources();
  const { computers, pool, summary, thisMac } = useHomeComputers();
  const { start } = useStartTask();
  const tasks = useAskTasks();
  const prep = usePrepare();
  const onPrepareAction = usePrepareAction();
  const [adding, setAdding] = useState(false);
  const [running, setRunning] = useState(false);
  const [seen, setSeen] = useState(readSeen);
  const [harness, setHarness] = useState("claude-code");
  const pick = useRunPick(harness);
  // The move a failed start makes reads the computers as they are then, not as they were when it started.
  const latest = useRef<{ computers: HomeComputer[]; pool: typeof pool }>({ computers: [], pool: [] });
  latest.current = { computers: computers ?? [], pool };

  const placement = useMemo(() => placeJob(core, { computers: pool }), [core, pool]);
  const freeOf = (id: string) => placement.rows.find((r) => r.id === id)?.free ?? null;

  if (!computers) return <div className="h-full" data-pool-home="" aria-busy="true" />;

  const only = computers.length <= 1;
  const byId = (id: string | null) => (id ? latest.current.computers.find((c) => c.machine.id === id) : undefined);
  const named = (c: HomeComputer) => ({ id: c.machine.id, name: c.machine.name });

  const run = (req: RunRequest, started: () => void) => {
    setHarness(req.harness);
    // Placed again as the person presses Run: a computer that went offline since the sheet opened is skipped, and
    // the job says where it moved from.
    const placed = placeJob(core, { computers: latest.current.pool, picked: req.picked });
    const chosen = byId(placed.chosen);
    if (!chosen) return;
    const movedFrom = byId(placed.movedFrom);
    const go = (where: ReturnType<typeof whereOn>) => {
      start(
        { prompt: req.prompt, agent: req.harness, where, model: req.model },
        {
          open: false,
          computer: named(chosen),
          ...(movedFrom ? { movedFrom: named(movedFrom) } : {}),
          move: (tried) => {
            const next = byId(placeJob(core, { computers: latest.current.pool, exclude: [...tried] }).chosen);
            return next ? { where: whereOn(next), computer: named(next) } : null;
          },
        },
      );
      started();
      setRunning(false);
    };
    // This computer asks once before an agent works on your own files and apps. "Pick another" goes back to the
    // sheet, with the task as it was.
    pick.choose(whereOn(chosen), go, () => setRunning(true));
  };

  const moved = movedTask(tasks, seen, Date.now());
  const dismiss = (id: string) => {
    try {
      globalThis.localStorage?.setItem(MOVED_SEEN_KEY, id);
    } catch {
      // Shown again next time.
    }
    setSeen(id);
  };
  const showJob = (task: AskTask) =>
    task.onHost || !task.spaceId
      ? void navigate({ to: "/home/$taskId", params: { taskId: task.id } })
      : void navigate({ to: "/spaces/$spaceId", params: { spaceId: task.spaceId } });

  return (
    <div className="h-full scroll-y-stable" data-pool-home="">
      <div className="mx-auto flex w-full max-w-[1120px] flex-col gap-6 px-5 pt-8 pb-16 sm:px-8 sm:pt-10">
        <header className="flex flex-wrap items-end justify-between gap-x-6 gap-y-4">
          <div className="min-w-0">
            <p className="text-[14px] text-muted-foreground">{t("home.pool.eyebrow")}</p>
            <h1 data-pool-headline="" className="mt-1 font-heading text-[30px] leading-tight font-semibold tracking-tight sm:text-[40px]">
              <Dots parts={poolHeadline(summary)} keep />
            </h1>
            <p data-pool-line="" className="mt-1 text-[15px] text-muted-foreground">
              {computers.length ? poolLine(summary) : t("home.pool.line_none")}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="lg" onClick={() => setAdding(true)} data-pool-add="">
              <PlusIcon /> {t("home.pool.add_title")}
            </Button>
            <Button size="lg" onClick={() => setRunning(true)} data-pool-run="" disabled={!computers.length}>
              <PlayIcon /> {t("home.pool.run")}
            </Button>
          </div>
        </header>

        <PrepareLine view={prep.view} onRetry={onPrepareAction} />
        <BringTheRest />

        {moved ? (
          <div role="status" data-pool-moved={moved.id} className="flex items-center gap-3 rounded-lg bg-tile px-4 py-3 text-[14px]">
            <ShuffleIcon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1">{t("home.pool.moved", { from: moved.movedFrom!.name, job: askTaskTitle(moved), to: moved.computer!.name })}</span>
            <button type="button" onClick={() => showJob(moved)} className="shrink-0 font-medium text-brand-strong hover:underline" data-pool-moved-show="">
              {t("home.pool.show_job")}
            </button>
            <button
              type="button"
              aria-label={t("home.pool.dismiss")}
              onClick={() => dismiss(moved.id)}
              className="flex size-6 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <XIcon className="size-4" />
            </button>
          </div>
        ) : null}

        <ComputerWarnings snapshot={resources} machines={computers.map((c) => c.machine)} />

        {computers.length === 0 ? (
          <div data-pool-none="" className="flex flex-col items-start gap-3 rounded-lg bg-tile p-6">
            <p className="text-[15px]">{t("home.pool.none_body", { thisComputer: thisComputerText() })}</p>
            <Button size="lg" render={<Link to="/spaces/computer/$computerId" params={{ computerId: THIS_COMPUTER }} />}>
              {t("home.pool.add_this", { thisComputer: thisComputerText() })}
            </Button>
          </div>
        ) : null}

        <div className="grid grid-cols-1 gap-5 md:grid-cols-2" data-pool-grid="">
          {computers.map((c) => (
            <ComputerCard
              key={c.machine.id}
              computer={c}
              free={freeOf(c.machine.id)}
              only={only}
              {...(c.needsSetup ? { onSetUp: thisMac.setUp, settingUp: thisMac.busy } : {})}
            />
          ))}
          <AddComputerTile onAdd={() => setAdding(true)} only={only} />
        </div>
      </div>
      <AddComputerSheet open={adding} onOpenChange={setAdding} />
      <RunSomethingSheet open={running} onOpenChange={setRunning} computers={computers} pool={pool} onRun={run} />
      {pick.dialog}
    </div>
  );
}
