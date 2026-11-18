// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

import { config } from "../auth/config";
import { claimsOf, session } from "../auth/session";
import { createDemoBackend } from "../data/demo/backend";
import { createLiveBackend } from "../data/live/backend";
import { DeviceManager, type EnrollState } from "../data/live/enroll";
import { RelayClient } from "../data/live/relay";
import type { Backend } from "../data/types";
import { createSealedStore } from "../store/sealed";
import { BackendProvider, clearThreadMemory } from "./backend";
import { StoresProvider } from "./outbox";
import { usePrefs } from "./prefs";

export type Stage = "booting" | "signed-out" | "enrolling" | "ready";

interface AppState {
  stage: Stage;
  backend: Backend | null;
  enroll: EnrollState | null;
  account: { email: string | null; name: string | null } | null;
  /** This phone's relay device id (`dev_...`), for push registration. */
  deviceId(): string | null;
  signIn(): Promise<void>;
  signOut(): Promise<void>;
  recheckEnrollment(): Promise<void>;
  /** Why the last sign-in did not finish, in words. */
  signInError: string | null;
  signingIn: boolean;
}

const Ctx = createContext<AppState | null>(null);
export const useApp = () => {
  const c = useContext(Ctx);
  if (!c) throw new Error("useApp outside AppProvider");
  return c;
};

export function AppProvider({ children }: { children: ReactNode }) {
  const ready = usePrefs((p) => p.ready);
  const sampleOffline = usePrefs((p) => p.sampleOffline);
  const sample = config.sample;
  const [signedIn, setSignedIn] = useState<boolean | null>(null);
  const [enroll, setEnroll] = useState<EnrollState | null>(null);
  const [signInError, setSignInError] = useState<string | null>(null);
  const [signingIn, setSigningIn] = useState(false);

  useEffect(() => {
    usePrefs.getState().hydrate();
    session.load().then(setSignedIn);
    return session.subscribe(setSignedIn);
  }, []);

  const relay = useMemo(() => new RelayClient(config.relayUrl), []);
  const device = useMemo(() => new DeviceManager(relay, () => session.accessToken()), [relay]);

  // A new Space for a sample task takes a moment, so the "starting" screen is seen.
  const demo = useMemo(() => createDemoBackend({ createMs: 5000 }), []);
  // The id the relay knows this phone by; a sample build has a made up one (it never reaches a relay).
  const deviceId = useCallback(() => (sample ? "dev_sample0000000000000000" : device.id()), [sample, device]);
  const me = useCallback(() => (typeof claimsOf(accessCache.current).sub === "string" ? (claimsOf(accessCache.current).sub as string) : null), []);
  const live = useMemo(
    () => createLiveBackend({ relay, token: () => session.accessToken(), deviceSession: () => device.session(), me, deviceId }),
    [relay, device, me, deviceId],
  );
  // What stays on this phone between launches (the outbox, cached threads and Spaces) is sealed with a key in the
  // Keychain. A sample build keeps nothing unless asked to, so made up data is never mistaken for saved data.
  const store = useMemo(
    () => createSealedStore({ namespace: sample ? "sample" : "live", sealer: sample && !config.samplePersist ? null : undefined }),
    [sample],
  );

  // The sample build's "pretend offline" switch (Settings) is kept across launches, so the restart checks can use it.
  useEffect(() => {
    if (ready) demo.sample?.setOffline(sampleOffline);
  }, [ready, sampleOffline, demo]);

  // The enrollment check runs once per sign-in.
  useEffect(() => {
    if (!signedIn || sample) return;
    let alive = true;
    (async () => {
      accessCache.current = await session.accessToken().catch(() => "");
      const s = (await device.session()) ? ({ state: "enrolled" } as EnrollState) : await device.enroll();
      if (alive) setEnroll(s);
    })();
    return () => {
      alive = false;
    };
  }, [signedIn, sample, device]);

  const signIn = useCallback(async () => {
    setSigningIn(true);
    setSignInError(null);
    try {
      await session.signIn();
    } catch (e) {
      const err = e as { cancelled?: boolean; message?: string };
      if (!err.cancelled) setSignInError(err.message ?? "Sign-in did not finish.");
    } finally {
      setSigningIn(false);
    }
  }, []);
  const signOut = useCallback(async () => {
    setEnroll(null);
    clearThreadMemory();
    // Nothing of this account stays on the phone: the sealed copies go and so does their key.
    await store.clear();
    await session.signOut();
  }, [store]);
  const recheckEnrollment = useCallback(async () => setEnroll(await device.check()), [device]);

  let stage: Stage = "booting";
  let backend: Backend | null = null;
  if (ready && signedIn !== null) {
    if (sample) {
      stage = "ready";
      backend = demo;
    } else if (!signedIn) stage = "signed-out";
    else if (!enroll) stage = "booting";
    else if (enroll.state === "enrolled") {
      stage = "ready";
      backend = live;
    } else stage = "enrolling";
  }
  const value: AppState = {
    stage,
    backend,
    enroll,
    account: signedIn ? session.account() : null,
    deviceId,
    signIn,
    signOut,
    recheckEnrollment,
    signInError,
    signingIn,
  };
  return (
    <Ctx.Provider value={value}>
      {backend ? (
        <BackendProvider backend={backend}>
          <StoresProvider store={store} me={me}>
            {children}
          </StoresProvider>
        </BackendProvider>
      ) : (
        children
      )}
    </Ctx.Provider>
  );
}

const accessCache = { current: "" };
