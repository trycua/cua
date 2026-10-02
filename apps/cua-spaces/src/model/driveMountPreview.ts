// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run's Cua Volume page miniature, from the app core
 * (`drive_mount_preview`): a Space with three files beside a Finder window
 * whose sidebar gains the "Cua Volume" volume, each file flying over into
 * it. The SwiftUI app draws the same scene and frames through UniFFI, so
 * both shells play the same beats.
 */
import { core } from "../core";
import type { PreviewWindow } from "./driverPreview";
import type { PreviewPoint, PreviewRect } from "./presentationPreview";

export interface DriveMountPreview {
  width: number;
  height: number;
  loopMs: number;
  /** The Space (left). */
  space: PreviewWindow;
  /** The Finder window (right). */
  finder: PreviewWindow;
  /** The Finder window's sidebar (a shade darker than its content). */
  sidebar: PreviewRect;
  /** The sidebar's other places (placeholder bars). */
  places: PreviewRect[];
  /** The volume's row (the selection highlight). */
  volume: PreviewRect;
  /** The volume's drive glyph. */
  volumeIcon: PreviewRect;
  /** The volume's name, drawn from `volumeLabelX`, centred on the row. */
  volumeLabel: string;
  volumeLabelX: number;
  fontSize: number;
  /** The Space's files, top to bottom. */
  sourceIcons: PreviewRect[];
  sourceLabels: PreviewRect[];
  /** Where each file lands in the volume. */
  destIcons: PreviewRect[];
  destLabels: PreviewRect[];
}

export interface DriveMountFrame {
  /** The volume in the sidebar, 0 absent to 1 shown (and selected). */
  volume: number;
  /** The file in flight: its icon's top left (drawn above both windows). */
  flight: PreviewPoint | null;
  /** Each file in the volume, 0 absent to 1 shown. */
  arrived: number[];
}

let scene: DriveMountPreview | null = null;

/** The page's miniature (cached: it never changes). */
export function drivePreview(): DriveMountPreview {
  scene ??= core<DriveMountPreview>("onboarding.drivePreview");
  return scene;
}

/** The miniature `tMs` into its loop (it wraps). */
export function drivePreviewFrame(tMs: number): DriveMountFrame {
  return core("onboarding.drivePreviewFrame", { tMs: Math.max(0, Math.floor(tMs)) });
}

/** The Reduce Motion picture: the volume mounted, files arriving. */
export function drivePreviewStill(): DriveMountFrame {
  return core("onboarding.drivePreviewStill");
}
