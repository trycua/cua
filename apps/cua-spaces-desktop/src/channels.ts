// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The renderer <-> main contract lives in the web bridge
// (apps/cua-spaces-web/src/bridge/electron-channels.ts: one request channel,
// one event channel, the native hosts' envelope); the shell imports it from
// there (tsdown bundles it), so both sides compile against one file.
// `pnpm check:channels` verifies the bundles carry it.
export { ELECTRON_BRIDGE_CHANNEL, ELECTRON_EVENT_CHANNEL } from "../../cua-spaces-web/src/bridge/electron-channels";

// Shell-internal: not part of the web contract.

export type ThemeSource = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

export interface ThemeState {
  source: ThemeSource;
  resolved: ResolvedTheme;
}

/** Synchronous channel the preload uses to read the platform at startup. */
export const PRELOAD_INIT_CHANNEL = "cua-desktop:init";

export interface PreloadInit {
  platform: NodeJS.Platform;
  /** The video bench reads the pages' video counters (video-bench.ts). */
  videoStats?: boolean;
}
