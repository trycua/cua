// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's This machine: its host status, which setup and the
 * page's buttons change in memory. */

import { HostError } from "../../adapter";
import type { HostStatus } from "../../contracts/host";
import { HOST_SETTING_CHANGE, isSettingsPane } from "../../ops/host-setup";
import { normalizePairingCode } from "../../pairing-code";
import { demoHostStatus } from "../demo-data";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoHostSetupState {
  /** This machine as a host (`machines.list`'s current row, `host.status`). */
  host: HostStatus;
  /** The demo Mac's Local Network switch for the app (off until Allow). */
  localNetworkGranted: boolean;
}

export const demoHostSetupState = (now: number, firstRun: boolean): DemoHostSetupState => ({ host: demoHostStatus(now, firstRun), localNetworkGranted: false });

export const LOCAL_NETWORK_PANE = "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

/**
 * `host.status` as the Mac app answers it: with `localNetwork`, a Mac set up to run Spaces (Apple silicon) also lists
 * its Local Network access, as the "local-network" permission.
 */
export function demoHostStatusFor(state: DemoHostSetupState, args?: { localNetwork?: boolean }): HostStatus {
  const h = { ...state.host };
  if (!args?.localNetwork || !h.configured || !h.provideSpaces) return h;
  return {
    ...h,
    permissions: [...h.permissions, { id: "local-network", label: "Local Network", settingsUrl: LOCAL_NETWORK_PANE, granted: state.localNetworkGranted }],
  };
}

export function demoHostSetupHandlers({
  state,
  now,
  wait,
  emit,
  stepMs,
}: DemoContext): DemoHandlers<"host.setUp" | "host.joinWithCode" | "host.action" | "host.limits" | "host.openSettings"> {
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
      // As cua-host decides when the request leaves it out: a machine set up
      // before keeps its setting; a new relay setup provides Spaces.
      const keep = state.host.configured ? state.host.provideSpaces : true;
      return set({
        ...demoHostStatus(now()),
        mode: direct ? "direct" : "relay",
        relayUrl: direct ? null : (request.relayUrl ?? "https://relay.cua.ai"),
        directUrl: direct ? `http://${request.direct ?? "0.0.0.0:3211"}` : null,
        name: request.name?.trim() || "This Mac",
        shareDesktop: request.shareDesktop ?? !spare,
        provideSpaces: !direct && (request.provideSpaces ?? (spare || keep)),
        permissions: state.host.permissions,
        clients: [],
        recentAccess: [],
      });
    },
    // The demo has no relay: any well-formed code joins, a bad one fails as the CLI does.
    "host.joinWithCode": async ({ code }) => {
      if (!normalizePairingCode(code))
        throw new HostError(`${code.trim() || "That"} is not a pairing code (12 letters and digits, like K7QM-4XPB-2RHD)`, "bad_args");
      await wait(stepMs * 2);
      return set({ ...demoHostStatus(now()), name: "This Mac", permissions: state.host.permissions, clients: [], recentAccess: [] });
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
    // The limits: what is given is kept, and use is counted from the Spaces provided here.
    "host.limits": async (limits) => {
      await wait(stepMs);
      const h = state.host;
      if (!h.configured) throw new HostError("This computer is not set up for access", "not_configured");
      return set({
        ...h,
        maxSpaces: limits.maxSpaces ?? h.maxSpaces,
        maxMemoryGb: limits.maxMemoryGb ?? h.maxMemoryGb,
        maxDiskGb: limits.maxDiskGb ?? h.maxDiskGb,
        maxRunMinutes: limits.maxRunMinutes ?? h.maxRunMinutes,
      });
    },
    // A browser can't open System Settings; the pane only has to be one.
    "host.openSettings": ({ url }) => {
      if (!isSettingsPane(url)) throw new HostError("Only System Settings panes open here", "bad_args");
      // Local Network: the demo plays macOS's prompt answered with Allow.
      if (url === LOCAL_NETWORK_PANE) {
        setTimeout(() => {
          state.localNetworkGranted = true;
          emit({ type: "machines.changed" });
        }, stepMs * 4);
        return null;
      }
      // The demo plays the user turning it on in System Settings.
      setTimeout(() => {
        state.host = { ...state.host, permissions: state.host.permissions.map((p) => (p.settingsUrl === url ? { ...p, granted: true } : p)) };
        emit({ type: "machines.changed" });
      }, stepMs * 4);
      return null;
    },
  };
}
