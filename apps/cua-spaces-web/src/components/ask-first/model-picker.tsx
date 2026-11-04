// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronDownIcon, PlusIcon, SearchIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { ModelGroup, ModelsView } from "@/bridge";
import { ProviderMark } from "@/components/settings/connections/provider-mark";
import { Popover, PopoverDivider } from "@/components/ui/popover";
import { modelLine, type ModelSelection, type PickedModel } from "@/lib/ask-first-model";
import { cn } from "@/lib/utils";

/** With this many models or fewer the menu needs no search. */
const SEARCH_FROM = 8;

interface Row {
  group: ModelGroup;
  id: string;
  label: string;
  fast: boolean;
  connected: boolean;
}

/**
 * The model menu of the ask box: the connected providers' models, the one
 * in use ticked, a quiet "Fast" on the quick ones, a provider whose sign-in
 * ran out marked and leading to Connections, and at the bottom the way to
 * connect another provider. Arrow keys move, Enter picks, the digits 1 to 9
 * pick a row, and typing searches when the list is long.
 */
export function ModelPicker({
  view,
  selected,
  onPick,
  onConnect,
}: {
  view: ModelsView | null;
  selected: ModelSelection | null;
  onPick: (p: PickedModel) => void;
  /** Opens Connections: to connect a provider, or reconnect the one named. */
  onConnect: (provider?: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const search = useRef<HTMLInputElement>(null);
  const rows: Row[] = useMemo(
    () => (view?.groups ?? []).flatMap((g) => g.models.map((m) => ({ group: g, id: m.id, label: m.label, fast: m.fast, connected: g.state === "connected" }))),
    [view],
  );
  const q = query.trim().toLowerCase();
  const shown = q ? rows.filter((r) => `${r.label} ${r.group.label}`.toLowerCase().includes(q)) : rows;
  const pickable = shown.filter((r) => r.connected);
  useEffect(() => setActive(0), [query, open]);
  const pick = (r: Row) => {
    if (!r.connected) {
      setOpen(false);
      onConnect(r.group.provider);
      return;
    }
    onPick({ provider: r.group.provider, model: r.id });
    setOpen(false);
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = pickable.length || 1;
      setActive((a) => (a + (e.key === "ArrowDown" ? 1 : -1) + n) % n);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const r = pickable[active];
      if (r) pick(r);
    } else if (/^[1-9]$/.test(e.key) && !q && !e.metaKey && !e.ctrlKey && !e.altKey) {
      const r = pickable[Number(e.key) - 1];
      if (r) {
        e.preventDefault();
        pick(r);
      }
    }
  };
  const empty = !view || view.groups.length === 0;
  const none = view?.empty ?? "Connect a provider to pick a model.";
  let n = 0;
  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      width={340}
      initialFocus={search}
      popupProps={{ "data-model-menu": "", onKeyDown: onKey } as never}
      trigger={
        <button
          type="button"
          data-picker="model"
          className={cn("flex h-9 items-center gap-2 rounded-lg px-2.5 text-[13px] outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-popup-open:bg-foreground/[0.07]", empty && "text-brand-strong")}
          aria-label={selected ? `Model: ${modelLine(selected)}` : "Pick a model"}
          onClick={(e) => {
            if (empty) {
              e.preventDefault();
              onConnect();
            }
          }}
        >
          {selected ? (
            <>
              <ProviderMark id={selected.group.provider} name={selected.group.label} plain />
              <span data-ask-model="" className="font-medium">
                {selected.model.label}
              </span>
            </>
          ) : (
            <span data-ask-model="">Connect a provider</span>
          )}
          {empty ? null : <ChevronDownIcon className="size-3.5 text-muted-foreground" />}
        </button>
      }
    >
      {rows.length > SEARCH_FROM ? (
        <label className="flex items-center gap-2 px-2.5 pb-2 pt-1.5 text-[13px] text-muted-foreground">
          <SearchIcon className="size-3.5 shrink-0" />
          <input ref={search} value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search models" aria-label="Search models" className="min-w-0 flex-1 bg-transparent text-foreground outline-none placeholder:text-muted-foreground" />
        </label>
      ) : (
        <input ref={search} aria-hidden tabIndex={-1} className="sr-only" readOnly />
      )}
      {rows.length > SEARCH_FROM ? <PopoverDivider /> : null}
      <div role="listbox" aria-label="Models" className="max-h-[320px] overflow-y-auto">
        {empty ? <p className="px-2.5 py-2 text-[13px] text-muted-foreground">{none}</p> : null}
        {(view?.groups ?? []).map((g) => {
          const list = shown.filter((r) => r.group.provider === g.provider);
          if (!list.length) return null;
          return (
            <div key={g.provider} data-model-group={g.provider} className="py-0.5">
              <div className="flex items-center gap-2 px-2.5 pt-1.5 pb-1 text-[12px] font-medium text-muted-foreground">
                <ProviderMark id={g.provider} name={g.label} plain />
                <span>{g.label}</span>
                {g.state === "needs-reconnect" ? <span className="font-normal">needs you to reconnect</span> : null}
              </div>
              {list.map((r) => {
                const index = r.connected ? n++ : -1;
                const isSelected = selected?.group.provider === r.group.provider && selected.model.id === r.id;
                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    key={`${r.group.provider}/${r.id}`}
                    data-model-row={r.id}
                    data-active={index === active ? "" : undefined}
                    onMouseEnter={() => index >= 0 && setActive(index)}
                    onClick={() => pick(r)}
                    className={cn("flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] outline-none", r.connected ? "data-active:bg-foreground/[0.06]" : "text-muted-foreground hover:bg-foreground/[0.04]")}
                  >
                    <span className="min-w-0 flex-1 truncate">{r.label}</span>
                    {r.fast ? <span className="text-[11px] text-muted-foreground">Fast</span> : null}
                    {isSelected ? <CheckIcon className="size-3.5 shrink-0" /> : null}
                    {r.connected && index < 9 && !q ? <kbd className="w-3 shrink-0 text-center text-[11px] text-muted-foreground">{index + 1}</kbd> : null}
                    {!r.connected ? <span className="text-[12px]">Reconnect</span> : null}
                  </button>
                );
              })}
            </div>
          );
        })}
        {!empty && shown.length === 0 ? <p className="px-2.5 py-2 text-[13px] text-muted-foreground">No model matches that.</p> : null}
      </div>
      <PopoverDivider />
      <button
        type="button"
        data-model-connect=""
        onClick={() => {
          setOpen(false);
          onConnect();
        }}
        className="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[13px] text-muted-foreground outline-none hover:bg-foreground/[0.04] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        <PlusIcon className="size-3.5" />
        {view?.connectLabel ?? "Connect a provider"}
      </button>
    </Popover>
  );
}
