// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { DisplayStyle, PortalEnvironment, PortalGeometry, WindowMode } from "./types";

/**
 * Thin, typed wrapper over the Rust commands. When the renderer is not
 * hosted by Tauri (Vite in a browser, Vitest) every call resolves against a
 * local stand-in so the UI stays fully usable for design work and tests.
 */
export interface NativeBridge {
  readonly isNative: boolean;
  getEnvironment(): Promise<PortalEnvironment>;
  setWindowMode(mode: WindowMode): Promise<PortalGeometry>;
  setDisplayStyle(style: DisplayStyle): Promise<PortalEnvironment>;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export function hasTauri(): boolean {
  return typeof window !== "undefined" && typeof window.__TAURI_INTERNALS__ !== "undefined";
}

/** Logical sizes mirrored from `geometry.rs` for the non-native fallback. */
export const FALLBACK_SIZES: Record<DisplayStyle, Record<WindowMode, { width: number; height: number }>> = {
  notched: {
    ambient: { width: 420, height: 38 },
    "ambient-teleport": { width: 420, height: 96 },
    switcher: { width: 760, height: 320 },
    "create-fleet": { width: 600, height: 700 },
  },
  "no-notch": {
    ambient: { width: 180, height: 34 },
    "ambient-teleport": { width: 240, height: 96 },
    switcher: { width: 760, height: 300 },
    "create-fleet": { width: 600, height: 680 },
  },
};

function fallbackGeometry(mode: WindowMode, displayStyle: DisplayStyle): PortalGeometry {
  const monitor = { x: 0, y: 0, width: 1512, height: 982 };
  const size = FALLBACK_SIZES[displayStyle][mode];
  return {
    mode,
    displayStyle,
    frame: {
      x: Math.round((monitor.width - size.width) / 2),
      y: 0,
      width: size.width,
      height: size.height,
    },
    monitor,
    scaleFactor: 2,
  };
}

function readDisplayStyleFromUrl(): DisplayStyle | undefined {
  if (typeof window === "undefined") return undefined;
  const value = new URLSearchParams(window.location.search).get("display");
  return value === "notched" || value === "no-notch" ? value : undefined;
}

export function createFallbackBridge(initialStyle?: DisplayStyle): NativeBridge {
  let displayStyle: DisplayStyle = initialStyle ?? readDisplayStyleFromUrl() ?? "no-notch";
  let mode: WindowMode = "ambient";

  const env = (): PortalEnvironment => ({
    platform: "other",
    displayStyle,
    displayStyleSource: "default",
    accessoryActivation: false,
    native: false,
    geometry: fallbackGeometry(mode, displayStyle),
  });

  return {
    isNative: false,
    getEnvironment: async () => env(),
    setWindowMode: async (next) => {
      mode = next;
      return fallbackGeometry(mode, displayStyle);
    },
    setDisplayStyle: async (style) => {
      displayStyle = style;
      return env();
    },
  };
}

export function createTauriBridge(): NativeBridge {
  // Imported lazily so the browser/test bundle never touches Tauri globals.
  const core = import("@tauri-apps/api/core");
  return {
    isNative: true,
    getEnvironment: async () => (await core).invoke<PortalEnvironment>("get_environment"),
    setWindowMode: async (mode) => (await core).invoke<PortalGeometry>("set_window_mode", { mode }),
    setDisplayStyle: async (style) => (await core).invoke<PortalEnvironment>("set_display_style", { style }),
  };
}

export function createBridge(): NativeBridge {
  return hasTauri() ? createTauriBridge() : createFallbackBridge();
}
