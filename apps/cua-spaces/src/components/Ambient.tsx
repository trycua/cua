// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ambientDots, countActive } from "../model/mru";
import { notchActivity, notchTab } from "../model/notch";
import type { Space } from "../model/types";
import type { DisplayStyle } from "../native/types";
import { HotspotGlyph, RemoteAccessGlyph } from "./HotspotGlyph";
import { RadialProgress } from "./RadialProgress";

interface AmbientProps {
  spaces: readonly Space[];
  displayStyle: DisplayStyle;
  onExpand: () => void;
  /** True while a supported window-drag is in its "prompt" phase: the ambient
   * tab collapses into the notch and the notch morphs into the compact
   * "Teleport to Cua" hint. Driven by the drag state machine in `App`. */
  teleportOpen?: boolean;
  /** Menu-bar presentation: the "N Spaces" switcher tab/capsule is hidden
   * (a macOS menu-bar status item is the entry point instead), but the
   * "Teleport to Cua" box still hangs under the notch during a drag. */
  menuBar?: boolean;
  /** True while this Mac is sharing its network to a Space (the hotspot). Shows
   * a broadcast glyph on the left of the notch, mirroring macOS Personal
   * Hotspot. Indicator only — stop sharing from the switcher footer / viewer. */
  hotspotActive?: boolean;
  /** A file/session transfer in flight — shows a progress pie in the left-of-
   * notch slot (determinate when byte totals are known). */
  transfer?: { active: boolean; sent?: number; total?: number };
}

/**
 * Ambient portal. Notched: a black tab hugging the notch that, during a drag
 * teleport, collapses into the notch while the notch itself grows into a
 * "Teleport to Cua" box. No-notch: a single centred menu-bar capsule (with the
 * same morphing box as a fallback).
 */
export function Ambient({
  spaces,
  displayStyle,
  onExpand,
  teleportOpen = false,
  menuBar = false,
  hotspotActive = false,
  transfer,
}: AmbientProps) {
  const dots = ambientDots(spaces);
  const active = countActive(spaces);
  // The tab's rows and the left-of-notch indicator are the core's
  // (`notch.tab`, `notch.activity`), so the SwiftUI notch shows the same.
  const tab = notchTab(spaces);
  const label = `${tab.count} ${tab.word}`;
  const summary = `${label}, ${active} active. Open Cua Spaces switcher.`;

  // The left-of-notch spot: a transfer in flight (brief, time-sensitive),
  // else the hotspot while sharing, else a startup ring while a Space is
  // coming up; nothing when idle. It scales with the notch on hover.
  const activity = notchActivity(spaces, hotspotActive, transfer);
  const leftSlot = activity ? (
    <div className="ambient-hotspot" role="status" aria-label={activity.label} title={activity.label} data-activity={activity.kind}>
      {activity.kind === "remote-access" ? (
        <RemoteAccessGlyph className="ambient-hotspot-glyph" />
      ) : activity.symbol ? (
        <HotspotGlyph className="ambient-hotspot-glyph" />
      ) : (
        <RadialProgress
          fraction={activity.permille == null ? undefined : activity.permille / 1000}
          startedAt={activity.startedAt ?? undefined}
          estimateMs={activity.estimateMs}
          className="ambient-progress-pie"
        />
      )}
    </div>
  ) : null;

  // The morphing teleport hint. It shares the notch's shape and colour so the
  // notch appears to grow slightly taller; a single compact line of white text
  // slides in as it opens — deliberately unintrusive (no icon/thumbnail).
  const teleportBox = (
    <div
      className="ambient-teleport-box"
      role="status"
      aria-hidden={!teleportOpen}
      aria-label="Teleport to Cua"
    >
      <span className="ambient-teleport-label">Teleport to Cua</span>
    </div>
  );

  if (displayStyle === "no-notch") {
    const dotsNode = (
      <span className="ambient-dots" data-pulse={dots.agentActive ? "on" : "off"}>
        <span className={`status-dot status-running ${dots.running ? "" : "is-off"}`} />
        <span className={`status-dot status-approval ${dots.approval ? "" : "is-off"}`} />
        <span className={`status-dot status-suspended ${dots.suspended ? "" : "is-off"}`} />
      </span>
    );
    return (
      <div
        className="ambient-wrap ambient-wrap-plain"
        data-teleport={teleportOpen ? "on" : "off"}
        data-menu-bar={menuBar ? "on" : "off"}
      >
        {leftSlot}
        {!menuBar && (
          <button type="button" className="ambient ambient-capsule" onClick={onExpand} aria-label={summary}>
            {dotsNode}
            <span className="ambient-label">{label}</span>
          </button>
        )}
        {teleportBox}
      </div>
    );
  }

  // Notched: a black tab the height of the notch, flush against the right edge
  // of the notch cap so the two read as one continuous shape. Two small rows:
  // the count over "Space(s)". During a drag teleport the tab retracts into the
  // notch (see `[data-teleport="on"] .ambient-tab`) and the box grows below it.
  return (
    <div
      className="ambient-notched"
      data-teleport={teleportOpen ? "on" : "off"}
      data-menu-bar={menuBar ? "on" : "off"}
    >
      {leftSlot}
      {!menuBar && (
        <button type="button" className="ambient ambient-tab" onClick={onExpand} aria-label={summary} title={label}>
          <span className="ambient-tab-count">{tab.count}</span>
          <span className="ambient-tab-word">{tab.word}</span>
        </button>
      )}
      {teleportBox}
    </div>
  );
}
