// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space's network, Volume and clipboard settings from the host
 * (`network.get`, `network.set`, `network.log`, `network.ackNotice`,
 * `spaces.setAccess`), behind the same store the Space page reads in demo
 * mode ([`NetworkPreview`]). A host that does not answer these operations
 * leaves `get` null, and none of it shows.
 *
 * Reads are cached per Space and refreshed when asked again after a while;
 * the blocked connections are read while a Space page asks for them, so a
 * connection the Space tried a moment ago shows as a notice there.
 */

import { isUnsupported, type DataAdapter } from "./adapter";
import type {
  BlockedConnection,
  ClipboardMode,
  NetworkPreset,
  NetworkPreview,
  SpaceNetwork,
  SpaceSecurity,
  SpaceTopology,
  VolumeScope,
} from "./contracts/network";
import type { NetworkEventWire, SpaceNetworkWire } from "./protocol";

/** How long a read stays fresh. */
const FRESH_MS = 10_000;

/** What a reason id means, in plain words (as `cua network log` says it). */
export function reasonWords(reason: string): string {
  switch (reason) {
    case "platform:metadata":
      return "Cloud metadata";
    case "platform:daemon":
      return "Cua on this computer";
    case "platform:other-tree":
      return "A Space in another tree";
    case "tree:tcp-only":
      return "Its tree, over UDP (VM Spaces talk over TCP only)";
    case "platform:loopback":
    case "default:host-loopback":
      return "This computer's localhost";
    case "default:host":
    case "default:host-address":
      return "This computer";
    case "default:lan":
      return "Local network";
    case "default:none":
      return "No network";
    case "user:deny":
      return "Your block list";
    case "dns_stripped":
      return "A name that pointed to a private address";
    default:
      return "a destination its settings don't allow";
  }
}

/**
 * A block the person can do nothing about: the guest's own name-service broadcast (NetBIOS, UDP 137 and 138
 * to the local network's `.255`) that macOS sends a few seconds after it boots. The log keeps it; the
 * notice under the header does not, or every new macOS Space opens with a warning nobody asked for.
 */
export function isBackgroundNoise(address: string): boolean {
  return /\.255:13[78]$/.test(address);
}

/** The page's network settings from the host's answer. */
export function networkFromWire(w: SpaceNetworkWire, parentName?: string): SpaceNetwork {
  const preset = (["internet", "none", "localhost", "lan", "localhost+lan", "custom"] as const).includes(w.preset as NetworkPreset)
    ? (w.preset as NetworkPreset)
    : "internet";
  return {
    preset,
    allow: w.allow ?? [],
    deny: w.deny ?? [],
    base: w.customBase === "none" ? "none" : "internet",
    localhostPorts: w.allLocalhostPorts ? [] : (w.localhostPorts ?? []),
    inheritedFrom: w.parent ? (parentName ?? nameOf(w.parent)) : undefined,
    enforced: w.enforcement === "enforced",
    via: w.via && w.via !== "internet" ? w.via : undefined,
  };
}

/** The host's topology JSON (`topologyJson`), or undefined. */
export function topologyFromJson(text: string | undefined): SpaceTopology | undefined {
  if (!text) return undefined;
  let t: Record<string, unknown>;
  try {
    t = JSON.parse(text) as Record<string, unknown>;
  } catch {
    return undefined;
  }
  const obj = (v: unknown) => (v && typeof v === "object" ? (v as Record<string, unknown>) : {});
  const str = (v: unknown) => (typeof v === "string" ? v : undefined);
  const arr = (v: unknown) => (Array.isArray(v) ? (v as unknown[]) : []);
  const via = obj(t.via);
  const exit = obj(t.exit);
  const label = str(via.label);
  return {
    name: str(obj(t.space).name),
    exit: label ? { label, state: str(exit.state) ?? "starting", note: str(exit.note) } : undefined,
    hostReach: arr(obj(t.host).reach).filter((r): r is string => typeof r === "string"),
    hostOnly: arr(obj(t.host_ports).at_host_only).flatMap((r) => {
      const [port, why] = arr(r);
      return typeof port === "number" && typeof why === "string" ? [{ port, why }] : [];
    }),
    members: arr(obj(t.tree).members).flatMap((m) => {
      const o = obj(m);
      const id = str(o.id);
      const name = str(o.name);
      // A member that runs on another machine and isn't one of this host's
      // Spaces (`machine` is set) can't be an exit choice.
      return id && name && !str(o.machine) ? [{ id, name, relation: str(o.relation) ?? "member" }] : [];
    }),
    remote: arr(obj(t.tree).members).flatMap((m) => {
      const o = obj(m);
      const name = str(o.name);
      const machine = str(o.machine);
      return name && machine ? [{ name, machine }] : [];
    }),
    paths: arr(obj(t.paths).machines).flatMap((m) => {
      const o = obj(m);
      const name = str(o.name);
      const why = str(o.why);
      return name && why ? [{ name, kind: str(o.kind) ?? "relay", why }] : [];
    }),
  };
}

/** The `network` word `network.set` takes for `n`. */
export function networkWord(n: SpaceNetwork): string {
  switch (n.preset) {
    case "localhost":
    case "localhost+lan":
      return n.localhostPorts && n.localhostPorts.length > 0 ? `${n.preset}:${n.localhostPorts.join(",")}` : `${n.preset}:all`;
    case "custom":
      return n.base === "none" ? "custom:none" : "custom";
    default:
      return n.preset;
  }
}

export function blockedFromWire(e: NetworkEventWire): BlockedConnection {
  return {
    address: e.dst,
    what: reasonWords(e.reason),
    at: e.lastUnix ? e.lastUnix * 1000 : 0,
    always: e.reason.startsWith("platform:"),
  };
}

const nameOf = (id: string) =>
  id
    .slice(id.lastIndexOf(":") + 1)
    .split("/")
    .pop() ?? id;

interface Entry {
  security: SpaceSecurity | null;
  blocked: BlockedConnection[];
  readAt: number;
  logAt: number;
  loading: boolean;
}

/** The store over `adapter`'s operations. */
export function createNetworkStore(adapter: DataAdapter, now: () => number = Date.now): NetworkPreview {
  const listeners = new Set<() => void>();
  const entries = new Map<string, Entry>();
  const dismissed = new Map<string, number>();
  const errors = new Map<string, string>();
  const openedAt = now();
  let unsupported = false;
  let version = 0;
  let wizardPreset: NetworkPreset = "internet";
  const changed = () => {
    version += 1;
    for (const l of listeners) l();
  };
  const entry = (id: string): Entry => {
    let e = entries.get(id);
    if (!e) {
      e = { security: null, blocked: [], readAt: 0, logAt: 0, loading: false };
      entries.set(id, e);
    }
    return e;
  };
  const failed = (e: unknown) => {
    if (isUnsupported(e)) unsupported = true;
  };

  /** This computer's own tile and a Space still being made have no network to read. */
  const readable = (id: string) => id !== "this-mac" && !id.startsWith("pending:");

  const refresh = (id: string) => {
    const e = entry(id);
    if (unsupported || e.loading || !readable(id)) return;
    e.loading = true;
    void (async () => {
      try {
        const w = await adapter.call("network.get", { spaceId: id });
        e.security = {
          network: networkFromWire(w),
          volume: (w.volume === "off" ? "off" : "own-folder") as VolumeScope,
          clipboard: (["viewing", "ask", "off"].includes(w.clipboard) ? w.clipboard : "viewing") as ClipboardMode,
          parent: w.parent ? { id: w.parent, name: nameOf(w.parent) } : undefined,
          children: [],
          noticePending: w.noticePending,
          summary: w.summary,
          point: w.point,
          note: w.note ?? undefined,
          topology: topologyFromJson(w.topologyJson),
        };
        const events = await adapter.call("network.log", { spaceId: id }).catch(() => [] as NetworkEventWire[]);
        e.blocked = events.map(blockedFromWire);
        e.logAt = now();
      } catch (err) {
        failed(err);
      } finally {
        e.readAt = now();
        e.loading = false;
        changed();
      }
    })();
  };

  const fresh = (id: string) => {
    const e = entry(id);
    if (now() - e.readAt > FRESH_MS) refresh(id);
    return e;
  };

  return {
    get: (spaceId) => (unsupported ? null : fresh(spaceId).security),
    set(spaceId, network, opts) {
      void (async () => {
        try {
          await adapter.call("network.set", {
            spaceId,
            network: networkWord(network),
            allow: network.preset === "custom" ? network.allow : [],
            deny: network.preset === "custom" ? network.deny : [],
            // Only when the user chose to let a child reach more than its parent.
            widen: opts?.widen === true,
            via: network.via ?? "internet",
          });
          errors.delete(spaceId);
        } catch (err) {
          failed(err);
          errors.set(spaceId, err instanceof Error ? err.message : String(err));
        } finally {
          entry(spaceId).readAt = 0;
          refresh(spaceId);
        }
      })();
    },
    error: (spaceId) => errors.get(spaceId) ?? null,
    blocked: (spaceId) => fresh(spaceId).blocked,
    notice(spaceId) {
      const newest = fresh(spaceId).blocked.find((b) => b.at > Math.max(openedAt - 60_000, dismissed.get(spaceId) ?? 0) && !isBackgroundNoise(b.address));
      return newest ?? null;
    },
    dismissNotice(spaceId) {
      dismissed.set(spaceId, now());
      changed();
    },
    // Volume requests are answered on the Volume page; the clipboard is the
    // viewer's to ask about (it sees what the Space copied).
    volumeRequest: () => null,
    answerVolume: () => {},
    clipboardOffer: () => null,
    answerClipboard: () => {},
    setAccess(spaceId, access) {
      void adapter
        .call("spaces.setAccess", {
          spaceId,
          ...(access.clipboard ? { clipboard: access.clipboard } : {}),
          ...(access.volume ? { volume: access.volume === "off" ? "off" : "own" } : {}),
        })
        .catch(failed)
        .finally(() => {
          entry(spaceId).readAt = 0;
          refresh(spaceId);
        });
    },
    ackNotice(spaceId) {
      const e = entry(spaceId);
      if (e.security) e.security = { ...e.security, noticePending: false };
      changed();
      void adapter.call("network.ackNotice", { spaceId }).catch(failed);
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
