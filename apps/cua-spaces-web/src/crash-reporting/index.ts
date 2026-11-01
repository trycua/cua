// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Crash reporting for the page (CUA-2343): `@sentry/browser`, started only
 * when the host says reports are allowed, and stopped when it says they
 * no longer are.
 *
 * The page runs in the Electron shell and in the SwiftUI app's WKWebView, so
 * it asks the host (`crashReporting.config`): `allowed` is the "Share usage
 * data" switch with its first-run notice, and the DSN, release and channel
 * are the app's. A host that doesn't answer (Tauri, the demo host, an older
 * app, an error) means off, and nothing of the SDK is even downloaded.
 *
 * The SDK is set up for errors only: no breadcrumbs, no tracing, no replay,
 * no screenshots, no feedback widget, no user, no request context. Every
 * event passes `scrubEvent` and a live check of the switch (`beforeSend`).
 * Native crashes and main-process errors are the shell's, not the page's.
 */

import type { BridgeMode, DataAdapter } from "../bridge/adapter";
import { createAdapter } from "../bridge/adapters";
import { detectMode } from "../bridge/detect";
import type { CrashReportingConfig } from "../bridge/contracts/host";
import { scrubEvent, type ScrubbableEvent } from "./scrub";

/** The hosts that can answer `crashReporting.config`: the two native shells. */
export const reportsFrom = (mode: BridgeMode): boolean => mode === "electron" || mode === "webkit";

/** The page's forced crash (CUA_CRASH_TEST=ui): only a `local` build, only when the host asks. */
export const wantsUiTestCrash = (c: CrashReportingConfig): boolean => c.crashTest === "ui" && c.environment === "local";

/** The subset of `@sentry/browser` used here (so tests can stand in). */
export interface SentryLike {
  init(options: Record<string, unknown>): unknown;
  close(timeout?: number): Promise<boolean>;
  globalHandlersIntegration(): unknown;
  dedupeIntegration(): unknown;
  linkedErrorsIntegration(): unknown;
  functionToStringIntegration(): unknown;
  eventFiltersIntegration(): unknown;
}

/** The SDK's options for this host's settings: error handlers and de-duplication only, nothing that records what people do. `isAllowed` is read for every event. */
export function sentryOptions(sentry: SentryLike, config: CrashReportingConfig, isAllowed: () => boolean): Record<string, unknown> {
  return {
    dsn: config.dsn,
    release: config.release,
    environment: config.environment,
    sendDefaultPii: false,
    defaultIntegrations: false,
    integrations: [sentry.globalHandlersIntegration(), sentry.dedupeIntegration(), sentry.linkedErrorsIntegration(), sentry.functionToStringIntegration(), sentry.eventFiltersIntegration()],
    maxBreadcrumbs: 0,
    sendClientReports: false,
    initialScope: { tags: { shell: config.shell, component: "ui" } },
    beforeBreadcrumb: () => null,
    beforeSendTransaction: () => null,
    beforeSend: (event: ScrubbableEvent) => (isAllowed() ? scrubEvent(event) : null),
  };
}

export interface CrashReportingHandle {
  /** Stops reporting and removes the listener. */
  stop(): void;
}

export interface StartOptions {
  adapter?: DataAdapter;
  mode?: BridgeMode;
  /** Loads the SDK; the default is a dynamic import, so a page that reports nothing never fetches it. */
  load?: () => Promise<SentryLike>;
  /** Runs the forced crash (a test hook; the default throws from a timer). */
  crash?: () => void;
}

/**
 * Asks the host and, when allowed, starts the SDK. Resolves to a handle, or
 * null when reporting is off. Never throws: crash reporting must not stop the
 * page from starting.
 */
export async function startCrashReporting(o: StartOptions = {}): Promise<CrashReportingHandle | null> {
  try {
    const mode = o.mode ?? o.adapter?.mode ?? detectMode();
    if (!reportsFrom(mode)) return null;
    const adapter = o.adapter ?? createAdapter(mode);
    const own = !o.adapter;
    const first = await adapter.call("crashReporting.config", {}).catch(() => null);
    if (!first || !first.allowed || !first.dsn) {
      if (own) adapter.dispose?.();
      return null;
    }
    let allowed = true;
    const sentry = await (o.load ?? (() => import("@sentry/browser") as unknown as Promise<SentryLike>))();
    sentry.init(sentryOptions(sentry, first, () => allowed));

    let stopped = false;
    const stop = () => {
      if (stopped) return;
      stopped = true;
      allowed = false;
      unsubscribe();
      void sentry.close(0).catch(() => {});
      if (own) adapter.dispose?.();
    };
    // The switch may go off while the page is open: the host pushes `settings.changed`.
    let unsubscribe = () => {};
    unsubscribe = adapter.subscribe((event) => {
      if (event.type !== "settings.changed") return;
      void adapter.call("crashReporting.config", {}).then(
        (c) => {
          if (!c.allowed) stop();
        },
        () => {},
      );
    });

    if (wantsUiTestCrash(first)) {
      const crash = o.crash ?? (() => { throw new Error("CUA_CRASH_TEST ui"); });
      setTimeout(crash, 3000);
    }
    return { stop };
  } catch {
    return null;
  }
}
