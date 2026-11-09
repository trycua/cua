// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CornerDownRightIcon, LockIcon, TriangleAlertIcon, XIcon } from "lucide-react";
import { useEffect, useState } from "react";

import type { NetworkPreset, SpaceNetwork, SpaceTopology } from "@/bridge/contracts/network";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { DetailsDisclosure, GroupedRow, GroupedSection, SectionHeading } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";

import { EXTRAS, PRESETS, alwaysBlocked, exceeds, exitDescription, extrasOf, isInternetBase, presetWith, validEntry, validProxy } from "./presets";
import { closeNetworkSheet, useNetworkPreview, useNetworkSheetFor } from "./use-network";

/**
 * A Space's Network sheet. The base as radio rows (Internet only by
 * default), under Internet only an "Also allow" list of two independent
 * checkboxes (services on this computer, other devices on the network),
 * Custom's allow and block lists, what is always blocked, and for a child
 * Space what its parent allows.
 */
export function NetworkSheet({ spaceName }: { spaceName: (id: string) => string }) {
  const store = useNetworkPreview();
  const spaceId = useNetworkSheetFor();
  const security = store && spaceId ? store.get(spaceId) : null;
  // While the sheet is open, read the Space's network and blocked log again now
  // and then (the store re-reads once its last read is older than ten seconds).
  const [, tick] = useState(0);
  const open = Boolean(store && spaceId);
  useEffect(() => {
    if (!open) return;
    const t = setInterval(() => tick((x) => x + 1), 5_000);
    return () => clearInterval(t);
  }, [open]);
  if (!store || !spaceId || !security) return null;
  return (
    <NetworkSheetBody
      key={spaceId}
      name={spaceName(spaceId)}
      initial={security.network}
      parent={security.parent?.name ?? null}
      hasChildren={security.children.length > 0}
      topology={security.topology}
      blocked={store.blocked(spaceId)}
      error={store.error?.(spaceId) ?? null}
      onSave={(n, widen) => {
        store.set(spaceId, n, { widen });
        closeNetworkSheet();
      }}
    />
  );
}

function NetworkSheetBody({
  name,
  initial,
  parent,
  hasChildren,
  topology,
  blocked,
  onSave,
  error,
}: {
  name: string;
  initial: SpaceNetwork;
  parent: string | null;
  hasChildren: boolean;
  /** Its name, exit, host ports and paths, when the host reports them. */
  topology?: SpaceTopology;
  blocked: { address: string; what: string; at: number; always: boolean }[];
  onSave: (n: SpaceNetwork, widen: boolean) => void;
  /** The host refused the last change, in its words. */
  error: string | null;
}) {
  const [n, setN] = useState<SpaceNetwork>(() => ({ ...initial, base: initial.base ?? "internet", localhostPorts: initial.localhostPorts ?? [5432, 3000] }));
  const [widen, setWiden] = useState(false);
  const pick = (preset: NetworkPreset) => setN((x) => ({ ...x, preset }));
  // A child Space: nothing looser than its parent, unless widened here on purpose.
  const ceiling = parent && !widen ? initial.preset : null;
  const tooMuch = (preset: NetworkPreset) => ceiling !== null && exceeds(preset, ceiling);
  const extras = extrasOf(n.preset);
  const setExtra = (id: "localhost" | "lan", on: boolean) => setN((x) => ({ ...x, preset: presetWith({ ...extrasOf(x.preset), [id]: on }) }));
  const inherited = Boolean(initial.inheritedFrom) && n.preset === initial.preset;
  return (
    <Dialog open onOpenChange={(open) => !open && closeNetworkSheet()}>
      <DialogPopup data-network-sheet="" data-preset={n.preset} className="top-[6vh] flex max-h-[88vh] w-[min(600px,calc(100vw-2rem))] flex-col">
        <div className="px-5 pt-4 pb-1">
          <DialogTitle className="text-[15px] font-semibold">Network</DialogTitle>
          <p className="mt-0.5 text-xs text-muted-foreground">{name}</p>
        </div>
        <div className="min-h-0 overflow-y-auto px-5 pt-3 pb-4">
          {parent ? (
            <p data-network-inherited="" className="mb-3 flex items-center gap-2 rounded-lg bg-muted/70 px-3 py-2 text-xs text-muted-foreground">
              <CornerDownRightIcon className="size-3.5 shrink-0" />
              Set by {parent}. Child Spaces can only be stricter.
            </p>
          ) : null}
          <GroupedSection role="radiogroup" aria-label="Network">
            {PRESETS.filter((p) => p.id === "internet" || p.id === "none" || p.id === "custom").map((p) => {
              const selected = p.id === "internet" ? isInternetBase(n.preset) : n.preset === p.id;
              const looser = tooMuch(p.id);
              return (
                <div key={p.id} data-network-preset={p.id} data-selected={selected || undefined}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    disabled={looser}
                    onClick={() => !selected && pick(p.id)}
                    className="flex w-full cursor-default items-start gap-3 px-4 py-2.5 text-left outline-none hover:bg-accent/50 focus-visible:bg-accent/50 disabled:hover:bg-transparent"
                  >
                    <Radio on={selected} className={cn("mt-0.5", looser && "opacity-40")} />
                    <span className={cn("min-w-0 flex-1", looser && "opacity-50")}>
                      <span className="flex items-center gap-2 text-[13px]">
                        {p.title}
                        {selected && inherited ? <span className="text-xs text-muted-foreground">Inherited</span> : null}
                      </span>
                      <span className="mt-px block text-xs text-muted-foreground">{p.description}</span>
                    </span>
                    {looser ? <span className="mt-0.5 shrink-0 text-xs text-muted-foreground">More than {parent} allows</span> : null}
                  </button>
                </div>
              );
            })}
          </GroupedSection>
          {isInternetBase(n.preset) ? (
            <div data-network-extras="" className="mt-4">
              <SectionHeading>Also allow</SectionHeading>
              <GroupedSection className="mt-2" role="group" aria-label="Also allow">
                {EXTRAS.map((x) => {
                  const on = extras[x.id];
                  const looser = tooMuch(presetWith({ ...extras, [x.id]: true }));
                  return (
                    <div key={x.id} data-network-extra={x.id} data-selected={on || undefined}>
                      <label className={cn("flex w-full cursor-default items-start gap-3 px-4 py-2.5", !looser && "hover:bg-accent/50")}>
                        <Checkbox className="mt-0.5" checked={on} disabled={looser && !on} aria-label={x.title} onCheckedChange={(v) => setExtra(x.id, v)} />
                        <span className={cn("min-w-0 flex-1", looser && !on && "opacity-50")}>
                          <span className="block text-[13px]">{x.title}</span>
                          <span className="mt-px block text-xs text-muted-foreground">{x.description}</span>
                        </span>
                        {looser && !on ? <span className="mt-0.5 shrink-0 text-xs text-muted-foreground">More than {parent} allows</span> : null}
                      </label>
                      {on ? (
                        <div className="px-4 pb-3 pl-11">
                          <p data-network-warning="" className="flex items-start gap-2 rounded-lg bg-warning/10 px-3 py-2 text-xs">
                            <TriangleAlertIcon className="mt-px size-3.5 shrink-0 text-warning" />
                            {x.warning}
                          </p>
                          {x.id === "localhost" ? (
                            <Ports ports={n.localhostPorts ?? []} onChange={(localhostPorts) => setN((y) => ({ ...y, localhostPorts }))} />
                          ) : null}
                        </div>
                      ) : null}
                    </div>
                  );
                })}
              </GroupedSection>
            </div>
          ) : null}
          {parent && !widen ? (
            <button
              type="button"
              data-network-widen=""
              onClick={() => setWiden(true)}
              className="mt-2 cursor-default px-1 text-xs text-brand-strong outline-none hover:underline focus-visible:underline"
            >
              Allow more for this Space only…
            </button>
          ) : null}
          {n.preset === "custom" ? <CustomEditor n={n} onChange={setN} /> : null}
          {n.preset !== "none" ? (
            <ExitThrough
              via={n.via}
              members={topology?.members ?? []}
              parent={parent}
              locked={Boolean(parent) && Boolean(initial.via)}
              exit={topology?.exit}
              onChange={(via) => setN((x) => ({ ...x, via }))}
            />
          ) : null}
          {topology ? <Reachable topology={topology} /> : null}
          <p
            data-network-always=""
            title="Cloud metadata, Cua's own services and other Spaces' trees are always blocked."
            className="mt-4 flex items-start gap-2 px-1 text-xs text-muted-foreground"
          >
            <LockIcon className="mt-px size-3.5 shrink-0" />
            Some addresses are always blocked.
          </p>
          {blocked.length ? <RecentlyBlocked rows={blocked} /> : null}
          {error ? (
            <p role="alert" data-network-error="" className="mt-3 px-1 text-xs text-destructive">
              {error}
            </p>
          ) : null}
        </div>
        <div /* design-ok: full-width dialog footer */ className="flex shrink-0 items-center gap-2 border-t bg-muted/60 px-5 py-3">
          <span className="flex-1 text-xs text-muted-foreground">{hasChildren ? "Applies to this Space and its child Spaces." : "Applies to this Space."}</span>
          <Button variant="secondary" onClick={closeNetworkSheet}>
            Cancel
          </Button>
          <Button data-network-save="" onClick={() => onSave(n, widen)}>
            Save
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

/** "Exit through": where the Space's internet traffic leaves. A child keeps its parent's. */
function ExitThrough({
  via,
  members,
  parent,
  locked,
  exit,
  onChange,
}: {
  via: string | undefined;
  members: SpaceTopology["members"];
  parent: string | null;
  /** A child Space whose parent exits somewhere: it can't leave it. */
  locked: boolean;
  exit: SpaceTopology["exit"];
  onChange: (via: string | undefined) => void;
}) {
  const proxy = via?.startsWith("socks5") ? via : undefined;
  const [proxyText, setProxyText] = useState(proxy ?? "");
  const [choosingProxy, setChoosingProxy] = useState(Boolean(proxy));
  const others = members.filter((m) => m.relation !== "self");
  const value = choosingProxy ? "proxy" : (via ?? "internet");
  const options = [
    { value: "internet", label: "Its own network" },
    { value: "this-machine", label: "This computer" },
    ...others.map((m) => ({ value: `space:${m.id}`, label: m.name.split(".")[0] ?? m.name })),
    { value: "proxy", label: "SOCKS5 proxy" },
  ];
  return (
    <div data-network-exit={via ?? "internet"} className="mt-5">
      <SectionHeading>Exit through</SectionHeading>
      <GroupedSection className="mt-2">
        <GroupedRow className="min-h-10 justify-between gap-6 py-1.5">
          <span className="min-w-0 text-xs text-muted-foreground">{exitDescription(choosingProxy ? "socks5://" : via)}</span>
          <Select
            aria-label="Exit through"
            className="h-7 min-w-44"
            disabled={locked}
            value={value}
            options={options}
            onValueChange={(v) => {
              if (v === "proxy") {
                setChoosingProxy(true);
                onChange(validProxy(proxyText) ? proxyText.trim() : undefined);
                return;
              }
              setChoosingProxy(false);
              onChange(v === "internet" ? undefined : v);
            }}
          />
        </GroupedRow>
        {choosingProxy ? (
          <GroupedRow className="min-h-10 gap-3 py-1.5">
            <Input
              data-network-proxy=""
              aria-label="Proxy address"
              placeholder="socks5://proxy.example.com:1080"
              value={proxyText}
              spellCheck={false}
              autoComplete="off"
              onChange={(e) => {
                setProxyText(e.target.value);
                if (validProxy(e.target.value)) onChange(e.target.value.trim());
              }}
              className="font-mono text-xs"
            />
          </GroupedRow>
        ) : null}
      </GroupedSection>
      {locked ? (
        <p className="mt-1.5 flex items-center gap-1.5 px-1 text-xs text-muted-foreground">
          <CornerDownRightIcon className="size-3 shrink-0" />
          Set by {parent}. Child Spaces exit the same way.
        </p>
      ) : null}
      {exit?.state === "unreachable" ? (
        <p data-network-exit-down="" className="mt-1.5 flex items-start gap-2 rounded-lg bg-warning/10 px-3 py-2 text-xs">
          <TriangleAlertIcon className="mt-px size-3.5 shrink-0 text-warning" />
          {exit.label} isn't reachable, so this Space has no internet until it's back.{exit.note ? ` ${exit.note}` : ""}
        </p>
      ) : via ? (
        <p className="mt-1.5 px-1 text-xs text-muted-foreground">
          If the exit stops answering, this Space's internet stops. It never falls back to its own network.
        </p>
      ) : null}
    </div>
  );
}

/** Where the Space is reachable, and how traffic reaches your other computers. */
function Reachable({ topology }: { topology: SpaceTopology }) {
  const rows: { key: string; value: string; detail: string }[] = [];
  if (topology.name) rows.push({ key: "name", value: topology.name, detail: "Its tree reaches it by this name" });
  for (const r of topology.hostReach) {
    const port = r.slice(r.lastIndexOf(":") + 1);
    rows.push({ key: r, value: r, detail: `This computer's port ${port}, in this Space` });
  }
  for (const h of topology.hostOnly) {
    rows.push({ key: `host:${h.port}`, value: `host:${h.port}`, detail: `localhost:${h.port} is in use in the Space` });
  }
  for (const p of topology.paths) {
    rows.push({ key: `path:${p.name}`, value: p.name, detail: p.why });
  }
  if (rows.length === 0) return null;
  return (
    <DetailsDisclosure title="Reachable at" className="mt-5" data-network-reachable="">
      <GroupedSection>
        {rows.map((r) => (
          <GroupedRow key={r.key} className="min-h-10 py-2" data-network-reach-row={r.key}>
            <span className="w-60 shrink-0 truncate font-mono text-xs" title={r.value}>
              {r.value}
            </span>
            <span className="min-w-0 flex-1 text-xs text-muted-foreground">{r.detail}</span>
          </GroupedRow>
        ))}
      </GroupedSection>
    </DetailsDisclosure>
  );
}

/** A radio's circle: filled with the brand when on. */
function Radio({ on, className }: { on: boolean; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        // design-ok: a radio's circle is a form control's outline, as Checkbox's
        "flex size-4 shrink-0 items-center justify-center rounded-full border",
        on ? "border-brand bg-brand" : "border-input bg-card dark:bg-input/30",
        className,
      )}
    >
      {on ? <span className="size-1.5 rounded-full bg-white" /> : null}
    </span>
  );
}

/** A removable value: a port, a domain. */
function Chip({ children, onRemove }: { children: string; onRemove: () => void }) {
  return (
    <span data-network-chip-value={children} className="inline-flex h-6 items-center gap-1 rounded-lg bg-muted pr-1 pl-2 font-mono text-xs">
      {children}
      <button
        type="button"
        aria-label={`Remove ${children}`}
        onClick={onRemove}
        className="inline-flex size-4 cursor-default items-center justify-center rounded-lg text-muted-foreground hover:text-foreground"
      >
        <XIcon className="size-3" />
      </button>
    </span>
  );
}

function Ports({ ports, onChange }: { ports: number[]; onChange: (p: number[]) => void }) {
  const [text, setText] = useState("");
  const add = () => {
    const p = Number(text.trim());
    if (Number.isInteger(p) && p > 0 && p < 65536 && !ports.includes(p)) onChange([...ports, p]);
    setText("");
  };
  return (
    <div data-network-ports="" className="mt-3 flex items-center gap-3">
      <span className="shrink-0 text-[13px]">Only these ports</span>
      {/* design-ok: a form field's outline, as Input's */}
      <div className="flex min-h-8 min-w-0 flex-1 flex-wrap items-center gap-1.5 rounded-lg border border-input bg-card px-1.5 py-1 shadow-xs dark:bg-input/30">
        {ports.map((p) => (
          <Chip key={p} onRemove={() => onChange(ports.filter((x) => x !== p))}>
            {String(p)}
          </Chip>
        ))}
        <input
          aria-label="Add a port"
          value={text}
          placeholder={ports.length ? "" : "Every port"}
          onChange={(e) => setText(e.target.value.replace(/\D/g, ""))}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === ",") {
              e.preventDefault();
              add();
            }
          }}
          className="h-6 min-w-12 flex-1 bg-transparent px-1 text-[13px] outline-none placeholder:text-muted-foreground"
        />
      </div>
    </div>
  );
}

function CustomEditor({ n, onChange }: { n: SpaceNetwork; onChange: (n: SpaceNetwork) => void }) {
  const [entry, setEntry] = useState("");
  const [list, setList] = useState<"allow" | "deny">("allow");
  const always = alwaysBlocked(entry);
  const valid = validEntry(entry);
  const canAdd = valid && !(list === "allow" && always);
  const add = () => {
    if (!canAdd) return;
    const e = entry.trim().toLowerCase();
    onChange({ ...n, [list]: [...n[list].filter((x) => x !== e), e] });
    setEntry("");
  };
  const remove = (which: "allow" | "deny", e: string) => onChange({ ...n, [which]: n[which].filter((x) => x !== e) });
  return (
    <div data-network-custom="" className="mt-5">
      <SectionHeading
        action={
          <Segmented
            aria-label="Start from"
            value={n.base ?? "internet"}
            options={[
              { value: "internet", label: "Internet only" },
              { value: "none", label: "No network" },
            ]}
            onValueChange={(base) => onChange({ ...n, base })}
          />
        }
      >
        Start from
      </SectionHeading>
      <div className="mt-3 grid gap-x-4 sm:grid-cols-2">
        <EntryList title="Allow" rows={n.allow} empty="Nothing added" onRemove={(e) => remove("allow", e)} />
        <EntryList title="Block" rows={n.deny} empty="Nothing added" onRemove={(e) => remove("deny", e)} />
      </div>
      <form
        className="mt-3 flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <Input
          data-network-entry=""
          aria-label="Domain or address range"
          placeholder="Domain or address range"
          value={entry}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setEntry(e.target.value)}
          className="font-mono text-xs"
        />
        <Segmented
          aria-label="List"
          value={list}
          options={[
            { value: "allow", label: "Allow" },
            { value: "deny", label: "Block" },
          ]}
          onValueChange={setList}
        />
        <Button type="submit" variant="secondary" disabled={!canAdd} data-network-add="">
          Add
        </Button>
      </form>
      {entry && always ? (
        <p data-network-entry-hint="" className="mt-1.5 flex items-center gap-1.5 px-1 text-xs text-muted-foreground">
          <LockIcon className="size-3" /> Always blocked: {always}
        </p>
      ) : entry && !valid ? (
        <p className="mt-1.5 px-1 text-xs text-muted-foreground">A domain (example.com, *.example.com) or an address range (10.0.0.0/8).</p>
      ) : null}
    </div>
  );
}

function EntryList({ title, rows, empty, onRemove }: { title: string; rows: string[]; empty: string; onRemove: (e: string) => void }) {
  return (
    <GroupedSection title={title} data-network-list={title.toLowerCase()}>
      {rows.length === 0 ? (
        <GroupedRow className="min-h-9 py-2 text-muted-foreground">{empty}</GroupedRow>
      ) : (
        rows.map((r) => (
          <GroupedRow key={r} className="min-h-9 justify-between py-1.5" data-network-entry-row={r}>
            <span className="min-w-0 truncate font-mono text-xs">{r}</span>
            <button
              type="button"
              aria-label={`Remove ${r}`}
              onClick={() => onRemove(r)}
              className="inline-flex size-6 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <XIcon className="size-3.5" />
            </button>
          </GroupedRow>
        ))
      )}
    </GroupedSection>
  );
}

/** When a connection was last blocked, in words: "Blocked just now", "Blocked 3 minutes ago". */
export const blockedWhen = (at: number, now = Date.now()) => {
  if (!at) return "Blocked";
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 45) return "Blocked just now";
  const m = Math.max(1, Math.round(s / 60));
  if (m < 60) return `Blocked ${m} ${m === 1 ? "minute" : "minutes"} ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `Blocked ${h} ${h === 1 ? "hour" : "hours"} ago`;
  const d = Math.round(h / 24);
  return `Blocked ${d} days ago`;
};

function RecentlyBlocked({ rows }: { rows: { address: string; what: string; at: number; always: boolean }[] }) {
  // Re-render the times now and then while it is open.
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((x) => x + 1), 30_000);
    return () => clearInterval(t);
  }, []);
  return (
    <DetailsDisclosure title="Recently blocked" className="mt-5" data-network-recent="">
      <GroupedSection>
        {rows.map((r) => (
          <GroupedRow key={r.address} className="min-h-10 py-2" data-network-blocked-row={r.address}>
            <span className="w-36 shrink-0 truncate font-mono text-xs">{r.address}</span>
            <span className="min-w-0 flex-1 truncate" title={r.always ? `${r.what}: a rule that always applies` : r.what}>
              {r.what}
              {r.always ? <span className="text-muted-foreground">, always applies</span> : null}
            </span>
            <span className="shrink-0 text-xs text-muted-foreground" data-network-blocked-when="">
              {blockedWhen(r.at)}
            </span>
          </GroupedRow>
        ))}
      </GroupedSection>
    </DetailsDisclosure>
  );
}
