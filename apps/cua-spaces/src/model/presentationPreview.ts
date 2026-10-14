// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The onboarding presentation cards' animated miniatures, from the app core
 * (`onboarding_preview`): each card's scene (the notch opening into the
 * Space tiles, or the menu bar icon's menu) and its frame at any moment of
 * the loop. The SwiftUI app draws the same scenes and frames through UniFFI,
 * so both shells play the same beats.
 */
import { core } from "../core";
import type { NotchTab } from "./notch";
import type { SpaceOs, SpaceStatus } from "./types";

export interface PreviewRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PreviewPoint {
  x: number;
  y: number;
}

export interface NotchRadii {
  top: number;
  bottom: number;
}

export interface PreviewTile {
  id: string;
  name: string;
  os: SpaceOs;
  status: SpaceStatus;
  dim: boolean;
  symbol: string;
  label: string;
}

export interface PreviewNotchView {
  tab: NotchTab;
  tiles: PreviewTile[];
  header: {
    placeholder: string;
    buttons: { id: string; symbol: string; label: string }[];
  } | null;
}

/** The notch miniature (stage points; `view` and `content*` real points). */
export interface NotchPreview {
  closed: PreviewRect;
  open: PreviewRect;
  closedRadii: NotchRadii;
  openRadii: NotchRadii;
  tab: PreviewRect;
  notch: PreviewRect;
  view: PreviewNotchView;
  contentWidth: number;
  contentHeight: number;
  notchHeight: number;
  side: number;
  scale: number;
}

export interface PreviewMenuItem {
  id: "status" | "separator" | "open" | "new-space" | "settings" | "quit";
  label: string;
  shortcut: string | null;
  enabled: boolean;
}

export interface MenuPreview {
  icon: PreviewRect;
  highlight: PreviewRect;
  clock: string;
  clockRight: number;
  menu: PreviewRect;
  radius: number;
  rows: { item: PreviewMenuItem; frame: PreviewRect }[];
  inset: number;
  fontSize: number;
}

export interface PresentationPreview {
  menuBar: boolean;
  width: number;
  height: number;
  menuBarHeight: number;
  loopMs: number;
  pointer: PreviewPoint[];
  notch: NotchPreview | null;
  menu: MenuPreview | null;
}

export interface PreviewFrame {
  pointer: PreviewPoint;
  pressed: boolean;
  hover: number;
  open: number;
  content: number;
  tab: number;
  highlighted: number | null;
  active: boolean;
}

/** The notch's shared motion (only what the miniature uses). */
export interface PreviewMotion {
  hoverScale: number;
  contentScale: number;
}

const scenes = new Map<boolean, PresentationPreview>();

/** A card's miniature (cached: it never changes). */
export function presentationPreview(menuBar: boolean): PresentationPreview {
  let s = scenes.get(menuBar);
  if (!s) {
    s = core<PresentationPreview>("onboarding.preview", { menuBar });
    scenes.set(menuBar, s);
  }
  return s;
}

/** The miniature `tMs` into its loop (it wraps). */
export function previewFrame(menuBar: boolean, tMs: number): PreviewFrame {
  return core("onboarding.previewFrame", { menuBar, tMs: Math.max(0, Math.floor(tMs)) });
}

/** The Reduce Motion picture: expanded, still. */
export function previewStill(menuBar: boolean): PreviewFrame {
  return core("onboarding.previewStill", { menuBar });
}

let motion: PreviewMotion | null = null;

export function notchMotion(): PreviewMotion {
  motion ??= core<PreviewMotion>("notch.motion");
  return motion;
}
