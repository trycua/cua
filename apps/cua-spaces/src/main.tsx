// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import { TeleportPicker } from "./components/TeleportPicker";
import { MainWindow } from "./components/desktop/MainWindow";
import { initCore } from "./core";
import { createBridge, hasTauri } from "./native/bridge";
import { createFleetBridge } from "./native/fleet";
import { declareExperiments } from "./state/experiments";
import { SpaceViewer } from "./viewer/SpaceViewer";
import { WindowStream } from "./viewer/WindowStream";
import "./styles/tokens.css";
import "./styles/app.css";
import "./styles/desktop.css";

/**
 * One bundle, two surfaces: the portal window renders the notch UI, while
 * `space-*` / `pip-*` windows (created by the shell) render the live desktop
 * viewer. The window label decides which; each viewer window asks the shell
 * for its configuration via `viewer_config`.
 */
async function pickSurface(): Promise<React.ReactElement> {
  if (hasTauri()) {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const label = getCurrentWindow().label;
    if (label.startsWith("space-") || label.startsWith("pip-")) {
      document.documentElement.classList.add("is-viewer");
      return <SpaceViewer fleet={createFleetBridge()} />;
    }
    if (label.startsWith("win-") || label.startsWith("winone-")) {
      // Per-window RCDP client: `win-*` hosts one panel per remote
      // window; `winone-*` is a single dedicated OS window for one remote window.
      document.documentElement.classList.add("is-viewer", "is-window-stream");
      return <WindowStream fleet={createFleetBridge()} />;
    }
    if (label === "main") {
      // The main window: an ordinary, opaque desktop window (sidebar, toolbar,
      // detail) with the traffic lights over its own title-bar area.
      document.documentElement.classList.add("is-main");
      // Which experiments are on (the day's `cua_app_active` carries them).
      declareExperiments();
      return <MainWindow />;
    }
    if (label.startsWith("teleport-picker")) {
      // The teleport picker and its consent screens: a standard, opaque,
      // decorated window.
      document.documentElement.classList.add("is-teleport-picker");
      return <TeleportPicker />;
    }
  } else {
    const params = new URLSearchParams(window.location.search);
    if (params.get("surface") === "main") {
      // Browser preview of the main window, on fixtures.
      document.documentElement.classList.add("is-main");
      return <MainWindow forceOnboarding={params.has("onboarding")} />;
    }
    // In a plain browser (design work), paint a stand-in wallpaper behind the
    // notch panel. Inside Tauri the portal page stays transparent around it.
    document.documentElement.classList.add("is-browser");
  }
  if (hasTauri()) {
    // Only the portal reaches this point; kick off a background auto-update
    // check so long-lived installs pick up new releases without user action.
    void import("./native/updater").then(({ checkForUpdateSilently }) =>
      checkForUpdateSilently(),
    );
  }
  return <App bridge={createBridge()} />;
}

// The app core (wasm) loads before anything renders: every surface's models
// call it synchronously.
void initCore()
  .then(pickSurface)
  .then((surface) => {
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>{surface}</React.StrictMode>,
  );
});
