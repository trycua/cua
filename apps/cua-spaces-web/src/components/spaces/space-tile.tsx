// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space on the Spaces page's grid: no container, the picture is the tile
 * (one 8 px radius, edge to edge), the text sits on the page under it. Its
 * state is a quiet dot and word at the end of the name row, as the card had
 * before the redesign; power shows on hover. Files dropped on a running tile go into it,
 * and a real window dragged over it is offered to it (macOS).
 *
 * The controls over the picture (checkbox, power, badges) are
 * `data-video-hole`: where the host draws the live desktop natively over
 * the page (macOS), it cuts them out of the video while they show, so a
 * live tile has them as any other does (`lib/video-slots.ts`).
 */

import { Link } from "@tanstack/react-router";
import { ArrowDownCircleIcon, EyeOffIcon, KeyRoundIcon, LoaderCircleIcon, PowerIcon } from "lucide-react";
import { useEffect, useRef, useState, type DragEvent, type MouseEvent, type ReactNode } from "react";

import { useBridge, useSpaceFiles, useSpaces, useSpaceThumbnail, useTips, useWindowDropTarget, type Space, type SpaceAccess } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { StateLabel } from "@/components/state-dot";
import { toast, toastError } from "@/components/ui/toast";
import { useTileVideo } from "@/components/video/tile-video";
import { useInView } from "@/hooks/use-in-view";
import { creatingLine } from "@/lib/host-labels";
import { createError, createProgress, osLabel, powerButton, powerError, type PowerButton } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import { PeopleStack, SpaceMark } from "./avatars";
import { desktopWord, type SpaceItem } from "./model";

export interface SpaceTileProps {
  item: SpaceItem;
  /** A live Keyvault sign-in is in it: its access key (Signed in badge). */
  accessKey?: string | null;
  /** Ticked: a ring around the picture. */
  selected?: boolean;
  /** Being worked on by a bulk action: dimmed, with a spinner. */
  busy?: boolean;
  /** Over the picture's top left (the checkbox). */
  topLeft?: ReactNode;
  /** After the text, on hover (the "⋯" menu). */
  more?: ReactNode;
  /** A machine's desktop: Hide, in place of the power button, on hover. */
  hide?: () => void;
  /** A click on the tile; call `preventDefault` to keep it from opening. */
  onClick?: (e: MouseEvent) => void;
}

/**
 * A tile's grid cell (`<li>`). Off-screen cells skip layout and paint
 * (`content-visibility`), which also clips what they paint to their box: the
 * cell keeps 4 px of room around the tile (padding, taken back by a negative
 * margin so the grid doesn't move) for the selected and focus rings drawn
 * just outside the picture.
 */
export const TILE_CELL = "-m-1 p-1 [content-visibility:auto] [contain-intrinsic-size:auto_240px]";

export function SpaceTile({ item, accessKey, selected = false, busy = false, topLeft, more, hide, onClick }: SpaceTileProps) {
  const { space, state, access } = item;
  const drop = useTileDrop(space, state === "running" && item.desktop !== "off");
  // A real window dragged over this tile (the host hit-tests the tiles' boxes and says which; macOS only).
  const windowOver = useWindowDropTarget() === space.id;
  const creating = state === "creating";
  const { core } = useBridge();
  // Shown wherever the Space has power at all; disabled, with why, where you may not use it.
  const power = item.desktop ? null : powerButton(core, space);
  const hasPower = power !== null;
  const hasAction = hasPower || Boolean(hide);
  return (
    <div
      className={cn("group/tile relative", (busy || state === "deleting") && "opacity-60")}
      data-video-hole-scope=""
      data-tile-drop={drop.over ?? undefined}
      data-drop-status={drop.status?.kind}
      data-window-drop={space.id}
      data-window-over={windowOver || undefined}
      {...drop.handlers}
    >
      <Link
        to="/spaces/$spaceId"
        params={{ spaceId: space.id }}
        data-space-id={space.id}
        data-state={state}
        data-selected={selected || undefined}
        onClick={onClick}
        className="group/link block cursor-default rounded-lg outline-none"
      >
        <Picture item={item} ring={selected || windowOver} busy={busy} />
        <div className="flex gap-3 pt-3">
          <SpaceMark space={space} access={access} dim={state !== "running"} />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-baseline gap-3">
              <p data-space-name="" className="min-w-0 flex-1 truncate text-[13px] leading-5 font-semibold">
                {space.name}
              </p>
              {item.desktop ? <DesktopTag /> : null}
              {/* The state, in the core's words ("Running", "Suspended", "Turning off…"), flush right; the hover "⋯" takes its place. */}
              <StateLabel
                state={item.desktop === "off" ? "stopped" : state}
                space={space}
                word={desktopWord(item)}
                className="shrink-0 group-has-[[data-space-more]]/tile:group-hover/tile:invisible group-has-[[data-space-more]]/tile:group-focus-within/tile:invisible group-has-[[data-popup-open]]/tile:invisible"
              />
            </div>
            {/* Where it runs ("This Mac", "Sam's Mac mini", "QA team"); while being created, the core's pending line. */}
            <p className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
              <span className="truncate">{creating && space.detail ? creatingLine(space.detail) : item.desktop && (access.place || item.machine) === space.name ? osLabel(space) : access.place || item.machine}</span>
              <PeopleStack access={access} />
            </p>
            {/* Downloading: how far, how fast, how long is left. */}
            {creating && space.progress?.transfer ? (
              <p data-create-transfer="" className="mt-0.5 truncate text-xs tabular-nums text-muted-foreground">
                {space.progress.transfer}
              </p>
            ) : null}
            {createError(space) ? (
              <p data-create-error="" title={createError(space)!} className="mt-1 line-clamp-2 text-xs text-destructive">
                {createError(space)}
              </p>
            ) : powerError(space) ? (
              // The last start or stop failed: why, inline, as the SwiftUI sidebar row says it.
              <p data-power-error="" title={powerError(space)!} className="mt-1 line-clamp-2 text-xs text-destructive">
                {powerError(space)}
              </p>
            ) : null}
          </div>
        </div>
      </Link>
      {drop.over ? <DropOverlay space={space} accepts={drop.over === "accept"} /> : null}
      {/* A drop on the tile still going in: the core's "Sending …" line. */}
      {drop.status?.kind === "working" && !drop.over ? (
        <div data-video-hole="" className="pointer-events-none absolute top-2 left-2 z-20 flex max-w-[60%] items-center gap-1.5 rounded-[4px] bg-black/70 px-1.5 py-px text-[11px] font-medium text-white">
          <LoaderCircleIcon className="size-3 shrink-0 animate-spin" aria-hidden />
          <span className="truncate">{drop.status.text}</span>
        </div>
      ) : null}
      {topLeft ? <div className="absolute top-2 left-2 z-10">{topLeft}</div> : null}
      {/* The sign-in badge hugs the right edge; power takes its place beside it on hover. */}
      <div className="absolute top-2 right-2 z-10 flex items-center justify-end gap-1.5">
        {/* A live Keyvault sign-in is in this Space: the badge opens Access with its row brought forward. */}
        {accessKey !== undefined ? (
          <Link
            to="/keyvault"
            search={{ view: "access", focus: accessKey ?? undefined }}
            data-signed-in={space.id}
            data-video-hole=""
            title="A Keyvault sign-in is live in this Space. Open Access."
            className={cn(
              "ml-auto inline-flex h-6 items-center gap-1 rounded-md bg-black/55 px-1.5 text-[11px] font-medium text-white/90 outline-none backdrop-blur-sm transition-[margin] hover:text-white focus-visible:ring-2 focus-visible:ring-ring/60",
              hasAction && "group-hover/tile:mr-7 group-focus-within/tile:mr-7",
            )}
          >
            <KeyRoundIcon className="size-3" /> Signed in
          </Link>
        ) : null}
        {hasPower ? (
          <div className="absolute right-0 top-0">
            <TilePower space={space} button={power} denied={access.canPower ? null : cantPower(access)} />
          </div>
        ) : hide ? (
          <div className="absolute right-0 top-0">
            <TileHide name={space.name} onHide={hide} />
          </div>
        ) : null}
      </div>
      {more ? <div className="absolute top-[calc(100%-2.75rem)] right-0 z-10">{more}</div> : null}
    </div>
  );
}

/** The "Desktop" tag beside a machine's own desktop in the list. */
export function DesktopTag() {
  return (
    <span data-desktop-tag="" className="shrink-0 text-xs text-muted-foreground">
      Desktop
    </span>
  );
}

/** Hide, on a desktop's hover where a Space has its power button: the row leaves the list until "Show hidden". */
function TileHide({ name, onHide }: { name: string; onHide: () => void }) {
  return (
    <button
      type="button"
      data-tile-hide=""
      data-video-hole=""
      title="Hide"
      aria-label={`Hide ${name}`}
      onClick={onHide}
      className="inline-flex size-6 items-center justify-center rounded-lg bg-black/55 text-white/90 opacity-0 outline-none backdrop-blur-sm group-focus-within/tile:opacity-100 group-hover/tile:opacity-100 hover:bg-black/70 hover:text-white focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      <EyeOffIcon className="size-3" aria-hidden />
    </button>
  );
}

/** The picture: its latest capture (dimmed while off) or its OS mark, live video over it while it runs. */
function Picture({ item, ring, busy }: { item: SpaceItem; ring: boolean; busy: boolean }) {
  const { space, state } = item;
  const ref = useRef<HTMLDivElement | null>(null);
  // Pictures and live video only for the tiles on screen (or close to it).
  const inView = useInView(ref);
  const video = useTileVideo(ref, space, inView);
  // Its latest capture (the notch's image), until live video draws over it.
  const thumbnail = useSpaceThumbnail(space.id, inView && (state === "running" || state === "stopped"));
  const live = state === "running";
  return (
    <div
      ref={ref}
      data-tile-picture=""
      data-stream-state={video?.phase}
      className={cn(
        "relative flex aspect-[16/10] items-center justify-center overflow-hidden rounded-lg bg-tile ring-offset-2 ring-offset-background transition-shadow",
        // Both rings sit just outside the picture (a live tile's video covers the inside), in the room `TILE_CELL` keeps.
        "group-focus-visible/link:ring-2 group-focus-visible/link:ring-ring/60",
        ring && "ring-2 ring-brand",
      )}
    >
      {thumbnail ? (
        <img data-tile-thumbnail="" src={thumbnail} alt="" aria-hidden className={cn("absolute inset-0 size-full object-cover", !live && "opacity-40 saturate-50")} />
      ) : (
        <OsIcon os={space.os} className={cn("size-6", live ? "text-muted-foreground/50" : "text-muted-foreground/25")} />
      )}
      {state === "creating" ? (
        <div className="absolute inset-x-3 bottom-8">
          <div className="h-1 overflow-hidden rounded-full bg-foreground/10">
            <div data-create-progress className="h-full rounded-full bg-brand" style={{ width: `${Math.round(createProgress(space) * 100)}%` }} />
          </div>
        </div>
      ) : null}
      {busy || state === "deleting" ? (
        <div data-tile-busy="" data-video-hole="" className="absolute inset-0 z-10 flex items-center justify-center bg-background/40">
          <LoaderCircleIcon className="size-5 animate-spin text-muted-foreground" aria-hidden />
        </div>
      ) : null}
    </div>
  );
}

/**
 * Files dropped from Finder on a running Space's tile go into that Space,
 * as a drop on its page's well does (`useSpaceFiles`: the host reads the
 * paths from the drop itself; an app bundle opens Teleport at that app).
 * While files are over the tile it says where they go; what the drop did
 * shows on the tile and in a toast.
 */
function useTileDrop(space: Space, accepts: boolean) {
  const files = useSpaceFiles(space);
  const [over, setOver] = useState<"accept" | "refuse" | null>(null);
  const status = files.status;
  useEffect(() => {
    if (!status) return;
    const group = `tile-drop:${space.id}`;
    if (status.kind === "working") toast(status.text, { group });
    else if (status.kind === "done") toast(`Sent to ${space.name}`, { description: status.text, type: "success", group });
    else toast(`Couldn't send to ${space.name}`, { description: status.text, type: "error", group });
  }, [status, space.id, space.name]);
  const carriesFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");
  const onDragOver = (e: DragEvent) => {
    if (!carriesFiles(e)) return;
    // Always handled here, so a drop on a tile never reaches the window.
    e.preventDefault();
    e.dataTransfer.dropEffect = accepts ? "copy" : "none";
    setOver(accepts ? "accept" : "refuse");
  };
  const onDragLeave = (e: DragEvent) => {
    // Leaving for one of the tile's own children is still over the tile.
    if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
    setOver(null);
  };
  const onDrop = (e: DragEvent) => {
    if (!carriesFiles(e)) return;
    e.preventDefault();
    setOver(null);
    if (!accepts) return;
    const names = Array.from(e.dataTransfer.files).map((f) => f.name);
    if (names.length) void files.drop(names);
  };
  return { over, status, handlers: { onDragEnter: onDragOver, onDragOver, onDragLeave, onDrop } };
}

/** Over a tile's picture while files are dragged on it: where they go, or why they can't. */
function DropOverlay({ space, accepts }: { space: Space; accepts: boolean }) {
  return (
    <div
      aria-hidden
      data-video-hole=""
      className={cn(
        "pointer-events-none absolute inset-x-0 top-0 z-20 flex aspect-[16/10] flex-col items-center justify-center gap-1.5 rounded-lg text-center text-[13px] backdrop-blur-[2px]",
        accepts ? "bg-brand/20 text-foreground ring-2 ring-brand" : "bg-background/75 text-muted-foreground",
      )}
    >
      {accepts ? <ArrowDownCircleIcon className="size-6 text-brand" strokeWidth={1.5} /> : null}
      <span className="max-w-[85%] truncate font-medium">{accepts ? `Send to ${space.name}` : `${space.name} isn't running`}</span>
    </div>
  );
}

/** Why you can't turn a Space on or off that has power (shared with you to watch). */
function cantPower(access: SpaceAccess): string {
  return access.role === "viewer" ? "You can view this Space but not turn it on or off" : "You can't turn this Space on or off";
}

/**
 * The core's power button, while the pointer is on the tile or focus is in
 * it, and always while it runs (the SwiftUI sidebar row's, which shows on
 * hover, on the selected row and while busy). `denied`: you may not use it
 * (a Space shared with you to watch); it shows disabled, saying why.
 */
function TilePower({ space, button, denied }: { space: Space; button: PowerButton; denied: string | null }) {
  const { startSpace, stopSpace } = useSpaces();
  const tips = useTips();
  const press = () => {
    if (denied) return;
    const [act, word] = button.turnOn ? [startSpace, "start"] : [stopSpace, "stop"];
    void tips.send({ type: "powered" });
    void act(space.id).catch(toastError(`Couldn't ${word} ${space.name}`));
  };
  return (
    <button
      type="button"
      data-tile-power={button.turnOn ? "on" : "off"}
      data-busy={button.busy || undefined}
      data-denied={denied ? "" : undefined}
      data-video-hole=""
      disabled={button.busy}
      aria-disabled={denied ? true : undefined}
      title={denied ?? button.help}
      aria-label={`${button.help} ${space.name}`}
      aria-description={denied ?? undefined}
      onClick={press}
      className={cn(
        "inline-flex size-6 items-center justify-center rounded-md bg-black/55 text-white/90 outline-none backdrop-blur-sm hover:bg-black/70 hover:text-white focus-visible:ring-2 focus-visible:ring-ring/60 disabled:cursor-default",
        !button.busy && "opacity-0 group-focus-within/tile:opacity-100 group-hover/tile:opacity-100",
        denied && "cursor-default text-white/45 hover:bg-black/55 hover:text-white/45",
      )}
    >
      {button.busy ? <LoaderCircleIcon className="size-3 animate-spin" aria-hidden /> : <PowerIcon className="size-3" aria-hidden />}
    </button>
  );
}
