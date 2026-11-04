// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The AI agents page's "background computer-use" card miniature, from the
 * app core (`driver_preview`): a back window where the cua-driver agent
 * cursor ticks checkboxes while the user drag-selects text in a front
 * window. The SwiftUI app draws the same scene and frames through UniFFI,
 * so both shells play the same beats.
 */
import { core } from "../core";
import type { PreviewPoint, PreviewRect } from "./presentationPreview";

export interface PreviewSegment {
  from: PreviewPoint;
  to: PreviewPoint;
}

export interface PreviewWindow {
  frame: PreviewRect;
  titleBar: number;
  radius: number;
}

export interface DriverPreview {
  width: number;
  height: number;
  loopMs: number;
  /** The window the agent works in (behind). */
  back: PreviewWindow;
  /** The window the user works in (in front). */
  front: PreviewWindow;
  checkboxes: PreviewRect[];
  labels: PreviewRect[];
  lines: PreviewRect[];
  /** An index into `lines`. */
  selectedLine: number;
  /** The user's pointer outline, tip at the origin. */
  pointer: PreviewPoint[];
  /** The agent cursor's outline, tip at the origin. */
  agentPointer: PreviewPoint[];
  agentFill: string;
  /** The click rays, relative to the agent cursor's tip. */
  agentRays: PreviewSegment[];
}

export interface DriverFrame {
  pointer: PreviewPoint;
  pressed: boolean;
  /** 0 to 1 of the selected line's width, from its left. */
  selection: number;
  agent: PreviewPoint;
  agentPressed: boolean;
  /** 0 hidden, else 0 to 1 through the click rays' burst. */
  ripple: number;
  /** Each checkbox's tick, 0 to 1. */
  checked: number[];
}

let scene: DriverPreview | null = null;

/** The card's miniature (cached: it never changes). */
export function driverPreview(): DriverPreview {
  scene ??= core<DriverPreview>("onboarding.driverPreview");
  return scene;
}

/** The miniature `tMs` into its loop (it wraps). */
export function driverPreviewFrame(tMs: number): DriverFrame {
  return core("onboarding.driverPreviewFrame", { tMs: Math.max(0, Math.floor(tMs)) });
}

/** The Reduce Motion picture: the agent mid-task, still. */
export function driverPreviewStill(): DriverFrame {
  return core("onboarding.driverPreviewStill");
}
