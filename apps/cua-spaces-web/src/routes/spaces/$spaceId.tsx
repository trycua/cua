// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { ArrowDownCircleIcon, ChevronLeftIcon, MonitorIcon, PlayIcon, SquareIcon, Trash2Icon, UserPlusIcon } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { parseDropped, useDevices, useExperiments, useMachines, useSession, useShareSheet, useSpaceDetail, useSpaceFiles, useSpaces, useTeleport, type Space, type StreamRow } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page } from "@/components/page";
import { AgentsSection } from "@/components/space-detail/agents-section";
import { DropWell } from "@/components/space-detail/drop-well";
import { FactList } from "@/components/space-detail/facts";
import { StreamSection } from "@/components/space-detail/stream-section";
import { StreamSurface } from "@/components/space-detail/stream-surface";
import { StateLabel } from "@/components/state-dot";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { useSpaceVolumeError } from "@/lib/space-volume";
import { volumeUnavailableNote } from "@/lib/volume-notes";
import { canDelete, canPower, createError, createProgress, machineName, powerPending, spacePlace, spaceState } from "@/lib/spaces";

export const Route = createFileRoute("/spaces/$spaceId")({
  component: SpaceDetailPage,
  // Apps and files dropped on this Space's tile in the notch, by name (the host holds their paths).
  validateSearch: (s: Record<string, unknown>): { dropped?: string[] } => {
    const dropped = parseDropped(s.dropped);
    return dropped ? { dropped } : {};
  },
});

type Confirm = "stop" | "delete" | null;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function SpaceDetailPage() {
  const { spaceId } = Route.useParams();
  const { dropped } = Route.useSearch();
  const navigate = useNavigate();
  const { data: spaces, isLoading, error, startSpace, stopSpace, deleteSpace, openSpace, dismissCreate, retryCreate, canRetryCreate, createdId } =
    useSpaces();
  const { data: machines = [] } = useMachines();
  const [confirm, setConfirm] = useState<Confirm>(null);
  const teleport = useTeleport();
  const sharing = useShareSheet();
  const { data: session } = useSession();
  const space = spaces?.find((s) => s.id === spaceId);
  // Apps and files dropped on this Space's notch tile: the same as a drop on the well (an app opens Teleport at it), once.
  const files = useSpaceFiles(space ?? { id: spaceId, name: spaceId });
  const droppedOnce = useRef<string[] | null>(null);
  useEffect(() => {
    if (!dropped || !space || droppedOnce.current === dropped) return;
    droppedOnce.current = dropped;
    void files.drop(dropped);
    // Off the address, so a reload does not teleport again.
    void navigate({ to: "/spaces/$spaceId", params: { spaceId }, search: {}, replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dropped, space?.id]);
  const thisMachine = machines.find((m) => m.current);
  const hostArch = thisMachine?.arch ?? null;
  // As this device sees it: Share only with Settings, Experiments, Sharing
  // on (every action where the host has no experiments), and a machine on
  // the relay greyed out while this device is not enrolled.
  const exp = useExperiments();
  const experiments = exp.unsupported ? undefined : (exp.data?.experiments ?? {});
  const access = thisMachine?.accessNotice ?? null;
  const devices = useDevices();
  const { detail, stream, windowsUnsupported, pip, query, setQuery, agents, copy } = useSpaceDetail(space, hostArch, { experiments, access });
  const volumeError = useSpaceVolumeError(space, space ? spaceState(space) === "running" : false);
  // Open on a creating row: once the create finished and the registry lists
  // the Space, its row (and this id) goes; follow it to the Space's own id.
  // Only a page that showed the row follows: the address of a row that is
  // already gone (Back after Remove from list) says so, and never jumps into
  // another Space.
  const [showed, setShowed] = useState<string | null>(null);
  useEffect(() => {
    if (space) setShowed(spaceId);
  }, [space, spaceId]);
  const created = !space && showed === spaceId ? createdId(spaceId) : undefined;
  useEffect(() => {
    if (created) void navigate({ to: "/spaces/$spaceId", params: { spaceId: created }, replace: true });
  }, [created, navigate]);
  if (created) return null;

  if (error || (!isLoading && !space)) {
    return (
      <Page>
        <BackLink />
        <EmptyState
          icon={<MonitorIcon />}
          title={error ? "Couldn't load this Space" : "This Space isn't here"}
          action={
            <Button variant="outline" render={<Link to="/spaces" />}>
              Back to Spaces
            </Button>
          }
        >
          {error ? error.message : "It may have been deleted, or it runs on a machine this app can't see."}
        </EmptyState>
      </Page>
    );
  }
  if (!space || !detail) return null;

  const state = spaceState(space);
  const machine = machineName(machines, space);
  const pending = powerPending(space);

  const run = (label: string, action: () => Promise<void>) =>
    action().catch((e: unknown) => toast(`Couldn't ${label} ${space.name}`, { description: message(e), type: "error" }));

  const start = () => run("start", () => startSpace(space.id));
  // The core's detail actions: present only where they apply (Share follows
  // Settings, Experiments), enabled only when the Space can stream and this
  // device may connect.
  const action = (id: string) => detail.actions.find((a) => a.id === id);
  const teleportAction = action("teleport");
  const shareAction = action("share");
  // Signed in, not enrolled: the access line's action opens Settings,
  // Devices with the enroll sheet.
  const enroll = () => {
    devices.startEnroll();
    void navigate({ to: "/settings/devices" });
  };
  const teleportApp = () => run("open Teleport for", () => teleport.open(space));
  const share = () => run("share", () => sharing.open(space, session?.signedIn ?? false));
  const stop = () => run("stop", () => stopSpace(space.id));
  // Leaving a Space that is going away: the grid takes this page's place in
  // the history, so Back never returns to a page for a Space that is gone.
  const leave = () => navigate({ to: "/spaces", replace: true });
  // Delete, or "Remove from List" (a Space in your cloud: forget it here,
  // keep it running there; `AppModel.delete(_:removeOnly:)`).
  const remove = (removeOnly = false) => {
    void leave();
    void run(removeOnly ? "remove" : "delete", async () => {
      await deleteSpace(space.id, removeOnly);
      toast(removeOnly ? `Removed ${space.name} from the list` : `Deleted ${space.name}`, { type: "success" });
    });
  };
  // A failed create only leaves the list (here and on the host). The grid
  // opens first, then the row goes: this page never shows "isn't here" in
  // between, and nothing else decides where the person lands.
  const dismiss = () => {
    void leave().then(() => run("remove", () => dismissCreate(space.id)));
  };
  // Try again: the same create as a new row; this page follows it.
  const retry = canRetryCreate(space.id)
    ? () => {
        const next = retryCreate(space.id);
        if (next) void navigate({ to: "/spaces/$spaceId", params: { spaceId: next }, replace: true });
      }
    : undefined;
  // Picture in picture where the host has panels; its own window where it doesn't.
  const onPip = (row: StreamRow) =>
    run("open", async () => {
      if ((await pip(row.id)) === "open-window") {
        await openSpace(space.id);
        toast(`Opening ${space.name}`, { description: "Picture in picture isn't available here, so its desktop opens in a separate window." });
      }
    });

  const display = stream?.rows.find((r) => r.kind === "desktop")?.resolution?.split("×").map(Number);
  const aspect = display?.length === 2 && display[0]! > 0 && display[1]! > 0 ? display[0]! / display[1]! : undefined;
  const showStream = detail.sections.includes("Stream") && stream;
  // The core's sections after Stream: Agents (where the host lists runs)
  // and Teleport (the drop well), as the SwiftUI detail draws them.
  const showAgents = detail.sections.includes("Agents") && !agents.unsupported;
  const showTeleport = detail.sections.includes("Teleport");

  return (
    <Page>
      <BackLink />
      <header className="mb-5 flex items-start justify-between gap-4">
        <div className="flex min-w-0 items-start gap-3">
          <div className="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg border bg-card shadow-xs">
            <OsIcon os={space.os} className="size-4.5 text-muted-foreground" />
          </div>
          <div className="min-w-0">
            <h1 data-detail-title className="truncate text-[22px] leading-tight font-semibold tracking-[-0.01em]">{detail.title}</h1>
            <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted-foreground">
              <span>{spacePlace(space, machine)}</span>
              {pending ? <span className="text-xs">{pending}</span> : <StateLabel state={state} space={space} />}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {teleportAction ? (
            <Button variant="outline" disabled={!teleportAction.enabled} title={teleportAction.help} onClick={teleportApp}>
              <ArrowDownCircleIcon /> {teleportAction.label}
            </Button>
          ) : null}
          {shareAction ? (
            <Button variant="outline" data-share-space="" disabled={!shareAction.enabled} title={shareAction.help} onClick={share}>
              <UserPlusIcon /> {shareAction.label}
            </Button>
          ) : null}
          {canPower(space) ? (
            state === "stopped" ? (
              <Button variant="outline" disabled={Boolean(pending)} onClick={start}>
                <PlayIcon /> Start
              </Button>
            ) : (
              <Button variant="outline" disabled={Boolean(pending)} onClick={() => setConfirm("stop")}>
                <SquareIcon className="size-3.5" /> Stop
              </Button>
            )
          ) : null}
          {state === "failed" ? (
            <Button variant="outline" data-dismiss-create="" onClick={dismiss}>
              <Trash2Icon className="text-destructive" /> Remove
            </Button>
          ) : null}
          {canDelete(space) ? (
            <Button variant="outline" aria-label={`${detail.deleteLabel} ${space.name}`} onClick={() => setConfirm("delete")}>
              <Trash2Icon className="text-destructive" /> {detail.deleteLabel}
            </Button>
          ) : null}
        </div>
      </header>

      {detail.powerError ? <p className="mb-4 text-[13px] text-destructive">{detail.powerError}</p> : null}

      {detail.desktopNote ? (
        // One of your machines that keeps its desktop private: why, in place
        // of the desktop, then the Spaces it provides (no New Space button
        // beside the note).
        <MachineSpaces note={detail.desktopNote} listSpaces={Boolean(detail.newSpace)} spaces={hostedSpaces(spaces ?? [], space.id)} />
      ) : (
        <StreamSurface
          spaceId={space.id}
          os={space.os}
          aspectRatio={aspect}
          canStream={detail.canStream}
          previewText={detail.previewText}
          access={detail.access}
          onAccessAction={enroll}
          className="h-72"
        >
          {surfaceState(space, pending, start, dismiss, retry)}
        </StreamSurface>
      )}

      <div className="mt-8 grid items-start gap-x-6 md:grid-cols-2">
        <div>
          <FactList title="Details" facts={detail.facts} />
          {volumeError ? (
            <p className="mt-3 px-1 text-[13px] text-muted-foreground" data-volume-note="" title={volumeError}>
              {volumeUnavailableNote(volumeError)}
            </p>
          ) : null}
          {showTeleport ? (
            <div className="mt-7">
              <DropWell
                title="Teleport"
                space={space}
                copy={{ caption: copy?.dropCaption ?? "Drop a file or window", sendFile: copy?.sendFile ?? "Send file…", teleportApp: copy?.teleportApp ?? "Teleport an app…" }}
                onTeleport={teleportApp}
              />
            </div>
          ) : null}
        </div>
        <div>
          {showStream ? (
            <StreamSection
              title="Stream"
              section={stream}
              query={query}
              onQuery={setQuery}
              onPip={(row) => void onPip(row)}
              hideStatus={windowsUnsupported}
            />
          ) : null}
          {showAgents ? (
            <AgentsSection
              title="Agents"
              runs={agents.runs}
              failed={agents.failed}
              copy={{
                loading: copy?.agentsLoading ?? "Looking for this Space’s agents…",
                empty: copy?.agentsEmpty ?? "No agents have been started in this Space.",
                failed: copy?.agentsFailed ?? "Could not read this Space’s agents.",
              }}
            />
          ) : null}
        </div>
      </div>

      <ConfirmDialog
        open={confirm === "stop"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={`Stop ${space.name}?`}
        description={
          space.power?.control === "suspend"
            ? "It's suspended in memory and picks up where it left off when you start it again."
            : "It shuts down. Anything unsaved inside it is lost."
        }
        confirmLabel="Stop"
        destructive={space.power?.control !== "suspend"}
        onConfirm={stop}
      />
      <ConfirmDialog
        open={confirm === "delete"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={detail.confirm.title}
        description={detail.confirm.message}
        confirmLabel={detail.confirm.confirmLabel}
        cancelLabel={detail.confirm.cancelLabel}
        disabledReason={detail.confirm.confirmEnabled ? null : (detail.confirm.disabledReason ?? null)}
        destructive
        onConfirm={() => remove()}
        alternative={detail.confirm.removeLabel ? { label: detail.confirm.removeLabel, onClick: () => remove(true) } : null}
      />
    </Page>
  );
}

/** The Spaces one of your machines provides (the core's `hosted_rows`:
 * those whose host is the machine). */
function hostedSpaces(spaces: readonly Space[], machineSpaceId: string): Space[] {
  if (!machineSpaceId.startsWith("relay:")) return [];
  const machine = machineSpaceId.slice("relay:".length);
  return spaces.filter((s) => s.host === machine);
}

/** A machine that does not share its desktop: the core's line, then the
 * Spaces it provides when it provides any. */
function MachineSpaces({ note, listSpaces, spaces }: { note: string; listSpaces: boolean; spaces: Space[] }) {
  return (
    <section data-machine-desktop-note="">
      <p className="rounded-xl border px-4 py-3 text-[13px] text-muted-foreground">{note}</p>
      {listSpaces ? (
        <div data-machine-spaces="" className="mt-6">
          <h2 className="mb-2 px-1 text-[13px] font-semibold">Spaces</h2>
          {spaces.length === 0 ? (
            <p className="px-1 text-[13px] text-muted-foreground">No Spaces yet</p>
          ) : (
            <ul className="divide-y rounded-xl border">
              {spaces.map((s) => (
                <li key={s.id}>
                  <Link
                    to="/spaces/$spaceId"
                    params={{ spaceId: s.id }}
                    className="flex h-10 items-center gap-2.5 px-4 text-[13px] outline-none hover:bg-muted/60 focus-visible:bg-muted/60"
                  >
                    <OsIcon os={s.os} className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="min-w-0 flex-1 truncate">{s.name}</span>
                    <StateLabel state={spaceState(s)} space={s} />
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </section>
  );
}

function BackLink() {
  return (
    <Link
      to="/spaces"
      className="mb-4 -ml-1 inline-flex h-6 items-center gap-0.5 rounded-md pr-1.5 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      <ChevronLeftIcon className="size-3.5" /> Spaces
    </Link>
  );
}

/** What replaces the stream while there is no desktop to show: a create's
 * progress, a delete, a stopped Space. Undefined: the stream surface. */
function surfaceState(space: Space, pending: string | null, onStart: () => void, onDismiss: () => void, onRetry?: () => void): ReactNode {
  const state = spaceState(space);
  if (state === "failed") {
    return (
      <>
        <p className="text-[13px] font-medium">Couldn't create this Space</p>
        <p data-create-error="" className="mt-2 max-w-md text-center text-xs text-destructive">
          {createError(space) || "No reason was reported."}
        </p>
        <div className="mt-4 flex gap-2">
          {onRetry ? <Button onClick={onRetry}>Try again</Button> : null}
          <Button variant="outline" onClick={onDismiss}>
            Remove from list
          </Button>
        </div>
      </>
    );
  }
  if (state === "creating") {
    return (
      <>
        <p className="text-[13px] font-medium">{space.progress?.label ?? "Creating…"}</p>
        <div className="mt-3 h-1 w-56 overflow-hidden rounded-full bg-foreground/10">
          <div className="h-full rounded-full bg-brand transition-[width]" style={{ width: `${Math.round(createProgress(space) * 100)}%` }} />
        </div>
        {space.progress?.transfer ? <p className="mt-2 text-xs text-muted-foreground tabular-nums">{space.progress.transfer}</p> : null}
        {space.progress?.error ? <p className="mt-2 text-xs text-destructive">{space.progress.error}</p> : null}
      </>
    );
  }
  if (state === "deleting") return <p className="text-[13px] text-muted-foreground">Deleting…</p>;
  if (state === "stopped") {
    return (
      <>
        <OsIcon os={space.os} className="mb-4 size-8 text-muted-foreground/30" />
        <p className="text-[13px] text-muted-foreground">
          {canPower(space) ? "Start this Space to open its desktop." : "This Space is stopped, and it can't be started from this app."}
        </p>
        {canPower(space) ? (
          <Button className="mt-4" variant="outline" disabled={Boolean(pending)} onClick={onStart}>
            <PlayIcon /> {pending ?? "Start"}
          </Button>
        ) : null}
      </>
    );
  }
  return undefined;
}
