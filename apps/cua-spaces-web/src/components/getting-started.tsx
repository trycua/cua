// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist on the Spaces page: five first steps,
 * ticked off as they happen (`bridge/checklist.ts` keeps them), closable,
 * and back from the command palette or Help > Getting Started.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, XIcon } from "lucide-react";
import { useEffect } from "react";

import {
  CHECKLIST_STEPS,
  checklistComplete,
  useAgents,
  useBridge,
  useChecklist,
  useExperiments,
  useMachines,
  useSession,
  useShareSheet,
  useSpaces,
  useTeleport,
  type ChecklistStep,
  type Space,
} from "@/bridge";
import { spaceDetail } from "@/bridge/space-detail";
import { WEBKIT_EVENT, WEBKIT_OPEN_CHECKLIST_EVENT, type WebkitEventDetail } from "@/bridge/webkit-protocol";
import { Button } from "@/components/ui/button";
import { toast, toastError } from "@/components/ui/toast";
import { useNewSpace } from "@/hooks/use-new-space";
import { realSpaces, spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

/**
 * Where "Add a machine" goes. One place, so a new add-machine flow (a link
 * or QR code, a team invite) only has to change this.
 */
export const ADD_MACHINE_HREF = "/machines?add=1";

const COPY: Record<ChecklistStep, { title: string; detail: string; action: string }> = {
  add_machine: { title: "Add a machine", detail: "Connect a Mac, Linux or Windows computer you own.", action: "Add machine" },
  create_space: { title: "Create a Space", detail: "A computer for one task, on one of your machines.", action: "Create" },
  teleport: { title: "Teleport an app", detail: "Move a signed-in app into a Space.", action: "Teleport" },
  share: { title: "Share a Space", detail: "Let a teammate watch or work in it.", action: "Share" },
  run_agent: { title: "Run an agent", detail: "Give an agent a Space to work in.", action: "Open Agents" },
};

/** The first Space whose own page offers `action` now (the core's detail
 * actions: Share follows Settings, Experiments; both need it running). */
function spaceFor(spaces: Space[], offers: (space: Space) => boolean | undefined): { space?: Space; offered: boolean } {
  const real = realSpaces(spaces).filter((s) => spaceState(s) === "running" || spaceState(s) === "stopped");
  let offered = false;
  for (const space of real) {
    const on = offers(space);
    if (on === undefined) continue;
    offered = true;
    if (on) return { space, offered };
  }
  return { offered };
}

export function GettingStarted() {
  const { state, shown, dismiss } = useChecklist();
  const navigate = useNavigate();
  const newSpace = useNewSpace();
  const { data: spaces = [], isLoading: spacesLoading } = useSpaces();
  const { data: session } = useSession();
  const teleport = useTeleport();
  const share = useShareSheet();
  const visible = !state.dismissed;
  const complete = checklistComplete(state);
  const count = CHECKLIST_STEPS.filter((s) => state.done[s]).length;

  useEffect(() => {
    if (visible) shown();
  }, [visible, shown]);

  const { core } = useBridge();
  const exp = useExperiments();
  const experiments = exp.unsupported ? undefined : (exp.data?.experiments ?? {});
  const { data: machines = [] } = useMachines();
  const thisMachine = machines.find((m) => m.current);
  const offers = (id: string) => (space: Space) => {
    const a = spaceDetail(core, space, null, thisMachine?.arch ?? null, experiments, thisMachine?.accessNotice ?? null).actions.find((x) => x.id === id);
    return a ? a.enabled : undefined;
  };
  /** Opens `open` on a Space that offers it, or says what is missing. */
  const onSpace = (id: string, open: (space: Space) => Promise<void>, what: string) => {
    if (spacesLoading) return;
    if (realSpaces(spaces).length === 0) return newSpace();
    const { space, offered } = spaceFor(spaces, offers(id));
    if (space) return void open(space).catch(toastError(`Couldn't open ${what}`));
    if (!offered && id === "share") {
      toast("Sharing is off", { description: "Turn it on in Settings, Experiments." });
      return void navigate({ to: "/settings/experiments" });
    }
    toast("Start a Space first", { description: `${what} works on a running Space.` });
  };

  if (!visible) return null;

  const go = (step: ChecklistStep) => {
    switch (step) {
      case "add_machine":
        return void navigate({ href: ADD_MACHINE_HREF });
      case "create_space":
        return newSpace();
      case "teleport":
        return onSpace("teleport", (space) => teleport.open(space), "Teleport");
      case "share":
        return onSpace("share", (space) => share.open(space, session?.signedIn ?? false), "Share");
      case "run_agent":
        return void navigate({ to: "/agents" });
    }
  };

  return (
    <section data-checklist="" aria-labelledby="checklist-title" className="mb-6 rounded-xl border bg-card shadow-xs">
      <header className="flex items-start gap-3 px-4 pt-3.5 pb-2">
        <div className="min-w-0 flex-1">
          <h2 id="checklist-title" className="text-[14px] font-semibold">
            {complete ? "You're set up" : "Get started"}
          </h2>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {complete
              ? "Your team and its agents can now share computers you own."
              : "Shared computers for your team and its agents, on machines you own."}
          </p>
        </div>
        <span className="mt-0.5 text-xs tabular-nums text-muted-foreground" data-checklist-count="">
          {count} of {CHECKLIST_STEPS.length}
        </span>
        <Button variant="ghost" size="icon-sm" aria-label="Close getting started" onClick={() => dismiss()}>
          <XIcon className="size-3.5 text-muted-foreground" />
        </Button>
      </header>
      <ol className="divide-y border-t">
        {CHECKLIST_STEPS.map((step) => {
          const done = state.done[step] === true;
          const copy = COPY[step];
          return (
            <li key={step} data-checklist-step={step} data-done={done || undefined} className="flex items-center gap-3 px-4 py-2.5">
              <span
                aria-hidden
                className={cn(
                  "flex size-5 shrink-0 items-center justify-center rounded-full border",
                  done ? "border-brand bg-brand text-white" : "border-foreground/20",
                )}
              >
                {done ? <CheckIcon className="size-3" strokeWidth={3} /> : null}
              </span>
              <div className="min-w-0 flex-1">
                <div className={cn("text-[13px] font-medium", done && "text-muted-foreground line-through decoration-foreground/30")}>
                  {copy.title}
                  <span className="sr-only">{done ? " (done)" : ""}</span>
                </div>
                <div className="truncate text-xs text-muted-foreground">{copy.detail}</div>
              </div>
              {done ? null : (
                <Button variant="outline" size="sm" onClick={() => go(step)}>
                  {copy.action}
                </Button>
              )}
            </li>
          );
        })}
      </ol>
    </section>
  );
}

/**
 * Ticks steps off from what the app already shows (another machine, a
 * Space, an agent's run) and answers Help > Getting Started. Teleport and
 * Share tick themselves off where they finish (`bridge/teleport.ts`,
 * `bridge/share.ts`). Mounted once, under the main layout.
 */
export function ChecklistTracker() {
  const { state, done, reopen } = useChecklist();
  const navigate = useNavigate();
  const { data: machines } = useMachines();
  const { data: spaces } = useSpaces();

  const otherMachine = machines?.some((m) => !m.current) ?? false;
  const hasSpace = spaces ? realSpaces(spaces).some((s) => ["running", "stopped"].includes(spaceState(s))) : false;
  useEffect(() => {
    if (otherMachine && !state.done.add_machine) done("add_machine");
  }, [otherMachine, state.done.add_machine, done]);
  useEffect(() => {
    if (hasSpace && !state.done.create_space) done("create_space");
  }, [hasSpace, state.done.create_space, done]);

  useEffect(() => {
    const onHostEvent = (event: Event) => {
      if ((event as CustomEvent<WebkitEventDetail>).detail?.event !== WEBKIT_OPEN_CHECKLIST_EVENT) return;
      reopen();
      void navigate({ to: "/spaces" });
    };
    window.addEventListener(WEBKIT_EVENT, onHostEvent);
    return () => window.removeEventListener(WEBKIT_EVENT, onHostEvent);
  }, [reopen, navigate]);

  // Agents are only read while this step is open.
  return state.done.run_agent ? null : <AgentProgress onRun={() => done("run_agent")} />;
}

function AgentProgress({ onRun }: { onRun: () => void }) {
  const { data } = useAgents();
  const ran = (data?.runs.length ?? 0) > 0 || (data?.agents.length ?? 0) > 0;
  useEffect(() => {
    if (ran) onRun();
  }, [ran, onRun]);
  return null;
}
