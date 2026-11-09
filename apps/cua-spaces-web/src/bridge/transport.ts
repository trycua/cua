// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * How the page reaches a native host that answers `webkit-protocol.ts`'s
 * methods: the SwiftUI app's WKWebView (`window.webkit.messageHandlers.cua`)
 * or the Electron shell's preload (`window.cuaDesktop`, one channel). Both
 * carry the same envelope and push the same `cua:event`s, so one adapter
 * (`adapters/webkit.ts`) serves both.
 */

import type { HostWindow } from "./detect";
import { ELECTRON_BRIDGE_CHANNEL, ELECTRON_EVENT_CHANNEL } from "./electron-channels";
import { WEBKIT_EVENT, type WebkitEventDetail, type WebkitRequest } from "./webkit-protocol";

export interface HostTransport {
  /** Sends a request; resolves with the host's envelope (`WebkitResponse`). */
  post(request: WebkitRequest): unknown;
  /** Delivers the host's `cua:event`s; returns the stop. */
  listen(handler: (detail: WebkitEventDetail) => void): () => void;
}

/** The SwiftUI app: `postMessage` (with a reply) and window `cua:event`s. */
export function webkitTransport(win: HostWindow): HostTransport | null {
  const handler = win.webkit?.messageHandlers?.cua;
  if (!handler) return null;
  return {
    post: (request) => handler.postMessage(request),
    listen(handle) {
      const onEvent = (e: Event) => {
        const detail = (e as CustomEvent<WebkitEventDetail>).detail;
        if (detail) handle(detail);
      };
      win.addEventListener?.(WEBKIT_EVENT, onEvent);
      return () => win.removeEventListener?.(WEBKIT_EVENT, onEvent);
    },
  };
}

/** The Electron shell: `invoke` on the bridge channel, events on `cua:event`. */
export function electronTransport(win: HostWindow): HostTransport | null {
  const desktop = win.cuaDesktop;
  if (!desktop || typeof desktop.invoke !== "function") return null;
  return {
    post: (request) => desktop.invoke(ELECTRON_BRIDGE_CHANNEL, request),
    listen(handle) {
      if (!desktop.on) return () => {};
      return desktop.on(ELECTRON_EVENT_CHANNEL, (payload) => {
        if (payload && typeof payload === "object" && typeof (payload as WebkitEventDetail).event === "string") {
          handle(payload as WebkitEventDetail);
        }
      });
    },
  };
}
