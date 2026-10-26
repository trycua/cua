// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The getting-started checklist in the sidebar: five first steps, ticked
 * off as they happen, each skippable, closable, and back from the command
 * palette or Help > Getting Started. Its steps, words and progress are the
 * core's (`bridge/checklist.ts`); this draws them and runs each step.
 */

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, ChevronDownIcon } from "lucide-react";
import { useEffect, useSyncExternalStore } from "react";

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

/** Whether the sidebar's checklist is open (Help > Getting Started opens it). */
let expanded = false;
const expandListeners = new Set<() => void>();
export function setExpanded(on: boolean): void {
  expanded = on;
  for (const l of [...expandListeners]) l();
}
function useExpanded(): boolean {
  return useSyncExternalStore(
    (l) => {
      expandListeners.add(l);
      return () => void expandListeners.delete(l);
    },
    () => expanded,
    () => false,
  );
}

/** What each step's row does when clicked. */
function useChecklistGo(): (step: ChecklistStep) => void {
  const navigate = useNavigate();
  const newSpace = useNewSpace();
  const { data: spaces = [], isLoading: spacesLoading } = useSpaces();
  const { data: session } = useSession();
  const teleport = useTeleport();
  const share = useShareSheet();
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
      return void toast("Sharing isn't available yet", { description: "It is not switched on for this account." });
    }
    toast("Start a Space first", { description: `${what} works on a running Space.` });
  };
  return (step) => {
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
}

/**
 * "Get started 2/5" at the bottom of the sidebar. Its steps open in place,
 * above it: each does its thing when clicked, and has a quiet Skip (a
 * skipped step counts as done). Hide closes it ("Done" once complete); Help
 * > Getting Started brings it back, open. The steps, words and progress are
 * the core's (`bridge/checklist.ts`).
 */
export function SidebarGetStarted() {
  const { view, shown, dismiss, skip } = useChecklist();
  const open = useExpanded();
  const go = useChecklistGo();
  const visible = view?.visible ?? false;

  useEffect(() => {
    if (visible) shown();
  }, [visible, shown]);

  if (!view || !visible) return null;
  const settled = view.rows.filter((r) => r.done || r.skipped).length;
  const share = settled / view.rows.length;
  return (
    <section data-checklist="" data-open={open || undefined} aria-label={view.title} className="mx-2 mb-1">
      {open ? (
        <ol className="mb-1 border-t border-foreground/10 pt-1" data-checklist-steps="">
          {view.rows.map(({ step, title, detail, done, skipped }) => (
            <li key={step} data-checklist-step={step} data-done={done || undefined} data-skipped={skipped || undefined} className="group flex h-7 items-center gap-2 rounded-md px-2 text-[12.5px] hover:bg-foreground/5">
              <span
                aria-hidden
                className={cn(
                  "flex size-3.5 shrink-0 items-center justify-center rounded-full border",
                  done ? "border-brand bg-brand text-white" : skipped ? "border-foreground/25 bg-foreground/10" : "border-foreground/25",
                )}
              >
                {done ? <CheckIcon className="size-2" strokeWidth={3.5} /> : null}
              </span>
              <button
                type="button"
                title={detail}
                disabled={done || skipped}
                onClick={() => go(step)}
                className={cn("min-w-0 flex-1 truncate text-left outline-none disabled:cursor-default", done || skipped ? "text-muted-foreground line-through decoration-foreground/30" : "text-foreground")}
              >
                {title}
                <span className="sr-only">{done ? " (done)" : skipped ? " (skipped)" : ""}</span>
              </button>
              {done ? null : skipped ? (
                <span className="text-[11px] text-muted-foreground">Skipped</span>
              ) : (
                <button type="button" className="text-[11px] text-muted-foreground opacity-70 hover:text-foreground hover:opacity-100" onClick={() => skip(step)} data-checklist-skip={step}>
                  {view.skipLabel}
                </button>
              )}
            </li>
          ))}
        </ol>
      ) : null}
      <div className="flex h-7 items-center gap-2 rounded-md px-2 text-[13px] text-muted-foreground hover:bg-foreground/5">
        <button type="button" className="flex min-w-0 flex-1 items-center gap-2 text-left outline-none hover:text-foreground" onClick={() => setExpanded(!open)} aria-expanded={open} data-checklist-toggle="">
          <svg viewBox="0 0 16 16" aria-hidden className="size-3.5 shrink-0 -rotate-90">
            <circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" strokeOpacity=".25" strokeWidth="2" />
            <circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" strokeWidth="2" strokeDasharray={`${share * 40.8} 40.8`} />
          </svg>
          <span className="flex-1 truncate">{view.title}</span>
          <span className="tabular-nums" data-checklist-count="">
            {settled}/{view.rows.length}
          </span>
          <ChevronDownIcon aria-hidden className={cn("size-3.5 transition-transform", open ? "" : "rotate-180")} />
        </button>
        {open || view.complete ? (
          <button type="button" className="text-[11px] hover:text-foreground" onClick={() => dismiss()} aria-label={view.closeLabel} data-checklist-hide="">
            {view.hideLabel}
          </button>
        ) : null}
      </div>
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
      setExpanded(true);
    };
    window.addEventListener(WEBKIT_EVENT, onHostEvent);
    return () => window.removeEventListener(WEBKIT_EVENT, onHostEvent);
  }, [reopen]);

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
