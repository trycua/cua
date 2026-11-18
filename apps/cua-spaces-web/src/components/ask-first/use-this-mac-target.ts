// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useContext, useEffect, useRef, useState } from "react";

import { useMachines, useSession } from "@/bridge";
import { BridgeContext } from "@/bridge/BridgeProvider";
import type { HostStatus } from "@/bridge/contracts/host";
import { toastError } from "@/components/ui/toast";
import { t } from "@/i18n";
import { thisComputerLabel } from "@/lib/host-labels";
import { thisMacReadiness, thisMacSetupSteps, type ThisMacReadiness } from "@/lib/this-mac-target";

export interface ThisMacTarget {
  readiness: ThisMacReadiness;
  /** Set up is running. */
  busy: boolean;
  /** Adds this computer to the account if needed and shares its desktop, so a job can start on it. */
  setUp: () => void;
}

/**
 * How this computer stands as a place for Home's jobs (`thisMacReadiness`), read from its host service
 * when Home opens, when the window comes back, and whenever the account's computers change, plus the
 * one Set up action the box's chip offers when it isn't ready.
 */
export function useThisMacTarget(): ThisMacTarget {
  const ctx = useContext(BridgeContext);
  const adapter = ctx?.adapter ?? null;
  const store = ctx?.store ?? null;
  const session = useSession();
  const signedIn = Boolean(session.data?.signedIn);
  const identity = session.data?.identity ?? null;
  const machines = useMachines();
  const [status, setStatus] = useState<HostStatus | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const live = useRef(true);

  const read = useCallback(async (): Promise<HostStatus | null> => {
    if (!adapter) return null;
    const s = await adapter.call("host.status", {}).catch(() => null);
    if (live.current) setStatus(s);
    return s;
  }, [adapter]);

  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  useEffect(() => {
    if (!signedIn) return;
    void read();
    const again = () => void read();
    window.addEventListener("focus", again);
    return () => window.removeEventListener("focus", again);
  }, [signedIn, read, machines.data]);

  const setUp = () => {
    if (!adapter || busy) return;
    setBusy(true);
    void (async () => {
      try {
        for (const step of thisMacSetupSteps(await read())) {
          if (step === "add") {
            if (!(await (store?.addThisComputer(identity) ?? Promise.resolve(false))))
              throw new Error(store?.thisMachine.get().setupError?.message ?? t("home.this_mac.add_failed", { computer: thisComputerLabel() }));
          } else await adapter.call("host.action", { action: step });
        }
      } catch (e) {
        toastError(t("home.this_mac.setup_failed", { computer: thisComputerLabel() }))(e);
      } finally {
        await read();
        void machines.refresh().catch(() => {});
        if (live.current) setBusy(false);
      }
    })();
  };

  return { readiness: thisMacReadiness(signedIn, status), busy, setUp };
}
