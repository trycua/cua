// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { MonitorIcon, PlusIcon, SearchIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { emptyHome, listView, spaceDiskBytes, useBridge, useResources, useFirstSpaceOffers, useMachines, useNewSpaceWizard, useSpaceAccess, useSpaces, type Space } from "@/bridge";
import { EmptyHome } from "@/components/empty-home";
import { useFirstSpaceMilestones } from "@/components/first-run-guide";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { ComputerWarnings } from "@/components/resources/computer-resources";
import type { SpaceItem } from "@/components/spaces/model";
import { BulkConfirmDialog, useBulkRemove } from "@/components/spaces/bulk-remove";
import { SpaceList } from "@/components/spaces/space-list";
import { SpaceContextMenu, SpaceMoreMenu, type MenuActions } from "@/components/spaces/space-menu";
import { SelectAllBox, SelectBox, SelectionBar, stoppedDeletable, useSelection } from "@/components/spaces/selection";
import { SpaceTile, TILE_CELL } from "@/components/spaces/space-tile";
import { CreatingElsewhereTile } from "@/components/spaces/creating-elsewhere-tile";
import { SpacesToolbar } from "@/components/spaces/spaces-toolbar";
import { PageTip, useTipsLoaded } from "@/components/tips";
import { useNewSpace } from "@/hooks/use-new-space";
import { useQuiet } from "@/hooks/use-quiet";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { thisComputerText } from "@/lib/host-labels";
import { plural } from "@/lib/plural";
import { cn } from "@/lib/utils";
import { accessOf, isMine } from "@/lib/space-access";
import { creatingElsewhereLine, machineName, matchesQuery, realSpaces, spaceState } from "@/lib/spaces";
import { computerChoices, gridArea, nextSearch, shownSpaces, spaceMachineId, validateSpacesSearch, type SpacesSearch } from "@/lib/spaces-page";
import { useFlag } from "@/bridge/flags";
import { LegacySpacesPage } from "@/legacy/routes/spaces/index";

export const Route = createFileRoute("/spaces/")({
  component: SpacesPageSwitch,
  // `view`, `sort`, `on`, `state`, `owner`, `select` (lib/spaces-page). The low-disk warning's "See what's using
  // space" opens `?view=list&on=<this computer>&sort=size`; `?sort=size` alone sorts the grid biggest first.
  validateSearch: (s: Record<string, unknown>): SpacesSearch => validateSpacesSearch(s),
});

function SpacesPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const setSearch = (next: Partial<Record<keyof SpacesSearch, string | undefined>>) =>
    void navigate({ search: (prev: SpacesSearch) => nextSearch(prev, next), replace: true });
  const { data: all = [], isLoading, error, listNotice, listStatus, startSpace, stopSpace, creatingElsewhere } = useSpaces();
  // Machines' own desktops are on Machines, not here.
  const spaces = realSpaces(all);
  const { data: machines = [] } = useMachines();
  const resources = useResources();
  const [query, setQuery] = useState("");
  const newSpace = useNewSpace();
  // Asks for the host's options while the list is empty: its architecture
  // decides the macOS tile, and New Space opens with them in.
  useFirstSpaceOffers();
  const { core } = useBridge();
  const { env } = useNewSpaceWizard();
  const home = emptyHome(core, env.hostArch);
  const items = useMemo(
    () =>
      new Map<string, SpaceItem>(
        spaces.map((space) => {
          const machine = machineName(machines, space);
          const mine = isMine(space);
          return [space.id, { space, state: spaceState(space), access: accessOf(space, machine), mine, bytes: mine ? spaceDiskBytes(resources, space.id) : null, machine }];
        }),
      ),
    [spaces, machines, resources],
  );
  const item = (s: Space) => items.get(s.id)!;
  const shown = shownSpaces({
    spaces,
    search,
    mine: (s) => item(s).mine,
    machineOf: (s) => spaceMachineId(s, machines),
    bytes: (s) => item(s).bytes,
    matches: (s) => matchesQuery(core, s, item(s).machine, query),
  }).map(item);
  const filter = search.state ?? "all";
  // Spaces your other computers are creating (CUA-2349): first in the grid, until they are ready.
  const elsewhere =
    filter === "all" || filter === "creating"
      ? creatingElsewhere.filter((c) => matchesQuery(core, { id: `${c.machineId}/${c.name}`, name: c.name, detail: creatingElsewhereLine(c) }, c.machineName, query))
      : [];
  const elsewhereTiles = elsewhere.map((c) => (
    <li key={`elsewhere:${c.machineId}/${c.name}`}>
      <CreatingElsewhereTile creating={c} />
    </li>
  ));
  const keyvault = useSpaceAccess(spaces);
  const running = spaces.filter((s) => spaceState(s) === "running").length;
  // No Spaces yet (or the list could not be read): the SwiftUI app's empty home, as the first-run guide.
  // The core decides: "Create your first Space" only for a complete list with no Space. A daemon still
  // starting (an app update) or computers still answering show what is known and one quiet line instead.
  const listed = listView(core, listStatus, spaces.length);
  // Never read, and the host says why: "Spaces could not be loaded" (the empty home's unread title).
  const unread = !listStatus.loaded && Boolean(listNotice);
  const empty = !error && !isLoading && (listed.empty || unread) && creatingElsewhere.length === 0;
  const guide = empty && home && home.tiles.length > 0 ? home : null;
  const reachingLine = error || unread ? null : listed.reachingLine;
  useFirstSpaceMilestones(spaces);
  const tipsLoaded = useTipsLoaded();
  // The warnings above the list and the list come from separate host reads. Draw them together once those have
  // answered, not one by one with the page jumping at each (CUA-1951).
  const settled = useQuiet([isLoading, listStatus.loaded, all.length, machines.length, resources ? 1 : 0, tipsLoaded ? 1 : 0].join("|"));
  // Picking several, and deleting or leaving them together.
  const present = useMemo(() => new Set(spaces.map((s) => s.id)), [spaces]);
  const clearPicked = useRef(() => {});
  const bulk = useBulkRemove(present, () => clearPicked.current());
  // While the confirm is up, ⌘A and Esc are the dialog's.
  const selection = useSelection(shown, Boolean(bulk.view));
  clearPicked.current = selection.clear;
  const picked = shown.filter((it) => selection.has(it.space.id));
  const power = (list: readonly SpaceItem[], on: boolean) => {
    for (const it of list) void (on ? startSpace : stopSpace)(it.space.id).catch(toastError(`Couldn't turn ${on ? "on" : "off"} ${it.space.name}`));
  };
  const actions: MenuActions = { power: (it, on) => power([it], on), remove: (it) => bulk.ask([it]) };
  // `?select=stopped` (the cleanup tip, a link): every stopped Space shown of yours, once the list is in.
  const ready = !isLoading && shown.length > 0;
  useEffect(() => {
    if (search.select !== "stopped" || !ready) return;
    selection.selectStopped();
    setSearch({ select: undefined });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, when the list is in
  }, [search.select, ready]);
  const view = search.view ?? "grid";
  const sort = search.sort ?? "used";
  const now = Date.now();

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
      <PageTip place="spaces" />
      {/* The page's tip goes here, above the warnings (the tips' own change places it). Then, in this order:
          the computers' warnings, the toolbar, the grid's place. */}
      <ComputerWarnings snapshot={resources} machines={machines} />
      {/* Then the toolbar, and under it the grid's place: the Spaces, or what stands in for them. */}
      {picked.length || bulk.progress ? (
        <SelectionBar picked={picked} progress={bulk.progress} onClear={selection.clear} onPower={(on) => (power(picked, on), selection.clear())} onRemove={() => bulk.ask(picked)} />
      ) : spaces.length + creatingElsewhere.length === 0 ? null : (
        // No Spaces yet: nothing to filter, sort or switch, so no toolbar (it comes with the first Space).
        <SpacesToolbar
          state={filter}
          onState={(state) => setSearch({ state: state === "all" ? undefined : state })}
          owner={search.owner}
          onOwner={(owner) => setSearch({ owner })}
          computers={computerChoices(machines)}
          on={search.on}
          onComputer={(on) => setSearch({ on })}
          sort={sort}
          onSort={(s) => setSearch({ sort: s === "used" ? undefined : s })}
          view={view}
          onView={(v) => setSearch({ view: v === "grid" ? undefined : v })}
          query={query}
          onQuery={setQuery}
          lead={
            view === "list" && shown.some(stoppedDeletable) ? (
              <button
                type="button"
                data-select-stopped=""
                onClick={selection.selectStopped}
                className="h-7 cursor-default rounded-lg px-2 text-xs font-medium text-brand-strong outline-none hover:bg-tile focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                Select stopped
              </button>
            ) : null
          }
        />
      )}
      {guide ? <EmptyHome home={guide} unread={Boolean(listNotice)} /> : null}
      {(() => {
        switch (gridArea({ error: Boolean(error), loading: isLoading, guide: Boolean(guide), total: spaces.length + creatingElsewhere.length, shown: shown.length + elsewhere.length, query })) {
          case "error":
            return (
              <EmptyState icon={<MonitorIcon />} title="Couldn't load Spaces">
                {error?.message}
              </EmptyState>
            );
          case "loading":
            return null;
          case "guide":
            return null;
          case "empty":
            // Computers still answering: no "No Spaces yet" until the list is complete.
            if (!empty) return null;
            return (
              <EmptyState
                icon={<MonitorIcon />}
                title="No Spaces yet"
                action={
                  <Button onClick={newSpace}>
                    <PlusIcon /> New Space
                  </Button>
                }
              >
                Create a Space on {thisComputerText()}.
              </EmptyState>
            );
          case "noMatches":
            return (
              <EmptyState icon={<SearchIcon />} title="No Spaces found">
                Try a different name or state.
              </EmptyState>
            );
          case "noneInFilter":
            return (
              <EmptyState icon={<MonitorIcon />} title={`No ${filter === "all" ? "" : `${filter} `}Spaces`}>
                Choose another state to see more Spaces.
              </EmptyState>
            );
          case "spaces":
            return view === "list" ? (
              <>
              {elsewhereTiles.length ? <ul className="mb-6 grid grid-cols-[repeat(auto-fill,minmax(232px,1fr))] gap-x-5 gap-y-7">{elsewhereTiles}</ul> : null}
              <SpaceList
                items={shown}
                sort={sort}
                onSort={(s) => setSearch({ sort: s === "used" ? undefined : s })}
                now={now}
                selection={{
                  header: <SelectAllBox items={shown} selection={selection} />,
                  row: (it) => <SelectBox item={it} selection={selection} />,
                  selected: (it) => selection.has(it.space.id),
                  busy: (it) => bulk.running.has(it.space.id),
                  onClick: selection.onClick,
                }}
                menu={(it, row) => (
                  <SpaceContextMenu item={it} actions={actions}>
                    {row}
                  </SpaceContextMenu>
                )}
                more={(it) => <SpaceMoreMenu item={it} actions={actions} />}
              />
              </>
            ) : (
              <ul className="grid grid-cols-[repeat(auto-fill,minmax(232px,1fr))] gap-x-5 gap-y-7">
                {elsewhereTiles}
                {shown.map((it) => (
                  // Off-screen tiles skip layout and paint until they scroll in (with room for the selected ring).
                  <li key={it.space.id} className={TILE_CELL}>
                    <SpaceContextMenu item={it} actions={actions}>
                      <SpaceTile
                        item={it}
                        accessKey={keyvault.signedIn.has(it.space.id) ? keyvault.accessKey(it.space.id) : undefined}
                        selected={selection.has(it.space.id)}
                        busy={bulk.running.has(it.space.id)}
                        onClick={(e) => selection.onClick(it, e)}
                        topLeft={
                          <SelectBox
                            item={it}
                            selection={selection}
                            onPicture
                            className={selection.ids.size ? undefined : "opacity-0 group-focus-within/tile:opacity-100 group-hover/tile:opacity-100"}
                          />
                        }
                        more={<SpaceMoreMenu item={it} actions={actions} />}
                      />
                    </SpaceContextMenu>
                  </li>
                ))}
              </ul>
            );
        }
      })()}
      {reachingLine && !isLoading ? (
        <p data-spaces-reaching="" role="status" className={cn("px-1 text-xs text-muted-foreground", spaces.length > 0 ? "mt-4" : "py-6 text-center")}>
          {reachingLine}
        </p>
      ) : null}
        </>
      ) : null}
      {bulk.view ? <BulkConfirmDialog view={bulk.view} items={bulk.asked} onCancel={bulk.cancel} onConfirm={() => void bulk.confirm()} /> : null}
    </Page>
  );
}

/** The redesign behind `spaces_redesign` (Settings → Advanced → Labs); off, the page as it was. */
function SpacesPageSwitch() {
  return useFlag("spaces_redesign") ? <SpacesPage /> : <LegacySpacesPage />;
}
