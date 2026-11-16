// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Jest setup: native modules the screens touch, and the core the way the app gets it on a phone
// (the Rust library cannot load in Jest, so the tests replay the answers the core is checked against
// in core/tests/fixtures.rs).
import "react-native-gesture-handler/jestSetup";

// Reanimated's own mock needs the native worklets runtime; the screens use four things from it.
jest.mock("react-native-reanimated", () => {
  const { View } = require("react-native");
  return {
    __esModule: true,
    default: { View, createAnimatedComponent: (c: unknown) => c },
    FadeIn: { duration: () => ({}) },
    useSharedValue: (v: unknown) => ({ value: v }),
    useAnimatedStyle: (f: () => unknown) => f(),
  };
});
jest.mock("react-native-gesture-handler/ReanimatedSwipeable", () => {
  const React = require("react");
  return { __esModule: true, default: React.forwardRef(({ children }: { children: unknown }, _ref: unknown) => children) };
});
jest.mock("react-native-keyboard-controller", () => require("react-native-keyboard-controller/jest"));
jest.mock("expo-haptics", () => ({
  selectionAsync: jest.fn(async () => {}),
  impactAsync: jest.fn(async () => {}),
  notificationAsync: jest.fn(async () => {}),
  ImpactFeedbackStyle: { Light: 0, Medium: 1 },
  NotificationFeedbackType: { Success: 0, Warning: 1 },
}));
jest.mock("expo-secure-store", () => {
  const m = new Map<string, string>();
  return {
    getItemAsync: jest.fn(async (k: string) => m.get(k) ?? null),
    setItemAsync: jest.fn(async (k: string, v: string) => void m.set(k, v)),
    deleteItemAsync: jest.fn(async (k: string) => void m.delete(k)),
    AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY: 1,
    WHEN_UNLOCKED_THIS_DEVICE_ONLY: 2,
  };
});
jest.mock("@react-native-async-storage/async-storage", () => require("@react-native-async-storage/async-storage/jest/async-storage-mock"));
jest.mock("react-native-webview", () => ({ WebView: "WebView" }));
jest.mock("expo-local-authentication", () => ({
  SecurityLevel: { NONE: 0, SECRET: 1, BIOMETRIC_WEAK: 2, BIOMETRIC_STRONG: 3 },
  getEnrolledLevelAsync: jest.fn(async () => 3),
  authenticateAsync: jest.fn(async () => ({ success: true })),
}));
jest.mock("expo-clipboard", () => ({ setStringAsync: jest.fn(async () => true) }));
jest.mock("expo-notifications", () => ({
  setNotificationHandler: jest.fn(),
  setNotificationCategoryAsync: jest.fn(async () => {}),
  getPermissionsAsync: jest.fn(async () => ({ granted: false, canAskAgain: true })),
  requestPermissionsAsync: jest.fn(async () => ({ granted: true })),
  scheduleNotificationAsync: jest.fn(async () => "id"),
  addNotificationResponseReceivedListener: jest.fn(() => ({ remove: jest.fn() })),
  getLastNotificationResponseAsync: jest.fn(async () => null),
  getDevicePushTokenAsync: jest.fn(async () => {
    throw new Error("no push token on this build");
  }),
  setNotificationChannelAsync: jest.fn(async () => {}),
  AndroidImportance: { HIGH: 4 },
}));

require("@shopify/flash-list/jestSetup");
jest.mock("react-native-safe-area-context", () => require("react-native-safe-area-context/jest/mock").default);
