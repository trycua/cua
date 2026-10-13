// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Machines page (`machines.list`) and This machine's host status
// (`host.status`), as the SwiftUI host answers them: the sidebar's This
// machine row and sections, the account's devices, who the relay sees
// connected, and the host's state with its relay machine id.
import type { AppModel } from "../model/app-model";
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

/** This machine as a host (the core's flattened `HostStatus`), plus its relay machine id and setup progress; null until it answered. */
export function hostStatus(model: AppModel): unknown {
  const state = model.host.state;
  if (!state) return null;
  return { ...(encode(state) as Record<string, unknown>), machineId: model.host.machineId, progress: model.host.progress };
}

/** `relay:<machine>` rows' reported hostnames, by machine id. */
export function relayHostnames(model: AppModel): Record<string, string> {
  const out: Record<string, string> = {};
  for (const space of model.spaces) {
    if (!space.id.startsWith("relay:") || space.host) continue;
    const name = model.backend.reportedHostname(space.id);
    if (name) out[space.id.slice(6)] = name;
  }
  return out;
}

export function machinesState(model: AppModel) {
  const sidebar = model.sidebar;
  const devices = model.devices;
  return {
    thisMachine: encode(sidebar.thisMachine),
    sections: encode(sidebar.sections),
    devices: devices.snapshot === null ? null : encode(devices.view),
    accessNotice: encode(devices.accessNotice),
    signedIn: devices.signedIn,
    // The Machines page's "This machine" panel.
    host: hostStatus(model),
    // So the page lists a machine and its enrolled device once.
    hostnames: relayHostnames(model),
    // Who the relay sees connected now.
    presence: devices.relayOnline,
    deviceStates: devices.deviceStates,
  };
}

export function machinesMethods({ model }: BridgeContext): Handlers {
  return {
    "machines.list": async () => {
      if (model.host.state === null && model.servicesIn) await model.host.refresh();
      // Who the relay sees connected now, not at the last visit; not before the services are in.
      if (model.servicesIn) await model.devices.refreshIfStale();
      return machinesState(model);
    },
    "host.status": async () => {
      if (model.host.state === null && model.servicesIn) await model.host.refresh();
      return hostStatus(model);
    },
  };
}
