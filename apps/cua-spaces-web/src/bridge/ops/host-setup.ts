// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The This machine page's buttons and the host setup form's submit: the
 * host commands the native apps already run (`HostModel` in the SwiftUI
 * app, `host_*` in Tauri). The request is the one the core's form builds
 * and validates (`host.formView`'s `request`); the page never makes up its
 * own.
 *
 * | Operation | Tauri | SwiftUI |
 * |---|---|---|
 * | `host.setUp {request}` | `host_setup` | `HostModel.submit` |
 * | `host.action {action}` | `host_stop_sharing`, `host_start_sharing`, `host_remove`, `host_configure` | `HostModel.run(action)` (`sign-in`: the inline sign-in, then sharing follows it) |
 * | `host.openSettings {url}` | `host_open_settings` | `PermissionRows.open` |
 */

import { HostError } from "../adapter";
import type { OpCoverage } from "../coverage";
import type { HostActionId, HostStatus } from "../contracts/host";

/** What host setup receives (`host::HostSetupRequest`). */
export interface HostSetupRequest {
  mode: "relay" | "direct" | string;
  relayUrl?: string | null;
  direct?: string | null;
  name?: string | null;
  allow?: string[] | null;
  profile?: string | null;
  shareDesktop?: boolean | null;
  provideSpaces?: boolean | null;
}

/** The form's own state (`host::HostFormState`). */
export interface HostFormState {
  name: string;
  allow: string;
  advanced: boolean;
  direct: boolean;
  listen: string;
  relayUrl: string;
  busy: boolean;
  error?: string | null;
  spare: boolean;
}

/** An input to the form (`host::HostFormAction`). */
export type HostFormAction =
  | { type: "set-name"; name: string }
  | { type: "set-allow"; allow: string }
  | { type: "toggle-advanced" }
  | { type: "set-direct"; on: boolean }
  | { type: "set-listen"; listen: string }
  | { type: "set-relay-url"; url: string }
  | { type: "set-profile"; profile: string }
  | { type: "submit" }
  | { type: "failed"; error: string };

/** The panel buttons a host runs (`set-up` only opens the form; `sign-in`
 * signs in to Cua, and relay sharing paused while signed out resumes). */
export type HostRunAction = Exclude<HostActionId, "set-up">;

export interface HostSetupOps {
  /** Sets this machine up for access; answers its status after. */
  "host.setUp": { args: { request: HostSetupRequest }; result: HostStatus };
  /** Runs a This machine button; answers the status after. */
  "host.action": { args: { action: HostRunAction }; result: HostStatus };
  /** Opens a System Settings privacy pane (`x-apple.systempreferences:` only). */
  "host.openSettings": { args: { url: string }; result: null };
}


/** The settings change a switch runs (`host::setting_change`). */
export const HOST_SETTING_CHANGE: Partial<Record<HostActionId, { shareDesktop?: boolean; provideSpaces?: boolean }>> = {
  "share-desktop": { shareDesktop: true },
  "hide-desktop": { shareDesktop: false },
  "provide-spaces": { provideSpaces: true },
  "stop-providing-spaces": { provideSpaces: false },
};

/** Only System Settings privacy panes open from the page. */
export const isSettingsPane = (url: string) => url.startsWith("x-apple.systempreferences:");

/* ---- Hosts ------------------------------------------------------------------------- */

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Args<K extends keyof HostSetupOps> = HostSetupOps[K]["args"];

export function tauriHostSetupOps(invoke: Invoke) {
  return {
    "host.setUp": ({ request }: Args<"host.setUp">) => invoke<HostStatus>("host_setup", { request }),
    "host.action": async ({ action }: Args<"host.action">): Promise<HostStatus> => {
      switch (action) {
        case "stop-sharing":
          return invoke("host_stop_sharing");
        case "resume-sharing":
          return invoke("host_start_sharing");
        case "remove":
          await invoke("host_remove");
          return invoke("host_status");
      }
      const change = HOST_SETTING_CHANGE[action];
      if (!change) throw new HostError(`${action} is not a host action`, "bad_args");
      return invoke("host_configure", { change });
    },
    "host.openSettings": async ({ url }: Args<"host.openSettings">) => {
      if (!isSettingsPane(url)) throw new HostError("Only System Settings panes open here", "bad_args");
      await invoke("host_open_settings", { url });
      return null;
    },
  };
}

export const HOST_SETUP_COVERAGE = {
  "host.setUp": { webkit: { methods: ["host.setUp"] }, tauri: ["host_setup"] },
  "host.action": {
    webkit: { methods: ["host.action"] },
    tauri: ["host_stop_sharing", "host_start_sharing", "host_remove", "host_status", "host_configure"],
  },
  "host.openSettings": {
    webkit: { methods: ["host.openSettings"] },
    tauri: ["host_open_settings"],
  },
} as const satisfies Record<keyof HostSetupOps, OpCoverage>;
