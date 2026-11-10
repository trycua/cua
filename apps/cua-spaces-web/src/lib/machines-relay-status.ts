// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Machine, MachinesView } from "@/bridge";
import { relayCallsOffline } from "@/lib/spaces-tree";

/**
 * The Machines page's view with the relay's own online answer applied. The host's view counts a relay connection that
 * hasn't been dropped yet, so a computer that powered off stays "Online" for minutes and "Remove from account" stays
 * disabled. A computer the relay calls offline shows offline, and (it being another machine) can be removed.
 */
export function withRelayStatus(view: MachinesView, machines: readonly Machine[], relayOnline: ReadonlyMap<string, boolean> | undefined): MachinesView {
  if (!relayOnline || relayOnline.size === 0) return view;
  const offline = new Set(machines.filter((m) => relayCallsOffline(m, relayOnline)).map((m) => m.id));
  if (!view.machines.some((m) => m.online && offline.has(m.id))) return view;
  return {
    ...view,
    machines: view.machines.map((m) =>
      m.online && offline.has(m.id)
        ? {
            ...m,
            online: false,
            status: "Offline",
            remove: m.remove && !m.remove.host && m.remove.machine ? { ...m.remove, enabled: true, help: null } : m.remove,
          }
        : m,
    ),
  };
}
