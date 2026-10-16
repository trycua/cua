// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Join with a code": sets this computer up with a one-time pairing code
 * (or the add-machine link) made on another device, the in-app way of
 * `cua host setup --pair <code>`. The host runs the bundled `cua`
 * (`host.joinWithCode`) and answers once the machine has joined.
 */

import { useCallback, useState } from "react";

import { isUnsupported } from "./adapter";
import { useBridge } from "./hooks";
import { plainJoinError } from "./friendly-errors";
import { normalizePairingCode, pairingLinkRelay } from "./pairing-code";

export type JoinPhase = "idle" | "joining" | "joined" | "failed";

export interface JoinHook {
  phase: JoinPhase;
  /** Why it failed, in words for people. */
  error: string | null;
  /** Joins with `input` (a code or an add-machine link); `relay` overrides the link's own. Resolves to whether it worked. */
  join(input: string, relay?: string | null): Promise<boolean>;
  reset(): void;
}

/** What the page says for a failure: the host's own words, or a plain line. */
export function joinFailureText(e: unknown): string {
  if (isUnsupported(e)) return "This version of Cua Spaces can't join with a code. Update the app, or run the install command on this computer.";
  return plainJoinError(e);
}

export function useJoinWithCode({ onJoined }: { onJoined?: () => void } = {}): JoinHook {
  const { data: adapter } = useBridge();
  const [phase, setPhase] = useState<JoinPhase>("idle");
  const [error, setError] = useState<string | null>(null);

  const join = useCallback(
    async (input: string, relay?: string | null) => {
      const code = normalizePairingCode(input);
      if (!code) {
        setPhase("failed");
        setError("That doesn't look like a code. It has 12 letters and digits, like K7QM-4XPB-2RHD.");
        return false;
      }
      if (!adapter) return false;
      setPhase("joining");
      setError(null);
      try {
        await adapter.call("host.joinWithCode", { code, relay: relay ?? pairingLinkRelay(input) });
        setPhase("joined");
        onJoined?.();
        return true;
      } catch (e) {
        setPhase("failed");
        setError(joinFailureText(e));
        return false;
      }
    },
    [adapter, onJoined],
  );

  return { phase, error, join, reset: useCallback(() => (setPhase("idle"), setError(null)), []) };
}
