// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Machines page (`machines.list`) and This machine's host status
// (`host.status`), as the SwiftUI host answers them: the sidebar's This
// machine row and sections, the account's devices, who the relay sees
// connected, and the host's state with its relay machine id.
import type { AppModel } from "../model/app-model";
import { computerName, DevicesUnavailable } from "../model/devices";
import { words } from "../model/errors";
import { withTimeout } from "../model/time";
import { optionalString, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { messageText } from "../../../cua-spaces-web/src/lib/messages";
import { encode } from "./value";

/** This machine as a host (the core's flattened `HostStatus`), plus its relay machine id and setup progress; null until it answered. */
export function hostStatus(model: AppModel): unknown {
  const state = model.host.state;
  if (!state) return null;
  const out: Record<string, unknown> = { ...(encode(state) as Record<string, unknown>), machineId: model.host.machineId, progress: model.host.progress };
  // Before setup the host has no name yet: the web UI's setup form starts
  // with what the system calls this computer.
  if (!state.configured && !out.name) out.name = computerName();
  return out;
}

/** `relay:<machine>` rows' reported hostnames, by machine id. */
export function relayHostnames(model: AppModel): Record<string, string> {
  return model.relayHostnames;
}

export function machinesState(model: AppModel) {
  const sidebar = model.sidebar;
  const devices = model.devices;
  return {
    // The web page reads the status as text.
    thisMachine: sidebar.thisMachine ? { ...(encode(sidebar.thisMachine) as Record<string, unknown>), statusText: messageText({ code: sidebar.thisMachine.statusText.code, vars: Object.fromEntries(sidebar.thisMachine.statusText.vars) }) } : null,
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
    // The page's rows as the core lists them (`machines::page_rows`), the
    // same the SwiftUI Machines page shows.
    rows: encode(model.machinePageRows),
    // The page as the core draws it (`machines_view::view`), the same the
    // SwiftUI Machines page draws.
    view: encode(model.machinesView),
  };
}

/** The relay's calls need the session's `Devices`; its refusals in the bridge's words. */
async function relayCall<T>(model: AppModel, call: () => Promise<T>): Promise<T> {
  if (!model.devices.devices()) throw Failure.unsupported("Machines need a signed-in Cua account");
  try {
    return await call();
  } catch (error) {
    if (error instanceof DevicesUnavailable) throw Failure.unsupported(error.message);
    if (error instanceof RangeError) throw Failure.badArgs(error.message);
    throw error;
  }
}

/** How long one machine may take to answer its health check. */
const HEALTH_SECONDS = 35;
/** Machines asked at once. */
const HEALTH_AT_ONCE = 3;

/**
 * `machines.checkHealth`: asks each machine the core's view names a
 * `healthTarget` for (This Mac through its daemon, another of yours over
 * the relay or mesh) and keeps the answers for the view. A machine that is
 * no longer a target (offline, removed) loses its old answer, so no stale
 * check shows as current.
 */
export async function checkMachinesHealth(model: AppModel): Promise<void> {
  const targets = model.machinesView.machines.filter((m) => m.healthTarget !== undefined && m.healthTarget !== null);
  const wanted = new Set(targets.map((m) => m.id));
  for (const id of [...model.machineHealth.keys()]) if (!wanted.has(id)) model.machineHealth.delete(id);
  const queue = [...targets];
  const worker = async () => {
    for (let m = queue.shift(); m; m = queue.shift()) {
      const target = m.healthTarget ? m.healthTarget : null;
      const o = await withTimeout(HEALTH_SECONDS, () => model.backend.machineHealth(target));
      model.machineHealth.set(
        m.id,
        o.ok
          ? {
              id: m.id,
              cuaVersion: o.value.cuaVersion,
              checks: o.value.checks.map((c) => ({ id: c.id, status: c.status, message: c.message, remediation: c.remediation, facts: c.facts })),
              error: undefined,
            }
          : { id: m.id, cuaVersion: "", checks: [], error: words(o.error) },
      );
    }
  };
  await Promise.all(Array.from({ length: Math.min(HEALTH_AT_ONCE, queue.length) }, worker));
  model.changed("machines");
}

export function machinesMethods({ model }: BridgeContext): Handlers {
  return {
    "machines.list": async () => {
      if (model.host.state === null && model.servicesIn) await model.host.refresh();
      // Who the relay sees connected now, not at the last visit; not before the services are in.
      if (model.servicesIn) await model.devices.refreshIfStale();
      return machinesState(model);
    },
    // Renames one of your machines on the relay; `this-mac` is this
    // machine's own registration (its host setup takes the name too).
    "machines.rename": async (args) => {
      const id = string(args, "id");
      const name = string(args, "name");
      const machineId = id === "this-mac" ? model.host.machineId : id;
      if (!machineId) throw Failure.badArgs("this machine is not on the relay");
      await relayCall(model, () => model.devices.renameMachine(machineId, name));
      if (id === "this-mac") await model.host.refresh();
      return null;
    },
    // Remove from account: the machine leaves the relay directory, then
    // the device on it is revoked; the Spaces and machines are read again.
    "machines.remove": async (args) => {
      const machine = optionalString(args, "machine");
      const device = optionalString(args, "device");
      if (!machine && !device) throw Failure.badArgs("machine or device");
      if (machine) await relayCall(model, () => model.backend.relayUnregister(machine));
      if (device) await relayCall(model, () => model.devices.revoke(device));
      else await model.devices.refresh();
      await model.refresh();
      return null;
    },
    "machines.checkHealth": async () => {
      if (model.servicesIn) await checkMachinesHealth(model);
      return null;
    },
    "host.status": async () => {
      if (model.host.state === null && model.servicesIn) await model.host.refresh();
      return hostStatus(model);
    },
  };
}
