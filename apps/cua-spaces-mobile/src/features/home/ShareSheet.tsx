// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";
import { Modal, Pressable, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { SpaceSummary } from "../../data/types";
import { useBackend } from "../../state/backend";
import { useTheme } from "../../theme/theme";
import { Text } from "../../ui/Text";

/** Share a Space you own: by email, as an editor or a viewer (the relay's allowlist). */
export function ShareSheet({ space, onClose }: { space: SpaceSummary | null; onClose(): void }) {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const backend = useBackend();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"editor" | "viewer">("editor");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const reset = () => {
    setEmail("");
    setError(null);
    setDone(false);
    onClose();
  };
  const valid = /^\S+@\S+\.\S+$/.test(email.trim());
  return (
    <Modal visible={!!space} transparent animationType="slide" onRequestClose={reset}>
      <Pressable style={{ flex: 1, backgroundColor: c.overlay }} onPress={reset} accessibilityLabel="Close" />
      <View style={{ backgroundColor: c.card, borderTopLeftRadius: 16, borderTopRightRadius: 16, padding: 16, paddingBottom: insets.bottom + 16, gap: 12 }}>
        <Text variant="headline">Share {space?.name}</Text>
        {done ? (
          <Text color="muted">
            Shared with {email.trim()}
            {backend.capabilities.sample ? " (sample data: nothing was sent)" : ""}.
          </Text>
        ) : (
          <>
            <TextInput
              value={email}
              onChangeText={setEmail}
              placeholder="Email address"
              placeholderTextColor={c.mutedForeground}
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType="email-address"
              accessibilityLabel="Email address"
              style={{ height: 44, borderRadius: 10, backgroundColor: c.tile, paddingHorizontal: 12, color: c.foreground, fontSize: 16 }}
            />
            <View style={{ flexDirection: "row", gap: 8 }}>
              {(["editor", "viewer"] as const).map((r) => (
                <Pressable
                  key={r}
                  onPress={() => setRole(r)}
                  accessibilityRole="radio"
                  accessibilityState={{ selected: role === r }}
                  style={{
                    flex: 1,
                    height: 40,
                    borderRadius: 10,
                    alignItems: "center",
                    justifyContent: "center",
                    backgroundColor: role === r ? c.primary : c.tile,
                  }}
                >
                  <Text weight="500" style={{ color: role === r ? c.primaryForeground : c.foreground }}>
                    {r === "editor" ? "Can use" : "Can watch"}
                  </Text>
                </Pressable>
              ))}
            </View>
            {error ? (
              <Text variant="footnote" color="destructive">
                {error}
              </Text>
            ) : null}
            <Pressable
              disabled={!valid || busy}
              onPress={async () => {
                setBusy(true);
                setError(null);
                try {
                  await backend.share(space!.id, email.trim(), role);
                  setDone(true);
                } catch (e) {
                  setError(e instanceof Error ? e.message : String(e));
                } finally {
                  setBusy(false);
                }
              }}
              accessibilityRole="button"
              style={{
                height: 46,
                borderRadius: 12,
                alignItems: "center",
                justifyContent: "center",
                backgroundColor: c.primary,
                opacity: valid && !busy ? 1 : 0.4,
              }}
            >
              <Text weight="600" style={{ color: c.primaryForeground }}>
                Share
              </Text>
            </Pressable>
          </>
        )}
      </View>
    </Modal>
  );
}
