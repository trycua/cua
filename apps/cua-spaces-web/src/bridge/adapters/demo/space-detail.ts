// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The demo host's Space detail: memory and storage use, windows, the
 * primary display and picture-in-picture panels, made up per OS. */

import { HostError } from "../../adapter";
import type { SpaceRow } from "../../contracts/spaces";
import type { RemoteWindow, SpaceUsage, SpaceWindows } from "../../ops/space-detail";
import type { DemoContext, DemoHandlers } from "./context";

export interface DemoSpaceDetailState {
  /** Open picture-in-picture rows per Space, in the order they opened. */
  pip: Map<string, string[]>;
}

export const demoSpaceDetailState = (): DemoSpaceDetailState => ({ pip: new Map() });

const GB = 1024 ** 3;

const running = (row: SpaceRow) => row.reachable && (row.powerState === undefined || row.powerState === "running");

function offline(row: SpaceRow): HostError {
  return new HostError(`${row.name} is not running`, "space_off");
}

/** A container shares the host's disk and has a cgroup memory limit; a VM has both its own. */
function usageOf(row: SpaceRow): SpaceUsage {
  const seed = [...row.id].reduce((n, c) => n + c.charCodeAt(0), 0);
  const frac = (k: number) => 0.25 + ((seed * k) % 50) / 100;
  if (row.kind === "container") {
    return { memoryUsed: Math.round(4 * GB * frac(7)), memoryTotal: 4 * GB, memoryLimited: true, diskUsed: 212 * GB, diskTotal: 460 * GB, diskLimited: false };
  }
  const memory = row.os === "macos" ? 8 : 6;
  const disk = row.os === "macos" ? 80 : 64;
  return {
    memoryUsed: Math.round(memory * GB * frac(11)),
    memoryTotal: memory * GB,
    memoryLimited: true,
    diskUsed: Math.round(disk * GB * frac(13)),
    diskTotal: disk * GB,
    diskLimited: true,
  };
}

const win = (id: string, appName: string, title: string, appId: string, pid: number): RemoteWindow => ({
  id,
  appName,
  title,
  visible: true,
  appId,
  targetEpoch: 1,
  pid,
});

const WINDOWS: Record<string, SpaceWindows> = {
  macos: {
    windows: [
      win("w-safari", "Safari", "Pull requests · trycua/cua", "com.apple.Safari", 511),
      win("w-terminal", "Terminal", "", "com.apple.Terminal", 602),
      win("w-finder", "Finder", "Downloads", "com.apple.finder", 388),
    ],
    display: { widthPx: 1920, heightPx: 1200 },
  },
  linux: {
    windows: [
      win("w-firefox", "Firefox", "Mozilla Firefox", "firefox", 812),
      win("w-xterm", "xterm", "", "xterm", 904),
      win("w-code", "Code", "build.rs - cua - Visual Studio Code", "code", 1120),
    ],
    display: { widthPx: 1280, heightPx: 800 },
  },
  windows: {
    windows: [win("w-edge", "Microsoft Edge", "New tab", "msedge", 4412), win("w-explorer", "File Explorer", "Documents", "explorer", 3020)],
    display: { widthPx: 1920, heightPx: 1080 },
  },
};

export function demoSpaceDetailHandlers({ state, findRow }: DemoContext): DemoHandlers<"spaces.usage" | "spaces.windows" | "stream.pip" | "spaces.thumbnail" | "spaces.chooseFiles" | "spaces.droppedFiles" | "spaces.sendFiles"> {
  return {
    "spaces.usage": ({ spaceId }) => {
      const row = findRow(spaceId);
      return running(row) ? usageOf(row) : null;
    },
    "spaces.windows": ({ spaceId }) => {
      const row = findRow(spaceId);
      if (!running(row)) throw offline(row);
      const w = WINDOWS[row.os ?? "linux"] ?? WINDOWS.linux!;
      return { windows: w.windows.map((x) => ({ ...x })), display: w.display ? { ...w.display } : null, open: [...(state.pip.get(spaceId) ?? [])] };
    },
    "stream.pip": ({ spaceId, command }) => {
      const row = findRow(spaceId);
      if (!running(row)) throw offline(row);
      const open = (state.pip.get(spaceId) ?? []).filter((r) => r !== command.row);
      if (command.type === "open") open.push(command.row);
      state.pip.set(spaceId, open);
      return [...open];
    },
    // Sample data has no screens to capture: the tiles draw their mark.
    "spaces.thumbnail": ({ spaceId }) => {
      findRow(spaceId);
      return null;
    },
    // Sample data has no files to pick; a drop's names stand in for paths.
    "spaces.chooseFiles": () => [],
    "spaces.droppedFiles": ({ names }) => names.map((n) => `/Users/demo/${n}`),
    "spaces.sendFiles": ({ spaceId, paths }) => {
      const row = findRow(spaceId);
      if (!running(row)) throw offline(row);
      return paths.map((p) => {
        const name = p.split("/").pop() || p;
        return { name, dest: `/home/cua/Downloads/${name}`, bytes: 1024 };
      });
    },
  };
}
