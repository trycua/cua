// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Cua Volume (`volume.*`): the daemon's `volume_*` tools, the mount, access
// requests and grants, sync conflicts, and Show in Finder / Explorer / the
// file manager (the SwiftUI host's WebUIBridge+Pages "Cua Volume"). Only a
// path in the user's home folder is shown.
import * as path from "node:path";
import { homeExpanded, osWord } from "../model/storage";
import { object, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { daemonTool, hostParts } from "./host-parts";

/** A path in the user's home folder (what Show in Finder may reveal), expanded and normalized. */
export function homePath(ctx: BridgeContext, raw: string): string {
  const home = path.resolve(hostParts(ctx).storage.answers.home);
  const full = path.resolve(homeExpanded(raw, home));
  const rel = path.relative(home, full);
  const inside = rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel));
  if (!inside) throw Failure.badArgs("path: in your home folder");
  return full;
}

/** A system settings pane the Storage section may open: macOS's (`x-apple.systempreferences:`), Windows' (`ms-settings:`). */
export function settingsUrl(url: string, platform: NodeJS.Platform): string {
  const scheme = platform === "darwin" ? "x-apple.systempreferences:" : platform === "win32" ? "ms-settings:" : null;
  if (!scheme || !url.startsWith(scheme)) throw Failure.badArgs("Only System Settings panes open here");
  return url;
}

const list = (v: unknown, key: string): unknown[] => {
  const x = v && typeof v === "object" ? (v as Record<string, unknown>)[key] : null;
  return Array.isArray(x) ? x : [];
};
const obj = (v: unknown) => (v && typeof v === "object" && !Array.isArray(v) ? v : null);

export function volumeMethods(ctx: BridgeContext): Handlers {
  const tool = (what = "This") => daemonTool(ctx, what);
  const none = (name: string, key: string, arg: string) => async (args: Record<string, unknown>) => {
    await tool()(name, { [arg]: string(args, key) });
    return null;
  };
  return {
    "volume.overview": async () => {
      const t = tool("Cua Volume");
      const quiet = (name: string) => t(name).catch(() => null);
      const [requests, grants, mount, sync] = await Promise.all([
        quiet("volume_requests"),
        quiet("volume_grants"),
        quiet("volume_mount_status"),
        quiet("volume_sync_status"),
      ]);
      return {
        os: osWord(ctx.platform),
        home: hostParts(ctx).storage.answers.home,
        requests: list(requests, "requests"),
        grants: list(grants, "grants"),
        mount: obj(mount),
        sync: obj(sync),
      };
    },
    // Never fails: no daemon, or no answer, is no store yet.
    "volume.storage": async () => {
      try {
        return (await tool()("volume_storage")) ?? null;
      } catch {
        return null;
      }
    },
    "volume.storageSet": (args) => tool()("volume_storage_set", object(args, "update")),
    "volume.mount": () => tool()("volume_mount"),
    "volume.unmount": () => tool()("volume_unmount"),
    "volume.approve": none("volume_approve", "id", "request_id"),
    "volume.deny": none("volume_deny", "id", "request_id"),
    "volume.revoke": none("volume_revoke", "id", "grant_id"),
    "volume.resolve": none("volume_sync_resolve", "path", "path"),
    "volume.reveal": (args) => {
      const system = hostParts(ctx).system;
      const full = homePath(ctx, string(args, "path"));
      if (!system) throw Failure.unsupported("Showing files is not available in this build");
      system.reveal(full);
      return null;
    },
  };
}
