// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Links to things you run (`links.*`, `ops/links.ts`): the list for Settings
 * and `createLink` for "Copy link". A host without links (Tauri) reads as
 * `unsupported`, and the page shows nothing.
 */

import { useCallback, useContext, useEffect, useState } from "react";
import { isUnsupported } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { LinkCreate, ShareLink } from "./ops/links";

export type { LinkAccess, LinkCreate, ShareLink } from "./ops/links";

export interface LinksHook {
  links: ShareLink[];
  unsupported: boolean;
  error: string | null;
  refresh(): Promise<void>;
  /** Shares `target` and returns the link. Rejects with the host's words. */
  createLink(req: LinkCreate): Promise<ShareLink>;
  stopLink(id: string): Promise<void>;
  stopAll(): Promise<void>;
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function useLinks(options: { load?: boolean } = {}): LinksHook {
  const load = options.load ?? true;
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const data = ctx.adapter;
  const [links, setLinks] = useState<ShareLink[]>([]);
  const [unsupported, setUnsupported] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!data) return;
    try {
      const r = await data.call("links.list", {});
      setLinks(r.links);
      setError(null);
    } catch (e) {
      if (isUnsupported(e)) setUnsupported(true);
      else setError(message(e));
    }
  }, [data]);

  useEffect(() => {
    if (!load) return;
    void refresh();
    // Links end on their own (a TTL): look again now and then.
    const t = setInterval(() => void refresh(), 30_000);
    return () => clearInterval(t);
  }, [load, refresh]);

  return {
    links,
    unsupported,
    error,
    refresh,
    async createLink(req) {
      if (!data) throw new Error("the host is not ready yet");
      const link = await data.call("links.create", req);
      if (load) void refresh();
      return link;
    },
    async stopLink(id) {
      await data?.call("links.stop", { id });
      await refresh();
    },
    async stopAll() {
      await data?.call("links.stop", { id: "all" });
      await refresh();
    },
  };
}

/** "ends in 59 min", "never". */
export function linkEnds(link: ShareLink, nowMs = Date.now()): string {
  if (link.expiresAtUnix === null) return "Until stopped";
  const s = Math.max(0, link.expiresAtUnix - Math.floor(nowMs / 1000));
  if (s < 90) return `${s} s`;
  if (s < 5400) return `${Math.round(s / 60)} min`;
  return `${Math.round(s / 3600)} h`;
}

/** Whether the host's words say the machine is not on the relay (so the fix is This machine's setup). */
export function needsRelaySetup(msg: string): boolean {
  return /not on your cua\.ai relay|cua host setup|not signed in to cua\.ai/i.test(msg);
}
