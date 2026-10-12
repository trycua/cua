// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { DevicesInput, PairingCode } from "../model/devices";
import { hasTauri } from "./bridge";

/**
 * This device on the relay: the shell's `devices_*` commands over
 * `cua-host` (enrollment, the account's devices and audit log). Approving
 * asks for presence in the shell (Touch ID or the login password on macOS,
 * else the Keyvault passphrase) before anything reaches the relay.
 */
export interface DevicesBridge {
  readonly isNative: boolean;
  snapshot(): Promise<DevicesInput>;
  /** Registers this device: enrolled at once, or a one-time code. */
  enroll(): Promise<{ enrolled: boolean; code: string | null }>;
  /** Whether an enrolled device approved this one yet. */
  checkEnrolled(): Promise<boolean>;
  /** Approving asks for a passphrase (no OS prompt on this system). */
  needsPassphrase(): Promise<boolean>;
  approve(request: { code?: string | null; deviceId?: string | null; passphrase?: string | null }): Promise<void>;
  rename(id: string, name: string): Promise<void>;
  revoke(id: string): Promise<void>;
  /** Vouches for a machine that registered without an enrolled device's
   * proof (S5), after the row's confirmation. */
  confirmMachine(id: string): Promise<void>;
  /** A one-time link, QR code and install commands that add another
   * computer to the account (or to `team`): "Add a machine". */
  createPairingCode(team?: string | null): Promise<PairingCode>;
  /** The account's pairing codes, newest first (without the codes). */
  pairingCodes(): Promise<PairingCode[]>;
  /** Revokes an unused pairing code. */
  revokePairingCode(id: string): Promise<void>;
  /** A system notification (a device asks for approval). */
  notify(title: string, body: string): Promise<void>;
}

export function createTauriDevicesBridge(): DevicesBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    snapshot: () => invoke<DevicesInput>("devices_snapshot"),
    enroll: () => invoke<{ enrolled: boolean; code: string | null }>("devices_enroll"),
    checkEnrolled: () => invoke<boolean>("devices_check_enrolled"),
    needsPassphrase: () => invoke<boolean>("devices_presence_needs_passphrase"),
    approve: ({ code, deviceId, passphrase }) =>
      invoke<void>("devices_approve", { code: code ?? null, deviceId: deviceId ?? null, passphrase: passphrase ?? null }),
    rename: (id, name) => invoke<void>("devices_rename", { id, name }),
    revoke: (id) => invoke<void>("devices_revoke", { id }),
    confirmMachine: (id) => invoke<void>("devices_confirm_machine", { id }),
    createPairingCode: (team) => invoke<PairingCode>("devices_create_pairing_code", { team: team ?? null }),
    pairingCodes: () => invoke<PairingCode[]>("devices_pairing_codes"),
    revokePairingCode: (id) => invoke<void>("devices_revoke_pairing_code", { id }),
    notify: (title, body) => invoke<void>("devices_notify", { title, body }),
  };
}

/** Outside the app (browser dev, tests without a bridge): nothing to show. */
export function createFallbackDevicesBridge(): DevicesBridge {
  const unavailable = () => Promise.reject(new Error("Devices need the Cua Spaces app"));
  return {
    isNative: false,
    snapshot: unavailable,
    enroll: unavailable,
    checkEnrolled: async () => false,
    needsPassphrase: async () => false,
    approve: unavailable,
    rename: unavailable,
    revoke: unavailable,
    confirmMachine: unavailable,
    createPairingCode: unavailable,
    pairingCodes: unavailable,
    revokePairingCode: unavailable,
    notify: async () => {},
  };
}

export function createDevicesBridge(): DevicesBridge {
  return hasTauri() ? createTauriDevicesBridge() : createFallbackDevicesBridge();
}

/**
 * In-memory relay for tests and design work: `enroll` answers with a code
 * (or enrolls when `firstDevice`), approvals flip a pending device, and
 * `presenceFails` makes approve refuse as a cancelled Touch ID would.
 */
export function createFakeDevicesBridge(
  options: {
    snapshot?: DevicesInput;
    firstDevice?: boolean;
    presenceFails?: string;
    needsPassphrase?: boolean;
    approvedAfterChecks?: number;
  } = {},
): DevicesBridge & { calls: string[]; state: { snapshot: DevicesInput } } {
  const state = { snapshot: options.snapshot ?? { devices: [], audit: [] } };
  const calls: string[] = [];
  const pairing: PairingCode[] = [];
  let checks = 0;
  return {
    isNative: true,
    calls,
    state,
    snapshot: async () => {
      calls.push("snapshot");
      return state.snapshot;
    },
    enroll: async () => {
      calls.push("enroll");
      return options.firstDevice ? { enrolled: true, code: null } : { enrolled: false, code: "K7QX-M2RP" };
    },
    checkEnrolled: async () => {
      calls.push("check");
      checks += 1;
      return checks >= (options.approvedAfterChecks ?? 1);
    },
    needsPassphrase: async () => Boolean(options.needsPassphrase),
    approve: async ({ code, deviceId, passphrase }) => {
      calls.push(`approve:${code ?? ""}:${deviceId ?? ""}${passphrase ? ":pass" : ""}`);
      if (options.presenceFails) throw new Error(options.presenceFails);
      state.snapshot = {
        ...state.snapshot,
        devices: state.snapshot.devices.map((d) =>
          d.id === deviceId || (code && d.state === "pending") ? { ...d, state: "enrolled" } : d,
        ),
      };
    },
    rename: async (id, name) => {
      calls.push(`rename:${id}:${name}`);
      state.snapshot = {
        ...state.snapshot,
        devices: state.snapshot.devices.map((d) => (d.id === id ? { ...d, name } : d)),
      };
    },
    revoke: async (id) => {
      calls.push(`revoke:${id}`);
      state.snapshot = {
        ...state.snapshot,
        devices: state.snapshot.devices.map((d) => (d.id === id ? { ...d, state: "revoked" } : d)),
      };
    },
    confirmMachine: async (id) => {
      calls.push(`confirm-machine:${id}`);
      state.snapshot = {
        ...state.snapshot,
        machines: (state.snapshot.machines ?? []).map((m) => (m.id === id ? { ...m, confirmed: true } : m)),
      };
    },
    createPairingCode: async (team) => {
      calls.push(`create-pairing-code:${team ?? ""}`);
      const now = Math.floor(Date.now() / 1000);
      const code = `K7QM-4XPB-${String(pairing.length + 1).padStart(4, "0")}`;
      const made: PairingCode = {
        id: `pc_fake${pairing.length + 1}`,
        code,
        url: `https://spaces.cua.ai/add/${code}`,
        installSh: `curl -fsSL https://cua.ai/install.sh | sh -s -- --pair ${code}`,
        installPowershell: `& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Pair ${code}`,
        state: "active",
        team: team ?? null,
        createdAt: now,
        expiresAt: now + 1800,
        usedAt: null,
        machine: null,
      };
      pairing.unshift(made);
      return made;
    },
    pairingCodes: async () => {
      calls.push("pairing-codes");
      return pairing.map((p) => ({ ...p, code: null, url: null, installSh: null, installPowershell: null }));
    },
    revokePairingCode: async (id) => {
      calls.push(`revoke-pairing-code:${id}`);
      for (const p of pairing) if (p.id === id) p.state = "revoked";
    },
    notify: async (title, body) => {
      calls.push(`notify:${title}:${body}`);
    },
  };
}
