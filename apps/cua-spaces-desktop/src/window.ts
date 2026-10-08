// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { app, BrowserWindow, ipcMain, screen, shell, type BrowserWindowConstructorOptions } from "electron";
import { existsSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import { captureRoutes } from "./capture";
import { windowIconPath } from "./icon";
import { APP_ORIGIN } from "./protocol";
import { readSettings, writeSettings, type WindowBounds } from "./settings";
import { DIM_CHANNEL, overlayColors } from "./overlay";
import { KEYBOARD_CHANNEL, applyKeyboard } from "./keyboard";
import { BACKGROUND, resolvedTheme } from "./theme";

const TITLEBAR_HEIGHT = 40;
// Room for the macOS traffic lights at x=16 plus a gap.
const MAC_TITLEBAR_LEFT_INSET = 78;
const DEFAULT_SIZE = { width: 1280, height: 820 };

export let readyToShowMs: number | null = null;

function titleBarOptions(): Pick<
  BrowserWindowConstructorOptions,
  "titleBarStyle" | "trafficLightPosition" | "titleBarOverlay"
> {
  if (process.platform === "darwin") {
    return {
      titleBarStyle: "hiddenInset",
      // Vertically centred on the 40px top bar (button radius ~6px).
      trafficLightPosition: { x: 16, y: TITLEBAR_HEIGHT / 2 - 6 },
    };
  }
  return {
    titleBarStyle: "hidden",
    titleBarOverlay: { ...overlayColors(resolvedTheme(), false), height: TITLEBAR_HEIGHT },
  };
}

// Restores saved bounds only if they still land on a connected display.
function restoredBounds(): WindowBounds {
  const saved = readSettings().windowBounds;
  if (!saved || saved.x === undefined || saved.y === undefined) return { ...DEFAULT_SIZE };
  const visible = screen.getAllDisplays().some(({ workArea: a }) => {
    const x = Math.max(a.x, saved.x!);
    const y = Math.max(a.y, saved.y!);
    const w = Math.min(a.x + a.width, saved.x! + saved.width) - x;
    const h = Math.min(a.y + a.height, saved.y! + saved.height) - y;
    return w >= 100 && h >= 100;
  });
  return visible ? saved : { ...DEFAULT_SIZE };
}

function trackBounds(win: BrowserWindow): void {
  let timer: NodeJS.Timeout | undefined;
  const save = () => {
    if (win.isDestroyed() || win.isMinimized() || win.isFullScreen()) return;
    const maximized = win.isMaximized();
    const b = maximized ? (readSettings().windowBounds ?? win.getNormalBounds()) : win.getBounds();
    writeSettings({ windowBounds: { ...b, maximized } });
  };
  const debounced = () => {
    clearTimeout(timer);
    timer = setTimeout(save, 400);
  };
  win.on("resize", debounced);
  win.on("move", debounced);
  win.on("maximize", save);
  win.on("unmaximize", save);
  win.on("close", () => {
    clearTimeout(timer);
    save();
  });
}

// `--titlebar-left-inset` tells the web UI how much room to leave for the
// macOS traffic lights. It is 0 in fullscreen and on other platforms, where
// the controls sit on the right inside the title bar overlay.
function syncTitlebarInset(win: BrowserWindow): void {
  let key: string | undefined;
  const apply = async () => {
    if (win.isDestroyed()) return;
    const inset = process.platform === "darwin" && !win.isFullScreen() ? MAC_TITLEBAR_LEFT_INSET : 0;
    const css = `:root{--titlebar-left-inset:${inset}px;--titlebar-height:${TITLEBAR_HEIGHT}px}`;
    const previous = key;
    key = await win.webContents.insertCSS(css, { cssOrigin: "author" });
    if (previous) await win.webContents.removeInsertedCSS(previous).catch(() => {});
  };
  win.webContents.on("dom-ready", () => {
    key = undefined;
    void apply();
  });
  win.on("enter-full-screen", () => void apply());
  win.on("leave-full-screen", () => void apply());
}

// Windows whose page shows a dialog backdrop (see overlay.ts).
const dimmed = new WeakSet<BrowserWindow>();

/** The page's background color: the window's, and (Windows, Linux) the
 * title bar overlay's in the matching theme (`window.setBackgroundColor`). */
export function setWindowBackground(win: BrowserWindow, color: string): void {
  if (win.isDestroyed()) return;
  win.setBackgroundColor(color);
  if (process.platform !== "darwin") {
    win.setTitleBarOverlay({ ...overlayColors(isDark(color) ? "dark" : "light", dimmed.has(win)), height: TITLEBAR_HEIGHT });
  }
}

/** Whether `#rrggbb` is a dark background (relative luminance under a half). */
export function isDark(color: string): boolean {
  const n = Number.parseInt(color.replace(/^#/, ""), 16);
  const [r, g, b] = [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
  return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255 < 0.5;
}

export function applyThemeToWindow(win: BrowserWindow): void {
  if (win.isDestroyed()) return;
  const theme = resolvedTheme();
  win.setBackgroundColor(BACKGROUND[theme]);
  if (process.platform !== "darwin") {
    win.setTitleBarOverlay({ ...overlayColors(theme, dimmed.has(win)), height: TITLEBAR_HEIGHT });
  }
}

// The preload reports when a dialog backdrop comes and goes; the overlay
// dims with the rest of the window. Only the app's own page may ask.
export function handleOverlayDim(): void {
  ipcMain.on(DIM_CHANNEL, (event, dim: unknown) => {
    const win = BrowserWindow.fromWebContents(event.sender);
    if (!win || !event.senderFrame?.url.startsWith(`${APP_ORIGIN}/`)) return;
    if (dim === true) dimmed.add(win);
    else dimmed.delete(win);
    applyThemeToWindow(win);
  });
}

// The preload reports when a Space's live desktop takes or gives back the
// keyboard; its window's menu shortcuts stand aside meanwhile (keyboard.ts).
export function handleViewerKeyboard(): void {
  ipcMain.on(KEYBOARD_CHANNEL, (event, has: unknown) => {
    if (!event.senderFrame?.url.startsWith(`${APP_ORIGIN}/`)) return;
    applyKeyboard(event.sender, has);
  });
}

/** End-to-end runs (`CUA_SPACES_E2E_HIDDEN=1`, the web UI's e2e on this
 * shell): the window never shows, so a run doesn't cover the screen. */
const hidden = () => process.env.CUA_SPACES_E2E_HIDDEN === "1";

/** A window of the app: its icon, title bar, theme and sandboxed preload;
 * links that leave the app open in the user's browser, and it never
 * navigates away from the app origin. */
export function appWindow(options: BrowserWindowConstructorOptions): BrowserWindow {
  const win = new BrowserWindow({
    minWidth: 720,
    minHeight: 480,
    show: false,
    icon: windowIconPath({
      platform: process.platform,
      packaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      devIconsDir: path.join(__dirname, "../../cua-spaces/src-tauri/icons"),
      exists: existsSync,
    }),
    backgroundColor: BACKGROUND[resolvedTheme()],
    ...titleBarOptions(),
    ...options,
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      webSecurity: true,
      spellcheck: false,
      // A hidden e2e window still runs its timers and animations.
      backgroundThrottling: !hidden(),
    },
  });
  syncTitlebarInset(win);
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:\/\//.test(url)) void shell.openExternal(url);
    return { action: "deny" };
  });
  win.webContents.on("will-navigate", (event, url) => {
    if (!url.startsWith(`${APP_ORIGIN}/`)) {
      event.preventDefault();
      if (/^https?:\/\//.test(url)) void shell.openExternal(url);
    }
  });
  return win;
}

/** `demo`: the parity flows' test switch (the page plays the browser demo
 * host). `remember: false` (the video bench, which places the window
 * itself): the saved bounds are neither used nor overwritten. */
export function createMainWindow({ demo = false, remember = true }: { demo?: boolean; remember?: boolean } = {}): BrowserWindow {
  const bounds = hidden() ? { ...DEFAULT_SIZE, width: 1440, height: 900 } : remember ? restoredBounds() : { ...DEFAULT_SIZE };
  const win = appWindow({ ...bounds, title: "Cua Spaces" });

  win.once("ready-to-show", () => {
    // performance.now() counts from process start.
    readyToShowMs = performance.now();
    if (process.env.CUA_SPACES_LOG_STARTUP) {
      console.log(`[cua-spaces] ready-to-show ${readyToShowMs.toFixed(0)}ms after process start`);
    }
    if (hidden()) return;
    if (bounds.maximized) win.maximize();
    win.show();
  });

  // Measurement hooks for scripts/measure.mjs: capture the window to a PNG
  // and/or quit once the page has loaded.
  const capturePath = process.env.CUA_SPACES_CAPTURE;
  if (capturePath || process.env.CUA_SPACES_QUIT_AFTER_LOAD) {
    win.webContents.once("did-finish-load", () => {
      setTimeout(async () => {
        if (capturePath) {
          const image = await win.webContents.capturePage();
          writeFileSync(capturePath, image.toPNG());
        }
        app.quit();
      }, capturePath ? 1500 : 0);
    });
  }

  const routesDir = process.env.CUA_SPACES_CAPTURE_ROUTES;
  if (routesDir) win.webContents.once("did-finish-load", () => void captureRoutes(win, routesDir));

  if (remember) trackBounds(win);
  void win.loadURL(demo ? `${APP_ORIGIN}/?bridge=demo` : `${APP_ORIGIN}/`);
  return win;
}
