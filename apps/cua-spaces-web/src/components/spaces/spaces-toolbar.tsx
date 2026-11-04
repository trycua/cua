// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Spaces page's one toolbar row: state chips (All, Running, Stopped,
 * Creating), then on the right Select stopped (list view), whose Spaces,
 * which computer, the sort, grid or list, and the filter box.
 */

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { Toggle } from "@base-ui/react/toggle";
import { ToggleGroup } from "@base-ui/react/toggle-group";
import { ArrowUpDownIcon, CheckIcon, ChevronDownIcon, LayoutGridIcon, ListIcon, MoreHorizontalIcon, SearchIcon } from "lucide-react";
import type { ReactNode } from "react";

import type { SpaceFilter } from "@/lib/spaces";
import type { SpacesOwner, SpacesSort, SpacesView } from "@/lib/spaces-page";
import { cn } from "@/lib/utils";

export const STATE_CHIPS: { value: SpaceFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "running", label: "Running" },
  { value: "stopped", label: "Stopped" },
  { value: "creating", label: "Creating" },
];

export const OWNERS: { value: "" | SpacesOwner; label: string }[] = [
  { value: "", label: "Anyone" },
  { value: "mine", label: "Mine" },
  { value: "shared", label: "Shared with me" },
];

export const SORTS: { value: SpacesSort; label: string }[] = [
  { value: "used", label: "Last used" },
  { value: "name", label: "Name" },
  { value: "size", label: "Size" },
];

/** A popup menu's look, shared with the Spaces' "⋯" menus. */
export const MENU_POPUP =
  "glass min-w-44 origin-(--transform-origin) rounded-lg p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0";
export const MENU_ITEM =
  "flex cursor-default items-center gap-2 rounded-md px-2 py-1 text-[13px] outline-none data-disabled:opacity-40 data-highlighted:bg-brand data-highlighted:text-white";

export interface ToolbarProps {
  state: SpaceFilter;
  onState: (state: SpaceFilter) => void;
  owner?: SpacesOwner;
  onOwner: (owner?: SpacesOwner) => void;
  /** This Mac first, then each machine. */
  computers: { value: string; label: string }[];
  on?: string;
  onComputer: (id?: string) => void;
  sort: SpacesSort;
  onSort: (sort: SpacesSort) => void;
  view: SpacesView;
  onView: (view: SpacesView) => void;
  query: string;
  onQuery: (query: string) => void;
  /** Before the menus (Select stopped). */
  lead?: ReactNode;
  /** Desktop rows hidden from the list: the overflow's "Show hidden". */
  hidden?: { count: number; onShow: () => void };
}

export function SpacesToolbar(p: ToolbarProps) {
  const computers = [{ value: "", label: "All machines" }, ...p.computers];
  const computer = computers.find((c) => c.value === (p.on ?? "")) ?? { value: p.on ?? "", label: "All machines" };
  return (
    <div data-spaces-toolbar="" className="mb-5 flex min-h-9 flex-wrap items-center gap-x-2 gap-y-2">
      <Chips value={p.state} onChange={p.onState} />
      <div className="ml-auto flex flex-wrap items-center gap-1">
        {p.lead}
        <MenuButton data="owner" label={OWNERS.find((o) => o.value === (p.owner ?? ""))!.label} items={OWNERS} value={p.owner ?? ""} onChange={(v) => p.onOwner((v || undefined) as SpacesOwner | undefined)} />
        <MenuButton data="computer" label={computer.label} items={computers} value={p.on ?? ""} onChange={(v) => p.onComputer(v || undefined)} />
        <MenuButton
          data="sort"
          icon={<ArrowUpDownIcon className="size-3.5" />}
          label={SORTS.find((s) => s.value === p.sort)!.label}
          items={SORTS}
          value={p.sort}
          onChange={(v) => p.onSort(v as SpacesSort)}
        />
        <ToggleGroup
          aria-label="View"
          value={[p.view]}
          onValueChange={(next) => next[0] && p.onView(next[0] as SpacesView)}
          className="ml-1 flex items-center gap-0.5 rounded-lg bg-tile p-0.5"
        >
          {(["grid", "list"] as const).map((v) => (
            <Toggle
              key={v}
              value={v}
              aria-label={v === "grid" ? "Grid" : "List"}
              data-view={v}
              className="flex size-6 items-center justify-center rounded-md text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-pressed:bg-background data-pressed:text-foreground data-pressed:shadow-xs"
            >
              {v === "grid" ? <LayoutGridIcon className="size-3.5" /> : <ListIcon className="size-3.5" />}
            </Toggle>
          ))}
        </ToggleGroup>
        {/* Find a Space by name, detail, id or machine (the SwiftUI sidebar's search). */}
        <div className="relative ml-1 w-44">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            type="search"
            value={p.query}
            onChange={(e) => p.onQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape" && p.query) {
                e.preventDefault();
                e.stopPropagation();
                p.onQuery("");
              }
            }}
            placeholder="Filter Spaces"
            aria-label="Filter Spaces"
            data-spaces-filter=""
            className="h-7 w-full rounded-lg bg-tile pr-2 pl-8 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring/60 [&::-webkit-search-cancel-button]:hidden"
          />
        </div>
        {p.hidden ? <Overflow hidden={p.hidden} /> : null}
      </div>
    </div>
  );
}

/** The list's overflow: "Show hidden", while a desktop row is hidden. */
function Overflow({ hidden }: { hidden: { count: number; onShow: () => void } }) {
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        aria-label="More"
        data-toolbar-menu="overflow"
        className="ml-1 flex size-7 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-tile hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-tile data-popup-open:text-foreground"
      >
        <MoreHorizontalIcon className="size-4" />
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="end" className="z-50 outline-none">
          <MenuPrimitive.Popup data-toolbar-popup="overflow" className={MENU_POPUP}>
            <MenuPrimitive.Item data-show-hidden="" onClick={hidden.onShow} className={MENU_ITEM}>
              Show hidden
            </MenuPrimitive.Item>
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}

/** The state chips: rounded rectangles in the one radius; the chosen one inverted. */
function Chips({ value, onChange }: { value: SpaceFilter; onChange: (v: SpaceFilter) => void }) {
  return (
    <ToggleGroup aria-label="Filter by state" value={[value]} onValueChange={(next) => next[0] && onChange(next[0] as SpaceFilter)} className="flex items-center gap-1.5">
      {STATE_CHIPS.map((f) => (
        <Toggle
          key={f.value}
          value={f.value}
          data-chip={f.value}
          className="h-7 cursor-default rounded-lg bg-tile px-3 text-xs font-medium text-foreground outline-none transition-colors hover:bg-tile-hover focus-visible:ring-2 focus-visible:ring-ring/60 data-pressed:bg-foreground data-pressed:text-background"
        >
          {f.label}
        </Toggle>
      ))}
    </ToggleGroup>
  );
}

function MenuButton({ label, icon, items, value, onChange, data }: { label: string; icon?: ReactNode; items: { value: string; label: string }[]; value: string; onChange: (v: string) => void; data: string }) {
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        data-toolbar-menu={data}
        className="inline-flex h-7 cursor-default items-center gap-1.5 rounded-lg px-2 text-xs font-medium text-muted-foreground outline-none hover:bg-tile hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-tile data-popup-open:text-foreground"
      >
        {icon}
        {label}
        <ChevronDownIcon className="size-3 opacity-70" />
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="end" className="z-50 outline-none">
          <MenuPrimitive.Popup data-toolbar-popup={data} className={MENU_POPUP}>
            {items.map((i) => (
              <MenuPrimitive.Item key={i.value} onClick={() => onChange(i.value)} className={MENU_ITEM}>
                <CheckIcon className={cn("size-3.5", i.value === value ? "opacity-100" : "opacity-0")} />
                {i.label}
              </MenuPrimitive.Item>
            ))}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
