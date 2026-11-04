// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Sandboxed preload: the only bridge between the web UI and the main process.
// It exposes `window.cuaDesktop` as the web bridge's Electron transport
// expects (apps/cua-spaces-web/src/bridge/electron-channels.ts): `invoke` on
// the one bridge channel resolves to the main process's envelope unchanged,
// and `on` subscribes to the one event channel. The renderer gets no Node or
// Electron APIs.
import { contextBridge, ipcRenderer, webUtils, type IpcRendererEvent } from "electron";
import { ELECTRON_BRIDGE_CHANNEL, ELECTRON_EVENT_CHANNEL, PRELOAD_INIT_CHANNEL, type PreloadInit } from "./channels";
import { withDroppedPaths } from "./drop";
import { KEYBOARD_CHANNEL, viewerHasKeyboard } from "./keyboard";
import { DIM_ATTRIBUTE, DIM_CHANNEL } from "./overlay";

const init = ipcRenderer.sendSync(PRELOAD_INIT_CHANNEL) as PreloadInit;

// Files dropped on the page: the page sees only their names, so the last
// drop's paths are kept here (read before the page's own handler runs) and
// go to `spaces.droppedFiles` in place of anything the page put there. The
// page can never name a path the person did not drop (bridge/files.ts).
let droppedPaths: string[] = [];
window.addEventListener(
  "drop",
  (e) => {
    const files = Array.from(e.dataTransfer?.files ?? []);
    droppedPaths = files.map((f) => webUtils.getPathForFile(f)).filter((p) => p !== "");
  },
  true,
);

function invoke(channel: string, request?: unknown): Promise<unknown> {
  if (channel !== ELECTRON_BRIDGE_CHANNEL) {
    return Promise.resolve({ ok: false, error: { message: `unknown channel: ${channel}`, code: "forbidden" } });
  }
  return ipcRenderer.invoke(channel, withDroppedPaths(request, droppedPaths));
}

function on(channel: string, listener: (payload: unknown) => void): () => void {
  if (channel !== ELECTRON_EVENT_CHANNEL) throw new Error(`unknown event channel: ${channel}`);
  const f = (_e: IpcRendererEvent, payload: unknown) => listener(payload);
  ipcRenderer.on(channel, f);
  return () => ipcRenderer.removeListener(channel, f);
}

contextBridge.exposeInMainWorld("cuaDesktop", init.videoStats ? { invoke, on, platform: init.platform, videoStats: true } : { invoke, on, platform: init.platform });

// A Space's live desktop with the keyboard takes the menu shortcuts too
// (keyboard.ts): tell the main process when its canvas gains or loses focus.
{
  let has = false;
  const sync = () => {
    const now = viewerHasKeyboard(document.activeElement);
    if (now === has) return;
    has = now;
    ipcRenderer.send(KEYBOARD_CHANNEL, now);
  };
  window.addEventListener("focusin", sync, true);
  // During focusout the old element may still be active: read once it settled.
  window.addEventListener("focusout", () => setTimeout(sync, 0), true);
}

// Windows and Linux draw the window controls in a title bar overlay above the
// page, which a dialog's backdrop can't cover. Tell the main process while a
// backdrop (marked `data-window-dim` by the web UI) is on the page, so it
// dims the overlay to match.
if (init.platform !== "darwin") {
  let dimmed = false;
  const sync = () => {
    const now = document.querySelector(`[${DIM_ATTRIBUTE}]`) !== null;
    if (now === dimmed) return;
    dimmed = now;
    ipcRenderer.send(DIM_CHANNEL, now);
  };
  const watch = () => {
    new MutationObserver(sync).observe(document.documentElement, { childList: true, subtree: true });
    sync();
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", watch, { once: true });
  else watch();
}
