// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";
import { MonitorIcon, PlusIcon, SearchIcon } from "lucide-react";
import { memo, useEffect, useMemo, useRef, useState, type RefObject } from "react";

import { emptyHome, listView, spaceDiskBytes, useBridge, useResources, useFirstSpaceOffers, useMachines, useNewSpaceWizard, useSpaceAccess, useSpaces, type Space } from "@/bridge";
import { EmptyHome } from "@/components/empty-home";
import { useFirstSpaceMilestones } from "@/components/first-run-guide";
import { EmptyState, Page, PageHeader } from "@/components/page";
import { ComputerWarnings } from "@/components/resources/computer-resources";
import { useListedSpaces } from "@/components/spaces/listed";
import { desktopOf, hasHideButton, hasMenu, type SpaceItem } from "@/components/spaces/model";
import { BulkConfirmDialog, useBulkRemove } from "@/components/spaces/bulk-remove";
import { SpaceList } from "@/components/spaces/space-list";
import { SpaceContextMenu, SpaceMoreMenu, type MenuActions } from "@/components/spaces/space-menu";
import { SelectAllBox, SelectBox, SelectionBar, stoppedDeletable, useSelection } from "@/components/spaces/selection";
import { SpaceTile, TILE_CELL } from "@/components/spaces/space-tile";
import { CreatingElsewhereTile } from "@/components/spaces/creating-elsewhere-tile";
import { WindowedGrid, type WindowedGridHandle } from "@/components/spaces/windowed-grid";
import { SpacesToolbar } from "@/components/spaces/spaces-toolbar";
import { PageTip, useTipsLoaded } from "@/components/tips";
import { useNewSpace } from "@/hooks/use-new-space";
import { useQuiet } from "@/hooks/use-quiet";
import { NewSpaceButton } from "@/components/new-space/new-space-button";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";
import { thisComputerText } from "@/lib/host-labels";
import { plural } from "@/lib/plural";
import { cn } from "@/lib/utils";
import { accessOf, isMine } from "@/lib/space-access";
import { creatingElsewhereLine, machineName, matchesQuery, spaceState } from "@/lib/spaces";
import { hideDesktop, showHiddenDesktops } from "@/lib/hidden-desktops";
import { computerChoices, gridArea, nextSearch, shownSpaces, spaceMachineId, validateSpacesSearch, type SpacesSearch } from "@/lib/spaces-page";

export const Route = createFileRoute("/spaces/")({
  component: SpacesPage,
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
  // Your Spaces and the machines' desktops: this Mac's reads "Not sharing" while its switch is off (never counted).
  // Stable while the list is: the tiles below redraw only when their own item changes.
  const { listed: spaces, counted, hiddenCount } = useListedSpaces(all);
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
          return [space.id, { space, state: spaceState(space), access: accessOf(space, machine), mine, bytes: mine ? spaceDiskBytes(resources, space.id) : null, machine, desktop: desktopOf(space) }];
        }),
      ),
    [spaces, machines, resources],
  );
  const item = (s: Space) => items.get(s.id)!;
  // Only when the list, the filters or the numbers change: a pick or a menu redraws the page without
  // filtering and sorting thousands of Spaces again.
  const shown = useMemo(
    () =>
      shownSpaces({
        spaces,
        search,
        mine: (s) => item(s).mine,
        machineOf: (s) => spaceMachineId(s, machines),
        bytes: (s) => item(s).bytes,
        matches: (s) => matchesQuery(core, s, item(s).machine, query),
      }).map(item),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `item` reads `items`
    [spaces, search, items, machines, query, core],
  );
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
  const running = counted.filter((s) => spaceState(s) === "running").length;
  // No Spaces yet (or the list could not be read): the SwiftUI app's empty home, as the first-run guide.
  // The core decides: "Create your first Space" only for a complete list with no Space. A daemon still
  // starting (an app update) or computers still answering show what is known and one quiet line instead.
  const listed = listView(core, listStatus, spaces.length);
  // Never read, and the host says why: "Spaces could not be loaded" (the empty home's unread title).
  const unread = !listStatus.loaded && Boolean(listNotice);
  const empty = !error && !isLoading && (listed.empty || unread) && creatingElsewhere.length === 0;
  const guide = empty && home && home.tiles.length > 0 ? home : null;
  const reachingLine = error || unread ? null : listed.reachingLine;
  useFirstSpaceMilestones(counted);
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
  const actions: MenuActions = { power: (it, on) => power([it], on), remove: (it) => bulk.ask([it]), hide: (it) => hideDesktop(it.space.id) };
  // The grid's tiles read the selection and the menus' actions through this,
  // so a tile redraws only when its own Space, pick or state changes (picking
  // one of 3,000 Spaces redrew all 3,000).
  const grid = useRef<WindowedGridHandle>(null);
  const live = useRef<GridLive>({ selection, actions });
  live.current = { selection, actions };
  // `?select=stopped` (the cleanup tip, a link): every stopped Space shown of yours, once the list is in.
  const ready = !isLoading && shown.length > 0;
  useEffect(() => {
    if (search.select !== "stopped" || !ready) return;
    selection.selectStopped();
    // The first one picked, on screen (a long grid draws only what is on screen).
    const first = shown.findIndex(stoppedDeletable);
    if (first >= 0) grid.current?.scrollToIndex(elsewhereTiles.length + first);
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
        description={isLoading || !listStatus.loaded ? "Loading Spaces…" : `${plural(counted.length, "Space")} on ${plural(machines.length, "machine")}, ${running} running`}
        actions={
          <NewSpaceButton />
        }
      />
      {settled ? (
        <>
      <PageTip place="spaces" />
      {/* The page's tip goes here, above the warnings (the tips' own change places it). Then, in this order:
          the machines' warnings, the toolbar, the grid's place. */}
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
          hidden={hiddenCount > 0 ? { count: hiddenCount, onShow: showHiddenDesktops } : undefined}
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
                hide={actions.hide}
              />
              </>
            ) : (
              <WindowedGrid
                ref={grid}
                picking={selection.ids.size > 0}
                count={elsewhereTiles.length + shown.length}
                renderAt={(i) => {
                  if (i < elsewhereTiles.length) return elsewhereTiles[i];
                  const it = shown[i - elsewhereTiles.length]!;
                  return (
                    <GridTile
                      key={it.space.id}
                      item={it}
                      selected={selection.has(it.space.id)}
                      busy={bulk.running.has(it.space.id)}
                      accessKey={keyvault.signedIn.has(it.space.id) ? keyvault.accessKey(it.space.id) : undefined}
                      live={live}
                    />
                  );
                }}
              />
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

interface GridLive {
  selection: ReturnType<typeof useSelection>;
  actions: MenuActions;
}

/** One tile of the grid, redrawn only when its own props change (`live` is a
 * stable ref to what every tile shares). */
const GridTile = memo(function GridTile({
  item,
  selected,
  busy,
  accessKey,
  live,
}: {
  item: SpaceItem;
  selected: boolean;
  busy: boolean;
  accessKey: string | null | undefined;
  live: RefObject<GridLive>;
}) {
  const actions = useMemo<MenuActions>(
    () => ({ power: (it, on) => live.current.actions.power(it, on), remove: (it) => live.current.actions.remove(it), hide: (it) => live.current.actions.hide?.(it) }),
    [live],
  );
  // The checkbox reads whether it is picked and toggles it (the latest selection).
  const selection = useMemo(
    () => ({ ...live.current.selection, has: (id: string) => live.current.selection.has(id), toggle: (it: SpaceItem, e?: { shiftKey?: boolean }) => live.current.selection.toggle(it, e) }),
    [live],
  );
  return (
    // Off-screen tiles skip layout and paint until they scroll in (with room for the selected ring).
    <li className={TILE_CELL}>
      <SpaceContextMenu item={item} actions={actions}>
        <SpaceTile
          item={item}
          accessKey={accessKey}
          selected={selected}
          busy={busy}
          onClick={(e) => live.current.selection.onClick(item, e)}
          topLeft={
            <SelectBox
              item={item}
              selection={selection}
              picked={selected}
              onPicture
              // Hidden until hovered, unless something is picked (the grid's `data-picking`): a
              // CSS group, so the first pick doesn't redraw every tile.
              className="opacity-0 group-focus-within/tile:opacity-100 group-hover/tile:opacity-100 group-data-[picking]/grid:opacity-100"
            />
          }
          hide={hasHideButton(item) ? () => actions.hide?.(item) : undefined}
          more={hasMenu(item) ? <SpaceMoreMenu item={item} actions={actions} /> : undefined}
        />
      </SpaceContextMenu>
    </li>
  );
});
