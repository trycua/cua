// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Electron shell's IPC (apps/cua-spaces-desktop). The shell is a native
 * host like the SwiftUI app: its main process answers the same methods
 * (`webkit-protocol.ts`) from the same Rust library, so the page talks to it
 * through the webkit adapter over this transport (`transport.ts`).
 *
 * The preload exposes `window.cuaDesktop`:
 *
 *   contextBridge.exposeInMainWorld("cuaDesktop", {
 *     invoke: (channel, request) => ipcRenderer.invoke(channel, request),
 *     on: (channel, listener) => { ... return unsubscribe },
 *     platform: process.platform,
 *   });
 *
 * `invoke(ELECTRON_BRIDGE_CHANNEL, { id, method, args })` resolves to the
 * host's envelope (`WebkitResponse`, never a throw: Electron rewrites thrown
 * messages). Pushed events arrive on `ELECTRON_EVENT_CHANNEL` as
 * `{ event, payload }` (`WebkitEventDetail`), the same as the SwiftUI
 * host's window `cua:event`. The preload refuses every other channel.
 */

/** The one request channel. */
export const ELECTRON_BRIDGE_CHANNEL = "cua:bridge";

/** The one event channel. */
export const ELECTRON_EVENT_CHANNEL = "cua:event";
