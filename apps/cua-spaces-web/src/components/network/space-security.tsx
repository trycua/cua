// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space page's Network, Volume and Clipboard rows, the exit-down notice at the
// top of More, a blocked connection's notice, the one-time notice about the new network
// default, and the clipboard prompt (demo; a native viewer asks itself).

import { ChevronRightIcon, ShieldIcon, ShieldAlertIcon, XIcon } from "lucide-react";

import { useState, type ReactNode } from "react";

import type { ClipboardMode, VolumeScope } from "@/bridge/contracts/network";
import { FactRow } from "@/components/space-detail/facts";
import { GroupedSection } from "@/components/ui/grouped";
import { Select } from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

import { CLIPBOARD_TEXT, VOLUME_TEXT, exitLabel, networkSummary } from "./presets";
import { openNetworkSheet, useNetworkPreview } from "./use-network";

/**
 * At the top of More: the Space's exit is down, so it has no internet until
 * it is back. The header says nothing about the network; this is where it
 * shows (the Network sheet has the same line under Exit through).
 */
export function ExitDownNotice({ spaceId }: { spaceId: string }) {
  const s = useNetworkPreview()?.get(spaceId);
  const exit = s?.topology?.exit;
  if (!s || exit?.state !== "unreachable") return null;
  return (
    <div role="status" data-network-exit-down-more="" className="mb-6 flex items-center gap-2.5 rounded-lg bg-warning/10 py-2 pr-1.5 pl-3 text-[13px]">
      <ShieldAlertIcon className="size-4 shrink-0 text-warning" />
      <span className="min-w-0 flex-1">{exitLabel(s.network.via, s.topology?.members)} isn't reachable, so this Space has no internet until it's back.</span>
      <Button size="sm" variant="ghost" onClick={() => openNetworkSheet(spaceId)}>
        Change exit
      </Button>
    </div>
  );
}

/** "Part of research-swarm", for a child Space. */
export function ParentLine({ spaceId }: { spaceId: string }) {
  const s = useNetworkPreview()?.get(spaceId);
  return s?.parent ? <span data-space-parent={s.parent.name}>Part of {s.parent.name}</span> : null;
}

/** "db.swarm on Studio computer": one plain line for the members of this Space's tree that run on your other computers. */
export function machinesLine(remote: readonly { name: string; machine: string }[]): string {
  const byMachine = new Map<string, string[]>();
  for (const r of remote) byMachine.set(r.machine, [...(byMachine.get(r.machine) ?? []), r.name]);
  return [...byMachine].map(([machine, names]) => `${names.join(", ")} on ${machine}`).join("; ");
}

/** More's Details row for members of the tree on other computers; nothing when they all run here. */
export function RunsOnMachinesRow({ spaceId }: { spaceId: string }) {
  const remote = useNetworkPreview()?.get(spaceId)?.topology?.remote;
  if (!remote?.length) return null;
  // The row names the machines; hovering it lists which member runs where.
  const machines = [...new Set(remote.map((r) => r.machine))].join(", ");
  return <FactRow fact={{ label: "On other computers", value: machines, help: machinesLine(remote) }} />;
}

/** The rail's one top-level row: Network (Volume and Clipboard live under More). */
export function NetworkRow({ spaceId }: { spaceId: string }) {
  const s = useNetworkPreview()?.get(spaceId);
  if (!s) return null;
  return <NetworkButton spaceId={spaceId} summary={networkSummary(s.network)} />;
}

function NetworkButton({ spaceId, summary }: { spaceId: string; summary: string }) {
  return (
    <button
      type="button"
      data-network-row=""
      onClick={() => openNetworkSheet(spaceId)}
      className="flex h-8 w-full cursor-default items-center justify-between gap-3 rounded-lg px-2 text-[13px] outline-none hover:bg-accent focus-visible:bg-accent"
    >
      <span className="shrink-0 text-muted-foreground">Network</span>
      <span className="flex min-w-0 items-center gap-1">
        <span className="truncate" title={summary}>
          {summary}
        </span>
        <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
      </span>
    </button>
  );
}

/** More's "Access" group: Network (opens its sheet), then Volume and Clipboard, each changeable here. */
export function SecurityRows({ spaceId, className }: { spaceId: string; className?: string }) {
  const store = useNetworkPreview();
  const s = store?.get(spaceId);
  // Shown at once; the host's answer follows (a host without the setting
  // keeps the pick here only).
  const [volume, setVolume] = useState<VolumeScope | null>(null);
  const [clipboard, setClipboard] = useState<ClipboardMode | null>(null);
  if (!s) return null;
  const volumeOptions = (["own-folder", "off"] as const).map((v) => ({ value: v, label: VOLUME_TEXT[v] }));
  const clipboardOptions = (["viewing", "ask", "off"] as const).map((v) => ({ value: v, label: CLIPBOARD_TEXT[v] }));
  return (
    <GroupedSection title="Access" data-security-rows="" className={className}>
      <button
        type="button"
        data-network-row=""
        onClick={() => openNetworkSheet(spaceId)}
        className="flex min-h-10 w-full cursor-default items-center justify-between gap-6 px-4 py-2 text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
      >
        <span className="shrink-0 text-muted-foreground">Network</span>
        <span className="flex min-w-0 items-center gap-1">
          <span className="truncate" title={networkSummary(s.network)}>
            {networkSummary(s.network)}
          </span>
          <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
        </span>
      </button>
      {s.network.via ? (
        <button
          type="button"
          data-network-exit-row={s.network.via}
          onClick={() => openNetworkSheet(spaceId)}
          className="flex min-h-10 w-full cursor-default items-center justify-between gap-6 px-4 py-2 text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
        >
          <span className="shrink-0 text-muted-foreground">Exit through</span>
          <span className={cn("flex min-w-0 items-center gap-1", s.topology?.exit?.state === "unreachable" && "text-warning")}>
            <span className="truncate">{exitLabel(s.network.via, s.topology?.members)}</span>
            <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
          </span>
        </button>
      ) : null}
      <div data-security-fact="Volume" className="flex min-h-10 items-center justify-between gap-6 px-4 py-1.5 text-[13px]">
        <span className="shrink-0 text-muted-foreground">Volume</span>
        <Select
          aria-label="Volume"
          className="h-7 min-w-44"
          value={volume ?? (s.volume === "off" ? "off" : "own-folder")}
          options={volumeOptions}
          onValueChange={(v) => {
            setVolume(v);
            store?.setAccess?.(spaceId, { volume: v });
          }}
        />
      </div>
      <div data-security-fact="Clipboard" className="flex min-h-10 items-center justify-between gap-6 px-4 py-1.5 text-[13px]">
        <span className="shrink-0 text-muted-foreground">Clipboard</span>
        <Select
          aria-label="Clipboard"
          className="h-7 min-w-44"
          value={clipboard ?? s.clipboard}
          options={clipboardOptions}
          onValueChange={(c) => {
            setClipboard(c);
            store?.setAccess?.(spaceId, { clipboard: c });
          }}
        />
      </div>
    </GroupedSection>
  );
}

/** A connection the Space tried and the settings blocked: one calm line under the header. */
export function BlockedNotice({ spaceId }: { spaceId: string }) {
  const store = useNetworkPreview();
  const b = store?.notice(spaceId);
  if (!store || !b) return null;
  // `10.0.2.2:5432` -> port 5432 (a network such as `10.0.0.0/8` has none).
  const port = /:(\d+)$/.exec(b.address)?.[1];
  const what = b.what.charAt(0).toLowerCase() + b.what.slice(1);
  return (
    <div role="status" data-network-blocked="" className="mx-5 mb-1 flex items-center gap-2.5 rounded-lg bg-warning/10 py-1.5 pr-1.5 pl-3 text-[13px]">
      <ShieldAlertIcon className="size-4 shrink-0 text-warning" />
      <span className="min-w-0 flex-1 truncate">
        Blocked: this Space can't reach {what}
        {port ? ` (port ${port})` : ""}.
      </span>
      <Button size="sm" variant="ghost" data-network-blocked-change="" onClick={() => openNetworkSheet(spaceId)}>
        Change network settings
      </Button>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => store.dismissNotice(spaceId)}
        className="inline-flex size-7 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground"
      >
        <XIcon className="size-3.5" />
      </button>
    </div>
  );
}

/** Once after the update that moved an existing Space to the new default:
 * what changed, in plain words, and where to change it. */
export function NetworkNotice({ spaceId }: { spaceId: string }) {
  const store = useNetworkPreview();
  const s = store?.get(spaceId);
  if (!store || !s?.noticePending) return null;
  const ack = () => store.ackNotice?.(spaceId);
  return (
    <div role="status" data-network-notice="" className="mx-5 mb-1 flex items-center gap-2.5 rounded-lg bg-muted py-1.5 pr-1.5 pl-3 text-[13px]">
      <ShieldIcon className="size-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate">This Space now reaches the internet only. It can't reach this computer or your local network.</span>
      <Button
        size="sm"
        variant="ghost"
        data-network-notice-change=""
        onClick={() => {
          ack();
          openNetworkSheet(spaceId);
        }}
      >
        Change network settings
      </Button>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={ack}
        className="inline-flex size-7 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground"
      >
        <XIcon className="size-3.5" />
      </button>
    </div>
  );
}

/** The stage, with the clipboard prompt over it while the Space has copied text for you; the stage as it is otherwise. */
export function WithClipboardPrompt({ spaceId, spaceName, children }: { spaceId: string; spaceName: string; children: ReactNode }) {
  const store = useNetworkPreview();
  if (!store?.clipboardOffer(spaceId)) return <>{children}</>;
  return (
    <div className="relative h-full">
      {children}
      <ClipboardPrompt spaceId={spaceId} spaceName={spaceName} />
    </div>
  );
}

/** The Space copied text while you watch: paste it on this computer? Anchored at the stream's top edge. */
export function ClipboardPrompt({ spaceId, spaceName }: { spaceId: string; spaceName: string }) {
  const store = useNetworkPreview();
  const offer = store?.clipboardOffer(spaceId);
  if (!store || !offer) return null;
  const answer = () => store.answerClipboard(spaceId);
  return (
    <div className="pointer-events-none absolute inset-x-0 top-3 z-10 flex justify-center px-4">
      <div
        role="dialog"
        aria-label="Clipboard"
        data-clipboard-prompt=""
        // design-ok: a popover floating over the stream, as the app's menus
        className="glass pointer-events-auto flex items-center gap-3 rounded-lg border py-1.5 pr-1.5 pl-3.5 text-[13px] shadow-float"
      >
        <span className="min-w-0">
          {spaceName} copied text. Paste it on your computer?
          <button
            type="button"
            data-clipboard-always=""
            onClick={answer}
            className="ml-2 cursor-default text-xs text-brand-strong outline-none hover:underline"
          >
            Always while I'm viewing
          </button>
        </span>
        <Button size="sm" variant="secondary" onClick={answer}>
          Not now
        </Button>
        <Button size="sm" data-clipboard-allow="" onClick={answer}>
          Allow once
        </Button>
      </div>
    </div>
  );
}
