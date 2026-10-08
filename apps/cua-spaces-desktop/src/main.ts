// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { execFileSync } from "node:child_process";
import { homedir } from "node:os";
import * as path from "node:path";
import { app, BrowserWindow, dialog, nativeTheme, Notification, screen, shell } from "electron";
import { createBridge } from "./bridge";
import { applyVideoDecodeSwitches } from "./gpu";
import { menuItems, realSpaces, startHost } from "./bridge/host-start";
import { installBridgeIpc } from "./bridge/ipc";
import { installMenu } from "./menu";
import { appCoreStatePaths, migrateFromSwiftApp, SWIFT_BUNDLE_ID } from "./migrate-swift";
import { openedAtLogin } from "./login-item";
import { makeModel, readOnboardingCompleted } from "./model/environment";
import { recordLaunch, refreshAfterUpdate } from "./model/update-refresh";
import { loadNative } from "./native/load";
import { nativeDir } from "./native/location";
import { connectNotch, type NotchCore } from "./notch";
import { dropOnSpace, feedNotch, notchThumbnail, openSpaceFromNotch, type NotchFeedUi } from "./notch-feed";
import { createPipWindows } from "./pip";
import { electronKeyvaultPrompts } from "./prompts";
import { APP_ORIGIN, handleAppProtocol, registerSchemePrivileges } from "./protocol";
import { readSettings, writeSettings } from "./settings";
import { makeSystem } from "./system";
import { initTheme, setThemeSource } from "./theme";
import { closesToTray, installTray, isQuitting } from "./tray";
import { countsVideo, startVideoBench, videoBenchEnv } from "./video-bench";
import { applyThemeToWindow, createMainWindow, handleOverlayDim, handleViewerKeyboard, openSpaceWindow, setWindowBackground } from "./window";

// Must match `appId` (Windows; macOS has the Swift app's bundle id) and the
// Linux desktop entry in electron-builder.config.cjs.
const APP_USER_MODEL_ID = "ai.cua.spaces.desktop";
const LINUX_ID = "cua-spaces";

/**
 * The parity flows' test switch (apps/cua-spaces-web/e2e/host.ts): no
 * native library, no daemon; the page plays the browser demo host
 * (`?bridge=demo`). A normal launch never shows sample data.
 */
const E2E_DEMO = process.env.CUA_SPACES_E2E_DEMO === "1";

app.setName("Cua Spaces");
// Windows groups taskbar buttons and toasts by this id; the NSIS shortcuts carry the same one.
if (process.platform === "win32") app.setAppUserModelId(APP_USER_MODEL_ID);
// Linux: WM_CLASS (X11) and the Wayland app id must match the desktop entry's
// StartupWMClass and file name, or docks show a generic icon. Set before ready.
if (process.platform === "linux") {
  app.commandLine.appendSwitch("class", LINUX_ID);
  app.setDesktopName(`${LINUX_ID}.desktop`);
}
if (process.env.CUA_SPACES_USER_DATA) app.setPath("userData", process.env.CUA_SPACES_USER_DATA);
// The page decodes Spaces' video with WebCodecs: on the GPU where it can (gpu.ts).
applyVideoDecodeSwitches(app.commandLine, process.platform, process.env);
const VIDEO_BENCH = videoBenchEnv(process.env);

/** macOS, packaged: take over the Swift app's settings once (migrate-swift.ts). */
function migrateSwiftApp() {
  if (process.platform !== "darwin" || !app.isPackaged) return;
  try {
    let defaultsXml: string | null = null;
    try {
      defaultsXml = execFileSync("/usr/bin/defaults", ["export", SWIFT_BUNDLE_ID, "-"], { encoding: "utf8", timeout: 5000 });
    } catch {
      // An empty domain: nothing to read.
    }
    const patch = migrateFromSwiftApp({
      home: process.env.HOME || homedir(),
      userData: app.getPath("userData"),
      settings: readSettings(),
      defaultsXml,
      primaryHeight: screen.getPrimaryDisplay().bounds.height,
      now: new Date(),
    });
    if (patch) writeSettings(patch);
  } catch (error) {
    console.warn("[cua-spaces] could not take over the Swift app's settings:", error);
  }
}

/** The native host: the library, the model on it and the bridge. */
async function startNativeHost(showRoute: (route: string) => void) {
  const dir = nativeDir({
    platform: process.platform,
    arch: process.arch,
    packaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    appRoot: path.join(__dirname, ".."),
    override: process.env.CUA_SPACES_NATIVE_DIR,
  });
  const native = await loadNative(dir);
  const openUrl = (url: string) => {
    if (/^https:\/\//.test(url)) void shell.openExternal(url);
  };
  // Before the model loads the settings: a launch after an update refreshes the coding agents' skills.
  const core = appCoreStatePaths(app.getPath("userData"));
  const refresh = recordLaunch(native, core.settings, app.getVersion(), "", readOnboardingCompleted(core.onboarding) ?? false);
  const env = makeModel({ native, nativeDir: dir, userData: app.getPath("userData"), version: app.getVersion(), openUrl });
  const pip = createPipWindows();
  app.on("will-quit", () => pip.closeAll());
  const prompts = await electronKeyvaultPrompts(native);
  const bridge = createBridge({
    model: env.model,
    supervisor: env.supervisor,
    version: app.getVersion(),
    platform: process.platform,
    env: process.env,
    system: makeSystem({ native, showRoute }),
    ui: {
      openSpace: (spaceId, name) => void openSpaceWindow(spaceId, name),
      setBackground: (win, color, appearance) => {
        if (appearance) setThemeSource(appearance);
        if (win instanceof BrowserWindow) setWindowBackground(win, color);
      },
      activate: () => app.focus({ steal: true }),
      chooseFiles: async (win) => {
        // Files and folders at once where the picker allows it (macOS); Windows and Linux pick files.
        const properties: ("openFile" | "openDirectory" | "multiSelections")[] =
          process.platform === "darwin" ? ["openFile", "openDirectory", "multiSelections"] : ["openFile", "multiSelections"];
        const r = win instanceof BrowserWindow ? await dialog.showOpenDialog(win, { properties }) : await dialog.showOpenDialog({ properties });
        return r.canceled ? null : r.filePaths;
      },
      pip,
      ...prompts,
    },
  });
  env.model.startListPoll();
  // Keyvault access is never silent: the notch indicator follows live deliveries even with no window open.
  env.model.keyvault.startPoll();
  if (refresh && env.cua) {
    const cua = env.cua;
    env.model.startup.onReady.push(() => void refreshAfterUpdate(native, cua).then((notice) => notice && env.model.show(notice, false)));
  }
  const model = env.model;
  // Running Spaces' thumbnails stay a minute or two old while the app is in
  // use (not hidden), for the notch's tiles and the page's previews.
  model.thumbnails.keepFresh(
    () => model.streamableSpaceIds,
    () => !(process.platform === "darwin" && app.isHidden()),
  );
  // Devices asking to join, the notifications feed, launch at login, the keychain prompt.
  const background = startHost(bridge.context, { onboarded: () => readOnboardingCompleted(env.onboardingPath) ?? false });

  // macOS: the notch (Cua Spaces Notch.app), fed from the model.
  const notchUi: NotchFeedUi = {
    showSpace: (spaceId, dropped) =>
      showRoute(`/spaces/${encodeURIComponent(spaceId)}${dropped?.length ? `?dropped=${encodeURIComponent(JSON.stringify(dropped))}` : ""}`),
    notify: (title, body) => {
      if (Notification.isSupported()) new Notification({ title, body, silent: true }).show();
    },
  };
  const notch = await connectNotch({
    core: native as unknown as NotchCore,
    thumbnail: notchThumbnail(model),
    actions: {
      openSpace: (spaceId) => openSpaceFromNotch(model, notchUi, spaceId),
      openMain: () => showRoute("/spaces"),
      openSettings: () => showRoute("/settings"),
      openAccess: () => showRoute("/keyvault?view=access"),
      // Hides the indicator and the tiles' key; nothing is revoked or wiped.
      dismissAccess: () => model.keyvault.dismiss(),
      // No window-drag monitor yet (no `teleport` below), so no drag commits.
      teleport: (spaceId) => showRoute(`/spaces/${encodeURIComponent(spaceId)}`),
      drop: (spaceId, paths) => void dropOnSpace(model, notchUi, spaceId, paths),
    },
  });
  if (notch) feedNotch(notch, model);
  return { env, bridge, background };
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  registerSchemePrivileges();

  let mainWindow: BrowserWindow | null = null;

  const showMain = () => {
    if (!mainWindow || mainWindow.isDestroyed()) {
      mainWindow = createMainWindow({ demo: E2E_DEMO });
      mainWindow.on("close", (event) => {
        if (closesToTray() && !isQuitting()) {
          event.preventDefault();
          mainWindow?.hide();
        }
      });
      mainWindow.on("closed", () => {
        mainWindow = null;
      });
      return mainWindow;
    }
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
    return mainWindow;
  };

  /** The main window on a route of the app (the notch's and the tray's clicks). */
  const showRoute = (route: string) => {
    const win = showMain();
    void win.loadURL(`${APP_ORIGIN}${route}`);
  };

  app.on("second-instance", showMain);

  app.whenReady().then(async () => {
    migrateSwiftApp();
    initTheme();
    installMenu();
    handleViewerKeyboard();
    if (process.platform !== "darwin") handleOverlayDim();
    const { webRoot } = handleAppProtocol();
    if (process.env.CUA_SPACES_LOG_STARTUP) console.log(`[cua-spaces] serving ${webRoot}`);

    let host: Awaited<ReturnType<typeof startNativeHost>> | null = null;
    if (!E2E_DEMO) {
      try {
        host = await startNativeHost(showRoute);
      } catch (error) {
        // A broken install: say so plainly, never show the app without its data.
        const why = error instanceof Error ? error.message : String(error);
        dialog.showErrorBox(
          "Cua Spaces can't start",
          app.isPackaged
            ? `Cua Spaces could not load its native library (${why}). Reinstall Cua Spaces.`
            : `The native layer is missing or broken (${why}). Build it with \`pnpm native\` in apps/cua-spaces-desktop.`,
        );
        app.exit(1);
        return;
      }
    }
    installBridgeIpc({
      registry: host?.bridge.registry ?? null,
      events: host?.bridge.events ?? null,
      // The parity flows' traces are a Mac's: under the e2e switch the page can play another platform.
      platform: (E2E_DEMO ? demoPlatform() : null) ?? process.platform,
      videoStats: countsVideo(VIDEO_BENCH),
    });

    nativeTheme.on("updated", () => {
      for (const win of BrowserWindow.getAllWindows()) applyThemeToWindow(win);
    });

    // Opened at login once the first run is done: the menu bar item (the
    // notch, the tray) only, as after the window is closed.
    const onboarded = host ? (readOnboardingCompleted(host.env.onboardingPath) ?? false) : true;
    const main = openedAtLogin() && onboarded ? null : showMain();
    if (main && host && (VIDEO_BENCH.bench || countsVideo(VIDEO_BENCH))) {
      const model = host.env.model;
      startVideoBench(VIDEO_BENCH, { main, openSpace: (id) => openSpaceWindow(id, model.spaces.find((s) => s.id === id)?.name ?? id) });
    }
    installTray({
      actions: {
        open: () => void showMain(),
        route: showRoute,
        // The page's New Space wizard, once the page listens.
        newSpace: () => {
          const fresh = !mainWindow || mainWindow.isDestroyed();
          const win = showMain();
          const ask = () => host?.bridge.events.emit("spaces.newRequested", { on: null });
          if (fresh || win.webContents.isLoading()) win.webContents.once("did-finish-load", () => setTimeout(ask, 500));
          else ask();
        },
      },
      items: () => (host ? menuItems(host.bridge.context) : null),
      spaces: () => realSpaces(host?.env.model.spaces ?? []),
      onChange: (listener) => host?.background.onMenuChange(listener) ?? (() => {}),
    });

    app.on("activate", showMain);
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin") app.quit();
  });
}

/** `CUA_SPACES_DEMO_PLATFORM=<platform>[/<arch>]` under the e2e switch. */
function demoPlatform(): NodeJS.Platform | null {
  const os = (process.env.CUA_SPACES_DEMO_PLATFORM ?? "").split("/")[0];
  return os === "darwin" || os === "win32" || os === "linux" ? os : null;
}
