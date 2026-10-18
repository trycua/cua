// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `CUA_SPACES_CAPTURE_ROUTES=<dir>`: loads every route in light and dark,
// checks that the page is on the Electron bridge and rendered, saves a
// 1440×900 `capturePage` PNG per route and theme into <dir>, prints a JSON report and
// quits.
import { app, type BrowserWindow } from "electron";
import { mkdirSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import { APP_ORIGIN } from "./protocol";
import { setThemeSource } from "./theme";

const ROUTES = ["spaces", "machines", "agents", "keyvault", "settings", "onboarding"];
const THEMES = ["light", "dark"] as const;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function load(win: BrowserWindow, url: string): Promise<void> {
  await win.loadURL(url);
  // Wait for the bridge to report its mode and the route to fill in.
  for (let i = 0; i < 50; i++) {
    const ready = await win.webContents.executeJavaScript(
      `Boolean(document.documentElement.dataset.bridge) && (document.querySelector("main")?.innerText.trim().length ?? 0) > 0`,
    );
    if (ready) break;
    await sleep(100);
  }
  await sleep(700);
}

export async function captureRoutes(win: BrowserWindow, dir: string): Promise<void> {
  mkdirSync(dir, { recursive: true });
  win.setContentSize(1440, 900);
  const report: unknown[] = [];
  try {
    for (const theme of THEMES) {
      setThemeSource(theme);
      await load(win, `${APP_ORIGIN}/spaces`);
      await win.webContents.executeJavaScript(`localStorage.setItem("cua-spaces:theme", ${JSON.stringify(theme)})`);
      for (const route of ROUTES) {
        await load(win, `${APP_ORIGIN}/${route}`);
        const page = (await win.webContents.executeJavaScript(`({
          bridge: document.documentElement.dataset.bridge,
          dark: document.documentElement.classList.contains("dark"),
          heading: document.querySelector("main h1")?.textContent ?? null,
          text: (document.querySelector("main")?.innerText ?? "").trim().length,
        })`)) as { bridge: string; dark: boolean; heading: string | null; text: number };
        const file = path.join(dir, `${route}-${theme}.png`);
        writeFileSync(file, (await win.webContents.capturePage()).toPNG());
        report.push({ route, theme, ...page, file: path.basename(file) });
      }
    }
    console.log(`[cua-spaces] capture ${JSON.stringify(report)}`);
  } finally {
    setThemeSource("system");
    app.quit();
  }
}
