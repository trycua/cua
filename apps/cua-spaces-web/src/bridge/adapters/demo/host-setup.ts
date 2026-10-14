// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's This machine: its host status, which setup and the
 * page's buttons change in memory. */

import { HostError } from "../../adapter";
import type { HostStatus } from "../../contracts/host";
import { HOST_SETTING_CHANGE, isSettingsPane } from "../../ops/host-setup";
import { demoHostStatus } from "../demo-data";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoHostSetupState {
  /** This machine as a host (`machines.list`'s current row, `host.status`). */
  host: HostStatus;
}

export const demoHostSetupState = (now: number, firstRun: boolean): DemoHostSetupState => ({ host: demoHostStatus(now, firstRun) });

export function demoHostSetupHandlers({ state, now, wait, emit, stepMs }: DemoContext): DemoHandlers<"host.setUp" | "host.action" | "host.openSettings"> {
  const set = (host: HostStatus) => {
    state.host = host;
    emit({ type: "machines.changed" });
    return { ...host };
  };
  return {
    "host.setUp": async ({ request }) => {
      await wait(stepMs * 2);
      const spare = request.profile === "spare";
      const direct = request.mode === "direct";
      return set({
        ...demoHostStatus(now()),
        mode: direct ? "direct" : "relay",
        relayUrl: direct ? null : (request.relayUrl ?? "https://relay.cua.ai"),
        directUrl: direct ? `http://${request.direct ?? "0.0.0.0:3211"}` : null,
        name: request.name?.trim() || "This Mac",
        shareDesktop: request.shareDesktop ?? !spare,
        provideSpaces: !direct && (request.provideSpaces ?? spare),
        permissions: state.host.permissions,
        clients: [],
        recentAccess: [],
      });
    },
    "host.action": async ({ action }) => {
      await wait(stepMs);
      const h = state.host;
      if (!h.configured) throw new HostError("This machine is not set up for access", "not_configured");
      switch (action) {
        case "stop-sharing":
          return set({ ...h, sharing: false, clients: [] });
        case "resume-sharing":
          return set({ ...h, sharing: true });
        case "remove":
          return set({ ...demoHostStatus(now(), true), permissions: h.permissions });
        case "sign-in":
          // Signed in again: relay sharing that waited for it resumes.
          return set({ ...h, pausedSignedOut: false });
      }
      const change = HOST_SETTING_CHANGE[action];
      if (!change) throw new HostError(`${action} is not a host action`, "bad_args");
      return set({ ...h, ...change });
    },
    // A browser can't open System Settings; the pane only has to be one.
    "host.openSettings": ({ url }) => {
      if (!isSettingsPane(url)) throw new HostError("Only System Settings panes open here", "bad_args");
      return null;
    },
  };
}
