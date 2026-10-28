// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Popover as PopoverPrimitive } from "@base-ui/react/popover";
import { DownloadIcon, LoaderCircleIcon } from "lucide-react";
import { useState, type ReactElement } from "react";

import { useAgents, useSession, useUpdate } from "@/bridge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { toast } from "@/components/ui/toast";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { restartWarning, runsInProgress, updateTooltip, updateView, type UpdateView } from "./update-copy";

/**
 * Everything the button needs (also the Machines page's "Restart to update"
 * issue action): what the host's updater shows, and the two actions. "Restart to update" asks first while an agent is running, and the
 * host restarts only on this call: nothing here restarts on its own.
 */
export function useUpdateIndicator() {
  const { data, install } = useUpdate();
  const { data: agents } = useAgents();
  const { openExternal } = useSession();
  const [asking, setAsking] = useState(false);
  const installing = data?.installing ?? false;
  const view = updateView(data?.state, installing);
  const running = runsInProgress(agents?.runs);
  const notesUrl = data?.state.notesUrl ?? null;

  const restartNow = () => {
    install().catch((e: unknown) => toast("Couldn't restart to update", { description: e instanceof Error ? e.message : String(e) }));
  };
  return {
    view,
    running,
    installing,
    asking,
    setAsking,
    restart: () => (running > 0 ? setAsking(true) : restartNow()),
    restartNow,
    whatsNew: notesUrl ? () => void openExternal(notesUrl).catch(() => {}) : null,
  };
}

type UpdateButtonUi = ReturnType<typeof useUpdateIndicator>;

/** The download arrow in a ring that fills with the download. */
function ProgressRing({ percent }: { percent: number | null }) {
  const r = 7;
  const c = 2 * Math.PI * r;
  return (
    <span className="relative grid size-5 place-items-center" aria-hidden>
      <svg viewBox="0 0 20 20" className={cn("absolute inset-0 size-full -rotate-90", percent === null && "animate-spin")}>
        <circle cx="10" cy="10" r={r} fill="none" className="stroke-current/20" strokeWidth="1.5" />
        <circle cx="10" cy="10" r={r} fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.5" strokeDasharray={c} strokeDashoffset={c * (1 - (percent ?? 25) / 100)} className="transition-[stroke-dashoffset] duration-300 motion-reduce:transition-none" />
      </svg>
      <DownloadIcon className="size-2.5" strokeWidth={2} />
    </span>
  );
}

/** The same arrow, with one accent dot: the update is ready. */
function ReadyIcon() {
  return (
    <span className="relative grid size-4 place-items-center" aria-hidden>
      <DownloadIcon className="size-4" strokeWidth={1.75} />
      <span data-update-dot="" className="absolute -top-0.5 -right-0.5 size-1.5 rounded-full bg-brand ring-2 ring-sidebar" />
    </span>
  );
}

function UpdateIcon({ view }: { view: UpdateView }) {
  switch (view.kind) {
    case "downloading":
      return <ProgressRing percent={view.percent} />;
    case "ready":
      return <ReadyIcon />;
    case "installing":
      return <LoaderCircleIcon className="size-4 animate-spin" strokeWidth={1.75} aria-hidden />;
    case "error":
      return <DownloadIcon className="size-4 opacity-50" strokeWidth={1.75} aria-hidden />;
  }
}

export function RestartDialog({ ui }: { ui: UpdateButtonUi }) {
  const w = restartWarning(ui.running);
  return <ConfirmDialog open={ui.asking} onOpenChange={ui.setAsking} title={w.title} description={w.description} confirmLabel={w.confirm} cancelLabel={w.cancel} destructive onConfirm={ui.restartNow} />;
}

/** What a click on the ready button opens: Restart to update, and What's new. */
function ReadyPopover({ ui, view, trigger, tip, side }: { ui: UpdateButtonUi; view: Extract<UpdateView, { kind: "ready" }>; trigger: ReactElement; tip: string; side: "top" | "bottom" }) {
  const [open, setOpen] = useState(false);
  return (
    <PopoverPrimitive.Root open={open} onOpenChange={setOpen}>
      <Tooltip content={tip} side={side}>
        <PopoverPrimitive.Trigger render={trigger} />
      </Tooltip>
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Positioner side={side} align="end" sideOffset={6} className="z-50 outline-none">
          <PopoverPrimitive.Popup
            data-update-popover=""
            className="glass w-56 origin-(--transform-origin) rounded-lg border p-3 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0"
          >
            <PopoverPrimitive.Title className="text-xs font-medium">{view.label}</PopoverPrimitive.Title>
            <PopoverPrimitive.Description className="text-2xs text-muted-foreground">{view.detail}</PopoverPrimitive.Description>
            <Button
              size="sm"
              className="mt-2.5 w-full"
              data-update-restart=""
              disabled={ui.installing}
              onClick={() => {
                setOpen(false);
                ui.restart();
              }}
            >
              Restart to update
            </Button>
            {ui.whatsNew ? (
              <button
                type="button"
                data-update-whats-new=""
                onClick={ui.whatsNew}
                className="mt-2 block w-full text-center text-2xs text-muted-foreground underline decoration-dotted underline-offset-4 hover:text-foreground"
              >
                What's new
              </button>
            ) : null}
          </PopoverPrimitive.Popup>
        </PopoverPrimitive.Positioner>
      </PopoverPrimitive.Portal>
    </PopoverPrimitive.Root>
  );
}

/**
 * The update button: a small icon, hidden while up to date. Downloading
 * fills a ring around the arrow; ready adds one accent dot, and a click
 * opens Restart to update and What's new; installing spins; a failed step
 * is a muted arrow whose tooltip says why. Its tooltip names the state.
 */
function UpdateButton({ side }: { side: "top" | "bottom" }) {
  const ui = useUpdateIndicator();
  const { view } = ui;
  if (!view) return null;
  const tip = updateTooltip(view);
  const button = (
    <Button variant="ghost" size="icon-sm" aria-label={tip} data-update-icon={view.kind} className={cn("text-muted-foreground hover:text-foreground", view.kind === "ready" && "text-foreground")}>
      <UpdateIcon view={view} />
    </Button>
  );
  return (
    <span data-update-indicator={view.kind} className="inline-flex" role={view.kind === "downloading" || view.kind === "installing" ? "status" : undefined}>
      {view.kind === "ready" ? (
        <ReadyPopover ui={ui} view={view} tip={tip} side={side} trigger={button} />
      ) : (
        <Tooltip content={tip} side={side}>
          {button}
        </Tooltip>
      )}
      <RestartDialog ui={ui} />
    </span>
  );
}

/** The update button in the sidebar's footer row. */
export function SidebarUpdateButton() {
  return <UpdateButton side="top" />;
}

/** The same button in the top bar, shown only while the sidebar is closed. */
export function TopBarUpdateIndicator() {
  return <UpdateButton side="bottom" />;
}
