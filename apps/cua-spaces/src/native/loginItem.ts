// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app as a login item, through the shell (`src-tauri/src/login_item.rs`:
 * an XDG autostart entry on Linux, the Run key on Windows, a LaunchAgent on
 * macOS). Outside the shell nothing is registered: `status` rejects and
 * Settings shows no toggle.
 */
import type { LoginItemStatus } from "../model/loginItem";
import { hasTauri } from "./bridge";

export interface LoginItemBridge {
  isNative: boolean;
  /** What the system holds now. */
  status(): Promise<LoginItemStatus>;
  /** Registers (`on`) or unregisters the app; what the system holds afterwards. */
  set(on: boolean): Promise<LoginItemStatus>;
}

export function createLoginItemBridge(): LoginItemBridge {
  if (hasTauri()) {
    const invoke = (cmd: string, args?: Record<string, unknown>) =>
      import("@tauri-apps/api/core").then(({ invoke }) => invoke<LoginItemStatus>(cmd, args));
    return {
      isNative: true,
      status: () => invoke("login_item_status"),
      set: (on) => invoke("login_item_set", { on }),
    };
  }
  const outside = () => Promise.reject(new Error("Launch at login needs the Cua Spaces app"));
  return { isNative: false, status: outside, set: outside };
}

/** An in-memory login item (tests): records each `set`. */
export function fakeLoginItemBridge(
  initial: LoginItemStatus = "notRegistered",
  options: { fail?: string } = {},
): LoginItemBridge & { calls: boolean[]; current: LoginItemStatus } {
  const fake = {
    isNative: true,
    calls: [] as boolean[],
    current: initial,
    status: async () => fake.current,
    set: async (on: boolean) => {
      fake.calls.push(on);
      if (options.fail) throw new Error(options.fail);
      fake.current = on ? "enabled" : "notRegistered";
      return fake.current;
    },
  };
  return fake;
}
