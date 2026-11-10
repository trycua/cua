// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * PREVIEW (demo only, `?demo=network`): per-Space network, Volume and
 * clipboard settings in memory, and a small tree of Spaces (research-swarm
 * with two workers) to show inherited settings. `net-blocked`,
 * `net-volume` and `net-clipboard` add a blocked connection, a Volume
 * request and a clipboard offer on swarm-worker-1. `net-exit` sends the
 * tree's internet traffic out through This computer (`net-exit-down`: through
 * swarm-worker-2, which is unreachable), with names, this computer's ports and
 * the paths to two other computers.
 */

import type {
  BlockedConnection,
  ClipboardOffer,
  NetworkPreset,
  NetworkPreview,
  SpaceNetwork,
  SpaceSecurity,
  SpaceTopology,
  VolumeRequest,
} from "../../contracts/network";
import type { SpaceRow } from "../../contracts/spaces";

const STREAMS = ["desktop_stream", "window_stream", "audio.desktop", "relay_attach"];

const at = (now: number, minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString();

const linux = (id: string, name: string, now: number, minutesAgo: number): SpaceRow => ({
  id,
  name,
  provider: "local",
  spacesdVersion: "0.6.0",
  features: STREAMS,
  addedAt: at(now, minutesAgo),
  os: "linux",
  osName: "Ubuntu",
  osPrettyName: "Ubuntu 24.04.3 LTS",
  image: "ubuntu-xfce",
  kind: "container",
  arch: "arm64",
  reachable: true,
  power: "stop",
  powerState: "running",
});

/** research-swarm and its two workers, on This computer. */
export function demoNetworkSpaceRows(now: number): SpaceRow[] {
  return [
    linux("local:research-swarm", "research-swarm", now, 3),
    linux("local:swarm-worker-1", "swarm-worker-1", now, 2),
    linux("local:swarm-worker-2", "swarm-worker-2", now, 2),
  ];
}

/** The agent's Home Space and the two Spaces it made for "test acme/app" (`?demo=agent-home`, CUA-2630). */
export function demoAgentHomeRows(now: number): SpaceRow[] {
  const mac: SpaceRow = {
    ...linux("local:acme-app-build", "acme-app-build", now, 2),
    os: "macos",
    osName: "macOS",
    osPrettyName: "macOS Tahoe 26.0",
    image: "macos-tahoe",
    kind: "vm",
    power: "suspend",
  };
  return [{ ...linux("local:agent-home-demo", "Home", now, 6), image: "linux-slim" }, linux("local:acme-app-db", "acme-app-db", now, 3), mac];
}

const internet = (extra: Partial<SpaceNetwork> = {}): SpaceNetwork => ({ preset: "internet", allow: [], deny: [], enforced: true, ...extra });

const MEMBERS = ["research-swarm", "swarm-worker-1", "swarm-worker-2"];

/** A demo Space's topology: its name, its exit, this computer's ports and the paths. */
function demoTopology(self: string, via: string | undefined, down: boolean, localhost: boolean, remote = false): SpaceTopology {
  const exitLabel = via === "this-machine" ? "This computer" : via?.startsWith("space:") ? via.slice(via.lastIndexOf(":") + 1) : undefined;
  return {
    name: `${self}.research-swarm`,
    exit: exitLabel ? { label: exitLabel, state: down ? "unreachable" : "active", note: down ? "It's stopped." : undefined } : undefined,
    hostReach: localhost ? ["localhost:3000", "localhost:5432"] : [],
    hostOnly: localhost ? [{ port: 8080, why: "in use" }] : [],
    members: MEMBERS.map((m) => ({
      id: `local:${m}`,
      name: `${m}.research-swarm`,
      relation: m === self ? "self" : m === "research-swarm" ? "parent" : "member",
    })),
    remote: remote
      ? [
          { name: "db.research-swarm", machine: "Studio computer" },
          { name: "gpu-worker.research-swarm", machine: "GPU box" },
        ]
      : undefined,
    paths: [
      { name: "studio-mac", kind: "lan", why: "Fastest path: the local network, 2 ms" },
      { name: "gpu-box", kind: "relay", why: "No direct path answered yet; the relay carries it (end-to-end encrypted)" },
    ],
  };
}

export function createDemoNetwork({ now, flags }: { now: () => number; flags: Set<string> }): NetworkPreview {
  const listeners = new Set<() => void>();
  let version = 0;
  const changed = () => {
    version += 1;
    for (const l of listeners) l();
  };
  const parent = { id: "local:research-swarm", name: "research-swarm" };
  const down = flags.has("net-exit-down");
  const via = down ? "space:local:swarm-worker-2" : flags.has("net-exit") ? "this-machine" : undefined;
  const remote = flags.has("net-remote");
  const topo = (self: string, v: string | undefined, localhost = false) => demoTopology(self, v, down, localhost, remote);
  const security = new Map<string, SpaceSecurity>([
    [
      "local:research-swarm",
      {
        network: internet({ via: down ? undefined : via }),
        volume: "own-folder",
        clipboard: "viewing",
        children: ["swarm-worker-1", "swarm-worker-2"],
        topology: topo("research-swarm", down ? undefined : via),
      },
    ],
    [
      "local:swarm-worker-1",
      {
        network: via
          ? { preset: "localhost", allow: [], deny: [], enforced: true, localhostPorts: [3000, 5432], inheritedFrom: "research-swarm", via }
          : internet({ inheritedFrom: "research-swarm" }),
        volume: "own-folder",
        clipboard: "viewing",
        parent,
        children: [],
        topology: topo("swarm-worker-1", via, Boolean(via)),
      },
    ],
    [
      "local:swarm-worker-2",
      {
        network: internet({ inheritedFrom: "research-swarm", via: down ? undefined : via }),
        volume: "own-folder",
        clipboard: "viewing",
        parent,
        children: [],
        topology: topo("swarm-worker-2", down ? undefined : via),
      },
    ],
  ]);
  const minutes = (n: number) => now() - n * 60_000;
  const blocked = new Map<string, BlockedConnection[]>([
    [
      "local:swarm-worker-1",
      [
        { address: "127.0.0.1:5432", what: "This computer's localhost", at: minutes(2), always: false },
        { address: "169.254.169.254", what: "Cloud metadata", at: minutes(6), always: true },
        { address: "192.168.1.20:445", what: "Local network", at: minutes(14), always: false },
      ],
    ],
  ]);
  const notices = new Set<string>(flags.has("net-blocked") ? ["local:swarm-worker-1"] : []);
  let volume: VolumeRequest | null = flags.has("net-volume") ? { id: "vr-1", spaceId: "local:swarm-worker-1", path: "Projects/site", who: "An agent" } : null;
  let clipboard: ClipboardOffer | null = flags.has("net-clipboard") ? { spaceId: "local:swarm-worker-1", chars: 214 } : null;
  let wizardPreset: NetworkPreset = "internet";

  return {
    get: (spaceId) => security.get(spaceId) ?? null,
    set(spaceId, network) {
      const s = security.get(spaceId);
      if (!s) return;
      const self = spaceNameOf(spaceId);
      security.set(spaceId, {
        ...s,
        network: { ...network, inheritedFrom: undefined },
        topology: demoTopology(self, network.via, false, network.preset === "localhost" || network.preset === "localhost+lan", remote),
      });
      // Children follow a parent's change (its exit too).
      for (const [id, child] of security) {
        if (child.parent?.id === spaceId) {
          security.set(id, {
            ...child,
            network: { ...network, inheritedFrom: s.parent ? undefined : self },
            topology: demoTopology(spaceNameOf(id), network.via, false, network.preset === "localhost" || network.preset === "localhost+lan", remote),
          });
        }
      }
      changed();
    },
    blocked: (spaceId) => blocked.get(spaceId) ?? [],
    notice: (spaceId) => (notices.has(spaceId) ? (blocked.get(spaceId)?.[0] ?? null) : null),
    dismissNotice(spaceId) {
      notices.delete(spaceId);
      changed();
    },
    volumeRequest: (spaceId) => (volume?.spaceId === spaceId ? volume : null),
    answerVolume(id) {
      if (volume?.id === id) volume = null;
      changed();
    },
    clipboardOffer: (spaceId) => (clipboard?.spaceId === spaceId ? clipboard : null),
    answerClipboard(spaceId) {
      if (clipboard?.spaceId === spaceId) clipboard = null;
      changed();
    },
    get wizardPreset() {
      return wizardPreset;
    },
    setWizardPreset(preset) {
      wizardPreset = preset;
      changed();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    version: () => version,
  };
}

const spaceNameOf = (id: string) => id.slice(id.indexOf(":") + 1);
