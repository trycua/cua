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
 * - One block with the app's one radius and a fill (no outline). The page sizes it
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
 * Nothing sits under the block (the header's picture-in-picture button is
 * the way out to a window of its own): while the desktop has the keyboard,
 * a small bar over its bottom edge says how to take keys back (Control+Option
 * pressed and released alone, a click outside, or Stop controlling), and
 * the host cuts the video out under that bar (`data-video-hole`). Anywhere
 * else it is the placeholder, and the desktop opens in its own native window.
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
  /** One of the Space's windows instead of its desktop (the Stream rail's pick). */
  windowId?: string | null;
  /** That window's app ("Firefox"), named where the window opens on its own. */
  windowName?: string | null;
  /** Pointer and keys go to the Space (default). False only shows it (Watch). */
  interactive?: boolean;
}

/** How long the desktop may take to send its first picture. After that the
 * cover says it didn't, why, and offers Try again: the viewer never waits
 * silently. */
export const CONNECT_TIMEOUT_MS = 20_000;
/** How long "Connecting…" may go on before the cover says it is taking longer
 * than usual (a Space that was just turned on, a machine far away). */
export const SLOW_CONNECT_MS = 4_000;
/** The cover's second line once a connect is slow: plain words, and it goes
 * with the picture. */
export const SLOW_CONNECT_HINT = "Still connecting. This is taking longer than usual.";
/** Why, when it timed out. */
export const CONNECT_TIMEOUT_REASON = "No picture arrived from the desktop within 20 seconds.";

/** True once `waiting` has held for `ms` without a break; a new `key` (another
 * Space, or Try again) starts the wait over. */
function useTimedOut(waiting: boolean, ms: number, key: string): boolean {
  const [over, setOver] = useState<string | null>(null);
  useEffect(() => {
    // A wait that ended (a picture came, or the slot went away) is forgotten:
    // the next wait under the same key starts from zero, never already over.
    if (!waiting) {
      setOver(null);
      return;
    }
    const t = setTimeout(() => setOver(key), ms);
    return () => clearTimeout(t);
  }, [waiting, ms, key]);
  return waiting && over === key;
}

/** The root's radius (`rounded-lg`, the app's one radius) and border (none: a fill), for the native layer's clip. */
const SURFACE_RADIUS = 8;
const SURFACE_BORDER = 0;

export function StreamSurface({ spaceId, os, aspectRatio = 16 / 10, canStream, previewText, access, onAccessAction, children, className, windowId, windowName, interactive = true }: StreamSurfaceProps) {
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
    interactive,
    radius: SURFACE_RADIUS,
    inset: SURFACE_BORDER,
    os,
    ...(windowId ? { windowId } : {}),
  });
  // Still no picture after CONNECT_TIMEOUT_MS: failed, with the reason and
  // Try again (a picture that arrives later still shows).
  const timedOut = useTimedOut(slot?.phase === "connecting", CONNECT_TIMEOUT_MS, `${spaceId}:${attempt}`);
  const slow = useTimedOut(slot?.phase === "connecting", SLOW_CONNECT_MS, `${spaceId}:${attempt}`);
  const phase: StreamPhase = !slot ? "nosession" : slot.phase === "live" ? "streaming" : slot.phase === "failed" || timedOut ? "failed" : "connecting";
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
  const state = slot ? (timedOut ? "failed" : slot.phase) : covered && cover.kind === "connecting" ? "connecting" : "placeholder";
  // Why the cover says it could not connect, on the cover itself.
  const reason = timedOut ? CONNECT_TIMEOUT_REASON : slot?.phase === "failed" ? (slot.reason ?? null) : null;
  const dark = covered && (Boolean(thumbnail) || cover.kind !== "status");
  return (
    <div
      ref={ref}
      data-stream-surface={spaceId}
      data-stream-state={state}
      data-aspect={aspectRatio.toFixed(4)}
      className={cn("relative w-full overflow-hidden rounded-lg", dark ? "bg-black" : "bg-muted/40", className)}
    >
      {covered ? (
        cover.kind === "stream" ? null : (
          <DesktopCoverView
            os={os}
            cover={cover}
            thumbnail={access ? null : thumbnail}
            reason={cover.kind === "status" ? reason : null}
            hint={cover.kind === "connecting" && slow ? SLOW_CONNECT_HINT : null}
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
                  <p className="text-[13px] text-muted-foreground">{windowId && windowName ? `${windowName} opens in its own window.` : "The desktop opens in its own window."}</p>
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
      {slot?.phase === "live" && slot.focused ? (
        // While the desktop has the keyboard: how to give it back, over the
        // video's bottom edge. The host cuts the video out under it
        // (`data-video-hole`), so no strip under the stream is needed.
        <div
          data-video-focus="space"
          data-video-hole=""
          className="glass absolute bottom-2 left-1/2 flex max-w-[calc(100%-1rem)] -translate-x-1/2 items-center gap-2 rounded-lg py-1 pr-1 pl-3 text-xs text-muted-foreground shadow-float"
        >
          <p className="min-w-0 truncate">
            {isMacPlatform() ? (
              <>
                Keys go to this Space. Press and release <Kbd>⌃⌥</Kbd> to stop.
              </>
            ) : (
              <>
                Keys go to this Space. Press <Kbd>Ctrl+Shift+F12</Kbd> to stop.
              </>
            )}
          </p>
          <Button variant="ghost" size="sm" onClick={() => slot.setFocus(false)}>
            Stop controlling
          </Button>
        </div>
      ) : null}
    </div>
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
  reason,
  hint,
  onButton,
  onAction,
  onOpen,
}: {
  os: SpaceOs;
  cover: DesktopCover;
  thumbnail: string | null;
  /** Why it could not connect, under the line. */
  reason?: string | null;
  /** While it connects slowly: that it is still going. */
  hint?: string | null;
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
        {reason ? (
          <p data-cover-reason="" className={cn("mt-1 max-w-md text-xs", dark ? "text-white/60" : "text-muted-foreground")}>
            {reason}
          </p>
        ) : null}
        {hint ? (
          <p data-cover-hint="" className="mt-1 max-w-md text-xs text-white/60">
            {hint}
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
