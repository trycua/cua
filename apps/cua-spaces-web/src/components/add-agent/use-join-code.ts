// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * One join code while "Connect your own agent" is open (CUA-2264): made
 * with the sheet's settings, made again when they change (the unused one
 * is cancelled), and watched every 2 s for the agent joining or the code
 * expiring. A code that was never copied is cancelled when the sheet
 * closes; one that was copied stays valid until it is used or expires.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { useBridge, type JoinCode, type JoinCodeState } from "@/bridge";

export interface JoinSettings {
  ttlS: number;
  canTask: "owner" | "members";
  askBeforeJoin: boolean;
}

export type JoinWatch =
  | { kind: "making" }
  | { kind: "ready"; code: JoinCode }
  /** Copied: waiting for the agent to run it. */
  | { kind: "waiting"; code: JoinCode }
  /** The agent ran it (`pending_approval`: it waits to be let in, in the thread). */
  | { kind: "joined"; code: JoinCode; state: Extract<JoinCodeState, "redeemed" | "pending_approval">; handle: string | null }
  | { kind: "expired"; code: JoinCode }
  | { kind: "failed"; error: string };

export const JOIN_POLL_MS = 2_000;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function useJoinCode(spaceId: string | null, settings: JoinSettings, active: boolean) {
  const { data } = useBridge();
  const [watch, setWatch] = useState<JoinWatch>({ kind: "making" });
  const current = useRef<{ id: string; copied: boolean } | null>(null);
  const [round, setRound] = useState(0);

  const cancelUnused = useCallback(() => {
    const c = current.current;
    current.current = null;
    if (c && !c.copied && data && spaceId) void data.call("thread.joinCodeCancel", { spaceId, id: c.id }).catch(() => {});
  }, [data, spaceId]);

  // Make a code for these settings; a new one replaces the last (unused) one.
  useEffect(() => {
    if (!active || !data || !spaceId) return;
    let alive = true;
    cancelUnused();
    setWatch({ kind: "making" });
    data.call("thread.joinCodeCreate", { spaceId, ttlS: settings.ttlS, canTask: settings.canTask, askBeforeJoin: settings.askBeforeJoin }).then(
      (code) => {
        if (!alive) {
          void data.call("thread.joinCodeCancel", { spaceId, id: code.id }).catch(() => {});
          return;
        }
        current.current = { id: code.id, copied: false };
        setWatch({ kind: "ready", code });
      },
      (e: unknown) => alive && setWatch({ kind: "failed", error: message(e) }),
    );
    return () => {
      alive = false;
    };
  }, [active, data, spaceId, settings.ttlS, settings.canTask, settings.askBeforeJoin, round, cancelUnused]);

  // The sheet closed (or this unmounts): an uncopied code goes.
  useEffect(() => {
    if (!active) cancelUnused();
    return () => cancelUnused();
  }, [active, cancelUnused]);

  // Watch the code: joined, waiting to be let in, or expired.
  const code = watch.kind === "ready" || watch.kind === "waiting" ? watch.code : null;
  useEffect(() => {
    if (!active || !data || !spaceId || !code) return;
    let alive = true;
    const look = () =>
      void data.call("thread.joinCodeList", { spaceId }).then(
        ({ codes }) => {
          if (!alive) return;
          const row = codes.find((c) => c.id === code.id);
          const expired = row ? row.state === "expired" : Date.now() > code.expiresAtMs;
          if (row && (row.state === "redeemed" || row.state === "pending_approval")) {
            current.current = null;
            setWatch({ kind: "joined", code, state: row.state, handle: row.memberHandle });
          } else if (expired) {
            current.current = null;
            setWatch({ kind: "expired", code });
          }
        },
        () => {},
      );
    look();
    const t = setInterval(look, JOIN_POLL_MS);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [active, data, spaceId, code]);

  return {
    watch,
    /** The line went to the clipboard: the code stays when the sheet closes, and the sheet waits. */
    copied() {
      if (current.current) current.current.copied = true;
      setWatch((w) => (w.kind === "ready" ? { kind: "waiting", code: w.code } : w));
    },
    /** A new code (after one expired or failed). */
    renew() {
      setRound((r) => r + 1);
    },
    /** Cancel the current code (used or not) and stop. */
    cancel() {
      const c = current.current;
      current.current = null;
      if (c && data && spaceId) void data.call("thread.joinCodeCancel", { spaceId, id: c.id }).catch(() => {});
    },
  };
}
