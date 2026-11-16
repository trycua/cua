// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useRef, useState } from "react";
import { Pressable, View } from "react-native";

import { askUnlock, useLock } from "../../auth/lock";
import { useTheme } from "../../theme/theme";
import { Text } from "../../ui/Text";

/** Asks Face ID or the passcode as soon as it shows, and again from its button. Opens by itself where nothing can be asked. */
export function LockScreen({ title, reason }: { title: string; reason: string }) {
  const { c } = useTheme();
  const [cancelled, setCancelled] = useState(false);
  const asking = useRef(false);
  const ask = useCallback(async () => {
    if (asking.current) return;
    asking.current = true;
    setCancelled(false);
    const r = await askUnlock(reason);
    asking.current = false;
    if (r === "cancelled") setCancelled(true);
    else useLock.getState().markUnlocked(Date.now());
  }, [reason]);
  useEffect(() => {
    void ask();
  }, [ask]);
  return (
    <View style={{ flex: 1, alignItems: "center", justifyContent: "center", gap: 14, padding: 32, backgroundColor: c.background }} testID="lock-screen">
      <Text variant="title" accessibilityRole="header">
        {title}
      </Text>
      {cancelled ? (
        <Text color="muted" style={{ textAlign: "center" }}>
          Use Face ID or your passcode to open it.
        </Text>
      ) : null}
      <Pressable
        onPress={() => void ask()}
        accessibilityRole="button"
        accessibilityLabel="Unlock"
        style={{ paddingVertical: 12, paddingHorizontal: 28, borderRadius: 12, backgroundColor: c.brand }}
      >
        <Text weight="600" color="inverse">
          Unlock
        </Text>
      </Pressable>
    </View>
  );
}
