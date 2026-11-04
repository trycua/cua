// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { KeyboardProvider } from "react-native-keyboard-controller";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { AppProvider, useApp } from "../src/state/app";
import { useTheme, ThemeProvider } from "../src/theme/theme";
import { useNotifier } from "../src/notifications/useNotifier";

function Notifier() {
  useNotifier();
  return null;
}

function Shell() {
  const { c, scheme } = useTheme();
  const { stage } = useApp();
  return (
    <>
      <StatusBar style={scheme === "dark" ? "light" : "dark"} />
      {stage === "ready" ? <Notifier /> : null}
      <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: c.background } }}>
        <Stack.Screen name="index" />
        <Stack.Screen name="space/[id]" />
        <Stack.Screen name="stream/[id]" options={{ presentation: "fullScreenModal", animation: "fade" }} />
        <Stack.Screen name="settings" options={{ presentation: "modal" }} />
        <Stack.Screen name="new" options={{ presentation: "modal" }} />
        <Stack.Screen name="diff/[id]" />
      </Stack>
    </>
  );
}

export default function Root() {
  return (
    <GestureHandlerRootView style={{ flex: 1 }}>
      <SafeAreaProvider>
        <KeyboardProvider>
          <ThemeProvider>
            <AppProvider>
              <Shell />
            </AppProvider>
          </ThemeProvider>
        </KeyboardProvider>
      </SafeAreaProvider>
    </GestureHandlerRootView>
  );
}
