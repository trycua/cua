// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Devices (`devices.*`): enrolling this machine, approving,
// renaming and revoking devices, confirming machines (the SwiftUI host's
// WebUIBridge+Pages `devices`, over `model/devices.ts`). Approving asks the
// person at this machine first, as the native sheet does.
import { computerName, DevicesUnavailable } from "../model/devices";
import { encode } from "./value";
import { optionalString, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { hostParts } from "./host-parts";

export function devicesMethods(ctx: BridgeContext): Handlers {
  const { model } = ctx;
  const devices = model.devices;
  /** The relay's calls need the session's `Devices` (the launch made it). */
  const relay = () => {
    if (!devices.devices()) throw Failure.unsupported("Devices need a signed-in Cua account");
  };
  const run = async <T>(call: () => Promise<T>): Promise<T> => {
    relay();
    try {
      return await call();
    } catch (error) {
      if (error instanceof DevicesUnavailable) throw Failure.unsupported(error.message);
      if (error instanceof RangeError) throw Failure.badArgs(error.message);
      throw error;
    }
  };
  return {
    "devices.get": async () => {
      relay();
      // As Settings → Devices: a relay that refuses this device (not
      // enrolled) still shows the page, "Needs enrollment" and Enroll…,
      // with the relay's words under it.
      await devices.refresh();
      return {
        ...(encode(devices.input) as Record<string, unknown>),
        readError: devices.error,
        // What the page names this machine before the relay does.
        deviceName: computerName(),
      };
    },
    "devices.enroll": () => run(() => devices.enroll()),
    "devices.checkEnrolled": () => run(() => devices.checkEnrolled()),
    "devices.approve": async (args) => {
      const code = optionalString(args, "code");
      const deviceId = optionalString(args, "deviceId");
      if (!code && !deviceId) throw Failure.badArgs("code or deviceId");
      // The owner check is wired with the system (`hostParts`).
      hostParts(ctx);
      await run(() => devices.approve(code, deviceId));
      return null;
    },
    "devices.rename": async (args) => {
      const id = string(args, "id");
      const name = string(args, "name");
      await run(() => devices.rename(id, name));
      return null;
    },
    "devices.revoke": async (args) => {
      const id = string(args, "id");
      await run(() => devices.revoke(id));
      return null;
    },
    "devices.confirmMachine": async (args) => {
      const id = string(args, "id");
      await run(() => devices.confirmMachine(id));
      return null;
    },
  };
}
