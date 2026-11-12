// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Image } from "expo-image";
import { ActivityIndicator, Pressable, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { useApp } from "../../state/app";
import { useTheme } from "../../theme/theme";
import { CUA_APP_ICON } from "../../ui/Avatar";
import { Text } from "../../ui/Text";

export function SignInScreen() {
  const { c } = useTheme();
  const insets = useSafeAreaInsets();
  const { signIn, signingIn, signInError } = useApp();
  return (
    <View style={{ flex: 1, backgroundColor: c.background, paddingTop: insets.top, paddingBottom: insets.bottom + 16, paddingHorizontal: 24 }}>
      <View style={{ flex: 1, alignItems: "center", justifyContent: "center", gap: 14 }}>
        <Image source={CUA_APP_ICON} style={{ width: 88, height: 88, borderRadius: 20 }} accessibilityLabel="Cua Spaces" />
        <Text variant="largeTitle" accessibilityRole="header">
          Cua Spaces
        </Text>
        <Text variant="body" color="muted" style={{ textAlign: "center", maxWidth: 320 }}>
          Talk to your agents, approve what they ask, and watch their screens from anywhere.
        </Text>
      </View>
      {signInError ? (
        <Text variant="subhead" color="destructive" style={{ textAlign: "center", marginBottom: 12 }} accessibilityLiveRegion="polite">
          {signInError}
        </Text>
      ) : null}
      <Pressable
        onPress={signIn}
        disabled={signingIn}
        accessibilityRole="button"
        accessibilityLabel="Sign in with your cua.ai account"
        testID="sign-in"
        style={({ pressed }) => ({
          height: 52,
          borderRadius: 14,
          backgroundColor: c.primary,
          alignItems: "center",
          justifyContent: "center",
          opacity: pressed || signingIn ? 0.7 : 1,
          flexDirection: "row",
          gap: 8,
        })}
      >
        {signingIn ? <ActivityIndicator color={c.primaryForeground} /> : null}
        <Text variant="headline" style={{ color: c.primaryForeground }}>
          Sign in with cua.ai
        </Text>
      </Pressable>
      <Text variant="footnote" color="muted" style={{ textAlign: "center", marginTop: 12 }}>
        You sign in on cua.ai in a secure sheet. Your password never reaches this app.
      </Text>
    </View>
  );
}
