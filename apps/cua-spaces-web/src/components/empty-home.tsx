// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The main window with no Spaces yet, as the SwiftUI app draws it: a
 * monitor, "No Spaces yet", one tile per system this machine runs (Linux,
 * and macOS on Apple silicon Macs) with its time and size from the core,
 * under the Spaces header (its New Space opens the wizard with nothing
 * chosen). A tile opens New Space with that system chosen, on this machine
 * (the wizard says when it can't run here).
 */

import { useNewSpaceWizard, type EmptyHome as EmptyHomeView, type EmptyTile } from "@/bridge";
import { OsIcon } from "@/components/os-icon";

/** `unread`: the host could not read the Spaces list (`listNotice`): the
 * title says so and no system is offered, as the SwiftUI app's does. */
export function EmptyHome({ home, unread = false }: { home: EmptyHomeView; unread?: boolean }) {
  const wizard = useNewSpaceWizard();
  const start = (tile: EmptyTile) => wizard.show(null, tile.os);
  return (
    <section data-empty-home="" aria-labelledby="empty-home-title" className="flex flex-col items-center px-8 pt-20 pb-10 text-center">
      <MonitorGlyph />
      <h2 id="empty-home-title" className="mt-4 text-[22px] font-bold tracking-[-0.01em] text-muted-foreground">
        {unread ? "Spaces could not be loaded" : home.title}
      </h2>
      {unread ? null : (
        <>
          <p className="mt-2.5 text-[13px] text-muted-foreground">{home.detail}</p>
          <div className="mt-5 flex flex-wrap justify-center gap-3">
            {home.tiles.map((tile) => (
              <Tile key={tile.os} tile={tile} onStart={start} />
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function Tile({ tile, onStart }: { tile: EmptyTile; onStart: (t: EmptyTile) => void }) {
  return (
    <button
      type="button"
      data-empty-tile={tile.os}
      aria-label={`${tile.name}, ${tile.detail}`}
      onClick={() => onStart(tile)}
      className="flex h-20 w-72 cursor-default items-center gap-3 rounded-2xl bg-card px-4 text-left shadow-[0_0_0_0.5px_hsl(var(--shadow-color)/0.06),0_2px_10px_hsl(var(--shadow-color)/0.08)] outline-none transition-[background-color,box-shadow,scale] duration-150 hover:shadow-[0_0_0_0.5px_hsl(var(--shadow-color)/0.1),0_4px_16px_hsl(var(--shadow-color)/0.13)] active:scale-[0.98] focus-visible:ring-2 focus-visible:ring-ring/60 dark:bg-white/[0.06] dark:hover:bg-white/[0.09]"
    >
      <span className="flex size-11 shrink-0 items-center justify-center rounded-[10px] bg-foreground/[0.06]">
        <OsIcon os={tile.os} className="size-6" />
      </span>
      <span className="min-w-0">
        <span className="block text-[13px] font-semibold">{tile.name}</span>
        <span data-empty-tile-detail="" className="mt-0.5 block truncate text-xs text-muted-foreground">
          {tile.detail}
        </span>
      </span>
    </button>
  );
}

/** SF Symbols' `desktopcomputer`, filled, in the secondary color. */
function MonitorGlyph() {
  return (
    <svg viewBox="0 0 40 34" aria-hidden className="h-[34px] w-10 text-muted-foreground/40">
      <rect x="0.5" y="0.5" width="39" height="26" rx="3.5" fill="currentColor" />
      <rect x="2.5" y="2.5" width="35" height="19" rx="1.5" className="fill-background" opacity="0.5" />
      <path d="M15 27h10l1 4.5h3v2H11v-2h3z" fill="currentColor" />
    </svg>
  );
}
