// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { PictureInPicture2Icon, SearchIcon } from "lucide-react";

import type { SpaceOs, StreamRow, StreamSection as StreamSectionView } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** The core's OS mark ids (`os-ubuntu`, `os-macos`, ..., `computer` for an
 * unknown OS) on this app's marks. */
function markOs(id: string): SpaceOs {
  if (id === "os-macos") return "macos";
  if (id === "os-windows") return "windows";
  if (id === "computer") return "unknown";
  return "linux";
}

/**
 * A Space's Stream section, as the core lays it out (`sidebar.streamSection`):
 * the Desktop row with the display's resolution, then one row per window,
 * each with its picture-in-picture button, and a line while there are no
 * window rows.
 */
export function StreamSection({
  title,
  section,
  query,
  onQuery,
  onPip,
  hideStatus,
}: {
  title: string;
  section: StreamSectionView;
  query: string;
  onQuery: (q: string) => void;
  onPip: (row: StreamRow) => void;
  /** The host can't list windows: no loading or empty line. */
  hideStatus?: boolean;
}) {
  return (
    <section data-section="stream" className="mb-7">
      <div className="mb-2 flex items-center justify-between gap-3 px-1">
        <h2 className="text-xs font-semibold text-muted-foreground">{title}</h2>
        <label className="flex h-6 w-40 items-center gap-1.5 rounded-md border bg-card px-2 text-xs text-muted-foreground shadow-xs focus-within:border-ring">
          <SearchIcon className="size-3 shrink-0" />
          <input
            data-stream-filter
            value={query}
            onChange={(e) => onQuery(e.target.value)}
            placeholder="Filter windows"
            aria-label="Filter windows"
            className="min-w-0 flex-1 bg-transparent text-foreground outline-none placeholder:text-muted-foreground"
          />
        </label>
      </div>
      <ul className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
        {section.rows.map((row) => (
          <Row key={row.id} row={row} onPip={() => onPip(row)} />
        ))}
        {section.statusText && !hideStatus ? (
          <li data-stream-status className="px-4 py-2.5 text-[13px] text-muted-foreground">
            {section.statusText}
          </li>
        ) : null}
      </ul>
    </section>
  );
}

function Row({ row, onPip }: { row: StreamRow; onPip: () => void }) {
  const pip = row.actions.find((a) => a.id === "pip");
  return (
    <li data-stream-row={row.id} data-kind={row.kind} className="flex min-h-10 items-center gap-3 px-4 py-2">
      <RowIcon row={row} />
      <span data-row-label title={row.help} className="min-w-0 flex-1 truncate text-[13px]">
        {row.label}
      </span>
      {row.resolution ? (
        <span data-row-resolution className="shrink-0 text-xs text-muted-foreground tabular-nums">
          {row.resolution}
        </span>
      ) : null}
      {pip ? (
        <Tooltip content={pip.help}>
          <button
            type="button"
            data-pip={row.id}
            aria-label={pip.help}
            aria-pressed={pip.active}
            onClick={onPip}
            className={cn(
              "inline-flex size-7 shrink-0 cursor-default items-center justify-center rounded-md outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60",
              pip.active ? "bg-brand/10 text-brand hover:bg-brand/15" : "text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground",
            )}
          >
            <PictureInPicture2Icon className="size-4" />
          </button>
        </Tooltip>
      ) : null}
    </li>
  );
}

function RowIcon({ row }: { row: StreamRow }) {
  if (row.icon.kind === "os") {
    return (
      <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
        <OsIcon os={markOs(row.icon.id)} className="size-3.5" />
      </span>
    );
  }
  // App icons come from the Space itself; until the bridge reads them, the app's initial.
  const name = row.icon.appName || row.icon.appId;
  return (
    <span
      data-app-icon={row.icon.appId}
      title={name}
      className="flex size-6 shrink-0 items-center justify-center rounded-md border bg-background text-2xs font-semibold text-muted-foreground"
    >
      {name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}
