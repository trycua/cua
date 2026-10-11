// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The bridge on Electron's IPC: the preload's one request channel
// (`ELECTRON_BRIDGE_CHANNEL`) into the method table, and its events out to
// every window on `ELECTRON_EVENT_CHANNEL`. Only the app's own pages, in
// their main frame, may call in (the SwiftUI host's origin check).
import { BrowserWindow, ipcMain, type IpcMainInvokeEvent } from "electron";
import { ELECTRON_BRIDGE_CHANNEL, ELECTRON_EVENT_CHANNEL, PRELOAD_INIT_CHANNEL, type PreloadInit } from "../channels";
import { APP_ORIGIN } from "../protocol";
import { Failure, reply, type BridgeEvent, type BridgeEvents, type BridgeRegistry } from "./host";

const trusted = (event: IpcMainInvokeEvent) =>
  event.senderFrame !== null && event.senderFrame.parent === null && event.senderFrame.url.startsWith(`${APP_ORIGIN}/`);

const requestId = (request: unknown) => {
  const id = (request as { id?: unknown } | null)?.id;
  return typeof id === "string" ? id : id === undefined ? "" : String(id);
};

export interface BridgeIpc {
  /** The method table; null when this run has no native host (the e2e demo switch). */
  registry: BridgeRegistry | null;
  events: BridgeEvents | null;
  /** The platform the page is told (the e2e switch can play another). */
  platform: NodeJS.Platform;
  /** The pages count their video for the bench. */
  videoStats?: boolean;
}

export function installBridgeIpc({ registry, events, platform, videoStats }: BridgeIpc): void {
  ipcMain.on(PRELOAD_INIT_CHANNEL, (event) => {
    const init: PreloadInit = videoStats ? { platform, videoStats } : { platform };
    event.returnValue = init;
  });

  ipcMain.handle(ELECTRON_BRIDGE_CHANNEL, async (event, request: unknown) => {
    const id = requestId(request);
    if (!trusted(event)) return reply(id, new Failure("forbidden", "origin not allowed"));
    if (!registry) return reply(id, Failure.unsupported("this run plays the page's demo host (CUA_SPACES_E2E_DEMO)"));
    return registry.dispatch(request, { window: BrowserWindow.fromWebContents(event.sender) });
  });

  events?.subscribe((e: BridgeEvent) => {
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) win.webContents.send(ELECTRON_EVENT_CHANNEL, e);
    }
  });
}
