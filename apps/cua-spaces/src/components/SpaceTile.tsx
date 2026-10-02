// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useRef, useState } from "react";

import { SPACES_CONFIG } from "../model/spaces";
import { getScreenshot, rememberScreenshot } from "../model/screenshotCache";
import type { Space } from "../model/types";
import { powerButton } from "../model/window";
import { useFleet } from "../state/cloud";
import { HotspotGlyph } from "./HotspotGlyph";
import { OsIcon } from "./OsIcon";
import { SfIcon } from "./SfIcon";
import { STATUS_LABEL, StatusDot } from "./StatusDot";
import { Thumbnail } from "./Thumbnail";

interface SpaceTileProps {
  space: Space;
  focused: boolean;
  onFocus: () => void;
  onSelect: () => void;
  /** Pin this Space as a picture-in-picture mirror (cloud Spaces only). */
  onPin?: () => void;
  /** Sync a local app's session to THIS Space (cloud Spaces only). */
  onShareApp?: () => void;
  /** This Space is borrowing the Mac's network — show a hotspot corner badge. */
  hotspot?: boolean;
  /** Delete this Space (a Space added by address is only removed). */
  onDelete?: () => void;
  /** Turn this Space off (`false`) or back on: the power button next to
   * Delete, for a Space that turns off and on. */
  onPower?: (on: boolean) => void;
  /** A window-drag is in progress: hide hover controls, act as a drop target. */
  dropMode?: boolean;
  /** Whether the dragged window is currently over this tile. */
  dropTarget?: boolean;
  /** Captured ghost of the dragged window (data URL). Shown inside this tile's
   * thumbnail only while it is the hovered drop target. */
  dropGhost?: string | null;
}

/** Minimum gap between hover-triggered screenshot fetches for one tile. */
const HOVER_PRELOAD_DEBOUNCE_MS = 1500;

/**
 * Poll a real screenshot for the tile at ~0.3 fps while it is visible and its
 * sandbox is bound, and expose a `preload` for hover (item C). Both the poll
 * and the hover preload write into the shared last-screenshot cache
 * (`screenshotCache`), which feeds the viewer's blurred connecting background
 * (item D) and the transfer overlay (item B). Synthetic tiles (no cloud ref /
 * not live) never poll and their preload is a no-op.
 */
function useLiveThumbnail(space: Space): { shot: string | null; preload: () => void } {
  const { live, bridge } = useFleet();
  // Seed from the shared cache so an established Space shows its last frame
  // immediately (no "connecting" flash) instead of the synthetic scene.
  const [shot, setShot] = useState<string | null>(() => getScreenshot(space.id)?.dataUrl ?? null);
  const registered = Boolean(space.sdk?.reachable);
  const running = space.status === "running";
  const spaceId = space.id;
  const lastPreloadRef = useRef(0);

  const ready = live && running && registered;

  useEffect(() => {
    if (!ready) {
      setShot(null);
      return;
    }
    let cancelled = false;
    const tick = async () => {
      try {
        const url = await bridge.screenshot(spaceId, SPACES_CONFIG.thumbnailMaxDimension);
        if (!cancelled) {
          setShot(url);
          rememberScreenshot(spaceId, url, Date.now());
        }
      } catch {
        // Keep the last frame (or the synthetic scene) on transient errors.
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), SPACES_CONFIG.thumbnailIntervalMs);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [ready, bridge, spaceId]);

  const preload = useCallback(() => {
    if (!ready) return;
    const now = Date.now();
    if (now - lastPreloadRef.current < HOVER_PRELOAD_DEBOUNCE_MS) return;
    lastPreloadRef.current = now;
    void bridge
      .screenshot(spaceId, SPACES_CONFIG.thumbnailMaxDimension)
      .then((url) => {
        setShot(url);
        rememberScreenshot(spaceId, url, Date.now());
      })
      .catch(() => {
        // Best-effort warm-up; ignore transient errors.
      });
  }, [ready, bridge, spaceId]);

  return { shot, preload };
}

export function SpaceTile({ space, focused, onFocus, onSelect, onPin, onShareApp, hotspot = false, onDelete, onPower, dropMode = false, dropTarget = false, dropGhost = null }: SpaceTileProps) {
  const ref = useRef<HTMLButtonElement>(null);
  const { shot, preload } = useLiveThumbnail(space);
  // The core's Deleting row: shows the instant Delete is pressed.
  const deleting = space.status === "deleting";

  // Roving tabindex: the logically focused tile owns real DOM focus.
  useEffect(() => {
    if (focused && ref.current && document.activeElement !== ref.current) {
      ref.current.focus({ preventScroll: true });
      // jsdom does not implement scrollIntoView.
      ref.current.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    }
  }, [focused]);

  const label = `${space.name}, ${STATUS_LABEL[space.status]}`;
  // In drop mode the tile is a clean drop target: hover controls are hidden.
  const pinnable = Boolean(onPin && space.sdk) && !dropMode;
  const shareable = Boolean(onShareApp && space.status !== "local") && !dropMode;
  const deletable =
    Boolean(onDelete && space.sdk && space.status !== "local") && !dropMode;
  // The core's power button: only for a Space that turns off and on.
  const power = onPower && space.power && !dropMode ? powerButton(space) : null;

  return (
    <span
      className="tile-wrap"
      data-drop-target={dropTarget ? "true" : undefined}
      onMouseEnter={preload}
    >
      <button
        ref={ref}
        type="button"
        role="option"
        // The grid no longer carries a selection: a tile click drills into that
        // Space's windows rather than making it "the current one", so nothing
        // here is ever selected. (Hover and keyboard focus are separate, and
        // both stay.)
        aria-selected={false}
        aria-label={label}
        title={label}
        tabIndex={focused ? 0 : -1}
        className="tile"
        data-status={space.status}
        data-space-id={space.id}
        onFocus={onFocus}
        onClick={onSelect}
      >
        <span className="tile-frame">
          <Thumbnail
            scene={space.scene}
            status={space.status}
            imageUrl={shot}
            statusText={space.progress?.label ?? STATUS_LABEL[space.status]}
            startedAt={space.startedAt}
            fraction={space.progress && !space.progress.error ? space.progress.permille / 1000 : undefined}
            deleting={deleting}
            local={space.provider === "local"}
          />
          {hotspot && (
            <span className="tile-hotspot-badge" title="Browsing through your network" aria-hidden="true">
              <SfIcon
                name="personalhotspot"
                size={11}
                className="tile-hotspot-badge-glyph"
                fallback={<HotspotGlyph className="tile-hotspot-badge-glyph" />}
              />
            </span>
          )}
          {dropTarget && dropGhost ? (
            <img className="tile-drop-ghost" src={dropGhost} alt="" aria-hidden="true" />
          ) : null}
        </span>
        <span className="tile-caption">
          <span className="tile-name">
            <OsIcon space={space} />
            {space.status === "local" ? <span className="tile-check" aria-hidden="true" /> : <StatusDot status={space.status} />}
            {space.name}
          </span>
        </span>
      </button>
      {deletable && !deleting && (
        <button
          type="button"
          className="tile-action tile-delete"
          data-owns-enter
          tabIndex={-1}
          aria-label={`Delete ${space.name}`}
          title="Delete this Space"
          onClick={(event) => {
            event.stopPropagation();
            onDelete?.();
          }}
        >
          <SfIcon name="trash" size={20} fallback={<TrashGlyph />} />
        </button>
      )}
      {power && !deleting && (
        <button
          type="button"
          className="tile-action tile-power"
          data-owns-enter
          data-busy={power.busy ? "true" : undefined}
          tabIndex={-1}
          aria-label={`${power.help} ${space.name}`}
          title={power.help}
          disabled={!power.enabled}
          onClick={(event) => {
            event.stopPropagation();
            onPower?.(power.turnOn);
          }}
        >
          {power.busy ? (
            <span className="dw-spinner" aria-hidden="true" />
          ) : (
            <SfIcon name={power.symbol} size={20} fallback={<PowerGlyph />} />
          )}
        </button>
      )}
      {shareable && (
        <button
          type="button"
          className="tile-action tile-share"
          data-owns-enter
          tabIndex={-1}
          aria-label={`Teleport an app to ${space.name}`}
          title="Teleport an app to this sandbox"
          onClick={(event) => {
            event.stopPropagation();
            onShareApp?.();
          }}
        >
          <SfIcon name="square.and.arrow.up" size={20} fallback={<ShareGlyph />} />
        </button>
      )}
      {pinnable && (
        <button
          type="button"
          className="tile-action tile-pin"
          data-owns-enter
          tabIndex={-1}
          aria-label={`Pin ${space.name} as picture-in-picture`}
          title="Pin as picture-in-picture"
          onClick={(event) => {
            event.stopPropagation();
            onPin?.();
          }}
        >
          <SfIcon name="pip.enter" size={20} fallback={<PinGlyph />} />
        </button>
      )}
    </span>
  );
}

function PowerGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
      <path
        d="M8 2.2v5.3M4.6 4.2a5 5 0 1 0 6.8 0"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
    </svg>
  );
}

function TrashGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
      <path
        d="M3 4.2h10M6.4 4.2V3.1a1 1 0 0 1 1-1h1.2a1 1 0 0 1 1 1v1.1M4.4 4.2l.5 8a1 1 0 0 0 1 .95h4.2a1 1 0 0 0 1-.95l.5-8"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ShareGlyph() {
  // A cloud with two sync arrows: "transfer this app's session to the cloud Space".
  return (
    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
      <path
        d="M4.4 12.2a2.9 2.9 0 0 1-.3-5.78 3.5 3.5 0 0 1 6.72-.6 2.6 2.6 0 0 1 .48 5.16z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
      <path d="M6 8.7a1.9 1.9 0 0 1 3.15-.85l.55.5" fill="none" stroke="currentColor" strokeWidth="1.15" strokeLinecap="round" />
      <path d="M10 6.9v1.6H8.4" fill="none" stroke="currentColor" strokeWidth="1.15" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M10 10.1a1.9 1.9 0 0 1-3.15.85l-.55-.5" fill="none" stroke="currentColor" strokeWidth="1.15" strokeLinecap="round" />
      <path d="M6 11.9v-1.6h1.6" fill="none" stroke="currentColor" strokeWidth="1.15" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function PinGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
      <rect x="1.5" y="2.5" width="13" height="10" rx="1.5" fill="none" stroke="currentColor" strokeWidth="1.3" />
      <rect x="8.5" y="7.5" width="4.5" height="3.5" rx="0.8" fill="currentColor" />
    </svg>
  );
}
