// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist on the Spaces page: five first steps,
 * ticked off as they happen, closable, and back from the command palette
 * or Help > Getting Started. Its steps, words and progress are the core's
 * (`bridge/checklist.ts`); this draws them and runs each step's button.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, XIcon } from "lucide-react";
import { useEffect } from "react";

import {
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

/** With a Space ready, the order that leads somewhere: run an agent in it first; Add computer and Share last. */
const READY_ORDER: ChecklistStep[] = ["run_agent", "create_space", "teleport", "add_machine", "share"];

/** `onRunAgent`: the Space is ready, so Run an agent opens "Run an agent here" and leads the list. */
export function GettingStarted({ onRunAgent }: { onRunAgent?: () => void } = {}) {
  const { view, shown, dismiss } = useChecklist();
  const navigate = useNavigate();
  const newSpace = useNewSpace();
  const { data: spaces = [], isLoading: spacesLoading } = useSpaces();
  const { data: session } = useSession();
  const teleport = useTeleport();
  const share = useShareSheet();
  const visible = view?.visible ?? false;

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

  if (!view || !visible) return null;

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
        return onRunAgent ? onRunAgent() : void navigate({ to: "/agents" });
    }
  };
  const rows = onRunAgent ? [...view.rows].sort((a, b) => READY_ORDER.indexOf(a.step) - READY_ORDER.indexOf(b.step)) : view.rows;

  return (
    <section data-checklist="" aria-labelledby="checklist-title" className="mb-6 rounded-xl border bg-card shadow-xs">
      <header className="flex items-start gap-3 px-4 pt-3.5 pb-2">
        <div className="min-w-0 flex-1">
          <h2 id="checklist-title" className="text-[14px] font-semibold">
            {view.title}
          </h2>
          <p className="mt-0.5 text-xs text-muted-foreground">{view.detail}</p>
        </div>
        <span className="mt-0.5 text-xs tabular-nums text-muted-foreground" data-checklist-count="">
          {view.progress} done
        </span>
        <Button variant="ghost" size="icon-sm" aria-label={view.closeLabel} onClick={() => dismiss()}>
          <XIcon className="size-3.5 text-muted-foreground" />
        </Button>
      </header>
      <ol className="divide-y border-t">
        {rows.map(({ step, title, detail, action, done }) => (
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
                {title}
                <span className="sr-only">{done ? " (done)" : ""}</span>
              </div>
              <div className="truncate text-xs text-muted-foreground">{detail}</div>
            </div>
            {done ? null : (
              <Button variant="outline" size="sm" onClick={() => go(step)}>
                {action}
              </Button>
            )}
          </li>
        ))}
      </ol>
    </section>
  );
}

/**
 * Tells the checklist what the app already shows (another machine, a
 * Space, an agent; the core ticks those steps off) and answers Help >
 * Getting Started. In the Mac app and Electron the host ticks those steps
 * off itself (`checklist::observe_from`) and this page's observations are
 * not sent; they count only where the page keeps its own copy (the demo). Teleport and Share tick themselves off where they
 * finish (`bridge/teleport.ts`, `bridge/share.ts`). Mounted once, under
 * the main layout.
 */
export function ChecklistTracker() {
  const { state, send, reopen } = useChecklist();
  const navigate = useNavigate();
  const { data: machines } = useMachines();
  const { data: spaces } = useSpaces();

  const otherMachine = machines?.some((m) => !m.current) ?? false;
  const hasSpace = spaces ? realSpaces(spaces).some((s) => ["running", "stopped"].includes(spaceState(s))) : false;
  useEffect(() => {
    if (otherMachine || hasSpace) send({ type: "observe", otherMachine, hasSpace, agentRan: false });
  }, [otherMachine, hasSpace, send]);

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
  return state.done.run_agent ? null : <AgentProgress onRun={() => send({ type: "observe", otherMachine: false, hasSpace: false, agentRan: true })} />;
}

function AgentProgress({ onRun }: { onRun: () => void }) {
  const { data } = useAgents();
  const ran = (data?.runs.length ?? 0) > 0 || (data?.agents.length ?? 0) > 0;
  useEffect(() => {
    if (ran) onRun();
  }, [ran, onRun]);
  return null;
}
