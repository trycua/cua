// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, DownloadIcon, RotateCwIcon } from "lucide-react";
import { useState } from "react";

import { useAgents, useSession, useUpdate } from "@/bridge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Menu } from "@/components/ui/menu";
import { toast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { useRelayAllowanceShowing } from "./relay-allowance-notice";
import { restartWarning, runsInProgress, updateView, type UpdateView } from "./update-copy";

/**
 * Everything the indicator needs: what the host's updater shows, and the two
 * actions. "Restart to update" asks first while an agent is running, and the
 * host restarts only on this call: nothing here restarts on its own.
 */
export function useUpdateIndicator() {
  const { data, install } = useUpdate();
  const { data: agents } = useAgents();
  const { openExternal } = useSession();
  const [asking, setAsking] = useState(false);
  const view = updateView(data?.state);
  const running = runsInProgress(agents?.runs);
  const notesUrl = data?.state.notesUrl ?? null;

  const restartNow = () => {
    install().catch((e: unknown) => toast("Couldn't restart to update", { description: e instanceof Error ? e.message : String(e) }));
  };
  return {
    view,
    running,
    installing: data?.installing ?? false,
    asking,
    setAsking,
    restart: () => (running > 0 ? setAsking(true) : restartNow()),
    restartNow,
    whatsNew: notesUrl ? () => void openExternal(notesUrl).catch(() => {}) : null,
  };
}

function ReadyIcon() {
  return (
    <span className="relative grid size-4 place-items-center" aria-hidden>
      <RotateCwIcon className="size-4" strokeWidth={1.75} />
      <span className="absolute -right-1 -bottom-1 grid size-2.5 place-items-center rounded-full bg-foreground text-background ring-2 ring-sidebar">
        <CheckIcon className="size-2" strokeWidth={3} />
      </span>
    </span>
  );
}

function ProgressRing({ percent }: { percent: number | null }) {
  const r = 7;
  const c = 2 * Math.PI * r;
  return (
    <span className="relative grid size-5 place-items-center" aria-hidden>
      <svg viewBox="0 0 20 20" className={cn("absolute inset-0 size-full -rotate-90", percent === null && "animate-spin")}>
        <circle cx="10" cy="10" r={r} fill="none" className="stroke-current/20" strokeWidth="1.5" />
        <circle cx="10" cy="10" r={r} fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="1.5" strokeDasharray={c} strokeDashoffset={c * (1 - (percent ?? 25) / 100)} className="motion-reduce:transition-none" />
      </svg>
      <DownloadIcon className="size-2.5" />
    </span>
  );
}

export function RestartDialog({ ui }: { ui: ReturnType<typeof useUpdateIndicator> }) {
  const w = restartWarning(ui.running);
  return (
    <ConfirmDialog open={ui.asking} onOpenChange={ui.setAsking} title={w.title} description={w.description} confirmLabel={w.confirm} cancelLabel={w.cancel} destructive onConfirm={ui.restartNow} />
  );
}

/**
 * The sidebar's update line, above the account: quiet while it downloads,
 * and a "Restart to update" with a "What's new" link once it is ready. While
 * the relay allowance notice (an upgrade prompt) is open it shrinks to an
 * icon, so the two are never both open.
 */
export function SidebarUpdateIndicator() {
  const ui = useUpdateIndicator();
  const upsellOpen = useRelayAllowanceShowing();
  if (!ui.view) return null;
  if (ui.view.kind === "downloading") return <DownloadingRow view={ui.view} />;
  return (
    <div data-update-indicator="ready" className="border-t px-3 py-2.5">
      {upsellOpen ? (
        <UpdateMenu ui={ui} compact />
      ) : (
        <>
          <div className="flex items-center gap-2.5">
            <span className="text-muted-foreground">
              <ReadyIcon />
            </span>
            <div className="min-w-0 flex-1 leading-tight">
              <div className="truncate text-xs font-medium">{ui.view.label}</div>
              <div className="truncate text-2xs text-muted-foreground">{ui.view.detail}</div>
            </div>
          </div>
          <div className="mt-2 flex items-center gap-3">
            <Button size="sm" variant="outline" className="min-w-0 flex-1" data-update-restart="" disabled={ui.installing} onClick={ui.restart}>
              Restart to update
            </Button>
            {ui.whatsNew ? (
              <button type="button" data-update-whats-new="" onClick={ui.whatsNew} className="shrink-0 text-2xs text-muted-foreground underline decoration-dotted underline-offset-4 hover:text-foreground">
                What's new
              </button>
            ) : null}
          </div>
        </>
      )}
      <RestartDialog ui={ui} />
    </div>
  );
}

function DownloadingRow({ view }: { view: Extract<UpdateView, { kind: "downloading" }> }) {
  return (
    <div data-update-indicator="downloading" role="status" className="flex items-center gap-2.5 border-t px-3 py-2.5 text-muted-foreground">
      <ProgressRing percent={view.percent} />
      <span className="min-w-0 flex-1 truncate text-2xs">{view.label}</span>
    </div>
  );
}

/** One icon with the same actions in a menu: the top bar (sidebar closed) and the compact sidebar. */
function UpdateMenu({ ui, compact = false }: { ui: ReturnType<typeof useUpdateIndicator>; compact?: boolean }) {
  if (!ui.view) return null;
  const tip = ui.view.kind === "ready" ? `${ui.view.label}: restart to install` : ui.view.label;
  const items = ui.view.kind === "ready" ? [{ label: "Restart to update", onSelect: ui.restart, disabled: ui.installing }, ...(ui.whatsNew ? [{ label: "What's new", onSelect: ui.whatsNew }] : [])] : [];
  const icon = ui.view.kind === "ready" ? <ReadyIcon /> : <ProgressRing percent={ui.view.percent} />;
  const button = (
    <Button variant="ghost" size="icon-sm" aria-label={tip} title={tip} data-update-icon="" className={cn("text-muted-foreground", compact && "w-full justify-start gap-2 px-1.5")}>
      {icon}
      {compact ? <span className="truncate text-2xs">{ui.view.label}</span> : null}
    </Button>
  );
  return (
    <>
      {items.length > 0 ? <Menu trigger={button} items={items} /> : button}
      <RestartDialog ui={ui} />
    </>
  );
}

/** The top bar's update icon, shown only while the sidebar (and its line) is closed. */
export function TopBarUpdateIndicator() {
  const ui = useUpdateIndicator();
  if (!ui.view) return null;
  return (
    <span data-update-indicator={ui.view.kind}>
      <UpdateMenu ui={ui} />
    </span>
  );
}
