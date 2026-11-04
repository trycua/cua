// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { BridgeMode } from "./adapter";

/** Tauri 2's globals: `__TAURI__` with `app.withGlobalTauri`, and
 * `__TAURI_INTERNALS__` always. */
export interface TauriGlobals {
  __TAURI__?: {
    core?: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
    event?: {
      listen: (event: string, handler: (e: { payload: unknown }) => void) => Promise<() => void>;
    };
  };
  __TAURI_INTERNALS__?: {
    invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  };
  /** UI settings the Tauri shell injects before the page runs. */
  __CUA_UI_STORAGE__?: Record<string, string>;
}

/** What the Electron preload exposes (`contextBridge.exposeInMainWorld`). */
export interface CuaDesktopBridge {
  invoke(channel: string, args?: unknown): Promise<unknown>;
  /** Subscribes to a pushed event channel; returns the unsubscribe. */
  on?(channel: string, listener: (payload: unknown) => void): () => void;
  platform?: string;
  /** The video bench reads the page's video counters (`CUA_SPACES_VIDEO_STATS`). */
  videoStats?: boolean;
}

/** WKScriptMessageHandler (or ...WithReply, whose postMessage returns a Promise). */
export interface WebkitMessageHandler {
  postMessage(message: unknown): unknown;
}

export interface HostWindow extends TauriGlobals {
  cuaDesktop?: CuaDesktopBridge;
  webkit?: { messageHandlers?: { cua?: WebkitMessageHandler } & Record<string, unknown> };
  location?: { search: string };
  /** The SwiftUI host's `cua:event` window events. */
  addEventListener?(type: string, listener: (e: Event) => void): void;
  removeEventListener?(type: string, listener: (e: Event) => void): void;
  matchMedia?(query: string): { matches: boolean };
  open?(url: string, target?: string, features?: string): unknown;
}

/**
 * Which host the page runs in. Order: Tauri, then the Electron preload,
 * then the SwiftUI WKWebView, then demo. `?bridge=demo` forces demo (design
 * work inside a real host).
 */
export function detectMode(win: HostWindow | undefined = globalThis.window as HostWindow | undefined): BridgeMode {
  if (!win) return "demo";
  const forced = readForcedMode(win);
  if (forced) return forced;
  if (win.__TAURI__ || win.__TAURI_INTERNALS__) return "tauri";
  if (win.cuaDesktop && typeof win.cuaDesktop.invoke === "function") return "electron";
  if (win.webkit?.messageHandlers?.cua && typeof win.webkit.messageHandlers.cua.postMessage === "function") {
    return "webkit";
  }
  return "demo";
}

function readForcedMode(win: HostWindow): BridgeMode | null {
  try {
    const v = new URLSearchParams(win.location?.search ?? "").get("bridge");
    return v === "demo" ? "demo" : null;
  } catch {
    return null;
  }
}
