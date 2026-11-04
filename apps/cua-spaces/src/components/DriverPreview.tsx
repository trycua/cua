// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRef, type ReactNode } from "react";

import {
  driverPreview,
  driverPreviewFrame,
  driverPreviewStill,
  type DriverFrame,
  type PreviewWindow,
} from "../model/driverPreview";
import type { PreviewRect } from "../model/presentationPreview";
import { useLoopTime, useReducedMotion } from "./previewLoop";

/**
 * The AI agents page's "background computer-use" card miniature: behind, a
 * window where the cua-driver agent cursor (the driver's default theme: the
 * Cua blue arrow, white edge, soft glow, click rays) ticks checkboxes; in
 * front, a document where the user's own pointer drag-selects a line. The
 * scene and every frame are the app core's (`onboarding.driverPreview`,
 * `onboarding.driverPreviewFrame`); the SwiftUI app's `DriverPreview` draws
 * the same, so both play the same beats. This only draws them.
 *
 * Z-order: desktop, back window, agent cursor, front window, user pointer
 * (the agent works behind the window the user is in). It loops while on
 * screen in a visible, focused window; with reduced motion it is the
 * core's still.
 */
export function DriverPreview({ fixedMs }: { fixedMs?: number }) {
  const scene = driverPreview();
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const animate = fixedMs === undefined && !reduced;
  const t = useLoopTime(ref, animate, scene.loopMs);
  const frame: DriverFrame =
    fixedMs !== undefined ? driverPreviewFrame(fixedMs) : reduced ? driverPreviewStill() : driverPreviewFrame(t);
  const points = (ps: { x: number; y: number }[]) => ps.map((p) => `${p.x},${p.y}`).join(" ");
  const line = scene.lines[scene.selectedLine];
  return (
    <div
      ref={ref}
      className="obp-picture"
      data-preview="driver"
      data-still={reduced && fixedMs === undefined ? "true" : undefined}
      style={{ height: scene.height }}
      aria-hidden="true"
    >
      <div className="obp-stage" style={{ width: scene.width, height: scene.height }}>
        <MiniWindow window={scene.back} role="back">
          {scene.checkboxes.map((c, i) => {
            const on = frame.checked[i] ?? 0;
            return (
              <span
                key={`box-${i}`}
                className="obdp-checkbox"
                data-checked={on}
                style={{ ...at(c, scene.back.frame), ["--obdp-checked" as string]: on }}
              >
                <svg viewBox="0 0 8 8" width={c.width} height={c.height}>
                  <polyline
                    points="1.8,4.2 3.4,5.7 6.3,2.4"
                    pathLength={1}
                    strokeDasharray={1}
                    strokeDashoffset={1 - on}
                  />
                </svg>
              </span>
            );
          })}
          {scene.labels.map((l, i) => (
            <span key={`label-${i}`} className="obdp-bar" style={at(l, scene.back.frame)} />
          ))}
        </MiniWindow>
        <svg
          className="obdp-agent"
          width="16"
          height="16"
          style={{ transform: `translate(${frame.agent.x}px, ${frame.agent.y}px)` }}
        >
          {frame.ripple > 0 ? (
            <g
              className="obdp-rays"
              opacity={1 - frame.ripple}
              transform={`scale(${1 + 0.4 * frame.ripple})`}
              stroke={scene.agentFill}
            >
              {scene.agentRays.map((r, i) => (
                <line key={i} x1={r.from.x} y1={r.from.y} x2={r.to.x} y2={r.to.y} />
              ))}
            </g>
          ) : null}
          <g transform={`scale(${frame.agentPressed ? 0.85 : 1})`}>
            <polygon
              className="obdp-agent-arrow"
              points={points(scene.agentPointer)}
              fill={scene.agentFill}
              style={{ filter: `drop-shadow(0 0 1.5px ${scene.agentFill})` }}
            />
          </g>
        </svg>
        <MiniWindow window={scene.front} role="front">
          {line ? (
            <span
              className="obdp-selection"
              data-selection={frame.selection}
              style={{
                left: line.x - scene.front.frame.x,
                top: line.y - scene.front.frame.y - 2,
                width: line.width * frame.selection,
                height: line.height + 4,
              }}
            />
          ) : null}
          {scene.lines.map((l, i) => (
            <span key={`line-${i}`} className="obdp-bar" style={at(l, scene.front.frame)} />
          ))}
        </MiniWindow>
        <svg
          className="obp-pointer"
          width="12"
          height="16"
          style={{
            transform: `translate(${frame.pointer.x}px, ${frame.pointer.y}px) scale(${frame.pressed ? 0.85 : 1})`,
          }}
        >
          <polygon points={points(scene.pointer)} />
        </svg>
      </div>
    </div>
  );
}

/** `r` (stage points) relative to the window frame `f`. */
export function at(r: PreviewRect, f: PreviewRect) {
  return { left: r.x - f.x, top: r.y - f.y, width: r.width, height: r.height };
}

/** A miniature window: its frame, a title bar with three buttons, `children`. */
export function MiniWindow({ window: w, role, children }: { window: PreviewWindow; role: string; children: ReactNode }) {
  const f = w.frame;
  return (
    <div
      className="obdp-window"
      data-window={role}
      style={{ left: f.x, top: f.y, width: f.width, height: f.height, borderRadius: w.radius }}
    >
      <span className="obdp-titlebar" style={{ height: w.titleBar }}>
        <i />
        <i />
        <i />
      </span>
      {children}
    </div>
  );
}
