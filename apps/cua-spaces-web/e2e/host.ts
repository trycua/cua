// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where the e2e specs run: the browser against the Vite dev server (the
 * default), or the Electron shell (`PARITY_HOST=electron`, `pnpm
 * parity:electron`): the built shell (apps/cua-spaces-desktop/dist-electron)
 * serving the built web UI (dist/) from `cua-spaces://app`, with a throwaway
 * HOME and user data and its window hidden. The shell runs with its test
 * switch `CUA_SPACES_E2E_DEMO=1`: it loads no native library and starts no
 * daemon, and the page runs the browser demo host (`?bridge=demo`, which
 * `page.goto` here adds to every route). Each test gets its own shell. Specs
 * import `test` and `expect` from here; `page.goto("/spaces?…")` opens that
 * route in whichever host runs.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { _electron, test as base, type ElectronApplication, type Page } from "@playwright/test";

export { expect } from "@playwright/test";

export const ELECTRON = process.env.PARITY_HOST === "electron";

const DESKTOP = resolve(fileURLToPath(new URL("../../cua-spaces-desktop/", import.meta.url)));
export const APP_ORIGIN = "cua-spaces://app";

const pick = (...keys: string[]) =>
  Object.fromEntries(keys.flatMap((k) => (process.env[k] ? [[k, process.env[k]!]] : [])));

/**
 * The Electron binary's path. Electron has no install script: requiring it
 * downloads the binary the first time, so `global-setup.ts` does that once,
 * before the workers start (two workers downloading into the same directory
 * race, and one gets `spawn ETXTBSY` from the binary the other is writing).
 */
export const electronPath = () => createRequire(join(DESKTOP, "package.json"))("electron") as unknown as string;

/** `url` with `bridge=demo` in its query (the shell's test switch plays the page's demo host). */
export function withDemoBridge(url: string): string {
  const u = new URL(url, `${APP_ORIGIN}/`);
  u.searchParams.set("bridge", "demo");
  return u.href;
}

/** Launches the built shell with its test switch (the page's demo host). */
export async function launchShell(): Promise<{ app: ElectronApplication; page: Page; close: () => Promise<void> }> {
  const home = mkdtempSync(join(tmpdir(), "cua-e2e-electron-"));
  const executablePath = electronPath();
  const app = await _electron.launch({
    executablePath,
    // The app directory (as `pnpm start`): its main is dist-electron/boot.cjs, and
    // the web root is ../cua-spaces-web/dist next to it.
    // Linux CI runners have no setuid sandbox helper.
    args: process.platform === "linux" && process.env.CI ? ["--no-sandbox", DESKTOP] : [DESKTOP],
    cwd: DESKTOP,
    env: {
      PATH: "/usr/bin:/bin",
      HOME: home,
      TMPDIR: tmpdir(),
      // The test switch: no native library, no daemon, the page's demo host.
      CUA_SPACES_E2E_DEMO: "1",
      CUA_HOME: join(home, ".cua"),
      CUA_SPACES_USER_DATA: join(home, "user-data"),
      CUA_SPACES_E2E_HIDDEN: "1",
      // The parity flows' traces are a Mac's: the shell reports a Mac to the page on any OS.
      CUA_SPACES_DEMO_PLATFORM: "darwin/arm64",
      CUA_TELEMETRY: "0",
      DO_NOT_TRACK: "1",
      // Linux: the X server (xvfb-run in CI) and its cookie.
      ...pick("DISPLAY", "XAUTHORITY"),
    },
  });
  const page = await app.firstWindow();
  await page.waitForURL(`${APP_ORIGIN}/**`);
  const goto = page.goto.bind(page);
  page.goto = (url, options) => goto(url.startsWith("/") || url.startsWith(APP_ORIGIN) ? withDemoBridge(url) : url, options);
  const close = async () => {
    await app.close().catch(() => {});
    rmSync(home, { recursive: true, force: true });
  };
  return { app, page, close };
}

/** `test` with `page` in the host this run targets. The parity flows'
 * traces are a Mac's ("This Mac"): on any OS the page runs as on a Mac (the
 * platform the shell reports, or the browser's `navigator.platform`). */
export const test = ELECTRON
  ? base.extend<{ page: Page }>({
      page: async ({}, use) => {
        const shell = await launchShell();
        try {
          await use(shell.page);
        } finally {
          await shell.close();
        }
      },
    })
  : base.extend<{ page: Page }>({
      page: async ({ page }, use) => {
        await page.addInitScript(() => Object.defineProperty(Navigator.prototype, "platform", { get: () => "MacIntel" }));
        await use(page);
      },
    });
