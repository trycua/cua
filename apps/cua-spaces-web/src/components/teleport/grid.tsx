// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { AppWindowIcon } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";

import { GRID_COLUMNS, iconKey, thumbnailKey, type PickerTile, type PickerTileIcon, type PickerTileThumbnail, type TeleportSession, type TeleportStore } from "@/bridge";
import { cn } from "@/lib/utils";

/** What a tile asks the host for, as `grid_frame` in the core's parity writes it. */
const iconSource = (i: PickerTileIcon) =>
  i.kind === "host" ? `host ${i.path}` : i.kind === "guest" ? `guest ${i.appName} ${i.appId} ${i.pid}` : "-";
const thumbnailSource = (t: PickerTileThumbnail) =>
  t.kind === "host-window" ? `window ${t.windowId}` : t.kind === "guest-window" ? `guest ${t.windowId}@${t.epoch}` : "-";

/**
 * The picker's tiles (the core's `grid.apps`, `grid.windows`, `grid.remote`):
 * a title per section, three columns of cards with the live preview, the
 * app's icon and one line of name. What the app can take is the tooltip; an
 * app that cannot move is dimmed. Arrow keys move the selection, Return and
 * a double click open it.
 */
export function TeleportGrid({ session: s, teleport, images }: { session: TeleportSession; teleport: TeleportStore; images: ReadonlyMap<string, string | null> }) {
  const onKeyDown = (e: KeyboardEvent) => {
    const delta = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: GRID_COLUMNS, ArrowUp: -GRID_COLUMNS }[e.key];
    if (delta !== undefined) {
      e.preventDefault();
      teleport.step(delta);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void teleport.activate();
    }
  };
  return (
    <div role="listbox" aria-label="Apps and windows" tabIndex={0} onKeyDown={onKeyDown} className="rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring/60">
      {s.grid.sections.map((section, i) => (
        <section key={`${section.title}-${i}`} data-grid-section={section.title} className="mb-4 last:mb-0">
          {section.title ? <h3 className="mb-2 text-xs font-medium text-muted-foreground">{section.title}</h3> : null}
          <div className="grid grid-cols-3 gap-3">
            {section.tiles.map((tile) => (
              <Tile key={tile.id} tile={tile} teleport={teleport} images={images} />
            ))}
          </div>
        </section>
      ))}
      {s.grid.emptyText ? (
        <p className="py-10 text-center text-[13px] text-muted-foreground" data-grid-empty>
          {s.grid.emptyText}
        </p>
      ) : null}
    </div>
  );
}

function Tile({ tile, teleport, images }: { tile: PickerTile; teleport: TeleportStore; images: ReadonlyMap<string, string | null> }) {
  // Only tiles in view ask the host for their icon and preview.
  const ref = useRef<HTMLDivElement | null>(null);
  const [seen, setSeen] = useState(typeof IntersectionObserver === "undefined");
  useEffect(() => {
    const el = ref.current;
    if (seen || !el) return;
    const io = new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && setSeen(true), { rootMargin: "200px" });
    io.observe(el);
    return () => io.disconnect();
  }, [seen]);
  useEffect(() => {
    if (!seen) return;
    teleport.icon(tile);
    teleport.thumbnail(tile);
  }, [teleport, tile, seen]);
  const icon = images.get(iconKey(tile.icon)) ?? null;
  const thumbnail = images.get(thumbnailKey(tile.thumbnail)) ?? null;
  return (
    <div
      ref={ref}
      role="option"
      aria-selected={tile.selected}
      aria-disabled={tile.disabled}
      title={tile.help}
      data-tile={tile.id}
      data-selected={tile.selected || undefined}
      data-disabled={tile.disabled || undefined}
      data-icon={iconSource(tile.icon)}
      data-thumbnail={thumbnailSource(tile.thumbnail)}
      onClick={() => !tile.disabled && teleport.select(tile.id)}
      onDoubleClick={() => void teleport.activate(tile)}
      className={cn("min-w-0 cursor-default", tile.disabled && "opacity-50")}
    >
      <div
        className={cn(
          "relative flex h-26 items-center justify-center overflow-hidden rounded-lg border bg-muted/70 transition-[border-color,box-shadow]",
          tile.selected ? "border-brand ring-2 ring-brand/40" : "hover:border-foreground/20",
        )}
      >
        {thumbnail ? (
          <img src={thumbnail} alt="" className="size-full object-cover" />
        ) : icon ? (
          <img src={icon} alt="" className="size-11" />
        ) : (
          <AppWindowIcon className="size-7 text-muted-foreground/40" />
        )}
      </div>
      <div className="mt-1.5 flex items-center gap-1.5 text-[13px]">
        {icon ? <img src={icon} alt="" className="size-4 shrink-0" /> : null}
        <span className="truncate" data-tile-title>
          {tile.title}
        </span>
      </div>
      {/* Why an app cannot move (the core's reason), not only in a tooltip. */}
      {tile.disabled && tile.help ? (
        <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground" data-tile-reason>
          {tile.help}
        </p>
      ) : null}
    </div>
  );
}
