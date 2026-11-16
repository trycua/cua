// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Pressable, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useApp } from "../../state/app";
import { useTheme } from "../../theme/theme";
import { Text } from "../../ui/Text";

/** The relay wants a second factor for a new phone: an enrolled machine approves a one-time code. */
export function EnrollScreen() {
  const { c, mono } = useTheme();
  const insets = useSafeAreaInsets();
  const { enroll, recheckEnrollment, signOut } = useApp();
  const refused = enroll?.state === "refused";
  return (
    <View style={{ flex: 1, backgroundColor: c.background, padding: 24, paddingTop: insets.top + 40, paddingBottom: insets.bottom + 16, gap: 14 }}>
      <Text variant="title" accessibilityRole="header">
        {refused ? "This phone could not be added" : "Approve this phone"}
      </Text>
      {enroll?.state === "pending" ? (
        <>
          <Text color="muted">
            cua.ai asks one of your computers to confirm a new phone. Approve it in Cua Spaces, or run this in a terminal on a computer that is already signed
            in:
          </Text>
          <View style={{ backgroundColor: c.muted, borderRadius: 10, padding: 12 }}>
            <Text variant="mono" selectable style={{ fontFamily: mono }}>
              cua devices approve {enroll.code ?? "<code>"}
            </Text>
          </View>
          {enroll.code ? (
            <Text
              variant="largeTitle"
              style={{ letterSpacing: 4, textAlign: "center", fontFamily: mono }}
              accessibilityLabel={`Code ${enroll.code.split("").join(" ")}`}
            >
              {enroll.code}
            </Text>
          ) : null}
          <Text variant="footnote" color="muted">
            The code works for 10 minutes.
          </Text>
        </>
      ) : (
        <Text color="destructive">{enroll?.state === "refused" ? enroll.message : ""}</Text>
      )}
      <View style={{ flex: 1 }} />
      <Pressable
        onPress={recheckEnrollment}
        accessibilityRole="button"
        style={{ height: 50, borderRadius: 14, backgroundColor: c.primary, alignItems: "center", justifyContent: "center" }}
      >
        <Text variant="headline" style={{ color: c.primaryForeground }}>
          I approved it, check again
        </Text>
      </Pressable>
      <Pressable onPress={signOut} accessibilityRole="button" style={{ alignItems: "center", padding: 10 }}>
        <Text color="muted">Sign out</Text>
      </Pressable>
    </View>
  );
}
