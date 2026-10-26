// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { ArrowDownCircleIcon, KeyRoundIcon, LoaderCircleIcon, MonitorIcon, PlusIcon, PowerIcon, SearchIcon } from "lucide-react";
import { useEffect, useRef, useState, type DragEvent } from "react";

import { emptyHome, listView, spaceResources, type CreatingElsewhere, useAgents, useBridge, useChecklist, useResources, useFirstSpaceOffers, useMachines, useNewSpaceWizard, useSpaceAccess, useSpaceFiles, useSpaces, useSpaceThumbnail, useWindowDropTarget, type Space } from "@/bridge";
import { EmptyHome } from "@/components/empty-home";
import { RunAgentDialog } from "@/components/agents/run-agent-dialog";
import { SpaceReadyCard, useAgentOffer } from "@/components/space-ready-card";
import { useFirstRunStep, useFirstSpaceMilestones } from "@/components/first-run-guide";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { ComputerWarnings } from "@/components/resources/computer-resources";
import { SpaceCanvas } from "@/components/space-canvas";
import { StateLabel } from "@/components/state-dot";
import { useTileVideo } from "@/components/video/tile-video";
import { useNewSpace } from "@/hooks/use-new-space";
import { useInView } from "@/hooks/use-in-view";
import { useQuiet } from "@/hooks/use-quiet";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { toast, toastError } from "@/components/ui/toast";
import { creatingLine, thisComputerText } from "@/lib/host-labels";
import { plural } from "@/lib/plural";
import { cn } from "@/lib/utils";
import { createError, createProgress, creatingElsewhereLine, creatingElsewherePercent, creatingElsewherePlace, elsewhereOs, machineName, matchesQuery, powerButton, powerError, realSpaces, spacePlace, spaceState, visibleSpaces, type SpaceFilter } from "@/lib/spaces";

export const Route = createFileRoute("/spaces/")({
  component: SpacesPage,
  // `?sort=size`: biggest on this Mac's disk first (the low-disk warning's "See what's using space").
  validateSearch: (s: Record<string, unknown>): { sort?: "size" } => (s.sort === "size" ? { sort: "size" } : {}),
});

const FILTERS: { value: SpaceFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "stopped", label: "Stopped" },
  { value: "creating", label: "Creating" },
];

function SpacesPage() {
  const { data: all = [], isLoading, error, listNotice, listStatus, creatingElsewhere } = useSpaces();
  // Machines' own desktops are on Machines, not here.
  const spaces = realSpaces(all);
  const { data: machines = [] } = useMachines();
  const { sort } = Route.useSearch();
  const resources = useResources();
  const [filter, setFilter] = useState<SpaceFilter>("all");
  const [query, setQuery] = useState("");
  const newSpace = useNewSpace();
  // Asks for the host's options while the list is empty: its architecture
  // decides the macOS tile, and New Space opens with them in.
  useFirstSpaceOffers();
  const { core } = useBridge();
  const { env } = useNewSpaceWizard();
  const home = emptyHome(core, env.hostArch);
  const visible = visibleSpaces(spaces, filter, (s) => matchesQuery(core, s, machineName(machines, s), query));
  const onDisk = (s: Space) => spaceResources(resources, s.id)?.diskOnHost?.bytes ?? -1;
  const shown = sort === "size" ? [...visible].sort((a, b) => onDisk(b) - onDisk(a)) : visible;
  // Spaces your other computers are creating (CUA-2349): first in the grid, until they are ready.
  const elsewhere =
    filter === "all" || filter === "creating"
      ? creatingElsewhere.filter((c) => matchesQuery(core, { id: `${c.machineId}/${c.name}`, name: c.name, detail: creatingElsewhereLine(c) }, c.machineName, query))
      : [];
  const access = useSpaceAccess(spaces);
  const running = spaces.filter((s) => spaceState(s) === "running").length;
  // After the first run's Finish later, its to-do list stands in for the checklist.
  // The first Space is ready and no agent has worked in it yet: one action, "Run an agent here".
  const { state: checklist } = useChecklist();
  const { data: agents, isLoading: agentsLoading } = useAgents();
  const readySpace = spaces.find((s) => spaceState(s) === "running");
  const agentRan = checklist.done.run_agent || (agents?.runs.length ?? 0) > 0 || (agents?.agents.length ?? 0) > 0;
  // Only once the agents are read: before that "no agent ran" isn't known, and the offer would show (and be
  // counted as shown) for a moment on a computer whose agents already work.
  const offer = useAgentOffer(Boolean(readySpace) && !agentsLoading && !agentRan);
  const runHere = readySpace && !agentRan && offer.visible ? readySpace : undefined;
  const [runOpen, setRunOpen] = useState(false);
  // The core decides: "Create your first Space" only for a complete list with no Space. A daemon still
  // starting (an app update) or computers still answering show what is known and one quiet line instead.
  const view = listView(core, listStatus, spaces.length);
  // Never read, and the host says why: "Spaces could not be loaded" (the empty home's unread title).
  const unread = !listStatus.loaded && Boolean(listNotice);
  const empty = !error && !isLoading && (view.empty || unread) && creatingElsewhere.length === 0;
  // No Spaces yet (or the list could not be read): the SwiftUI app's empty home, as the first-run guide.
  const guide = empty && home && home.tiles.length > 0 ? home : null;
  const reachingLine = error || unread ? null : view.reachingLine;
  // The suggested task's request, while the run dialog was opened from it.
  const [runPrompt, setRunPrompt] = useState<string | undefined>();
  const firstRunStep = useFirstRunStep();
  useFirstSpaceMilestones(spaces);
  const navigate = useNavigate();
  // The cards above the list (ready, checklist, computers) and the list come from separate host reads. Draw them
  // together once those have answered, not one by one with the page jumping at each (CUA-1951).
  const settled = useQuiet(
    [isLoading, listStatus.loaded, all.length, machines.length, Object.keys(checklist.done).length, checklist.dismissed, agents?.runs.length, agents?.agents.length, resources ? 1 : 0].join("|"),
  );

  return (
    <Page>
      <PageHeader
        title="Spaces"
        description={isLoading || !listStatus.loaded ? "Loading Spaces…" : `${plural(spaces.length, "Space")} on ${plural(machines.length, "computer")}, ${running} running`}
        actions={
          <Button onClick={newSpace}>
            <PlusIcon /> New Space
          </Button>
        }
      />
      {settled ? (
        <>
      {runHere ? <SpaceReadyCard
          space={runHere}
          onRun={(task) => {
            offer.clicked();
            firstRunStep("task_started");
            setRunPrompt(task.prompt);
            setRunOpen(true);
          }}
          onDismiss={offer.dismiss}
        /> : null}
      {readySpace ? (
        <>
          <RunAgentDialog open={runOpen} space={readySpace} prompt={runPrompt} onOpenChange={setRunOpen} onManual={() => void navigate({ to: "/settings/connected-apps" })} onConnected={offer.completed} />
        </>
      ) : null}
      <ComputerWarnings snapshot={resources} machines={machines} />
      {sort === "size" ? (
        <p data-sorted-by-size="" className="mb-3 flex items-center gap-2 px-1 text-xs text-muted-foreground">
          Biggest first, by how much space each one uses on its computer.
          <Link to="/spaces" search={{}} className="font-medium text-foreground underline-offset-2 hover:underline">
            Show the usual order
          </Link>
        </p>
      ) : null}
      <div className="mb-4 flex items-center gap-3">
        <Segmented aria-label="Filter by state" value={filter} options={FILTERS} onValueChange={setFilter} />
        {/* Find a Space by name, detail, id or machine (the SwiftUI sidebar's search). */}
        <div className="relative ml-auto w-56">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape" && query) {
                e.preventDefault();
                setQuery("");
              }
            }}
            placeholder="Filter"
            aria-label="Filter Spaces"
            data-spaces-filter=""
            className="pl-8"
          />
        </div>
      </div>
      {/* The empty home and the "Reaching N computers…" line take the grid's place, under the warnings and the
          filters. */}
      {error ? (
        <EmptyState icon={<MonitorIcon />} title="Couldn't load Spaces">
          {error.message}
        </EmptyState>
      ) : isLoading ? null : shown.length + elsewhere.length > 0 ? (
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(232px,1fr))] gap-4">
          {/* Being created, first: as this computer's own creates sort. */}
          {elsewhere.map((c) => (
            <li key={`elsewhere:${c.machineId}/${c.name}`}>
              <CreatingElsewhereTile creating={c} />
            </li>
          ))}
          {shown.map((space) => (
            // Off-screen tiles skip layout and paint until they scroll in.
            <li key={space.id} className="[content-visibility:auto] [contain-intrinsic-size:auto_232px]">
              <SpaceTile space={space} machine={machineName(machines, space)} accessKey={access.signedIn.has(space.id) ? access.accessKey(space.id) : undefined} />
            </li>
          ))}
        </ul>
      ) : guide ? (
        <EmptyHome home={guide} unread={Boolean(listNotice)} />
      ) : spaces.length === 0 && !empty ? null : spaces.length === 0 ? (
        <EmptyState
          icon={<MonitorIcon />}
          title="No Spaces yet"
          action={
            <Button onClick={newSpace}>
              <PlusIcon /> New Space
            </Button>
          }
        >
          Create one on {thisComputerText()}.
        </EmptyState>
      ) : shown.length === 0 && query.trim() ? (
        <EmptyState icon={<SearchIcon />} title="No matches">
          No {filter === "all" ? "" : `${filter} `}Space matches “{query.trim()}”.
        </EmptyState>
      ) : shown.length === 0 ? (
        <EmptyState icon={<MonitorIcon />} title={`No ${filter} Spaces`}>
          Pick another filter to see the rest.
        </EmptyState>
      ) : null}
      {reachingLine && !isLoading ? (
        <p data-spaces-reaching="" role="status" className={cn("px-1 text-xs text-muted-foreground", spaces.length > 0 ? "mt-4" : "py-6 text-center")}>
          {reachingLine}
        </p>
      ) : null}
        </>
      ) : null}
    </Page>
  );
}

function SpaceTile({ space, machine, accessKey }: { space: Space; machine: string; accessKey?: string | null }) {
  const drop = useTileDrop(space);
  // A real window dragged over this tile (the host hit-tests the tiles' boxes and says which; macOS only).
  const windowOver = useWindowDropTarget() === space.id;
  return (
    <div
      className="group/tile relative"
      data-tile-drop={drop.over ?? undefined}
      data-drop-status={drop.status?.kind}
      data-window-drop={space.id}
      data-window-over={windowOver || undefined}
      {...drop.handlers}
    >
      <SpaceLink space={space} machine={machine} windowOver={windowOver} />
      {drop.over ? <DropOverlay space={space} accepts={drop.over === "accept"} /> : null}
      {/* A drop on the tile still going in: the core's "Sending …" line. */}
      {drop.status?.kind === "working" && !drop.over ? (
        <div className="pointer-events-none absolute top-3 left-3 flex max-w-[60%] items-center gap-1.5 rounded-full border bg-card/90 px-2 py-0.5 text-2xs font-medium text-muted-foreground shadow-xs backdrop-blur-sm">
          <LoaderCircleIcon className="size-3 shrink-0 animate-spin" aria-hidden />
          <span className="truncate">{drop.status.text}</span>
        </div>
      ) : null}
      {/* Over the thumbnail, as the SwiftUI sidebar row ends: "Signed in", then power. */}
      <div className="absolute top-3 right-3 flex items-center gap-1.5">
        {/* A live Keyvault sign-in is in this Space: the badge opens Access with its row brought forward. */}
        {accessKey !== undefined ? (
          <Link
            to="/keyvault"
            search={{ view: "access", focus: accessKey ?? undefined }}
            data-signed-in={space.id}
            title="A Keyvault sign-in is live in this Space. Open Access."
            className="inline-flex items-center gap-1 rounded-full border bg-card/90 px-2 py-0.5 text-2xs font-medium text-muted-foreground shadow-xs outline-none backdrop-blur-sm hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
          >
            <KeyRoundIcon className="size-3" /> Signed in
          </Link>
        ) : null}
        <TilePower space={space} />
      </div>
    </div>
  );
}

/**
 * Files dropped from Finder on a running Space's tile go into that Space,
 * as a drop on its page's well does (`useSpaceFiles`: the host reads the
 * paths from the drop itself; an app bundle opens Teleport at that app).
 * While files are over the tile it says where they go; what the drop did
 * shows on the tile and in a toast.
 */
function useTileDrop(space: Space) {
  const files = useSpaceFiles(space);
  const [over, setOver] = useState<"accept" | "refuse" | null>(null);
  const accepts = spaceState(space) === "running";
  const status = files.status;
  useEffect(() => {
    if (!status) return;
    const group = `tile-drop:${space.id}`;
    if (status.kind === "working") toast(status.text, { group });
    else if (status.kind === "done") toast(`Sent to ${space.name}`, { description: status.text, type: "success", group });
    else toast(`Couldn't send to ${space.name}`, { description: status.text, type: "error", group });
  }, [status, space.id, space.name]);
  const carriesFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");
  const onDragOver = (e: DragEvent) => {
    if (!carriesFiles(e)) return;
    // Always handled here, so a drop on a tile never reaches the window.
    e.preventDefault();
    e.dataTransfer.dropEffect = accepts ? "copy" : "none";
    setOver(accepts ? "accept" : "refuse");
  };
  const onDragLeave = (e: DragEvent) => {
    // Leaving for one of the tile's own children is still over the tile.
    if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
    setOver(null);
  };
  const onDrop = (e: DragEvent) => {
    if (!carriesFiles(e)) return;
    e.preventDefault();
    setOver(null);
    if (!accepts) return;
    const names = Array.from(e.dataTransfer.files).map((f) => f.name);
    if (names.length) void files.drop(names);
  };
  return { over, status, handlers: { onDragEnter: onDragOver, onDragOver, onDragLeave, onDrop } };
}

/** Over a tile while files are dragged on it: where they go, or why they can't. */
function DropOverlay({ space, accepts }: { space: Space; accepts: boolean }) {
  return (
    <div
      aria-hidden
      className={cn(
        "pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-1.5 rounded-xl border-2 text-center text-[13px] backdrop-blur-[2px]",
        accepts ? "border-brand bg-brand/15 text-foreground" : "border-dashed border-border bg-card/80 text-muted-foreground",
      )}
    >
      {accepts ? <ArrowDownCircleIcon className="size-6 text-brand" strokeWidth={1.5} /> : null}
      <span className="max-w-[85%] truncate font-medium">{accepts ? `Send to ${space.name}` : `${space.name} isn't running`}</span>
    </div>
  );
}

/**
 * The core's power button, while the pointer is on the tile or focus is in
 * it, and always while it runs (the SwiftUI sidebar row's, which shows on
 * hover, on the selected row and while busy).
 */
function TilePower({ space }: { space: Space }) {
  const { core } = useBridge();
  const { startSpace, stopSpace } = useSpaces();
  const button = powerButton(core, space);
  if (!button) return null;
  const press = () => {
    const [act, word] = button.turnOn ? [startSpace, "start"] : [stopSpace, "stop"];
    void act(space.id).catch(toastError(`Couldn't ${word} ${space.name}`));
  };
  return (
    <button
      type="button"
      data-tile-power={button.turnOn ? "on" : "off"}
      data-busy={button.busy || undefined}
      disabled={button.busy}
      title={button.help}
      aria-label={`${button.help} ${space.name}`}
      onClick={press}
      className={cn(
        "inline-flex size-6 items-center justify-center rounded-full border bg-card/90 text-muted-foreground shadow-xs outline-none backdrop-blur-sm hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 disabled:cursor-default",
        !button.busy && "opacity-0 group-focus-within/tile:opacity-100 group-hover/tile:opacity-100",
      )}
    >
      {button.busy ? <LoaderCircleIcon className="size-3 animate-spin" aria-hidden /> : <PowerIcon className="size-3" aria-hidden />}
    </button>
  );
}

function SpaceLink({ space, machine, windowOver = false }: { space: Space; machine: string; windowOver?: boolean }) {
  return (
    <Link
      to="/spaces/$spaceId"
      params={{ spaceId: space.id }}
      data-space-id={space.id}
      data-state={spaceState(space)}
      className={cn(
        "group block w-full cursor-default rounded-xl border bg-card p-1.5 text-left shadow-xs outline-none transition-[box-shadow,border-color] hover:border-foreground/15 hover:shadow-sm focus-visible:border-brand focus-visible:ring-3 focus-visible:ring-brand/25",
        windowOver && "border-brand ring-3 ring-brand/25",
      )}
    >
      <Thumbnail space={space} />
      <div className="flex items-start gap-2.5 px-1.5 pt-2.5 pb-1">
        <OsIcon os={space.os} className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium">{space.name}</div>
          {/* Being created: the core's pending line ("This Mac · Downloading"), as the SwiftUI row says it, naming this computer by its system. */}
          <div className="mt-0.5 truncate text-xs text-muted-foreground">
            {spaceState(space) === "creating" && space.detail ? creatingLine(space.detail) : spacePlace(space, machine)}
          </div>
          {/* Downloading: how far, how fast, how long is left. */}
          {spaceState(space) === "creating" && space.progress?.transfer ? (
            <div data-create-transfer="" className="mt-0.5 truncate text-xs tabular-nums text-muted-foreground">
              {space.progress.transfer}
            </div>
          ) : null}
          {createError(space) ? (
            <p data-create-error="" title={createError(space)!} className="mt-1 line-clamp-2 text-xs text-destructive">
              {createError(space)}
            </p>
          ) : powerError(space) ? (
            // The last start or stop failed: why, inline, as the SwiftUI sidebar row says it.
            <p data-power-error="" title={powerError(space)!} className="mt-1 line-clamp-2 text-xs text-destructive">
              {powerError(space)}
            </p>
          ) : null}
        </div>
        <StateLabel state={spaceState(space)} space={space} className="mt-px shrink-0" />
      </div>
      {/* What it was given and uses now, when its computer says. */}
    </Link>
  );
}

/**
 * A Space another of your computers is creating (CUA-2349): the same tile
 * as one this computer creates, with "Creating macOS on Mac mini… 40%" from
 * that computer's own report. Nothing to open yet; once it is ready it is a
 * Space like any other (or that computer's count, when its Spaces are not
 * shared).
 */
function CreatingElsewhereTile({ creating }: { creating: CreatingElsewhere }) {
  const os = elsewhereOs(creating);
  const line = creatingElsewhereLine(creating);
  return (
    <div data-creating-elsewhere={creating.machineId} data-state="creating" role="group" aria-label={`${creating.name}: ${line}`} className="block w-full rounded-xl border bg-card p-1.5 text-left shadow-xs">
      <SpaceCanvas>
        <OsIcon os={os} className="size-6 text-muted-foreground/30" />
        <div className="absolute inset-x-3 bottom-3">
          <div className="h-1 overflow-hidden rounded-full bg-foreground/10">
            <div data-create-progress className="h-full rounded-full bg-brand transition-[width] duration-500" style={{ width: `${Math.min(100, Math.max(0, creating.percent))}%` }} />
          </div>
        </div>
      </SpaceCanvas>
      <div className="flex items-start gap-2.5 px-1.5 pt-2.5 pb-1">
        <OsIcon os={os} className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium">{creating.name}</div>
          <div className="mt-0.5 truncate text-xs text-muted-foreground" title={line}>
            {creatingElsewherePlace(creating)}
          </div>
        </div>
        {/* How far, where this computer's own creates say "Creating". */}
        <span data-state-label="creating" className="mt-px inline-flex shrink-0 items-center gap-1.5 text-xs tabular-nums text-muted-foreground">
          <span className="size-1.5 animate-pulse rounded-full bg-brand" aria-hidden />
          {creatingElsewherePercent(creating)}
        </span>
      </div>
    </div>
  );
}

function Thumbnail({ space }: { space: Space }) {
  const state = spaceState(space);
  const ref = useRef<HTMLDivElement | null>(null);
  // Pictures and live video only for the tiles on screen (or close to it).
  const inView = useInView(ref);
  const video = useTileVideo(ref, space, inView);
  // Its latest capture (the notch's image), until live video draws over it.
  const thumbnail = useSpaceThumbnail(space.id, inView && (state === "running" || state === "stopped"));
  return (
    <SpaceCanvas ref={ref} dim={state === "stopped"} streamState={video?.phase}>
      {thumbnail ? (
        <img data-tile-thumbnail="" src={thumbnail} alt="" aria-hidden className="absolute inset-0 size-full object-cover" />
      ) : (
        <OsIcon os={space.os} className="size-6 text-muted-foreground/30" />
      )}
      {state === "creating" ? (
        <div className="absolute inset-x-3 bottom-3">
          <div className="h-1 overflow-hidden rounded-full bg-foreground/10">
            <div data-create-progress className="h-full rounded-full bg-brand" style={{ width: `${Math.round(createProgress(space) * 100)}%` }} />
          </div>
        </div>
      ) : null}
    </SpaceCanvas>
  );
}
