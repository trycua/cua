// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react-native";
import * as LocalAuthentication from "expo-local-authentication";
import { Text } from "react-native";

import { ensureUnlocked, needsUnlock, RELOCK_AFTER_MS, useLock, VIEWER_FRESH_MS } from "../src/auth/lock";
import { Gated } from "../src/features/auth/Gated";
import { usePrefs } from "../src/state/prefs";
import { ThemeProvider } from "../src/theme/theme";

let mockSample = false;
jest.mock("../src/state/app", () => ({ useApp: () => ({ stage: "ready", backend: { capabilities: { sample: mockSample } } }) }));

const authenticate = LocalAuthentication.authenticateAsync as jest.Mock;
const level = LocalAuthentication.getEnrolledLevelAsync as jest.Mock;

const app = async () =>
  await render(
    <ThemeProvider force="light">
      <Gated>
        <Text>the Spaces</Text>
      </Gated>
    </ThemeProvider>,
  );

beforeEach(() => {
  mockSample = false;
  authenticate.mockReset().mockResolvedValue({ success: true });
  level.mockReset().mockResolvedValue(3);
  useLock.setState({ unlockedAt: null, leftAt: null });
  usePrefs.setState({ requireUnlock: true });
});

describe("the phone's lock", () => {
  it("decides from what is required, the last unlock and how old a unlock may be", () => {
    expect(needsUnlock({ required: false, unlockedAt: null, now: 10 })).toBe(false);
    expect(needsUnlock({ required: true, unlockedAt: null, now: 10 })).toBe(true);
    expect(needsUnlock({ required: true, unlockedAt: 5, now: 10 })).toBe(false);
    expect(needsUnlock({ required: true, unlockedAt: 0, now: VIEWER_FRESH_MS + 1, maxAgeMs: VIEWER_FRESH_MS })).toBe(true);
    expect(needsUnlock({ required: true, unlockedAt: 0, now: VIEWER_FRESH_MS, maxAgeMs: VIEWER_FRESH_MS })).toBe(false);
  });

  it("locks again after a minute away, not before", () => {
    useLock.getState().markUnlocked(1_000);
    useLock.getState().left(2_000);
    useLock.getState().returned(2_000 + RELOCK_AFTER_MS);
    expect(useLock.getState().unlockedAt).toBe(1_000);
    useLock.getState().left(100_000);
    useLock.getState().returned(100_000 + RELOCK_AFTER_MS + 1);
    expect(useLock.getState().unlockedAt).toBeNull();
  });

  it("asks Face ID or the passcode when the app opens, and shows the app after", async () => {
    let answer!: (v: { success: boolean }) => void;
    authenticate.mockReturnValueOnce(new Promise((r) => (answer = r)));
    await app();
    expect(screen.queryByText("the Spaces")).toBeNull();
    expect(screen.getByText("Cua Spaces is locked")).toBeTruthy();
    await act(async () => answer({ success: true }));
    expect(await screen.findByText("the Spaces")).toBeTruthy();
    expect(authenticate).toHaveBeenCalledTimes(1);
    expect(authenticate.mock.calls[0]![0]).toMatchObject({ promptMessage: "Open Cua Spaces", fallbackLabel: "Use passcode" });
  });

  it("stays locked when it is cancelled, says how to open it, and asks again from the button", async () => {
    authenticate.mockResolvedValueOnce({ success: false, error: "user_cancel" });
    await app();
    expect(await screen.findByText("Use Face ID or your passcode to open it.")).toBeTruthy();
    expect(screen.queryByText("the Spaces")).toBeNull();
    fireEvent.press(screen.getByLabelText("Unlock"));
    expect(await screen.findByText("the Spaces")).toBeTruthy();
    expect(authenticate).toHaveBeenCalledTimes(2);
  });

  it("opens by itself on a phone with no passcode, and does not ask", async () => {
    level.mockResolvedValue(0);
    await app();
    expect(await screen.findByText("the Spaces")).toBeTruthy();
    expect(authenticate).not.toHaveBeenCalled();
  });

  it("is not asked when it is turned off", async () => {
    usePrefs.setState({ requireUnlock: false });
    await app();
    expect(screen.getByText("the Spaces")).toBeTruthy();
    expect(authenticate).not.toHaveBeenCalled();
  });

  it("is not asked in a sample build", async () => {
    mockSample = true;
    await app();
    expect(screen.getByText("the Spaces")).toBeTruthy();
    expect(authenticate).not.toHaveBeenCalled();
  });

  it("locks the open app again when it comes back after a minute", async () => {
    useLock.getState().markUnlocked(Date.now());
    await app();
    expect(screen.getByText("the Spaces")).toBeTruthy();
    authenticate.mockResolvedValue({ success: false, error: "user_cancel" });
    await act(async () => {
      useLock.getState().left(Date.now() - RELOCK_AFTER_MS - 5);
      useLock.getState().returned(Date.now());
    });
    await waitFor(() => expect(screen.getByText("Cua Spaces is locked")).toBeTruthy());
    expect(screen.queryByText("the Spaces")).toBeNull();
  });
});

describe("the unlock before something that matters", () => {
  it("asks only when it is required and the last unlock is old, and says no when it is cancelled", async () => {
    // Not required: never asks.
    expect(await ensureUnlocked("Answer your agent", false)).toBe(true);
    expect(authenticate).not.toHaveBeenCalled();
    // Required, unlocked a moment ago: no second prompt.
    useLock.getState().markUnlocked(Date.now());
    expect(await ensureUnlocked("Answer your agent", true)).toBe(true);
    expect(authenticate).not.toHaveBeenCalled();
    // Required, unlocked long ago: asks, and a yes counts as a fresh unlock.
    useLock.getState().markUnlocked(Date.now() - VIEWER_FRESH_MS - 1000);
    expect(await ensureUnlocked("Take control of this computer", true)).toBe(true);
    expect(authenticate.mock.calls[0]![0]).toMatchObject({ promptMessage: "Take control of this computer" });
    expect(await ensureUnlocked("Take control of this computer", true)).toBe(true);
    expect(authenticate).toHaveBeenCalledTimes(1);
    // Cancelled: not allowed, and the old unlock stays old.
    useLock.getState().markUnlocked(Date.now() - VIEWER_FRESH_MS - 1000);
    authenticate.mockResolvedValueOnce({ success: false, error: "user_cancel" });
    expect(await ensureUnlocked("Sign out of Cua Spaces", true)).toBe(false);
    expect(needsUnlock({ required: true, unlockedAt: useLock.getState().unlockedAt, now: Date.now(), maxAgeMs: VIEWER_FRESH_MS })).toBe(true);
    // No passcode on the phone: nothing can be asked, so it goes through.
    level.mockResolvedValue(0);
    useLock.getState().markUnlocked(Date.now() - VIEWER_FRESH_MS - 1000);
    expect(await ensureUnlocked("Sign out of Cua Spaces", true)).toBe(true);
  });
});
