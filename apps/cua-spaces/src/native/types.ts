// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Types shared with the Rust shell. Keep in sync with
 * `src-tauri/src/geometry.rs` (serde uses kebab-case for enums).
 */

export type WindowMode = "ambient" | "ambient-teleport" | "switcher" | "create-fleet";

/** How the portal should draw itself. */
export type DisplayStyle = "notched" | "no-notch";

/**
 * How the shell decided on the display style.
 *  - `override`  : explicit request from the renderer or CUA_SPACES_DISPLAY env var
 *  - `heuristic` : best-effort match on known notched MacBook panel sizes
 *  - `default`   : no signal; fell back to no-notch
 */
export type DisplayStyleSource = "override" | "heuristic" | "default";

export interface LogicalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PortalGeometry {
  mode: WindowMode;
  displayStyle: DisplayStyle;
  /** Window frame in logical points, relative to the desktop origin. */
  frame: LogicalRect;
  /** Monitor the frame was clamped to, in logical points. */
  monitor: LogicalRect;
  scaleFactor: number;
}

export interface PortalEnvironment {
  platform: "macos" | "windows" | "linux" | "other";
  displayStyle: DisplayStyle;
  displayStyleSource: DisplayStyleSource;
  /** True when the shell could apply accessory activation (macOS only). */
  accessoryActivation: boolean;
  /** True when running inside Tauri; false in a plain browser or tests. */
  native: boolean;
  geometry: PortalGeometry;
}
