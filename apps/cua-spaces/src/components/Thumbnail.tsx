// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceStatus, ThumbnailScene } from "../model/types";
import { RadialProgress } from "./RadialProgress";

interface ThumbnailProps {
  scene: ThumbnailScene;
  status: SpaceStatus;
  /** Real screenshot (data URL). When present it replaces the synthetic scene. */
  imageUrl?: string | null;
  /** Status label shown under the progress pie while starting. */
  statusText?: string;
  /** One quiet line under the status while it downloads
   * ("4.2 of 23.9 GB · 85 MB/s · about 4 min"). */
  progressText?: string;
  /** Epoch ms the Space was created; anchors the progress pie. */
  startedAt?: number;
  /** Real progress while starting (0 to 1, the SDK's create progress);
   * without it the pie eases on an estimate. */
  fraction?: number;
  /** The Space is being deleted: show a "Deleting…" overlay until the tile
   * disappears. */
  deleting?: boolean;
  /** A Space on this Mac (Lume macOS): its lifecycle is much faster than the
   * cloud's, so the progress pie is calibrated to the measured
   * clone/boot/teardown times. */
  local?: boolean;
}

// Measured Lume lifecycle (avg of 3 clone→ready→teardown cycles): clone ~2s +
// boot→running ~15s ≈ ~17s to a streamable/running tile; stop ~7s + delete ~2s
// ≈ ~8s teardown. Cua Cloud keeps the default ~80s estimate.
const LOCAL_START_ESTIMATE_MS = 17_000;
const LOCAL_DELETE_ESTIMATE_MS = 8_000;

/**
 * Space thumbnail. With a real screenshot (live data) it shows the
 * actual desktop; otherwise a synthetic scene drawn entirely with CSS.
 * Animations respect `prefers-reduced-motion`.
 */
export function Thumbnail({
  scene,
  status,
  imageUrl,
  statusText,
  progressText,
  startedAt,
  fraction,
  deleting,
  local,
}: ThumbnailProps) {
  const live = status === "running" || status === "local";
  // Overlays render over BOTH the screenshot and the synthetic scene, so a
  // starting tile shows the pie even if it has a cached shot, and a
  // delete shows immediately.
  const overlay = deleting ? (
    <span className="thumb-provisioning">
      <RadialProgress
        className="thumb-radial"
        estimateMs={local ? LOCAL_DELETE_ESTIMATE_MS : undefined}
      />
      <span className="thumb-provisioning-status">Deleting…</span>
    </span>
  ) : status === "provisioning" ? (
    <span className="thumb-provisioning">
      <RadialProgress
        startedAt={startedAt}
        className="thumb-radial"
        fraction={fraction}
        estimateMs={local ? LOCAL_START_ESTIMATE_MS : undefined}
      />
      {statusText && <span className="thumb-provisioning-status">{statusText}</span>}
      {progressText && <span className="thumb-provisioning-detail">{progressText}</span>}
    </span>
  ) : status === "suspended" ? (
    <span className="thumb-dim" />
  ) : null;

  if (imageUrl) {
    return (
      <span className={`thumb thumb-live`} data-live={live ? "on" : "off"} data-status={status} aria-hidden="true">
        <img className="thumb-img" src={imageUrl} alt="" />
        {overlay}
      </span>
    );
  }
  return (
    <span className={`thumb scene-${scene}`} data-live={live ? "on" : "off"} data-status={status} aria-hidden="true">
      {scene === "mac-desktop" && (
        <>
          <span className="thumb-menubar" />
          <span className="thumb-window thumb-window-a">
            <span className="thumb-titlebar" />
            <span className="thumb-line w60" />
            <span className="thumb-line w40" />
          </span>
          <span className="thumb-window thumb-window-b thumb-terminal">
            <span className="thumb-line w70 is-green" />
            <span className="thumb-line w30" />
            <span className="thumb-cursor" />
          </span>
          <span className="thumb-dock" />
        </>
      )}
      {scene === "windows-desktop" && (
        <>
          <span className="thumb-window thumb-window-center">
            <span className="thumb-titlebar" />
            <span className="thumb-tiles" />
          </span>
          <span className="thumb-taskbar" />
          <span className="thumb-agent-pointer" />
        </>
      )}
      {scene === "linux-terminal" && (
        <>
          <span className="thumb-line w80 is-green" />
          <span className="thumb-line w50" />
          <span className="thumb-line w65" />
          <span className="thumb-line w35 is-amber" />
          <span className="thumb-progress" />
          <span className="thumb-cursor" />
        </>
      )}
      {scene === "notes" && (
        <>
          <span className="thumb-note-title" />
          <span className="thumb-line w90" />
          <span className="thumb-line w75" />
          <span className="thumb-line w85" />
          <span className="thumb-line w50" />
        </>
      )}
      {scene === "browser" && (
        <>
          <span className="thumb-window thumb-window-full">
            <span className="thumb-titlebar">
              <span className="thumb-urlbar" />
            </span>
            <span className="thumb-hero" />
            <span className="thumb-line w70" />
            <span className="thumb-line w45" />
          </span>
        </>
      )}
      {overlay}
    </span>
  );
}
