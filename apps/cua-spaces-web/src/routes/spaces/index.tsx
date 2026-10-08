// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link } from "@tanstack/react-router";
import { KeyRoundIcon, MonitorIcon, PlusIcon } from "lucide-react";
import { useRef, useState } from "react";

import { emptyHome, useBridge, useFirstSpaceOffers, useMachines, useNewSpaceWizard, useSpaceAccess, useSpaces, useSpaceThumbnail, type Space } from "@/bridge";
import { EmptyHome } from "@/components/empty-home";
import { OsIcon } from "@/components/os-icon";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { SpaceCanvas } from "@/components/space-canvas";
import { StateLabel } from "@/components/state-dot";
import { useTileVideo } from "@/components/video/tile-video";
import { useNewSpace } from "@/hooks/use-new-space";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { thisComputerText } from "@/lib/host-labels";
import { plural } from "@/lib/plural";
import { createError, createProgress, machineName, realSpaces, spacePlace, spaceState, visibleSpaces, type SpaceFilter } from "@/lib/spaces";

export const Route = createFileRoute("/spaces/")({ component: SpacesPage });

const FILTERS: { value: SpaceFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "stopped", label: "Stopped" },
  { value: "creating", label: "Creating" },
];

function SpacesPage() {
  const { data: all = [], isLoading, error, listNotice } = useSpaces();
  // Machines' own desktops are on Machines, not here.
  const spaces = realSpaces(all);
  const { data: machines = [] } = useMachines();
  const [filter, setFilter] = useState<SpaceFilter>("all");
  const newSpace = useNewSpace();
  // Asks for the host's options while the list is empty: its architecture
  // decides the macOS tile, and New Space opens with them in.
  useFirstSpaceOffers();
  const { core } = useBridge();
  const { env } = useNewSpaceWizard();
  const home = emptyHome(core, env.hostArch);
  const shown = visibleSpaces(spaces, filter);
  const access = useSpaceAccess(spaces);
  const running = spaces.filter((s) => spaceState(s) === "running").length;

  return (
    <Page>
      <PageHeader
        title="Spaces"
        description={isLoading ? "Loading Spaces…" : `${plural(spaces.length, "Space")} on ${plural(machines.length, "machine")}, ${running} running`}
        actions={
          <Button onClick={newSpace}>
            <PlusIcon /> New Space
          </Button>
        }
      />
      <div className="mb-4">
        <Segmented aria-label="Filter by state" value={filter} options={FILTERS} onValueChange={setFilter} />
      </div>
      {error ? (
        <EmptyState icon={<MonitorIcon />} title="Couldn't load Spaces">
          {error.message}
        </EmptyState>
      ) : isLoading ? null : spaces.length === 0 && home && home.tiles.length > 0 ? (
        // No Spaces yet (or the list could not be read): the SwiftUI app's
        // empty home, under the header.
        <EmptyHome home={home} unread={Boolean(listNotice)} />
      ) : spaces.length === 0 ? (
        <EmptyState
          icon={<MonitorIcon />}
          title="No Spaces yet"
          action={
            <Button onClick={newSpace}>
              <PlusIcon /> New Space
            </Button>
          }
        >
          Create one on {thisComputerText()} or on Cua Cloud.
        </EmptyState>
      ) : shown.length === 0 ? (
        <EmptyState icon={<MonitorIcon />} title={`No ${filter} Spaces`}>
          Pick another filter to see the rest.
        </EmptyState>
      ) : (
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(232px,1fr))] gap-4">
          {shown.map((space) => (
            <li key={space.id}>
              <SpaceTile space={space} machine={machineName(machines, space)} accessKey={access.signedIn.has(space.id) ? access.accessKey(space.id) : undefined} />
            </li>
          ))}
        </ul>
      )}
    </Page>
  );
}

function SpaceTile({ space, machine, accessKey }: { space: Space; machine: string; accessKey?: string | null }) {
  return (
    <div className="relative">
      <SpaceLink space={space} machine={machine} />
      {/* A live Keyvault sign-in is in this Space: the badge opens Access with its row brought forward. */}
      {accessKey !== undefined ? (
        <Link
          to="/keyvault"
          search={{ view: "access", focus: accessKey ?? undefined }}
          data-signed-in={space.id}
          title="A Keyvault sign-in is live in this Space. Open Access."
          className="absolute top-3 right-3 inline-flex items-center gap-1 rounded-full border bg-card/90 px-2 py-0.5 text-2xs font-medium text-muted-foreground shadow-xs outline-none backdrop-blur-sm hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          <KeyRoundIcon className="size-3" /> Signed in
        </Link>
      ) : null}
    </div>
  );
}

function SpaceLink({ space, machine }: { space: Space; machine: string }) {
  return (
    <Link
      to="/spaces/$spaceId"
      params={{ spaceId: space.id }}
      data-space-id={space.id}
      data-state={spaceState(space)}
      className="group block w-full cursor-default rounded-xl border bg-card p-1.5 text-left shadow-xs outline-none transition-[box-shadow,border-color] hover:border-foreground/15 hover:shadow-sm focus-visible:border-brand focus-visible:ring-3 focus-visible:ring-brand/25"
    >
      <Thumbnail space={space} />
      <div className="flex items-start gap-2.5 px-1.5 pt-2.5 pb-1">
        <OsIcon os={space.os} className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[13px] font-medium">{space.name}</div>
          {/* Being created: the core's pending line ("This Mac · Downloading"), as the SwiftUI row says it. */}
          <div className="mt-0.5 truncate text-xs text-muted-foreground">
            {spaceState(space) === "creating" && space.detail ? space.detail : spacePlace(space, machine)}
          </div>
          {createError(space) ? (
            <p data-create-error="" title={createError(space)!} className="mt-1 line-clamp-2 text-xs text-destructive">
              {createError(space)}
            </p>
          ) : null}
        </div>
        <StateLabel state={spaceState(space)} space={space} className="mt-px shrink-0" />
      </div>
    </Link>
  );
}

function Thumbnail({ space }: { space: Space }) {
  const state = spaceState(space);
  const ref = useRef<HTMLDivElement | null>(null);
  const video = useTileVideo(ref, space);
  // Its latest capture (the notch's image), until live video draws over it.
  const thumbnail = useSpaceThumbnail(space.id, state === "running" || state === "stopped");
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
