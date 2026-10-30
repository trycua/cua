// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Crash reporting, the Electron half (CUA-2343): Sentry in the main process
// only. It catches native crashes (Crashpad minidumps), main-process errors
// and one session per launch for release health. The page reports its own
// errors (apps/cua-spaces-web/src/crash-reporting), and the Rust core and
// daemon get the same settings through CUA_CRASH_* in the environment.
//
// Sentry must start before the app is ready, and before the native library
// loads, so the consent rule is read from the files the library reads
// (crash-reporting.ts). Reports stop when the switch goes off (the SDK is
// closed, and every event is checked again); a switch turned on takes effect
// at the next launch.
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import * as Sentry from "@sentry/electron/main";
import { app } from "electron";
import {
  crashConsent,
  crashEnv,
  crashTestKind,
  environmentFor,
  onConsentMayHaveChanged,
  parseCrashConfig,
  releaseFor,
  scrubEvent,
  setCrashSettings,
  type ConsentSource,
  type ScrubbableEvent,
} from "./crash-reporting";

/** The DSN, from apps/cua-spaces-desktop/config/spaces-crash-reporting.json at build time (tsdown.config.mts). Absent in a test run. */
declare const __CUA_CRASH_CONFIG__: string | undefined;

/** Integrations that would record content or the machine's identity: breadcrumbs, local variables and source lines, screenshots, the renderer hooks. */
const DROPPED = new Set([
  "Console",
  "ElectronBreadcrumbs",
  "ElectronNet",
  "NodeFetch",
  "LocalVariables",
  "ContextLines",
  "Screenshots",
  "PreloadInjection",
  "RendererEventLoopBlock",
  "RendererProfiling",
  "AdditionalContext",
  "GpuContext",
]);

function machine(): ConsentSource {
  return {
    env: process.env,
    read: (file) => {
      try {
        return readFileSync(file, "utf8");
      } catch {
        return null;
      }
    },
    exists: (file) => existsSync(file),
    home: homedir(),
  };
}

/** Starts Sentry when a DSN is configured and the switch allows it; call first thing in the main process. */
export function initCrashReporting(): void {
  const { dsn } = parseCrashConfig(typeof __CUA_CRASH_CONFIG__ === "string" ? __CUA_CRASH_CONFIG__ : "");
  if (!dsn) return;
  const version = app.getVersion();
  const release = releaseFor(version);
  const environment = environmentFor(version, app.isPackaged);
  const test = crashTestKind(process.env.CUA_CRASH_TEST, environment);
  if (!crashConsent(machine()).allowed) return;

  Object.assign(process.env, crashEnv({ dsn, release, environment }));
  setCrashSettings({ dsn, release, environment, uiTest: test === "ui" });
  const home = homedir();
  Sentry.init({
    dsn,
    release,
    environment,
    // Sentry 11 replaced `sendDefaultPii` with `dataCollection`: nothing about the user, cookies, headers, bodies or query strings.
    dataCollection: { userInfo: false, cookies: false, httpHeaders: false, httpBodies: [], urlQueryParams: false },
    debug: process.env.CUA_CRASH_DEBUG === "1",
    maxBreadcrumbs: 0,
    attachScreenshot: false,
    enableRendererProfiling: false,
    // No traces, no profiles, no logs.
    tracesSampleRate: 0,
    // No renderer is wired to Sentry: no session of the default window, no protocol route.
    ipcMode: Sentry.IPCMode.Classic,
    getSessions: () => [],
    initialScope: { tags: { shell: "electron", component: "shell" } },
    integrations: (defaults) => defaults.filter((i) => !DROPPED.has(i.name)),
    beforeBreadcrumb: () => null,
    beforeSendTransaction: () => null,
    beforeSend: (event) => (crashConsent(machine()).allowed ? (scrubEvent(event as unknown as ScrubbableEvent, home) as unknown as typeof event) : null),
  });

  onConsentMayHaveChanged(() => {
    if (crashConsent(machine()).allowed) return;
    setCrashSettings(null);
    void Sentry.close(0);
  });

  if (test === "main-js") setTimeout(() => { throw new Error("CUA_CRASH_TEST main-js"); }, 4000);
  if (test === "main-native") setTimeout(() => process.crash(), 4000);
}
