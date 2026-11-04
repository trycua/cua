// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The agents on This Mac and whether each is really connected: the one
 * status the app, `cua connect status` and the `connect_status` MCP tool
 * share (`agents.status`, CONTRACT §4). The core turns each row into its
 * words and one button (`agents.connectionRows`); this hook reads the
 * status when it opens and again when the window gets focus (the host
 * caches it for a few seconds), and runs the buttons: Connect and
 * Reconnect (`agents.connect`, tools and the sign-in copy), Sign in
 * (`agents.signIn`, the agent's own) and Disconnect (`agents.disconnect`,
 * both undone).
 */

import { useCallback, useContext, useEffect, useRef, useState } from "react";

import { isUnsupported } from "./adapter";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import type { ConnectionReport, ConnectionRow, ConnectionRowView } from "./contracts/agents";

export type { ConnectionReport, ConnectionRow, ConnectionRowView, ConnectResult } from "./contracts/agents";

/** The core's rows; the same rules when this build's core lacks them. */
export function connectionRows(core: CoreClient | null, rows: ConnectionRow[]): ConnectionRowView[] {
  return core?.tryCall<ConnectionRowView[]>("agents.connectionRows", { rows }) ?? fallbackRows(rows);
}

function fallbackRows(rows: ConnectionRow[]): ConnectionRowView[] {
  const act = (a: string | undefined) =>
    a === "reconnect"
      ? { id: "reconnect", label: "Reconnect" }
      : a === "sign_in"
        ? { id: "sign-in", label: "Sign in" }
        : a === "disconnect"
          ? { id: "disconnect", label: "Disconnect" }
          : a === "install"
            ? { id: "install", label: "Install" }
            : null;
  return rows
    .filter((r) => r.state !== "not_installed")
    .map((r) => {
      const detail = r.version ? (r.previous_version && r.previous_version !== r.version ? `${r.version}, updated from ${r.previous_version}` : r.version) : null;
      const signIn = r.signin_import === "ok" && r.sign_in ? `${r.sign_in} in Keyvault` : null;
      if (r.state === "connected")
        return { id: r.id, name: r.name, state: "connected", label: "Connected", problem: false, detail, signIn, action: act(r.problem?.action) };
      if (r.state === "attention")
        return { id: r.id, name: r.name, state: "attention", label: r.problem?.line ?? "Something needs you.", problem: true, detail, signIn, action: act(r.problem?.action) };
      return { id: r.id, name: r.name, state: "not_connected", label: "Not connected", problem: false, detail, signIn, action: { id: "connect", label: "Connect" } };
    });
}

export interface ConnectionsHook {
  /** The status as the host answered (none until read). */
  report: ConnectionReport | null;
  /** The rows worth drawing, in the status's order. */
  rows: ConnectionRowView[];
  /** This host can't tell (an older app, the Tauri shell). */
  unsupported: boolean;
  error: string | null;
  /** The agent whose button is running. */
  busy: string | null;
  /** What the last Connect said for an agent (a refused sign-in copy). */
  notes: Record<string, string>;
  /** A row's button: `connect`, `reconnect`, `sign-in`, `disconnect`. */
  act(agent: string, action: string): Promise<void>;
  refresh(): Promise<void>;
}

/** The connection rows, read now and on window focus while `active`. */
export function useConnections(active = true): ConnectionsHook {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  const { core, adapter } = ctx;
  const [report, setReport] = useState<ConnectionReport | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notes, setNotes] = useState<Record<string, string>>({});
  const live = useRef(true);

  const read = useCallback(
    async (refresh: boolean) => {
      if (!adapter) return;
      try {
        const r = await adapter.call("agents.status", { refresh });
        if (live.current) {
          setReport(r);
          setError(null);
        }
      } catch (e) {
        if (!live.current) return;
        if (isUnsupported(e)) setUnsupported(true);
        else setError(e instanceof Error ? e.message : String(e));
      }
    },
    [adapter],
  );

  useEffect(() => {
    live.current = true;
    if (!active) return;
    void read(false);
    const focus = () => void read(true);
    window.addEventListener("focus", focus);
    return () => {
      live.current = false;
      window.removeEventListener("focus", focus);
    };
  }, [active, read]);

  const act = async (agent: string, action: string) => {
    if (!adapter || busy) return;
    setBusy(agent);
    try {
      if (action === "connect" || action === "reconnect") {
        const [r] = await adapter.call("agents.connect", { agents: [agent], signIn: true });
        const line = r?.tools_line ?? r?.sign_in?.line ?? null;
        setNotes((n) => {
          const next = { ...n };
          if (line) next[agent] = line;
          else delete next[agent];
          return next;
        });
      } else if (action === "sign-in") {
        await adapter.call("agents.signIn", { agent });
      } else if (action === "disconnect") {
        await adapter.call("agents.disconnect", { agents: [agent], signIn: true });
      }
    } catch (e) {
      setNotes((n) => ({ ...n, [agent]: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(null);
      await read(true);
    }
  };

  return {
    report,
    rows: report ? connectionRows(core, report.agents) : [],
    unsupported,
    error,
    busy,
    notes,
    act,
    refresh: () => read(true),
  };
}
