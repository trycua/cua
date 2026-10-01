// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRef } from "react";

import {
  drivePreview,
  drivePreviewFrame,
  drivePreviewStill,
  type DriveMountFrame,
} from "../model/driveMountPreview";
import type { PreviewRect } from "../model/presentationPreview";
import { at, MiniWindow } from "./DriverPreview";
import { useLoopTime, useReducedMotion } from "./previewLoop";

/**
 * The first run's Cua Volume page miniature: a Space with three files on the
 * left, a Finder window on the right whose sidebar gains the "Cua Volume"
 * volume (fading in, selected), then each file flies over and lands in it.
 * The scene and every frame are the app core's (`onboarding.drivePreview`,
 * `onboarding.drivePreviewFrame`); the SwiftUI app draws the same. This
 * only draws them, the same way as `DriverPreview`.
 *
 * Z-order: desktop, the Space, the Finder window, the file in flight. It
 * loops while on screen in a visible, focused window; with reduced motion
 * it is the core's still.
 */
export function DriveMountPreview({ fixedMs }: { fixedMs?: number }) {
  const scene = drivePreview();
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const animate = fixedMs === undefined && !reduced;
  const t = useLoopTime(ref, animate, scene.loopMs);
  const frame: DriveMountFrame =
    fixedMs !== undefined ? drivePreviewFrame(fixedMs) : reduced ? drivePreviewStill() : drivePreviewFrame(t);
  const finder = scene.finder.frame;
  const icon = scene.sourceIcons[0];
  return (
    <div
      ref={ref}
      className="obp-picture"
      data-preview="drive"
      data-still={reduced && fixedMs === undefined ? "true" : undefined}
      style={{ height: scene.height }}
      aria-hidden="true"
    >
      <div className="obp-stage" style={{ width: scene.width, height: scene.height }}>
        <MiniWindow window={scene.space} role="space">
          {scene.sourceIcons.map((r, i) => (
            <FileIcon key={`src-${i}`} rect={r} frame={scene.space.frame} />
          ))}
          {scene.sourceLabels.map((l, i) => (
            <span key={`src-label-${i}`} className="obdp-bar" style={at(l, scene.space.frame)} />
          ))}
        </MiniWindow>
        <MiniWindow window={scene.finder} role="finder">
          <span className="obdmp-sidebar" style={{ ...at(scene.sidebar, finder), top: scene.finder.titleBar, height: scene.sidebar.height - scene.finder.titleBar }} />
          {scene.places.map((p, i) => (
            <span key={`place-${i}`} className="obdp-bar" style={at(p, finder)} />
          ))}
          <span
            className="obdmp-volume"
            data-volume={frame.volume}
            style={{ ...at(scene.volume, finder), opacity: frame.volume }}
          />
          <span
            className="obdmp-volume-icon"
            style={{ ...at(scene.volumeIcon, finder), opacity: frame.volume }}
          />
          <span
            className="obdmp-volume-label"
            style={{
              left: scene.volumeLabelX - finder.x,
              top: scene.volume.y - finder.y,
              height: scene.volume.height,
              lineHeight: `${scene.volume.height}px`,
              fontSize: scene.fontSize,
              opacity: frame.volume,
            }}
          >
            {scene.volumeLabel}
          </span>
          {scene.destIcons.map((r, i) => (
            <FileIcon key={`dst-${i}`} rect={r} frame={finder} opacity={frame.arrived[i] ?? 0} arrived />
          ))}
          {scene.destLabels.map((l, i) => (
            <span
              key={`dst-label-${i}`}
              className="obdp-bar"
              data-arrived={frame.arrived[i] ?? 0}
              style={{ ...at(l, finder), opacity: frame.arrived[i] ?? 0 }}
            />
          ))}
        </MiniWindow>
        {frame.flight && icon ? (
          <FileIcon
            rect={{ x: frame.flight.x, y: frame.flight.y, width: icon.width, height: icon.height }}
            frame={{ x: 0, y: 0, width: scene.width, height: scene.height }}
            flight
          />
        ) : null}
      </div>
    </div>
  );
}

/** A document glyph: a page with a folded corner. */
function FileIcon({
  rect,
  frame,
  opacity,
  arrived,
  flight,
}: {
  rect: PreviewRect;
  frame: PreviewRect;
  opacity?: number;
  arrived?: boolean;
  flight?: boolean;
}) {
  const { width: w, height: h } = rect;
  const fold = Math.min(w, h) * 0.4;
  return (
    <svg
      className={flight ? "obdmp-file obdmp-flight" : "obdmp-file"}
      data-arrived={arrived ? opacity : undefined}
      width={w}
      height={h}
      viewBox={`0 0 ${w} ${h}`}
      style={{ ...at(rect, frame), opacity }}
    >
      <path d={`M0.5 0.5 H${w - fold} L${w - 0.5} ${fold} V${h - 0.5} H0.5 Z`} />
    </svg>
  );
}
