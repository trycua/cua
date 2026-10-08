// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { AppWindowIcon } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { useDesktopCover, useSettings, useSpaces, useSpaceThumbnail, type DesktopCover, type MachineAccessNotice, type SpaceOs, type StreamPhase } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { toast } from "@/components/ui/toast";
import { useVideoSlot, videoHostPresent } from "@/components/video/use-video-slot";
import { cn, isMacPlatform } from "@/lib/utils";

/**
 * Where a Space's live desktop goes.
 *
 * Layout contract:
 * - One block with the app's card border and radius. The page sizes it
 *   (`className`); it never resizes itself, so nothing around it moves when
 *   video starts or stops.
 * - The video fits inside at the guest display's aspect ratio
 *   (`aspectRatio`, also on the root as `data-aspect`), letterboxed and
 *   centred.
 * - The root carries `data-stream-surface={spaceId}` and `data-stream-state`:
 *   `placeholder` (no video here), `connecting`, `live` (frames draw) or
 *   `failed` (the host couldn't stream it: the cover's Try again, with
 *   Open window; when no stream could be opened at all, an error toast says
 *   why, once per attempt).
 * - `children` replaces the stream entirely (a create's progress, a
 *   stopped Space).
 *
 * Native video (apps/cua-spaces-macos/docs/native-video.md): when the host
 * registers the `cuaVideo` handler (the SwiftUI shell, unless its
 * `WebUINativeVideo` default is NO) and the Space can stream, the host draws
 * the Space's desktop over the block and sends it pointer, scroll and keys.
 * A line under the block says where keys go and how to take them back
 * (Control+Option pressed and released alone, or a click outside). Anywhere else it is the placeholder, and the
 * desktop opens in its own native window.
 */
export interface StreamSurfaceProps {
  spaceId: string;
  /** For the placeholder's mark. */
  os: SpaceOs;
  /** Guest display width / height; 16:10 until known. */
  aspectRatio?: number;
  /** Streaming is possible (the core's `canStream`). */
  canStream: boolean;
  /** What shows when it can't stream (the core's `previewText`: "Not reachable"). */
  previewText: string;
  /** Signed in, but this device is not enrolled (the detail's `access`):
   * the core's cover says why, with Connect greyed out and the action that
   * fixes it, in place of everything else. */
  access?: MachineAccessNotice | null;
  /** The access line's action ("Enroll This Mac…"). */
  onAccessAction?: () => void;
  /** Shown instead of the stream (progress, stopped). */
  children?: ReactNode;
  className?: string;
}

/** The root's radius (`rounded-xl`) and border, for the native layer's clip. */
const SURFACE_RADIUS = 12;
const SURFACE_BORDER = 1;

export function StreamSurface({ spaceId, os, aspectRatio = 16 / 10, canStream, previewText, access, onAccessAction, children, className }: StreamSurfaceProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const settings = useSettings();
  const autoConnect = settings.data?.values.autoConnect ?? true;
  // Nothing opens before the setting is read (it may say wait for Connect).
  const settingsRead = settings.data !== undefined || settings.error != null;
  // Connect (or Try again) pressed for this Space; a new attempt opens the
  // stream again (`SpaceDetailView.connectRequested` and `press`).
  const [pressed, setPressed] = useState<{ spaceId: string; attempt: number } | null>(null);
  const requested = pressed?.spaceId === spaceId;
  const attempt = requested ? pressed.attempt : 0;
  const videoHere = videoHostPresent();
  // The page's own content (a create's progress, a stopped Space) replaces
  // the stream, unless this device may not connect at all.
  const replaced = children != null && !access;
  const wanted = canStream && !replaced && !access && ((settingsRead && autoConnect) || requested);
  const slot = useVideoSlot(ref, {
    active: wanted,
    attempt,
    spaceId,
    tier: "full",
    interactive: true,
    radius: SURFACE_RADIUS,
    inset: SURFACE_BORDER,
    os,
  });
  const phase: StreamPhase = !slot ? "nosession" : slot.phase === "live" ? "streaming" : slot.phase === "failed" ? "failed" : "connecting";
  const cover = useDesktopCover({ canStream, previewText, autoConnect, connectRequested: requested, stream: phase, access });
  const thumbnail = useSpaceThumbnail(spaceId, videoHere && canStream && !replaced && !access);
  const press = () => setPressed({ spaceId, attempt: attempt + (cover.kind === "status" ? 1 : 0) });
  const { data: spaces, openSpace } = useSpaces();
  const name = spaces?.find((s) => s.id === spaceId)?.name ?? spaceId;
  // No stream could be opened (the SwiftUI detail's provider failure): the
  // cover offers Try again, and the reason shows once per attempt, as that
  // detail's banner ("Could not open …"). Nothing asks again by itself: a
  // poll or an approval leaves the failure as it is.
  const openFailure = slot?.phase === "failed" && slot.opening ? (slot.reason ?? "") : null;
  useEffect(() => {
    if (openFailure === null) return;
    toast(`Could not open ${name}`, { description: openFailure || undefined, type: "error" });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per failed attempt (each starts as connecting), not per rename
  }, [openFailure]);
  const open = () =>
    openSpace(spaceId).then(
      () => toast(`Opening ${name}`, { description: "Its desktop opens in a separate window." }),
      (e: unknown) => toast(`Couldn't open ${name}`, { description: e instanceof Error ? e.message : String(e), type: "error" }),
    );
  // Where the host draws video, the core's cover (or nothing over live
  // video); elsewhere the placeholder, and the desktop in its own window.
  // Asked for video and got none: this shell has none for this page (no
  // cua, sample data), so the placeholder after all.
  const noVideoHere = wanted && slot === null;
  const covered = !replaced && (Boolean(access) || (videoHere && !noVideoHere));
  const state = slot ? slot.phase : covered && cover.kind === "connecting" ? "connecting" : "placeholder";
  const dark = covered && (Boolean(thumbnail) || cover.kind !== "status");
  return (
    <>
      <div
        ref={ref}
        data-stream-surface={spaceId}
        data-stream-state={state}
        data-aspect={aspectRatio.toFixed(4)}
        className={cn("relative w-full overflow-hidden rounded-xl border shadow-xs", dark ? "bg-black" : "bg-muted/40", className)}
      >
        {covered ? (
          cover.kind === "stream" ? null : (
            <DesktopCoverView
              os={os}
              cover={cover}
              thumbnail={access ? null : thumbnail}
              onButton={press}
              onAction={onAccessAction}
              onOpen={slot?.phase === "failed" ? () => void open() : undefined}
            />
          )
        ) : (
          <div className="absolute inset-0 flex flex-col items-center justify-center px-6 text-center">
            {children ?? (
              <>
                <OsIcon os={os} className="mb-4 size-8 text-muted-foreground/30" />
                {canStream ? (
                  <>
                    <p className="text-[13px] text-muted-foreground">The desktop opens in its own window.</p>
                    <Button className="mt-4" onClick={() => void open()}>
                      <AppWindowIcon /> Open window
                    </Button>
                  </>
                ) : (
                  <p data-preview-text className="text-[13px] text-muted-foreground">
                    {previewText}
                  </p>
                )}
              </>
            )}
          </div>
        )}
      </div>
      {slot ? (
        <div data-video-focus={slot.focused ? "space" : "page"} className="mt-2 flex h-7 items-center justify-between gap-3 px-1 text-xs text-muted-foreground">
          <p className="min-w-0 truncate">
            {slot.phase !== "live" ? null : slot.focused ? (
              <>
                {isMacPlatform() ? (
                  <>
                    Keys go to this Space. Press and release <Kbd>⌃⌥</Kbd> to stop.
                  </>
                ) : (
                  <>
                    Keys go to this Space. Press <Kbd>Ctrl+Shift+F12</Kbd> to stop.
                  </>
                )}
              </>
            ) : (
              "Click the desktop to control it."
            )}
          </p>
          <div className="flex shrink-0 items-center gap-1">
            {slot.phase === "live" && slot.focused ? (
              <Button variant="ghost" size="sm" onClick={() => slot.setFocus(false)}>
                Stop controlling
              </Button>
            ) : null}
            {slot.phase !== "failed" ? (
              <Button variant="ghost" size="sm" onClick={() => void open()}>
                <AppWindowIcon /> Open in window
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
    </>
  );
}

/**
 * What the preview shows over (or instead of) the live desktop, as the
 * core says (`spaces.desktopCover`, the SwiftUI app's `DesktopCoverView`):
 * the Space's latest thumbnail strongly blurred and slightly dimmed, with
 * "Connecting…", Connect, or a line and Try again centered on it. With no
 * thumbnail: black while it connects (or waits for Connect), the card's
 * quiet fill for a Space that cannot stream. Signed in but not enrolled:
 * why, its action, and Connect greyed out.
 */
function DesktopCoverView({
  os,
  cover,
  thumbnail,
  onButton,
  onAction,
  onOpen,
}: {
  os: SpaceOs;
  cover: DesktopCover;
  thumbnail: string | null;
  onButton: () => void;
  onAction?: () => void;
  /** The stream failed here: its own window instead. */
  onOpen?: () => void;
}) {
  const dark = Boolean(thumbnail) || cover.kind !== "status";
  return (
    <div data-desktop-cover={cover.kind} className={cn("absolute inset-0 flex flex-col items-center justify-center px-6 text-center", dark && "dark text-white")}>
      {thumbnail ? (
        <>
          <img data-cover-thumbnail="" src={thumbnail} alt="" aria-hidden className="absolute inset-0 size-full scale-110 object-cover blur-2xl" />
          <div className="absolute inset-0 bg-black/30" />
        </>
      ) : null}
      <div className="relative flex flex-col items-center">
        {!dark ? <OsIcon os={os} className="mb-4 size-8 text-muted-foreground/30" /> : null}
        {cover.text ? (
          <p
            data-cover-text=""
            {...(cover.kind === "status" ? { "data-preview-text": "" } : {})}
            className={cn("max-w-md", cover.kind === "status" ? "text-[13px]" : "text-[15px]", dark ? "text-white/70" : "text-muted-foreground")}
          >
            {cover.text}
          </p>
        ) : null}
        {cover.action ? (
          <button
            type="button"
            className={cn("mt-2 text-[13px] outline-none hover:underline focus-visible:underline", dark ? "text-white" : "text-brand-strong")}
            onClick={onAction}
            data-access-action=""
          >
            {cover.action}
          </button>
        ) : null}
        {cover.button || onOpen ? (
          <div className="mt-4 flex gap-2">
            {cover.button ? (
              <Button
                size={cover.kind === "connect" ? "lg" : "default"}
                disabled={cover.buttonDisabled}
                title={cover.buttonHelp ?? undefined}
                onClick={onButton}
                data-cover-button={cover.kind === "connect" ? "connect" : "retry"}
              >
                {cover.button}
              </Button>
            ) : null}
            {onOpen ? (
              <Button variant="outline" onClick={onOpen}>
                <AppWindowIcon /> Open window
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}
