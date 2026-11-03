// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, type ReactNode } from "react";
import { ActivityIndicator, AppState, View } from "react-native";

import { needsUnlock, useLock } from "../../auth/lock";
import { useApp } from "../../state/app";
import { usePrefs } from "../../state/prefs";
import { useTheme } from "../../theme/theme";
import { EnrollScreen } from "./EnrollScreen";
import { LockScreen } from "./LockScreen";
import { SignInScreen } from "./SignInScreen";

/** A screen that needs the account: sign-in, the new-phone approval, or a spinner stand in until it is ready. */
export function Gated({ children }: { children: ReactNode }) {
  const { stage, backend } = useApp();
  const { c } = useTheme();
  const required = usePrefs((p) => p.requireUnlock) && !backend?.capabilities.sample;
  const unlockedAt = useLock((s) => s.unlockedAt);
  // Away for more than a minute: the app locks again.
  useEffect(() => {
    const sub = AppState.addEventListener("change", (s) => {
      if (s === "active") useLock.getState().returned(Date.now());
      else useLock.getState().left(Date.now());
    });
    return () => sub.remove();
  }, []);
  if (stage === "ready") {
    return needsUnlock({ required, unlockedAt, now: Date.now() }) ? <LockScreen title="Cua Spaces is locked" reason="Open Cua Spaces" /> : <>{children}</>;
  }
  if (stage === "signed-out") return <SignInScreen />;
  if (stage === "enrolling") return <EnrollScreen />;
  return (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center", backgroundColor: c.background }}>
      <ActivityIndicator color={c.mutedForeground} />
    </View>
  );
}
